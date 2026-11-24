//! Native869..873/877/878 KCDB objects and persistent bit-state containers.
//! Raw serialization addresses belong to the existing host raw-buffer store.
use std::collections::BTreeMap;
use cs2_formats::database::DatabaseFile;
use cs2_script::{NativeReturn, Result, Vm};
use cs2_script::data_snapshot::{DatabaseObjectSnapshot, DatabaseRowSnapshot};
use super::{LoggingHost, expand_kcs_format, native_state_error, read_u32, trim_nul};
use super::fes_driver::{FesDriverEffect, FesScriptDriver};
use cs2_script::ExpressionState;

#[derive(Default)]
pub(super) struct DatabaseStore {
    objects: BTreeMap<u32, DatabaseObject>,
}

struct DatabaseObject {
    separators: [Vec<u8>; 3],
    rows: BTreeMap<Vec<u8>, DatabaseRow>,
    dirty: bool,
}

struct DatabaseRow {
    id: Vec<u8>,
    short_title: Vec<u8>,
    title: Vec<u8>,
    indications: Vec<Vec<u8>>,
    state: [u32; 3],
}

impl DatabaseStore {
    pub(super) fn snapshots(&self) -> Result<Vec<DatabaseObjectSnapshot>> {
        self.objects.iter().map(|(&handle, object)| {
            let saved = DatabaseObjectSnapshot {
                handle, separators: object.separators.clone(), dirty: object.dirty,
                rows: object.rows.iter().map(|(index, row)| DatabaseRowSnapshot {
                    index: index.clone(), id: row.id.clone(), short_title: row.short_title.clone(),
                    title: row.title.clone(), indications: row.indications.clone(), state: row.state,
                }).collect(),
            };
            saved.storage_bytes()?;
            Ok(saved)
        }).collect()
    }

    pub(super) fn from_snapshots(snapshots: &[DatabaseObjectSnapshot]) -> Result<Self> {
        let mut objects = BTreeMap::new();
        for saved in snapshots {
            saved.storage_bytes()?;
            let object = DatabaseObject {
                separators: saved.separators.clone(), dirty: saved.dirty,
                rows: saved.rows.iter().map(|row| (row.index.clone(), DatabaseRow {
                    id: row.id.clone(), short_title: row.short_title.clone(), title: row.title.clone(),
                    indications: row.indications.clone(), state: row.state,
                })).collect(),
            };
            if objects.insert(saved.handle, object).is_some() {
                return Err(native_state_error("database snapshot repeats an object identity"));
            }
        }
        Ok(Self { objects })
    }
}

