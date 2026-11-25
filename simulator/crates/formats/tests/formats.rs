use cs2_formats::anm::AnmVersion;
use cs2_formats::{
    AnmFile, Archive, ConfigXml, CssError, CssFile, CstFile, FesFile, HgError, HgFile, KcsFile,
    OggVorbis, OggVorbisError, WavPcm, WavPcmError, WssSource,
};
use flate2::{Compression, write::ZlibEncoder};
use image::{ColorType, codecs::jpeg::JpegEncoder, codecs::webp::WebPEncoder};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

fn workspace_root() -> PathBuf {
    if let Some(root) = std::env::var_os("CS2_SAMPLE_ROOT").map(PathBuf::from) {
        let catalog = root.join("games");
        return if catalog.is_dir() { catalog } else { root };
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../games")
}

fn sample_file(parts: &[&str]) -> Option<Vec<u8>> {
    let path = parts
        .iter()
        .fold(workspace_root(), |path, part| path.join(part));
    fs::read(path).ok()
}

#[test]
fn decodes_real_ogg_vorbis_samples_and_rejects_truncation() {
    for parts in [
        &["cs2_full_v401", "system", "sound", "se", "se001.ogg"][..],
        &["牛顿与苹果树", "A_ali_h03_013_cut.ogg"][..],
    ] {
        let Some(bytes) = sample_file(parts) else {
            continue;
        };
        let metadata = OggVorbis::metadata(&bytes).unwrap();
        assert!(metadata.sample_rate > 0);
        assert!(metadata.channels > 0);

        let pcm_i16 = OggVorbis::decode_i16(&bytes).unwrap();
        assert_eq!(pcm_i16.metadata, metadata);
        assert!(pcm_i16.frames > 0);
        assert_eq!(
            pcm_i16.samples.len(),
            pcm_i16.frames * usize::from(metadata.channels)
        );
        assert!(pcm_i16.samples.iter().any(|&sample| sample != 0));

        let pcm_f32 = OggVorbis::decode_f32(&bytes).unwrap();
        assert_eq!(pcm_f32.metadata, metadata);
        assert_eq!(pcm_f32.frames, pcm_i16.frames);
        assert_eq!(
            pcm_f32.samples.len(),
            pcm_f32.frames * usize::from(metadata.channels)
        );
        assert!(pcm_f32.samples.iter().any(|&sample| sample != 0.0));
        assert!(pcm_f32.samples.iter().all(|sample| sample.is_finite()));

        assert!(matches!(
            OggVorbis::metadata(&bytes[..bytes.len() - 1]),
            Err(OggVorbisError::Truncated)
        ));
        assert!(matches!(
            OggVorbis::decode_i16(&bytes[..bytes.len() - 1]),
            Err(OggVorbisError::Truncated)
        ));
    }
}

#[test]
fn ogg_vorbis_reports_bad_capture_and_short_header() {
    assert!(matches!(
        OggVorbis::metadata(b"OggS"),
        Err(OggVorbisError::Truncated)
    ));
    assert!(matches!(
        OggVorbis::metadata(b"Not an Ogg stream, but long enough to check the capture pattern"),
        Err(OggVorbisError::InvalidOgg)
    ));
}

#[test]
fn parses_and_rejects_structured_wav_pcm() {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&38u32.to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&22050u32.to_le_bytes());
    bytes.extend_from_slice(&44100u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&(-123i16).to_le_bytes());
    let wav = WavPcm::parse(&bytes).unwrap();
    assert_eq!(wav.metadata.sample_rate, 22050);
    assert_eq!(wav.samples, [-123]);

    let mut bad = bytes;
    bad[20..22].copy_from_slice(&3u16.to_le_bytes());
    assert_eq!(WavPcm::parse(&bad), Err(WavPcmError::UnsupportedFormat));
}

#[test]
fn parses_real_newton_cst() {
    let Some(bytes) = sample_file(&["牛顿与苹果树", "ali_001A.cst"]) else {
        return;
    };
    let file = CstFile::parse(&bytes).unwrap();
    assert!(!file.records.is_empty());
    assert!(!file.inputs.is_empty());
    assert_eq!(file.inputs.last().unwrap().end, file.records.len());
}

#[test]
fn parses_real_newton_kcs() {
    let Some(bytes) = sample_file(&["牛顿与苹果树", "kcs", "sscript.kcs"]) else {
        return;
    };
    let file = KcsFile::parse(&bytes).unwrap();
    assert_eq!(file.header.version, 2);
    assert_eq!(file.header.code_length as usize, file.code.len());
    assert!(!file.instructions.is_empty());
}

#[test]
fn parses_real_update05_cst_with_inner_declared_script_length() {
    let Some(bytes) = sample_file(&["突然之间发现我已恋上你", "UPDATE05.int"]) else {
        return;
    };
    let archive = Archive::parse_with_name_seed(&bytes, 0x151a_b7df).unwrap();
    let cst_bytes = archive.read(&bytes, b"eika_ind01.cst").unwrap().unwrap();
    let file = CstFile::parse(&cst_bytes).unwrap();
    assert_eq!(
        file.variant,
        cs2_formats::cst::CstVariant::ZlibDeclaredLengthMismatch
    );
    assert!(
        file.diagnostics
            .contains(&cs2_formats::cst::CstDiagnostic::ScriptLengthMismatch(
                cs2_formats::cst::CstLengthInfo {
                    declared: file.script_length.declared,
                    actual: file.script_length.actual,
                }
            ))
    );
    assert!(!file.records.is_empty());
    assert_eq!(
        file.inputs.last().map(|range| range.end),
        Some(file.records.len())
    );
}

#[test]
fn restores_every_real_grisaia_update01_cst_including_overwritten_keys() {
    let Some(bytes) = sample_file(&[
        "灰色的果实",
        "UPDATE01.INT",
    ]) else {
        return;
    };
    let archive = Archive::parse_with_name_seed(&bytes, 0x1dad_9120).unwrap();
    let mut count = 0;
    let mut recovered_keys = Vec::new();
    let mut records = 0;
    let mut inputs = 0;
    for entry in &archive.entries {
        if !entry.name.ends_with(b".cst") {
            continue;
        }
        let cst_bytes = archive.read_entry(&bytes, entry).unwrap();
        let file = CstFile::parse(&cst_bytes).unwrap();
        if file.variant == cs2_formats::cst::CstVariant::XorObfuscatedZlibRecoveredKey {
            recovered_keys.push(entry.name.clone());
        } else {
            assert_eq!(file.variant, cs2_formats::cst::CstVariant::XorObfuscatedZlib);
        }
        assert_eq!(file.length.declared, file.length.actual);
        assert_eq!(file.script_length.declared, file.script_length.actual);
        assert_eq!(
            file.inputs.last().map(|range| range.end),
            Some(file.records.len())
        );
        count += 1;
        records += file.records.len();
        inputs += file.inputs.len();
    }
    assert_eq!((count, records, inputs), (290, 550108, 64525));
    recovered_keys.sort();
    assert_eq!(
        recovered_keys,
        [b"mic_016a.cst".to_vec(), b"opmovie.cst".to_vec()]
    );

    for name in [b"ama_002b.cst".as_slice(), b"mic_016a.cst", b"opmovie.cst"] {
        let mut damaged = archive.read(&bytes, name).unwrap().unwrap();
        *damaged.last_mut().unwrap() ^= 1;
        assert!(CstFile::parse(&damaged).is_err());
    }
}

