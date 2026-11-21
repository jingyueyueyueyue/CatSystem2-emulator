//! Independent resource routing from docs/resources.md and fixed PE mount code.
//! Paths are UTF-16; only prefix comparison is ASCII insensitive. No path
//! normalization, global candidate sorting, resource decoding or I/O happens here.
use crate::{RuntimeError, RuntimeResult};
use encoding_rs::SHIFT_JIS;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;

const ISLAND_TEXT_ARCHIVES: [((&str, u64, &str), (&str, u64, &str)); 2] = [
    (
        ("scene.int", 2_187_392, "4b510173fee978d6af3baab0c3c78e26694e556790704193037dc34edeeda7de"),
        ("cnscene.int", 3_142_412, "6c3447a90a6a0bc706d2805cb3c58d8089478a24b51691a76d9f0fc468d0751a"),
    ),
    (
        ("fes.int", 171_679, "96152bcf12f60204c32238ce1eed08ccbe75a39befe9aad47ffe214ce0c12d05"),
        ("cnfes.int", 172_736, "6bc9f5d3033457555a885802fe820170b917b43df19598f4f7d2fe205ab5919d"),
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ResourceMode { File = 1, Archive = 2, LooseFirst = 3, ArchiveFirst = 4 }

impl TryFrom<u32> for ResourceMode {
    type Error = RuntimeError;
    fn try_from(value: u32) -> RuntimeResult<Self> {
        match value {
            1 => Ok(Self::File), 2 => Ok(Self::Archive), 3 => Ok(Self::LooseFirst),
            4 => Ok(Self::ArchiveFirst),
            _ => Err(RuntimeError::Host(format!("unknown resource mode {value}"))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveMount {
    /// The lookup path in archive mode, independently of the slot's registered prefix.
    pub path: Vec<u16>,
    /// Index in ResourceIndex::archives(), or a host-defined archive identity.
    pub archive: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceSlot {
    pub prefix: Vec<u16>,
    pub loose_directory: Vec<u16>,
    pub loose_enabled: bool,
    pub archive: Option<ArchiveMount>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceMember {
    pub slot: Option<u8>,
    pub loose_directory: Vec<u16>,
    pub loose_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceGroup {
    pub prefix: Vec<u16>,
    pub members: Vec<ResourceMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceRouter {
    pub slots: Vec<ResourceSlot>,
    pub groups: Vec<ResourceGroup>,
    pub patches: Vec<ArchiveMount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    island_chinese_scene: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    island_chinese_fes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    daydream_gbk_scene_archive: Option<usize>,
}

impl Default for ResourceRouter {
    fn default() -> Self {
        Self { slots: vec![ResourceSlot::default(); 256], groups: Vec::new(), patches: Vec::new(),
            island_chinese_scene: None, island_chinese_fes: None, daydream_gbk_scene_archive: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutedSource {
    File(Vec<u16>),
    Archive { slot: u8 },
    Patch { index: usize },
}

#[derive(Debug, Clone)]
pub struct RoutedResource<T> { pub source: RoutedSource, pub value: T }

pub(crate) const MAX_PATH_UNITS: usize = 259;
pub(crate) const MAX_ARCHIVE_KEY_BYTES: usize = 2 * MAX_PATH_UNITS + 2;

pub(crate) fn encode_wide_name<'a>(name: &str, scratch: &'a mut [u16; MAX_PATH_UNITS])
    -> RuntimeResult<&'a [u16]> {
    let mut len = 0;
    for unit in name.encode_utf16().take_while(|&unit| unit != 0) {
        *scratch.get_mut(len).ok_or_else(|| error("resource path exceeds 259 UTF-16 units"))? = unit;
        len += 1;
    }
    Ok(&scratch[..len])
}

enum RouteSource<'a> { File(&'a [u16]), Archive(u8), Patch(usize) }

impl ResourceRouter {
    pub fn is_island_chinese_scene(&self, index: &crate::ResourceIndex, path: &Path) -> bool {
        Self::matches_archive(self.island_chinese_scene, index, path)
    }

    pub fn is_island_chinese_fes(&self, index: &crate::ResourceIndex, path: &Path) -> bool {
        Self::matches_archive(self.island_chinese_fes, index, path)
    }

    pub fn is_daydream_gbk_scene_archive(&self, index: &crate::ResourceIndex, path: &Path) -> bool {
        Self::matches_archive(self.daydream_gbk_scene_archive, index, path)
    }

    fn matches_archive(archive: Option<usize>, index: &crate::ResourceIndex, path: &Path) -> bool {
        archive.and_then(|archive| index.archives().get(archive))
            .is_some_and(|archive| index.root().join(&archive.path) == path)
    }

    pub fn route<T>(
        &self, name: &[u8], mode: ResourceMode,
        file: impl FnMut(&[u16]) -> RuntimeResult<Option<T>>,
        archive: impl FnMut(usize, &[u16]) -> RuntimeResult<Option<T>>,
    ) -> RuntimeResult<Option<RoutedResource<T>>> {
        let raw = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
        let (name, invalid) = SHIFT_JIS.decode_without_bom_handling(raw);
        if invalid { return Err(error("invalid CP932 resource name")); }
        self.route_utf16(encode_wide_name(&name, &mut [0; MAX_PATH_UNITS])?, mode, file, archive)
    }

    /// Callbacks distinguish an absent/open-failed resource (None) from an
    /// actual read error (Err). A successful empty value stops the search.
    pub fn route_utf16<T>(
        &self, name: &[u16], mode: ResourceMode,
        file: impl FnMut(&[u16]) -> RuntimeResult<Option<T>>,
        archive: impl FnMut(usize, &[u16]) -> RuntimeResult<Option<T>>,
    ) -> RuntimeResult<Option<RoutedResource<T>>> {
        self.route_with(name, mode, file, archive, |source, value| RoutedResource {
            source: match source {
                RouteSource::File(path) => RoutedSource::File(path.to_vec()),
                RouteSource::Archive(slot) => RoutedSource::Archive { slot },
                RouteSource::Patch(index) => RoutedSource::Patch { index },
            },
            value,
        })
    }

    pub(crate) fn route_value_utf16<T>(
        &self, name: &[u16], mode: ResourceMode,
        file: impl FnMut(&[u16]) -> RuntimeResult<Option<T>>,
        archive: impl FnMut(usize, &[u16]) -> RuntimeResult<Option<T>>,
    ) -> RuntimeResult<Option<T>> {
        self.route_with(name, mode, file, archive, |_, value| value)
    }

    fn route_with<T, U>(
        &self, name: &[u16], mode: ResourceMode,
        mut file: impl FnMut(&[u16]) -> RuntimeResult<Option<T>>,
        mut archive: impl FnMut(usize, &[u16]) -> RuntimeResult<Option<T>>,
        mut output: impl FnMut(RouteSource<'_>, T) -> U,
    ) -> RuntimeResult<Option<U>> {
        if self.slots.len() != 256 { return Err(error("resource router requires 256 slots")); }
        let name = terminated(name);
        check_path(name)?;
        let (prefix, internal) = split_name(name);
        for (index, mount) in self.patches.iter().enumerate() {
            if let Some(value) = archive(mount.archive, internal)? {
                return Ok(Some(output(RouteSource::Patch(index), value)));
            }
        }
        if mode == ResourceMode::File {
            return Ok(file(name)?.map(|value| output(RouteSource::File(name), value)));
        }
        let mut scratch = [0; MAX_PATH_UNITS];
        if list_prefix(prefix) {
            for group in &self.groups {
                if !prefix_equal(prefix, &group.prefix)? { continue; }
                for member in &group.members {
                    let mount = member.slot.and_then(|slot| self.slots[usize::from(slot)].archive.as_ref());
                    if let Some(found) = attempt(mode, member.slot, mount, internal,
                        &member.loose_directory, member.loose_enabled, true, &mut scratch, &mut file, &mut archive, &mut output)? {
                        return Ok(Some(found));
                    }
                }
            }
            return Ok(None);
        }
        for (index, slot) in self.slots.iter().enumerate() {
            let force_loose = mode == ResourceMode::LooseFirst && prefix.is_empty();
            let matches = if force_loose { true } else if mode == ResourceMode::Archive {
                match &slot.archive { Some(mount) => prefix_equal(prefix, &mount.path)?, None => false }
            } else { prefix_equal(prefix, &slot.prefix)? };
            if !matches { continue; }
            if let Some(found) = attempt(mode, Some(index as u8), slot.archive.as_ref(), internal,
                &slot.loose_directory, slot.loose_enabled || force_loose,
                mode == ResourceMode::ArchiveFirst, &mut scratch, &mut file, &mut archive, &mut output)? {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }
}

fn attempt<T, U>(
    mode: ResourceMode, slot: Option<u8>, mount: Option<&ArchiveMount>, internal: &[u16],
    directory: &[u16], enabled: bool, keep_empty_separator: bool,
    scratch: &mut [u16; MAX_PATH_UNITS],
    file: &mut impl FnMut(&[u16]) -> RuntimeResult<Option<T>>,
    archive: &mut impl FnMut(usize, &[u16]) -> RuntimeResult<Option<T>>,
    output: &mut impl FnMut(RouteSource<'_>, T) -> U,
) -> RuntimeResult<Option<U>> {
    let read_archive = |archive: &mut dyn FnMut(usize, &[u16]) -> RuntimeResult<Option<T>>|
        -> RuntimeResult<Option<(u8, T)>> {
        match (slot, mount) {
            (Some(slot), Some(mount)) => Ok(archive(mount.archive, internal)?
                .map(|value| (slot, value))),
            _ => Ok(None),
        }
    };
    if mode != ResourceMode::LooseFirst {
        if let Some((slot, value)) = read_archive(archive)? { return Ok(Some(output(RouteSource::Archive(slot), value))); }
    }
    if mode != ResourceMode::Archive && enabled {
        let directory = terminated(directory);
        let internal_start = directory.len() + usize::from(!directory.is_empty() || keep_empty_separator);
        let len = internal_start + internal.len();
        let path = scratch.get_mut(..len).ok_or_else(|| error("resource path exceeds 259 UTF-16 units"))?;
        path[..directory.len()].copy_from_slice(directory);
        if internal_start != directory.len() { path[directory.len()] = u16::from(b'\\'); }
        path[internal_start..].copy_from_slice(internal);
        if let Some(value) = file(path)? {
            return Ok(Some(output(RouteSource::File(path), value)));
        }
    }
    if mode == ResourceMode::LooseFirst {
        return Ok(read_archive(archive)?.map(|(slot, value)| output(RouteSource::Archive(slot), value)));
    }
    Ok(None)
}

pub(crate) fn terminated(name: &[u16]) -> &[u16] {
    &name[..name.iter().position(|&unit| unit == 0).unwrap_or(name.len())]
}

pub(crate) fn split_name(name: &[u16]) -> (&[u16], &[u16]) {
    match name.iter().position(|&unit| unit == u16::from(b'/')) {
        Some(index) => (&name[..index], &name[index + 1..]), None => (&[], name),
    }
}

pub(crate) fn prefix_equal(a: &[u16], b: &[u16]) -> RuntimeResult<bool> {
    let (a, b) = (terminated(a), terminated(b));
    if a.iter().chain(b).any(|&unit| unit > 127) {
        return Err(error("non-ASCII resource prefix comparison is not recovered"));
    }
    Ok(a.len() == b.len() && a.iter().zip(b).all(|(&a, &b)| (a as u8).eq_ignore_ascii_case(&(b as u8))))
}

pub(crate) fn list_prefix(prefix: &[u16]) -> bool {
    let component = prefix.rsplit(|&unit| unit == 47 || unit == 92).next().unwrap_or_default();
    component.iter().rposition(|&unit| unit == 46).is_some_and(|dot| {
        component[dot + 1..].iter().copied().eq([108,115,116])
            || component[dot + 1..].iter().map(|&unit| {
                if unit <= 127 { (unit as u8).to_ascii_lowercase() as u16 } else { unit }
            }).eq([108,115,116])
    })
}

fn check_path(path: &[u16]) -> RuntimeResult<()> {
    if path.len() > MAX_PATH_UNITS { Err(error("resource path exceeds 259 UTF-16 units")) } else { Ok(()) }
}

#[cfg(test)]
mod resource_lookup_tests {
    use super::*;

    #[test]
    fn resource_router_keeps_patch_and_loose_order_with_scratch() {
        let mut router = ResourceRouter::default();
        router.patches.push(ArchiveMount { archive: 3, path: Vec::new() });
        router.slots[0] = ResourceSlot {
            loose_directory: "data".encode_utf16().collect(),
            ..ResourceSlot::default()
        };
        let mut scratch = [0; MAX_PATH_UNITS];
        let name = encode_wide_name("entry.bin\0ignored", &mut scratch).unwrap();
        let calls = std::cell::RefCell::new(Vec::new());
        let file = |path: &[u16]| {
            calls.borrow_mut().push(1);
            assert_eq!(String::from_utf16(path).unwrap(), "data\\entry.bin");
            Ok(Some(7))
        };
        let archive = |mount, internal: &[u16]| {
            calls.borrow_mut().push(0);
            assert_eq!(mount, 3);
            assert_eq!(internal, name);
            Ok(None)
        };
        assert_eq!(router.route_value_utf16(name, ResourceMode::LooseFirst, file, archive).unwrap(), Some(7));
        assert_eq!(*calls.borrow(), [0, 1]);
        let routed = router.route_utf16(name, ResourceMode::LooseFirst, file, archive).unwrap().unwrap();
        assert_eq!(routed.source, RoutedSource::File("data\\entry.bin".encode_utf16().collect()));
    }

    #[test]
    fn resource_router_preserves_cp932_wide_case_rules() {
        let mut scratch = [0; MAX_ARCHIVE_KEY_BYTES];
        let name: Vec<_> = "\u{3042}aBc\0ignored".encode_utf16().collect();
        assert_eq!(wide_query_key(&name, &mut scratch).unwrap(), b"\x82\xa0aBC");
        assert_eq!(wide_query_key(&[u16::from(b'a'); MAX_PATH_UNITS], &mut scratch).unwrap(), [b'A'; MAX_PATH_UNITS]);
        assert!(wide_query_key(&[0xd800], &mut scratch).is_err());
        assert!(wide_query_key(&[0x2603], &mut scratch).is_err());
    }
}

pub(crate) fn error(message: &str) -> RuntimeError { RuntimeError::Host(message.to_owned()) }

pub(crate) fn has_category_directory(source: &str) -> RuntimeResult<bool> {
    let (bytes, _, invalid) = SHIFT_JIS.encode(source);
    if invalid { return Err(error("category input is not representable in CP932")); }
    let mut offset = 0;
    let mut separator = None;
    while offset < bytes.len() {
        if matches!(bytes[offset], 0x81..=0x9f | 0xe0..=0xef)
            && bytes.get(offset + 1).is_some_and(|&b| matches!(b, 0x40..=0x7e | 0x80..=0xfc)) {
            offset += 2;
        } else {
            if matches!(bytes[offset], b'/' | b'\\') { separator = Some(offset); }
            offset += 1;
        }
    }
    Ok(separator.is_some_and(|offset| offset > 0))
}

pub(crate) fn complete_category(
    config: &cs2_formats::ConfigXml, slot: u32, source: &str,
) -> RuntimeResult<String> {
    if slot >= 128 { return Err(error("INT category slot is outside 0..127")); }
    let source = source.split('\0').next().unwrap_or_default();
    if has_category_directory(source)? { return Ok(source.to_owned()); }
    let key = format!("int{slot:02}");
    let field = |leaf: &[u8], capacity: usize| -> RuntimeResult<String> {
        let bytes = config.text(&[b"document", b"INT", key.as_bytes(), leaf]).unwrap_or_default();
        let bytes = &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())];
        if bytes.len() >= capacity { return Err(error("INT field exceeds original capacity")); }
        String::from_utf8(bytes.to_vec()).map_err(|_| error("runtime INT field is not UTF-8"))
    };
    let prefix = field(b"int", 32)?;
    let extension = field(b"ext", 8)?;
    let (bytes, _, _) = SHIFT_JIS.encode(source);
    // The subsequent extension test scans raw bytes backwards. In
    // particular a DBCS trail 0x5c is a boundary here, unlike the first scan.
    let has_extension = bytes.rsplit(|&b| b == b'/' || b == b'\\').next()
        .unwrap_or_default().contains(&b'.');
    let filename = if has_extension { source.to_owned() } else { format!("{source}.{extension}") };
    let name = if prefix.is_empty() { filename } else { format!("{prefix}/{filename}") };
    check_path(&name.encode_utf16().collect::<Vec<_>>())?;
    Ok(name)
}

/// Directory indexing folds CP932 ASCII while skipping double-byte pairs.
pub(crate) fn directory_key(raw: &[u8]) -> Vec<u8> {
    let mut key = raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())].to_vec();
    let mut at = 0;
    while at < key.len() {
        if matches!(key[at], 0x81..=0x9f | 0xe0..=0xfc) && key.get(at + 1)
            .is_some_and(|&trail| matches!(trail, 0x40..=0x7e | 0x80..=0xfc)) { at += 2; }
        else { key[at] = key[at].to_ascii_uppercase(); at += 1; }
    }
    key
}

/// The original wide query skips TWO units when a unit has a nonzero high
/// byte. Retain the consequent skipped ASCII character; no slash folding.
pub(crate) fn wide_query_key<'a>(name: &[u16], scratch: &'a mut [u8; MAX_ARCHIVE_KEY_BYTES])
    -> RuntimeResult<&'a [u8]> {
    let name = terminated(name);
    check_path(name)?;
    if name.iter().all(|&unit| unit < 128) {
        for (byte, &unit) in scratch.iter_mut().zip(name) {
            *byte = (unit as u8).to_ascii_uppercase();
        }
        return Ok(&scratch[..name.len()]);
    }
    if char::decode_utf16(name.iter().copied()).any(|character| character.is_err()) {
        return Err(error("invalid UTF-16 archive name"));
    }
    let mut units = [0; MAX_PATH_UNITS];
    let units = &mut units[..name.len()];
    units.copy_from_slice(name);
    let mut at = 0;
    while at < units.len() {
        if units[at] & 0xff00 != 0 { at += 2; }
        else { if (97..=122).contains(&units[at]) { units[at] -= 32; } at += 1; }
    }
    let (status, _, written) = SHIFT_JIS.new_encoder().encode_from_utf16_without_replacement(units, scratch, true);
    match status {
        encoding_rs::EncoderResult::InputEmpty => Ok(&scratch[..written]),
        encoding_rs::EncoderResult::Unmappable(_) => Err(error("archive name is not representable in CP932")),
        encoding_rs::EncoderResult::OutputFull => Err(error("archive query exceeds CP932 key capacity")),
    }
}

impl ResourceRouter {
    /// Prepare mounts from actual inventory and INT config. Registration is
    /// explicit and ordered; no same-name archive is selected globally.
    pub(crate) fn from_index(
        index: &crate::ResourceIndex, config: Option<&cs2_formats::ConfigXml>,
        base: &std::path::Path,
    ) -> RuntimeResult<Self> {
        let mut builder = MountBuilder { index, base, router: Self::default(), occupied: [false; 256], active_lists: Vec::new() };
        if let Some(config) = config {
            for slot in 0..128 {
                let key = format!("int{slot:02}");
                let field = |leaf: &[u8], limit: usize| -> RuntimeResult<String> {
                    let bytes = config.text(&[b"document", b"INT", key.as_bytes(), leaf]).unwrap_or_default();
                    let bytes = &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())];
                    if bytes.len() >= limit { return Err(error("startup INT field exceeds original fixed capacity")); }
                    String::from_utf8(bytes.to_vec()).map_err(|_| error("startup INT field is not UTF-8"))
                };
                // All fixed fields are loaded before registration; validate
                // even unused/default entries without changing their order.
                let _ = field(b"name", 32)?;
                let _ = field(b"ext", 8)?;
                let directory = field(b"dir", 260)?;
                // Startup 48d421..48d42b skips a zero archive-name field.
                if config.text(&[b"document", b"INT", key.as_bytes(), b"int"]).is_none() { continue; }
                let name = field(b"int", 32)?;
                if name.is_empty() { continue; }
                let loaded = builder.register(&name, &directory)?;
                if loaded.is_none() && (0..64).any(|need| {
                    config.integer(&[b"document", b"INT", format!("need{need:02}").as_bytes()], -1) == slot
                }) { return Err(error("required startup INT registration failed")); }
            }
            builder.bind_island_translation();
        } else {
            // For configuration-less tooling, expose actual mounts in stable
            // inventory order. This is an explicit host policy, not proof of
            // the original startup registration sequence.
            for archive in index.archives() {
                if patch_number(&archive.path).is_none() {
                    builder.register(&archive.path.to_string_lossy(), "")?;
                }
            }
        }
        let mut patches = index.archives().iter().enumerate().filter_map(|(archive, entry)| {
            let relative = entry.path.strip_prefix(base).ok()?;
            if relative.components().count() != 1 { return None; }
            patch_number(relative).map(|number| (number, archive, entry))
        }).collect::<Vec<_>>();
        patches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        builder.router.patches = patches.into_iter().filter(|(_, _, entry)| entry.is_readable())
            .map(|(_, archive, entry)| ArchiveMount { archive, path: entry.path.to_string_lossy().encode_utf16().collect() }).collect();
        // The CST members of this exact Daydream patch use GBK.
        builder.router.daydream_gbk_scene_archive = builder.verified_archive((
            "update03.int", 28_078_220,
            "cac79038baad0a266ce1e1ca75143285f2b09ad895e7b1af1af3a61df807c0f9",
        ));

        Ok(builder.router)
    }
}

struct MountBuilder<'a> {
    index: &'a crate::ResourceIndex,
    base: &'a std::path::Path,
    router: ResourceRouter,
    occupied: [bool; 256],
    active_lists: Vec<String>,
}

impl MountBuilder<'_> {
    fn bind_island_translation(&mut self) {
        for (original, translated) in ISLAND_TEXT_ARCHIVES {
            let Some(chinese) = self.verified_archive(translated) else { continue; };
            let Some(japanese) = self.verified_archive(original) else { continue; };
            let mut mounted = false;
            for slot in &mut self.router.slots {
                if let Some(mount) = &mut slot.archive && mount.archive == japanese {
                    mount.archive = chinese;
                    mounted = true;
                }
            }
            if mounted {
                if translated.0 == "cnscene.int" {
                    self.router.island_chinese_scene = Some(chinese);
                } else {
                    self.router.island_chinese_fes = Some(chinese);
                }
            }
        }
    }

    fn verified_archive(&self, (name, size, sha256): (&str, u64, &str)) -> Option<usize> {
        let path = self.base.join(name);
        let (archive, entry) = self.index.archives().iter().enumerate()
            .find(|(_, entry)| physical_path_equal(&entry.path, &path) && entry.is_readable())?;
        if !self.index.files().iter().any(|file| file.relative_path == entry.path && file.size == size) {
            return None;
        }
        let file = std::fs::File::open(self.index.root().join(&entry.path)).ok()?;
        let mut bytes = Vec::new();
        file.take(size + 1).read_to_end(&mut bytes).ok()?;
        if bytes.len() as u64 != size {
            return None;
        }
        let mut digest = crate::sha256::Sha256::default();
        digest.update(&bytes);
        (digest.finish() == sha256).then_some(archive)
    }

    fn register(&mut self, prefix: &str, directory: &str) -> RuntimeResult<Option<u8>> {
        // PE 52c7f1..52c83b unloads an existing successful mount with the
        // same actual loaded name before taking the first free pointer slot.
        for (slot, state) in self.router.slots.iter_mut().enumerate() {
            if state.archive.as_ref().is_some_and(|mount| String::from_utf16_lossy(&mount.path).eq_ignore_ascii_case(prefix)) {
                state.archive = None; self.occupied[slot] = false;
            }
        }
        let Some(slot) = self.occupied.iter().position(|&occupied| !occupied) else {
            return Err(error("archive registrations exceed 256 slots"));
        };
        self.occupied[slot] = true; // -1 failed/list slots remain occupied.
        let directory_wide = directory.encode_utf16().collect::<Vec<_>>();
        self.router.slots[slot] = ResourceSlot {
            prefix: prefix.encode_utf16().collect(), loose_directory: directory_wide.clone(),
            loose_enabled: directory.is_empty() || self.directory_exists(directory), archive: None,
        };
        if list_prefix(&prefix.encode_utf16().collect::<Vec<_>>()) {
            if self.active_lists.iter().any(|name| name.eq_ignore_ascii_case(prefix)) {
                return Err(error("recursive archive list registration"));
            }
            let Some(path) = self.index_path(prefix, false) else { return Ok(None); };
            let bytes = std::fs::read(self.index.root().join(path)).map_err(|source| crate::RuntimeError::Io {
                path: self.index.root().join(path), source,
            })?;
            if bytes.len() > 64 * 1024 * 1024 { return Err(error("archive list exceeds 64 MiB")); }
            let definitions = parse_list(&bytes)?;
            self.active_lists.push(prefix.to_owned());
            let group = self.router.groups.len();
            self.router.groups.push(ResourceGroup { prefix: prefix.encode_utf16().collect(), members: Vec::new() });
            for (directory, name) in definitions {
                let member_slot = self.register(&name, &directory)?;
                let enabled = self.directory_exists(&directory);
                self.router.groups[group].members.push(ResourceMember {
                    slot: member_slot, loose_directory: directory.encode_utf16().collect(), loose_enabled: enabled,
                });
            }
            self.active_lists.pop();
        } else if let Some(path) = self.index_path(prefix, true).map(std::path::Path::to_path_buf) {
            if let Some((archive, entry)) = self.index.archives().iter().enumerate().find(|(_, entry)| entry.path == path) {
                if entry.is_readable() {
                    // First attempt uses the registered name; fallback uses
                    // the actual base-qualified load path (mode2 compares it).
                    let actual = if path == std::path::PathBuf::from(prefix.replace('\\', "/")) {
                        prefix.to_owned()
                    } else { path.to_string_lossy().into_owned() };
                    self.router.slots[slot].archive = Some(ArchiveMount { archive, path: actual.encode_utf16().collect() });
                }
            }
            if self.router.slots[slot].archive.is_none() { return Ok(None); }
        } else {
            return Ok(None);
        }
        Ok(Some(slot as u8))
    }

    fn directory_exists(&self, directory: &str) -> bool {
        self.index.root().join(self.base).join(directory.replace('\\', "/")).is_dir()
    }

    fn index_path(&self, name: &str, archive: bool) -> Option<&std::path::Path> {
        let relative = std::path::PathBuf::from(name.replace('\\', "/"));
        for path in [&relative, &self.base.join(&relative)] {
            if archive {
                if let Some(entry) = self.index.archives().iter().find(|entry| physical_path_equal(&entry.path, path)) {
                    return Some(&entry.path);
                }
            } else if let Some(entry) = self.index.files().iter().find(|entry| physical_path_equal(&entry.relative_path, path)) {
                return Some(&entry.relative_path);
            }
        }
        None
    }
}

fn physical_path_equal(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.to_string_lossy().replace('\\', "/").eq_ignore_ascii_case(&b.to_string_lossy().replace('\\', "/"))
}

/// PE 52ca83..52cb2f/600800: no trimming/comments, LF or CRLF only,
/// split first forward slash; bytes are widened individually with CBW.
fn parse_list(data: &[u8]) -> RuntimeResult<Vec<(String, String)>> {
    let data = &data[..data.iter().position(|&b| b == 0).unwrap_or(data.len())];
    let mut members = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let newline = data[offset..].iter().position(|&b| b == b'\n').map(|at| offset + at);
        let end = newline.unwrap_or(data.len());
        let line = &data[offset..end];
        let line = if newline.is_some() { line.strip_suffix(b"\r").unwrap_or(line) } else { line };
        offset = newline.map_or(data.len(), |at| at + 1);
        if line.iter().any(|&b| b >= 128) { return Err(error("non-ASCII list CBW names need a version-specific host path")); }
        let Some(slash) = line.iter().position(|&b| b == b'/') else {
            return Err(error("archive list line has no directory/archive separator"));
        };
        let (directory, archive) = (&line[..slash], &line[slash + 1..]);
        if directory.len() >= 64 || archive.len() >= 32 {
            return Err(error("archive list member exceeds fixed directory/archive fields"));
        }
        members.push((String::from_utf8(directory.to_vec()).unwrap(), String::from_utf8(archive.to_vec()).unwrap()));
        if members.len() > 256 { return Err(error("archive list exceeds available registration slots")); }
    }
    Ok(members)
}

fn patch_number(path: &std::path::Path) -> Option<u8> {
    let name = path.file_name()?.to_str()?.as_bytes();
    if name.len() != 12 || !name[..6].eq_ignore_ascii_case(b"update") || !name[8..].eq_ignore_ascii_case(b".int")
        || !name[6..8].iter().all(u8::is_ascii_digit) { return None; }
    Some((name[6] - b'0') * 10 + name[7] - b'0')
}

#[cfg(test)]
mod island_translation_tests {
    use super::*;

    #[test]
    fn routes_local_island_text_archives_through_original_names() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../games/ISLAND");
        if !root.join("cnscene.int").is_file() || !root.join("cnfes.int").is_file() {
            return;
        }
        let index = crate::ResourceIndex::scan(root).unwrap();
        let router = index.resource_router().unwrap();
        let restored: ResourceRouter = serde_json::from_slice(&serde_json::to_vec(router).unwrap()).unwrap();
        assert_eq!(&restored, router);
        for (category, name, pack, sha256) in [
            (b"scene".as_slice(), "BAD_02", "cnscene.int",
                "eac9dbe3277572bca3505ecd36496b2f4fec852a63fa3cb11b1154d56e2b4f11"),
            (b"scene".as_slice(), "com01_selC", "cnscene.int",
                "de3a10ca5b7ed4d4059f66ad2440f6c49c37a7b8a351438012328fac1c479773"),
            (b"fes".as_slice(), "title", "cnfes.int",
                "6417ab8bf1bc70729efb1281ff99644b49bf6db2999ee511190559a3de5246de"),
        ] {
            let names = index.category_names(category, name).unwrap();
            let ordinary_candidate = index.find(&names[0]).unwrap();
            let wide = names[0].encode_utf16().collect::<Vec<_>>();
            for mode in [ResourceMode::LooseFirst, ResourceMode::Archive, ResourceMode::ArchiveFirst] {
                let candidate = index.resolve_mode_utf16(&wide, mode).unwrap().unwrap().value;
                assert_eq!(candidate, ordinary_candidate);
                let crate::ResourceLocation::Archive { path, .. } = &candidate.location else {
                    panic!("expected archive entry for {name}");
                };
                assert_eq!(*path, index.root().join(pack));
                assert_eq!(router.is_island_chinese_scene(&index, path), pack == "cnscene.int");
                assert_eq!(router.is_island_chinese_fes(&index, path), pack == "cnfes.int");
                let mut digest = crate::sha256::Sha256::default();
                digest.update(&index.read_candidate(candidate).unwrap());
                assert_eq!(digest.finish(), sha256);
            }
        }
        assert!(!router.is_island_chinese_scene(&index, &index.root().join("scene.int")));
    }
}
