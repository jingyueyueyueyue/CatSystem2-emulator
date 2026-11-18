//! Persistent ordinary-native data services, independently implemented from
//! documented contracts. This module does not depend on a reference engine.
//!
//! The host must pass the same monotonic handle counter to every allocator,
//! including allocators outside this module. Raw allocations advance it by
//! their entire reserved range; their addresses are never VM addresses.

use std::collections::BTreeMap;

use cs2_formats::CssFile;
use cs2_runtime::{ResourceIndex, RuntimeError};
use cs2_script::{NativeReturn, Result, ScriptError, Vm};

use super::{ScriptTextEncoding, expand_kcs_format};

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_RAW_TOTAL: usize = 256 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;
const MAX_FIELD: usize = 1023;

type Dictionaries = BTreeMap<u32, BTreeMap<Vec<u8>, u32>>;
type Containers = BTreeMap<u32, CssFile>;

#[derive(Debug, Clone, Default)]
struct TripleTable {
    groups: BTreeMap<Vec<u8>, [u32; 3]>,
    entries: BTreeMap<Vec<u8>, [u32; 3]>,
}

#[derive(Debug, Clone)]
struct NameRow {
    id: u32,
    name: Vec<u8>,
    additional: [Vec<u8>; 2],
    aliases: Vec<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct RawBlock {
    bytes: Vec<u8>,
    reserved_end: u32,
}

pub(super) struct PreparedRawStorage {
    blocks: BTreeMap<u32, RawBlock>,
    bytes: usize,
}

pub(super) struct PreparedDataTables {
    triples: BTreeMap<u32, TripleTable>,
    names: BTreeMap<u32, Vec<NameRow>>,
}

#[derive(Default)]
pub(super) struct DataServices {
    pub(super) achievements: super::achievement_native::AchievementState,
    pub(super) top_clock: super::top_task_native::TopClockState,
    pub(super) frame_capture: super::frame_capture::FrameCaptureState,
    pub(super) host_input_coordinates: Option<cs2_runtime::input_coordinates::InputCoordinateMap>,
    pub(super) host_quit_requested: bool,
    pub(super) window_control: super::window_control::WindowControlState,
    pub(super) clipboard: super::clipboard_native::ClipboardState,
    pub(super) system_open: super::system_open_native::SystemOpenState,
    pub(super) dialogs: super::dialog_native::DialogState,
    pub(super) wallpapers: super::wallpaper_native::WallpaperStore,
    pub(super) tweets: super::tweet_native::TweetStore,
    pub(super) databases: super::database_native::DatabaseStore,
    pub(super) cst_windows: super::cst_window_native::CstWindowStore,
    pub(super) emote: super::emote_native::EmoteNativeServices,
    pub(super) effekseer: super::effekseer_native::EffekseerStore,
    pub(super) emocon: super::emocon_native::EmoconStore,
    pub(super) configuration: super::configuration::ConfigurationState,
    pub(super) fes_motion: super::fes_motion::FesMotionStore,
    pub(super) pending_host_input: std::collections::VecDeque<cs2_runtime::InputEvent>,
    pub(super) host_pause: super::sound_control::HostPauseState,
    pub(super) fes_controls: super::fes_controls::FesControlStore,
    pub(super) fes_text: super::fes_text::FesTextStore,
    pub(super) fes_sound: super::fes_sound::FesSoundStore,
    pub(super) fes_particles: super::fes_particle::FesParticleStore,
    pub(super) fes_globals: super::fes_globals::FesGlobalStore,
    pub(super) fes_settings: super::fes_settings::FesSettingsStore,
    pub(super) fes_draw: super::fes_draw::FesDrawStore,
    pub(super) sounds: super::sound_task::SoundTaskStore,
    pub(super) legacy_sound: super::sound_legacy::LegacySoundSettings,
    pub(super) sound_media: super::sound_media::SoundMediaStore,
    pub(super) host_clock: super::media_clock::HostClock,
    pub(super) list_native: super::list_native::ListNativeStore,
    pub(super) canvas_native: super::canvas_native::CanvasNativeState,
    pub(super) transitions: super::transition_native::TransitionStore,
    pub(super) files: super::file_native::FileNativeStore,
    pub(super) planes: super::plane_native::PlaneServices,
    pub(super) image_wrapper_owners: BTreeMap<u32, u32>,
    pub(super) image_wrapper_pixels: BTreeMap<u32, u32>,
    pub(super) image_control_values: BTreeMap<u32, u32>,
    pub(super) image_export: super::image_export_native::ImageExportState,
    pub(super) image_manager: super::image_manager::ImageManager,
    pub(super) image_record_policy: super::resource_direct_native::ImageRecordPolicy,
    pub(super) particle_external: super::particle_external::ParticleExternalStore,
    pub(super) particle_message_pe_sha256: Option<&'static str>,
    pub(super) native_profile: Option<&'static cs2_runtime::native_profile::FeatureProfile>,
    pub(super) css_task_pe_sha256: Option<&'static str>,
    pub(super) css_models: super::css_model::CssModelStore,
    pub(super) language: super::language_native::LanguageNativeServices,
    pub(super) messages: super::message_native::MessageNativeServices,
    pub(super) message_device: super::message_backend::DesktopMessageDevice,
    pub(super) tasks: super::native_tasks::NativeTaskStore,
    pub(super) ordinary: super::ordinary_native::OrdinaryNativeServices,
    pub(super) memory_copy_mode: cs2_runtime::NativeMemoryCopyMode,
    triples: BTreeMap<u32, TripleTable>,
    names: BTreeMap<u32, Vec<NameRow>>,
    raw: BTreeMap<u32, RawBlock>,
    raw_bytes: usize,
}

impl DataServices {
    pub(crate) fn graph_opaque_data_count(&self) -> usize {
        self.effekseer.manager.task_count() + self.css_models.owners.len()
            + self.fes_particles.graph_object_count()
    }