#[test]
fn parses_all_paradise_and_update05_csts_with_explicit_variant_diagnostics() {
    for (path, expected_count, expected_records, expected_inputs, expected_mismatches, expected_missing)
        in [
        (&["灰色的乐园v1", "scene.int"][..], 136, 177837, 23141, 0, 2),
        (&["突然之间发现我已恋上你", "UPDATE05.int"][..], 156, 148865, 30488, 144, 0),
    ] {
        let Some(bytes) = sample_file(path) else {
            continue;
        };
        let archive = Archive::parse(&bytes).unwrap();
        let mut totals = (0, 0, 0, 0, 0);
        for entry in &archive.entries {
            if !entry.name.ends_with(b".cst") {
                continue;
            }
            let cst_bytes = archive.read_entry(&bytes, entry).unwrap();
            let file = CstFile::parse(&cst_bytes).unwrap();
            totals.0 += 1;
            totals.1 += file.records.len();
            totals.2 += file.inputs.len();
            totals.3 += usize::from(file.script_length.declared != file.script_length.actual);
            totals.4 += usize::from(
                file.diagnostics
                    .contains(&cs2_formats::cst::CstDiagnostic::MissingAdler32Zero),
            );
            assert_eq!(file.length.declared, file.length.actual);
        }
        assert_eq!(
            totals,
            (expected_count, expected_records, expected_inputs, expected_mismatches, expected_missing)
        );
    }
}

#[test]
fn parses_real_newton_hg3_metadata() {
    let Some(bytes) = sample_file(&["牛顿与苹果树", "boot.hg3"]) else {
        return;
    };
    let file = HgFile::parse(&bytes).unwrap();
    assert_eq!(&file.signature, b"HG-3");
    assert_eq!(file.frames.len(), 1);
    assert_eq!((file.frames[0].width, file.frames[0].height), (352, 226));
    assert!(file.frames[0].image.is_some());
}

#[test]
fn accepts_zero_padding_after_hg3_tags_and_rejects_nonzero_padding() {
    let zero_commands = [0b0000_0011, 0b0000_0011];
    let mut padded = synthetic_hg3(1, 1, 32, &[], &zero_commands);
    padded.extend_from_slice(&[0; 7]);
    assert_eq!(HgFile::parse(&padded).unwrap().frames.len(), 1);

    let last = padded.len() - 1;
    padded[last] = 1;
    assert!(matches!(
        HgFile::parse(&padded),
        Err(cs2_formats::HgError::InvalidFrame)
    ));
}

#[test]
fn restricts_extended_hg3_padding_to_aligned_cptype() {
    let make_frame = |tag, padding_len: usize, alignment_shift| {
        let mut bytes = synthetic_hg3_with_tags(&[
            (*b"stdinfo\0", synthetic_stdinfo(1, 1)),
            (*b"custom\0\0", vec![0; 16 - padding_len + alignment_shift]),
            (tag, vec![0; 4]),
        ]);
        bytes.resize(bytes.len() + padding_len, 0);
        assert_eq!(bytes.len() % 8, alignment_shift);
        bytes
    };
    let append_frame = |bytes: &mut Vec<u8>| {
        let next_frame = u32::try_from(bytes.len() - 12).unwrap();
        bytes[12..16].copy_from_slice(&next_frame.to_le_bytes());
        let next = synthetic_hg3_with_tags(&[(*b"stdinfo\0", synthetic_stdinfo(1, 1))]);
        bytes.extend_from_slice(&next[12..]);
    };

    let cptype = *b"cptype\0\0";
    for padding_len in 8..=15 {
        let mut bytes = make_frame(cptype, padding_len, 0);
        assert_eq!(HgFile::parse(&bytes).unwrap().frames.len(), 1);
        let next_frame_offset = bytes.len();
        append_frame(&mut bytes);
        let file = HgFile::parse(&bytes).unwrap();
        assert_eq!(file.frames.len(), 2);
        assert_eq!(file.frames[1].frame_offset, next_frame_offset as u64);
    }

    for (case, tag, padding_len, alignment_shift, followed_by_frame, nonzero) in [
        ("oversized inter-frame padding", cptype, 16, 0, true, false),
        ("different tag", *b"unknown\0", 9, 0, true, false),
        ("unaligned next frame", cptype, 9, 1, true, false),
        ("oversized file-end padding", cptype, 16, 0, false, false),
        ("different tag at file end", *b"unknown\0", 9, 0, false, false),
        ("unaligned file end", cptype, 9, 1, false, false),
        ("nonzero padding", cptype, 9, 0, true, true),
        ("nonzero file end", cptype, 9, 0, false, true),
    ] {
        let mut bytes = make_frame(tag, padding_len, alignment_shift);
        if nonzero {
            *bytes.last_mut().unwrap() = 1;
        }
        if followed_by_frame {
            append_frame(&mut bytes);
        }
        assert!(
            matches!(HgFile::parse(&bytes), Err(HgError::InvalidFrame)),
            "{case}"
        );
    }
}

#[test]
fn decodes_kanmi_hg3_message_window_frame_after_cptype_padding() {
    let Some(bytes) = sample_file(&[
        "\u{5c1a}\u{6709}\u{4f73}\u{871c}\u{4f34}\u{604b}\u{5fc3}",
        "image",
        "sys_meswnd.hg3",
    ]) else {
        assert!(
            std::env::var_os("CS2_SAMPLE_ROOT").is_none(),
            "Kanmi HG3 fixture is absent"
        );
        return;
    };
    let file = HgFile::parse(&bytes).unwrap();
    assert_eq!(file.frames[0].id, 1406);
    assert_eq!(file.frames[1].frame_offset, 2072);
    let (index, frame) = file
        .frames
        .iter()
        .enumerate()
        .find(|(_, frame)| frame.id == 30)
        .unwrap();
    assert_eq!(
        (frame.frame_offset, frame.width, frame.height),
        (104920, 312, 260)
    );
    let pixels = file.decode_frame_rgba8(&bytes, index).unwrap();
    assert_eq!(pixels.len(), 312 * 260 * 4);
    assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
}

#[test]
fn indexes_real_encrypted_kif_archives() {
    for parts in [
        &["牛顿与苹果树", "config.int"][..],
        &[
            "[170526][NEKO WORKs] ネコぱら vol.3 steam版 (files)",
            "nekopara_vol3",
            "config.int",
        ][..],
    ] {
        let Some(bytes) = sample_file(parts) else {
            continue;
        };
        let archive = Archive::parse_raw(&bytes).unwrap();
        assert!(archive.is_encrypted());
        assert_eq!(archive.layout(), cs2_formats::ArchiveLayout::Name64);
        assert!(!archive.entries.is_empty());
        assert_eq!(archive.entries[0].name, b"__key__.dat");
    }
}

