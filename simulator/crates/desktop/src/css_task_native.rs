//! SHA-bound CSS576 wrappers and concrete, independently verified receivers.
//! A wrapper's boolean is not evidence that a CSS body was restored.
//! Evidence: css_task_576_F_contracts / css_task_receiver_F_20261117 reports.
use cs2_runtime::{TaskCommand, TaskCommandOutcome};
use cs2_script::{NativeReturn, Result};

use crate::{LoggingHost, native_state_error, read_u32};

// (exact image, restore message, independently recovered Top/ordinary
// output-preserving paths). The last column is NOT a whole-class support bit:
// css_receiver still checks the actual target class before routing.
const COMMANDS: &[(&str, u32, bool)] = &[
    ("659d5f069f3c97f69a61e12449cf1510b45e48a10649b9ac6977c2b93d6d28ad", 366, false),
    ("a9f8b08713aac278671853b2293a13b2623a122ede9341cb921901b09601cb52", 372, false),
    ("f6af7437475b8c066be973245aa79688bea510ca1192264add4f1914ebee4c0e", 393, true),
    ("012c71635932c241cde4df986377516279a50b907166757e89ea7b86ff3f813f", 395, true),
    ("9f3a5ad0ed22074420685f1e2a3bc92436d68289c7bac854d5f7d7331dc3ef67", 389, true),
    ("72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8", 341, false),
    ("0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f", 426, true),
    ("c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc", 431, true),
    ("474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1", 421, true),
    ("63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976", 437, true),
    ("c47c9d883126f641ce07d92eb57b6ac9833fe7495996001c12c68c55b99974d3", 423, true),
    ("4ccb48ad4bb6bfb5a15a45b8e1f2c2e62072cfad9f1487f869561a473770ed94", 431, true),
    ("c48d383c335c5b283bb0c17bd9f14b2b8bfef4a2e3d635365157b1c69498df78", 421, true),
    ("c2af2ee9baf4b26a3c2cf8a1c867f15f92b54ad1ac5fac381858046a47818f6c", 407, true),
    ("ac226c8455eff72114d6b1b9929fae01ac49ba90b8ba579f049f179c4c6dd8d5", 400, true),
    ("3b359fc2d737d5ceed454d6b65e6e5b5e6dfb4a24cf88ee361df86e454203f18", 428, true),
    ("6f75ae3877e018de1982a68f1089c127870d895d748c6290dd2ee85cd8daa4de", 428, true),
    ("1f3288d5fd5408bee4d395faab708aca78c9eeaa6c172dc06dcce09c844ba9d5", 437, true),
];

pub(super) fn identified_pe_sha256(sha: &str) -> Option<&'static str> {
    COMMANDS.iter().find(|entry| entry.0 == sha).map(|entry| entry.0)
}

#[derive(Clone, Copy)]
enum Receiver {
    // The concrete Top/ordinary method and generic base both miss this
    // command. No CSS byte or output word is read/written on that path.
    PreserveOutput { propagate: bool },
    // The identified old kcTask3DView receivers have explicit output-zero leaves.
    RejectRestore,
}