    pub(super) fn triple_snapshots(&self) -> BTreeMap<u32, cs2_script::data_snapshot::TripleTableSnapshot> {
        self.triples.iter().map(|(&handle, table)| (handle,
            cs2_script::data_snapshot::TripleTableSnapshot {
                groups: table.groups.clone(), entries: table.entries.clone(),
            })).collect()
    }

    pub(super) fn name_snapshots(&self) -> BTreeMap<u32, Vec<cs2_script::data_snapshot::NameRowSnapshot>> {
        self.names.iter().map(|(&handle, rows)| (handle, rows.iter().map(|row|
            cs2_script::data_snapshot::NameRowSnapshot { id: row.id, name: row.name.clone(),
                additional: row.additional.clone(), aliases: row.aliases.clone() }).collect())).collect()
    }

    /// The enclosing group has already checked limits and cross-type identities.
    pub(super) fn prepare_table_restore(saved: &cs2_script::data_snapshot::ScriptDataObjectsSnapshot) -> PreparedDataTables {
        PreparedDataTables {
            triples: saved.triple_tables.iter().map(|(&handle, table)| (handle,
                TripleTable { groups: table.groups.clone(), entries: table.entries.clone() })).collect(),
            names: saved.name_tables.iter().map(|(&handle, rows)| (handle, rows.iter().map(|row|
                NameRow { id: row.id, name: row.name.clone(), additional: row.additional.clone(),
                    aliases: row.aliases.clone() }).collect())).collect(),
        }
    }

    pub(super) fn commit_table_restore(&mut self, prepared: PreparedDataTables) {
        self.triples = prepared.triples;
        self.names = prepared.names;
    }
    pub(super) fn raw_snapshots(&self) -> Vec<cs2_script::data_snapshot::RawBlockSnapshot> {
        self.raw.iter().map(|(&base, block)| cs2_script::data_snapshot::RawBlockSnapshot {
            base, reserved_end: block.reserved_end, bytes: block.bytes.clone(),
        }).collect()
    }

    /// Caller validates the enclosing data group, including object collisions,
    /// before preparing this storage. No allocator identities are regenerated.
    pub(super) fn prepare_raw_restore(
        snapshots: &[cs2_script::data_snapshot::RawBlockSnapshot],
    ) -> PreparedRawStorage {
        PreparedRawStorage {
            bytes: snapshots.iter().map(|block| block.bytes.len()).sum(),
            blocks: snapshots.iter().map(|block| (block.base, RawBlock {
                bytes: block.bytes.clone(), reserved_end: block.reserved_end,
            })).collect(),
        }
    }

    pub(super) fn commit_raw_restore(&mut self, prepared: PreparedRawStorage) {
        self.raw = prepared.blocks;
        self.raw_bytes = prepared.bytes;
    }

    pub(super) fn set_sequence_name_mode(&mut self, mode: u32) {
        self.ordinary.sequence_name_mode = mode;
    }

