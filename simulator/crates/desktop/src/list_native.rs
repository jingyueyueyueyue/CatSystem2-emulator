//! A batch of ordinary wrappers for the same type-15 image-list receiver.
//! Scalar ABI/return rules are centralized; entries retain their real image
//! resources and contribute pixels to the same output as FES-owned layers.
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::BTreeMap;
use std::rc::Rc;

use anyhow::{Context, Result, bail};
use cs2_renderer::{Canvas, ImageDrawOptions, Point, Rect, RgbaFrame};
use cs2_script::{ImageAnimation, NativeReturn, Vm};

use super::fes_task_platform::Layer;
use super::{LoggingHost, native_state_error, read_u32};

/// Complete wrappers in this batch. Command IDs belong to the object protocol,
/// separately from ordinary-native numeric IDs.
#[derive(Clone, Copy)]
struct Contract {
    native: u32,
    command: u32,
    bytes: usize,
    returns_priority: bool,
}

const CONTRACTS: &[Contract] = &[
    Contract {
        native: 256,
        command: 285,
        bytes: 12,
        returns_priority: false,
    },
    Contract {
        native: 260,
        command: 289,
        bytes: 12,
        returns_priority: false,
    },
    Contract {
        native: 261,
        command: 291,
        bytes: 4,
        returns_priority: false,
    },
    Contract {
        native: 262,
        command: 292,
        bytes: 8,
        returns_priority: false,
    },
    Contract {
        native: 340,
        command: 296,
        bytes: 4,
        returns_priority: false,
    },
    Contract {
        native: 341,
        command: 297,
        bytes: 4,
        returns_priority: false,
    },
    Contract {
        native: 342,
        command: 298,
        bytes: 4,
        returns_priority: false,
    },
    Contract {
        native: 534,
        command: 310,
        bytes: 8,
        returns_priority: false,
    },
    Contract {
        native: 657,
        command: 313,
        bytes: 8,
        returns_priority: false,
    },
    Contract {
        native: 879,
        command: 317,
        bytes: 12,
        returns_priority: false,
    },
    Contract {
        native: 881,
        command: 319,
        bytes: 12,
        returns_priority: false,
    },
    Contract {
        native: 257,
        command: 286,
        bytes: 52,
        returns_priority: false,
    },
    Contract {
        native: 258,
        command: 287,
        bytes: 8,
        returns_priority: false,
    },
    Contract {
        native: 259,
        command: 288,
        bytes: 52,
        returns_priority: false,
    },
    Contract {
        native: 622,
        command: 286,
        bytes: 52,
        returns_priority: false,
    },
    Contract {
        native: 623,
        command: 288,
        bytes: 52,
        returns_priority: false,
    },
    Contract {
        native: 626,
        command: 288,
        bytes: 48,
        returns_priority: false,
    },
    Contract {
        native: 627,
        command: 288,
        bytes: 48,
        returns_priority: false,
    },
    Contract {
        native: 707,
        command: 286,
        bytes: 52,
        returns_priority: false,
    },
    Contract {
        native: 708,
        command: 288,
        bytes: 52,
        returns_priority: false,
    },
    Contract {
        native: 263,
        command: 293,
        bytes: 12,
        returns_priority: false,
    },
    Contract {
        native: 339,
        command: 295,
        bytes: 24,
        returns_priority: false,
    },
    Contract {
        native: 455,
        command: 302,
        bytes: 8,
        returns_priority: false,
    },
    Contract {
        native: 462,
        command: 304,
        bytes: 8,
        returns_priority: true,
    },
    Contract {
        native: 463,
        command: 305,
        bytes: 12,
        returns_priority: false,
    },
    Contract {
        native: 464,
        command: 306,
        bytes: 8,
        returns_priority: false,
    },
    Contract {
        native: 625,
        command: 301,
        bytes: 12,
        returns_priority: false,
    },
    Contract {
        native: 663,
        command: 314,
        bytes: 12,
        returns_priority: false,
    },
    Contract {
        native: 732,
        command: 288,
        bytes: 52,
        returns_priority: false,
    },
    Contract {
        native: 862,
        command: 316,
        bytes: 20,
        returns_priority: false,
    },
    Contract {
        native: 880,
        command: 318,
        bytes: 16,
        returns_priority: false,
    },
];

const LEGACY_EMPTY_LIST_SOURCE: &[u8; 4] = b"LEMP";

fn is_legacy_list_pe(sha: Option<&str>) -> bool {
    matches!(sha, Some(super::css_model::UTATE_SHA | super::particle_external::CAMPANELLA_SHA))
}

fn is_short_registration_pe(sha: Option<&str>) -> bool {
    is_legacy_list_pe(sha) || sha == Some(super::css_model::SHA)
}

fn uses_short_registration(sha: Option<&str>, index: u32, bytes: usize) -> bool {
    matches!(index, 257 | 259) && bytes == 32 && is_short_registration_pe(sha)
}

#[derive(Default)]
pub(super) struct ListNativeStore {
    entries: EntryBindings,
    objects: BTreeMap<u32, SharedEntry>,
    object_owners: BTreeMap<u32, u32>,
    frame_cache: ListFrameCache,
    surfaces: BTreeMap<u32, tiles::ListSurface>,
    invalidated: std::collections::BTreeSet<(u32, u32)>,
    backgrounds: BTreeMap<u32, Background>,
    animation_updates: BTreeMap<u32, AnimationUpdate>,
    sound_bindings: BTreeMap<(u32, u32), (u32, u32)>,
    task_offsets: BTreeMap<u32, [i32; 2]>,
    switches: BTreeMap<u32, [bool; 3]>,
}

type SharedEntry = Rc<RefCell<Entry>>;

const MAX_LIST_FRAME_CACHE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Default)]
struct ListFrameCache(Rc<Cell<usize>>);

impl ListFrameCache {
    fn reserve(&self, bytes: usize) -> Result<ListFrameCharge> {
        let live = self.0.get();
        let total = live.checked_add(bytes)
            .filter(|&total| total <= MAX_LIST_FRAME_CACHE_BYTES)
            .with_context(|| format!(
                "retained list frame cache exceeds 256 MiB ({live} bytes live, {bytes} requested)"
            ))?;
        self.0.set(total);
        Ok(ListFrameCharge { total: Rc::clone(&self.0), bytes })
    }
}

struct ListFrameCharge {
    total: Rc<Cell<usize>>,
    bytes: usize,
}

impl Drop for ListFrameCharge {
    fn drop(&mut self) {
        self.total.set(self.total.get() - self.bytes);
    }
}

#[path = "list_snapshot.rs"]
mod snapshots;
pub(crate) use snapshots::PreparedImageLists;

#[path = "list_tiles.rs"]
mod tiles;
pub(super) use tiles::compose;

#[derive(Default)]
struct EntryBindings(BTreeMap<(u32, u32), SharedEntry>);

impl EntryBindings {
    fn get(&self, key: &(u32, u32)) -> Option<Ref<'_, Entry>> {
        self.0.get(key).map(|entry| entry.borrow())
    }
    fn get_mut(&self, key: &(u32, u32)) -> Option<RefMut<'_, Entry>> {
        self.0.get(key).map(|entry| entry.borrow_mut())
    }
    fn contains_key(&self, key: &(u32, u32)) -> bool {
        self.0.contains_key(key)
    }
    fn keys(&self) -> impl Iterator<Item = &(u32, u32)> {
        self.0.keys()
    }
    fn iter(&self) -> impl Iterator<Item = (&(u32, u32), Ref<'_, Entry>)> {
        self.0.iter().map(|(key, entry)| (key, entry.borrow()))
    }
    fn values(&self) -> impl Iterator<Item = Ref<'_, Entry>> {
        self.0.values().map(|entry| entry.borrow())
    }
    fn remove(&mut self, key: &(u32, u32)) -> Option<SharedEntry> {
        self.0.remove(key)
    }
    fn insert(&mut self, key: (u32, u32), entry: Entry) -> Option<SharedEntry> {
        self.0.insert(key, Rc::new(RefCell::new(entry)))
    }
}

impl ListNativeStore {
    pub(super) fn initial_switches(&self, id: u32) -> [bool; 3] {
        self.switches
            .get(&id)
            .copied()
            .unwrap_or([true, true, false])
    }
}

struct AnimationUpdate {
    command: Option<cs2_runtime::TaskCommand>,
    now: u32,
    keys: Vec<u32>,
    cursor: usize,
    compose: bool,
    signal_ready: bool,
    entry_ready: bool,
    resource_changes: Vec<u32>,
}

impl LoggingHost<'_> {
    pub(super) fn is_list_output_wrapper(&self, handle: u32) -> bool {
        let pixels = &self.data_services.image_wrapper_pixels;
        self.data_services.tasks.fes.platform.lists.values().any(|list| {
            list.output == handle || pixels.get(&handle)
                .is_some_and(|pixel| pixels.get(&list.output) == Some(pixel))
        })
    }

    pub(crate) fn synchronize_list_resource_pixels_only(&mut self, handle: u32, frame: &RgbaFrame) -> Result<()> {
        let mut wrappers = std::collections::BTreeSet::new();
        for entry in self.data_services.list_native.entries.0.values()
            .chain(self.data_services.list_native.objects.values()) {
            if let Source::Resource { handle: source, wrapper } = entry.borrow().source
                && source == handle { wrappers.insert(wrapper); }
        }
        for wrapper in wrappers { self.replace_image_wrapper_pixels(wrapper, frame.clone())?; }
        Ok(())
    }

    /// One canonical resource can have several independently held wrappers.
    /// Update all of them before consuming its shared changed flag.
    pub(crate) fn synchronize_list_resource(&mut self, handle: u32) -> Result<()> {
        let frame = self.image_resources.get(&handle)
            .context("list resource synchronization has no live resource")?.image.clone();
        self.data_services.image_manager.synchronize_single_static_source(handle, &frame);
        let bindings = self.data_services.list_native.entries.iter()
            .filter_map(|(&(list, key), entry)| matches!(entry.source,
                Source::Resource { handle: source, .. } if source == handle).then_some((list, key)))
            .collect::<Vec<_>>();
        for &(list, key) in &bindings { record_geometry(self, list, key, false)?; }
        self.synchronize_list_resource_pixels_only(handle, &frame)?;
        for (list, key) in bindings {
            record_geometry(self, list, key, false)?;
            invalidate_entry(self, list, key);
        }
        Ok(())
    }
}

struct Entry {
    key: u32,
    source: Source,
    position: [f32; 2],
    base: [f32; 2],
    rotation: f32,
    scale: [f32; 2],
    opacity: u8,
    raw_opacity: u32,
    visible: bool,
    transform: u32,
    priority: i32,
    clip: Option<Rect>,
    animation: ImageAnimation,
    frames: BTreeMap<u32, RgbaFrame>,
    frame_handles: BTreeMap<u32, u32>,
    frame_charges: BTreeMap<u32, ListFrameCharge>,
    selected_frame: Option<u32>,
    identity: u32,
}

impl Entry {
    fn empty(source: Source) -> Self {
        Self {
            key: 0,
            source,
            position: [0.; 2],
            base: [0.; 2],
            rotation: 0.,
            scale: [1.; 2],
            opacity: 255,
            raw_opacity: 255,
            visible: true,
            transform: 0,
            priority: 0,
            clip: None,
            animation: ImageAnimation::default(),
            frames: BTreeMap::new(),
            frame_handles: BTreeMap::new(),
            frame_charges: BTreeMap::new(),
            selected_frame: None,
            identity: 0,
        }
    }

    fn current_wrapper(&self) -> Option<u32> {
        self.selected_frame
            .and_then(|id| self.frame_handles.get(&id).copied())
            .or_else(|| match self.source {
                Source::Resource { wrapper, .. } | Source::Wrapper(wrapper) => Some(wrapper),
                Source::Frame | Source::Empty => None,
            })
    }
}

#[derive(Clone, Copy)]
enum Source {
    Resource {
        handle: u32,
        wrapper: u32,
    },
    /// An independent unowned wrapper keeps the shared pixel binding alive.
    Wrapper(u32),
    Frame,
    Empty,
}

struct Background {
    wrapper: u32,
    active: bool,
}

pub(super) fn has_animation(host: &LoggingHost<'_>, command: cs2_runtime::TaskCommand) -> bool {
    let key = list_key(command.sender as u32, command.data[0]);
    host.data_services
        .list_native
        .entries
        .get(&(command.target as u32, key))
        .is_some_and(|entry| entry.animation.timeline().is_some()
            || matches!(entry.source, Source::Resource { handle, .. }
                if host.data_services.image_manager.resource_has_animation(handle)))
}

pub(super) fn has_entry(host: &LoggingHost<'_>, command: cs2_runtime::TaskCommand) -> bool {
    host.data_services.list_native.entries.contains_key(&(
        command.target as u32,
        list_key(command.sender as u32, command.data[0]),
    ))
}

fn current_pixels(host: &LoggingHost<'_>, entry: &Entry) -> Result<u32> {
    if matches!(entry.source, Source::Empty) {
        return Ok(0);
    }
    let wrapper = entry.current_wrapper()
        .context("list source has no current pixel wrapper")?;
    host.data_services
        .image_wrapper_pixels
        .get(&wrapper)
        .copied()
        .context("list source pixel identity disappeared")
}

fn ensure_shared_entries_idle(store: &ListNativeStore, id: u32) -> Result<()> {
    for (&owner, pending) in &store.animation_updates {
        if owner == id {
            continue;
        }
        for key in &pending.keys {
            let Some(source) = store.entries.0.get(&(owner, *key)) else {
                continue;
            };
            if store
                .entries
                .0
                .iter()
                .any(|(&(target, _), entry)| target == id &&
                    (Rc::ptr_eq(source, entry) || shared_resource(source, entry)))
            {
                bail!("shared list entry has an unfinished ANM round in list {owner}");
            }
        }
    }
    Ok(())
}

fn ensure_source_idle(store: &ListNativeStore, source: &SharedEntry) -> Result<()> {
    for (&owner, pending) in &store.animation_updates {
        if pending.keys.iter().any(|key| {
            store
                .entries
                .0
                .get(&(owner, *key))
                .is_some_and(|entry| Rc::ptr_eq(source, entry) || shared_resource(source, entry))
        }) {
            bail!("entry-object source has an unfinished ANM round in list {owner}");
        }
    }
    Ok(())
}

fn shared_resource(left: &SharedEntry, right: &SharedEntry) -> bool {
    matches!((left.borrow().source, right.borrow().source),
        (Source::Resource { handle: a, .. }, Source::Resource { handle: b, .. }) if a == b)
}

