//! The native half of lower41/42's paused command/refresh/callback protocol.
//! The controls bank and FES notification own the other two halves.
use super::*;
use std::collections::BTreeMap;
use anyhow::{Context, Result as HostResult, bail};
use cs2_runtime::graph_serde::{Deserialize, Serialize};
use cs2_runtime::object_graph as wire;
use crate::fes_controls::FesControlStore;
use crate::fes_driver::FesDriverInput;
use crate::fes_tasks::FesTaskStore;
use super::super::NativeTaskStore;

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Notification { owner: u32, command: TaskCommand, event: FesDriverInput }
#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Pending {
    command: TaskCommand, input: Option<Vec<u8>>, output: u32, owner: u32,
    notification: Option<Notification>, commands: Vec<TaskCommand>,
    command_cursor: usize, refresh: bool, stage: u8,
}
#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Snapshot { version: u32, pending: BTreeMap<u32, Pending> }
pub(crate) struct PreparedLowerCommands(BTreeMap<u32, LowerPending>);

impl NativeTaskStore {
    pub(crate) fn capture_lower_command_state(&self, controls: &FesControlStore,
        fes: &FesTaskStore) -> HostResult<Vec<u8>> {
        validate(&self.lower_commands, &self.checkpoint_task_kinds(), controls, fes)?;
        Ok(wire::encode_component(&Snapshot { version: 1,
            pending: self.lower_commands.iter().map(|(&id, p)| (id, Pending {
                command: p.command, input: p.input.clone(), output: p.output, owner: p.owner,
                notification: p.notification.as_ref().map(|n| Notification {
                    owner: n.owner, command: n.command, event: n.event.clone(),
                }), commands: p.commands.clone(), command_cursor: p.command_cursor,
                refresh: p.refresh, stage: p.stage,
            })).collect(),
        })?)
    }

    pub(crate) fn prepare_lower_command_state(&self, bytes: &[u8], controls: &FesControlStore,
        fes: &FesTaskStore) -> HostResult<PreparedLowerCommands> {
        let saved: Snapshot = wire::decode_component(bytes)?;
        if saved.version != 1 || saved.pending.len() > 1_000_000 {
            bail!("lower command checkpoint version/count is invalid");
        }
        let pending = saved.pending.into_iter().map(|(id, p)| (id, LowerPending {
            command: p.command, input: p.input, output: p.output, owner: p.owner,
            notification: p.notification.map(|n| ControlNotification {
                owner: n.owner, command: n.command, event: n.event,
            }), commands: p.commands, command_cursor: p.command_cursor,
            refresh: p.refresh, stage: p.stage,
        })).collect();
        validate(&pending, &self.checkpoint_task_kinds(), controls, fes)?;
        Ok(PreparedLowerCommands(pending))
    }

    pub(crate) fn commit_lower_command_state(&mut self, prepared: PreparedLowerCommands) {
        self.lower_commands = prepared.0;
    }
}

fn validate(pending: &BTreeMap<u32, LowerPending>, kinds: &BTreeMap<u32, u32>,
    controls: &FesControlStore, fes: &FesTaskStore) -> HostResult<()> {
    if pending.len() > 1_000_000 { bail!("lower command checkpoint exceeds task bounds"); }
    let banks = controls.checkpoint_bank_identities();
    let mut bytes = 0usize;
    for (&id, p) in pending {
        if !matches!(kinds.get(&id), Some(41 | 42))
            || controls.checkpoint_lower_owner(id) != Some(p.owner)
            || kinds.get(&p.owner) != Some(&34) || p.command.target as u32 != id
            || !matches!(p.command.code, 6 | 9 | 10 | 64 | 65 | 365..=382)
            || p.stage > 3 || p.commands.len() > 4096 || p.command_cursor > p.commands.len()
            || p.stage > 0 && p.command_cursor != p.commands.len() {
            bail!("lower command checkpoint lost its actual bank, task or cursor");
        }
        let required = match p.command.code { 365 | 374 => Some(0x124), 368 => Some(8),
            376 => Some(12), _ => None };
        if required.is_some_and(|length| p.input.as_ref().is_none_or(|data| data.len() < length))
            || matches!(p.command.code, 6 | 9 | 10 | 64 | 65) && p.input.is_some() {
            bail!("lower checkpoint command lost its retained input record");
        }
        bytes = bytes.checked_add(p.input.as_ref().map_or(0, Vec::len))
            .context("lower command checkpoint byte accounting overflow")?;
        if let Some(n) = &p.notification {
            let target = n.event.target.as_ref().context("lower notification checkpoint has no bank target")?;
            if n.owner != p.owner || n.command.target as u32 != p.owner
                || n.command.sender as u32 != id || !matches!(n.command.code, 27 | 28)
                || target.value as u32 != id || !banks.contains(&(p.owner, target.name.clone(), target.bank))
                || n.event.event.as_ref().is_none_or(|event| event.is_empty() || event.len() > 259) {
                bail!("lower notification checkpoint packet differs from its actual FES bank");
            }
            fes.validate_checkpoint_lower_notification(p.owner, n.command, &n.event)?;
        }
    }
    if bytes > wire::MAX_GRAPH_BYTES { bail!("lower command checkpoint exceeds graph byte bound"); }
    Ok(())
}