    pub(super) fn supports(index: u32) -> bool {
        super::ordinary_native::OrdinaryNativeServices::supports(index)
            || matches!(index, 290 | 594..=597 | 633..=640 | 677..=680 | 833..=836 | 891 | 892 | 900 | 901)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn call(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
        resources: &ResourceIndex,
        containers: &mut Containers,
        dictionaries: &mut Dictionaries,
        next_handle: &mut u32,
        unicode_mode: bool,
        encoding: ScriptTextEncoding,
    ) -> Result<NativeReturn> {
        if super::ordinary_native::OrdinaryNativeServices::supports(index) {
            return self.ordinary.call(index, args, vm, resources, containers, next_handle);
        }
        match index {
            677..=680 => {
                let handle = argument(args, 0)?;
                if index == 680 {
                    // Its output address is resolved even on the zero-map
                    // branch, but only a found value dereferences it.
                    let output = argument(args, 2)?;
                    if handle == 0 { return Ok(NativeReturn::Value(0)); }
                    let map = dictionaries.get(&handle).ok_or_else(|| error(
                        "native 680: invalid or wrong-type integer-map handle",
                    ))?;
                    let value = argument(args, 1)?;
                    let Some((key, _)) = map.iter().find(|(_, candidate)| **candidate == value) else {
                        return Ok(NativeReturn::Value(0));
                    };
                    write_string(vm, output, key)?;
                    return Ok(NativeReturn::Value(1));
                }
                // 679 reads the default value before checking the map.
                let default = if index == 679 { argument(args, 2)? } else { 0 };
                if handle == 0 {
                    return Ok(if index == 679 {
                        NativeReturn::Value(default)
                    } else {
                        NativeReturn::Continue
                    });
                }
                let key = formatted(vm, argument(args, 1)?)?;
                let map = dictionaries.get_mut(&handle).ok_or_else(|| error(
                    "native 677..679: invalid or wrong-type integer-map handle",
                ))?;
                match index {
                    677 => {
                        let value = argument(args, 2)?;
                        if map.len() >= MAX_ITEMS && !map.contains_key(&key) {
                            return Err(error("native 677: integer-map entry limit exceeded"));
                        }
                        map.insert(key, value);
                        Ok(NativeReturn::Continue)
                    }
                    678 => {
                        map.remove(&key);
                        Ok(NativeReturn::Continue)
                    }
                    679 => Ok(NativeReturn::Value(map.get(&key).copied().unwrap_or(default))),
                    _ => unreachable!(),
                }
            }
            639 | 640 => {
                let handle = argument(args, 0)?;
                if handle == 0 { return Ok(NativeReturn::Value(0)); }
                let row_index = argument(args, 1)? as usize;
                let input = argument(args, 2)?;
                let output = if index == 640 { Some(argument(args, 3)?) } else { None };
                let row = self.names.get(&handle).ok_or_else(|| error(
                    "native 639/640: invalid or wrong-type name-table handle",
                ))?.get(row_index).ok_or_else(|| error(
                    "native 639/640: row has no format strings (original dereferences null)",
                ))?;
                if index == 639 {
                    let source = vm.read_string(input)?;
                    Ok(NativeReturn::Value(u32::from(super::name_table_format::matches(
                        &source, &row.additional[0],
                    )?)))
                } else {
                    super::name_table_format::format(
                        vm, input, output.expect("640 has an output"),
                        &row.additional[0], &row.additional[1],
                    )?;
                    Ok(NativeReturn::Value(1))
                }
            }
            594 => {
                let name = resource_name(vm, argument(args, 0)?)?;
                let Some(bytes) = load_resource(resources, None, &name)? else {
                    return Ok(NativeReturn::Value(0));
                };
                let table = parse_triples(&bytes)?;
                let (handle, next) = self.reserve(*next_handle, 1, 1, containers, dictionaries)?;
                self.triples.insert(handle, table);
                *next_handle = next;
                Ok(NativeReturn::Value(handle))
            }
            595 => {
                let handle = argument(args, 0)?;
                if handle != 0 && self.triples.remove(&handle).is_none() {
                    return Err(error(
                        "native 595: invalid or wrong-type triple-table handle",
                    ));
                }
                Ok(NativeReturn::Continue)
            }
            596 | 597 => {
                let handle = argument(args, 0)?;
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let table = self.triples.get(&handle).ok_or_else(|| {
                    error("native 596/597: invalid or wrong-type triple-table handle")
                })?;
                let name = formatted(vm, argument(args, 1)?)?;
                let maximum = if index == 596 { 511 } else { 1023 };
                if name.len() > maximum {
                    return Err(error("native 596/597: query name exceeds its byte limit"));
                }
                let key = folded_name(&name);
                let found = if index == 596 {
                    table.groups.get(&key)
                } else {
                    table.entries.get(&key)
                };
                let Some(values) = found.copied() else {
                    return Ok(NativeReturn::Value(0));
                };
                // Deliberately validate and write in parameter order. A bad
                // second/third output must retain earlier successful writes.
                for (component, value) in values.into_iter().enumerate() {
                    vm.write(argument(args, component + 2)?, &value.to_le_bytes())?;
                }
                Ok(NativeReturn::Value(1))
            }
            633 => {
                let name = resource_name(vm, argument(args, 0)?)?;
                let Some(bytes) = load_resource(resources, Some(b"fes"), &name)? else {
                    return Ok(NativeReturn::Value(0));
                };
                if bytes.is_empty() {
                    return Ok(NativeReturn::Value(0));
                }
                let rows = parse_names(&bytes, unicode_mode, encoding)?;
                let (handle, next) = self.reserve(*next_handle, 1, 1, containers, dictionaries)?;
                self.names.insert(handle, rows);
                *next_handle = next;
                Ok(NativeReturn::Value(handle))
            }
            634 => {
                let handle = argument(args, 0)?;
                if handle != 0 && self.names.remove(&handle).is_none() {
                    return Err(error("native 634: invalid or wrong-type name-table handle"));
                }
                Ok(NativeReturn::Continue)
            }
            635..=638 => {
                let handle = argument(args, 0)?;
                if handle == 0 {
                    return Ok(NativeReturn::Value(if index <= 636 { u32::MAX } else { 0 }));
                }
                let rows = self.names.get(&handle).ok_or_else(|| {
                    error("native 635..638: invalid or wrong-type name-table handle")
                })?;
                if index == 635 {
                    let query = formatted(vm, argument(args, 1)?)?;
                    return Ok(NativeReturn::Value(
                        rows.iter()
                            .position(|row| row.aliases.iter().any(|alias| alias == &query))
                            .map_or(u32::MAX, |position| position as u32),
                    ));
                }
                let Some(row) = rows.get(argument(args, 1)? as usize) else {
                    return Ok(NativeReturn::Value(if index == 636 { u32::MAX } else { 0 }));
                };
                match index {
                    636 => Ok(NativeReturn::Value(row.id)),
                    637 => {
                        write_string(vm, argument(args, 2)?, &row.name)?;
                        Ok(NativeReturn::Value(1))
                    }
                    638 => {
                        let additional = argument(args, 2)? as usize;
                        let Some(value) = row.additional.get(additional) else {
                            return Ok(NativeReturn::Value(0));
                        };
                        let target = argument(args, 3)?;
                        // This entry treats an invalid *start* as successful
                        // no-write; a valid start with insufficient tail room
                        // still diagnoses the complete C-string write.
                        if vm.read(target, 1).is_ok() {
                            write_string(vm, target, value)?;
                        }
                        Ok(NativeReturn::Value(1))
                    }
                    _ => unreachable!(),
                }
            }
            833 => {
                let length = argument(args, 0)? as usize;
                if length > MAX_BYTES {
                    return Err(error("native 833: raw allocation exceeds 64 MiB"));
                }
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(length)
                    .map_err(|_| error("native 833: raw allocation failed"))?;
                bytes.resize(length, 0);
                let address = self.allocate_raw(bytes, next_handle, containers, dictionaries)?;
                Ok(NativeReturn::Value(address))
            }
            834 | 290 => {
                let address = argument(args, 0)?;
                if address != 0 {
                    let block = self.raw.remove(&address).ok_or_else(|| {
                        error("native 834/290: address is not a live raw-allocation base")
                    })?;
                    self.raw_bytes -= block.bytes.len();
                }
                Ok(NativeReturn::Continue)
            }
            835 => {
                let handle = argument(args, 0)?;
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let record = argument(args, 1)?;
                let length = argument(args, 2)? as usize;
                let address = argument(args, 3)?;
                if address == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let container = containers.get_mut(&handle).ok_or_else(|| {
                    error("native 835: invalid or wrong-type data-container handle")
                })?;
                let source = self.raw_buffer(address, length)?;
                container
                    .append(record, source)
                    .map_err(|_| error("native 835: container append exceeds its size limit"))?;
                Ok(NativeReturn::Value(1))
            }
            836 => {
                let handle = argument(args, 0)?;
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let container = containers.get(&handle).ok_or_else(|| {
                    error("native 836: invalid or wrong-type data-container handle")
                })?;
                let Some(source) = container.try_record(argument(args, 1)?)
                    .map_err(|_| error("native 836: malformed CSS record storage"))? else {
                    return Ok(NativeReturn::Value(0));
                };
                if source.len() > MAX_BYTES {
                    return Err(error("native 836: record exceeds the raw-allocation limit"));
                }
                let target = argument(args, 2)?;
                // Validate the complete length slot before allocating. Address
                // zero is a valid VM location, not a null pointer here.
                vm.read(target, 4)?;
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(source.len())
                    .map_err(|_| error("native 836: raw allocation failed"))?;
                bytes.extend_from_slice(source);
                let length = bytes.len() as u32;
                let address = self.allocate_raw(bytes, next_handle, containers, dictionaries)?;
                // The VM's storage sizes cannot change between the range
                // validation and this write.
                vm.write(target, &length.to_le_bytes())?;
                Ok(NativeReturn::Value(address))
            }
            891 | 900 => {
                let handle = argument(args, 0)?;
                if handle == 0 {
                    return Ok(NativeReturn::Continue);
                }
                let address = argument(args, 1)?;
                if address == 0 {
                    return Ok(NativeReturn::Continue);
                }
                let dictionary = dictionaries
                    .get(&handle)
                    .ok_or_else(|| error("native 900: invalid or wrong-type dictionary handle"))?;
                let length = Self::dictionary_size(dictionary)? as usize;
                let output = self.raw_buffer_mut(address, length)?;
                output[..4].copy_from_slice(&(dictionary.len() as u32).to_le_bytes());
                let mut cursor = 4usize;
                // BTreeMap<Vec<u8>, _> uses the required unsigned byte order.
                for (key, value) in dictionary {
                    let key_span = key_span(key)?;
                    output[cursor..cursor + key.len()].copy_from_slice(key);
                    output[cursor + key.len()] = 0;
                    // Leave alignment padding and bytes after this serialization
                    // untouched, including in a nonzero-filled destination.
                    cursor += key_span;
                    output[cursor..cursor + 4].copy_from_slice(&value.to_le_bytes());
                    cursor += 4;
                }
                Ok(NativeReturn::Continue)
            }
            892 | 901 => {
                let handle = argument(args, 0)?;
                if handle == 0 {
                    return Ok(NativeReturn::Continue);
                }
                let address = argument(args, 1)?;
                if address == 0 {
                    return Ok(NativeReturn::Continue);
                }
                let dictionary = dictionaries
                    .get_mut(&handle)
                    .ok_or_else(|| error("native 901: invalid or wrong-type dictionary handle"))?;
                let input = self.raw_tail(address)?;
                let count = argument(input, 0)? as usize;
                if count > MAX_ITEMS {
                    return Err(error("native 901: serialized entry count exceeds limit"));
                }
                if count == 0 {
                    return Ok(NativeReturn::Continue);
                }
                let mut stored_length = Self::dictionary_size(dictionary)? as usize;
                let mut cursor = 4usize;
                for _ in 0..count {
                    let suffix = input
                        .get(cursor..)
                        .ok_or_else(|| error("native 901: serialized key starts outside block"))?;
                    let key_length =
                        suffix.iter().position(|&byte| byte == 0).ok_or_else(|| {
                            error("native 901: serialized key has no terminating zero")
                        })?;
                    let key = &suffix[..key_length];
                    let padded = key_span(key)?;
                    let value_offset = cursor
                        .checked_add(padded)
                        .ok_or_else(|| error("native 901: serialized entry offset overflow"))?;
                    let next = value_offset
                        .checked_add(4)
                        .ok_or_else(|| error("native 901: serialized entry offset overflow"))?;
                    let value = input
                        .get(value_offset..next)
                        .ok_or_else(|| error("native 901: serialized DWORD is truncated"))?;
                    let value = u32::from_le_bytes(value.try_into().unwrap());
                    if !dictionary.contains_key(key) {
                        if dictionary.len() >= MAX_ITEMS {
                            return Err(error("native 901: merged dictionary count exceeds limit"));
                        }
                        stored_length = stored_length
                            .checked_add(padded)
                            .and_then(|length| length.checked_add(4))
                            .filter(|&length| length <= MAX_BYTES)
                            .ok_or_else(|| error("native 901: merged dictionary exceeds 64 MiB"))?;
                        // Existing keys and repeated input keys both retain
                        // their first value. Completed entries survive a later
                        // malformed entry, in import order.
                        dictionary.insert(key.to_vec(), value);
                    }
                    cursor = next;
                }
                Ok(NativeReturn::Continue)
            }
            _ => Err(ScriptError::UnimplementedNativeCall { index }),
        }
    }

    /// Native 899/890 should use this same formula when the host wires this
    /// module in: count + aligned(key + NUL) + value, without an extra DWORD.
    pub(super) fn dictionary_size(dictionary: &BTreeMap<Vec<u8>, u32>) -> Result<u32> {
        if dictionary.len() > MAX_ITEMS {
            return Err(error("dictionary: entry count exceeds limit"));
        }
        let mut length = 4usize;
        for key in dictionary.keys() {
            length = length
                .checked_add(key_span(key)?)
                .and_then(|length| length.checked_add(4))
                .filter(|&length| length <= MAX_BYTES)
                .ok_or_else(|| error("dictionary: serialized data exceeds 64 MiB"))?;
        }
        Ok(length as u32)
    }

    /// A raw address may include a block-relative offset. A zero-length view
    /// at the exact block end is valid; alignment padding is not live storage.
    pub(super) fn raw_buffer(&self, address: u32, length: usize) -> Result<&[u8]> {
        let (&base, block) = self
            .raw
            .range(..=address)
            .next_back()
            .ok_or_else(|| error("raw buffer: address does not name a live allocation"))?;
        let offset = (address - base) as usize;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| error("raw buffer: range length overflow"))?;
        block
            .bytes
            .get(offset..end)
            .ok_or_else(|| error("raw buffer: range crosses its allocation boundary"))
    }