#[test]
fn indexes_tsunaban_legacy_kif_archive_directories() {
    let game_dir = workspace_root().join("ツナバン らぶみくす");
    if !game_dir.is_dir() {
        return;
    }

    let mut paths: Vec<_> = fs::read_dir(&game_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("int"))
        })
        .collect();
    paths.sort();
    assert_eq!(paths.len(), 22);

    for path in paths {
        let mut file = fs::File::open(&path).unwrap();
        let archive_length = file.metadata().unwrap().len();
        let mut header = [0; 8];
        file.read_exact(&mut header).unwrap();
        assert_eq!(&header[..4], b"KIF\0", "{}", path.display());
        let entry_count = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
        let table_length = entry_count
            .checked_mul(72)
            .and_then(|length| length.checked_add(8))
            .unwrap();
        let mut directory = vec![0; table_length];
        file.seek(SeekFrom::Start(0)).unwrap();
        file.read_exact(&mut directory).unwrap();

        let archive = Archive::parse_directory(&directory, archive_length, Some(0))
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        assert!(archive.is_encrypted(), "{}", path.display());
        assert_eq!(
            archive.layout(),
            cs2_formats::ArchiveLayout::Name32,
            "{}",
            path.display()
        );
        assert_eq!(archive.entries.len(), entry_count - 1, "{}", path.display());
        let legacy_table_end = (8 + entry_count * 40) as u64;
        assert!(
            archive.entries.iter().all(|entry| {
                entry.offset >= legacy_table_end
                    && entry
                        .offset
                        .checked_add(entry.size)
                        .is_some_and(|end| end <= archive_length)
            }),
            "{}",
            path.display()
        );
    }
}

#[test]
fn legacy_vcode_seeds_read_real_main_kcs_payloads() {
    for (directory, code, layout) in [
        (
            "うたてめぐり", b"UTATEMEGURI".as_slice(),
            cs2_formats::ArchiveLayout::Name64,
        ),
        (
            "ツナバン らぶみくす", b"TSUNABAN-LM".as_slice(),
            cs2_formats::ArchiveLayout::Name32,
        ),
        (
            "祝福のカンパネラ", b"CAMPANELLA".as_slice(),
            cs2_formats::ArchiveLayout::Name64,
        ),
    ] {
        let Some(bytes) = sample_file(&[directory, "kcs.int"]) else {
            continue;
        };
        let seed = cs2_formats::discover_archive_name_seed(None, Some(code), None).unwrap();
        assert_eq!(seed.version, cs2_formats::ArchiveSeedVersion::VCode);
        let archive = Archive::parse_with_name_seed(&bytes, seed.value).unwrap();
        assert_eq!(archive.layout(), layout);
        let main = archive.read(&bytes, b"main.kcs").unwrap().unwrap();
        let program = KcsFile::parse(&main).unwrap();
        assert!(!program.instructions.is_empty());
    }
}

#[test]
fn reads_plain_kif_entry() {
    let mut bytes = b"KIF\0".to_vec();
    bytes.extend_from_slice(&1u32.to_le_bytes());
    let mut name = [0; 64];
    name[..10].copy_from_slice(b"sample.cst");
    bytes.extend_from_slice(&name);
    bytes.extend_from_slice(&80u32.to_le_bytes());
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(b"data");

    let archive = Archive::parse(&bytes).unwrap();
    assert_eq!(
        archive.read(&bytes, b"SAMPLE.CST").unwrap(),
        Some(b"data".to_vec())
    );
    assert_eq!(archive.read(&bytes, b"missing.cst").unwrap(), None);
}

#[test]
fn reads_plain_legacy_32_byte_kif_when_layout_is_explicit() {
    let mut bytes = b"KIF\0".to_vec();
    bytes.extend_from_slice(&1u32.to_le_bytes());
    let mut name = [0; 32];
    name[..10].copy_from_slice(b"sample.cst");
    bytes.extend_from_slice(&name);
    bytes.extend_from_slice(&48u32.to_le_bytes());
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(b"data");

    let archive = Archive::parse_with_layout(&bytes, cs2_formats::ArchiveLayout::Name32).unwrap();
    assert_eq!(archive.layout(), cs2_formats::ArchiveLayout::Name32);
    assert_eq!(
        archive.read(&bytes, b"SAMPLE.CST").unwrap(),
        Some(b"data".to_vec())
    );
}

#[test]
fn rejects_truncated_real_formats() {
    let Some(cst) = sample_file(&["牛顿与苹果树", "ali_001A.cst"]) else {
        return;
    };
    assert!(CstFile::parse(&cst[..cst.len() - 1]).is_err());

    let Some(kcs) = sample_file(&["牛顿与苹果树", "kcs", "sscript.kcs"]) else {
        return;
    };
    assert!(KcsFile::parse(&kcs[..kcs.len() - 1]).is_err());

    let Some(hg3) = sample_file(&["牛顿与苹果树", "boot.hg3"]) else {
        return;
    };
    assert!(HgFile::parse(&hg3[..hg3.len() - 1]).is_err());
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn cst_with_declared_length(declared_length: u32, body: &[u8]) -> Vec<u8> {
    let compressed = zlib(body);
    let mut bytes = b"CatScene".to_vec();
    bytes.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&declared_length.to_le_bytes());
    bytes.extend_from_slice(&compressed);
    bytes
}

fn minimal_cst_body() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&16u32.to_le_bytes());
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(&8u32.to_le_bytes());
    body.extend_from_slice(&12u32.to_le_bytes());
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&[1, 0x20, b'x', 0]);
    assert_eq!(body.len(), 32);
    body
}

#[test]
fn accepts_complete_cst_stream_when_declared_output_is_larger() {
    let body = minimal_cst_body();
    let bytes = cst_with_declared_length((body.len() + 17) as u32, &body);
    let file = CstFile::parse(&bytes).unwrap();
    assert_eq!(
        file.variant,
        cs2_formats::cst::CstVariant::ZlibDeclaredLengthMismatch
    );
    assert_eq!(
        file.length,
        cs2_formats::cst::CstLengthInfo {
            declared: (body.len() + 17) as u32,
            actual: body.len() as u32,
        }
    );
    assert_eq!(
        file.script_length,
        cs2_formats::cst::CstLengthInfo {
            declared: 16,
            actual: 16,
        }
    );
    assert_eq!(file.records.len(), 1);
    assert_eq!(file.inputs, vec![0..1]);
    assert_eq!(
        file.diagnostics,
        vec![cs2_formats::cst::CstDiagnostic::DeclaredLengthMismatch(
            cs2_formats::cst::CstLengthInfo {
                declared: (body.len() + 17) as u32,
                actual: body.len() as u32,
            }
        )]
    );
}

#[test]
fn accepts_cst_when_inner_script_length_exceeds_actual_body() {
    let mut body = minimal_cst_body();
    body[..4].copy_from_slice(&64u32.to_le_bytes());
    let bytes = cst_with_declared_length(body.len() as u32, &body);
    let file = CstFile::parse(&bytes).unwrap();
    assert_eq!(
        file.variant,
        cs2_formats::cst::CstVariant::ZlibDeclaredLengthMismatch
    );
    assert_eq!(
        file.script_length,
        cs2_formats::cst::CstLengthInfo {
            declared: 64,
            actual: 16,
        }
    );
    assert_eq!(
        file.diagnostics,
        vec![cs2_formats::cst::CstDiagnostic::ScriptLengthMismatch(
            cs2_formats::cst::CstLengthInfo {
                declared: 64,
                actual: 16,
            }
        )]
    );
}

