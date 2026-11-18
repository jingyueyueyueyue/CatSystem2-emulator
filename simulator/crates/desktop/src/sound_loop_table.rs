//! Ordinary400 sound loop tables. This parser follows the byte-level
//! documentation, preserving DWORD/i64 wrap and encoded resource identities.
use cs2_runtime::resource_router::ResourceMode;
use cs2_runtime::{TaskCommand, TaskCommandBuffers};
use cs2_script::{NativeReturn, Result, Vm};

use super::sound_native::{decode_name, encode_name};
use super::{LoggingHost, audio_script_error, expand_kcs_format, read_u32, trim_nul};

pub(super) fn legacy_sound_loop_root_query(sha: Option<&str>) -> Option<u32> {
    match sha {
        Some("72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8") => Some(30),
        Some(
            // Utate, Kamikaze, Djibril5, and Grisaia. Top32 returns the
            // sound task created by Top. The wrappers use literal "sound"
            // and send 48-byte loop records via 182/185/187/185 respectively.
            "a9f8b08713aac278671853b2293a13b2623a122ede9341cb921901b09601cb52"
                | "f6af7437475b8c066be973245aa79688bea510ca1192264add4f1914ebee4c0e"
                | "012c71635932c241cde4df986377516279a50b907166757e89ea7b86ff3f813f"
                | "9f3a5ad0ed22074420685f1e2a3bc92436d68289c7bac854d5f7d7331dc3ef67"
        ) => Some(32),
        _ => None,
    }
}