    pub(super) fn raw_buffer_mut(&mut self, address: u32, length: usize) -> Result<&mut [u8]> {
        let base = self
            .raw
            .range(..=address)
            .next_back()
            .map(|(&base, _)| base)
            .ok_or_else(|| error("raw buffer: address does not name a live allocation"))?;
        let offset = (address - base) as usize;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| error("raw buffer: range length overflow"))?;
        self.raw
            .get_mut(&base)
            .unwrap()
            .bytes
            .get_mut(offset..end)
            .ok_or_else(|| error("raw buffer: range crosses its allocation boundary"))
    }

    fn raw_tail(&self, address: u32) -> Result<&[u8]> {
        let (&base, block) = self
            .raw
            .range(..=address)
            .next_back()
            .ok_or_else(|| error("raw buffer: address does not name a live allocation"))?;
        block
            .bytes
            .get((address - base) as usize..)
            .ok_or_else(|| error("raw buffer: start is outside its allocation"))
    }

    fn allocate_raw(
        &mut self,
        bytes: Vec<u8>,
        counter: &mut u32,
        containers: &Containers,
        dictionaries: &Dictionaries,
    ) -> Result<u32> {
        let total = self
            .raw_bytes
            .checked_add(bytes.len())
            .filter(|&length| length <= MAX_RAW_TOTAL && bytes.len() <= MAX_BYTES)
            .ok_or_else(|| error("raw buffer: live storage exceeds its allocation limit"))?;
        // Include the zero-length tail address in the reservation. Otherwise
        // an adjacent object could be mistaken for the preceding block's end.
        let span = u32::try_from(bytes.len())
            .ok()
            .and_then(|length| length.checked_add(4))
            .map(|length| length & !3)
            .ok_or_else(|| error("raw buffer: reserved address range overflow"))?;
        let (base, next) = self.reserve(*counter, span, 4, containers, dictionaries)?;
        self.raw.insert(
            base,
            RawBlock {
                bytes,
                reserved_end: next,
            },
        );
        self.raw_bytes = total;
        *counter = next;
        Ok(base)
    }