/// The FES dictionary wrappers accept the same handle that KCS 869 returns.
/// KCS unlocks and CSS save/restore therefore affect FES immediately.
pub(super) fn apply_fes_effect(
    host: &mut LoggingHost<'_>,
    driver: &mut FesScriptDriver,
    effect: &FesDriverEffect,
    shared: &mut ExpressionState,
) -> anyhow::Result<bool> {
    let FesDriverEffect::Command(command) = effect else { return Ok(false); };
    let [verb, operands @ ..] = command.parameters.as_slice() else { return Ok(false); };
    let verb = verb.to_ascii_lowercase();
    let count = match verb.as_slice() {
        b"dic_find" => 6,
        b"dic_title" | b"dic_update" => 4,
        b"dic_appear" | b"dic_max_appear" | b"dic_id" | b"dic_ind" => 3,
        b"dic_update_clear" => 2,
        _ => return Ok(false),
    };
    if operands.len() < count { return Ok(true); }
    let mut number = |index: usize| driver.evaluate_parameter(&operands[index], command.context.current, shared);
    let handle = number(0)? as u32;
    let Some(object) = host.data_services.databases.objects.get_mut(&handle) else {
        return Ok(true);
    };

    let is_find = verb == b"dic_find";
    let first = if count > 2 { number(1)? } else { 0 };
    let second = if count >= 4 { number(2)? } else { 0 };
    if is_find {
        let ordinal = number(3)? as usize;
        let bound = |bytes: &[u8]| {
            // The localized Newton tips script retains EUC-JP kana boundaries.
            // Convert those two-byte constants to the database's CP932 order.
            if host.data_services.newton_legacy_gbk
                && let [0xa4, trail @ 0xa1..=0xf3] = bytes
            {
                return 0x829f + u16::from(trail - 0xa1);
            }
            dictionary_head(bytes)
        };
        let start = bound(&operands[4]);
        let end = bound(&operands[5]);
        let found = object.rows.keys()
            .skip_while(|key| !(start..=end).contains(&dictionary_head(key)))
            .take_while(|key| dictionary_head(key) <= end)
            .nth(ordinal);
        write_fes_string(shared, second, found.map_or_else(Vec::new, Vec::clone));
        write_fes_number(shared, first, u32::from(found.is_some()));
        return Ok(true);
    }
    let key = normalize_key(trim_nul(&operands[count - 1]))?;
    let row = object.rows.get(&key);
    match verb.as_slice() {
        b"dic_appear" => write_fes_number(shared, first, row.map_or(0, |row| row.state[0])),
        b"dic_max_appear" => {
            let mask = row.map_or(0, |row| row.indications.iter().take(24).enumerate()
                .filter(|(_, text)| !text.is_empty())
                .fold(0, |mask, (bit, _)| mask | (1 << bit)));
            write_fes_number(shared, first, mask);
        }
        b"dic_id" => {
            let ordinal = object.rows.keys().position(|index| *index == key).map_or(0, |i| i + 1);
            write_fes_number(shared, first, ordinal as u32);
        }
        b"dic_title" => {
            write_fes_string(shared, first, row.map_or_else(Vec::new, |row| row.title.clone()));
            write_fes_string(shared, second, row.map_or_else(Vec::new, |row| row.short_title.clone()));
        }
        b"dic_ind" => {
            let mut output = Vec::new();
            if let Some(row) = row {
                for (bit, text) in row.indications.iter().take(24).enumerate() {
                    if row.state[0] & (1 << bit) != 0 && !text.is_empty() {
                        output.extend_from_slice(&object.separators[1]);
                        output.extend_from_slice(text);
                        output.extend_from_slice(&object.separators[2]);
                    }
                }
            }
            write_fes_string(shared, first, output);
        }
        b"dic_update" => {
            let [_, updated_bits, new] = row.map_or([0; 3], |row| row.state);
            write_fes_number(shared, second, updated_bits);
            write_fes_number(shared, first, new);
        }
        b"dic_update_clear" => {
            if let Some(row) = object.rows.get_mut(&key) {
                if row.state[1..] != [0, 0] {
                    row.state[1..].fill(0);
                    object.dirty = true;
                }
            }
        }
        _ => unreachable!(),
    }
    Ok(true)
}

fn dictionary_head(bytes: &[u8]) -> u16 {
    match bytes {
        [lead @ (0x81..=0x9f | 0xe0..=0xfc), trail, ..] => u16::from_be_bytes([*lead, *trail]),
        [byte, ..] => u16::from(*byte),
        [] => 0,
    }
}

fn write_fes_number(shared: &mut ExpressionState, index: i32, value: u32) {
    if let Some(slot) = shared.shared.get_mut(index as usize) { *slot = value as i32; }
}

fn write_fes_string(shared: &mut ExpressionState, index: i32, value: Vec<u8>) {
    if index >= 0 { shared.strings.insert(index as u32, value); }
}

