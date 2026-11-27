//! Versioned portable host graph. The CSS record contains an explicitly tagged
//! simulator graph, never an unverified original kind43 payload.
use std::collections::{BTreeMap, BTreeSet};
use serde::{Deserialize, Serialize};
use cs2_script::{VmSnapshotGroup, data_snapshot::ScriptDataObjectsSnapshot,
    shared_numeric_snapshot::SharedNumericStateSnapshot};
use crate::{OrdinaryTaskSnapshot, RuntimeResult, RuntimeError, RuntimeState, TaskClock, TaskMessage};
use crate::language::{LanguageState, LanguageNotificationCache};
use crate::message_object::input_snapshot::InputSamplingStateSnapshot;
use crate::object_state::{ImageExportStateSnapshot, ObjectAllocatorSnapshot};
use crate::scene_tools::SceneMessageMemorySnapshot;

// Desktop additionally proves the actual consumers: 41/42 must be the live
// FES SCROLL2/SLIDER lower tasks, never an unrelated legacy model kind42.
pub const SUPPORTED_TASK_KINDS: &[u32] = &[0, 1, 2, 6, 11, 12, 13, 15, 16, 18, 25, 26, 27, 34, 41, 42];
pub const MAX_GRAPH_BYTES: usize = 64 * 1024 * 1024;
// Version 4 includes standalone data objects, record stacks and parsed XML.
// Old unpublished simulator
// wire layouts are rejected before decoding; native game CSS is unaffected.
const MAGIC: &[u8; 12] = b"CS2HOSTGRPH4";
#[path = "graph_codec.rs"]
mod codec;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExternalBindingSnapshot { Vm(u32), Detached(usize) }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VmOwner { Root, Attached, Ordinary }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskRecordSnapshot {
    pub kind: u32, pub clock: TaskClock, pub deletion: u32,
    pub ordinary: Option<OrdinaryTaskSnapshot>, pub queued: Vec<TaskMessage>,
    pub context: Vec<u8>, pub had_vm: bool, pub property: u32,
    pub external: Option<ExternalBindingSnapshot>, pub language: LanguageNotificationCache,
    pub shared_area: Option<u64>, pub registered: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneNodeSnapshot {
    pub id: u32, pub kind: u32, pub parent: u32, pub name: String,
    pub size_bits: [u32; 2], pub position_bits: [u32; 3], pub scale_bits: [u32; 3],
    pub color: u32, pub overlay_color: u32, pub visible: bool, pub initialized: bool,
    pub fes_block_name: Vec<u8>, pub fes_object_prefix: Vec<u8>, pub fes_bank: u32,
    pub fes_parameters: Vec<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageFrameSnapshot {
    pub size: [u32; 2], pub rgba: Vec<u8>, pub container: u32,
    pub frame_id: Option<u32>, pub bit_depth: u8, pub offset: [i32; 2],
    pub canvas_size: [u32; 2], pub base: [i32; 2], pub alpha_flag: u32,
    pub attributes: BTreeMap<u16, [u32; 5]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageResourceSnapshot {
    pub image: ImageFrameSnapshot, pub owner: Option<u32>, pub references: usize,
    pub has_source_layer: bool, pub updated: bool, pub uses_alpha: bool,
    pub auxiliary: u32, pub attributes: BTreeMap<u32, [u32; 5]>,
    pub source_layers: Option<ImageResourceLayersSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageSourceLayerSnapshot {
    pub frames: BTreeMap<u32, ImageFrameSnapshot>, pub selected: u32,
    pub animation: cs2_script::anm::snapshot::ImageAnimationSnapshot,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageResourceRoundSnapshot {
    pub now: u32, pub cursor: usize, pub restart: bool, pub started: bool, pub changed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageResourceLayersSnapshot {
    pub layers: Vec<ImageSourceLayerSnapshot>, pub bounds: Option<[i32; 4]>,
    pub canvas: [u32; 2], pub base: Option<[i32; 2]>,
    pub pending: Option<ImageResourceRoundSnapshot>, pub changed: bool,
    pub surface: Option<ImageResourceSurfaceSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageResourceRegistrationSnapshot {
    pub position: [i32; 2], pub pivot: [i32; 2], pub size: [u32; 2],
    pub history: Option<[i32; 4]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageResourceSurfaceSnapshot {
    pub size: [u32; 2], pub tile: [u32; 2],
    pub registrations: Vec<ImageResourceRegistrationSnapshot>,
    pub dirty: Vec<Option<[i32; 4]>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectGraphSnapshot {
    pub version: u32, pub pe_sha256: String, pub root: u32, pub next_task_id: u32,
    pub host_continuation: Vec<u8>, pub external_size: usize, pub configured_shared_slots: usize,
    pub records: BTreeMap<u32, TaskRecordSnapshot>, pub reserved: BTreeSet<u32>,
    pub external_tasks: BTreeSet<u32>, pub parents: BTreeMap<u32, u32>,
    pub nodes: BTreeMap<u32, SceneNodeSnapshot>, pub programs: VmSnapshotGroup,
    pub vm_owners: BTreeMap<u32, VmOwner>, pub detached_external: Vec<Vec<u8>>,
    pub factory_external: Option<ExternalBindingSnapshot>,
    pub numeric: SharedNumericStateSnapshot, pub scene_memory: SceneMessageMemorySnapshot,
    pub input: InputSamplingStateSnapshot, pub allocator: ObjectAllocatorSnapshot,
    pub top_clock: [u32; 2], pub state: RuntimeState,
    pub image_pixels: BTreeMap<u32, ImageFrameSnapshot>, pub wrapper_pixels: BTreeMap<u32, u32>,
    pub wrapper_owners: BTreeMap<u32, u32>, pub image_control: BTreeMap<u32, u32>,
    pub image_export: ImageExportStateSnapshot, pub back_buffer: ImageFrameSnapshot,
    pub image_resources: BTreeMap<u32, ImageResourceSnapshot>,
    /// Desktop image-list component: shared entries, frame tables and cursors.
    pub image_lists: Vec<u8>,
    pub message_defaults: Option<crate::message_object::MessageDefaults>,
    pub graphics_canvas: ImageFrameSnapshot,
    pub data: ScriptDataObjectsSnapshot,
    pub backlogs: BTreeMap<u32, crate::backlog::BacklogSnapshot>,
    pub emocon: BTreeMap<u32, crate::emocon::EmoconConfiguration>,
    pub pending_messages: Vec<TaskMessage>, pub pending_fes_events: Vec<(u32, Option<crate::InputEvent>)>,
    pub message_wait_address: Option<u32>, pub current_parameter: u32,
    pub menu_items: Vec<(u32, u32)>, pub debug_mode: u32, pub sound_config_enabled: u32,
    pub auto_select: u32, pub system_config: [Vec<u8>; 4], pub encoding: u32,
    pub random_integer: Vec<u32>, pub random_float: Vec<u32>, pub random_indices: [usize; 2],
    pub languages: LanguageState, pub language_dirty: u32,
}

/// Component wire shares the enclosing graph's version and resource bounds.
pub fn encode_component<T: Serialize>(value: &T) -> RuntimeResult<Vec<u8>> {
    codec::encode(value).map_err(|error| invalid(&error.to_string()))
}
pub fn decode_component<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> RuntimeResult<T> {
    codec::decode(bytes).map_err(|error| invalid(&error.to_string()))
}

impl ObjectGraphSnapshot {
    pub fn validate_structure(&self) -> RuntimeResult<()> {
        if self.version != 4 || self.records.len() > 1_000_000 || self.reserved.len() > 1_000_000
            || self.parents.len() > 1_000_000 || self.nodes.len() > 1_000_000
            || !self.records.contains_key(&self.root)
            || self.vm_owners.keys().ne(self.programs.programs.keys())
            || self.numeric.task_bindings.keys().ne(self.records.keys())
            || self.random_integer.len() != 624 || self.random_float.len() != 624
            || self.random_indices.iter().any(|&index| index > 624) || self.encoding > 2
            || self.pending_messages.len() > 1_000_000 || self.pending_fes_events.len() > 1_000_000
            || self.detached_external.len() > 1_000_000 || self.emocon.len() > 65_536
            || self.image_pixels.len() > 1_000_000 || self.image_resources.len() > 1_000_000
            || self.host_continuation.len() > MAX_GRAPH_BYTES
            || self.image_lists.len() > MAX_GRAPH_BYTES
        { return Err(invalid("host graph has invalid version, identity sets or fixed-size state")); }
        for (&id, record) in &self.records {
            if !SUPPORTED_TASK_KINDS.contains(&record.kind) || record.queued.len() > 1_000_000
                || record.context.len() > 31 || record.context.contains(&0)
                || record.ordinary.is_some() != matches!(record.kind, 0 | 12)
            { return Err(invalid("host graph has an unsupported task family or invalid task record")); }
            if let Some(ordinary) = &record.ordinary {
                ordinary.validate()?;
                if ordinary.task_id as u32 != id || ordinary.has_vm != (self.vm_owners.get(&id) == Some(&VmOwner::Ordinary)) {
                    return Err(invalid("ordinary VM owner differs from its task record"));
                }
            }
            if record.shared_area != self.numeric.task_bindings[&id] {
                return Err(invalid("task numeric binding differs from the shared group"));
            }
        }
        let root_vms = self.vm_owners.iter().filter(|(_, owner)| **owner == VmOwner::Root).collect::<Vec<_>>();
        if root_vms.len() > 1 || root_vms.first().is_some_and(|(id, _)| **id != self.root)
            || self.vm_owners.keys().any(|id| !self.records.contains_key(id))
            || self.external_tasks.iter().any(|id| !self.records.contains_key(id))
        { return Err(invalid("host graph VM root/attached ownership is invalid")); }
        for (&id, node) in &self.nodes {
            if node.id != id || self.parents.get(&id) != Some(&node.parent)
                || self.records.get(&id).is_none_or(|record| record.kind != node.kind)
                || node.name.len() > 65536 || node.fes_parameters.len() != 512
            { return Err(invalid("host graph scene metadata differs from its task tree")); }
        }
        for (&id, &parent) in &self.parents {
            if !self.records.contains_key(&id) || (parent != self.root && !self.records.contains_key(&parent)) {
                return Err(invalid("host graph task parent is absent"));
            }
            let mut path = BTreeSet::new();
            let mut cursor = id;
            while cursor != self.root {
                if !path.insert(cursor) || path.len() > 256 {
                    return Err(invalid("host graph task hierarchy is cyclic or too deep"));
                }
                cursor = *self.parents.get(&cursor).ok_or_else(|| invalid("host graph has a disconnected task"))?;
            }
        }
        if self.records.keys().any(|id| *id != self.root && !self.parents.contains_key(id)) {
            return Err(invalid("host graph has a task outside the restored hierarchy"));
        }
        let mut external_bytes = 0usize;
        for bytes in &self.detached_external {
            external_bytes = external_bytes.checked_add(bytes.len()).filter(|&total| total <= MAX_GRAPH_BYTES)
                .ok_or_else(|| invalid("detached external group exceeds 64 MiB"))?;
        }
        self.numeric.validate()?;
        self.scene_memory.validate()?;
        self.input.validate()?;
        if self.input.host_input != self.state.input {
            return Err(invalid("host graph has conflicting input state copies"));
        }
        self.data.validate()?;
        if self.input.frame != self.state.clock.frame { return Err(invalid("host graph input and clock have different frames")); }
        for (&owner, configuration) in &self.emocon {
            if !self.records.contains_key(&owner) { return Err(invalid("emocon owner is absent from host graph")); }
            configuration.validate()?;
        }
        Ok(())
    }

    pub fn encode(&self) -> RuntimeResult<Vec<u8>> {
        self.validate_structure()?;
        let body = codec::encode(self).map_err(|error| RuntimeError::InvalidSave(error.to_string()))?;
        if body.len() > MAX_GRAPH_BYTES - MAGIC.len() { return Err(invalid("host graph exceeds 64 MiB")); }
        let mut output = MAGIC.to_vec();
        output.extend_from_slice(&body);
        Ok(output)
    }

    pub fn decode(bytes: &[u8]) -> RuntimeResult<Self> {
        if bytes.len() > MAX_GRAPH_BYTES || !bytes.starts_with(MAGIC) {
            return Err(invalid("CSS record is not a supported versioned host graph"));
        }
        let saved: Self = codec::decode(&bytes[MAGIC.len()..])
            .map_err(|error| RuntimeError::InvalidSave(error.to_string()))?;
        saved.validate_structure()?;
        Ok(saved)
    }
}

fn invalid(message: &str) -> RuntimeError { RuntimeError::InvalidSave(message.into()) }