    fn reserve(
        &self,
        counter: u32,
        span: u32,
        alignment: u32,
        containers: &Containers,
        dictionaries: &Dictionaries,
    ) -> Result<(u32, u32)> {
        let object_count = self
            .triples
            .len()
            .saturating_add(self.names.len())
            .saturating_add(self.raw.len())
            .saturating_add(containers.len())
            .saturating_add(dictionaries.len());
        if object_count >= MAX_ITEMS {
            return Err(error("data services: object count exceeds limit"));
        }
        let base = counter
            .max(1)
            .checked_add(alignment - 1)
            .map(|value| value & !(alignment - 1))
            .ok_or_else(|| error("data services: shared handle address space exhausted"))?;
        let end = base
            .checked_add(span)
            .ok_or_else(|| error("data services: shared handle address space exhausted"))?;
        let occupied = self.triples.range(base..end).next().is_some()
            || self.names.range(base..end).next().is_some()
            || containers.range(base..end).next().is_some()
            || dictionaries.range(base..end).next().is_some()
            || self
                .raw
                .range(..end)
                .next_back()
                .is_some_and(|(_, block)| block.reserved_end > base);
        if occupied {
            return Err(error(
                "data services: shared allocator would overlap a live object",
            ));
        }
        Ok((base, end))
    }
}