#[test]
fn cst_missing_zero_checksum_recovery_does_not_accept_other_stream_damage() {
    let mut body = minimal_cst_body();
    let complete = (1u8..=255)
        .find_map(|value| {
            body[30] = value;
            let bytes = cst_with_declared_length(body.len() as u32, &body);
            (bytes.last() == Some(&0)).then_some(bytes)
        })
        .expect("one-byte message can produce a zero final Adler32 byte");
    let mut missing = complete.clone();
    missing.pop();
    let stored = (missing.len() - 16) as u32;
    missing[8..12].copy_from_slice(&stored.to_le_bytes());
    let parsed = CstFile::parse(&missing).unwrap();
    assert_eq!(
        parsed.variant,
        cs2_formats::cst::CstVariant::ZlibMissingAdler32Zero
    );
    assert!(
        parsed.diagnostics
            .contains(&cs2_formats::cst::CstDiagnostic::MissingAdler32Zero)
    );
    assert_eq!(parsed.records[0].text.len(), 1);

    let mut bad_checksum = missing.clone();
    *bad_checksum.last_mut().unwrap() ^= 1;
    assert!(CstFile::parse(&bad_checksum).is_err());

    missing.pop();
    let stored = (missing.len() - 16) as u32;
    missing[8..12].copy_from_slice(&stored.to_le_bytes());
    assert!(CstFile::parse(&missing).is_err());

    let mut trailing = complete;
    trailing.push(0);
    let stored = (trailing.len() - 16) as u32;
    trailing[8..12].copy_from_slice(&stored.to_le_bytes());
    assert!(CstFile::parse(&trailing).is_err());
}

#[test]
fn rejects_cst_unknown_compression_as_an_explicit_legacy_variant() {
    let mut bytes = b"CatScene".to_vec();
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&[0x55, 0xaa, 0x12, 0x34]);
    assert!(matches!(
        CstFile::parse(&bytes),
        Err(cs2_formats::CstError::UnsupportedVariant(
            cs2_formats::cst::CstVariant::LegacyUnknownCompression
        ))
    ));
}

