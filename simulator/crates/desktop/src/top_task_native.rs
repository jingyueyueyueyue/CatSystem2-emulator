//! Root clock control. The root samples this DWORD after updating its tree;
//! the next platform iteration keeps dispatching while holding game time.
use crate::{LoggingHost, native_state_error, read_u32};
use cs2_runtime::{TaskCommand, TaskCommandOutcome};
use cs2_script::{NativeReturn, Result};

#[derive(Default)]
pub(super) struct TopClockState {
    pub(super) requested: u32,
    pub(super) sampled: u32,
}

impl LoggingHost<'_> {
    fn root_clock_command(&self) -> Option<u32> {
        self.data_services
            .ordinary_native_table
            .as_ref()?
            .root_clock_command()
    }

    pub(super) fn top_clock_native(&mut self, args: &[u8]) -> Result<NativeReturn> {
        let value = read_u32(args, 0)?;
        let code = self.root_clock_command().ok_or_else(|| {
            native_state_error("root clock hold requires a verified ordinary native 577 ABI")
        })?;
        self.receive_top_clock_command(TaskCommand {
            target: 0,
            sender: self.current_task_id as i32,
            code,
            data: [0, value, 0],
        })?
        .ok_or_else(|| native_state_error("root clock hold receiver is absent"))?;
        Ok(NativeReturn::Continue)
    }

    pub(crate) fn receive_top_clock_command(
        &mut self,
        command: TaskCommand,
    ) -> Result<Option<TaskCommandOutcome>> {
        let code = self.root_clock_command();
        if command.target != 0 || code != Some(command.code) {
            return Ok(None);
        }
        self.data_services.top_clock.requested = command.data[1];
        Ok(Some(TaskCommandOutcome {
            done: true,
            propagate: false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HeadlessPlayer, tests::TempDir};
    use cs2_runtime::{TaskCommandBuffers, native_table::OrdinaryNativeTable};
    use cs2_script::{KcsProgram, Vm};
    use std::{fs, path::Path};

    #[test]
    fn ordinary_native_577_holds_and_root_command_releases_game_time() -> anyhow::Result<()> {
        let game = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games/ISLAND/ISLAND.exe");
        if !game.is_file() {
            return Ok(());
        }
        let temp = TempDir::new();
        let mut player = HeadlessPlayer::open_with_save_root(&temp.0, temp.0.join("profile"))?;
        let mut vm = Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 64], 64)?)?;
        player.with_host(|host| {
            assert!(
                host.dispatch_kcs_native(577, &1u32.to_le_bytes(), &mut vm)
                    .is_err()
            );
            Ok(())
        })?;
        let table = OrdinaryNativeTable::from_pe(&fs::read(game)?).unwrap();
        let code = table.root_clock_command().unwrap();
        player.data_services.ordinary_native_table = Some(table);
        player.with_host(|host| {
            assert_eq!(
                host.dispatch_kcs_native(577, &2u32.to_le_bytes(), &mut vm)?,
                NativeReturn::Continue,
            );
            Ok(())
        })?;
        assert_eq!(player.data_services.top_clock.requested, 2);
        assert_eq!(player.data_services.top_clock.sampled, 0);
        player.run_frames(1, 1000)?;
        assert_eq!(player.data_services.top_clock.sampled, 2);
        let tick = player.state.clock.elapsed_ticks;
        let frame = player.state.clock.frame;
        player.run_frames(1, 1000)?;
        assert_eq!(player.state.clock.elapsed_ticks, tick);
        assert_eq!(player.state.clock.frame, frame + 1);

        player.with_host(|host| {
            assert_eq!(
                host.receive_task_command(
                    TaskCommand {
                        target: 0,
                        sender: 1,
                        code,
                        data: [0; 3]
                    },
                    &mut TaskCommandBuffers::default(),
                    &mut 0,
                    1000,
                )?,
                Some(TaskCommandOutcome {
                    done: true,
                    propagate: false
                }),
            );
            Ok(())
        })?;
        assert_eq!(player.data_services.top_clock.requested, 0);
        assert_eq!(player.data_services.top_clock.sampled, 2);
        player.run_frames(1, 1000)?;
        assert_eq!(player.data_services.top_clock.sampled, 0);
        assert_eq!(player.state.clock.elapsed_ticks, tick);
        player.run_frames(1, 1000)?;
        assert_eq!(player.state.clock.elapsed_ticks, tick + 1);
        Ok(())
    }
}
