//! Native869..873/877/878 KCDB objects and persistent bit-state containers.
//! Raw serialization addresses belong to the existing host raw-buffer store.
use std::collections::BTreeMap;
use cs2_formats::database::DatabaseFile;
use cs2_script::{NativeReturn, Result, Vm};
use cs2_script::data_snapshot::{DatabaseObjectSnapshot, DatabaseRowSnapshot};
use super::{LoggingHost, expand_kcs_format, native_state_error, read_u32, trim_nul};

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