impl LoggingHost<'_> {
    pub(super) fn database_native(&mut self, index: u32, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        if index == 869 {
            let name = formatted(vm, read_u32(args, 0)?, 260)?;
            let separators = [formatted(vm, read_u32(args, 4)?, 32)?,
                formatted(vm, read_u32(args, 8)?, 32)?, formatted(vm, read_u32(args, 12)?, 32)?];
            let mut object = DatabaseObject { separators, rows: BTreeMap::new(), dirty: false };
            let name = self.text_encoding.decode(&name);
            let names = self.resources.category_names(b"database", &name)
                .map_err(|_| native_state_error("native 869 database category resolution failed"))?;
            let mut bytes = None;
            for name in names {
                if let Some(cached) = self.data_services.ordinary.cached_resource(&name) {
                    bytes = Some(cached.to_vec()); break;
                }
                if let Some(candidate) = self.resources.resolve_mode_utf16(
                    &name.encode_utf16().collect::<Vec<_>>(), cs2_runtime::resource_router::ResourceMode::LooseFirst,
                ).map_err(|_| native_state_error("KCDB database resource routing failed"))? {
                    let candidate = candidate.value;
                    if candidate.size > 64 * 1024 * 1024 {
                        return Err(native_state_error("KCDB database resource exceeds 64 MiB"));
                    }
                    bytes = Some(self.resources.read_candidate(candidate)
                        .map_err(|_| native_state_error("KCDB database resource read failed"))?);
                    break;
                }
            }
            // The wrapper retains a real empty object on missing resource.
            if let Some(bytes) = bytes {
                let database = DatabaseFile::parse(&bytes)
                    .map_err(|_| native_state_error("KCDB database resource is malformed"))?;
                for row in 0..database.row_count() {
                    let Some(id) = database.field(row, b"ID") else { break; };
                    if id.is_empty() { break; }
                    if id.len() > 120 { return Err(native_state_error("KCDB ID exceeds the original metadata buffer")); }
                    let key = text_field(&database, row, b"INDEX", 2048)?;
                    let key = normalize_key(&key)?;
                    if key.len() >= 256 { return Err(native_state_error("KCDB index exceeds its serialized name field")); }
                    let short_title = text_field(&database, row, b"TITLE_S", 2048)?;
                    let title = text_field(&database, row, b"TITLE", 2048)?;
                    let mut indications = Vec::with_capacity(24);
                    for number in 1..=24 {
                        let field = format!("IND{number}");
                        let text = text_field(&database, row, field.as_bytes(), 2048)?;
                        indications.push(if object.separators[0] == b"\\n" { text }
                            else { replace_newline(&text, &object.separators[0])? });
                    }
                    object.rows.entry(key).or_insert(DatabaseRow {
                        id: id.to_vec(), short_title, title, indications,
                        state: [108, 112, 116].map(|offset| id.get(offset..offset + 4)
                            .map_or(0, |bytes| u32::from_le_bytes(bytes.try_into().unwrap()))),
                    });
                }
            }
            let handle = self.allocate_handle();
            self.data_services.databases.objects.insert(handle, object);
            return Ok(NativeReturn::Value(handle));
        }
        let handle = read_u32(args, 0)?;
        if handle == 0 {
            return Ok(if matches!(index, 872 | 873 | 877) { NativeReturn::Value(0) } else { NativeReturn::Continue });
        }
        if index == 870 {
            self.data_services.databases.objects.remove(&handle)
                .ok_or_else(|| native_state_error("native 870 invalid/wrong-type database object"))?;
            return Ok(NativeReturn::Continue);
        }
        if !self.data_services.databases.objects.contains_key(&handle) {
            return Err(native_state_error("database operation has an invalid/wrong-type object"));
        }
        if index == 877 {
            return Ok(NativeReturn::Value(u32::from(self.data_services.databases.objects[&handle].dirty)));
        }
        if index == 878 {
            // 0x810d10 acknowledges the dirty flag only; row states survive.
            self.data_services.databases.objects.get_mut(&handle).unwrap().dirty = false;
            return Ok(NativeReturn::Continue);
        }
        if index == 871 {
            let bit = read_u32(args, 4)? & 31;
            let key = normalize_key(&formatted(vm, read_u32(args, 8)?, 2048)?)?;
            let object = self.data_services.databases.objects.get_mut(&handle).unwrap();
            if let Some(row) = object.rows.get_mut(&key) {
                if row.state[0] == 0 { row.state[2] = 1; object.dirty = true; }
                let mask = 1u32 << bit;
                if row.state[0] & mask == 0 {
                    row.state[0] |= mask; row.state[1] |= mask; object.dirty = true;
                }
            }
            return Ok(NativeReturn::Continue);
        }
        let address = read_u32(args, 4)?;
        if index == 872 {
            let object = &self.data_services.databases.objects[&handle];
            let length = object.rows.len().checked_mul(268).and_then(|length| length.checked_add(4))
                .filter(|&length| length <= 64 * 1024 * 1024)
                .ok_or_else(|| native_state_error("database state output exceeds 64 MiB"))?;
            if address != 0 {
                let mut output = vec![0; length];
                output[..4].copy_from_slice(&(object.rows.len() as u32).to_le_bytes());
                for (index, (name, row)) in object.rows.iter().enumerate() {
                    let offset = 4 + index * 268;
                    output[offset..offset + name.len()].copy_from_slice(name);
                    for (number, value) in row.state.iter().enumerate() {
                        output[offset + 256 + number * 4..offset + 260 + number * 4]
                            .copy_from_slice(&value.to_le_bytes());
                    }
                }
                self.data_services.raw_buffer_mut(address, length)?.copy_from_slice(&output);
            }
            return Ok(NativeReturn::Value(length as u32));
        }
        if index == 873 {
            if address == 0 { return Ok(NativeReturn::Value(0)); }
            let count = read_u32(self.data_services.raw_buffer(address, 4)?, 0)? as usize;
            let length = count.checked_mul(268).and_then(|length| length.checked_add(4))
                .filter(|&length| count <= 1_000_000 && length <= 64 * 1024 * 1024)
                .ok_or_else(|| native_state_error("database state input exceeds its size/count limit"))?;
            let mut bytes = self.data_services.raw_buffer(address, length)?.to_vec();
            let mut restored = Vec::with_capacity(count);
            for record in bytes[4..].chunks_exact_mut(268) {
                let end = record[..256].iter().position(|&byte| byte == 0)
                    .ok_or_else(|| native_state_error("database state index has no terminator"))?;
                let key = normalize_key(&record[..end])?;
                // 0x811ff3 also normalizes the caller's RAW buffer in place,
                // including keys that are absent from the database.
                record[..end].copy_from_slice(&key);
                restored.push((key, [read_u32(record, 256)?, read_u32(record, 260)?, read_u32(record, 264)?]));
            }
            self.data_services.raw_buffer_mut(address, length)?.copy_from_slice(&bytes);
            let object = self.data_services.databases.objects.get_mut(&handle).unwrap();
            for (key, state) in restored {
                if let Some(row) = object.rows.get_mut(&key) { row.state = state; }
            }
            return Ok(NativeReturn::Value(1));
        }
        Err(cs2_script::ScriptError::UnimplementedNativeCall { index })
    }
}