impl LoggingHost<'_> {
    pub(super) fn load_sound_loop_native(
        &mut self,
        args: &[u8],
        vm: &mut Vm,
    ) -> Result<NativeReturn> {
        if args.len() != 8 {
            return Err(audio_script_error("native400 requires exactly two DWORDs"));
        }
        let profiled = self.has_sound_loop_pe_profile()?;
        if !profiled && (read_u32(args, 0)? != 19 || !self.verified_kanmi_sound_loop_source()?) {
            return Err(audio_script_error("native400 category ABI needs a verified PE profile"));
        }
        let source = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
        let (table_name, sound_task) = if profiled {
            // The verified wrappers complete the name using "sound" and
            // query the startup-owned sound service before opening the table.
            let name = self.complete_sound_loop_name(&source)?;
            (decode_name(self.text_encoding, &name)?, self.query_root_sound_task()?)
        } else {
            // This packed sample's startup call supplies a qualified path.
            // Its shipped archive has no loop table.
            let name = "bgm.int/LoopPoint.txt";
            if trim_nul(&source) != name.as_bytes() {
                return Err(audio_script_error(
                    "native400 packed startup requires the verified loop table path",
                ));
            }
            (name.to_owned(), None)
        };
        let Some(bytes) = self.resources
            .read_mode(&table_name, ResourceMode::LooseFirst)
            .map_err(|cause| self.sound_host_error(cause))? else {
            return Ok(NativeReturn::Continue);
        };
        if bytes.is_empty() {
            return Ok(NativeReturn::Continue);
        }
        if !profiled {
            return Err(audio_script_error(
                "native400 packed startup has a nonempty table with an unverified receiver ABI",
            ));
        }
        let mut cleaned = strip_comments(trim_nul(&bytes));
        cleaned.retain(|&byte| byte != b'"');
        for (index, (name, start, end)) in loop_table_entries(&cleaned).enumerate() {
            if index >= 65_536 {
                return Err(audio_script_error("sound loop table exceeds 65536 records"));
            }
            let name = self.complete_sound_loop_name(name)?;
            let Some(mut record) = loop_record(&name, start, end) else {
                continue;
            };
            let task = sound_task.ok_or_else(|| {
                audio_script_error("nonempty sound loop table requires a reachable kind13 receiver")
            })?;
            let mut output = 0;
            if !self.dispatch_sound_task_command(
                task,
                TaskCommand {
                    target: task as i32,
                    sender: self.current_task_id as i32,
                    code: 199,
                    data: [0; 3],
                },
                &mut TaskCommandBuffers {
                    data0: None,
                    data1: Some(&mut record),
                },
                &mut output,
            )? {
                return Err(audio_script_error(
                    "sound loop target did not consume command199",
                ));
            }
        }
        Ok(NativeReturn::Continue)
    }

    fn has_sound_loop_pe_profile(&self) -> Result<bool> {
        // Reuse independently verified startup identities when the ordinary
        // table has no general FeatureProfile. Happiness2's companion binding
        // additionally pins its packed package and effective producer KCS.
        let sha = self.data_services.native_profile.map(|profile| profile.pe_sha256)
            .or(self.data_services.css_task_pe_sha256)
            .or(self.data_services.particle_message_pe_sha256);
        if legacy_sound_loop_root_query(sha).is_some() || matches!(sha, Some(
            "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f"
            // Campanella has root31/sound177 at this slot.
            | "659d5f069f3c97f69a61e12449cf1510b45e48a10649b9ac6977c2b93d6d28ad"
            // Happiness2's plain companion has root35/sound199 at this slot.
            | "5869b7d03a636c200f5042f0c3fc37878a362615bf724f58d47aec5eb8b2d44c"
            | "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc"
            | "474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1"
            | "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"
            | "c47c9d883126f641ce07d92eb57b6ac9833fe7495996001c12c68c55b99974d3"
            | "4ccb48ad4bb6bfb5a15a45b8e1f2c2e62072cfad9f1487f869561a473770ed94"
            | "c48d383c335c5b283bb0c17bd9f14b2b8bfef4a2e3d635365157b1c69498df78"
            | "c2af2ee9baf4b26a3c2cf8a1c867f15f92b54ad1ac5fac381858046a47818f6c"
            | "ac226c8455eff72114d6b1b9929fae01ac49ba90b8ba579f049f179c4c6dd8d5"
            | "3b359fc2d737d5ceed454d6b65e6e5b5e6dfb4a24cf88ee361df86e454203f18"
            | "6f75ae3877e018de1982a68f1089c127870d895d748c6290dd2ee85cd8daa4de"
            | "1f3288d5fd5408bee4d395faab708aca78c9eeaa6c172dc06dcce09c844ba9d5"
        )) {
            return Ok(true);
        }
        Ok(self.verified_utate_sound_loop_source()? || self.verified_sudden_sound_loop_source()?)
    }

    pub(super) fn verified_kanmi_sound_loop_source(&self) -> Result<bool> {
        const PACKAGE_EXE: &str =
            "\u{5c1a}\u{6709}\u{4f73}\u{5bc6}\u{4f34}\u{604b}\u{5fc3}.exe";
        const PACKAGE_SHA256: &str =
            "c264b2465889c78723079a694d2654ef1091253db657a735e92f58b75031aa26";
        const PRODUCER_SHA256: &str =
            "7e11ce11146b4eb4fa27679f6c2547055f08eeff24c8ab122d2d0dfc959128a4";
        Ok(self.verified_sound_loop_executable(PACKAGE_EXE, 4_223_965, PACKAGE_SHA256)?
            && self.verified_sound_loop_producer(PRODUCER_SHA256)?)
    }

    pub(super) fn verified_sudden_sound_loop_source(&self) -> Result<bool> {
        const PACKAGE: &str =
            "35ec21bcfcee0f324206e945eef1c082e35fd8af594ea0e738a46cd941f5b3f3";
        const COMPANION: &str =
            "d158c8915d4982c82badc631b4f92e1f140b246df5929ff2d9924d242c66765c";
        const PRODUCER: &str =
            "3294c9b35ec41e6ce157444a637d9529a0cd9876bdfbc126733e8ddc397b76cf";
        Ok(self.verified_sound_loop_executable("cs2_cn.exe", 3_842_048, COMPANION)?
            && self.verified_sound_loop_executable("cs2.exe", 4_929_024, PACKAGE)?
            && self.verified_sound_loop_producer(PRODUCER)?)
    }

    pub(super) fn verified_utate_sound_loop_source(&self) -> Result<bool> {
        // Utate's current scanner ranks cs2conf.exe before UTATEMEGURI.exe.
        // The configuration tool is not an ABI alias: verify the real runtime
        // AND the effective startup producer instead of allowing its SHA.
        // Evidence: 20261118_startup_batch4/utate-identity-evidence.json and
        // 20261118_D_campanella400.json (table29de80[400] ->517e0).
        const RUNTIME: &str =
            "a9f8b08713aac278671853b2293a13b2623a122ede9341cb921901b09601cb52";
        const PRODUCER: &str =
            "f3ca86f40e108dd96da7dd031bf8dda23dd4ff1a1b0e1037ee9441a6c7aec3ce";
        Ok(self.verified_sound_loop_executable("UTATEMEGURI.exe", 2_993_152, RUNTIME)?
            && self.verified_sound_loop_producer(PRODUCER)?)
    }

    fn verified_sound_loop_executable(&self, name: &str, size: u64, expected_sha: &str) -> Result<bool> {
        let Some(file) = self.resources.files().iter().find(|file| {
            file.size == size && file.relative_path.to_str()
                .is_some_and(|path| path.eq_ignore_ascii_case(name))
        }) else { return Ok(false); };
        // Read the indexed root file directly; an archive/alias with this
        // basename must not supply executable identity.
        let bytes = std::fs::read(self.resources.root().join(&file.relative_path))
            .map_err(|cause| super::native_state_error_owned(format!(
                "native400 runtime identity read failed for {name}: {cause}")))?;
        Ok(super::particle_plugin_digest::sha256(&bytes) == expected_sha)
    }

    fn verified_sound_loop_producer(&self, expected_sha: &str) -> Result<bool> {
        let producer = self.resources.read_mode("kcs.int/main.kcs", ResourceMode::LooseFirst)
            .map_err(|cause| super::native_state_error_owned(format!(
                "native400 startup producer identity read failed: {cause}")))?;
        Ok(producer.is_some_and(|bytes|
            super::particle_plugin_digest::sha256(&bytes) == expected_sha))
    }

    fn complete_sound_loop_name(&mut self, source: &[u8]) -> Result<Vec<u8>> {
        let source = trim_nul(source);
        if source.len() >= 260 {
            return Err(audio_script_error("sound loop name exceeds 259 bytes"));
        }
        let name = decode_name(self.text_encoding, source)?;
        // The PE helper resolves the special sound category via the current
        // soundconf/VOICE definition, not a literal startup INT named sound.
        // Its miss clears the output; native400 ignores the returned BOOL.
        let completed = self.media_category_candidates(b"sound", &name)?
            .and_then(|names| names.into_iter().next()).unwrap_or_default();
        let name = encode_name(self.text_encoding, &completed)?;
        if name.len() >= 260 {
            return Err(audio_script_error(
                "completed sound loop name exceeds 259 bytes",
            ));
        }
        Ok(name)
    }
}

