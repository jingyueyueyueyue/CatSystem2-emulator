//! Reserved ordinary callbacks backed by complete function bodies.
use cs2_runtime::native_table::OrdinaryNativeTable;
use cs2_script::{NativeReturn, Result, ScriptError};

pub(super) fn call(table: Option<&OrdinaryNativeTable>, index: u32) -> Result<NativeReturn> {
    table
        .and_then(|table| table.constant(index))
        .map(|constant| constant.native_return())
        .ok_or(ScriptError::UnimplementedNativeCall { index })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_script::{Host, Vm};
    use std::path::Path;

    #[test]
    #[ignore = "requires the installed Djibril 5 and Grisaia games and an isolated save root"]
    fn legacy_native439_routes_through_the_actual_game_registration() -> anyhow::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let saves = std::env::var_os("CS2_SIDECAR9_PROFILE").expect("isolated profile required");
        for (game, executable) in [
            ("\u{6226}\u{56fd}\u{5929}\u{4f7f}\u{30b8}\u{30d6}\u{30ea}\u{30fc}\u{30eb}", "Djibril5.exe"),
            ("\u{7070}\u{8272}\u{7684}\u{679c}\u{5b9e}", "Grisaia.exe"),
        ] {
            let mut player = crate::HeadlessPlayer::open_with_save_root(
                games.join(game), Path::new(&saves).join(executable),
            )?;
            assert_eq!(
                player.scan.engine.primary_executable.as_ref().unwrap().relative_path,
                Path::new(executable),
            );
            player.with_host(|host| {
                let mut vm = Vm::new();
                assert_eq!(host.call(439, &[0; 8], &mut vm)?, NativeReturn::Continue);
                host.data_services.ordinary_native_table = None;
                assert!(matches!(host.call(439, &[0; 8], &mut vm),
                    Err(ScriptError::UnimplementedNativeCall { index: 439 })));
                Ok(())
            })?;
        }
        Ok(())
    }
}