fn invalidate_entry(host: &mut LoggingHost<'_>, list: u32, key: u32) {
    let shared = host
        .data_services
        .list_native
        .entries
        .0
        .get(&(list, key))
        .cloned();
    let aliases: Vec<_> = host
        .data_services
        .list_native
        .entries
        .0
        .iter()
        .filter_map(|(&stamp, entry)| {
            shared
                .as_ref()
                .is_some_and(|source| Rc::ptr_eq(source, entry))
                .then_some(stamp)
        })
        .collect();
    host.data_services
        .list_native
        .invalidated
        .insert((list, key));
    host.data_services.list_native.invalidated.extend(aliases);
}

fn record_geometry(
    host: &mut LoggingHost<'_>,
    list: u32,
    key: u32,
    source_base: bool,
) -> Result<()> {
    let aliases: Vec<_> = match host.data_services.list_native.entries.0.get(&(list, key)) {
        Some(source) => host
            .data_services
            .list_native
            .entries
            .0
            .iter()
            .filter_map(|(&stamp, entry)| Rc::ptr_eq(source, entry).then_some(stamp))
            .collect(),
        None => return Ok(()),
    };
    for (list, key) in aliases {
        if !host.data_services.list_native.surfaces.contains_key(&list) {
            continue;
        }
        let bounds = {
            let entry = host
                .data_services
                .list_native
                .entries
                .get(&(list, key))
                .unwrap();
            if matches!(entry.source, Source::Empty) {
                continue;
            }
            let wrapper = entry.current_wrapper()
                .context("list history source has no current wrapper")?;
            let frame = host
                .image_handles
                .get(&wrapper)
                .context("list history wrapper disappeared")?;
            let size = host.data_services.tasks.fes.platform.lists[&list].size;
            let geometry = source_geometry(host, &entry, frame)?;
            let base = if source_base {
                geometry.base.map(|value| value as f32)
            } else {
                entry.base
            };
            entry_bounds(geometry, &entry, base, size, entry.clip)?
        };
        host.data_services
            .list_native
            .surfaces
            .get_mut(&list)
            .unwrap()
            .registration(key, bounds);
    }
    Ok(())
}

fn registration_history(host: &mut LoggingHost<'_>, list: u32, key: u32) -> Result<()> {
    record_geometry(host, list, key, true)?;
    record_geometry(host, list, key, false)
}

/// Addressed type-15 protocol. Native wrappers and raw 69/70 enter the same
/// receiver with owned buffers; no DWORD here is treated as a host pointer.
pub(super) fn receive(
    host: &mut LoggingHost<'_>,
    command: cs2_runtime::TaskCommand,
    buffers: &mut super::OwnedTaskBuffers,
    output: &mut u32,
    budget: usize,
) -> cs2_script::Result<Option<cs2_runtime::TaskCommandOutcome>> {
    if command.code != 19 && !(285..=319).contains(&command.code) {
        return Ok(None);
    }
    let id = command.target as u32;
    if host.scene_nodes.get(&id).is_none_or(|node| node.kind != 15) {
        return Ok(None);
    }
    if host
        .data_services
        .tasks
        .message_task_state(id)
        .is_some_and(|(_, deleted, _)| deleted != 0)
    {
        return Ok(Some(cs2_runtime::TaskCommandOutcome {
            done: true,
            propagate: false,
        }));
    }
    if command.code == 309 {
        return Ok(Some(cs2_runtime::TaskCommandOutcome {
            done: true,
            propagate: true,
        }));
    }
    if let Some(result) = host.receive_list_animation(command, buffers, output, budget)? {
        return Ok(Some(result));
    }
    let result = receive_inner(host, command, buffers, output);
    result
        .map(|()| {
            Some(cs2_runtime::TaskCommandOutcome {
                done: true,
                propagate: false,
            })
        })
        .map_err(|error| {
            host.events.push(super::LogicEvent {
                frame: host.frame,
                kind: "image-list-error".into(),
                detail: error.to_string(),
            });
            native_state_error("type-15 receiver failed; see image-list-error")
        })
}

fn receive_inner(
    host: &mut LoggingHost<'_>,
    command: cs2_runtime::TaskCommand,
    buffers: &mut super::OwnedTaskBuffers,
    output: &mut u32,
) -> Result<()> {
    let id = command.target as u32;
    let code = command.code;
    let initialized = host
        .data_services
        .tasks
        .fes
        .platform
        .lists
        .contains_key(&id);
    if matches!(code, 19 | 300 | 311 | 312) {
        return Ok(());
    }
    if host
        .data_services
        .list_native
        .animation_updates
        .contains_key(&id)
    {
        bail!("finish the list animation round before mutating or querying its sources");
    }
    ensure_shared_entries_idle(&host.data_services.list_native, id)?;
    let key = list_key(command.sender as u32, command.data[0]);
    let stamp = (id, key);
    match code {
        285 => {
            host.initialize_fes_image_list(id, command.data[0], command.data[1])?;
            host.data_services.list_native.surfaces.remove(&id);
            restore_background(host, id)?;
        }
        289 => {
            host.data_services
                .list_native
                .task_offsets
                .insert(id, [command.data[0] as i32, command.data[1] as i32]);
            if let Some(list) = host.data_services.tasks.fes.platform.lists.get_mut(&id) {
                list.offset = [command.data[0] as i32, command.data[1] as i32];
            }
        }
        290 => {
            let offset = host
                .data_services
                .list_native
                .task_offsets
                .get(&id)
                .copied()
                .unwrap_or([0; 2]);
            buffers.write_data0(&(offset[0] as u32).to_le_bytes())?;
            buffers.write_data1(&(offset[1] as u32).to_le_bytes())?;
        }
        291 => {
            let list = host
                .data_services
                .tasks
                .fes
                .platform
                .lists
                .get(&id)
                .context("list output query requires a created list")?;
            buffers.write_data1(&list.output.to_le_bytes())?;
        }
        286 | 288 if initialized => register_owned(
            host,
            command,
            buffers
                .data1()
                .filter(|bytes| bytes.len() >= 60)
                .context("list registration requires fifteen DWORDs")?,
        )?,
        286 | 288 => {}
        287 => {
            invalidate_entry(host, id, key);
            if let Some(entry) = host.data_services.list_native.entries.remove(&stamp) {
                host.release_list_entry(entry)?;
            }
        }
        292 => {
            let value = command.data[1] != 0;
            let switches = host
                .data_services
                .list_native
                .switches
                .entry(id)
                .or_insert([true, true, false]);
            switches[1] = value;
            switches[2] = value;
            if value {
                switches[0] = true;
            }
            if let Some(list) = host.data_services.tasks.fes.platform.lists.get_mut(&id) {
                list.submit = value;
                list.force = value;
                if list.submit {
                    list.enabled = true;
                }
            }
        }
        293 => {
            if host
                .data_services
                .list_native
                .entries
                .get(&stamp)
                .is_some_and(|entry| entry.visible != (command.data[1] != 0))
            {
                record_geometry(host, id, key, false)?;
            }
            let changed =
                if let Some(mut entry) = host.data_services.list_native.entries.get_mut(&stamp) {
                    let changed = entry.visible != (command.data[1] != 0);
                    entry.visible = command.data[1] != 0;
                    changed
                } else {
                    false
                };
            if changed {
                invalidate_entry(host, id, key);
            }
        }
        294 => {
            if !initialized {
                bail!("list count query requires a created list");
            }
            *output = host
                .data_services
                .list_native
                .entries
                .keys()
                .filter(|(target, _)| *target == id)
                .count() as u32
                + host.data_services.tasks.fes.platform.lists[&id]
                    .layers
                    .keys()
                    .filter(|(owner, _)| *owner != u32::MAX)
                    .count() as u32;
        }
        295 => {
            if buffers.data1().is_some() {
                record_geometry(host, id, key, false)?;
            }
            let mut dirty = false;
            if let Some(mut entry) = host.data_services.list_native.entries.get_mut(&stamp) {
                let clip = match buffers.data1() {
                    Some(bytes) => rectangle([
                        read_u32(bytes, 0)? as i32,
                        read_u32(bytes, 4)? as i32,
                        read_u32(bytes, 8)? as i32,
                        read_u32(bytes, 12)? as i32,
                    ]),
                    None if command.data[1] == 0 => None,
                    None => bail!("list clip has a nonzero pointer without owned bytes"),
                };
                dirty = buffers.data1().is_some() && entry.clip != clip;
                entry.clip = clip;
            }
            if dirty {
                invalidate_entry(host, id, key);
            } else if buffers.data1().is_none() {
                if let Some(surface) = host.data_services.list_native.surfaces.get_mut(&id) {
                    surface.clear_clip(key);
                }
            }
        }
        296 | 298 | 316 => {
            host.data_services
                .list_native
                .switches
                .entry(id)
                .or_insert([true, true, false])[0] = true;
            if !initialized {
                return Ok(());
            }
            host.data_services
                .tasks
                .fes
                .platform
                .lists
                .get_mut(&id)
                .unwrap()
                .enabled = true;
            if code == 316
                && let Some(bytes) = buffers.data1()
            {
                if let Some(clip) = rectangle([
                    read_u32(bytes, 0)? as i32,
                    read_u32(bytes, 4)? as i32,
                    read_u32(bytes, 8)? as i32,
                    read_u32(bytes, 12)? as i32,
                ]) {
                    host.update_native_list_region(id, clip)?;
                }
            } else {
                super::fes_task_platform::compose_list(host, id, code == 298)?;
            }
        }
        297 => {
            clear_entries(host, id)?;
            if let Some(list) = host.data_services.tasks.fes.platform.lists.get_mut(&id) {
                list.layers.clear();
                let (handle, size) = (list.output, list.size);
                host.replace_image_wrapper_pixels(handle, blank(size)?)?;
            }
            restore_background(host, id)?;
            if let Some(surface) = host.data_services.list_native.surfaces.get_mut(&id) {
                surface.invalidate_all();
            }
        }
        302 => {
            let handle = command.data[1];
            invalidate_entry(host, id, 0);
            if let Some(entry) = host.data_services.list_native.entries.remove(&(id, 0)) {
                host.release_list_entry(entry)?;
            }
            if handle == 0 {
                if let Some(background) = host.data_services.list_native.backgrounds.get_mut(&id) {
                    background.active = false;
                }
            } else {
                let source = host
                    .image_handles
                    .get(&handle)
                    .context("list background requires an image wrapper")?;
                let image = if let Some(bytes) = buffers.data0() {
                    let [left, top, right, bottom] = [
                        read_u32(bytes, 0)? as i32,
                        read_u32(bytes, 4)? as i32,
                        read_u32(bytes, 8)? as i32,
                        read_u32(bytes, 12)? as i32,
                    ];
                    if left < 0 || top < 0 || right < left || bottom < top {
                        bail!("background crop has invalid bottom-origin endpoints");
                    }
                    let right = right.min(source.width() as i32);
                    let bottom = bottom.min(source.height() as i32);
                    if right <= left || bottom <= top {
                        if !host.data_services.list_native.backgrounds.contains_key(&id) {
                            bail!("background crop has no pixels and no previous snapshot");
                        }
                        restore_background(host, id)?;
                        return Ok(());
                    }
                    let cropped = source.crop(Rect::new(
                        left,
                        source.height() as i32 - bottom,
                        (right - left) as u32,
                        (bottom - top) as u32,
                    ))?;
                    let mut metadata = cropped.metadata();
                    metadata.offset = [0; 2];
                    metadata.canvas_size = [cropped.width(), cropped.height()];
                    cropped.with_raw_metadata(metadata)
                } else {
                    source.clone()
                };
                let wrapper = host.register_image_wrapper(image, false)?;
                let old = host.data_services.list_native.backgrounds.insert(
                    id,
                    Background {
                        wrapper,
                        active: true,
                    },
                );
                if let Some(old) = old {
                    host.release_image_wrapper(old.wrapper)?;
                }
                restore_background(host, id)?;
            }
            if let Some(child) = host
                .data_services
                .tasks
                .fes
                .platform
                .lists
                .get(&id)
                .and_then(|list| list.base_plane)
            {
                if let Some(node) = host.scene_nodes.get_mut(&child) {
                    host.data_services
                        .planes
                        .set_list_plane_visible(node, handle != 0);
                }
            }
        }
        303 => {
            if let Some(child) = host
                .data_services
                .tasks
                .fes
                .platform
                .lists
                .get(&id)
                .and_then(|list| list.base_plane)
            {
                let mut vector: Vec<_> = [0u32, 0, (command.data[1] as i32 as f32).to_bits()]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect();
                if !host.dispatch_plane_command(
                    cs2_runtime::TaskCommand {
                        target: child as i32,
                        sender: id as i32,
                        code: 49,
                        data: [0, 1, 1],
                    },
                    &mut super::plane_native::PlaneCommandBuffers {
                        data0: None,
                        data1: Some(&mut vector),
                    },
                    &mut 0,
                )? {
                    bail!("image-list base plane depth receiver is absent");
                }
            }
        }
        304 => {
            if let Some(entry) = host.data_services.list_native.entries.get(&stamp) {
                buffers.write_data1(&(entry.priority as u32).to_le_bytes())?;
                *output = 1;
            } else {
                *output = 0;
            }
        }
        305 => {
            let mut visible = false;
            if let Some(mut entry) = host.data_services.list_native.entries.get_mut(&stamp) {
                entry.priority = command.data[1] as i32;
                visible = entry.visible;
                *output = 1;
            } else {
                *output = 0;
            }
            if visible {
                invalidate_entry(host, id, key);
            }
        }
        307 | 308 | 315 => {
            let mut found = None;
            for key in signed_keys(&host.data_services.list_native, id) {
                let entry = host
                    .data_services
                    .list_native
                    .entries
                    .get(&(id, key))
                    .unwrap();
                if match code {
                    308 => entry.identity == command.data[0],
                    315 => {
                        (match entry.source {
                            Source::Resource { handle, .. } => handle,
                            _ => 0,
                        }) == command.data[0]
                    }
                    _ => !matches!(entry.source, Source::Empty)
                        && current_pixels(host, &entry)? == command.data[0],
                } {
                    found = Some(entry.key);
                    break;
                }
            }
            if let Some(key) = found {
                buffers.write_data1(&(key >> 16).to_le_bytes())?;
                *output = 1;
            } else {
                *output = 0;
            }
        }
        310 => {
            host.data_services
                .list_native
                .switches
                .entry(id)
                .or_insert([true, true, false])[0] = command.data[1] != 0;
            if let Some(list) = host.data_services.tasks.fes.platform.lists.get_mut(&id) {
                list.enabled = command.data[1] != 0;
            }
        }
        313 => {
            let handle = match host.data_services.list_native.entries.get(&stamp) {
                Some(entry) => current_pixels(host, &entry)?,
                None => 0,
            };
            buffers.write_data1(&handle.to_le_bytes())?;
        }
        314 if initialized => {
            let frame = blank([command.data[0], command.data[1]])?;
            let handle = host.register_image_wrapper(frame, false)?;
            let list = host
                .data_services
                .tasks
                .fes
                .platform
                .lists
                .get_mut(&id)
                .unwrap();
            let old = list.output;
            list.output = handle;
            list.size = [command.data[0], command.data[1]];
            if host.image_handles.contains_key(&old) {
                host.release_image_wrapper(old)?;
            }
            if host.data_services.list_native.surfaces.contains_key(&id) {
                host.ensure_canvas_state()?;
                let tile = host
                    .data_services
                    .canvas_native
                    .tile_size()
                    .unwrap_or([command.data[0].max(1), command.data[1].max(1)]);
                host.data_services
                    .list_native
                    .surfaces
                    .get_mut(&id)
                    .unwrap()
                    .resize([command.data[0], command.data[1]], tile)?;
            }
            let keys: Vec<_> = host
                .data_services
                .list_native
                .entries
                .keys()
                .filter(|(list, _)| *list == id)
                .copied()
                .collect();
            host.data_services.list_native.invalidated.extend(keys);
        }
        314 => {}
        317 => {
            if command.data[1] == u32::MAX {
                if let Some((channel, _)) =
                    host.data_services.list_native.sound_bindings.remove(&stamp)
                {
                    sound_signal(host, command.target, channel, Some(0))?;
                }
                if let Some(mut entry) = host
                    .data_services
                    .list_native
                    .entries
                    .get_mut(&stamp)
                    .filter(|entry| entry.animation.timeline().is_some())
                {
                    entry
                        .animation
                        .timeline_mut()
                        .unwrap()
                        .interpreter
                        .variables[0] = 0;
                }
            } else {
                sound_signal(host, command.target, command.data[1], Some(1))?;
                let threshold = host
                    .data_services
                    .list_native
                    .sound_bindings
                    .get(&stamp)
                    .map_or(1000, |value| value.1);
                host.data_services
                    .list_native
                    .sound_bindings
                    .insert(stamp, (command.data[1], threshold));
            }
        }
        319 => {
            if let Some(binding) = host
                .data_services
                .list_native
                .sound_bindings
                .get_mut(&stamp)
            {
                binding.1 = command.data[1];
            }
        }
        _ => bail!("type-15 command {code} has no documented receiver in this implementation"),
    }
    Ok(())
}