fn loop_table_entries(cleaned: &[u8]) -> impl Iterator<Item = (&[u8], i64, i64)> {
    cleaned.split(|&byte| byte == b'\r' || byte == b'\n').filter_map(|line| {
        let mut words = line
            .split(|&byte| byte == b' ' || byte == b'\t')
            .filter(|word| !word.is_empty());
        Some((words.next()?, words.next().map_or(0, parse_number), words.next().map_or(-1, parse_number)))
    })
}

fn loop_record(name: &[u8], start: i64, end: i64) -> Option<[u8; 48]> {
    let mut record = [0; 48];
    if name.len() >= 32 {
        return None;
    }
    record[..name.len()].copy_from_slice(name);
    record[32..40].copy_from_slice(&start.to_le_bytes());
    record[40..48].copy_from_slice(&end.to_le_bytes());
    Some(record)
}

fn strip_comments(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    let mut cursor = 0;
    let mut block = false;
    while cursor < input.len() {
        if block {
            if input[cursor..].starts_with(b"*/") {
                block = false;
                cursor += 2;
            } else {
                if matches!(input[cursor], b'\r' | b'\n') {
                    output.push(input[cursor]);
                }
                cursor += 1;
            }
        } else if input[cursor..].starts_with(b"/*") {
            block = true;
            cursor += 2;
        } else if input[cursor..].starts_with(b"//") {
            while cursor < input.len() && !matches!(input[cursor], b'\r' | b'\n') {
                cursor += 1;
            }
        } else {
            output.push(input[cursor]);
            cursor += 1;
        }
    }
    output
}

