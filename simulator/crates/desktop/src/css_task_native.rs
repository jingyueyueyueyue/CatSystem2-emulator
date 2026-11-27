//! Registered CSS576 wrappers and concrete, independently verified receivers.
//! A wrapper's boolean is not evidence that a CSS body was restored.
//! Evidence: css_task_576_F_contracts / css_task_receiver_F_20261117 reports.
use cs2_runtime::{TaskCommand, TaskCommandOutcome};
use cs2_runtime::native_table::CssRestoreCommand;
use cs2_script::{NativeReturn, Result};

use crate::{LoggingHost, native_state_error, read_u32};

// Retain recognized engine identities for other legacy consumers sharing this
// field. Slot 576 itself recovers commands and receiver behavior at startup.
const LEGACY_COMPATIBILITY_PE: &[&str] = &[
    "659d5f069f3c97f69a61e12449cf1510b45e48a10649b9ac6977c2b93d6d28ad",
    "a9f8b08713aac278671853b2293a13b2623a122ede9341cb921901b09601cb52",
    "f6af7437475b8c066be973245aa79688bea510ca1192264add4f1914ebee4c0e",
    "012c71635932c241cde4df986377516279a50b907166757e89ea7b86ff3f813f",
    "9f3a5ad0ed22074420685f1e2a3bc92436d68289c7bac854d5f7d7331dc3ef67",
    "72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8",
    "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f",
    "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc",
    "474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1",
    "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976",
    "c47c9d883126f641ce07d92eb57b6ac9833fe7495996001c12c68c55b99974d3",
    "4ccb48ad4bb6bfb5a15a45b8e1f2c2e62072cfad9f1487f869561a473770ed94",
    "c48d383c335c5b283bb0c17bd9f14b2b8bfef4a2e3d635365157b1c69498df78",
    "c2af2ee9baf4b26a3c2cf8a1c867f15f92b54ad1ac5fac381858046a47818f6c",
    "ac226c8455eff72114d6b1b9929fae01ac49ba90b8ba579f049f179c4c6dd8d5",
    "3b359fc2d737d5ceed454d6b65e6e5b5e6dfb4a24cf88ee361df86e454203f18",
    "6f75ae3877e018de1982a68f1089c127870d895d748c6290dd2ee85cd8daa4de",
    "1f3288d5fd5408bee4d395faab708aca78c9eeaa6c172dc06dcce09c844ba9d5",
    "7bd9f6dcc7d62868d4ad9cc68d5580452e71bd907540ec4f993d960d051d2c45",
    "a540b1dec1185a63cc68c4d283b6bc99bcce8de219e1bead3b8d113577de4027",
    "cfb7f74ac3674962a8d982fe7e0ee53b3306fb47362653db794ad6dcb0fcb35d",
    "f36f63798eb27b1eb6742c2fe7c3a13f172a249ee6623a12fc79fd8c3dee3cfe",
    "b70a802a74c49a5d6f429114f81d878f83e25a4acbcb2778c6f4d5f68528de7b",
    "985096b6064e35c7f48c080aa9008d67442fc0df7e33fec7210fe13fefc1dc4f",
    "a099ac1cee7af60c8f32869485c9df4aab344b5eb91c1018ae8bb7df2c1f39f3",
    "5869b7d03a636c200f5042f0c3fc37878a362615bf724f58d47aec5eb8b2d44c",
];