fn register_owned(
    host: &mut LoggingHost<'_>,
    command: cs2_runtime::TaskCommand,
    bytes: &[u8],
) -> Result<()> {
    let id = command.target as u32;
    let key = list_key(command.sender as u32, command.data[0]);
    let stamp = (id, key);
    let sources = [
        read_u32(bytes, 0)?,
        read_u32(bytes, 4)?,
        read_u32(bytes, 8)?,
        read_u32(bytes, 12)?,
    ];
    let identity = read_u32(bytes, 56)?;
    let position = [float(bytes, 16)?, float(bytes, 20)?];
    let base = [float(bytes, 24)?, float(bytes, 28)?];
    let rotation = float(bytes, 32)?;
    let scale = [float(bytes, 36)?, float(bytes, 40)?];
    let raw_opacity = read_u32(bytes, 44)?;
    let visible = read_u32(bytes, 48)? != 0;
    let transform = read_u32(bytes, 52)?;
    let no_sources = sources == [0; 4];
    let legacy_empty = no_sources
        && bytes.get(60..) == Some(LEGACY_EMPTY_LIST_SOURCE.as_slice())
        && is_short_registration_pe(host.css_task_sha());
    if no_sources && !legacy_empty {
        if command.code == 286 && identity != 0 {
            if let Some(mut entry) = host.data_services.list_native.entries.get_mut(&stamp) {
                entry.identity = identity;
            }
        }
        return Ok(());
    }
    record_geometry(host, id, key, false)?;
    // Updating an existing third-source entry changes its properties only.
    // The original receiver never reads or substitutes the supplied object.
    let existing_object = (sources[0] == 0 && sources[1] == 0 && sources[2] != 0)
        .then(|| {
            host.data_services
                .list_native
                .entries
                .0
                .get(&stamp)
                .cloned()
        })
        .flatten();
    if let Some(shared) = existing_object {
        let mut entry = shared.borrow_mut();
        entry.position = position;
        entry.base = base;
        entry.rotation = rotation;
        entry.scale = scale;
        entry.opacity = raw_opacity.min(255) as u8;
        entry.raw_opacity = raw_opacity;
        entry.visible = visible;
        entry.transform = transform;
        if command.code == 286 && identity != 0 {
            entry.identity = identity;
        }
        drop(entry);
        registration_history(host, id, key)?;
        invalidate_entry(host, id, key);
        return Ok(());
    }
    let (source, mut frames, mut selection, mut frame_handles, mut frame_charges) = if legacy_empty {
        (Source::Empty, BTreeMap::new(), None, BTreeMap::new(), BTreeMap::new())
    } else if sources[0] != 0 {
        let original = host
            .data_services
            .image_wrapper_pixels
            .iter()
            .find_map(|(&wrapper, &pixels)| (pixels == sources[0]).then_some(wrapper))
            .context("list first source is not a live pixel identity")?;
        (
            Source::Wrapper(host.share_image_wrapper(original)?),
            BTreeMap::new(),
            None,
            BTreeMap::new(),
            BTreeMap::new(),
        )
    } else if sources[1] != 0 {
        let bytes = host.data_services.ordinary.raw_image(sources[1])?;
        let object = host.create_list_entry_source(&bytes)?;
        if !host.data_services.list_native.entries.contains_key(&stamp) {
            {
                let mut entry = host.data_services.list_native.objects[&object].borrow_mut();
                entry.priority = (key >> 16) as i32;
                entry.animation.reset_time_on_update = true;
            }
            let record = bytes_for_registration(
                position,
                base,
                rotation,
                scale,
                raw_opacity,
                visible,
                transform,
                identity,
                object,
            );
            let result = register_owned(host, command, &record);
            host.release_list_entry_source(object)?;
            return result;
        }
        let shared = host
            .data_services
            .list_native
            .objects
            .remove(&object)
            .unwrap();
        let mut produced = Rc::try_unwrap(shared)
            .map_err(|_| anyhow::anyhow!("fresh list producer unexpectedly shared its object"))?
            .into_inner();
        host.data_services.list_native.object_owners.remove(&object);
        let frame_handles = std::mem::take(&mut produced.frame_handles);
        let frame_charges = std::mem::take(&mut produced.frame_charges);
        // The registration below adopts these exact wrappers.
        (Source::Frame, produced.frames, produced.selected_frame, frame_handles, frame_charges)
    } else if sources[2] != 0 {
        let shared = host
            .data_services
            .list_native
            .objects
            .get(&sources[2])
            .cloned()
            .context("list third source is not a retained entry-object handle")?;
        ensure_source_idle(&host.data_services.list_native, &shared)?;
        let previous_binding = host
            .data_services
            .list_native
            .entries
            .0
            .iter()
            .find_map(|(&stamp, entry)| Rc::ptr_eq(entry, &shared).then_some(stamp));
        if let Some((list, key)) = previous_binding {
            record_geometry(host, list, key, false)?;
        }
        {
            let mut entry = shared.borrow_mut();
            entry.position = position;
            entry.base = base;
            entry.rotation = rotation;
            entry.scale = scale;
            entry.opacity = raw_opacity.min(255) as u8;
            entry.raw_opacity = raw_opacity;
            entry.visible = visible;
            entry.transform = transform;
            entry.key = key;
            entry.clip = None;
            if command.code == 286 && identity != 0 {
                entry.identity = identity;
            }
        }
        host.data_services
            .list_native
            .entries
            .0
            .insert(stamp, shared);
        registration_history(host, id, key)?;
        invalidate_entry(host, id, key);
        return Ok(());
    } else {
        let resource = host
            .image_resources
            .get(&sources[3])
            .context("list fourth source is not a live image resource")?;
        let references = resource
            .references
            .checked_add(1)
            .context("list resource reference overflow")?;
        let frame = resource.image.clone();
        let existing =
            host.data_services
                .list_native
                .entries
                .values()
                .find_map(|entry| match entry.source {
                    Source::Resource { handle, wrapper } if handle == sources[3] => Some(wrapper),
                    _ => None,
                });
        let wrapper = match existing {
            Some(wrapper) => {
                if visible {
                    host.replace_image_wrapper_pixels(wrapper, frame)?;
                }
                host.share_image_wrapper(wrapper)?
            }
            None => host.register_image_wrapper(frame, false)?,
        };
        host.image_resources
            .get_mut(&sources[3])
            .unwrap()
            .references = references;
        (
            Source::Resource {
                handle: sources[3],
                wrapper,
            },
            BTreeMap::new(),
            None,
            BTreeMap::new(),
            BTreeMap::new(),
        )
    };
    let replacement = (|| -> Result<_> {
        let Source::Wrapper(wrapper) = source else { return Ok(None); };
        let Some((id, old_wrapper, original_bytes)) = host.data_services.list_native.entries.get(&stamp)
            .and_then(|previous| {
                let id = previous.selected_frame?;
                let frame = previous.frames.get(&id)?;
                let wrapper = *previous.frame_handles.get(&id)?;
                Some((id, wrapper, frame.pixels().len()))
            })
        else { return Ok(None); };
        if !host.image_handles.contains_key(&old_wrapper) {
            bail!("list frame cache wrapper was released");
        }
        let bytes = host.image_handles.get(&wrapper)
            .context("list replacement pixel wrapper disappeared")?.pixels().len()
            .checked_add(original_bytes).context("retained list frame cache allocation size overflow")?;
        let charge = host.data_services.list_native.frame_cache.reserve(bytes)?;
        let wrapper = host.share_image_wrapper(wrapper)?;
        Ok(Some((id, wrapper, charge)))
    })();
    let replacement = match replacement {
        Ok(replacement) => replacement,
        Err(error) => {
            host.release_list_source(source)?;
            return Err(error);
        }
    };
    let previous = host.data_services.list_native.entries.remove(&stamp);
    if frames.is_empty() && !legacy_empty {
        if let Some(previous) = previous.as_ref() {
            let mut previous = previous.borrow_mut();
            frames = std::mem::take(&mut previous.frames);
            frame_handles = std::mem::take(&mut previous.frame_handles);
            frame_charges = std::mem::take(&mut previous.frame_charges);
            selection = previous.selected_frame;
            if let Some((id, wrapper, charge)) = replacement {
                if let Some(old) = frame_handles.insert(id, wrapper) {
                    host.release_image_wrapper(old)?;
                }
                frame_charges.insert(id, charge);
            }
        }
    }
    let (priority, clip, mut animation, old_identity) = previous
        .as_ref()
        .map(|entry| {
            let entry = entry.borrow();
            (
                entry.priority,
                entry.clip,
                entry.animation.clone(),
                entry.identity,
            )
        })
        .unwrap_or(((key >> 16) as i32, None, ImageAnimation::default(), 0));
    if let Some(timeline) = animation.timeline_mut() {
        timeline.interpreter.max_image_id = frames.keys().copied().max().unwrap_or(0) as i32;
    }
    if matches!(source, Source::Frame) {
        animation.reset_time_on_update = true;
    }
    let next = Entry {
        key,
        source,
        position,
        base,
        rotation,
        scale,
        opacity: raw_opacity.min(255) as u8,
        raw_opacity,
        visible,
        transform,
        priority,
        clip,
        animation,
        frames,
        frame_handles,
        frame_charges,
        selected_frame: selection,
        identity: if command.code == 286 && identity != 0 {
            identity
        } else {
            old_identity
        },
    };
    if let Some(previous) = previous {
        let old = previous.replace(next);
        host.data_services
            .list_native
            .entries
            .0
            .insert(stamp, previous);
        host.release_list_entry_values(old)?;
    } else {
        host.data_services.list_native.entries.insert(stamp, next);
    }
    if visible && let Source::Resource { handle, .. } = source {
        host.image_resources.get_mut(&handle).unwrap().updated = false;
    }
    registration_history(host, id, key)?;
    invalidate_entry(host, id, key);
    Ok(())
}