fn synthetic_hg3(
    width: u32,
    height: u32,
    bit_depth: u32,
    literals: &[u8],
    commands: &[u8],
) -> Vec<u8> {
    let compressed_data = zlib(literals);
    let compressed_commands = zlib(commands);
    let mut bytes = b"HG-3".to_vec();
    bytes.extend_from_slice(&12u32.to_le_bytes());
    bytes.extend_from_slice(&0x300u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&7u32.to_le_bytes());
    bytes.extend_from_slice(b"stdinfo\0");
    bytes.extend_from_slice(&56u32.to_le_bytes());
    bytes.extend_from_slice(&40u32.to_le_bytes());
    for value in [width, height, bit_depth, 0, 0, width, height, 1, 0, 0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(b"img0000\0");
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(
        &u32::try_from(24 + compressed_data.len() + compressed_commands.len())
            .unwrap()
            .to_le_bytes(),
    );
    for value in [
        0u32,
        height,
        compressed_data.len() as u32,
        literals.len() as u32,
        compressed_commands.len() as u32,
        commands.len() as u32,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&compressed_data);
    bytes.extend_from_slice(&compressed_commands);
    bytes
}

fn synthetic_hg2(
    width: u32,
    height: u32,
    bit_depth: u32,
    literals: &[u8],
    commands: &[u8],
) -> Vec<u8> {
    let compressed_data = zlib(literals);
    let compressed_commands = zlib(commands);
    let mut bytes = b"HG-2".to_vec();
    for value in [
        12u32,
        0x25,
        width,
        height,
        bit_depth,
        0,
        0,
        compressed_data.len() as u32,
        literals.len() as u32,
        compressed_commands.len() as u32,
        commands.len() as u32,
        40,
        7,
        width,
        height,
        0,
        0,
        1,
        0,
        0,
        0,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&compressed_data);
    bytes.extend_from_slice(&compressed_commands);
    bytes
}

fn synthetic_hg3_with_tags(tags: &[([u8; 8], Vec<u8>)]) -> Vec<u8> {
    let mut bytes = b"HG-3".to_vec();
    bytes.extend_from_slice(&12_u32.to_le_bytes());
    bytes.extend_from_slice(&0x300_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&7_u32.to_le_bytes());
    for (index, (name, payload)) in tags.iter().enumerate() {
        bytes.extend_from_slice(name);
        let next = if index + 1 == tags.len() {
            0
        } else {
            u32::try_from(16 + payload.len()).unwrap()
        };
        bytes.extend_from_slice(&next.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(payload);
    }
    let stdinfo_payload = tags
        .iter()
        .find(|(name, _)| name == b"stdinfo\0")
        .map(|(_, payload)| payload)
        .unwrap();
    assert_eq!(stdinfo_payload.len(), 40);
    bytes
}

fn synthetic_stdinfo(width: u32, height: u32) -> Vec<u8> {
    [width, height, 32, 0, 0, width, height, 1, 0, 0]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}

#[test]
fn native_hg3_decodes_bottom_up_rows_as_top_down() {
    let bytes = synthetic_hg3(
        1,
        2,
        24,
        &[0, 0, 0, 0x40, 0x94, 0x3e, 0x22, 0x22],
        &[0x11, 0x08],
    );
    let parsed = HgFile::parse(&bytes).unwrap();
    assert_eq!(
        parsed.decode_frame(&bytes, 0).unwrap(),
        [5, 7, 9, 255, 1, 2, 3, 255]
    );
    assert_eq!(
        parsed.decode_frame_rgba8(&bytes, 0).unwrap(),
        [9, 7, 5, 255, 3, 2, 1, 255]
    );
}

#[test]
fn native_hg_slices_use_explicit_rows_and_independent_predictors() {
    let slice = |start: u32, rows: u32, literals: &[u8], commands: &[u8]| {
        let data = zlib(literals);
        let packed_commands = zlib(commands);
        let mut payload: Vec<_> = [
            start, rows, data.len() as u32, literals.len() as u32,
            packed_commands.len() as u32, commands.len() as u32,
        ].into_iter().flat_map(u32::to_le_bytes).collect();
        payload.extend_from_slice(&data);
        payload.extend_from_slice(&packed_commands);
        payload
    };
    // Tag number, file order and destination row are intentionally different.
    // The two-row slice encodes deltas 1..=8, so its bottom-up rows are
    // [1,2,3,4] and [6,8,10,12]; the one-row slice starts its own predictor.
    let mut tags = [
        (*b"stdinfo\0", synthetic_stdinfo(1, 3)),
        (*b"img0001\0", slice(0, 2, &[0, 0, 0, 0x40, 0x94, 0x3e, 0x22, 0x22], &[0x11, 0x08])),
        (*b"img0000\0", slice(2, 1, &[0, 0x40, 0x3e, 0x22], &[0x09, 0x01])),
    ];
    let bytes = synthetic_hg3_with_tags(&tags);
    let parsed = HgFile::parse(&bytes).unwrap();
    assert_eq!(parsed.decode_frame(&bytes, 0).unwrap(), [6, 8, 10, 12, 1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(parsed.decode_frame_rgba8(&bytes, 0).unwrap(), [10, 8, 6, 12, 3, 2, 1, 4, 7, 6, 5, 8]);
    tags[2].1[..4].copy_from_slice(&0u32.to_le_bytes());
    let overlap = synthetic_hg3_with_tags(&tags);
    assert!(matches!(HgFile::parse(&overlap).unwrap().decode_frame(&overlap, 0),
        Err(HgError::InvalidImageSlices(_))));
    tags[2].0 = *b"img0003\0";
    let missing = synthetic_hg3_with_tags(&tags);
    assert!(matches!(HgFile::parse(&missing).unwrap().decode_frame(&missing, 0),
        Err(HgError::InvalidImageSlices(_))));
}

#[test]
fn hg_attribute_ids_keep_signed_prefixes_aliases_and_later_values() {
    let attributes = |value: u32| [value, 9, 8, 7, 6].into_iter()
        .flat_map(u32::to_le_bytes).collect::<Vec<_>>();
    let tags = [
        (*b"stdinfo\0", synthetic_stdinfo(2, 1)),
        (*b"ats+17\0\0", attributes(40)),
        (*b"ats0X11\0", attributes(41)),
        (*b"ats-2\0\0\0", attributes(42)),
        (*b"ats3.2\0\0", attributes(43)),
        (*b"ATS0017\0", attributes(44)),
    ];
    let bytes = synthetic_hg3_with_tags(&tags);
    let parsed = HgFile::parse(&bytes).unwrap();
    let values = &parsed.frames[0].attributes;
    assert_eq!(values.len(), 2);
    assert_eq!(values[0].id, 17);
    assert_eq!(values[0].values[0], 41);
    assert_eq!(values[1].id, 65534);
    let mut continued = *b"ats12345\x36\0\0\0\x14\0\0\0";
    assert_eq!(cs2_formats::hg::attribute_tag_id(&continued), Some(123456u32 as u16));
    continued[8] = 0;
    assert_eq!(cs2_formats::hg::attribute_tag_id(&continued), Some(12345));
}

fn synthetic_alpha_tag(values: &[u8]) -> Vec<u8> {
    let compressed = zlib(values);
    let mut payload = Vec::with_capacity(8 + compressed.len());
    payload.extend_from_slice(&u32::try_from(compressed.len()).unwrap().to_le_bytes());
    payload.extend_from_slice(&u32::try_from(values.len()).unwrap().to_le_bytes());
    payload.extend_from_slice(&compressed);
    payload
}

#[test]
fn decodes_hg3_jpeg_with_separate_alpha_mask() {
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 100)
        .encode(&[48, 112, 176], 1, 1, ColorType::Rgb8.into())
        .unwrap();
    let bytes = synthetic_hg3_with_tags(&[
        (*b"stdinfo\0", synthetic_stdinfo(1, 1)),
        (*b"img_jpg\0", jpeg),
        (*b"img_al\0\0", synthetic_alpha_tag(&[73])),
    ]);
    let file = HgFile::parse(&bytes).unwrap();
    assert_eq!(
        file.frames[0].encoded_image.as_ref().unwrap().encoding,
        cs2_formats::HgImageEncoding::JpegAlpha
    );
    let rgba = file.decode_frame_rgba8(&bytes, 0).unwrap();
    assert_eq!(rgba[3], 73);
    assert!(rgba[0].abs_diff(48) <= 8);
    assert!(rgba[1].abs_diff(112) <= 8);
    assert!(rgba[2].abs_diff(176) <= 8);
    let bgra = file.decode_frame(&bytes, 0).unwrap();
    assert_eq!(&bgra[..3], &[rgba[2], rgba[1], rgba[0]]);
}

#[test]
fn decodes_hg3_blank_jpeg_placeholder_with_invalid_alpha() {
    let tiny_jpeg = |rgb: [u8; 3]| {
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 100)
            .encode(&rgb.repeat(4), 2, 2, ColorType::Rgb8.into()).unwrap();
        assert_eq!(&jpeg[2..4], &[0xff, 0xe0]);
        // Replace the generated JFIF segment with a comment to make the test
        // JPEG the same size as the original empty overlay.
        let end = 4 + usize::from(u16::from_be_bytes([jpeg[4], jpeg[5]]));
        let comment_len = 631 - (jpeg.len() - end) - 4;
        let mut comment = vec![0xff, 0xfe];
        comment.extend_from_slice(&(comment_len as u16).to_be_bytes());
        comment.resize(comment_len + 2, 0);
        jpeg.splice(2..end, comment);
        assert_eq!(jpeg.len(), 631);
        jpeg
    };
    let mut stdinfo = synthetic_stdinfo(2, 2);
    stdinfo[20..24].copy_from_slice(&916u32.to_le_bytes());
    stdinfo[24..28].copy_from_slice(&1272u32.to_le_bytes());
    let mut invalid_alpha = [12u32, 4].into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
    invalid_alpha.extend_from_slice(&[0x78, 0xda, 0x61, 0x06, 0, 0, 0, 0, 0, 0, 0, 0]);
    let mut tags = [
        (*b"stdinfo\0", stdinfo),
        (*b"img_jpg\0", tiny_jpeg([0, 0, 0])),
        (*b"img_al\0\0", invalid_alpha.clone()),
    ];
    let decode = |tags: &[([u8; 8], Vec<u8>)]| {
        let bytes = synthetic_hg3_with_tags(tags);
        HgFile::parse(&bytes).unwrap().decode_frame_rgba8(&bytes, 0)
    };
    assert_eq!(decode(&tags).unwrap(), [0; 16]);
    tags[2].1 = synthetic_alpha_tag(&[1, 2, 3, 4]);
    let pixels = decode(&tags).unwrap();
    assert_eq!(pixels.iter().skip(3).step_by(4).copied().collect::<Vec<_>>(), [1, 2, 3, 4]);

    tags[2].1 = invalid_alpha;
    tags[1].1 = tiny_jpeg([127, 127, 127]);
    assert!(matches!(decode(&tags), Err(HgError::InvalidCompression)));
    tags[1].1 = tiny_jpeg([0, 0, 0]);
    tags[0].1[12..16].copy_from_slice(&1u32.to_le_bytes());
    assert!(matches!(decode(&tags), Err(HgError::InvalidCompression)));
}

#[test]
fn decodes_hg3_webp_and_standalone_alpha_tags() {
    let rgba_source = [14, 80, 190, 255];
    let mut webp = Vec::new();
    WebPEncoder::new_lossless(&mut webp)
        .encode(&rgba_source, 1, 1, ColorType::Rgba8.into())
        .unwrap();
    let webp_bytes = synthetic_hg3_with_tags(&[
        (*b"stdinfo\0", synthetic_stdinfo(1, 1)),
        (*b"img_wbp\0", webp),
    ]);
    let webp_file = HgFile::parse(&webp_bytes).unwrap();
    assert_eq!(
        webp_file.frames[0].encoded_image.as_ref().unwrap().encoding,
        cs2_formats::HgImageEncoding::Webp
    );
    assert_eq!(
        webp_file.decode_frame_rgba8(&webp_bytes, 0).unwrap(),
        rgba_source
    );
    let mut mislabeled = webp_bytes.clone();
    mislabeled[76..84].copy_from_slice(b"img_jpg\0");
    let mislabeled_file = HgFile::parse(&mislabeled).unwrap();
    assert!(matches!(mislabeled_file.decode_frame_rgba8(&mislabeled, 0),
        Err(HgError::InvalidEncodedImage)));
    let oversized = synthetic_hg3_with_tags(&[
        (*b"stdinfo\0", synthetic_stdinfo(100_000, 100_000)),
        (*b"img_al\0\0", synthetic_alpha_tag(&[0])),
    ]);
    let oversized_file = HgFile::parse(&oversized).unwrap();
    assert!(matches!(oversized_file.decode_frame(&oversized, 0),
        Err(HgError::PixelOutputTooLarge)));

    let alpha_bytes = synthetic_hg3_with_tags(&[
        (*b"stdinfo\0", synthetic_stdinfo(1, 1)),
        (*b"img_al\0\0", synthetic_alpha_tag(&[37])),
    ]);
    let alpha_file = HgFile::parse(&alpha_bytes).unwrap();
    assert_eq!(
        alpha_file.frames[0]
            .encoded_image
            .as_ref()
            .unwrap()
            .encoding,
        cs2_formats::HgImageEncoding::Alpha
    );
    assert_eq!(
        alpha_file.decode_frame_rgba8(&alpha_bytes, 0).unwrap(),
        [218, 218, 218, 255]
    );
}

#[test]
fn decodes_real_hg3_pixels_to_bgra_and_rgba_and_rejects_truncation() {
    let Some(bytes) = sample_file(&["cs2_full_v401", "system", "image", "sys_click.hg3"]) else {
        return;
    };
    let file = HgFile::parse(&bytes).unwrap();
    let bgra = file.decode_frame(&bytes, 0).unwrap();
    let rgba = file.decode_frame_rgba8(&bytes, 0).unwrap();
    assert_eq!(bgra.len(), 26 * 15 * 4);
    assert!(bgra.iter().any(|&value| value != 0));
    for (bgra_pixel, rgba_pixel) in bgra.chunks_exact(4).zip(rgba.chunks_exact(4)) {
        assert_eq!(
            rgba_pixel,
            &[bgra_pixel[2], bgra_pixel[1], bgra_pixel[0], bgra_pixel[3]]
        );
    }
    let image = file.frames[0].image.as_ref().unwrap();
    let payload_end = image.data_offset as usize
        + image.compressed_data_length as usize
        + image.compressed_command_length as usize;
    assert!(file.decode_frame(&bytes[..payload_end - 1], 0).is_err());
}

#[test]
fn decodes_hg3_24_bit_and_preserves_32_bit_transparency() {
    let zero_commands = [0x08, 0x01];
    let hg3_24 = synthetic_hg3(1, 1, 24, &[], &zero_commands);
    let file_24 = HgFile::parse(&hg3_24).unwrap();
    assert_eq!(file_24.decode_frame(&hg3_24, 0).unwrap(), [0, 0, 0, 255]);
    assert_eq!(
        file_24.decode_frame_rgba8(&hg3_24, 0).unwrap(),
        [0, 0, 0, 255]
    );

    let hg3_32 = synthetic_hg3(1, 1, 32, &[], &zero_commands);
    let file_32 = HgFile::parse(&hg3_32).unwrap();
    assert_eq!(file_32.decode_frame(&hg3_32, 0).unwrap(), [0, 0, 0, 0]);
    let mut marked_depth = hg3_32.clone();
    marked_depth[44..48].copy_from_slice(&0xaaff_0020u32.to_le_bytes());
    let marked = HgFile::parse(&marked_depth).unwrap();
    assert_eq!(marked.frames[0].bit_depth, 0xaaff_0020);
    assert_eq!(marked.decode_frame(&marked_depth, 0).unwrap(), [0, 0, 0, 0]);
}

#[test]
fn decodes_real_24_bit_hg3_to_tightly_packed_rgba8() {
    let Some(bytes) = sample_file(&["祝福のカンパネラ", "install.hg3"]) else {
        return;
    };
    let file = HgFile::parse(&bytes).unwrap();
    assert_eq!(
        (
            file.frames[0].width,
            file.frames[0].height,
            file.frames[0].bit_depth
        ),
        (800, 600, 24)
    );
    let pixels = file.decode_frame_rgba8(&bytes, 0).unwrap();
    assert_eq!(pixels.len(), 800 * 600 * 4);
    assert!(pixels.chunks_exact(4).any(|pixel| pixel[..3] != [0, 0, 0]));
    assert!(pixels.chunks_exact(4).all(|pixel| pixel[3] == 255));
}

#[test]
fn decodes_hg2_pixels_and_reports_pixel_stream_errors() {
    let zero_commands = [0x08, 0x01];
    let hg2 = synthetic_hg2(1, 1, 32, &[], &zero_commands);
    let file = HgFile::parse(&hg2).unwrap();
    assert_eq!(file.decode_frame(&hg2, 0).unwrap(), [0, 0, 0, 0]);
    let mut extended = hg2.clone();
    extended[48..52].copy_from_slice(&56u32.to_le_bytes());
    drop(extended.splice(88..88, [0xa5; 16]));
    let extended_file = HgFile::parse(&extended).unwrap();
    assert_eq!(extended_file.frames[0].image.as_ref().unwrap().data_offset, 104);
    assert_eq!(extended_file.decode_frame(&extended, 0).unwrap(), [0, 0, 0, 0]);

    let mut legacy = hg2[..48].to_vec();
    legacy[8..12].copy_from_slice(&0x1234_0010u32.to_le_bytes());
    legacy.extend_from_slice(&hg2[88..]);
    let legacy_file = HgFile::parse(&legacy).unwrap();
    assert_eq!(legacy_file.version, 0x1234_0010);
    assert_eq!(legacy_file.frames[0].id, 0);
    assert_eq!(legacy_file.frames[0].canvas_size, [1, 1]);
    assert_eq!(legacy_file.decode_frame(&legacy, 0).unwrap(), [0, 0, 0, 0]);
    assert!(legacy_file.native_aggregate_metadata().is_err());

    // The eight plane bytes encode raw deltas 1..=8. Two packed BGR rows
    // consume the first six bytes, independently of four-byte run padding.
    let packed = synthetic_hg2(1, 2, 24,
        &[0, 0, 0, 0x40, 0x94, 0x3e, 0x22, 0x22], &[0x11, 0x08]);
    assert_eq!(HgFile::parse(&packed).unwrap().decode_frame(&packed, 0).unwrap(),
        [1, 2, 3, 255, 5, 7, 9, 255]);
    let precision = synthetic_hg2(1, 1, (4 << 16) | 32,
        &[0, 0, 0x94, 0x22], &[0x09, 0x01]);
    assert_eq!(HgFile::parse(&precision).unwrap().decode_frame(&precision, 0).unwrap(),
        [17, 34, 51, 68]);
    let mut aggregate = file.clone();
    let mut second = aggregate.frames[0].clone();
    second.id = 8;
    second.base = [-3, -4];
    aggregate.frames.push(second);
    assert_eq!(&aggregate.native_aggregate_metadata().unwrap()[7..], &[
        (-3i32) as u32, (-4i32) as u32,
    ]);

    let mut truncated = hg2.clone();
    truncated.pop();
    assert!(matches!(
        file.decode_frame(&truncated, 0),
        Err(HgError::Truncated | HgError::InvalidCompression)
    ));

    let bad_commands = synthetic_hg3(1, 1, 32, &[], &[]);
    let bad_file = HgFile::parse(&bad_commands).unwrap();
    assert!(matches!(
        bad_file.decode_frame(&bad_commands, 0),
        Err(HgError::Truncated | HgError::InvalidPixelCommands)
    ));

    let mut unsupported_encoding = synthetic_hg3(1, 1, 32, &[], &zero_commands);
    unsupported_encoding[76..84].copy_from_slice(b"img_jpa\0");
    let unsupported_encoding_file = HgFile::parse(&unsupported_encoding).unwrap();
    assert!(matches!(
        unsupported_encoding_file.decode_frame(&unsupported_encoding, 0),
        Err(HgError::UnsupportedImageEncoding)
    ));

    let unsupported_depth = synthetic_hg3(1, 1, 16, &[], &zero_commands);
    let unsupported_file = HgFile::parse(&unsupported_depth).unwrap();
    assert!(matches!(
        unsupported_file.decode_frame(&unsupported_depth, 0),
        Err(HgError::UnsupportedBitDepth(16))
    ));
}

fn css(records: &[u8], used: u32, tag: u32) -> Vec<u8> {
    let compressed = zlib(records);
    let mut bytes = b"CSS\0".to_vec();
    bytes.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&used.to_le_bytes());
    bytes.extend_from_slice(&tag.to_le_bytes());
    bytes.extend_from_slice(&compressed);
    bytes
}

#[test]
fn parses_real_fes_and_rejects_container_boundary_errors() {
    for path in [
        &["cs2_full_v401", "system", "fes", "title.fes"][..],
        &["牛顿与苹果树", "fes", "tips.fes"][..],
    ] {
        let Some(bytes) = sample_file(path) else {
            continue;
        };
        let file = FesFile::parse(&bytes).unwrap();
        assert!(!file.text.is_empty());
        assert_eq!(file.reserved, 0);
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
            bytes.len() - 16
        );

        assert_eq!(
            FesFile::parse(&bytes[..3]),
            Err(cs2_formats::FesError::InvalidSignature)
        );
        assert_eq!(
            FesFile::parse(&bytes[..15]),
            Err(cs2_formats::FesError::Truncated)
        );
        assert!(FesFile::parse(&bytes[..bytes.len() - 1]).is_err());
        let mut invalid_stream = bytes.clone();
        invalid_stream[16] ^= 0xff;
        assert!(matches!(
            FesFile::parse(&invalid_stream),
            Err(cs2_formats::FesError::InvalidCompression)
        ));
    }
}