fn argument(bytes: &[u8], number: usize) -> Result<u32> {
    let offset = number
        .checked_mul(4)
        .ok_or_else(|| error("data services: argument offset overflow"))?;
    let end = offset
        .checked_add(4)
        .ok_or_else(|| error("data services: argument offset overflow"))?;
    let word = bytes
        .get(offset..end)
        .ok_or_else(|| error("data services: required argument DWORD is missing"))?;
    Ok(u32::from_le_bytes(word.try_into().unwrap()))
}

fn key_span(key: &[u8]) -> Result<usize> {
    if key.contains(&0) {
        return Err(error(
            "dictionary: key contains an embedded terminating zero",
        ));
    }
    key.len()
        .checked_add(4)
        .map(|length| length & !3)
        .filter(|&length| length <= MAX_BYTES)
        .ok_or_else(|| error("dictionary: aligned key length exceeds limit"))
}

fn error(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

fn terminated(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len())]
}

fn formatted(vm: &Vm, address: u32) -> Result<Vec<u8>> {
    let source = vm.read_string(address)?;
    let expanded = expand_kcs_format(vm, &source, 0)?;
    Ok(terminated(&expanded).to_vec())
}

fn write_string(vm: &mut Vm, address: u32, value: &[u8]) -> Result<()> {
    if value.contains(&0) {
        return Err(error("name table: embedded zero in a string field"));
    }
    let mut bytes = Vec::with_capacity(value.len() + 1);
    bytes.extend_from_slice(value);
    bytes.push(0);
    vm.write(address, &bytes)
}

fn resource_name(vm: &Vm, address: u32) -> Result<String> {
    let bytes = formatted(vm, address)?;
    // Resource paths retain CP932 independently of the selected script-text
    // encoding; Unicode mode applies only to a loaded table's primary names.
    Ok(String::from_utf8(cp932_utf8(&bytes)).expect("CP932 decoder emits UTF-8"))
}

fn load_resource(
    resources: &ResourceIndex,
    category: Option<&[u8]>,
    name: &str,
) -> Result<Option<Vec<u8>>> {
    let names = match category {
        Some(category) => resources
            .category_names(category, name)
            .map_err(|_| error("data services: resource category resolution failed"))?,
        None => vec![name.to_owned()],
    };
    // Delegate all directory/archive precedence to the index. Inspect the
    // candidate size before reading, and check the actual bytes again below
    // in case a loose file changed after its index was built.
    for name in names {
        if let Some(candidate) = resources.resolve_mode_utf16(
            &name.encode_utf16().collect::<Vec<_>>(), cs2_runtime::resource_router::ResourceMode::LooseFirst,
        ).map_err(|_| error("data services: resource routing failed"))? {
            let candidate = candidate.value;
            if candidate.size > MAX_BYTES as u64 {
                return Err(error("data services: indexed resource exceeds 64 MiB"));
            }
            return optional_resource(resources.read_candidate(candidate));
        }
    }
    Ok(None)
}

fn optional_resource(result: cs2_runtime::RuntimeResult<Vec<u8>>) -> Result<Option<Vec<u8>>> {
    match result {
        Ok(bytes) if bytes.len() <= MAX_BYTES => Ok(Some(bytes)),
        Ok(_) => Err(error("data services: resource exceeds the 64 MiB limit")),
        Err(RuntimeError::ResourceNotFound(_)) => Ok(None),
        Err(RuntimeError::Io { source, .. })
            if matches!(
                source.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            Ok(None)
        }
        Err(_) => Err(error("data services: resource service failed")),
    }
}