fn text_field(database: &DatabaseFile, row: usize, name: &[u8], capacity: usize) -> Result<Vec<u8>> {
    let Some(bytes) = database.field(row, name) else { return Ok(Vec::new()); };
    if bytes.len() > capacity { return Err(native_state_error("KCDB text field exceeds its original fixed buffer")); }
    Ok(trim_nul(bytes).to_vec())
}

fn formatted(vm: &Vm, address: u32, capacity: usize) -> Result<Vec<u8>> {
    let value = expand_kcs_format(vm, &vm.read_string(address)?, 0)?;
    let value = trim_nul(&value);
    if value.len() >= capacity { return Err(native_state_error("database formatted text exceeds its fixed buffer")); }
    Ok(value.to_vec())
}

fn replace_newline(text: &[u8], separator: &[u8]) -> Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        if text[cursor..].starts_with(b"\\n") {
            result.extend_from_slice(separator); cursor += 2;
        } else { result.push(text[cursor]); cursor += 1; }
        if result.len() > 64 * 1024 * 1024 { return Err(native_state_error("database indication replacement exceeds 64 MiB")); }
    }
    Ok(result)
}

fn normalize_key(key: &[u8]) -> Result<Vec<u8>> {
    const PAIRS: &[(u8, u8)] = &[
        (0xaa,0xa9),(0xac,0xab),(0xae,0xad),(0xb0,0xaf),(0xb2,0xb1),
        (0xb4,0xb3),(0xb6,0xb5),(0xb8,0xb7),(0xba,0xb9),(0xbc,0xbb),
        (0xbe,0xbd),(0xc0,0xbf),(0xc3,0xc2),(0xc5,0xc4),(0xc7,0xc6),
        (0xce,0xcd),(0xd1,0xd0),(0xd4,0xd3),(0xd7,0xd6),(0xda,0xd9),
        (0xcf,0xcd),(0xd2,0xd0),(0xd5,0xd3),(0xd8,0xd6),(0xdb,0xd9),
        (0x9f,0xa0),(0xa1,0xa2),(0xa3,0xa4),(0xa5,0xa6),(0xa7,0xa8),
        (0xe1,0xe2),(0xe3,0xe4),(0xe5,0xe6),(0xc1,0xc2),
    ];
    let mut result = key.to_vec();
    let mut cursor = 0;
    while cursor < result.len() {
        let byte = result[cursor];
        if matches!(byte, 0x81..=0x9f | 0xe0..=0xfc) {
            if cursor + 1 == result.len() { return Err(native_state_error("database index ends with an incomplete CP932 pair")); }
            if byte == 0x82 {
                if let Some(&(_, replacement)) = PAIRS.iter().find(|&&(source, _)| source == result[cursor + 1]) {
                    result[cursor + 1] = replacement;
                }
            }
            cursor += 2;
        } else { cursor += 1; }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HeadlessPlayer;
    use cs2_script::Host;

    #[test]
    fn fes_dictionary_queries_real_newton_qdb_and_kcs_unlocks() -> anyhow::Result<()> {
        let game_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../games/\u{725b}\u{987f}\u{4e0e}\u{82f9}\u{679c}\u{6811}");
        if !game_root.join("tips.qdb").is_file() { return Ok(()); }
        let root = std::env::temp_dir().join(format!(
            "cs2-dictionary-test-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()
        ));
        std::fs::create_dir(&root)?;
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
        }
        let fixture = Fixture(root);
        std::fs::create_dir(fixture.0.join("qdb"))?;
        std::fs::copy(game_root.join("tips.qdb"), fixture.0.join("qdb/tips.qdb"))?;
        std::fs::write(fixture.0.join("startup.xml"), b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><document><INT><int00><name>database</name><int>qdb.int</int><ext>qdb</ext><dir>qdb</dir></int00></INT></document>")?;
        let mut player = HeadlessPlayer::open_with_save_root(&fixture.0, fixture.0.join("profile"))?;
        let owner = player.current_task_id;
        let (handle, ordinal, id) = player.with_host(|host| {
            let mut vm = Vm::new();
            vm.load_program(cs2_script::KcsProgram::from_code(&[0x3b], vec![0; 256], 64)?, vec![])?;
            vm.write(8, b"tips\0")?;
            vm.write(24, b"\\n\0")?;
            vm.write(40, b"[\0")?;
            vm.write(56, b"]\0")?;
            let args = [8_u32, 24, 40, 56].map(u32::to_le_bytes).concat();
            let NativeReturn::Value(handle) = host.call(869, &args, &mut vm)? else {
                anyhow::bail!("KCDB create failed");
            };
            anyhow::ensure!(host.data_services.databases.objects[&handle].rows.len() > 100);
            vm.write(72, b"\x82\xa0\x82\xa2\0")?;
            let args = [handle, 0, 72].map(u32::to_le_bytes).concat();
            host.call(871, &args, &mut vm)?;
            let rows = &host.data_services.databases.objects[&handle].rows;
            let key = b"\x82\xa0\x82\xa2";
            let ordinal = rows.keys().filter(|index| (0x82a0..=0x82a8).contains(&dictionary_head(index)))
                .position(|index| index.as_slice() == key).unwrap();
            let id = rows.keys().position(|index| index.as_slice() == key).unwrap() + 1;
            Ok((handle, ordinal, id))
        })?;
        let lines = [
            format!("$99 = {handle}").into_bytes(),
            b"strvar 20 \x82\xa0".to_vec(),
            b"strvar 21 \x82\xa8".to_vec(),
            format!("dic_find $99 40 30 {ordinal} $str20 $str21").into_bytes(),
            b"dic_id $99 41 $str30".to_vec(),
            b"dic_appear $99 42 $str30".to_vec(),
            b"dic_max_appear $99 43 $str30".to_vec(),
            b"dic_title $99 31 32 $str30".to_vec(),
            b"dic_ind $99 33 $str30".to_vec(),
            b"dic_update $99 44 45 $str30".to_vec(),
            b"dic_update_clear $99 $str30".to_vec(),
            b"dic_update $99 46 47 $str30".to_vec(),
            b"wait 1".to_vec(),
        ];
        let document = cs2_script::FesDocument {
            sections: vec![cs2_script::FesSection {
                header: b"START".to_vec(),
                lines: lines.into_iter().enumerate().map(|(offset, bytes)|
                    cs2_script::FesLine { offset, bytes }).collect(),
            }],
        };
        player.with_host(|host| Ok(host.begin_direct_fes_entry(owner, document, Default::default())?))?;
        player.run_frames(1, 10_000)?;
        player.with_host(|host| {
            let state = &host.data_services.databases.objects[&handle].rows[b"\x82\xa0\x82\xa2".as_slice()].state;
            assert_eq!(*state, [1, 0, 0]);
            assert_eq!(host.shared_values[40..48], [1, id as u32, 1, 1, 1, 1, 0, 0]);
            assert_eq!(host.string_values.get(&30).map(Vec::as_slice), Some(b"\x82\xa0\x82\xa2".as_slice()));
            assert_eq!(host.string_values.get(&31).map(Vec::as_slice), Some(b"\x88\xa4".as_slice()));
            assert_eq!(host.string_values.get(&32).map(Vec::as_slice), Some(b"\x82\xa0\x82\xa2".as_slice()));
            assert!(host.string_values.get(&33).is_some_and(|value| value.starts_with(b"[")));
            Ok(())
        })?;
        Ok(())
    }
}