#[test]
fn rejects_fes_reserved_fields_and_declared_compressed_length_mismatches() {
    let text = b"#BOOT\n";
    let compressed = zlib(text);
    let mut bytes = b"FES\0".to_vec();
    bytes.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(text.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&compressed);
    assert_eq!(
        FesFile::parse(&bytes),
        Err(cs2_formats::FesError::InvalidReserved)
    );

    let mut truncated = bytes;
    truncated[12..16].copy_from_slice(&0u32.to_le_bytes());
    truncated.pop();
    assert!(matches!(
        FesFile::parse(&truncated),
        Err(cs2_formats::FesError::Truncated)
    ));
}

#[test]
fn parses_real_anm_fixed_instruction_table() {
    let Some(bytes) = sample_file(&["cs2_full_v401", "system", "image", "sys_click.anm"]) else {
        return;
    };
    let file = AnmFile::parse(&bytes).unwrap();
    assert_eq!(file.version, AnmVersion::FixedInstructionTable);
    assert_eq!(file.instructions.len(), 9);
    assert_eq!(file.instructions[0].opcode, 0);
    assert_eq!(file.instructions[0].operands[0].value, 0);
    assert!(AnmFile::parse(&bytes[..31]).is_err());
    assert!(AnmFile::parse(&bytes[..bytes.len() - 1]).is_err());

    let mut wrong_signature = bytes;
    wrong_signature[0] = b'X';
    assert!(matches!(
        AnmFile::parse(&wrong_signature),
        Err(cs2_formats::AnmError::InvalidSignature)
    ));
}