fn bytes_for_registration(
    position: [f32; 2],
    base: [f32; 2],
    rotation: f32,
    scale: [f32; 2],
    opacity: u32,
    visible: bool,
    transform: u32,
    identity: u32,
    object: u32,
) -> Vec<u8> {
    [
        0,
        0,
        object,
        0,
        position[0].to_bits(),
        position[1].to_bits(),
        base[0].to_bits(),
        base[1].to_bits(),
        rotation.to_bits(),
        scale[0].to_bits(),
        scale[1].to_bits(),
        opacity,
        u32::from(visible),
        transform,
        identity,
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect()
}

impl LoggingHost<'_> {
    /// Ordinary 703 owns its producer through a VM record, independently of
    /// any task list. Registration can later retain this same entry object.
    pub(super) fn create_unowned_list_entry_source(&mut self, bytes: &[u8], path: Option<&str>) -> Result<u32> {
        self.create_retained_image_source(bytes, false, path)
    }

    /// Produces the retained entry object used by the image-name/ANM loader.
    /// The returned identity belongs to the third registration source, and
    /// deliberately has no image-wrapper or pixel-table binding.
    pub(super) fn create_list_entry_source(&mut self, bytes: &[u8]) -> Result<u32> {
        if !bytes.starts_with(b"HG-3") { bail!("list raw entry producer requires HG-3"); }
        self.create_retained_image_source(bytes, true, None)
    }

    fn create_retained_image_source(&mut self, bytes: &[u8], owned: bool, path: Option<&str>) -> Result<u32> {
        if self.data_services.list_native.objects.len() >= 65_536 {
            bail!("retained list entry-object table exceeds 65536 handles");
        }
        // 5e1d50 enumerates HG-3 IDs; all other image dispatchers provide
        // the single ID zero. Decode through the common dispatcher so every
        // cached frame retains attributes, pixel depth and native geometry.
        use crate::perf_detail::{ImageSourceStage, ImageSourceStep};
        let ids = {
            let _stage = ImageSourceStep::new(ImageSourceStage::RawDirectory);
            if bytes.starts_with(b"HG-3") {
                cs2_formats::HgFile::parse(bytes)?.frames.into_iter().map(|frame| frame.id).collect::<Vec<_>>()
            } else { vec![0] }
        };
        let mut entry = Entry::empty(Source::Frame);
        let result = (|| {
            for id in ids {
                if entry.frames.contains_key(&id) {
                    continue;
                }
                let image = {
                    let _stage = ImageSourceStep::new(ImageSourceStage::RawDecode);
                    match path {
                        Some(path) => self.decode_named_image_frame(bytes, id, path)?,
                        None => self.decode_managed_image_frame(bytes, id)?,
                    }
                };
                let Some(image) = image else { continue; };
                let _stage = ImageSourceStep::new(ImageSourceStage::RawRegister);
                // The source frame keeps its original geometry. The wrapper
                // holds an independent copy that callers may mutate.
                let bytes = image.pixels().len().checked_mul(2)
                    .context("retained list frame cache allocation size overflow")?;
                let charge = self.data_services.list_native.frame_cache.reserve(bytes)?;
                let wrapper = self.register_image_wrapper(image.clone(), false)?;
                entry.frame_handles.insert(id, wrapper);
                entry.frame_charges.insert(id, charge);
                entry.frames.insert(id, image);
            }
            let (&id, frame) = entry.frames.first_key_value().context("list entry producer has no frame")?;
            entry.selected_frame = Some(id);
            entry.base = frame.metadata().base.map(|value| value as f32);
            Ok(())
        })();
        if let Err(error) = result {
            self.release_list_entry_values(entry)?;
            return Err(error);
        }
        let handle = self.allocate_handle();
        self.data_services
            .list_native
            .objects
            .insert(handle, Rc::new(RefCell::new(entry)));
        if owned {
            self.data_services.list_native.object_owners.insert(handle, self.current_task_id);
        }
        Ok(handle)
    }

    pub(super) fn retain_list_entry_source(
        &mut self,
        list: u32,
        sender: u32,
        slot: u32,
    ) -> Result<u32> {
        if self.data_services.list_native.objects.len() >= 65_536 {
            bail!("retained list entry-object table exceeds 65536 handles");
        }
        let shared = self
            .data_services
            .list_native
            .entries
            .0
            .get(&(list, list_key(sender, slot)))
            .cloned()
            .context("list entry-object producer target is absent")?;
        ensure_source_idle(&self.data_services.list_native, &shared)?;
        let handle = self.allocate_handle();
        self.data_services
            .list_native
            .objects
            .insert(handle, shared);
        self.data_services
            .list_native
            .object_owners
            .insert(handle, self.current_task_id);
        Ok(handle)
    }

    pub(super) fn release_list_entry_source(&mut self, handle: u32) -> Result<()> {
        let source = self
            .data_services
            .list_native
            .objects
            .remove(&handle)
            .context("list entry-object handle is absent or already released")?;
        self.data_services.list_native.object_owners.remove(&handle);
        self.release_list_entry(source)
    }

    /// FES raw IMAGE uses the same retained frame directory and wrappers as
    /// raw list registration. No file reopen or second decoded cache is needed.
    pub(crate) fn retained_image_source_frame(&self, handle: u32, select: Option<u32>)
        -> Result<Option<RgbaFrame>> {
        let source = self.data_services.list_native.objects.get(&handle)
            .context("FES raw IMAGE retained source was released")?;
        let mut entry = source.borrow_mut();
        if let Some(id) = select.filter(|id| entry.frames.contains_key(id)) {
            entry.selected_frame = Some(id);
            entry.base = entry.frames[&id].metadata().base.map(|value| value as f32);
        }
        let Some(id) = entry.selected_frame else { return Ok(None); };
        if let Some(wrapper) = entry.frame_handles.get(&id) {
            return self.image_handles.get(wrapper).cloned().map(Some)
                .context("FES raw IMAGE cached frame wrapper was released");
        }
        Ok(entry.frames.get(&id).cloned())
    }

    pub(crate) fn has_retained_image_source(&self, handle: u32) -> bool {
        self.data_services.list_native.objects.contains_key(&handle)
    }

    /// 706 reads the smallest unsigned frame ID, independently of the
    /// currently displayed frame. 709 borrows the Nth wrapper in that same
    /// order; 6a1e90 only waits for decoding and does NOT select the frame or
    /// add a reference. Our retained entries decode synchronously on attach.
    pub(super) fn list_entry_record_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        if args.len() != 8 {
            return Err(native_state_error("native 706/709 requires exactly 8 argument bytes"));
        }
        let handle = read_u32(args, 0)?;
        if handle == 0 { return Ok(NativeReturn::Value(0)); }
        let shared = self.data_services.list_native.objects.get(&handle)
            .ok_or_else(|| native_state_error("native 706/709 invalid retained image entry"))?;
        let entry = shared.borrow();
        if index == 709 {
            let ordinal = (read_u32(args, 4)? as i32).max(0) as usize;
            let wrapper = entry.frame_handles.values().nth(ordinal).copied().unwrap_or(0);
            if wrapper != 0 && !self.image_handles.contains_key(&wrapper) {
                return Err(native_state_error("native 709 borrowed frame wrapper was released"));
            }
            return Ok(NativeReturn::Value(wrapper));
        }
        let address = read_u32(args, 4)?;
        match vm.read(address, 36) {
            Err(cs2_script::ScriptError::MemoryOutOfBounds { .. }) => return Ok(NativeReturn::Value(0)),
            other => { other?; }
        }
        // The original clears the complete output before asking the entry.
        vm.write(address, &[0; 36])?;
        let frame = if let Some(wrapper) = entry.frame_handles.values().next() {
            Some(self.image_handles.get(wrapper)
                .ok_or_else(|| native_state_error("native 706 first frame wrapper was released"))?)
        } else {
            match entry.source {
                Source::Wrapper(wrapper) | Source::Resource { wrapper, .. } =>
                    Some(self.image_handles.get(&wrapper)
                        .ok_or_else(|| native_state_error("native 706 current frame wrapper was released"))?),
                Source::Frame | Source::Empty => None,
            }
        };
        let Some(frame) = frame else { return Ok(NativeReturn::Value(0)); };
        let metadata = frame.metadata();
        let fields = [metadata.offset[0] as u32, metadata.offset[1] as u32,
            if metadata.canvas_size[0] == 0 { frame.width() } else { metadata.canvas_size[0] },
            if metadata.canvas_size[1] == 0 { frame.height() } else { metadata.canvas_size[1] },
            frame.width(), frame.height(), u32::from(metadata.bit_depth) / 8,
            metadata.base[0] as u32, metadata.base[1] as u32];
        vm.write(address, &fields.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>())?;
        Ok(NativeReturn::Value(1))
    }

    pub(super) fn receive_list_animation(
        &mut self,
        command: cs2_runtime::TaskCommand,
        buffers: &super::OwnedTaskBuffers,
        _output: &mut u32,
        budget: usize,
    ) -> cs2_script::Result<Option<cs2_runtime::TaskCommandOutcome>> {
        if !matches!(command.code, 299 | 301 | 306 | 318) {
            return Ok(None);
        }
        let id = command.target as u32;
        let result = (|| -> Result<bool> {
            if self.scene_nodes.get(&id).is_none_or(|node| node.kind != 15)
                || self
                    .data_services
                    .tasks
                    .message_task_state(id)
                    .is_some_and(|(_, deleted, _)| deleted != 0)
            {
                return Ok(true);
            }
            ensure_shared_entries_idle(&self.data_services.list_native, id)?;
            if let Some(pending) = self.data_services.list_native.animation_updates.get(&id) {
                if pending.command != Some(command) {
                    bail!("list animation continuation has another command");
                }
                let now = pending.now;
                return update_animations(self, id, now, budget);
            }
            let key = list_key(command.sender as u32, command.data[0]);
            if command.code == 299 {
                let bytes = buffers
                    .data1()
                    .context("list ANM filename buffer is absent")?;
                let end = bytes
                    .iter()
                    .position(|&byte| byte == 0)
                    .context("list ANM filename is not terminated")?;
                let name = decode_filename(&bytes[..end], self.text_encoding)?;
                let Some(resource) = self.resources.resolve_mode_utf16(
                    &name.encode_utf16().collect::<Vec<_>>(), cs2_runtime::resource_router::ResourceMode::LooseFirst,
                )? else {
                    return Ok(true);
                };
                let resource = resource.value;
                if resource.size > 64 * 1024 * 1024 {
                    bail!("list ANM source exceeds 64 MiB");
                }
                let bytes = self.resources.read_candidate(resource)?;
                if bytes.is_empty() {
                    return Ok(true);
                }
                if let Some(mut entry) = self.data_services.list_native.entries.get_mut(&(id, key))
                {
                    let maximum = entry.frames.keys().copied().max().unwrap_or(0);
                    entry.animation.load(&bytes, maximum)?;
                }
                return Ok(true);
            }
            let resource = self.data_services.list_native.entries.get(&(id, key))
                .and_then(|entry| match entry.source {
                    Source::Resource { handle, .. } => Some(handle), _ => None,
                });
            let now = self.data_services.tasks.task_clock(id).map_or(0, |clock| clock.elapsed());
            if let Some(handle) = resource {
                if command.code == 318 {
                    let bytes = buffers.data1().filter(|bytes| bytes.len() >= 8)
                        .context("list ANM variable needs eight explicit bytes")?;
                    self.set_image_resource_animation_variable(handle, read_u32(bytes, 0)? as usize,
                        read_u32(bytes, 4)? as i32);
                } else if command.code == 301 {
                    self.set_image_resource_animation(handle, command.data[1] as i32, now);
                } else { self.restart_image_resource_animation(handle); }
            }
            let Some(mut entry) = self.data_services.list_native.entries.get_mut(&(id, key)) else {
                return Ok(true);
            };
            if command.code == 318 {
                if entry.animation.timeline().is_none() { return Ok(true); }
                let bytes = buffers.data1().filter(|bytes| bytes.len() >= 8)
                    .context("list ANM variable needs eight explicit bytes")?;
                let index = read_u32(bytes, 0)? as usize;
                let value = read_u32(bytes, 4)? as i32;
                if let Some(timeline) = entry.animation.timeline_mut() {
                    if let Some(slot) = timeline.interpreter.variables.get_mut(index) {
                        *slot = value;
                    }
                }
                return Ok(true);
            }
            let keys = if command.code == 301 {
                entry.animation.playing = command.data[1] as i32;
                entry.animation.time = now;
                entry.animation.reset_time_on_update = false;
                if let Some(timeline) = entry.animation.timeline_mut() {
                    timeline.origin = now;
                }
                signed_keys(&self.data_services.list_native, id)
            } else {
                // Timeline restart retains variables/speed/origin. The object
                // wrapper performs one forced update even while paused.
                let reset_frame = entry.frames.contains_key(&10000);
                // The PE recursively restarts the fourth-source resource,
                // including when this outer entry has no ANM of its own.
                if !entry
                    .animation
                    .restart_with_random(budget, reset_frame, &mut || {
                        self.script_random.next_integer()
                    })?
                {
                    self.data_services.list_native.animation_updates.insert(
                        id,
                        AnimationUpdate {
                            command: Some(command),
                            now: u32::MAX,
                            keys: vec![key],
                            cursor: 0,
                            compose: true,
                            signal_ready: true,
                            entry_ready: false,
                            resource_changes: Vec::new(),
                        },
                    );
                    return Ok(false);
                }
                select_animation_frame(&mut entry, self.image_handles);
                drop(entry);
                invalidate_entry(self, id, key);
                self.data_services.list_native.animation_updates.insert(
                    id,
                    AnimationUpdate {
                        command: Some(command),
                        now: u32::MAX,
                        keys: vec![key],
                        cursor: 0,
                        compose: true,
                        signal_ready: true,
                        entry_ready: true,
                        resource_changes: Vec::new(),
                    },
                );
                return update_animations(self, id, u32::MAX, budget);
            };
            drop(entry);
            self.data_services.list_native.animation_updates.insert(
                id,
                AnimationUpdate {
                    command: Some(command),
                    now,
                    keys,
                    cursor: 0,
                    compose: true,
                    signal_ready: false,
                    entry_ready: false,
                    resource_changes: Vec::new(),
                },
            );
            update_animations(self, id, now, budget)
        })();
        result
            .map(|done| {
                Some(cs2_runtime::TaskCommandOutcome {
                    done,
                    propagate: false,
                })
            })
            .map_err(|error| {
                self.events.push(super::LogicEvent {
                    frame: self.frame,
                    kind: "image-list-error".into(),
                    detail: error.to_string(),
                });
                native_state_error("list ANM receiver failed; see image-list-error")
            })
    }

    pub(super) fn list_batch_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        if index == 624 {
            return self.list_animation_file_native(args, vm);
        }
        if matches!(index, 622 | 623) && args.len() < 52 {
            // The fixed original wrapper reads thirteen DWORDs even at the
            // observed eight-DWORD KCS callsites. These are the actual VM tail
            // bytes, not zero-filled defaults or additional popped arguments.
            let storage = vm.native_argument_storage(52)?.to_vec();
            return self.list_contract_native(index, &storage);
        }
        self.list_contract_native(index, args)
    }

    fn list_animation_file_native(
        &mut self, args: &[u8], vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        let id = read_u32(args, 0)?;
        let slot = read_u32(args, 4)?;
        let command = cs2_runtime::TaskCommand {
            target: id as i32, sender: self.current_task_id as i32,
            code: 299, data: [slot, 1, 0],
        };
        if let Some(mut buffers) = self.pending_owned_task_buffers(command) {
            // Resume the receiver's owned filename without formatting again.
            // A previous callback may already have changed the VM string or
            // resource aliases while exhausting its instruction budget.
            let outcome = self.route_task_command_buffers(command, &mut buffers, &mut 0, 4096)?;
            return Ok(if outcome.is_some_and(|outcome| !outcome.done) {
                NativeReturn::Retry
            } else { NativeReturn::Continue });
        }
        let formatted = super::expand_kcs_format(vm, &vm.read_string(read_u32(args, 8)?)?, 0)?;
        let name = super::sound_native::decode_name(self.text_encoding, super::trim_nul(&formatted))?;
        // Verified old wrappers complete image + .anm directly; later ones
        // query the manager/alias and send even an empty lookup result.
        let resolved = self.list_animation_filename624(&name)?;
        let mut encoded = match self.text_encoding {
            super::ScriptTextEncoding::Cp932 => {
                let (bytes, _, invalid) = encoding_rs::SHIFT_JIS.encode(&resolved);
                if invalid { return Err(native_state_error("list ANM path is not representable in CP932")); }
                bytes.into_owned()
            }
            super::ScriptTextEncoding::Gbk => {
                let (bytes, _, invalid) = encoding_rs::GBK.encode(&resolved);
                if invalid { return Err(native_state_error("list ANM path is not representable in GBK")); }
                bytes.into_owned()
            }
            super::ScriptTextEncoding::Utf8 => resolved.into_bytes(),
        };
        if encoded.len() > 1023 {
            return Err(native_state_error("list ANM filename exceeds its native temporary buffer"));
        }
        encoded.push(0);
        let mut buffers = super::OwnedTaskBuffers::new(None, Some(encoded))?;
        let outcome = self.route_task_command_buffers(command, &mut buffers, &mut 0, 4096)?;
        Ok(if outcome.is_some_and(|outcome| !outcome.done) {
            NativeReturn::Retry
        } else { NativeReturn::Continue })
    }

    pub(super) fn legacy_list_profile(&mut self) -> cs2_script::Result<bool> {
        if self.css_task_sha().is_none() {
            self.bind_utate_plane_input_profile()?;
        }
        Ok(is_legacy_list_pe(self.css_task_sha()))
    }

    pub(super) fn list_contract_native(
        &mut self,
        index: u32,
        args: &[u8],
    ) -> cs2_script::Result<NativeReturn> {
        let contract = CONTRACTS
            .iter()
            .find(|spec| spec.native == index)
            .ok_or_else(|| native_state_error("native has no image-list batch contract"))?;
        if matches!(index, 257 | 259) && args.len() == 32 {
            self.legacy_list_profile()?;
        }
        let short = uses_short_registration(self.css_task_sha(), index, args.len());
        if !short && args.len() < contract.bytes {
            return Err(native_state_error("image-list arguments are truncated"));
        }
        if matches!(index, 626 | 627) && args.len() != contract.bytes {
            return Err(native_state_error("legacy affine registration requires exactly 48 argument bytes"));
        }
        let id = read_u32(args, 0)?;
        let slot = if contract.bytes >= 8 {
            read_u32(args, 4)?
        } else {
            0
        };
        let (data, second) = match contract.command {
            286 | 288 => {
                let source = read_u32(args, 8)?;
                let field = match index { 622 | 623 => 4, 707 | 708 => 8, 732 => 12, _ => 0 };
                let handle = if matches!(index, 622 | 623 | 707 | 708 | 732) || source == 0 {
                    source
                } else {
                    *self
                        .data_services
                        .image_wrapper_pixels
                        .get(&source)
                        .ok_or_else(|| {
                            native_state_error("list source wrapper has no pixel identity")
                        })?
                };
                let record = list_registration_record(index, args, field, handle, short)?;
                ([slot, 1, 0], Some(record))
            }
            295 => {
                let x = read_u32(args, 8)?;
                let y = read_u32(args, 12)?;
                let record = [
                    x,
                    y,
                    x.wrapping_add(read_u32(args, 16)?),
                    y.wrapping_add(read_u32(args, 20)?),
                ];
                (
                    [slot, 1, 0],
                    Some(record.into_iter().flat_map(u32::to_le_bytes).collect()),
                )
            }
            316 => ([0, 1, 0], Some(args[4..20].to_vec())),
            304 => ([slot, 1, 0], Some(u32::MAX.to_le_bytes().to_vec())),
            291 => ([0, 1, 0], Some(vec![0; 4])),
            313 => ([slot, 1, 0], Some(vec![0; 4])),
            302 => ([0, slot, 0], None),
            285 | 289 | 314 => ([slot, read_u32(args, 8)?, 0], None),
            292 | 310 => ([0, slot, 0], None),
            318 => ([slot, 1, 0], Some(args[8..16].to_vec())),
            293 | 301 | 305 | 317 | 319 => ([slot, read_u32(args, 8)?, 0], None),
            _ => ([slot, 0, 0], None),
        };
        let mut buffers = super::OwnedTaskBuffers::new(None, second)?;
        let mut output = 0;
        let result = self.route_task_command_buffers(
            cs2_runtime::TaskCommand {
                target: id as i32,
                sender: self.current_task_id as i32,
                code: contract.command,
                data,
            },
            &mut buffers,
            &mut output,
            4096,
        )?;
        Ok(if result.is_some_and(|outcome| !outcome.done) {
            NativeReturn::Retry
        } else if contract.returns_priority || matches!(index, 261 | 657) {
            NativeReturn::Value(read_u32(buffers.data1().unwrap(), 0)?)
        } else {
            NativeReturn::Continue
        })
    }

    fn release_list_resource(&mut self, handle: u32) -> Result<()> {
        let resource = self
            .image_resources
            .get_mut(&handle)
            .context("image-list held resource disappeared")?;
        resource.references = resource
            .references
            .checked_sub(1)
            .context("image-list resource reference underflow")?;
        if resource.references == 0 {
            self.image_resources.remove(&handle);
            self.release_image_resource_layers(handle);
        }
        Ok(())
    }

    fn release_list_source(&mut self, source: Source) -> Result<()> {
        match source {
            Source::Resource { handle, wrapper } => {
                self.release_image_wrapper(wrapper)?;
                self.release_list_resource(handle)
            }
            Source::Wrapper(handle) => Ok(self.release_image_wrapper(handle)?),
            Source::Frame | Source::Empty => Ok(()),
        }
    }

    fn release_list_entry(&mut self, entry: SharedEntry) -> Result<()> {
        if let Ok(entry) = Rc::try_unwrap(entry) {
            self.release_list_entry_values(entry.into_inner())?;
        }
        Ok(())
    }

    fn release_list_entry_values(&mut self, entry: Entry) -> Result<()> {
        let mut result = self.release_list_source(entry.source);
        for wrapper in entry.frame_handles.into_values() {
            let released = self.release_image_wrapper(wrapper);
            result = result.and(released.map_err(Into::into));
        }
        result
    }

    fn update_native_list_region(&mut self, id: u32, clip: Rect) -> Result<()> {
        tiles::compose(self, id, false, Some(clip))
    }
}

