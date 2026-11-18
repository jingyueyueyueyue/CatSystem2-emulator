//! Single-statement media alias dispatcher. Its native boolean is independent
//! of host budget, handler completion and an interpreter block boundary.
use anyhow::{Result, bail};
use cs2_script::{ExpressionState, FesEvent, FesStep, scene::FesControlPosition};

use super::{FesDriverCommand, FesDriverEffect, FesDriverTarget,
    FesMediaNotificationScope, FesScriptDriver};

#[derive(Clone, Debug)]
pub(crate) enum FesStatementStep {
    Effect(FesDriverEffect),
    Yield,
    Returned { complete: bool, statement: u32, wait_output: u32 },
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct FesStatementReturn {
    pub complete: bool,
    pub statement: u32,
    pub wait_output: u32,
    header: Vec<u8>,
    target: FesDriverTarget,
    initial_statement: u32,
}

impl FesStatementReturn {
    pub(super) fn validate_checkpoint(&self) -> Result<()> {
        if self.header.len() > 63 || self.header.contains(&0)
            || self.target.name.len() > 31 || self.target.name.contains(&0) {
            bail!("media alias receipt lost its encoded header/object identity");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct MediaStatement {
    header: Vec<u8>,
    statement: u32,
    target: FesDriverTarget,
    saved_control: Option<FesControlPosition>,
    saved_notifications: Option<FesMediaNotificationScope>,
    pending_effect: Option<FesDriverEffect>,
    raw_result: Option<i32>,
    next_statement: u32,
    wait_output: u32,
    error: Option<String>,
}

impl MediaStatement {
    pub(super) fn validate_checkpoint(&self, driver: &FesScriptDriver) -> Result<()> {
        if let Some(control) = &self.saved_control {
            control.validate_execution_checkpoint(&driver.interpreter)?;
        }
        if let Some(scope) = &self.saved_notifications { scope.validate_checkpoint(driver)?; }
        if self.header.len() > 63 || self.target.name.len() > 31 {
            bail!("media statement checkpoint exceeds its original fields");
        }
        Ok(())
    }

    pub(super) fn checkpoint_external(&mut self, external: &cs2_script::SharedExternalStorage, bind: bool) -> Result<()> {
        if let Some(effect) = &mut self.pending_effect { effect.checkpoint_external(external, bind)?; }
        if let Some(scope) = &mut self.saved_notifications { scope.checkpoint_external(external, bind)?; }
        Ok(())
    }
}

impl FesScriptDriver {
    /// Call once per SoundMediaNotice::Alias iteration, then advance until a
    /// Returned result. Repeated begin while pending must use the same packet.
    pub(crate) fn begin_media_statement(
        &mut self, header: &[u8], statement: u32, target: FesDriverTarget,
    ) -> Result<()> {
        if self.media_statement_result.is_some() {
            bail!("consume the preceding media alias dispatcher result first");
        }
        if let Some(pending) = &self.media_statement {
            if pending.header != header || pending.statement != statement || pending.target != target {
                bail!("media alias statement continuation changed its invocation");
            }
            return Ok(());
        }
        if header.len() > 63 || header.contains(&0) || target.name.len() > 31
            || target.name.contains(&0)
        {
            bail!("media alias header/object exceeds its original encoded field");
        }
        self.media_statement = Some(MediaStatement {
            header: header.to_vec(), statement, target,
            saved_control: None, saved_notifications: None, pending_effect: None,
            raw_result: None, next_statement: statement,
            // Native2b5130 initializes this on every dispatch, including
            // attempts whose header lookup fails.
            wait_output: 0,
            error: None,
        });
        Ok(())
    }

    /// Apply a yielded Effect with the existing FES task platform before
    /// acknowledging it. A failed/suspended effect stays exactly as emitted.
    pub(crate) fn complete_media_statement_effect(&mut self) -> Result<()> {
        let pending = self.media_statement.as_mut()
            .ok_or_else(|| anyhow::anyhow!("no active media alias statement"))?;
        if pending.pending_effect.take().is_none() {
            bail!("media alias statement has no effect to acknowledge");
        }
        Ok(())
    }

    pub(crate) fn media_statement_effect_pending(&self) -> bool {
        self.media_statement.as_ref().is_some_and(|pending| pending.pending_effect.is_some())
    }

    /// Called by SOUND only after the outer task pump applied all real
    /// effects. Merely reading this receipt never dispatches a new statement.
    pub(crate) fn take_media_statement_result(&mut self) -> Option<FesStatementReturn> {
        self.media_statement_result.take()
    }

    pub(crate) fn advance_media_statement(
        &mut self, frame: u64, shared: &mut ExpressionState, budget: usize,
    ) -> Result<FesStatementStep> {
        let mut pending = self.media_statement.take()
            .ok_or_else(|| anyhow::anyhow!("begin a media alias statement before advancing"))?;
        let result = self.dispatch_media_statement_inner(&mut pending, frame, shared, budget);
        if let Err(cause) = &result {
            // A failed statement never leaves the caller's real handler
            // parked. Keep the error terminal for this invocation so retry
            // does not repeat any numeric/variable side effects.
            if let Some(control) = pending.saved_control.take() {
                self.interpreter.restore_control_position(control)?;
            }
            if let Some(scope) = pending.saved_notifications.take() {
                self.end_media_notification_scope(scope);
            }
            pending.error = Some(format!("{cause:#}"));
        }
        match &result {
            Ok(FesStatementStep::Returned { complete, statement, wait_output }) => {
                self.media_statement_result = Some(FesStatementReturn {
                    complete: *complete, statement: *statement, wait_output: *wait_output,
                    header: pending.header, target: pending.target, initial_statement: pending.statement,
                });
            }
            _ => self.media_statement = Some(pending),
        }
        result
    }

    pub(crate) fn checkpoint_media_alias(&self, name: &[u8], bank: i32,
        header: &[u8], statement: u32) -> bool {
        let target = |target: &FesDriverTarget| target.name == name && target.bank == bank;
        self.media_statement.as_ref().is_some_and(|work|
            work.header == header && work.statement == statement && target(&work.target))
            || self.media_statement_result.as_ref().is_some_and(|receipt|
                receipt.header == header && receipt.initial_statement == statement && target(&receipt.target))
    }

    pub(crate) fn checkpoint_statement_object_notice(&self, name: &[u8], bank: i32, event: &[u8]) -> bool {
        self.media_statement.as_ref().is_some_and(|work|
            work.saved_notifications.as_ref().is_some_and(|scope| scope.checkpoint_object_notice(name, bank, event))
                || work.pending_effect.as_ref().is_some_and(|effect| effect.checkpoint_object_notice(name, bank, event)))
    }

    fn dispatch_media_statement_inner(
        &mut self, pending: &mut MediaStatement, _frame: u64,
        shared: &mut ExpressionState, budget: usize,
    ) -> Result<FesStatementStep> {
        if let Some(error) = &pending.error { bail!("{error}"); }
        if let Some(effect) = &pending.pending_effect {
            return Ok(FesStatementStep::Effect(effect.clone()));
        }
        if budget == 0 { return Ok(FesStatementStep::Yield); }
        if pending.raw_result.is_none() {
            if self.interpreter.initialization_pending() {
                bail!("media alias statement requires completed declarations");
            }
            let original = self.interpreter.control_position();
            if !self.interpreter.enter_statement(&pending.header, pending.statement) {
                // The original dispatcher returns false at2c73e2; neither a
                // missing header nor an exhausted cursor is successful EOF.
                return Ok(FesStatementStep::Returned {
                    complete: false, statement: pending.statement, wait_output: 0,
                });
            }
            pending.saved_control = Some(original);
            pending.saved_notifications = Some(self.begin_media_notification_scope());
            let source = self.interpreter.program.block(&pending.header)
                .and_then(|block| block.lines.get((pending.statement-1) as usize))
                .ok_or_else(|| anyhow::anyhow!("selected media alias statement disappeared"))?
                .bytes.clone();
            let parameters = self.interpreter.program.command_parameters(&source, &shared.strings)?;
            let name = parameters.first().map_or(&[][..], Vec::as_slice);
            // A control transfer rewrites the caller's header buffer in the
            // original ABI. The current notice adapter exposes only a DWORD
            // cursor, so refusing it preserves the real boundary explicitly.
            if matches!(name, b"next" | b"call") {
                bail!("media alias next/call requires mutable header/call continuation");
            }
            let object_command = parameters.get(1).is_some_and(|verb|
                matches!(verb.as_slice(), b"disp" | b"stop" | b"play" | b"pos" | b"keypos" | b"attach" | b"pos2"));
            let focus_command = name == b"lockactive" || parameters.get(1).is_some_and(|verb| verb == b"noact");
            if !object_command && !focus_command && !name.is_empty()
                && !matches!(name, b"if" | b"endif" | b"break" | b"wait" | b"exit")
                && !matches!(source.first(), Some(b'$' | b'\\'))
            {
                bail!("media alias single-statement return ABI is not recovered for this command");
            }
            if name == b"wait" && parameters.get(1)
                .is_some_and(|value| !statement_number(value))
            {
                bail!("media alias named wait requires its exact owner wait-field adapter");
            }
            std::mem::swap(&mut self.expression.shared, &mut shared.shared);
            std::mem::swap(&mut self.expression.strings, &mut shared.strings);
            self.expression.current = pending.target.bank;
            let step = self.interpreter.run(&mut self.expression, 1);
            let context = self.interpreter.context(&self.expression);
            std::mem::swap(&mut self.expression.strings, &mut shared.strings);
            std::mem::swap(&mut self.expression.shared, &mut shared.shared);
            let step = step?;
            if matches!(name, b"if" | b"break") {
                pending.next_statement = u32::try_from(self.interpreter.position().1)
                    .ok().and_then(|cursor| cursor.checked_add(1))
                    .ok_or_else(|| anyhow::anyhow!("media alias statement cursor overflow"))?;
                pending.wait_output = 1;
            }
            let event = if step == FesStep::Event { self.interpreter.pending().cloned() } else { None };
            let (raw, effect) = match event {
                None => (i32::from(name.is_empty()), None),
                Some(FesEvent::Wait(count)) => (count.max(1), None),
                Some(FesEvent::WaitFrame) => (-1, None),
                Some(FesEvent::Exit(value)) => (-3, Some(FesDriverEffect::Exit(value))),
                Some(FesEvent::Command(line)) if object_command || focus_command => {
                    let dispatch = self.adapter.dispatch_pending(&self.interpreter)?;
                    let _ = dispatch;
                    let (_, mut arguments) = self.adapter.take_command_with_parameters()
                        .ok_or_else(|| anyhow::anyhow!("alias object command lost its parameters"))?;
                    let mut arguments = arguments.take()
                        .ok_or_else(|| anyhow::anyhow!("alias object command has no structured arguments"))?;
                    if arguments.first().is_some_and(|name| name == b"@") {
                        arguments[0] = pending.target.parameter();
                    }
                    let command = FesDriverCommand {
                        offset: line.offset, source: line.bytes, parameters: arguments,
                        context, object_event: Some(pending.target.clone()),
                    };
                    if focus_command { (self.alias_focus_command(&command, shared)?, None) }
                    else { (0, Some(FesDriverEffect::Command(command))) }
                }
                Some(_) => bail!("media alias statement reached an unrecovered dispatcher return"),
            };
            self.interpreter.resume();
            pending.raw_result = Some(raw);
            pending.pending_effect = effect;
            if let Some(effect) = &pending.pending_effect {
                return Ok(FesStatementStep::Effect(effect.clone()));
            }
        }
        self.interpreter.restore_control_position(pending.saved_control.take()
            .ok_or_else(|| anyhow::anyhow!("media alias lost its original control position"))?)?;
        self.end_media_notification_scope(pending.saved_notifications.take()
            .ok_or_else(|| anyhow::anyhow!("media alias lost its caller notification scope"))?);
        Ok(FesStatementStep::Returned {
            complete: pending.raw_result.unwrap() != 0,
            statement: pending.next_statement,
            wait_output: pending.wait_output,
        })
    }
}

fn statement_number(input: &[u8]) -> bool {
    input.first().is_some_and(|byte| byte.is_ascii_digit()
        || matches!(byte, b'$' | b'\\' | b'(' | b'@' | b'-' | b'+' | b'!' | b'~'))
}