#[test]
fn inspects_real_legacy_anm_headers() {
    let samples = [
        (
            &["Yukikoi Melt", "image.int"][..],
            0x23a8_93a2,
            b"manpu_kidukL_s.anm".as_slice(),
        ),
        (
            &["灰色的乐园v1", "image00.int"][..],
            0x69df_d7a6,
            b"manpu_kidukL_s.anm".as_slice(),
        ),
        (
            &["灰色的迷宫", "image.int"][..],
            0x9bcd_2948,
            b"manpu_kidukL.anm".as_slice(),
        ),
        (
            &["灰色的迷宫", "image.int"][..],
            0x9bcd_2948,
            b"manpu_kidukL_m.anm".as_slice(),
        ),
        (
            &["灰色的迷宫", "image.int"][..],
            0x9bcd_2948,
            b"manpu_kidukL_s.anm".as_slice(),
        ),
    ];

    for (archive_path, name_seed, entry_name) in samples {
        let Some(archive_bytes) = sample_file(archive_path) else {
            return;
        };
        let archive = Archive::parse_with_name_seed(&archive_bytes, name_seed).unwrap();
        let bytes = archive.read(&archive_bytes, entry_name).unwrap().unwrap();
        assert_eq!(bytes.len(), 32);
        assert_eq!(&bytes[..4], b"ANM\0");
        assert_eq!(&bytes[4..], &[0; 28]);
        let file = AnmFile::parse(&bytes).unwrap();
        assert_eq!(file.version, AnmVersion::LegacyEmpty);
        assert!(file.instructions.is_empty());
    }
}

#[test]
fn distinguishes_legacy_empty_anm_from_invalid_zero_count_tables() {
    let mut legacy = vec![0; 32];
    legacy[..4].copy_from_slice(b"ANM\0");
    let parsed = AnmFile::parse(&legacy).unwrap();
    assert_eq!(parsed.version, AnmVersion::LegacyEmpty);
    assert!(parsed.instructions.is_empty());

    let mut trailing = legacy.clone();
    trailing.push(0);
    assert_eq!(
        AnmFile::parse(&trailing),
        Err(cs2_formats::AnmError::InvalidTable)
    );

    let mut truncated_record = legacy;
    truncated_record[8..12].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(
        AnmFile::parse(&truncated_record),
        Err(cs2_formats::AnmError::Truncated)
    );
}

#[test]
fn parses_real_wss_source_as_bytes_without_compiling_it() {
    let Some(bytes) = sample_file(&["cs2_full_v401", "system", "ptcl", "3dstorm.wss"]) else {
        return;
    };
    let source = WssSource::parse(&bytes).unwrap();
    assert!(!source.lines.is_empty());
    assert!(source.line(0).unwrap().starts_with(b"/*"));
    assert!(
        source
            .source
            .windows(b"#include \"ptcl.def\"".len())
            .any(|window| window == b"#include \"ptcl.def\"")
    );
    assert!(source.includes.iter().any(|name| name == b"ptcl.def"));
    let source = WssSource::parse(b"#include <shared.def>\r\n").unwrap();
    assert_eq!(source.includes, vec![b"shared.def".to_vec()]);
    assert!(WssSource::parse(b"int value;\0").is_err());
}