fn double_byte(bytes: &[u8]) -> bool {
    bytes.len() >= 2
        && matches!(bytes[0], 0x81..=0x9f | 0xe0..=0xef)
        && matches!(bytes[1], 0x40..=0x7e | 0x80..=0xfc)
}

fn folded_name(input: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(input.len());
    let mut cursor = 0;
    while cursor < input.len() {
        if double_byte(&input[cursor..]) {
            result.extend_from_slice(&input[cursor..cursor + 2]);
            cursor += 2;
        } else {
            result.push(input[cursor].to_ascii_uppercase());
            cursor += 1;
        }
    }
    result
}

fn integer_prefix(bytes: &[u8]) -> u32 {
    let mut cursor = 0;
    let negative = bytes.first() == Some(&b'-');
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        cursor += 1;
    }
    let hex =
        bytes.get(cursor) == Some(&b'0') && matches!(bytes.get(cursor + 1), Some(b'x' | b'X'));
    if hex {
        cursor += 2;
    }
    let start = cursor;
    let mut value = 0u32;
    while let Some(&byte) = bytes.get(cursor) {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' if hex => byte - b'a' + 10,
            b'A'..=b'F' if hex => byte - b'A' + 10,
            _ => break,
        };
        value = value
            .wrapping_mul(if hex { 16 } else { 10 })
            .wrapping_add(u32::from(digit));
        cursor += 1;
    }
    if cursor == start || (!hex && bytes.get(cursor) == Some(&b'.')) {
        0
    } else if negative {
        value.wrapping_neg()
    } else {
        value
    }
}

fn strip_comments(input: &[u8]) -> Vec<u8> {
    let mut clean = Vec::with_capacity(input.len());
    let mut cursor = 0;
    while cursor < input.len() {
        if input[cursor..].starts_with(b"//") {
            cursor += 2;
            while cursor < input.len() && !matches!(input[cursor], b'\r' | b'\n') {
                cursor += 1;
            }
        } else if input[cursor..].starts_with(b"/*") {
            cursor += 2;
            while cursor < input.len() && !input[cursor..].starts_with(b"*/") {
                cursor += 1;
            }
            cursor = cursor.saturating_add(2).min(input.len());
        } else {
            clean.push(input[cursor]);
            cursor += 1;
        }
    }
    clean
}

fn parse_triples(input: &[u8]) -> Result<TripleTable> {
    let clean = strip_comments(terminated(input));
    let mut table = TripleTable::default();
    let mut current = b"__UNKNOWN__".to_vec();
    let mut values = [100u32; 3];
    let mut rows = 0usize;
    let mut stored_bytes = 0usize;
    for source in clean.split(|&byte| matches!(byte, b'\r' | b'\n')) {
        let line: Vec<u8> = source
            .iter()
            .copied()
            .filter(|&byte| !matches!(byte, b' ' | b'\t'))
            .collect();
        if line.is_empty() {
            continue;
        }
        if let Some(hash) = line.iter().position(|&byte| byte == b'#') {
            if let Some(relative) = line[hash..].iter().position(|&byte| byte == b'(') {
                let opening = hash + relative;
                if opening - hash > 511 {
                    return Err(error("triple table: group name exceeds 511 bytes"));
                }
                current = folded_name(&line[hash..opening]);
                let mut cursor = opening + 1;
                for (component, delimiter) in [b',', b',', b')'].into_iter().enumerate() {
                    let Some(end) = line[cursor..]
                        .iter()
                        .position(|&byte| byte == delimiter)
                        .map(|relative| cursor + relative)
                    else {
                        break;
                    };
                    values[component] = integer_prefix(&line[cursor..end]);
                    cursor = end + 1;
                }
                continue;
            }
        }
        if line.len() > 511 {
            return Err(error("triple table: entry name exceeds 511 bytes"));
        }
        rows += 1;
        if rows > MAX_ITEMS {
            return Err(error("triple table: entry count exceeds limit"));
        }
        // Register the group before checking whether an entry already exists.
        let registered = match table.groups.get(&current) {
            Some(&first) => first,
            None => {
                stored_bytes = table_storage(stored_bytes, current.len() + 12)?;
                table.groups.insert(current.clone(), values);
                values
            }
        };
        let entry = folded_name(&line);
        if !table.entries.contains_key(&entry) {
            stored_bytes = table_storage(stored_bytes, entry.len() + 12)?;
            // Group values never change after first registration, so retaining
            // their three DWORDs is equivalent to retaining that group binding
            // without duplicating its potentially 511-byte name per entry.
            table.entries.insert(entry, registered);
        }
    }
    Ok(table)
}