pub(super) fn identified_pe_sha256(sha: &str) -> Option<&'static str> {
    LEGACY_COMPATIBILITY_PE.iter().copied().find(|&known| known == sha)
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

    fn css_restore_spec(&self) -> Option<CssRestoreCommand> {
        self.data_services.ordinary_native_table.as_ref()
            .and_then(|table| table.css_restore_command())
            .or_else(|| self.data_services.startup_companion_natives.as_ref()
                .and_then(|companion| companion.css_restore_command))
    }

    /// Route the legacy 3D owner's lifecycle packet through its task receiver.
    pub(super) fn css_owner_lifecycle_native(
        &mut self, index: u32, args: &[u8],
    ) -> Result<Option<NativeReturn>> {
        if index != 83 {
            return Ok(None);
        }
        let Some(code) = self.data_services.ordinary_native_table.as_ref()
            .and_then(|table| table.legacy_view_lifecycle())
            .or_else(|| self.data_services.startup_companion_natives.as_ref()
                .and_then(|companion| companion.legacy_view_lifecycle))
        else {
            return Ok(None);
        };
        let target = read_u32(args, 0)?;
        if target == 0 || !self.data_services.tasks.message_task_state(target)
            .is_some_and(|(kind, _, _)| kind == 42)
        {
            return Ok(None);
        }
        if !self.data_services.css_models.owners.contains_key(&target)
            && !self.data_services.particle_external.canvases.contains_key(&target)
        {
            return Ok(None);
        }
        let command = TaskCommand { target: target as i32, sender: self.current_task_id as i32,
            code, data: [0; 3] };
        let mut buffers = super::native_tasks::OwnedTaskBuffers::new(None, None)?;
        let outcome = self.route_task_command_buffers(command, &mut buffers, &mut 0, 4096)?;
        Ok(Some(if outcome.is_some_and(|outcome| !outcome.done) {
            NativeReturn::Retry
        } else { NativeReturn::Continue }))
    }

    fn css_receiver(&self, target: u32, code: u32) -> Result<Option<Receiver>> {
        let Some(CssRestoreCommand { code: expected, common_receivers_preserve_output, view_rejects_restore }) = self.css_restore_spec()
            else { return Ok(None); };
        if code != expected { return Ok(None); }
        let Some((kind, deletion, _)) = self.data_services.tasks.message_task_state(target)
            else { return Ok(None); };
        if common_receivers_preserve_output && matches!(kind, 0 | 12) {
            // Top's out-of-switch return consumes; ordinary/Base delegates
            // to the generic handler, which leaves this output untouched.
            return Ok(Some(Receiver::PreserveOutput { propagate: target != 0 }));
        }
        if view_rejects_restore && kind == 42 {
            if deletion != 0 {
                return Ok(Some(Receiver::PreserveOutput { propagate: false }));
            }
            // The live CPU graph/manager is allocated by the real task
            // constructor. Availability precedes configure/model load; an
            // empty graph is not an unavailable constructor. Never create a
            // manager here, or mistake a modern kind42 SCROLL2 for this class.
            let owner = self.data_services.css_models.utate_models.contains_key(&target)
                || self.data_services.particle_external.canvases.contains_key(&target);
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
        let CssRestoreCommand { code, common_receivers_preserve_output, .. } = self.css_restore_spec()
            .ok_or_else(|| native_state_error("native576 needs a registered CSS restore wrapper"))?;
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
        if code == 341 && self.data_services.css_models.owners.contains_key(&target) {
            let command = TaskCommand { target: target as i32,
                sender: self.current_task_id as i32, code, data: [0, 1, 0] };
            let mut buffers = match self.pending_owned_task_buffers(command) {
                Some(pending) => pending,
                None => super::native_tasks::OwnedTaskBuffers::new(None, Some(record.to_vec()))?,
            };
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
        if known || (!reachable && common_receivers_preserve_output) {
            // The out address aliases the native's argument-pointer stack
            // slot (entry ESP+12). That pointer is valid and nonzero. Only
            // its zero/nonzero value escapes this wrapper, so retain that
            // boolean instead of fabricating an x86 address or seeding0.
            // The registered wrapper proves that alias and ignores routing's EAX.
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
            "native576 receiver is unrecovered: command={code} sender={} target={target} CSS={container} item={item} body_bytes={length}",
            self.current_task_id,
        );
        Err(self.task_error(target, cs2_runtime::RuntimeError::Host(detail)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HeadlessPlayer, tests::TempDir};
    use cs2_formats::CssFile;
    use cs2_script::{KcsProgram, Vm};
    use std::path::Path;

    #[test]
    #[ignore = "requires the installed V401 engine"]
    fn native_css_restore_registered_desktop_preserves_live_output() -> anyhow::Result<()> {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let scratch = TempDir::new();
        let mut player = HeadlessPlayer::open_with_save_root(workspace.join("games/cs2_full_v401"), &scratch.0)?;
        player.data_services.native_profile = None;
        player.data_services.css_task_pe_sha256 = None;
        let mut css = CssFile::new(1000);
        css.append(42, b"")?;
        let mut vm = Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 8], 0)?)?;
        let args = |record, target| [1, record, target].map(u32::to_le_bytes).concat();
        player.with_host(|host| {
            host.css_handles.insert(1, css.clone());
            assert_eq!(host.dispatch_kcs_native(576, &0u32.to_le_bytes(), &mut vm)?, NativeReturn::Value(0));
            assert_eq!(host.dispatch_kcs_native(576, &args(17, 0), &mut vm)?, NativeReturn::Value(0));
            assert_eq!(host.dispatch_kcs_native(576, &args(42, 0), &mut vm)?, NativeReturn::Value(1));
            assert_eq!(host.dispatch_kcs_native(576, &args(42, 10_000), &mut vm)?, NativeReturn::Value(1));
            let child = host.create_task_object(0, 0, b"CSS restore regression", b"")?;
            assert_eq!(host.dispatch_kcs_native(576, &args(42, child), &mut vm)?, NativeReturn::Value(1));
            let mut output = 0xa55a;
            let outcome = host.receive_css_task_command(TaskCommand {
                target: child as i32, sender: 0, code: 426, data: [0; 3],
            }, &mut output)?.unwrap();
            assert!(outcome.done && outcome.propagate);
            assert_eq!(output, 0xa55a);
            assert_eq!(host.css_handles.get(&1), Some(&css));

            host.data_services.ordinary_native_table = None;
            host.data_services.startup_companion_natives = None;
            let error = host.dispatch_kcs_native(576, &args(42, 0), &mut vm).unwrap_err();
            assert!(error.to_string().contains("needs a registered CSS restore wrapper"));
            assert_eq!(host.css_handles.get(&1), Some(&css));
            Ok(())
        })
    }
}