#[test]
fn parses_real_startup_xml_without_decoding_legacy_text() {
    for path in [
        &["牛顿与苹果树", "config", "startup.xml"][..],
        &[
            "[170526][NEKO WORKs] ネコぱら vol.3 steam版 (files)",
            "nekopara_vol3",
            "config",
            "startup.xml",
        ][..],
    ] {
        let Some(bytes) = sample_file(path) else {
            continue;
        };
        let xml = ConfigXml::parse(&bytes).unwrap();
        assert_eq!(xml.root, b"document");
        assert!(
            xml.text(&[b"document", b"INT", b"int00", b"name"])
                .is_some()
        );
        assert!(xml.text(&[b"document", b"INT", b"int00", b"int"]).is_some());
    }
}

#[test]
fn config_xml_reports_malformed_structure_and_keeps_literal_entities() {
    let xml = ConfigXml::parse(
        br#"<?xml version="1.0"?><document><a><n>-12tail</n><e>a&amp;b</e></a></document>"#,
    )
    .unwrap();
    assert_eq!(xml.integer(&[b"document", b"a", b"n"], 7), -12);
    assert_eq!(
        xml.text(&[b"document", b"a", b"e"]),
        Some(b"a&amp;b".as_slice())
    );
    // Integer XML fields preserve the engine's numeric-prefix grammar.
    // A missing field is distinct from existing invalid/empty text.
    for (text, expected) in [
        ("12.5tail", 0), ("-12.", 0), ("+12.0", 0),
        ("0x12.5", 18), ("-0X12.tail", -18),
        ("12tail.5", 12), (" 12", 0), ("", 0),
        ("4294967297suffix", 1),
    ] {
        let bytes = format!("<document><n>{text}</n></document>");
        let field = ConfigXml::parse(bytes.as_bytes()).unwrap();
        assert_eq!(field.integer(&[b"document", b"n"], 7), expected, "{text:?}");
        assert_eq!(field.integer(&[b"document", b"missing"], 7), 7);
    }
    assert!(ConfigXml::parse(b"<document><a></document>").is_err());
    assert!(ConfigXml::parse(b"<document>").is_err());
    assert!(ConfigXml::parse(b"<document></document> \n<!-- trailing -->").is_ok());
    assert!(ConfigXml::parse(b"<document></document><second/>").is_err());
    assert!(ConfigXml::parse(b"<document><![CDATA[inside]]></document>").is_ok());
}

#[test]
fn parses_css_records_and_preserves_loader_inflation_diagnostics() {
    let mut records = Vec::new();
    records.extend_from_slice(&7u32.to_le_bytes());
    records.extend_from_slice(&3u32.to_le_bytes());
    records.extend_from_slice(b"abc");
    records.extend_from_slice(&7u32.to_le_bytes());
    records.extend_from_slice(&2u32.to_le_bytes());
    records.extend_from_slice(b"xy");
    let file = css(&records, records.len() as u32, 0x1234_5678);
    let parsed = CssFile::parse(&file).unwrap();
    assert_eq!(parsed.tag, 0x1234_5678);
    assert_eq!(parsed.record(7), Some(b"abc".as_slice()));

    assert!(matches!(
        CssFile::parse(b"NOPE"),
        Err(CssError::InvalidSignature)
    ));
    assert!(matches!(CssFile::parse(b"CSS\0"), Err(CssError::Truncated)));
    assert!(matches!(
        CssFile::parse(&file[..file.len() - 1]),
        Err(CssError::Truncated)
    ));
    let mut bad_zlib = file;
    *bad_zlib.last_mut().unwrap() ^= 0xff;
    let recovered = CssFile::parse(&bad_zlib).unwrap();
    assert_eq!(recovered.record(7), Some(b"abc".as_slice()));
    assert!(recovered.inflation_status().unwrap().error.is_some());

    let mut trailing = css(&records, records.len() as u32, 0x1234_5678);
    trailing.extend_from_slice(b"ignored");
    assert_eq!(CssFile::parse(&trailing).unwrap().record(7), Some(b"abc".as_slice()));
    let past_declared_use = CssFile::parse(&css(&records, 0, 0x1234_5678)).unwrap();
    assert_eq!(past_declared_use.used_len(), 0);
    assert_eq!(past_declared_use.record(7), Some(b"abc".as_slice()));
    assert!(past_declared_use.used_bytes().unwrap().is_empty());
}

#[test]
fn css_drains_buffered_records_after_input_is_consumed() {
    let mut container = CssFile::new(1000);
    container.append(1, &vec![0; 0x2a5c2]).unwrap();
    let mut layout = [0; 576];
    layout[..8].copy_from_slice(b"default\0");
    container.append(0x2000_0000, &layout).unwrap();
    container.append(0x100, &vec![0; 10_201]).unwrap();
    let loaded = CssFile::parse(&container.to_css().unwrap()).unwrap();
    assert_eq!(loaded.try_record(0x2000_0000).unwrap(), Some(layout.as_slice()));
    assert_eq!(loaded.used_bytes().unwrap(), container.used_bytes().unwrap());
    assert_eq!(loaded.inflation_status().unwrap().output_written, loaded.used_len());
    assert!(loaded.inflation_status().unwrap().stream_end);
}

#[test]
fn css_container_records_serialize_and_snapshot_round_trip() {
    let mut container = CssFile::new(0x55aa);
    container.append(12, b"first").unwrap();
    container.append(12, b"second").unwrap();
    let encoded = container.to_css().unwrap();
    let loaded = CssFile::parse(&encoded).unwrap();
    assert_eq!(loaded.tag, 0x55aa);
    assert_eq!(loaded.record(12), Some(b"first".as_slice()));

    let snapshot = loaded.snapshot().unwrap();
    let restored = CssFile::from_snapshot(&snapshot).unwrap();
    assert_eq!(restored, loaded);
    let mut restored = restored;
    let allocation = restored.storage().len();
    let old_used = restored.used_len();
    assert!(restored.remove(12));
    assert_eq!(restored.record(12), Some(b"second".as_slice()));
    assert_eq!(restored.capacity(), 0x80000 - 13);
    assert_eq!(restored.used_len(), old_used - 13);
    assert_eq!(restored.storage().len(), allocation);
    let copy = restored.try_clone_container().unwrap();
    assert_eq!(copy.storage().len(), restored.capacity() as usize);
    assert_eq!(copy.record(12), restored.record(12));
    let mut snapshot = restored.snapshot().unwrap();
    snapshot.extend_from_slice(b"unconsumed suffix");
    let mut from_snapshot = CssFile::from_snapshot(&snapshot).unwrap();
    assert_eq!(from_snapshot.capacity(), restored.capacity());
    from_snapshot.clear();
    assert_eq!(from_snapshot.capacity(), 0x80000);
    assert_eq!(from_snapshot.used_len(), 0);
    assert_eq!(from_snapshot.tag(), 0x55aa);
}