impl LoggingHost<'_> {
    pub(super) fn css_task_sha(&self) -> Option<&'static str> {
        self.data_services.css_task_pe_sha256.or_else(||
            self.data_services.native_profile.map(|profile| profile.pe_sha256))
    }

    /// Campanella native83 sends61, which misses both the 3D332..376 switch
    /// and the generic2..28 switch. A generic plane fallback must not destroy
    /// this real owner or its graph/material/particle dependents.
    pub(super) fn css_owner_lifecycle_native(
        &mut self, index: u32, args: &[u8],
    ) -> Result<Option<NativeReturn>> {
        let sha = self.css_task_sha();
        let legacy_dae = sha == Some(super::css_model::SHA);
        if index != 83 || (!legacy_dae && sha != Some(super::particle_external::CAMPANELLA_SHA)) {
            return Ok(None);
        }
        let target = read_u32(args, 0)?;
        if target == 0 || !self.data_services.tasks.message_task_state(target)
            .is_some_and(|(kind, _, _)| kind == 42)
        {
            return Ok(None);
        }
        if legacy_dae && !self.data_services.css_models.owners.contains_key(&target) {
            return Ok(None);
        }
        let command = TaskCommand { target: target as i32, sender: self.current_task_id as i32,
            // 72c9 slot83=44a0f0 sends60. It does not destroy its DAE owner.
            // Campanella's independently recovered wrapper sends61.
            code: if legacy_dae { 60 } else { 61 }, data: [0; 3] };
        let mut buffers = super::native_tasks::OwnedTaskBuffers::new(None, None)?;
        let outcome = self.route_task_command_buffers(command, &mut buffers, &mut 0, 4096)?;
        Ok(Some(if outcome.is_some_and(|outcome| !outcome.done) {
            NativeReturn::Retry
        } else { NativeReturn::Continue }))
    }

    fn css_receiver(&self, target: u32, code: u32) -> Result<Option<Receiver>> {
        let Some(sha) = self.css_task_sha() else { return Ok(None); };
        let Some(&(_, expected, common_paths)) = COMMANDS.iter().find(|entry| entry.0 == sha)
            else { return Ok(None); };
        if code != expected { return Ok(None); }
        let Some((kind, deletion, _)) = self.data_services.tasks.message_task_state(target)
            else { return Ok(None); };
        if common_paths && matches!(kind, 0 | 12) {
            // Top's out-of-switch return consumes; ordinary/Base delegates
            // to the generic handler, which leaves this output untouched.
            return Ok(Some(Receiver::PreserveOutput { propagate: target != 0 }));
        }
        if matches!(sha, super::particle_external::CAMPANELLA_SHA | super::css_model::UTATE_SHA) && kind == 42 {
            if deletion != 0 {
                return Ok(Some(Receiver::PreserveOutput { propagate: false }));
            }
            // The live CPU graph/manager is allocated by the real task
            // constructor. Availability precedes configure/model load; an
            // empty graph is not an unavailable constructor. Never create a
            // manager here, or mistake a modern kind42 SCROLL2 for this class.
            let owner = if sha == super::css_model::UTATE_SHA {
                //Utate372 -> 41f7b1 explicitly writes0. It never decodes
                //a CSS material body; the independent binary KXS owner is
                //constructor evidence, not a Campanella/modern type alias.
                self.data_services.css_models.utate_models.contains_key(&target)
            } else {
                self.data_services.particle_external.canvases.contains_key(&target)
            };
            if !owner {
                return Err(native_state_error("CSS restore requires its identified live kcTask3DView owner"));
            }
            return Ok(Some(Receiver::RejectRestore));
        }
        Ok(None)
    }

    /// Shared by native576, typed buffers and raw69/70. These precise leaves
    /// intentionally do not dereference data1, including a zero-length body.
    pub(crate) fn receive_css_task_command(
        &mut self, command: TaskCommand, output: &mut u32,
    ) -> Result<Option<TaskCommandOutcome>> {
        let Some(receiver) = self.css_receiver(command.target as u32, command.code)?
            else { return Ok(None); };
        let propagate = match receiver {
            Receiver::PreserveOutput { propagate } => propagate,
            Receiver::RejectRestore => { *output = 0; false }
        };
        Ok(Some(TaskCommandOutcome { done: true, propagate }))
    }

    pub(crate) fn dispatch_css_task_message(
        &mut self, target: u32, packet: [u32; 4], queued: bool, output: &mut u32,
    ) -> Result<Option<TaskCommandOutcome>> {
        if self.css_receiver(target, packet[1])?.is_none() { return Ok(None); }
        let command = TaskCommand { target: target as i32, sender: packet[0] as i32,
            code: packet[1], data: [packet[2], packet[3], 0] };
        if queued {
            let buffers = super::native_tasks::OwnedTaskBuffers::new(None, None)?;
            return Ok(self.queue_task_command_buffers(command, &buffers, output)?
                .then_some(TaskCommandOutcome { done: true, propagate: false }));
        }
        self.receive_css_task_command(command, output)
    }

    pub(super) fn restore_css_task_native(&mut self, args: &[u8]) -> Result<NativeReturn> {
        let sha = self.css_task_sha()
            .ok_or_else(|| native_state_error("native576 needs an identified PE wrapper"))?;
        let &(_, code, common_paths) = COMMANDS.iter().find(|entry| entry.0 == sha)
            .ok_or_else(|| native_state_error("native576 has no exact PE wrapper descriptor"))?;
        let container = read_u32(args, 0)?;
        if container == 0 {
            // The original does not read item ID or target on this branch.
            return Ok(NativeReturn::Value(0));
        }
        let item = read_u32(args, 4)?;
        let record = self.css_handles.get(&container)
            .ok_or_else(|| native_state_error("native576 has an invalid/wrong-type CSS handle"))?
            .try_record(item)
            .map_err(|_| native_state_error("native576 CSS record scan is malformed"))?;
        let Some(record) = record else {
            // try_record scans retained capacity and chooses the first item.
            // A present zero-length body is not a missing record.
            return Ok(NativeReturn::Value(0));
        };
        let length = record.len();
        let target = read_u32(args, 8)?;
        if sha == super::css_model::SHA
            && self.data_services.css_models.owners.contains_key(&target)
        {
            let body = record.to_vec();
            let command = TaskCommand { target: target as i32,
                sender: self.current_task_id as i32, code: 341, data: [0, 1, 0] };
            let mut buffers = self.pending_owned_task_buffers(command).unwrap_or(
                super::native_tasks::OwnedTaskBuffers::new(None, Some(body))?);
            let mut output = 1;
            let outcome = self.route_task_command_buffers(command, &mut buffers, &mut output, 4096)?;
            return Ok(if outcome.is_some_and(|outcome| !outcome.done) {
                NativeReturn::Retry
            } else { NativeReturn::Value(u32::from(output != 0)) });
        }
        let command = TaskCommand { target: target as i32, sender: self.current_task_id as i32,
            code, data: [0, 1, 0] };
        let reachable = self.task_is_reachable(target);
        if reachable { self.ensure_task_record(target)?; }
        let known = reachable && self.css_receiver(target, code)?.is_some();
        if known || (!reachable && common_paths) {
            // The out address aliases the native's argument-pointer stack
            // slot (entry ESP+12). That pointer is valid and nonzero. Only
            // its zero/nonzero value escapes this wrapper, so retain that
            // boolean instead of fabricating an x86 address or seeding0.
            // The fifteen reviewed wrappers and the old366 wrapper have this
            // same independently checked alias. Routing's EAX is ignored.
            let mut output = 1;
            let mut buffers = super::native_tasks::OwnedTaskBuffers::new(None, None)?;
            let outcome = self.route_task_command_buffers(command, &mut buffers, &mut output, 4096)?;
            return Ok(if outcome.is_some_and(|outcome| !outcome.done) {
                NativeReturn::Retry
            } else { NativeReturn::Value(u32::from(output != 0)) });
        }
        // The wrapper sends sender=current task, data=[0,body pointer,0].
        // Its helper's length output is NOT passed to the receiving object.
        // Other concrete/external targets still need their actual receiver.
        // No record layout, encoding or modern body consumer is inferred
        // from the legacy341 material reader or these non-reading leaves.
        let detail = format!(
            "native576 receiver is unrecovered: PE={sha} command={code} sender={} target={target} CSS={container} item={item} body_bytes={length}",
            self.current_task_id,
        );
        Err(self.task_error(target, cs2_runtime::RuntimeError::Host(detail)))
    }
}