fn table_storage(current: usize, added: usize) -> Result<usize> {
    current
        .checked_add(added)
        .filter(|&length| length <= MAX_BYTES)
        .ok_or_else(|| error("data table: stored payload exceeds 64 MiB"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldEnd {
    Tab,
    Row,
    File,
}

/// Tabs inside an opening-quote field are data, line breaks disappear, and a
/// closed quote discards all following bytes until a real column/row boundary.
fn name_field(input: &[u8], cursor: &mut usize) -> Result<(Vec<u8>, FieldEnd)> {
    let quoted = input.get(*cursor) == Some(&b'"');
    if quoted {
        *cursor += 1;
    }
    let mut closed = false;
    let mut value = Vec::new();
    while let Some(&byte) = input.get(*cursor) {
        if byte == 0 {
            return Err(error("name table: embedded zero in the resource text"));
        }
        if quoted && !closed {
            if byte == b'"' {
                *cursor += 1;
                if input.get(*cursor) == Some(&b'"') {
                    value.push(b'"');
                    *cursor += 1;
                } else {
                    closed = true;
                }
            } else if matches!(byte, b'\r' | b'\n') {
                *cursor += 1;
            } else {
                let width = if double_byte(&input[*cursor..]) { 2 } else { 1 };
                value.extend_from_slice(&input[*cursor..*cursor + width]);
                *cursor += width;
            }
        } else {
            match byte {
                b'\t' => {
                    *cursor += 1;
                    return Ok((value, FieldEnd::Tab));
                }
                b'\r' | b'\n' => {
                    *cursor += 1;
                    if byte == b'\r' && input.get(*cursor) == Some(&b'\n') {
                        *cursor += 1;
                    }
                    return Ok((value, FieldEnd::Row));
                }
                _ => {
                    let width = if double_byte(&input[*cursor..]) { 2 } else { 1 };
                    if !closed {
                        value.extend_from_slice(&input[*cursor..*cursor + width]);
                    }
                    *cursor += width;
                }
            }
        }
        if value.len() > MAX_FIELD {
            return Err(error("name table: field exceeds 1023 bytes"));
        }
    }
    Ok((value, FieldEnd::File))
}

fn parse_names(input: &[u8], unicode: bool, encoding: ScriptTextEncoding) -> Result<Vec<NameRow>> {
    let mut rows = Vec::new();
    let mut cursor = 0usize;
    let mut field_count = 0usize;
    let mut stored_bytes = 0usize;
    while cursor < input.len() {
        let mut columns = Vec::new();
        loop {
            let (value, end) = name_field(input, &mut cursor)?;
            field_count += 1;
            if field_count > MAX_ITEMS {
                return Err(error("name table: field count exceeds limit"));
            }
            columns.push(value);
            if end != FieldEnd::Tab {
                break;
            }
        }
        if columns.len() < 2
            || !columns[0].first().is_some_and(u8::is_ascii_digit)
            || columns[1].len() >= 256
        {
            continue;
        }
        let name = if unicode {
            match encoding {
                ScriptTextEncoding::Cp932 => cp932_utf8(&columns[1]),
                ScriptTextEncoding::Gbk => encoding_rs::GBK
                    .decode_without_bom_handling(&columns[1])
                    .0
                    .into_owned()
                    .into_bytes(),
                ScriptTextEncoding::Utf8 => std::str::from_utf8(&columns[1])
                    .map_err(|_| error("name table: primary name is invalid UTF-8"))?
                    .as_bytes()
                    .to_vec(),
            }
        } else {
            columns[1].clone()
        };
        let additional = [
            columns.get(2).cloned().unwrap_or_default(),
            columns.get(3).cloned().unwrap_or_default(),
        ];
        if name.len() >= 256 || additional.iter().any(|value| value.len() >= 256) {
            return Err(error(
                "name table: primary or additional text exceeds 255 bytes",
            ));
        }
        let mut aliases = Vec::new();
        aliases.push(name.clone());
        aliases.extend(columns.iter().skip(4).cloned());
        let bytes = name
            .len()
            .checked_add(additional.iter().map(Vec::len).sum::<usize>())
            .and_then(|length| length.checked_add(aliases.iter().map(Vec::len).sum::<usize>()))
            .ok_or_else(|| error("name table: storage size overflow"))?;
        stored_bytes = stored_bytes
            .checked_add(bytes)
            .filter(|&length| length <= MAX_BYTES)
            .ok_or_else(|| error("name table: storage exceeds the 64 MiB limit"))?;
        if rows.len() >= MAX_ITEMS {
            return Err(error("name table: row count exceeds limit"));
        }
        rows.push(NameRow {
            id: integer_prefix(&columns[0]),
            name,
            additional,
            aliases,
        });
    }
    Ok(rows)
}

fn cp932_utf8(input: &[u8]) -> Vec<u8> {
    let mut text = String::new();
    let mut cursor = 0;
    while cursor < input.len() {
        let first = input[cursor];
        match first {
            0x00..=0x80 => text.push(char::from(first)),
            0xa0 => text.push('\u{f8f0}'),
            0xa1..=0xdf => {
                text.push(char::from_u32(0xff61 + u32::from(first - 0xa1)).unwrap());
            }
            0xfd..=0xff => {
                text.push(char::from_u32(0xf8f1 + u32::from(first - 0xfd)).unwrap());
            }
            _ => {
                let end = (cursor + 2).min(input.len());
                match encoding_rs::SHIFT_JIS
                    .decode_without_bom_handling_and_without_replacement(&input[cursor..end])
                {
                    Some(decoded) => text.push_str(&decoded),
                    None => text.push('\u{30fb}'),
                }
                cursor = end;
                continue;
            }
        }
        cursor += 1;
    }
    text.into_bytes()
}