fn list_registration_record(
    index: u32,
    args: &[u8],
    field: usize,
    handle: u32,
    short: bool,
) -> cs2_script::Result<Vec<u8>> {
    let mut record = vec![0u8; 60];
    record[field..field + 4].copy_from_slice(&handle.to_le_bytes());
    if short {
        let visibility = read_u32(args, 28)?;
        if read_u32(args, 24)? != 1 {
            return Err(native_state_error("unsupported legacy image-list registration field"));
        }
        let position_x = read_u32(args, 12)? as i32 as f32;
        let position_y = read_u32(args, 16)? as i32 as f32;
        record[16..20].copy_from_slice(&position_x.to_le_bytes());
        record[20..24].copy_from_slice(&position_y.to_le_bytes());
        record[36..40].copy_from_slice(&1f32.to_le_bytes());
        record[40..44].copy_from_slice(&1f32.to_le_bytes());
        record[44..48].copy_from_slice(&read_u32(args, 20)?.min(255).to_le_bytes());
        record[48..52].copy_from_slice(&visibility.to_le_bytes());
    } else if matches!(index, 626 | 627) {
        record[16..52].copy_from_slice(
            args.get(12..48).ok_or_else(|| native_state_error("image-list arguments are truncated"))?,
        );
        record[44..48].copy_from_slice(&read_u32(args, 40)?.min(255).to_le_bytes());
        record[52..56].copy_from_slice(&1u32.to_le_bytes());
    } else {
        record[16..56].copy_from_slice(
            args.get(12..52).ok_or_else(|| native_state_error("image-list arguments are truncated"))?,
        );
        if matches!(index, 257 | 259 | 622 | 623 | 707 | 708 | 732) {
            record[44..48].copy_from_slice(&read_u32(args, 40)?.min(255).to_le_bytes());
        }
    }
    if (short || matches!(index, 626 | 627)) && handle == 0 {
        record.extend_from_slice(LEGACY_EMPTY_LIST_SOURCE);
    }
    Ok(record)
}

#[cfg(test)]
mod registration_tests {
    use super::*;

    fn words(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|value| value.to_le_bytes()).collect()
    }

    #[test]
    fn story_entry_records_are_short_only_for_verified_executables() {
        for (sha, target, wrapper) in [
            (crate::css_model::UTATE_SHA, 11, 40),
            (crate::particle_external::CAMPANELLA_SHA, 33, 33),
        ] {
            let args = words(&[target, 1, wrapper, 0, 0, 255, 1, 1]);
            let expected = words(&[91, 0, 0, 0, 0, 0, 0, 0, 0, 1f32.to_bits(), 1f32.to_bits(), 255, 1, 0, 0]);
            for native in [257, 259] {
                assert!(uses_short_registration(Some(sha), native, args.len()));
                assert_eq!(list_registration_record(native, &args, 0, 91, true).unwrap(), expected);
                assert!(!uses_short_registration(None, native, args.len()));
                assert!(!uses_short_registration(Some("other"), native, args.len()));
                assert!(!uses_short_registration(Some(sha), native, 31));
                assert!(!uses_short_registration(Some(sha), native, 33));
                assert!(!uses_short_registration(Some(sha), native, 52));
            }
            assert!(!uses_short_registration(Some(sha), 622, args.len()));
        }
    }

    #[test]
    fn tsunaban_meswnd2_uses_integer_registration_without_the_affine_profile() {
        let sha = Some(super::super::css_model::SHA);
        let captured = words(&[10, 10, 46, 0, (-374i32) as u32, 255, 1, 1]);
        let expected = words(&[
            91, 0, 0, 0, 0, (-374f32).to_bits(), 0, 0, 0, 1f32.to_bits(), 1f32.to_bits(),
            255, 1, 0, 0,
        ]);
        for index in [257, 259] {
            assert!(uses_short_registration(sha, index, captured.len()));
            assert_eq!(list_registration_record(index, &captured, 0, 91, true).unwrap(), expected);
            assert!(!uses_short_registration(sha, index, 31));
            assert!(!uses_short_registration(sha, index, 33));
            assert!(!uses_short_registration(sha, index, 52));
            let empty = list_registration_record(index, &captured, 0, 0, true).unwrap();
            assert_eq!(&empty[60..], LEGACY_EMPTY_LIST_SOURCE);
        }
        assert!(!uses_short_registration(sha, 622, captured.len()));
        assert!(!uses_short_registration(sha, 627, captured.len()));
        assert!(!is_legacy_list_pe(sha));
    }

    #[test]
    fn legacy_position_opacity_visibility_and_empty_source_follow_the_owned_record() {
        let args = words(&[33, 1, 0, (-21i32) as u32, 57, 999, 1, 0]);
        let record = list_registration_record(259, &args, 0, 0, true).unwrap();
        assert_eq!(float(&record, 16).unwrap(), -21.0);
        assert_eq!(float(&record, 20).unwrap(), 57.0);
        assert_eq!(read_u32(&record, 44).unwrap(), 255);
        assert_eq!(read_u32(&record, 48).unwrap(), 0);
        assert_eq!(&record[60..], LEGACY_EMPTY_LIST_SOURCE);
        assert!(list_registration_record(259, &args[..31], 0, 0, true).is_err());

        let unknown_field = words(&[33, 1, 0, 0, 0, 255, 2, 1]);
        assert!(list_registration_record(259, &unknown_field, 0, 0, true).is_err());
    }

    #[test]
    fn legacy_affine_expands_the_record_and_modern_registration_keeps_its_layout() {
        let args = words(&[11, 300, 86, 1, 2, 3, 4, 5, 6, 7, 999, 9, 10]);
        let affine = list_registration_record(627, &args[..48], 0, 91, false).unwrap();
        assert_eq!(affine, words(&[91, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 255, 9, 1, 0]));
        assert!(list_registration_record(627, &args[..47], 0, 91, false).is_err());

        let modern = list_registration_record(257, &args, 0, 0, false).unwrap();
        assert_eq!(modern, words(&[0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 255, 9, 10, 0]));
        assert!(list_registration_record(257, &args[..32], 0, 91, false).is_err());
    }

    #[test]
    fn campanella_affine_wrappers_use_the_same_record_and_receiver() {
        let args = words(&[33, 60, 46, 772f32.to_bits(), 263f32.to_bits(), 772f32.to_bits(),
            563f32.to_bits(), 0, 1f32.to_bits(), 1f32.to_bits(), 255, 0]);
        let registered = list_registration_record(626, &args, 0, 91, false).unwrap();
        let sibling = list_registration_record(627, &args, 0, 91, false).unwrap();
        assert_eq!(registered, sibling);
        assert_eq!(registered, words(&[91, 0, 0, 0, 772f32.to_bits(), 263f32.to_bits(),
            772f32.to_bits(), 563f32.to_bits(), 0, 1f32.to_bits(), 1f32.to_bits(), 255, 0, 1, 0]));
        for slot in [626, 627] {
            let contract = CONTRACTS.iter().find(|contract| contract.native == slot).unwrap();
            assert_eq!((contract.bytes, contract.command), (48, 288));
        }
    }
}

fn list_key(sender: u32, slot: u32) -> u32 {
    (slot << 16) | sender
}

fn signed_keys(store: &ListNativeStore, id: u32) -> Vec<u32> {
    let mut keys: Vec<_> = store
        .entries
        .keys()
        .filter_map(|&(target, key)| (target == id).then_some(key))
        .collect();
    keys.sort_by_key(|&key| key as i32);
    keys
}

fn select_animation_frame(entry: &mut Entry, images: &BTreeMap<u32, RgbaFrame>) -> bool {
    if let Some(frame) = entry.animation.frame_change() {
        if entry.frames.contains_key(&(frame as u32)) {
            entry.selected_frame = Some(frame as u32);
            if let Some(source) = entry.frame_handles.get(&(frame as u32))
                .and_then(|wrapper| images.get(wrapper))
                .or_else(|| entry.frames.get(&(frame as u32)))
            {
                entry.base = source.metadata().base.map(|value| value as f32);
            }
        }
        return true;
    }
    false
}

