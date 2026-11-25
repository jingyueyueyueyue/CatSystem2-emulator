use cs2_script::NativeReturn;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the installed Rebellions game and an isolated save root"]
    fn rebellions_registered_ordinary_constants_use_installed_game() -> anyhow::Result<()> {
        use std::path::Path;
        use cs2_runtime::native_profile::ConstantNativeReturn;
        use cs2_script::{Host, Vm};

        let game = std::env::var_os("CS2_REBELLIONS_TEST_GAME").expect("installed game root required");
        let profile = std::env::var_os("CS2_SIDECAR9_PROFILE").expect("isolated profile required");
        let mut player = crate::HeadlessPlayer::open_with_save_root(
            game, Path::new(&profile).join("rebellions-native-constants"),
        )?;
        let primary = player.scan.engine.primary_executable.as_ref().unwrap();
        assert!(primary.relative_path.as_os_str().eq_ignore_ascii_case("REBELLIONS.exe"));
        assert_eq!(player.default_entry.as_deref(), Some("kcs.int/main.kcs"));
        let table = player.data_services.ordinary_native_table.as_ref()
            .expect("startup resolves the registered ordinary table");
        assert_eq!(table.constant(439), Some(ConstantNativeReturn::Continue));
        assert_eq!(table.feature_constant(925), Some(ConstantNativeReturn::Value(0)));
        player.load_startup()?;
        player.with_host(|host| {
            let mut vm = Vm::new();
            let mut resume_args = [0u8; 8];
            resume_args[..4].copy_from_slice(&1u32.to_le_bytes());
            assert_eq!(host.call(439, &resume_args, &mut vm)?, NativeReturn::Continue);
            assert_eq!(host.call(925, &[], &mut vm)?, NativeReturn::Value(0));
            Ok(())
        })
    }

    #[test]
    #[ignore = "requires the installed Happiness!2 game and an isolated save root"]
    fn happiness2_native439_and_925_use_registered_archive_companion() -> anyhow::Result<()> {
        use std::path::Path;
        use cs2_script::{Host, ScriptError, Vm};

        let game = std::path::PathBuf::from(
            std::env::var_os("CS2_HAPPINESS2_TEST_GAME").expect("installed game root required"),
        );
        let profile = std::env::var_os("CS2_SIDECAR9_PROFILE").expect("isolated profile required");
        let mut player = crate::HeadlessPlayer::open_with_save_root(
            &game, Path::new(&profile).join("happiness2-native925"),
        )?;
        assert!(player.data_services.ordinary_native_table.is_none());
        let companion = player.data_services.startup_companion_natives.as_ref().unwrap();
        assert!(companion.native439_continue && companion.native925_zero);
        assert_eq!(companion.message_list_base, Some(448));
        let raster_messages = companion.raster_messages.expect("registered Raster18 commands");
        assert_eq!((raster_messages.start, raster_messages.configure), (263, 269));
        assert!(companion.raster_finalize_passthrough);
        const TABLE_OFFSET: usize = 0x5e26e8;
        let mut companion = std::fs::read(game.join("cs2_crack.exe"))?;
        let mut primary = companion.clone();
        let slot_925 = TABLE_OFFSET + 925 * 4;
        primary.copy_within(slot_925..slot_925 + 4, TABLE_OFFSET + 439 * 4);
        let primary = cs2_runtime::native_table::OrdinaryNativeTable::from_pe(&primary).unwrap();
        player.load_startup()?;
        player.with_host(|host| {
            // The producer's registered return does not depend on the particle profile.
            host.data_services.particle_message_pe_sha256 = None;
            let mut vm = Vm::new();
            assert_eq!(host.call(925, &[], &mut vm)?, NativeReturn::Value(0));
            let inspector = host.create_task_object(47, 0, b"message list", b"")?;
            let binding = [inspector, 0].map(u32::to_le_bytes).concat();
            assert_eq!(host.call(571, &binding, &mut vm)?, NativeReturn::Continue);
            let raster = host.create_task_object(18, 0, b"Raster", b"")?;
            let mut raster_vm = Vm::from_program(cs2_script::KcsProgram::from_code(
                &[0x3b], vec![0; 64], 0,
            )?)?;
            let record = [0u32, 0, 0, 15f32.to_bits(), 20f32.to_bits()]
                .map(u32::to_le_bytes).concat();
            raster_vm.write(4, &record)?;
            let configure = [raster, 4].map(u32::to_le_bytes).concat();
            let receiver = raster.to_le_bytes();
            assert_eq!(host.call(196, &configure, &mut raster_vm)?, NativeReturn::Continue);
            assert_eq!(host.call(197, &receiver, &mut raster_vm)?, NativeReturn::Continue);
            assert_eq!(host.call(200, &receiver, &mut raster_vm)?, NativeReturn::Value(1));
            assert_eq!(host.call(203, &receiver, &mut raster_vm)?, NativeReturn::Continue);
            assert_eq!(host.call(198, &receiver, &mut raster_vm)?, NativeReturn::Continue);
            assert_eq!(host.call(200, &receiver, &mut raster_vm)?, NativeReturn::Value(0));
            for flag in [0u32, 1] {
                let arguments = [17u32, flag].map(u32::to_le_bytes).concat();
                assert_eq!(host.call(439, &arguments, &mut vm)?, NativeReturn::Continue);
            }
            let bad_flag = [17u32, 2].map(u32::to_le_bytes).concat();
            for (index, arguments) in [
                (925, &[0; 4][..]), (924, &[][..]), (439, &[0; 4][..]), (439, bad_flag.as_slice()),
            ] {
                assert!(matches!(host.call(index, arguments, &mut vm),
                    Err(ScriptError::UnimplementedNativeCall { index: reported }) if reported == index));
            }
            host.data_services.ordinary_native_table = Some(primary);
            assert_eq!(host.call(439, &[0; 8], &mut vm)?, NativeReturn::Value(0));
            host.data_services.ordinary_native_table = None;
            host.data_services.startup_companion_natives = None;
            assert!(matches!(host.call(439, &[0; 8], &mut vm),
                Err(ScriptError::UnimplementedNativeCall { index: 439 })));
            Ok(())
        })?;

        let fixture = Path::new(&profile).join(format!("companion-925-{}", std::process::id()));
        std::fs::create_dir_all(fixture.join("config"))?;
        for name in ["cs2.exe", "kcs.int"] {
            std::fs::copy(game.join(name), fixture.join(name))?;
        }
        std::fs::write(fixture.join("config/startup.xml"), br#"<?xml version="1.0" encoding="UTF-8"?>
<document><SCRIPT><start>kcs.int/main.kcs</start></SCRIPT><INT>
  <int01><name>kcs</name><int>kcs.int</int><ext>kcs</ext><dir>kcs</dir></int01>
</INT></document>
"#)?;
        companion.extend_from_slice(b"unrelated PE overlay");
        std::fs::write(fixture.join("companion.exe"), &companion)?;

        let scan_fixture = || cs2_runtime::GameRoot::open(&fixture)?.scan();
        let scan = scan_fixture()?;
        let recover = cs2_runtime::native_table::OrdinaryNativeTable::startup_companion_natives;
        use cs2_runtime::native_table::RenderArgumentOrder::DurationFirst;
        assert_eq!(scan.engine.primary_executable.as_ref().unwrap().relative_path, Path::new("cs2.exe"));
        let contracts = recover(&scan, "kcs.int/main.kcs").unwrap();
        assert!(contracts.native439_continue && contracts.native925_zero);
        assert_eq!((contracts.draw_order, contracts.wipe_order), (Some(DurationFirst), Some(DurationFirst)));
        assert_eq!(contracts.message_list_base, Some(448));
        assert_eq!(contracts.raster_messages, Some(raster_messages));
        assert!(contracts.raster_finalize_passthrough);
        assert!(recover(&scan, "config/startup.xml").is_none());

        let scene_script = scan.resources.read_category(b"kcs", "sscript")?;
        let scene_candidate = scan.resources.resolve("kcs.int/sscript.kcs")?.unwrap();
        let cs2_runtime::ResourceLocation::Archive { path, offset, size, .. } = &scene_candidate.location else {
            panic!("expected a scene script in the copied archive");
        };
        let mut archive = std::fs::read(path)?;
        let last_byte = usize::try_from(offset + size - 1)?;
        archive[last_byte] ^= 1;
        std::fs::write(path, archive)?;
        let changed_script = scan_fixture()?;
        assert_ne!(changed_script.resources.read_category(b"kcs", "sscript")?, scene_script);
        assert_eq!(recover(&changed_script, "kcs.int/main.kcs").unwrap().message_list_base, Some(448));

        // Redirect 439 to the adjacent side-effecting wrapper while preserving the table and seed.
        let slot_438 = TABLE_OFFSET + 438 * 4;
        companion.copy_within(slot_438..slot_438 + 4, slot_438 + 4);
        std::fs::write(fixture.join("companion.exe"), &companion)?;
        let changed = recover(&scan_fixture()?, "kcs.int/main.kcs").unwrap();
        assert!(!changed.native439_continue && changed.native925_zero);

        std::fs::copy(game.join("cs2_crack.exe"), fixture.join("another.exe"))?;
        assert!(recover(&scan_fixture()?, "kcs.int/main.kcs").is_none());

        std::fs::rename(fixture.join("another.exe"), fixture.join("another.exe.disabled"))?;
        std::fs::create_dir_all(fixture.join("kcs"))?;
        std::fs::write(fixture.join("kcs/sscript.kcs"), scene_script)?;
        let overridden = scan_fixture()?;
        assert!(recover(&overridden, "kcs.int/main.kcs").is_none());
        Ok(())
    }

}