fn parse_number(input: &[u8]) -> i64 {
    let (negative, input) = match input.first() {
        Some(b'-') => (true, &input[1..]),
        Some(b'+') => (false, &input[1..]),
        _ => (false, input),
    };
    let (radix, input) = if input.starts_with(b"0x") || input.starts_with(b"0X") {
        (16_u64, &input[2..])
    } else {
        (10_u64, input)
    };
    let mut value = 0_u64;
    for &byte in input {
        let digit = match byte {
            b'0'..=b'9' => u64::from(byte - b'0'),
            b'a'..=b'f' => u64::from(byte - b'a' + 10),
            b'A'..=b'F' => u64::from(byte - b'A' + 10),
            b'.' if radix == 10 => return 0,
            _ => break,
        };
        if digit >= radix {
            break;
        }
        value = value.wrapping_mul(radix).wrapping_add(digit);
    }
    if negative {
        value.wrapping_neg() as i64
    } else {
        value as i64
    }
}

#[cfg(test)]
mod tests {
    use super::{legacy_sound_loop_root_query, loop_record, loop_table_entries, strip_comments};

    #[test]
    fn legacy_sound_query_uses_the_exact_image_contract() {
        let tsunaban = "72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8";
        let djibril = "012c71635932c241cde4df986377516279a50b907166757e89ea7b86ff3f813f";
        let packed_sudden = "35ec21bcfcee0f324206e945eef1c082e35fd8af594ea0e738a46cd941f5b3f3";
        let packed_kanmi = "c264b2465889c78723079a694d2654ef1091253db657a735e92f58b75031aa26";
        assert_eq!(legacy_sound_loop_root_query(Some(tsunaban)), Some(30));
        assert_eq!(legacy_sound_loop_root_query(Some(djibril)), Some(32));
        assert_eq!(legacy_sound_loop_root_query(Some(packed_sudden)), None);
        assert_eq!(legacy_sound_loop_root_query(Some(packed_kanmi)), None);
        assert_eq!(legacy_sound_loop_root_query(None), None);
    }

    #[test]
    fn future_radio_long_loop_name_leaves_other_rows_spanning_it_intact() {
        let mut cleaned = strip_comments(
            b"\"bgm29_radio.ogg\"\t0\t5789539\r\nbgm29_radio_add_jingle\t345518\t6134753\r\n\"bgm30.ogg\"\t0\t5819322",
        );
        cleaned.retain(|&byte| byte != b'"');
        let rows = loop_table_entries(&cleaned)
            .map(|(name, start, end)| {
                let mut completed = b"bgm.int/".to_vec();
                completed.extend_from_slice(name);
                if !name.ends_with(b".ogg") {
                    completed.extend_from_slice(b".ogg");
                }
                loop_record(&completed, start, end)
            })
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 3);
        let before = rows[0].unwrap();
        assert_eq!(&before[..20], b"bgm.int/bgm29_radio.");
        assert_eq!(&before[40..], &5_789_539i64.to_le_bytes());
        assert_eq!(rows[1], None);
        let after = rows[2].unwrap();
        assert_eq!(&after[..18], b"bgm.int/bgm30.ogg\0");
        assert_eq!(&after[40..], &5_819_322i64.to_le_bytes());
    }

    #[test]
    fn loop_record_keeps_a_nul_after_a_31_byte_name() {
        let record = loop_record(&[b'a'; 31], -42, -1).unwrap();
        assert_eq!(&record[..31], &[b'a'; 31]);
        assert_eq!(record[31], 0);
        assert_eq!(&record[32..40], &(-42i64).to_le_bytes());
        assert_eq!(&record[40..48], &(-1i64).to_le_bytes());
        assert!(loop_record(&[b'a'; 32], 0, -1).is_none());
    }
}