pub(super) fn update_animations(
    host: &mut LoggingHost<'_>,
    id: u32,
    now: u32,
    budget: usize,
) -> Result<bool> {
    ensure_shared_entries_idle(&host.data_services.list_native, id)?;
    let mut progress = host
        .data_services
        .list_native
        .animation_updates
        .remove(&id)
        .unwrap_or(AnimationUpdate {
            command: None,
            now,
            keys: signed_keys(&host.data_services.list_native, id),
            cursor: 0,
            compose: false,
            signal_ready: false,
            entry_ready: false,
            resource_changes: Vec::new(),
        });
    let result = (|| {
        while let Some(&key) = progress.keys.get(progress.cursor) {
            if !progress.signal_ready {
                if let Some(Source::Resource { handle, wrapper }) = host
                    .data_services
                    .list_native
                    .entries
                    .get(&(id, key))
                    .map(|entry| entry.source)
                {
                    let resource = host
                        .image_resources
                        .get_mut(&handle)
                        .context("list retained resource disappeared during animation update")?;
                    if resource.updated {
                        let frame = resource.image.clone();
                        resource.updated = false;
                        host.replace_image_wrapper_pixels(wrapper, frame)?;
                    }
                }
                if let Some((channel, threshold)) = host
                    .data_services
                    .list_native
                    .sound_bindings
                    .get(&(id, key))
                    .copied()
                {
                    let signal = sound_signal(host, id as i32, channel, None)?;
                    if let Some(mut entry) = host
                        .data_services
                        .list_native
                        .entries
                        .get_mut(&(id, key))
                        .filter(|entry| entry.animation.timeline().is_some())
                    {
                        entry
                            .animation
                            .timeline_mut()
                            .unwrap()
                            .interpreter
                            .variables[0] = i32::from(signal >= threshold);
                    }
                }
                progress.signal_ready = true;
            }
            if !progress.entry_ready && let Some(mut entry) = host.data_services.list_native.entries.get_mut(&(id, key)) {
                let reset_frame = entry.frames.contains_key(&10000);
                if !entry.animation.update_with_random(
                    progress.now,
                    budget,
                    reset_frame,
                    &mut || host.script_random.next_integer(),
                )? {
                    return Ok(false);
                }
            }
            progress.entry_ready = true;
            if let Some(Source::Resource { handle, wrapper }) = host.data_services.list_native.entries
                .get(&(id, key)).map(|entry| entry.source)
            {
                if !host.update_image_resource_animation(handle, progress.now, budget)? {
                    return Ok(false);
                }
                if host.take_image_resource_animation_change(handle) {
                    record_geometry(host, id, key, false)?;
                    let resource = host.image_resources.get_mut(&handle)
                        .context("internal ANM resource was released")?;
                    let frame = resource.image.clone();
                    resource.updated = false;
                    host.replace_image_wrapper_pixels(wrapper, frame)?;
                    progress.resource_changes.push(key);
                }
            }
            progress.cursor += 1;
            progress.signal_ready = false;
            progress.entry_ready = false;
        }
        for key in &progress.keys {
            record_geometry(host, id, *key, false)?;
            let dirty = if let Some(mut entry) =
                host.data_services.list_native.entries.get_mut(&(id, *key))
            {
                let changed = select_animation_frame(&mut entry, host.image_handles);
                changed && entry.visible && entry.animation.display().visible != 0
            } else {
                false
            };
            record_geometry(host, id, *key, false)?;
            if dirty || progress.resource_changes.contains(key)
                || progress.command.is_some_and(|command| command.code == 306) {
                invalidate_entry(host, id, *key);
            }
        }
        if progress.compose {
            super::fes_task_platform::compose_list(host, id, false)?;
        }
        Ok(true)
    })();
    if !matches!(result, Ok(true)) {
        host.data_services
            .list_native
            .animation_updates
            .insert(id, progress);
    }
    result
}

fn decode_filename(bytes: &[u8], encoding: super::ScriptTextEncoding) -> Result<String> {
    let codec = match encoding {
        super::ScriptTextEncoding::Cp932 => encoding_rs::SHIFT_JIS,
        super::ScriptTextEncoding::Gbk => encoding_rs::GBK,
        super::ScriptTextEncoding::Utf8 => encoding_rs::UTF_8,
    };
    let (value, invalid) = codec.decode_without_bom_handling(bytes);
    if invalid {
        bail!("list ANM filename is invalid in selected script encoding");
    }
    Ok(value.into_owned())
}

/// Called by the normal type-15 compositor. The reserved layer owner cannot
/// collide with task IDs, which are limited to the positive i32 range.
pub(super) fn prepare_layers(host: &mut LoggingHost<'_>, id: u32) -> Result<()> {
    if host
        .data_services
        .list_native
        .animation_updates
        .contains_key(&id)
    {
        bail!("finish the pending list ANM round before drawing or submitting");
    }
    ensure_shared_entries_idle(&host.data_services.list_native, id)?;
    let resources = host.data_services.list_native.entries.iter()
        .filter_map(|(&(target, _), entry)| {
            (target == id && entry.visible && (entry.animation.timeline().is_none()
                || entry.animation.display().visible != 0)).then_some(entry.source)
        }).filter_map(|source| match source {
            Source::Resource { handle, .. } => Some(handle), _ => None,
        }).collect::<std::collections::BTreeSet<_>>();
    for handle in resources { host.redraw_image_resource_surface(handle)?; }
    let Some(list) = host.data_services.tasks.fes.platform.lists.get(&id) else {
        return Ok(());
    };
    let is_utate = host.css_task_sha() == Some(super::css_model::UTATE_SHA);
    let mut layers = Vec::new();
    for ((target, key), entry) in host.data_services.list_native.entries.iter() {
        if *target != id || matches!(entry.source, Source::Empty) {
            continue;
        }
        let wrapper = entry.current_wrapper().context("list source has no current pixel wrapper")?;
        let source = host.image_handles.get(&wrapper)
            .context("list entry's retained pixel wrapper disappeared")?;
        let offset = source_geometry(host, &entry, source)?.offset;
        let [left, top] = entry_top_left(entry.position, entry.base, offset);
        let mut options = ImageDrawOptions::new(Point::new(left, top));
        options.visible = entry.visible;
        options.opacity = entry.opacity;
        if entry.animation.timeline().is_some() {
            let display = entry.animation.display().compose(
                (entry.position[0], entry.position[1]),
                i32::from(entry.opacity),
                i32::from(entry.visible),
            );
            options.visible = display.visible != 0;
            options.opacity = display.blend.clamp(0, 255) as u8;
        }
        options.clip = entry.clip;
        if entry.transform != 0 {
            options.scale_x = entry.scale[0];
            options.scale_y = entry.scale[1];
            options.rotation_radians = entry.rotation;
            options.pivot = entry.base;
        } else if is_utate && source.metadata().bit_depth == 32
            && host.data_services.image_control_values.get(&wrapper) == Some(&4)
        {
            options.blend_mode = cs2_renderer::BlendMode::ListAlphaFromAlpha;
        }
        let layer_key = (u32::MAX, format!("\0list-native:{key:08x}"));
        if list.layers.get(&layer_key).is_some_and(|previous| {
            previous.options.blend_mode != options.blend_mode
        }) {
            host.data_services.list_native.invalidated.insert((id, *key));
        }
        layers.push((
            layer_key,
            Layer {
                frame: source.clone(),
                options,
                priority: entry.priority,
                affine: None,
            },
        ));
    }
    let list = host
        .data_services
        .tasks
        .fes
        .platform
        .lists
        .get_mut(&id)
        .unwrap();
    list.layers.retain(|(owner, _), _| *owner != u32::MAX);
    list.layers.extend(layers);
    Ok(())
}

/// Command 297 removes entries while retaining the independent background.
pub(super) fn clear_entries(host: &mut LoggingHost<'_>, id: u32) -> Result<()> {
    if host
        .data_services
        .list_native
        .animation_updates
        .contains_key(&id)
    {
        bail!("image-list clear requires its pending animation round to finish");
    }
    let keys: Vec<_> = host
        .data_services
        .list_native
        .entries
        .keys()
        .filter(|(target, _)| *target == id)
        .copied()
        .collect();
    for key in keys {
        let entry = host.data_services.list_native.entries.remove(&key).unwrap();
        host.release_list_entry(entry)?;
    }
    Ok(())
}

pub(super) fn restore_background(host: &mut LoggingHost<'_>, id: u32) -> Result<()> {
    if !host
        .data_services
        .tasks
        .fes
        .platform
        .lists
        .contains_key(&id)
    {
        return Ok(());
    }
    let Some(wrapper) = host
        .data_services
        .list_native
        .backgrounds
        .get(&id)
        .filter(|background| background.active)
        .map(|background| background.wrapper)
    else {
        return Ok(());
    };
    let wrapper = host.share_image_wrapper(wrapper)?;
    let entry = Entry {
        key: 0,
        source: Source::Wrapper(wrapper),
        position: [0.; 2],
        base: [0.; 2],
        rotation: 0.,
        scale: [1.; 2],
        opacity: 255,
        raw_opacity: 255,
        visible: true,
        transform: 0,
        priority: 0,
        clip: None,
        animation: ImageAnimation::default(),
        frames: BTreeMap::new(),
        frame_handles: BTreeMap::new(),
        frame_charges: BTreeMap::new(),
        selected_frame: None,
        identity: 0,
    };
    if let Some(old) = host
        .data_services
        .list_native
        .entries
        .insert((id, 0), entry)
    {
        host.release_list_entry(old)?;
    }
    Ok(())
}

pub(super) fn release_list(host: &mut LoggingHost<'_>, id: u32) -> Result<()> {
    host.data_services.list_native.animation_updates.remove(&id);
    clear_entries(host, id)?;
    if let Some(background) = host.data_services.list_native.backgrounds.remove(&id) {
        host.release_image_wrapper(background.wrapper)?;
    }
    host.data_services.list_native.task_offsets.remove(&id);
    host.data_services.list_native.switches.remove(&id);
    host.data_services.list_native.surfaces.remove(&id);
    host.data_services
        .list_native
        .invalidated
        .retain(|(list, _)| *list != id);
    let handles: Vec<_> = host
        .data_services
        .list_native
        .object_owners
        .iter()
        .filter_map(|(&handle, &owner)| (owner == id).then_some(handle))
        .collect();
    for handle in handles {
        host.release_list_entry_source(handle)?;
    }
    let bindings: Vec<_> = host
        .data_services
        .list_native
        .sound_bindings
        .iter()
        .filter_map(|(&(list, key), &(channel, _))| (list == id).then_some((key, channel)))
        .collect();
    for (key, channel) in bindings {
        sound_signal(host, id as i32, channel, Some(0))?;
        host.data_services
            .list_native
            .sound_bindings
            .remove(&(id, key));
    }
    Ok(())
}

fn sound_signal(
    host: &mut LoggingHost<'_>,
    sender: i32,
    channel: u32,
    enabled: Option<u32>,
) -> Result<u32> {
    let task = host
        .default_sound_task()?
        .context("list lip-sync requires a reachable sound task")?;
    let mut output = 0;
    if !host.dispatch_sound_task_command(
        task,
        cs2_runtime::TaskCommand {
            target: task as i32,
            sender,
            code: if enabled.is_some() { 212 } else { 213 },
            data: [channel, enabled.unwrap_or(0), 0],
        },
        &mut cs2_runtime::TaskCommandBuffers::default(),
        &mut output,
    )? {
        bail!("list lip-sync sound receiver declined its signal request");
    }
    Ok(output)
}

fn rectangle(values: [i32; 4]) -> Option<Rect> {
    let width = values[2].wrapping_sub(values[0]);
    let height = values[3].wrapping_sub(values[1]);
    (width > 0 && height > 0).then(|| Rect::new(values[0], values[1], width as u32, height as u32))
}

pub(super) fn compare_keys(left: &(u32, String), right: &(u32, String)) -> std::cmp::Ordering {
    let numeric = |key: &(u32, String)| {
        (key.0 == u32::MAX)
            .then(|| key.1.strip_prefix("\0list-native:"))
            .flatten()
            .and_then(|value| u32::from_str_radix(value, 16).ok())
    };
    match (numeric(left), numeric(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(0), None) => std::cmp::Ordering::Less,
        (None, Some(0)) => std::cmp::Ordering::Greater,
        _ => left.cmp(right),
    }
}

/// Full-DWORD opacity belongs to the list entry rather than ImageDrawOptions'
/// byte field. This path retains the documented byte lookup-table arithmetic.
pub(super) fn draw_layer(
    host: &LoggingHost<'_>,
    list_id: u32,
    canvas: &mut Canvas,
    key: &(u32, String),
    frame: &RgbaFrame,
    options: &ImageDrawOptions,
    affine: Option<AffinePlacement>,
    opaque: &mut Option<bool>,
) -> Result<()> {
    if !options.visible || frame.is_empty() { return Ok(()); }
    if let Some(placement) = affine {
        return draw_affine_placement(canvas, frame, [frame.width(), frame.height()], placement,
            u32::from(options.opacity), options.clip, options.blend_mode);
    }
    let entry = native_layer_entry(host, list_id, key);
    let Some(entry) = entry else {
        // FES IMAGE/CIMAGE/BUTTON, text and control projections are entries in
        // this same CPU list. Keep opacity outside the sampled alpha: the list
        // lookup uses opacity * (source_alpha + 1) >> 8, not a rounded /255.
        if plain_list_options(options) {
            return draw_plain_frame(
                canvas,
                frame,
                options,
                u32::from(options.opacity),
                opaque,
            );
        }
        use super::perf_detail::{GpuDecision, list_gpu_decision};
        let reason = if options.mask.is_some() {
            GpuDecision::DrawMask
        } else if options.blend_mode != cs2_renderer::BlendMode::SourceOver {
            GpuDecision::DrawBlend
        } else {
            GpuDecision::DrawTransform
        };
        let area = options.clip.map_or(u64::from(canvas.width()) * u64::from(canvas.height()),
            |clip| u64::from(clip.width) * u64::from(clip.height));
        list_gpu_decision(reason, area);
        canvas.draw_frame(frame, options)?;
        return Ok(());
    };
    let opacity = native_layer_opacity(&entry);
    if opacity == 0 {
        return Ok(());
    }
    verify_distinct_layer_source(host, list_id, &entry)?;
    if entry.transform != 0 {
        return draw_affine(canvas, frame, source_geometry(host, &entry, frame)?,
            &entry, opacity, options.clip);
    }
    draw_plain_frame(canvas, frame, options, opacity, opaque)
}

fn native_layer_entry<'a>(host: &'a LoggingHost<'_>, list_id: u32, key: &(u32, String)) -> Option<std::cell::Ref<'a, Entry>> {
    if key.0 != u32::MAX { return None; }
    let number = u32::from_str_radix(key.1.strip_prefix("\0list-native:")?, 16).ok()?;
    host.data_services.list_native.entries.get(&(list_id, number))
}

fn native_layer_opacity(entry: &Entry) -> u32 {
    if entry.animation.timeline().is_none() { return entry.raw_opacity; }
    let product = entry.raw_opacity.wrapping_mul(entry.animation.display().blend as u32);
    if product < 0xff00 { product / 255 } else { 255 }
}

fn verify_distinct_layer_source(host: &LoggingHost<'_>, list_id: u32, entry: &Entry) -> Result<()> {
    let output = host
        .data_services
        .tasks
        .fes
        .platform
        .lists
        .get(&list_id)
        .context("list draw target disappeared")?
        .output;
    if host
        .data_services
        .image_wrapper_pixels
        .get(&output)
        .copied()
        == Some(current_pixels(host, entry)?)
    {
        bail!("an image list cannot draw its own output pixels as a source");
    }
    Ok(())
}

fn plain_list_options(options: &ImageDrawOptions) -> bool {
    options.scale_x == 1.
        && options.scale_y == 1.
        && options.rotation_radians == 0.
        && options.pivot == [0.; 2]
        && !options.flip_x
        && !options.flip_y
        && options.mask.is_none()
        && options.blend_mode == cs2_renderer::BlendMode::SourceOver
}

