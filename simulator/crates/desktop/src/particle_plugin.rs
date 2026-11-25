//! Independent SDK2 effect models. No PE code is loaded or
//! called. Evidence: 20261117_D_ptclpi_* and20261117_D_ppi_*.
use cs2_script::Result;
use cs2_runtime::particle_plugin_pe::ParticlePluginPe;
use super::particle_vm::{ParticleCanvas, ParticleQuad, ParticleRandomSource,
    ParticleRenderSnapshot, ParticleVertex};
#[path = "particle_family.rs"]
mod family;
#[path = "particle_common.rs"]
mod common;
#[path = "particle_plugin_state.rs"]
pub(crate) mod checkpoint;
#[path = "particle_plugin_layout.rs"]
mod plugin_layout;
use plugin_layout::Layout;

const MAX_STORAGE: usize = 64 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};

    #[test]
    #[ignore = "requires installed SDK1 and SDK2 particle plugins"]
    fn ppi_load_uses_registered_sdk_callbacks() -> anyhow::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let samples = [
            ("\u{30ab}\u{30df}\u{30ab}\u{30bc}\u{2606}\u{30a8}\u{30af}\u{30b9}\u{30d7}\u{30ed}\u{30fc}\u{30e9}\u{30fc}!/ptclpi/rain.ppi", Layout::Rain, 1),
            ("\u{3053}\u{3053}\u{304b}\u{3089}\u{590f}\u{306e}\u{30a4}\u{30ce}\u{30bb}\u{30f3}\u{30b9}\u{ff01}/ptclpi/rain2.ppi", Layout::Rain, 1),
            ("\u{3053}\u{3053}\u{304b}\u{3089}\u{590f}\u{306e}\u{30a4}\u{30ce}\u{30bb}\u{30f3}\u{30b9}\u{ff01}/ptclpi/rain.ppi", Layout::ScaledRain, 1),
            ("Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}/ptclpi/rain.ppi", Layout::AtlasRain, 1),
            ("Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}/ptclpi/bang2.ppi", Layout::Burst, 1),
            ("Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}/ptclpi/cmnptcl.ppi", Layout::Common, 0x24),
            ("Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}/ptclpi/wc_cmnptcl.ppi", Layout::Common, 4),
        ];
        let load = |bytes: &[u8]| ParticlePlugin::load(bytes, 2, [32.; 2], ParticleCanvas::default());
        for (path, expected, allocation_operand) in samples {
            let mut bytes = fs::read(games.join(path))?;
            assert_eq!(plugin_layout::detect(&ParticlePluginPe::read(&bytes).unwrap()), Some(expected), "{path}");
            let original = load(&bytes)?.snapshot(255);
            bytes.extend_from_slice(b"unrelated overlay");
            let renamed = load(&bytes)?.snapshot(255);
            assert_eq!(original.quads, renamed.quads, "{path}");

            let pe = ParticlePluginPe::read(&bytes).unwrap();
            let constructor = pe.constructor().unwrap();
            let allocation = constructor.as_ptr() as usize - bytes.as_ptr() as usize + allocation_operand;
            bytes[allocation] ^= 1;
            assert!(load(&bytes).is_err(), "altered object layout was accepted for {path}");
        }
        let mut sdk1 = fs::read(games.join("cs2_full_v401/system/ptclpi/sample_rain.ppi"))?;
        sdk1.extend_from_slice(b"unrelated overlay");
        let error = load(&sdk1).unwrap_err().to_string();
        assert!(error.contains("rejects type1/version0x10000"));
        Ok(())
    }

    #[test]
    #[ignore = "requires the installed Sakura plugin"]
    fn ppi_sakura_checkpoint_uses_the_registered_path_model() -> anyhow::Result<()> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(concat!(
            "../../../games/Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}/",
            "ptclpi/sakura.ppi",
        ));
        let original = fs::read(path)?;
        let mut overlaid = original.clone();
        overlaid.extend_from_slice(b"unrelated overlay");
        for bytes in [&original, &overlaid] {
            let extent = [32.; 2];
            let plugin = ParticlePlugin::load(bytes, 2, extent, ParticleCanvas::default())?;
            let restored = plugin.capture_checkpoint()?.prepare(bytes, 2, extent)?;
            assert_eq!(restored.snapshot(255).quads, plugin.snapshot(255).quads);
        }
        let plugin = ParticlePlugin::load(&original, 2, [32.; 2], ParticleCanvas::default())?;
        assert!(plugin.capture_checkpoint()?.prepare(&overlaid, 2, [32.; 2]).is_err());
        let pe = ParticlePluginPe::read(&overlaid).unwrap();
        let offset = pe.constructor().unwrap().as_ptr() as usize - overlaid.as_ptr() as usize + 4;
        overlaid[offset] ^= 1;
        assert!(ParticlePlugin::load(&overlaid, 2, [32.; 2], ParticleCanvas::default()).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Question reaction plugins"]
    fn ppi_question_uses_its_phase_table_for_the_oscillation_variant() -> anyhow::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let happiness = games.join("Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}/ptclpi");
        let neko = games.join(concat!(
            "[170526][NEKO WORKs] \u{30cd}\u{30b3}\u{3071}\u{3089} vol.3 ",
            "steam\u{7248} (files)/nekopara_vol3/ptclpi",
        ));
        for (path, oscillation) in [
            (happiness.join("em_question.ppi"), true),
            (neko.join("em_question.ppi"), true),
            (neko.join("em_question_a.ppi"), false),
        ] {
            let mut bytes = fs::read(&path)?;
            for overlay in [b"".as_slice(), b"unrelated PE overlay"] {
                bytes.extend_from_slice(overlay);
                let pe = ParticlePluginPe::read(&bytes).unwrap();
                assert_eq!(plugin_layout::question_oscillation(&pe), Some(oscillation), "{path:?}");
                let plugin = ParticlePlugin::load(&bytes, 2, [32.; 2], ParticleCanvas::default())?;
                if oscillation {
                    let state = plugin.capture_checkpoint()?;
                    assert_eq!(state.prepare(&bytes, 2, [32.; 2])?.snapshot(255).quads,
                        plugin.snapshot(255).quads, "{path:?}");
                } else {
                    assert!(plugin.capture_checkpoint().is_err());
                }
            }
            let pe = ParticlePluginPe::read(&bytes).unwrap();
            let allocate = pe.constructor().unwrap().as_ptr() as usize - bytes.as_ptr() as usize + 4;
            bytes[allocate] ^= 1;
            assert!(ParticlePlugin::load(&bytes, 2, [32.; 2], ParticleCanvas::default()).is_err(), "{path:?}");
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires the installed Dark Field plugins"]
    fn ppi_dark_fields_use_registered_stride_centers_and_damping() -> anyhow::Result<()> {
        use crate::particle_vm::ParticleRandom;
        use plugin_layout::DarkField;

        let game = Path::new(env!("CARGO_MANIFEST_DIR")).join(concat!(
            "../../../games/Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}",
        ));
        let resources = crate::ResourceIndex::scan(&game)?;
        let extent = [32.; 2];
        let load = |bytes: &[u8]| ParticlePlugin::load(bytes, 5, extent, ParticleCanvas::default());
        let mut parameters = [0; 128];
        for (offset, value) in [20_f32, 30., 100., 120., 160., 140., 60., 80., 0., 200., 180., 100., 1.]
            .into_iter().enumerate()
        {
            parameters[offset * 4..offset * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (file, variant) in [
            ("wc_dark.ppi", DarkField::SingleCenter),
            ("dark_tri.ppi", DarkField::ThreeCenters),
            ("wc_darkund.ppi", DarkField::Rising),
            ("wc_ground.ppi", DarkField::Ground),
        ] {
            let bytes = fs::read(game.join("ptclpi").join(file))?;
            let mut overlaid = bytes.clone();
            overlaid.extend_from_slice(b"unrelated PE overlay");
            let pe = ParticlePluginPe::read(&overlaid).unwrap();
            assert_eq!(plugin_layout::dark_field(&pe), Some(variant), "{file}");
            let mut original = load(&bytes)?;
            let mut changed = load(&overlaid)?;
            let mut random = ParticleRandom::default();
            let mut changed_random = random.clone();
            original.start(&parameters, [1280, 720], &resources, &mut random)?;
            changed.start(&parameters, [1280, 720], &resources, &mut changed_random)?;
            for tick in (0..=500).step_by(5) {
                original.update(tick, &parameters, &mut random);
                changed.update(tick, &parameters, &mut changed_random);
                assert_eq!(changed.snapshot(255).quads, original.snapshot(255).quads, "{file} at {tick}");
            }
            if variant == DarkField::Ground {
                assert!(changed.capture_checkpoint().is_err());
            } else {
                assert!(changed.capture_checkpoint()?.prepare(&bytes, 5, extent).is_err());
                let mut restored = changed.capture_checkpoint()?.prepare(&overlaid, 5, extent)?;
                assert_eq!(restored.snapshot(255).quads, changed.snapshot(255).quads, "{file}");
                restored.update(505, &parameters, &mut changed_random.clone());
                changed.update(505, &parameters, &mut changed_random);
                assert_eq!(restored.snapshot(255).quads, changed.snapshot(255).quads, "{file}");
            }
            let factory = pe.constructor().unwrap();
            let allocation = if variant == DarkField::ThreeCenters { 0x25 } else { 4 };
            let offset = factory.as_ptr() as usize - overlaid.as_ptr() as usize + allocation;
            overlaid[offset] ^= 1;
            assert!(load(&overlaid).is_err(), "changed object allocation accepted for {file}");
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed path effect plugins"]
    fn ppi_path_effects_use_registered_storage_and_parameters() -> anyhow::Result<()> {
        use crate::particle_vm::ParticleRandom;
        use plugin_layout::PathEffect;

        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let happiness = games.join("Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}");
        let neko = games.join(concat!(
            "[170526][NEKO WORKs] \u{30cd}\u{30b3}\u{3071}\u{3089} vol.3 ",
            "steam\u{7248} (files)/nekopara_vol3",
        ));
        let innocence = games.join(
            "\u{3053}\u{3053}\u{304b}\u{3089}\u{590f}\u{306e}\u{30a4}\u{30ce}\u{30bb}\u{30f3}\u{30b9}\u{ff01}",
        );
        let samples = [
            (&happiness, "em_note.ppi", PathEffect::Note, 0xb6, 0x19, 0x29),
            (&neko, "em_note.ppi", PathEffect::Note, 0xb6, 0x19, 0x29),
            (&happiness, "em_oko.ppi", PathEffect::Oko, 0xa9, 0x19, 0x29),
            (&happiness, "em_sweat.ppi", PathEffect::Sweat, 0xb4, 0x19, 0x29),
            (&neko, "em_sweat.ppi", PathEffect::Sweat, 0xb4, 0x19, 0x29),
            (&happiness, "em_sigh.ppi", PathEffect::Sigh, 0x68, 0x19, 0x29),
            (&happiness, "energy.ppi", PathEffect::Energy, 0x6d, 0x19, 0x29),
            (&happiness, "kirakira.ppi", PathEffect::Kirakira, 0xb8, 0x19, 0x29),
            (&happiness, "warukira.ppi", PathEffect::Warukira, 0xb8, 0x19, 0x29),
            (&happiness, "em_shy.ppi", PathEffect::Shy, 0x1b, 0x43, 0x6a),
            (&happiness, "steam.ppi", PathEffect::Steam, 0x1b, 0x43, 0x6a),
            (&innocence, "steam.ppi", PathEffect::Steam, 0x1b, 0x43, 0x6a),
            (&happiness, "wc_steam.ppi", PathEffect::SteamAligned, 0x1b, 0x43, 0x6a),
        ];
        let resources = crate::ResourceIndex::scan(&happiness)?;
        let extent = [32.; 2];
        let count = 12;
        let load = |bytes: &[u8]| ParticlePlugin::load(bytes, count, extent, ParticleCanvas::default());
        let mut parameters = [0; 128];
        for (index, value) in [80_f32, 50., 120., 5., 10., 80., 20., 30., 40., 50., 60., 70., 80.]
            .into_iter().enumerate()
        {
            parameters[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (game, file, effect, parameter, constructor_call, table_offset) in samples {
            let original = fs::read(game.join("ptclpi").join(file))?;
            let mut overlaid = original.clone();
            overlaid.extend_from_slice(b"unrelated PE overlay");
            let pe = ParticlePluginPe::read(&overlaid).unwrap();
            assert_eq!(plugin_layout::path_effect(&pe), Some(effect), "{file}");

            let mut original_plugin = load(&original)?;
            let mut changed = load(&overlaid)?;
            let mut original_random = ParticleRandom::default();
            let mut changed_random = original_random.clone();
            original_plugin.start(&parameters, [1280, 720], &resources, &mut original_random)?;
            changed.start(&parameters, [1280, 720], &resources, &mut changed_random)?;
            for tick in (0..=100).step_by(5) {
                original_plugin.update(tick, &parameters, &mut original_random);
                changed.update(tick, &parameters, &mut changed_random);
                assert_eq!(changed.snapshot(255).quads, original_plugin.snapshot(255).quads, "{file} at {tick}");
            }
            assert!(changed.capture_checkpoint()?.prepare(&original, count, extent).is_err(), "{file}");
            let mut restored = changed.capture_checkpoint()?.prepare(&overlaid, count, extent)?;
            assert_eq!(restored.snapshot(255).quads, changed.snapshot(255).quads, "{file}");
            restored.update(105, &parameters, &mut changed_random.clone());
            changed.update(105, &parameters, &mut changed_random);
            assert_eq!(restored.snapshot(255).quads, changed.snapshot(255).quads, "{file}");

            let ctor = pe.constructor_call(constructor_call).unwrap();
            let table = u32::from_le_bytes(ctor[table_offset..table_offset + 4].try_into()?);
            let callbacks = pe.callbacks(table).unwrap();
            let start_offset = callbacks[1].as_ptr() as usize - overlaid.as_ptr() as usize;
            let table_bytes = pe.read_only_data(table).unwrap();
            let registration = table_bytes.as_ptr() as usize - overlaid.as_ptr() as usize;

            let mut changed_parameter = overlaid.clone();
            changed_parameter[start_offset + parameter] ^= 1;
            assert!(load(&changed_parameter).is_err(), "changed start parameter accepted for {file}");
            let registered_update = overlaid[registration + 8..registration + 12].to_vec();
            overlaid[registration + 4..registration + 8].copy_from_slice(&registered_update);
            assert!(load(&overlaid).is_err(), "changed start callback accepted for {file}");
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed fire, thunder, and film noise plugins"]
    fn ppi_path_emitters_use_registered_record_sizes_and_counters() -> anyhow::Result<()> {
        use crate::particle_vm::ParticleRandom;
        use plugin_layout::PathEffect;

        let game = Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../games/Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}",
        );
        let resources = crate::ResourceIndex::scan(&game)?;
        let extent = [32.; 2];
        let load = |bytes: &[u8]| ParticlePlugin::load(bytes, 12, extent, ParticleCanvas::default());
        let samples = [
            ("hadou.ppi", Some(PathEffect::Hadou), 0x19, 0x29, 0x12),
            ("wc_thunder.ppi", Some(PathEffect::Thunder), 0x19, 0x29, 0x12),
            ("wc_firebase.ppi", Some(PathEffect::Firebase), 0x43, 0x65, 0x12),
            ("wc_firebase2.ppi", Some(PathEffect::Firebase2), 0x19, 0x29, 0x12),
            ("wc_firebase3.ppi", Some(PathEffect::Firebase3), 0x19, 0x29, 0x12),
            ("wc_firebase4.ppi", Some(PathEffect::Firebase4), 0x19, 0x29, 0x12),
            ("filmnoise.ppi", None, 0x43, 0x67, 0x17),
        ];
        let mut parameters = [0; 128];
        for (index, value) in [2_f32, 80., 100., 60., 50., 5., 20., 30., 40., 50., 60., 70., 80., 50., 5.]
            .into_iter().enumerate()
        {
            parameters[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (file, effect, constructor_call, table_offset, operand) in samples {
            let original = fs::read(game.join("ptclpi").join(file))?;
            let mut overlaid = original.clone();
            overlaid.extend_from_slice(b"unrelated PE overlay");
            let pe = ParticlePluginPe::read(&overlaid).unwrap();
            assert_eq!(plugin_layout::path_effect(&pe), effect, "{file}");
            if effect.is_none() {
                assert_eq!(plugin_layout::sprite_effect(&pe), Some(family::profiles::SpriteProfile::FilmNoise));
            }
            // Thunder uses the module's thread-local CRT RNG for its atlas.
            // Each run starts on a fresh thread to compare the same sequence.
            let run = |bytes: &[u8]| {
                std::thread::scope(|scope| scope.spawn(|| -> anyhow::Result<_> {
                    let mut plugin = load(bytes)?;
                    let mut random = ParticleRandom::default();
                    plugin.start(&parameters, [1280, 720], &resources, &mut random)?;
                    let mut snapshots = Vec::new();
                    for tick in (0..=100).step_by(5) {
                        plugin.update(tick, &parameters, &mut random);
                        snapshots.push(plugin.snapshot(255).quads);
                    }
                    if effect.is_none() {
                        let restored = plugin.capture_checkpoint()?.prepare(bytes, 12, extent)?;
                        assert_eq!(restored.snapshot(255).quads, plugin.snapshot(255).quads, "{file}");
                    } else {
                        assert!(plugin.capture_checkpoint().is_err(), "{file}");
                    }
                    Ok(snapshots)
                }).join().unwrap())
            };
            for (step, (original, changed)) in run(&original)?.into_iter().zip(run(&overlaid)?).enumerate() {
                assert!(original == changed, "{file} at tick {}", step * 5);
            }
            let constructor = pe.constructor_call(constructor_call).unwrap();
            let table = u32::from_le_bytes(constructor[table_offset..table_offset + 4].try_into()?);
            let start = pe.callbacks(table).unwrap()[1].as_ptr() as usize - overlaid.as_ptr() as usize;
            overlaid[start + operand] ^= 1;
            assert!(load(&overlaid).is_err(), "changed allocation accepted for {file}");
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed fireball, slash, volt, and windball plugins"]
    fn ppi_ribbons_use_registered_storage_and_spawn_parameters() -> anyhow::Result<()> {
        use crate::particle_vm::ParticleRandom;
        use plugin_layout::PathEffect;

        let game = Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../games/Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}",
        );
        let resources = crate::ResourceIndex::scan(&game)?;
        let extent = [32.; 2];
        let load = |bytes: &[u8]| ParticlePlugin::load(bytes, 12, extent, ParticleCanvas::default());
        let samples = [
            ("fireball.ppi", PathEffect::Fireball, 0x62, 0x1b, None),
            ("fireball2.ppi", PathEffect::Fireball2, 0x62, 0x1b, None),
            ("wc_slash.ppi", PathEffect::Slash, 0x58, 0x19, None),
            ("wc_volt.ppi", PathEffect::Volt, 0x62, 0x1b, Some(0x9c)),
            ("wc_volt2.ppi", PathEffect::Volt2, 0x62, 0x1b, Some(0x9c)),
            ("wc_volt3.ppi", PathEffect::Volt3, 0x62, 0x1b, Some(0x35)),
            ("wc_volt4.ppi", PathEffect::Volt4, 0x5d, 0x1b, None),
            ("wc_windball.ppi", PathEffect::Windball, 0x62, 0x1b, None),
        ];
        let mut parameters = [0; 128];
        for (index, value) in [2_f32, 80., 100., 60., 50., 5., 20., 30., 40., 50., 60., 70., 80., 50., 5.]
            .into_iter().enumerate()
        {
            parameters[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (file, effect, table_offset, stride_operand, spawn_operand) in samples {
            let original = fs::read(game.join("ptclpi").join(file))?;
            let mut overlaid = original.clone();
            overlaid.extend_from_slice(b"unrelated PE overlay");
            let pe = ParticlePluginPe::read(&overlaid).unwrap();
            assert_eq!(plugin_layout::path_effect(&pe), Some(effect), "{file}");

            let run = |bytes: &[u8]| {
                std::thread::scope(|scope| scope.spawn(|| -> anyhow::Result<_> {
                    let mut plugin = load(bytes)?;
                    let mut random = ParticleRandom::default();
                    plugin.start(&parameters, [1280, 720], &resources, &mut random)?;
                    let mut snapshots = Vec::new();
                    for tick in (0..=100).step_by(5) {
                        plugin.update(tick, &parameters, &mut random);
                        snapshots.push(plugin.snapshot(255).quads);
                    }
                    Ok(snapshots)
                }).join().unwrap())
            };
            let original_frames = run(&original)?;
            assert!(original_frames.iter().any(|frame| !frame.is_empty()), "{file}");
            for (step, (original, changed)) in original_frames.into_iter().zip(run(&overlaid)?).enumerate() {
                assert!(original == changed, "{file} at tick {}", step * 5);
            }

            let constructor = pe.constructor_call(0x43).unwrap();
            let table = u32::from_le_bytes(constructor[table_offset..table_offset + 4].try_into()?);
            let start = pe.callbacks(table).unwrap()[1].as_ptr() as usize - overlaid.as_ptr() as usize;
            if let Some(operand) = spawn_operand {
                let spawn = pe.update_call(table, 0x100).unwrap().as_ptr() as usize - overlaid.as_ptr() as usize;
                let mut changed_spawn = overlaid.clone();
                changed_spawn[spawn + operand] ^= 1;
                assert!(load(&changed_spawn).is_err(), "changed spawn parameter accepted for {file}");
            }
            overlaid[start + stride_operand] ^= 1;
            assert!(load(&overlaid).is_err(), "changed allocation accepted for {file}");
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed concent, dash, rising, paper and hotaru plugins"]
    fn ppi_inline_and_nested_paths_use_registered_record_layouts() -> anyhow::Result<()> {
        use crate::particle_vm::ParticleRandom;
        use plugin_layout::PathEffect;

        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let happiness = games.join("Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}");
        let innocence = games.join(concat!("\u{3053}\u{3053}\u{304b}\u{3089}\u{590f}\u{306e}",
            "\u{30a4}\u{30ce}\u{30bb}\u{30f3}\u{30b9}\u{ff01}"));
        let resources = crate::ResourceIndex::scan(&happiness)?;
        let extent = [32.; 2];
        let load = |bytes: &[u8]| ParticlePlugin::load(bytes, 12, extent, ParticleCanvas::default());
        let samples = [
            (&happiness, "concent.ppi", PathEffect::Concent, None, 0x39, 0x17),
            (&happiness, "dash.ppi", PathEffect::Dash, None, 0x35, 0x1c),
            (&happiness, "rising3.ppi", PathEffect::Rising3, None, 0x25, 0x13),
            (&happiness, "rising4.ppi", PathEffect::Rising4, None, 0x35, 0x12),
            (&happiness, "paper.ppi", PathEffect::Paper, None, 0x25, 0x14),
            (&innocence, "hotaru.ppi", PathEffect::Hotaru, Some(0x43), 0x6a, 0x15),
            (&innocence, "hotaru_move.ppi", PathEffect::HotaruMove, None, 0x7b, 0x17),
            (&innocence, "hotaru_stay.ppi", PathEffect::HotaruStay, None, 0x7b, 0x17),
        ];
        for (game, file, effect, constructor_call, table_offset, stride_operand) in samples {
            let original = fs::read(game.join("ptclpi").join(file))?;
            let mut overlaid = original.clone();
            overlaid.extend_from_slice(b"unrelated PE overlay");
            let pe = ParticlePluginPe::read(&overlaid).unwrap();
            assert_eq!(plugin_layout::path_effect(&pe), Some(effect), "{file}");

            let mut parameters = [0; 128];
            let mut values = [2_f32, 80., 100., 60., 50., 5., 20., 30., 40., 50., 60., 70., 80., 50., 5.];
            match effect {
                PathEffect::HotaruMove => values[5] = 2.,
                PathEffect::HotaruStay => values[6] = 2.,
                _ => {}
            }
            for (index, value) in values.into_iter().enumerate() {
                parameters[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
            }
            let run = |bytes: &[u8]| {
                std::thread::scope(|scope| scope.spawn(|| -> anyhow::Result<_> {
                    let mut plugin = load(bytes)?;
                    let mut random = ParticleRandom::default();
                    plugin.start(&parameters, [1280, 720], &resources, &mut random)?;
                    let mut snapshots = Vec::new();
                    for tick in (0..=100).step_by(5) {
                        plugin.update(tick, &parameters, &mut random);
                        snapshots.push(plugin.snapshot(255).quads);
                    }
                    match effect {
                        PathEffect::Concent | PathEffect::Dash | PathEffect::Rising3 | PathEffect::Rising4 | PathEffect::Hotaru => {
                            let restored = plugin.capture_checkpoint()?.prepare(bytes, 12, extent)?;
                            assert_eq!(restored.snapshot(255).quads, plugin.snapshot(255).quads, "{file}");
                        }
                        _ => assert!(plugin.capture_checkpoint().is_err(), "{file}"),
                    }
                    Ok(snapshots)
                }).join().unwrap())
            };
            // Concent and Dash consume counters shared by all plugin instances.
            let before_original = checkpoint::ModuleEmissionState::capture();
            let before_overlaid = checkpoint::ModuleEmissionState::capture();
            let original_frames = run(&original);
            before_overlaid.commit();
            let overlaid_frames = run(&overlaid);
            before_original.commit();
            let original_frames = original_frames?;
            assert!(original_frames.iter().any(|frame| !frame.is_empty()), "{file}");
            for (step, (original, changed)) in original_frames.into_iter().zip(overlaid_frames?).enumerate() {
                assert!(original == changed, "{file} at tick {}", step * 5);
            }

            let constructor = match constructor_call {
                Some(offset) => pe.constructor_call(offset),
                None => pe.constructor(),
            }.unwrap();
            let table = u32::from_le_bytes(constructor[table_offset..table_offset + 4].try_into()?);
            let start = pe.callbacks(table).unwrap()[1].as_ptr() as usize - overlaid.as_ptr() as usize;
            overlaid[start + stride_operand] ^= 1;
            assert!(load(&overlaid).is_err(), "changed allocation accepted for {file}");
        }
        Ok(())
    }

#[test]
    #[ignore = "requires installed SDK2 particle plugins with private pools"]
    fn ppi_pool_effects_use_registered_storage_and_generators() -> anyhow::Result<()> {
        use plugin_layout::PathEffect;
        use crate::particle_vm::ParticleRandom;

        let game = Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../games/Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}",
        );
        let resources = crate::ResourceIndex::scan(&game)?;
        let samples = [
            ("s_020402.ppi", PathEffect::Splash, 12, Some(0x44), 0x54, 0x1a),
            ("rubble.ppi", PathEffect::Rubble, 12, Some(0x44), 0x54, 0x1e),
            ("s_080103.ppi", PathEffect::Tiles, 4, None, 0x25, 0x12),
            ("exprode.ppi", PathEffect::Exprode, 8, None, 0x25, 0x12),
            ("star.ppi", PathEffect::Star, 28, None, 0x27, 0x19),
            ("wc_mirror.ppi", PathEffect::Mirror, 12, Some(0x19), 0x29, 0x12),
            ("wc_prism.ppi", PathEffect::PrismOrbit, 12, Some(0x19), 0x29, 0x12),
            ("protect.ppi", PathEffect::Protect, 26, Some(0x19), 0x2c, 0x15),
            ("wc_roundy.ppi", PathEffect::RoundY, 10, Some(0x19), 0x2c, 0x15),
            ("wc_roundz.ppi", PathEffect::RoundZ, 10, Some(0x19), 0x2c, 0x15),
        ];
        let mut parameters = [0; 128];
        for (index, value) in [60_f32, 80., 75., 90., 50., 60., 20., 30., 40., 50., 60., 70., 80., 50.]
            .into_iter().enumerate()
        {
            parameters[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (file, effect, count, constructor_call, table_offset, stride_operand) in samples {
            let original = fs::read(game.join("ptclpi").join(file))?;
            let mut overlaid = original.clone();
            overlaid.extend_from_slice(b"unrelated PE overlay");
            let pe = ParticlePluginPe::read(&overlaid).unwrap();
            assert_eq!(plugin_layout::path_effect(&pe), Some(effect), "{file}");
            let load = |bytes: &[u8]| ParticlePlugin::load(bytes, count, [32.; 2], ParticleCanvas::default());
            let mut original_plugin = load(&original)?;
            let mut overlaid_plugin = load(&overlaid)?;
            let mut original_random = ParticleRandom::default();
            let mut overlaid_random = original_random.clone();
            original_plugin.start(&parameters, [1280, 720], &resources, &mut original_random)?;
            overlaid_plugin.start(&parameters, [1280, 720], &resources, &mut overlaid_random)?;
            // Exprode's first update activates all eight records in either instance.
            for tick in (0..=100).step_by(5) {
                original_plugin.update(tick, &parameters, &mut original_random);
                overlaid_plugin.update(tick, &parameters, &mut overlaid_random);
                assert_eq!(original_plugin.snapshot(255).quads, overlaid_plugin.snapshot(255).quads, "{file} at {tick}");
            }

            let constructor = match constructor_call {
                Some(offset) => pe.constructor_call(offset),
                None => pe.constructor(),
            }.unwrap();
            let table = u32::from_le_bytes(constructor[table_offset..table_offset + 4].try_into()?);
            let start = pe.callbacks(table).unwrap()[1].as_ptr() as usize - overlaid.as_ptr() as usize;
            let generator = match effect {
                PathEffect::Mirror => pe.update_call(table, 0xce).map(|body| (body, 0x8a)),
                PathEffect::PrismOrbit => pe.update_call(table, 0xce).map(|body| (body, 0x89)),
                PathEffect::RoundY => pe.start_call(table, 0x13b).map(|body| (body, 0xb0)),
                PathEffect::RoundZ => pe.start_call(table, 0x13b).map(|body| (body, 0x9a)),
                _ => None,
            }.map(|(body, offset)| body.as_ptr() as usize - overlaid.as_ptr() as usize + offset);
            if let Some(offset) = generator {
                let mut edited = overlaid.clone();
                edited[offset] ^= 1;
                assert!(load(&edited).is_err(), "changed generator accepted for {file}");
            }
            overlaid[start + stride_operand] ^= 1;
            assert!(load(&overlaid).is_err(), "changed allocation accepted for {file}");
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed SDK2 particle plugins"]
    fn ppi_extended_pool_effects_use_registered_variants() -> anyhow::Result<()> {
        use plugin_layout::PathEffect;

        let game = Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../games/Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}/ptclpi",
        );
        let samples = [
            ("o_150102.ppi", PathEffect::StarLegacy, None, 0x27, 0x19),
            ("s_040102.ppi", PathEffect::PrismPath, None, 0x27, 0x1b),
            ("spark.ppi", PathEffect::Spark, None, 0x25, 0x19),
            ("spark_ex.ppi", PathEffect::SparkEx, None, 0x25, 0x19),
            ("erase.ppi", PathEffect::Erase, Some(0x44), 0x54, 0x1b),
            ("s_040101.ppi", PathEffect::EraseAtlas, Some(0x44), 0x54, 0x1b),
            ("s_080107.ppi", PathEffect::BaselineSpark, Some(0x44), 0x54, 0x1f),
            ("a_spark.ppi", PathEffect::ASpark, Some(0x44), 0x54, 0x1b),
            ("s_arrow.ppi", PathEffect::Arrow, None, 0x27, 0x12),
            ("fall_spark.ppi", PathEffect::FallSpark, None, 0x25, 0x15),
            ("radiate.ppi", PathEffect::Radiate, None, 0x39, 0x17),
            ("wc_seed.ppi", PathEffect::Seed, None, 0x39, 0x12),
            ("wc_seedrot.ppi", PathEffect::SeedRot, None, 0x39, 0x12),
            ("protect2.ppi", PathEffect::Protect2, None, 0x3c, 0x17),
        ];
        let load = |bytes: &[u8]| ParticlePlugin::load(bytes, 14, [32.; 2], ParticleCanvas::default());
        for (file, effect, constructor_call, table_offset, operand) in samples {
            let mut bytes = fs::read(game.join(file))?;
            assert!(load(&bytes).is_ok(), "{file}");
            bytes.extend_from_slice(b"unrelated PE overlay");
            let pe = ParticlePluginPe::read(&bytes).unwrap();
            assert_eq!(plugin_layout::path_effect(&pe), Some(effect), "{file}");
            assert!(load(&bytes).is_ok(), "{file}");
            let constructor = match constructor_call {
                Some(offset) => pe.constructor_call(offset),
                None => pe.constructor(),
            }.unwrap();
            let table = u32::from_le_bytes(constructor[table_offset..table_offset + 4].try_into()?);
            let start = pe.callbacks(table).unwrap()[1].as_ptr() as usize - bytes.as_ptr() as usize;
            bytes[start + operand] ^= 1;
            assert!(load(&bytes).is_err(), "changed allocation accepted for {file}");
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed reaction effect plugins"]
    fn ppi_reaction_effects_use_registered_private_records_and_phases() -> anyhow::Result<()> {
        use family::profiles::SpriteProfile;
        use crate::particle_vm::ParticleRandom;

        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let happiness = games.join("Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}");
        let neko = games.join(concat!(
            "[170526][NEKO WORKs] \u{30cd}\u{30b3}\u{3071}\u{3089} vol.3 ",
            "steam\u{7248} (files)/nekopara_vol3",
        ));
        let samples = [
            (&happiness, "asease.ppi", SpriteProfile::Asease, 0x17),
            (&happiness, "em_common.ppi", SpriteProfile::Common, 0x17),
            (&happiness, "em_dis.ppi", SpriteProfile::Dis, 0x17),
            (&happiness, "em_angry.ppi", SpriteProfile::Angry, 0x17),
            (&neko, "em_angry.ppi", SpriteProfile::Angry, 0x17),
            (&happiness, "em_notice.ppi", SpriteProfile::Notice, 0x12),
            (&happiness, "em_suprise.ppi", SpriteProfile::Surprise, 0x12),
            (&happiness, "em_kyupin.ppi", SpriteProfile::Kyupin, 0x12),
            (&happiness, "em_silent.ppi", SpriteProfile::Silent, 0x12),
            (&happiness, "em_down.ppi", SpriteProfile::Down, 0x14),
            (&happiness, "em_exclamation.ppi", SpriteProfile::Exclamation, 0x1c),
            (&happiness, "ohanaburst.ppi", SpriteProfile::FlowerBurst, 0x14),
            (&happiness, "ring.ppi", SpriteProfile::Ring, 0x12),
            (&happiness, "digi.ppi", SpriteProfile::Digital { orbit: false }, 0x12),
            (&happiness, "digiring.ppi", SpriteProfile::Digital { orbit: true }, 0x17),
            (&happiness, "em_double.ppi", SpriteProfile::Double { shake_by_index: false, loops: true }, 0x22),
            (&neko, "em_double.ppi", SpriteProfile::Double { shake_by_index: true, loops: false }, 0x22),
        ];
        let resources = crate::ResourceIndex::scan(&happiness)?;
        let extent = [32.; 2];
        let count = 4;
        let load = |bytes: &[u8]| ParticlePlugin::load(bytes, count, extent, ParticleCanvas::default());
        let mut parameters = [0; 128];
        for (index, value) in [2_f32, 30., 120., 15., 50., 80., 20., 30., 40., 50., 60., 70., 80.]
            .into_iter().enumerate()
        {
            parameters[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (game, file, effect, operand) in samples {
            let original = fs::read(game.join("ptclpi").join(file))?;
            let mut overlaid = original.clone();
            overlaid.extend_from_slice(b"unrelated PE overlay");
            let pe = ParticlePluginPe::read(&overlaid).unwrap();
            assert_eq!(plugin_layout::sprite_effect(&pe), Some(effect), "{file}");
            let mut original_plugin = load(&original)?;
            let mut plugin = load(&overlaid)?;
            let mut original_random = ParticleRandom::default();
            let mut random = original_random.clone();
            original_plugin.start(&parameters, [1280, 720], &resources, &mut original_random)?;
            plugin.start(&parameters, [1280, 720], &resources, &mut random)?;
            for tick in (0..=100).step_by(5) {
                original_plugin.update(tick, &parameters, &mut original_random);
                plugin.update(tick, &parameters, &mut random);
                assert_eq!(plugin.snapshot(255).quads, original_plugin.snapshot(255).quads, "{file} at {tick}");
            }
            assert!(plugin.capture_checkpoint()?.prepare(&original, count, extent).is_err(), "{file}");
            let mut restored = plugin.capture_checkpoint()?.prepare(&overlaid, count, extent)?;
            restored.update(105, &parameters, &mut random.clone());
            plugin.update(105, &parameters, &mut random);
            assert_eq!(restored.snapshot(255).quads, plugin.snapshot(255).quads, "{file}");

            let ctor = pe.constructor_call(0x19).unwrap();
            let table = u32::from_le_bytes(ctor[0x29..0x2d].try_into()?);
            let start = pe.callbacks(table).unwrap()[1].as_ptr() as usize - overlaid.as_ptr() as usize;
            let mut edited = overlaid.clone();
            edited[start + operand] ^= 1;
            assert!(load(&edited).is_err(), "changed allocation accepted for {file}");
            let registration = pe.read_only_data(table).unwrap().as_ptr() as usize - overlaid.as_ptr() as usize;
            let registered_finish = overlaid[registration + 12..registration + 16].to_vec();
            overlaid[registration + 8..registration + 12].copy_from_slice(&registered_finish);
            assert!(load(&overlaid).is_err(), "changed update callback accepted for {file}");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
struct RainDrop {
    position: [f32; 3],
    velocity: [f32; 3],
    rotation: f32,
}

#[derive(Debug)]
pub(super) struct ParticlePlugin {
    digest: String,
    drops: Vec<RainDrop>,
    field: Option<super::particle_rain_field::RainField>,
    burst: Option<super::particle_burst::ParticleBurst>,
    family: Option<family::Family>,
    common: Option<common::CommonParticles>,
    source_position: [f32; 3],
    image_size: [f32; 2],
    canvas: ParticleCanvas,
    active: bool,
    initialized: bool,
    origin: u32,
    previous_time: u32,
    elapsed: u32,
    previous_elapsed: u32,
    submitted: bool,
}

impl ParticlePlugin {
    pub(super) fn load(bytes: &[u8], count: u32, image_size: [f32; 2], canvas: ParticleCanvas) -> Result<Self> {
        let pe = ParticlePluginPe::read(bytes)
            .ok_or_else(|| super::native_state_error("invalid particle plugin PE or SDK export family"))?;
        if pe.version() == 0x10000 {
            return Err(super::native_state_error("fixed SDK2 particle host rejects type1/version0x10000 before MakeInstance"));
        }
        if !matches!(pe.version(), 0x20000..=0x20002) {
            return Err(super::native_state_error("fixed SDK2 particle host rejects the plugin version"));
        }
        let layout = plugin_layout::detect(&pe);
        // Keep the source identity for checkpoints with exact resource binding.
        let digest = super::particle_plugin_digest::sha256(bytes);
        let field = match layout {
            Some(Layout::AtlasRain) => Some(super::particle_rain_field::RainFieldKind::Atlas),
            Some(Layout::ScaledRain) => Some(super::particle_rain_field::RainFieldKind::Scaled),
            _ => None,
        };
        let burst = layout == Some(Layout::Burst);
        let common = layout == Some(Layout::Common);
        let described = layout.is_none() && family::Family::uses_family(&pe, &digest);
        if layout.is_none() && !described {
            return Err(super::native_state_error("particle plugin has no independent model for its SDK callbacks"));
        }
        // Include SDK/current/saved/render snapshots in the allocation bound.
        let bytes_per_particle = if described || common { 2048 } else { 512 };
        if (count as usize).checked_mul(bytes_per_particle).is_none_or(|n| n > MAX_STORAGE) {
            return Err(super::native_state_error("particle plugin grid exceeds64 MiB"));
        }
        let family = if described { family::Family::load(&pe, &digest, count, image_size)? } else { None };
        Ok(Self { digest, drops: if field.is_none() && !burst && !described && !common { vec![RainDrop::default(); count as usize] } else { Vec::new() },
            family, source_position: [0.;3],
            common:common.then(||common::CommonParticles::new(count,image_size)),
            burst: burst.then(|| super::particle_burst::ParticleBurst::new(count,image_size)),
            field: field.map(|kind| super::particle_rain_field::RainField::new(kind,count,image_size)), image_size, canvas,
            active: false, initialized: false, origin: 0, previous_time: 0, elapsed: 0,
            previous_elapsed: u32::MAX, submitted: false })
    }

    pub(super) fn start(&mut self, parameters: &[u8; 128], viewport: [u32; 2], resources:&super::ResourceIndex,
        random: &mut impl ParticleRandomSource) -> Result<()> {
        //14ee4f queries the actual canvas at start, not the plugin filename or
        // a synthetic default. A resize between load/start is observable.
        self.canvas.viewport = viewport;
        self.origin = 0; self.previous_time = 0;
        self.elapsed = 0; self.previous_elapsed = u32::MAX; self.submitted = false;
        if let Some(common) = self.common.as_mut() {common.start(parameters,viewport,resources,random)?;}
        else if let Some(family) = self.family.as_mut() { family.start(parameters,viewport,resources,random)?; }
        else if let Some(burst) = self.burst.as_mut() { burst.start(parameters,random)?; }
        else if let Some(field) = self.field.as_mut() { field.start(parameters,viewport,random); }
        else { for drop in &mut self.drops {
            *drop = RainDrop::default();
            respawn(drop, self.canvas.viewport, parameters, random);
        } }
        self.active = true; self.initialized = true;
        Ok(())
    }

    // SDK finish (rain RVA12f0) is a real no-op; storage is retained until
    // release/reload destroys this object. Stop still removes the draw callback.
    pub(super) fn stop(&mut self) { self.active = false; }
    pub(super) fn active(&self) -> bool { self.active }

    //14e5f0 creates a fresh instance and initializes canonical quads; it does
    //NOT call SDK start. Preserve the task's existing active bit on reload
    //without inventing a start callback or consuming any random samples.
    pub(super) fn retain_loaded_active(&mut self, active: bool) { self.active = active; }

    //14f490 fills the SDK source before each update; +24/+28/+2c are the
    //source's3D coordinates. Explicit detach zeros them; a failed lookup
    //retains the last values, matching the same portable source service as VM.
    pub(super) fn refresh_source(&mut self, id: i32, source: Option<super::particle_vm::ParticleSource>) {
        if id == -1 { self.source_position = [0.;3]; }
        else if let Some(source) = source { self.source_position = source.position_3d; }
    }

    pub(super) fn reset_extent(&mut self, image_size: [f32; 2]) -> Result<()> {
        if self.active { return Err(super::native_state_error("bind plugin extent before start")); }
        self.image_size = image_size; self.submitted = false;
        if let Some(field) = self.field.as_mut() { field.reset_extent(image_size); }
        if let Some(burst) = self.burst.as_mut() { burst.reset_extent(image_size); }
        if let Some(family) = self.family.as_mut() { family.reset_extent(image_size); }
        if let Some(common) = self.common.as_mut() {common.reset_extent(image_size);}
        Ok(())
    }

    pub(super) fn update(&mut self, now: u32, parameters: &[u8; 128], random: &mut impl ParticleRandomSource) {
        if !self.active { return; }
        // Same DWORD task clock/sentinel as14e424. First update has delta1,
        // including when the scheduler's actual tick is0; never substitute ms.
        if self.origin == 0 {
            self.origin = now; self.elapsed = 0; self.previous_elapsed = u32::MAX;
        } else { self.elapsed = self.elapsed.wrapping_add(now.wrapping_sub(self.previous_time)); }
        self.previous_time = now;
        let delta = self.elapsed.wrapping_sub(self.previous_elapsed) as f32;
        if !self.initialized { self.previous_elapsed = self.elapsed; return; }
        if let Some(common) = self.common.as_mut() {
            common.update(delta,self.source_position,random);
            self.previous_elapsed=self.elapsed;self.submitted=true;return;
        }
        if let Some(family) = self.family.as_mut() {
            family.update(delta,parameters,self.source_position,random);
            //SDK04 clears active in the owning task, including before its
            //afterimage pool drains. Storage still belongs to release/reload.
            if family.finish_requested() {self.active=false;}
            self.previous_elapsed = self.elapsed; self.submitted = true; return;
        }
        if let Some(burst) = self.burst.as_mut() {
            burst.update(delta,parameters);
            self.previous_elapsed = self.elapsed; self.submitted = true; return;
        }
        if let Some(field) = self.field.as_mut() {
            field.update(delta,random);
            self.previous_elapsed = self.elapsed; self.submitted = true; return;
        }
        let limit = self.canvas.viewport[1] as f64 + 200.;
        for drop in &mut self.drops {
            for axis in 0..3 {
                drop.position[axis] = (f64::from(drop.velocity[axis]) * f64::from(delta)
                    + f64::from(drop.position[axis])) as f32;
            }
            if f64::from(drop.position[1]) > limit { respawn(drop, self.canvas.viewport, parameters, random); }
        }
        self.previous_elapsed = self.elapsed; self.submitted = true;
    }

    pub(super) fn snapshot(&self, opacity: u32) -> ParticleRenderSnapshot {
        if let Some(common) = self.common.as_ref() {
            return common.snapshot(self.canvas,self.active && self.initialized,self.submitted,self.elapsed,self.previous_elapsed,opacity);
        }
        if let Some(family) = self.family.as_ref() {
            return family.snapshot(self.canvas,self.active && self.initialized,self.submitted,self.elapsed,self.previous_elapsed,opacity);
        }
        if let Some(burst) = self.burst.as_ref() {
            return burst.snapshot(self.canvas,self.active && self.initialized,self.submitted,self.elapsed,self.previous_elapsed,opacity);
        }
        if let Some(field) = self.field.as_ref() {
            return field.snapshot(self.canvas,self.active && self.initialized,self.submitted,self.elapsed,self.previous_elapsed,opacity);
        }
        let [w,h] = self.image_size;
        let positions = [[0.,0.],[w,0.],[0.,h],[w,h]];
        let uv = [[0.,0.],[1.,0.],[0.,1.],[1.,1.]];
        ParticleRenderSnapshot { active: self.active && self.initialized, frame_complete: true,
            elapsed: self.elapsed, previous_elapsed: self.previous_elapsed,
            current_quad: self.drops.len() as u32, render_mode: 0, opacity, canvas: self.canvas,
            quads: self.drops.iter().enumerate().map(|(i, drop)| {
                //14e200 subtracts pivot, then scale/XYZ rotations/translation.
                // Rain changes only Z. Bake that matrix into the vertices and
                // leave ordinary projector offsets zero (it subtracts them).
                // MatrixRotationZ stores its sin/cos as float before SSE
                // matrix multiplication (Yuki33af6..33b71).
                let (sin,cos) = f64::from(drop.rotation).sin_cos();
                let (sin,cos) = (sin as f32,cos as f32);
                let vertices = std::array::from_fn(|n| {
                    let [mut x,mut y] = positions[n];
                    if self.submitted {
                        x -= w * 0.5; y -= h * 0.5;
                        let a = x; let b = y;
                        x = a * cos - b * sin;
                        y = a * sin + b * cos;
                        x += drop.position[0]; y += drop.position[1];
                    }
                    ParticleVertex { position: [x,y,if self.submitted { drop.position[2] } else { 0. }],
                        uv: uv[n], ..Default::default() }
                });
                ParticleQuad { index: i as u32, vertices, offset: [0.;4], depth_bias:0., flag: 1 }
            }).collect() }
    }
}

fn respawn(drop: &mut RainDrop, viewport: [u32;2], parameters: &[u8;128], random: &mut impl ParticleRandomSource) {
    let sample = |r: &mut _| f64::from(f32::from_bits(ParticleRandomSource::next_float_bits(r)));
    drop.position[0] = (2. * f64::from(viewport[0]) * sample(random) - 0.5 * f64::from(viewport[0])) as f32;
    drop.position[1] = (-2. * f64::from(viewport[1]) * sample(random)) as f32;
    let word = |i: usize| f32::from_le_bytes(parameters[i * 4..i * 4 + 4].try_into().unwrap());
    let radians = |degrees: f32| (f64::from(degrees) * f64::from(std::f32::consts::PI) / 180.) as f32;
    let min = radians(word(1)); let max = radians(word(2));
    let angle = (f64::from(min) + sample(random) * (f64::from(max) - f64::from(min))) as f32;
    let speed = word(0); let (sin,cos) = f64::from(angle).sin_cos();
    let (sin,cos) = (sin as f32,cos as f32);
    // SDK callback+5c rotates clockwise. The plugin argument slot is reused:
    //1070:1141/114f and1390:13c6/13d4 read NEGATIVE ANGLE, not speed.
    drop.velocity = [speed * sin, speed * cos, 0.];
    drop.rotation = -angle;
}