fn opaque_list_source(source: &RgbaFrame) -> bool {
    match source.metadata().container {
        cs2_renderer::HgContainer::Hg3V300 => source.metadata().alpha_flag == 0,
        _ => !source.has_alpha(),
    }
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
pub(super) fn append_layer_jobs<'a>(
    host: &LoggingHost<'_>, list_id: u32, key: &(u32, String), layer: &'a Layer, clips: &[Rect],
    draws: &mut Vec<super::gpu_raster::RasterJob<'a>>,
) -> bool {
    if clips.is_empty() || !layer.options.visible || layer.frame.is_empty() { return true; }
    let opacity = if let Some(entry) = native_layer_entry(host, list_id, key) {
        let opacity = native_layer_opacity(&entry);
        if opacity != 0 && (entry.transform != 0 || verify_distinct_layer_source(host, list_id, &entry).is_err()) {
            return false;
        }
        opacity
    } else {
        u32::from(layer.options.opacity)
    };
    append_plain_jobs(layer, clips, opacity, draws)
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
fn append_plain_jobs<'a>(layer: &'a Layer, clips: &[Rect], opacity: u32,
    draws: &mut Vec<super::gpu_raster::RasterJob<'a>>,
) -> bool {
    use super::gpu_raster::{Draw, RasterJob, RasterRect};
    let options = &layer.options;
    let source = &layer.frame;
    if !options.visible || opacity == 0 || source.is_empty() { return true; }
    if !plain_list_options(options) || layer.affine.is_some() { return false; }
    let copy = opacity == 255 && opaque_list_source(source);
    let left = i64::from(options.position.x);
    let top = i64::from(options.position.y);
    let right = left + i64::from(source.width());
    let bottom = top + i64::from(source.height());
    for clip in clips {
        let clip_left = i64::from(clip.x);
        let clip_right = clip_left + i64::from(clip.width);
        let draw_left = left.max(clip_left);
        let draw_right = right.min(clip_right);
        let draw_top = top.max(i64::from(clip.y));
        let draw_bottom = bottom.min(i64::from(clip.y) + i64::from(clip.height));
        if draw_left >= draw_right || draw_top >= draw_bottom {
            continue;
        }
        let left_halo = !copy && draw_left > clip_left;
        let right_halo = !copy && draw_right < clip_right;
        let width = (draw_right - draw_left) as u32;
        let rect = RasterRect {
            origin: [(draw_left - i64::from(left_halo)) as u32, draw_top as u32],
            size: [width + u32::from(left_halo) + u32::from(right_halo), (draw_bottom - draw_top) as u32],
        };
        let source_origin = [(draw_left - left) as u32, (draw_top - top) as u32];
        let operation = if copy { Draw::Copy { source_origin } }
            else { Draw::ListSpan { source_origin, width, left_halo, opacity } };
        draws.push(RasterJob { rect, source: Some(source), atlas: None, operation });
        if draws.len() > 1024 { return false; }
    }
    true
}

fn draw_plain_frame(
    canvas: &mut Canvas,
    source: &RgbaFrame,
    options: &ImageDrawOptions,
    opacity: u32,
    opaque: &mut Option<bool>,
) -> Result<()> {
    if !options.visible || opacity == 0 {
        return Ok(());
    }
    use super::perf_detail::{GpuDecision, list_gpu_decision};
    if options.blend_mode.is_list_alpha_mask() {
        let area = options.clip.map_or(u64::from(canvas.width()) * u64::from(canvas.height()),
            |clip| u64::from(clip.width) * u64::from(clip.height));
        list_gpu_decision(GpuDecision::AlphaMask, area);
        canvas.draw_frame(source, options)?;
        return Ok(());
    }
    let (mut clip_left, mut clip_top) = (0, 0);
    let (mut clip_right, mut clip_bottom) = (i64::from(canvas.width()), i64::from(canvas.height()));
    if let Some(clip) = options.clip {
        clip_left = clip_left.max(i64::from(clip.x));
        clip_top = clip_top.max(i64::from(clip.y));
        clip_right = clip_right.min(i64::from(clip.x) + i64::from(clip.width));
        clip_bottom = clip_bottom.min(i64::from(clip.y) + i64::from(clip.height));
    }
    let left = i64::from(options.position.x);
    let top = i64::from(options.position.y);
    let draw_left = clip_left.max(left);
    let draw_top = clip_top.max(top);
    let draw_right = clip_right.min(left + i64::from(source.width()));
    let draw_bottom = clip_bottom.min(top + i64::from(source.height()));
    if draw_left >= draw_right || draw_top >= draw_bottom {
        return Ok(());
    }

    let copy = opacity == 255 && *opaque.get_or_insert_with(|| opaque_list_source(source));
    if copy {
        list_gpu_decision(GpuDecision::OpaqueCopy, ((draw_right - draw_left) * (draw_bottom - draw_top)) as u64);
    }
    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    if !copy {
        use super::gpu_raster::{Draw, RasterRect, draw};
        let left_halo = draw_left > clip_left;
        let right_halo = draw_right < clip_right;
        if draw(canvas, Some(source), RasterRect {
            origin: [(draw_left - i64::from(left_halo)) as u32, draw_top as u32],
            size: [(draw_right - draw_left) as u32 + u32::from(left_halo) + u32::from(right_halo),
                (draw_bottom - draw_top) as u32],
        }, Draw::ListSpan {
            source_origin: [(draw_left - left) as u32, (draw_top - top) as u32],
            width: (draw_right - draw_left) as u32,
            left_halo,
            opacity,
        }) {
            return Ok(());
        }
    }
    let source_stride = source.width() as usize * 4;
    let target_stride = canvas.width() as usize * 4;
    let target_x = draw_left as usize * 4;
    let row_bytes = (draw_right - draw_left) as usize * 4;
    let clip_start = clip_left as usize * 4;
    let clip_end = clip_right as usize * 4;
    for row in draw_top..draw_bottom {
        let source_at = (row - top) as usize * source_stride + (draw_left - left) as usize * 4;
        let src = &source.pixels()[source_at..source_at + row_bytes];
        let target_at = row as usize * target_stride;
        let dst = &mut canvas.pixels_mut()[target_at..target_at + target_stride];
        if copy {
            dst[target_x..target_x + row_bytes].copy_from_slice(src);
            continue;
        }
        for (column, pixel) in src.chunks_exact(4).enumerate() {
            let at = target_x + column * 4;
            blend_list_pixel(&mut dst[at..at + 4], [pixel[0], pixel[1], pixel[2], pixel[3]], opacity);
            if at > clip_start && dst[at - 4..at] == [0; 4] {
                dst[at - 4..at - 1].copy_from_slice(&pixel[..3]);
            }
            let right = at + 4;
            if right < clip_end && dst[right..right + 4] == [0; 4] {
                dst[right..right + 3].copy_from_slice(&pixel[..3]);
            }
        }
    }
    Ok(())
}

fn blend_pixel(canvas: &mut Canvas, x: i32, y: i32, source: [u8; 4], opacity: u32) -> Result<()> {
    let dst = canvas.pixel(x, y)?;
    let mut out = [dst.r, dst.g, dst.b, dst.a];
    blend_list_pixel(&mut out, source, opacity);
    canvas.set_pixel(x, y, cs2_renderer::Color::rgba(out[0], out[1], out[2], out[3]))?;
    Ok(())
}

const LIST_ALPHA_RECIPROCAL: [u32; 256] = {
    let mut table = [0; 256];
    let mut alpha = 1;
    while alpha < table.len() {
        table[alpha] = 65536 / alpha as u32;
        alpha += 1;
    }
    table
};

pub(super) fn blend_list_pixel(out: &mut [u8], source: [u8; 4], opacity: u32) {
    let alpha = (((opacity & 0xffff) * (u32::from(source[3]) + 1)) >> 8) & 255;
    if alpha == 255 || out[3] == 0 {
        out[..3].copy_from_slice(&source[..3]);
        out[3] = alpha as u8;
    } else if alpha != 0 && out[3] == 255 {
        for axis in 0..3 {
            out[axis] = (i32::from(out[axis])
                + ((i32::from(source[axis]) - i32::from(out[axis])) * alpha as i32 >> 8))
                as u8;
        }
    } else if alpha != 0 {
        let destination = u32::from(out[3]);
        let overlap = (alpha * destination) >> 8;
        let dw = ((256 - alpha) * destination) >> 8;
        let sw = (((256 - destination) * alpha) >> 8) + overlap;
        let oa = (sw + dw) | 1;
        let reciprocal = LIST_ALPHA_RECIPROCAL[oa as usize];
        for axis in 0..3 {
            out[axis] = ((u32::from(source[axis]) * sw + u32::from(out[axis]) * dw) * reciprocal
                >> 16) as u8;
        }
        out[3] = oa as u8;
    }
}

/// The software list path treats every nonzero category as the same affine
/// operation. Coordinates, products and interpolation retain fifteen-bit
/// fractions and DWORD wrap at the same conversion boundaries.
#[derive(Clone, Copy)]
struct Affine15 {
    xx: i32,
    xy: i32,
    yx: i32,
    yy: i32,
}

impl Affine15 {
    fn new(rotation: f32, scale: [f32; 2]) -> Self {
        let angle = truncate((rotation / std::f32::consts::TAU) * 32768.) as u16 as i32;
        let sine = sine15(angle);
        let cosine = sine15(8192_i32.wrapping_sub(angle));
        let sx = fixed15(scale[0]);
        let sy = fixed15(scale[1]);
        Self {
            xx: multiply15(cosine, sx),
            xy: multiply15(sine, sx),
            yx: multiply15(sine.wrapping_neg(), sy),
            yy: multiply15(cosine, sy),
        }
    }

    fn transform(self, point: [i32; 2]) -> [i32; 2] {
        [
            multiply15(self.xx, point[0]).wrapping_add(multiply15(self.xy, point[1])),
            multiply15(self.yx, point[0]).wrapping_add(multiply15(self.yy, point[1])),
        ]
    }

    fn inverse(self) -> Self {
        let determinant = (i64::from(self.xx) * i64::from(self.yy))
            .wrapping_sub(i64::from(self.yx) * i64::from(self.xy))
            >> 15;
        if determinant == 0 {
            // The original zero-determinant branch returns the unchanged
            // matrix; it neither substitutes identity nor divides by zero.
            return self;
        }
        let reciprocal = ((1i64 << 30) / determinant) as i32;
        Self {
            xx: multiply15(self.yy, reciprocal),
            xy: multiply15(self.xy.wrapping_neg(), reciprocal),
            yx: multiply15(self.yx.wrapping_neg(), reciprocal),
            yy: multiply15(self.xx, reciprocal),
        }
    }
}

fn multiply15(left: i32, right: i32) -> i32 {
    ((i64::from(left) * i64::from(right)) >> 15) as i32
}

fn fixed15(value: f32) -> i32 {
    let floor = value.floor();
    truncate(floor).wrapping_shl(15) | truncate((value - floor) * 32768.)
}

fn sine15(angle: i32) -> i32 {
    let index = angle.rem_euclid(32768);
    ((f64::from(index) * 0.000_191_747_598_485_705_15).sin() * 32768.).trunc() as i32
}

struct AffineRaster {
    base: [f32; 2],
    forward: Affine15,
    lower: [i64; 2],
    upper: [i64; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct AffinePlacement {
    pub(super) position: [f32; 2],
    pub(super) base: [f32; 2],
    pub(super) offset: [i32; 2],
    pub(super) scale: [f32; 2],
    pub(super) rotation: f32,
}

impl AffinePlacement {
    fn from_entry(entry: &Entry, base: [f32; 2], offset: [i32; 2]) -> Self {
        Self { position: entry.position, base, offset, scale: entry.scale, rotation: entry.rotation }
    }

    pub(super) fn validate(self) -> Result<()> {
        if self.position.into_iter().chain(self.base).chain(self.scale)
            .chain([self.rotation]).any(|value| !value.is_finite()) {
            bail!("list affine properties contain a nonfinite value");
        }
        Ok(())
    }
}

fn affine_raster(size: [u32; 2], placement: AffinePlacement) -> Result<AffineRaster> {
    placement.validate()?;
    let AffinePlacement { position, base, offset, scale, rotation } = placement;
    // This path converts the pixel offset as an unsigned DWORD to float.
    let base = [
        base[0] - (offset[0] as u32) as f32,
        base[1] - (offset[1] as u32) as f32,
    ];
    let forward = Affine15::new(rotation, scale);
    let start = base.map(|value| fixed15(-value));
    let end = [
        start[0].wrapping_add((size[0] as i32).wrapping_shl(15)),
        start[1].wrapping_add((size[1] as i32).wrapping_shl(15)),
    ];
    let vertices = [
        forward.transform(start),
        forward.transform([end[0], start[1]]),
        forward.transform(end),
        forward.transform([start[0], end[1]]),
    ];
    let position = position.map(truncate);
    let mut lower = [0i64; 2];
    let mut upper = [0i64; 2];
    for axis in 0..2 {
        let min = vertices.iter().map(|vertex| vertex[axis]).min().unwrap();
        let max = vertices[1..]
            .iter()
            .map(|vertex| vertex[axis])
            .fold(vertices[0][axis].wrapping_add(1), i32::max);
        let integer_max = vertices[1..]
            .iter()
            .map(|vertex| vertex[axis] >> 15)
            .fold((vertices[0][axis] >> 15).wrapping_add(1), i32::max);
        lower[axis] = i64::from(
            (min >> 15)
                .wrapping_add(position[axis])
                .wrapping_sub(i32::from(min & 0x7fff != 0)),
        );
        upper[axis] = i64::from(
            integer_max
                .wrapping_add(position[axis])
                .wrapping_add(i32::from(max & 0x7fff != 0)),
        );
    }
    Ok(AffineRaster {
        base,
        forward,
        lower,
        upper,
    })
}

// A fourth-source entry reads 69b320/69b2e0/69b2a0, not the current
// pixel surface metadata (718..720 and 726 can deliberately separate them).
fn source_geometry(host: &LoggingHost<'_>, entry: &Entry, frame: &RgbaFrame)
    -> Result<crate::image_manager::ResourceGeometry> {
    if entry.selected_frame.is_none() && let Source::Resource { handle, .. } = entry.source {
        return Ok(host.image_resources.get(&handle)
            .context("list geometry resource was released")?.geometry);
    }
    Ok(crate::image_manager::ResourceGeometry::from_frame(frame))
}

pub(super) fn entry_top_left(position: [f32; 2], base: [f32; 2], offset: [i32; 2]) -> [i32; 2] {
    std::array::from_fn(|axis| {
        truncate(position[axis])
            .wrapping_sub(truncate(base[axis]))
            .wrapping_add(offset[axis])
    })
}

fn entry_bounds(
    source: crate::image_manager::ResourceGeometry,
    entry: &Entry,
    base: [f32; 2],
    size: [u32; 2],
    clip: Option<Rect>,
) -> Result<Option<Rect>> {
    if source.size.contains(&0) {
        return Ok(None);
    }
    let (lower, upper) = if entry.transform != 0 {
        return affine_bounds(source.size, AffinePlacement::from_entry(entry, base, source.offset), size, clip);
    } else {
        let at = entry_top_left(entry.position, base, source.offset);
        (
            at.map(i64::from),
            [
                i64::from(at[0]) + i64::from(source.size[0]),
                i64::from(at[1]) + i64::from(source.size[1]),
            ],
        )
    };
    Ok(clipped_bounds(lower, upper, size, clip))
}

pub(super) fn affine_bounds(
    source_size: [u32; 2], placement: AffinePlacement, size: [u32; 2], clip: Option<Rect>,
) -> Result<Option<Rect>> {
    let raster = affine_raster(source_size, placement)?;
    Ok(clipped_bounds(raster.lower, raster.upper, size, clip))
}

fn clipped_bounds(mut lower: [i64; 2], mut upper: [i64; 2], size: [u32; 2], clip: Option<Rect>) -> Option<Rect> {
    for axis in 0..2 {
        lower[axis] = lower[axis].max(0);
        upper[axis] = upper[axis].min(i64::from(size[axis]));
    }
    if let Some(clip) = clip {
        lower[0] = lower[0].max(i64::from(clip.x));
        lower[1] = lower[1].max(i64::from(clip.y));
        upper[0] = upper[0].min(i64::from(clip.x) + i64::from(clip.width));
        upper[1] = upper[1].min(i64::from(clip.y) + i64::from(clip.height));
    }
    (lower[0] < upper[0] && lower[1] < upper[1]).then(|| {
        Rect::new(
            lower[0] as i32,
            lower[1] as i32,
            (upper[0] - lower[0]) as u32,
            (upper[1] - lower[1]) as u32,
        )
    })
}

fn draw_affine(
    canvas: &mut Canvas,
    source: &RgbaFrame,
    geometry: crate::image_manager::ResourceGeometry,
    entry: &Entry,
    opacity: u32,
    clip: Option<Rect>,
) -> Result<()> {
    draw_affine_placement(canvas, source, geometry.size,
        AffinePlacement::from_entry(entry, entry.base, geometry.offset), opacity, clip,
        cs2_renderer::BlendMode::SourceOver)
}

fn draw_affine_placement(
    canvas: &mut Canvas,
    source: &RgbaFrame,
    size: [u32; 2],
    placement: AffinePlacement,
    opacity: u32,
    clip: Option<Rect>,
    blend: cs2_renderer::BlendMode,
) -> Result<()> {
    let AffineRaster {
        base,
        forward,
        mut lower,
        mut upper,
    } = affine_raster(size, placement)?;
    lower[0] = lower[0].max(0);
    lower[1] = lower[1].max(0);
    upper[0] = upper[0].min(i64::from(canvas.width()));
    upper[1] = upper[1].min(i64::from(canvas.height()));
    if let Some(clip) = clip {
        lower[0] = lower[0].max(i64::from(clip.x));
        lower[1] = lower[1].max(i64::from(clip.y));
        upper[0] = upper[0].min(i64::from(clip.x) + i64::from(clip.width));
        upper[1] = upper[1].min(i64::from(clip.y) + i64::from(clip.height));
    }
    let inverse = forward.inverse();
    let origin = placement.position.map(fixed15);
    let pivot = forward
        .transform(base.map(fixed15))
        .map(|value| value >> 15);
    let integer_base = base.map(truncate);
    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    if lower[0] < upper[0] && lower[1] < upper[1] {
        use super::gpu_raster::{Draw, RasterRect, draw};
        if draw(canvas, Some(source), RasterRect {
            origin: lower.map(|value| value as u32),
            size: std::array::from_fn(|axis| (upper[axis] - lower[axis]) as u32),
        }, Draw::ListAffine {
            inverse: [inverse.xx, inverse.xy, inverse.yx, inverse.yy],
            offset: std::array::from_fn(|axis| pivot[axis].wrapping_sub(origin[axis])),
            base: integer_base,
            opacity,
            blend,
        }) {
            return Ok(());
        }
    }
    for y in lower[1]..upper[1] {
        for x in lower[0]..upper[0] {
            let relative = [
                (x as i32)
                    .wrapping_shl(15)
                    .wrapping_sub(origin[0])
                    .wrapping_add(pivot[0]),
                (y as i32)
                    .wrapping_shl(15)
                    .wrapping_sub(origin[1])
                    .wrapping_add(pivot[1]),
            ];
            let point = inverse.transform(relative);
            let integer = [
                (point[0] >> 15).wrapping_add(integer_base[0]),
                (point[1] >> 15).wrapping_add(integer_base[1]),
            ];
            if let Some(sample) = interpolate15(source, integer, point.map(|value| value & 0x7fff))
            {
                if blend.is_list_alpha_mask() {
                    let [r, g, b, a] = sample;
                    canvas.blend_pixel_with_mode(x as i32, y as i32,
                        cs2_renderer::Color::rgba(r, g, b, a), blend)?;
                } else {
                    blend_pixel(canvas, x as i32, y as i32, sample, opacity)?;
                }
            }
        }
    }
    Ok(())
}

fn interpolate15(source: &RgbaFrame, at: [i32; 2], fraction: [i32; 2]) -> Option<[u8; 4]> {
    let [x, y] = at;
    if x < 0 || y < 0 || x as u32 >= source.width() || y as u32 >= source.height() {
        return None;
    }
    let pixel = |x: u32, y: u32| -> [u8; 4] {
        let index = (y as usize * source.width() as usize + x as usize) * 4;
        source.pixels()[index..index + 4].try_into().unwrap()
    };
    let (x, y) = (x as u32, y as u32);
    let first = pixel(x, y);
    let neighbors = [
        first,
        if x + 1 < source.width() {
            pixel(x + 1, y)
        } else {
            first
        },
        if y + 1 < source.height() {
            pixel(x, y + 1)
        } else {
            first
        },
        if x + 1 < source.width() && y + 1 < source.height() {
            pixel(x + 1, y + 1)
        } else {
            first
        },
    ];
    if neighbors.iter().all(|pixel| pixel[3] == 0) {
        return None;
    }
    let [x, y] = fraction;
    let weights = [
        multiply15(32768 - x, 32768 - y),
        multiply15(x, 32768 - y),
        multiply15(32768 - x, y),
        multiply15(x, y),
    ];
    Some(std::array::from_fn(|axis| {
        (neighbors
            .iter()
            .zip(weights)
            .map(|(pixel, weight)| i32::from(pixel[axis]) * weight)
            .sum::<i32>()
            >> 15) as u8
    }))
}

fn blank(size: [u32; 2]) -> Result<RgbaFrame> {
    let length = (size[0] as usize)
        .checked_mul(size[1] as usize)
        .and_then(|n| n.checked_mul(4))
        .filter(|&n| n <= 128 * 1024 * 1024)
        .context("image-list output exceeds 128 MiB")?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(length)?;
    pixels.resize(length, 0);
    Ok(RgbaFrame::from_native_rgba8(size[0], size[1], pixels,
        cs2_renderer::FrameMetadata::raster(size))?)
}

fn float(args: &[u8], offset: usize) -> Result<f32> {
    Ok(f32::from_bits(read_u32(args, offset)?))
}

fn truncate(value: f32) -> i32 {
    if !value.is_finite() || value < -2_147_483_648.0 || value >= 2_147_483_648.0 {
        i32::MIN
    } else {
        value.trunc() as i32
    }
}

#[cfg(test)]
mod raster_tests {
    use super::*;
    use cs2_renderer::Color;

    #[test]
    fn native_entries_draw_at_the_top_of_a_taller_atlas() {
        let source = RgbaFrame::from_rgba8(1, 2, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        let geometry = crate::image_manager::ResourceGeometry::from_frame(&source);
        let mut entry = Entry::empty(Source::Frame);
        entry.position = [2., 1.];
        let clip = Rect::new(2, 1, 1, 2);
        for transform in [0, 1] {
            entry.transform = transform;
            let bounds = entry_bounds(geometry, &entry, entry.base, [6, 8], Some(clip)).unwrap();
            assert_eq!(bounds, Some(clip));
            let mut target = Canvas::new(6, 8).unwrap();
            if transform == 0 {
                let [left, top] = entry_top_left(entry.position, entry.base, geometry.offset);
                let mut options = ImageDrawOptions::new(Point::new(left, top));
                options.clip = Some(clip);
                draw_plain_frame(&mut target, &source, &options, 255, &mut None).unwrap();
            } else {
                draw_affine(&mut target, &source, geometry, &entry, 255, Some(clip)).unwrap();
            }
            assert_eq!(target.pixel(2, 1).unwrap(), Color::rgba(255, 0, 0, 255));
            assert_eq!(target.pixel(2, 2).unwrap(), Color::rgba(0, 0, 255, 255));
            assert_eq!(target.pixel(2, 6).unwrap(), Color::TRANSPARENT);
        }
    }

    fn scalar_draw(canvas: &mut Canvas, source: &RgbaFrame, options: &ImageDrawOptions, opacity: u32) {
        let contains = |x: i32, y: i32| {
            options.clip.is_none_or(|clip| {
                x >= clip.x && i64::from(x) < i64::from(clip.x) + i64::from(clip.width)
                    && y >= clip.y && i64::from(y) < i64::from(clip.y) + i64::from(clip.height)
            })
        };
        let opaque = match source.metadata().container {
            cs2_renderer::HgContainer::Hg3V300 => source.metadata().alpha_flag == 0,
            _ => !source.has_alpha(),
        };
        for row in 0..source.height() {
            for column in 0..source.width() {
                let target_x = options.position.x + column as i32;
                let target_y = options.position.y + row as i32;
                if !contains(target_x, target_y) || canvas.pixel(target_x, target_y).is_err() {
                    continue;
                }
                let at = (row * source.width() + column) as usize * 4;
                let pixel: [u8; 4] = source.pixels()[at..at + 4].try_into().unwrap();
                let [red, green, blue, alpha] = pixel;
                if opacity == 255 && opaque {
                    canvas.set_pixel(target_x, target_y, Color::rgba(red, green, blue, alpha)).unwrap();
                    continue;
                }
                blend_pixel(canvas, target_x, target_y, pixel, opacity).unwrap();
                for neighbor in [target_x - 1, target_x + 1] {
                    if contains(neighbor, target_y)
                        && canvas.pixel(neighbor, target_y).ok() == Some(Color::TRANSPARENT)
                    {
                        canvas.set_pixel(neighbor, target_y, Color::rgba(red, green, blue, 0)).unwrap();
                    }
                }
            }
        }
    }

    #[test]
    fn raster_clipped_overlapping_tiles_match_scalar_pixels() {
        let pixels: Vec<_> = (0u8..12)
            .flat_map(|index| [19 + index, 34, 62, [0, 1, 130, 255][index as usize % 4]])
            .collect();
        let opaque_pixels = pixels.chunks_exact(4)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect();
        let translucent = RgbaFrame::from_rgba8(4, 3, pixels.clone()).unwrap();
        let opaque = RgbaFrame::from_rgba8(4, 3, opaque_pixels).unwrap();
        let mut metadata = cs2_renderer::FrameMetadata::raster([4, 3]);
        metadata.container = cs2_renderer::HgContainer::Hg3V300;
        let metadata_opaque = RgbaFrame::from_native_rgba8(4, 3, pixels, metadata).unwrap();

        let mut background = Canvas::new(7, 5).unwrap();
        background.set_pixel(2, 1, Color::rgba(80, 90, 100, 255)).unwrap();
        background.set_pixel(3, 1, Color::rgba(80, 90, 100, 87)).unwrap();
        background.set_pixel(5, 2, Color::rgba(80, 90, 100, 0)).unwrap();
        for source in [&translucent, &opaque, &metadata_opaque] {
            for position in [Point::new(-1, -1), Point::new(1, 1), Point::new(5, 4)] {
                for opacity in [255, 143, 0x1234] {
                    let mut options = ImageDrawOptions::new(position);
                    let mut expected = background.clone();
                    let mut actual = background.clone();
                    let mut cached_opaque = None;
                    for clip in [Rect::new(-1, -1, 3, 4), Rect::new(1, 0, 4, 4), Rect::new(4, 1, 5, 6)] {
                        options.clip = Some(clip);
                        scalar_draw(&mut expected, source, &options, opacity);
                        draw_plain_frame(&mut actual, source, &options, opacity, &mut cached_opaque).unwrap();
                        assert_eq!(actual.pixels(), expected.pixels(), "{position:?} {clip:?} {opacity}");
                    }
                }
            }
        }
    }

    #[test]
    fn plain_list_alpha_mask_preserves_destination_rgb_and_clip() {
        let source = RgbaFrame::from_rgba8(5, 1, vec![
            0, 255, 0, 0, 0, 255, 0, 255, 0, 255, 0, 128, 0, 255, 0, 0, 0, 255, 0, 255,
        ]).unwrap();
        let mut canvas = Canvas::new(5, 1).unwrap();
        for (column, alpha) in [255, 255, 127, 255, 0].into_iter().enumerate() {
            canvas.set_pixel(column as i32, 0, Color::rgba(80, 90, 100, alpha)).unwrap();
        }
        let mut options = ImageDrawOptions::new(Point::new(0, 0));
        options.clip = Some(Rect::new(1, 0, 3, 1));
        options.opacity = 128;
        options.blend_mode = cs2_renderer::BlendMode::ListAlphaFromAlpha;
        draw_plain_frame(&mut canvas, &source, &options, 128, &mut None).unwrap();
        for (pixel, alpha) in canvas.pixels().chunks_exact(4).zip([255, 255, 64, 0, 0]) {
            assert_eq!(pixel, [80, 90, 100, alpha]);
        }
    }

    #[test]
    fn raster_seed_copy_preserves_hidden_rgb_and_clip() {
        let pixels = (0u8..6)
            .flat_map(|index| [index, index + 16, 64, index % 2 * 255])
            .collect();
        let source = RgbaFrame::from_rgba8(3, 2, pixels).unwrap();
        let mut actual = Canvas::new(4, 3).unwrap();
        actual.set_pixel(0, 0, Color::rgba(80, 90, 100, 255)).unwrap();
        let mut expected = actual.clone();
        expected.set_pixel(1, 0, Color::rgba(2, 18, 64, 0)).unwrap();
        expected.set_pixel(1, 1, Color::rgba(5, 21, 64, 255)).unwrap();
        let mut options = ImageDrawOptions::new(Point::new(-1, 0));
        options.clip = Some(Rect::new(1, -1, 2, 3));
        actual.copy_frame(&source, &options).unwrap();
        assert_eq!(actual.pixels(), expected.pixels());
    }
}
