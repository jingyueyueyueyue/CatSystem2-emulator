use std::collections::BTreeMap;
use std::path::{Component, Path};

use crate::native_profile::ConstantNativeReturn;
use crate::native_task_clock_profile::supports_single_step;
use crate::pe_resources::{Section, archive_name_seed_from_pe, parse_sections, x86_image_layout};
use crate::resource_router::ResourceMode;
use crate::{GameScan, ResourceLocation};

#[path = "native_achievements.rs"]
mod achievements;
#[path = "native_configuration.rs"]
mod configuration;
#[path = "native_css_restore.rs"]
mod css_restore;
#[path = "native_factory.rs"]
mod factory;
#[path = "native_context_profile.rs"]
mod vm_context;
#[path = "native_hg_window.rs"]
mod hg_window;
#[path = "native_image_fill.rs"]
mod image_fill;
#[path = "native_image_records.rs"]
mod image_records;
#[path = "native_javascript_save.rs"]
mod javascript_save;
#[path = "native_language.rs"]
mod language;
#[path = "native_legacy_layout.rs"]
mod legacy_layout;
#[path = "native_message_list.rs"]
mod message_list;
#[path = "native_scene_tools.rs"]
mod scene_tools;
pub use scene_tools::SceneToolCommands;
#[path = "native_particle.rs"]
mod particle;
#[path = "native_raster.rs"]
mod raster;
#[path = "native_root_clock.rs"]
mod root_clock;
#[path = "native_root_sound.rs"]
mod root_sound;
#[path = "native_state_tree.rs"]
mod state_tree;
#[path = "native_shell_tasks.rs"]
mod shell_tasks;
#[path = "native_string_plane.rs"]
mod string_plane;
pub use css_restore::CssRestoreCommand;
pub use factory::NullTaskFactory;
pub use hg_window::HgWindowBlend;
pub use image_records::{ImageProducer703, ImageRecordLayers, ImageRecordLayout};
pub use particle::ParticleMessages;
pub use raster::RasterMessages;
pub use state_tree::StateTreeCommands;

// mov eax,[esp+4]; mov [ecx+0x38],eax; mov eax,1; ret 8
const VM_TABLE_SETTER: &[u8] = b"\x8b\x44\x24\x04\x89\x41\x38\xb8\x01\x00\x00\x00\xc2\x08\x00";
const REGISTRATION_LEN: usize = 12;
const INLINE_REGISTRATION_LEN: usize = 23;
const MIN_ORDINARY_ENTRIES: usize = 501;
const LAST_TRACKED_SLOT: usize = 1074;
const STRING_SPLIT_WRAPPER: &[u8] =
    b"\x53\x8b\x5c\x24\x10\x55\x56\x57\xff\x33\x8b\x7c\x24\x18\x8b\xcf\xe8????\
\xff\x73\x04\x8b\xcf\x8b\xe8\xe8????\xff\x73\x08\x8b\xcf\x8b\xf0\xe8????\x50\x56\x55\xe8????\
\x83\xc4\x0c\xb8\x02\x00\x00\x00\x5f\x5e\x5d\x5b\xc2\x10\x00";
const BYTE_STRING_SPLITTER: &[u8] =
    b"\x53\x8b\x5c\x24\x08\x85\xdb\x75\x04\x33\xc0\x5b\xc3\
\x56\x8b\x74\x24\x10\x85\xf6\x75\x05\x5e\x33\xc0\x5b\xc3\
\x57\x8b\x7c\x24\x18\x85\xff\x75\x06\x5f\x5e\x33\xc0\x5b\xc3\
\x33\xd2\x55\x8d\x6a\x01\x38\x16\x74\x41\x8a\x07\x33\xc9\x84\xc0\x74\x15\x8a\x1e\
\x8b\xff\x3a\xd8\x74\x09\x8a\x44\x39\x01\x41\x84\xc0\x75\xf3\x8b\x5c\x24\x14\
\x80\x3c\x39\x00\x75\x0a\x8a\x06\x88\x04\x1a\x42\x33\xed\xeb\x0e\x85\xed\x75\x0a\
\xc6\x04\x1a\x00\xbd\x01\x00\x00\x00\x42\x46\x80\x3e\x00\x75\xbf\x5d\x5f\x5e\
\x66\xc7\x04\x1a\x00\x00\xb8\x01\x00\x00\x00\x5b\xc3";
// Ordinary wrapper ABI: call the dispatch vtable entry at +0x0c with the
// sender at context+4, target at args[0], and data [0, args[1], 0].
// The command immediate lies between the prefix and the fixed status-2 tail.
const IMMEDIATE_TASK_MESSAGE_PREFIX: &[u8] =
    b"\x8b\x4c\x24\x08\x8b\x11\x8b\x52\x0c\x56\x8d\x44\x24\x0c\x50\
\x8b\x44\x24\x14\x8b\x70\x04\x8b\x00\x6a\x00\x56\x6a\x00\x68";
const IMMEDIATE_TASK_MESSAGE_TAIL: &[u8] =
    b"\x50\x8b\x41\x04\x50\xff\xd2\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00";
const SINGLE_TARGET_MESSAGE_PREFIX: &[u8] =
    b"\x8b\x4c\x24\x08\x8b\x01\x8b\x40\x0c\x8d\x54\x24\x08\x52\x8b\x54\x24\x10\x8b\x12\
\x6a\x00\x6a\x00\x6a\x00\x68";
const SINGLE_TARGET_MESSAGE_TAIL: &[u8] =
    b"\x52\x8b\x51\x04\x52\xff\xd0\xb8\x02\x00\x00\x00\xc2\x10\x00";
// Slot 661 resolves two VM addresses, then sends message 358 with two
// float3 scratch buffers. The wrapper copies both in order on return.
const CAMERA_DIRECTION_WRAPPER: &[u8] =
    b"\x55\x8b\xec\x83\xe4\xf8\x83\xec\x1c\x53\x8b\x5d\x10\x8b\x43\x04\x56\x8b\x75\x08\x57\x50\x56\xe8????\
\x8b\xf8\x85\xff\x74\x7b\x8b\x4b\x08\x51\x56\xe8????\x8b\xf0\x85\xf6\x74\x6b\
\xd9\xe8\x8b\x4d\x0c\x8b\x11\xd9\x54\x24\x1c\xd9\xee\x8b\x52\x0c\x8d\x44\x24\x0c\xd9\x54\x24\x20\x50\
\xd9\x54\x24\x28\x6a\x00\xd9\x54\x24\x18\x8d\x44\x24\x24\xd9\x5c\x24\x1c\x50\x8d\x44\x24\x1c\x50\
\x8b\x03\xd9\x5c\x24\x28\x68\x66\x01\x00\x00\x50\x8b\x41\x04\x50\xff\xd2\
\xd9\x44\x24\x10\xd9\x1f\xd9\x44\x24\x14\xd9\x5f\x04\xd9\x44\x24\x18\xd9\x5f\x08\
\xd9\x44\x24\x1c\xd9\x1e\xd9\x44\x24\x20\xd9\x5e\x04\xd9\x44\x24\x24\xd9\x5e\x08\
\x5f\x5e\xb8\x02\x00\x00\x00\x5b\x8b\xe5\x5d\xc2\x10\x00";
const CAMERA_VM_ADDRESS_RESOLVER: &[u8] =
    b"\x55\x8b\xec\x83\xe4\xf8\x51\x8b\x55\x0c\x53\x56\x57\x85\xd2\x79\x20";
// Slot 660 formats a node path and sends message 357 with one float3 output.
const MODEL_POSITION_WRAPPER: &[u8] =
    b"\x81\xec\x14\x01\x00\x00\xa1????\x33\xc4\x89\x84\x24\x10\x01\x00\x00\
\x53\x8b\x9c\x24\x20\x01\x00\x00\x56\x8b\xb4\x24\x20\x01\x00\x00\x57\x8b\xbc\x24\x2c\x01\x00\x00\
\x8b\x47\x04\x50\x56\xe8????\x8d\x4c\x24\x1c\x51\x50\x8b\xce\xe8????\x8b\x57\x08\x52\x56\xe8????\
\x8b\xf0\x85\xf6\x74\x48\xd9\xee\x8b\x03\x8d\x4c\x24\x18\xd9\x54\x24\x0c\x51\xd9\x54\x24\x14\
\x6a\x00\xd9\x5c\x24\x1c\x8d\x54\x24\x14\x52\x8b\x17\x8d\x4c\x24\x28\x51\x8b\x4b\x04\
\x68\x65\x01\x00\x00\x52\x8b\x50\x0c\x51\x8b\xcb\xff\xd2\xd9\x44\x24\x0c\xd9\x1e\xd9\x44\x24\x10\
\xd9\x5e\x04\xd9\x44\x24\x14\xd9\x5e\x08\x8b\x8c\x24\x1c\x01\x00\x00\x5f\x5e\x5b\x33\xcc\
\xb8\x02\x00\x00\x00\xe8????\x81\xc4\x14\x01\x00\x00\xc2\x10\x00";
// Slots 671/672 share a wrapper and send 370/369, respectively.
const MODEL_LOCAL_TRANSFORM_QUERY_WRAPPER: &[u8] =
    b"\x55\x8b\xec\x83\xe4\xf8\x81\xec\x4c\x01\x00\x00\xa1????\x33\xc4\x89\x84\x24\
\x48\x01\x00\x00\x53\x8b\x5d\x0c\x33\xc0\x56\x8b\x75\x10\x89\x44\x24\x14\x89\x44\x24\x10\
\x89\x44\x24\x0c\x8b\x46\x08\x57\x8b\x7d\x08\x83\xf8\xff\x74\x0b\x50\x57\xe8????\
\x89\x44\x24\x18\x8b\x46\x0c\x83\xf8\xff\x74\x0b\x50\x57\xe8????\x89\x44\
\x24\x14\x8b\x46\x10\x83\xf8\xff\x74\x0b\x50\x57\xe8????\x89\x44\x24\x10\x8b\
\x46\x04\x50\x57\xe8????\x8d\x4c\x24\x50\x51\x50\x8b\xcf\xe8????\
\x8b\x13\x8b\x52\x0c\x8d\x44\x24\x1c\x50\x6a\x00\x8d\x4c\x24\x28\x51\x8b\x0e\x8d\x44\x24\
\x5c\x50\x8b\x43\x04\x68?\x01\x00\x00\x51\x50\x8b\xcb\xff\xd2\x8b\x44\x24\x18\x85\xc0\
\x74\x14\xd9\x44\x24\x20\xd9\x18\xd9\x44\x24\x24\xd9\x58\x04\xd9\x44\x24\x28\xd9\x58\x08\
\x8b\x44\x24\x14\x85\xc0\x74\x14\xd9\x44\x24\x30\xd9\x18\xd9\x44\x24\x34\xd9\x58\x04\xd9\
\x44\x24\x38\xd9\x58\x08\x8b\x44\x24\x10\x85\xc0\x74\x0b\x8d\x4c\x24\x40\x51\x50\xe8????\
\x8b\x8c\x24\x54\x01\x00\x00\x5f\x5e\x5b\x33\xcc\xb8\x02\x00\x00\x00\xe8????\
\x8b\xe5\x5d\xc2\x10\x00";
const MODEL_CLEAR_OVERRIDES_WRAPPER: &[u8] =
    b"\x56\x8b\x74\x24\x10\x8b\x46\x04\x57\x8b\x7c\x24\x0c\x50\x57\xe8????\
\x68????\x50\x8b\xcf\xe8????\x8b\x4c\x24\x10\x8b\x01\x8b\x40\x0c\x8d\x54\x24\x14\x52\x8b\x16\
\x6a\x00\x6a\x00\x68????\x68\x75\x01\x00\x00\x52\x8b\x51\x04\x52\xff\xd0\x5f\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00";
const MODEL_SET_VISIBILITY_WRAPPER: &[u8] =
    b"\x56\x8b\x74\x24\x10\x8b\x46\x04\x57\x8b\x7c\x24\x0c\x50\x57\xe8????\
\x68????\x50\x8b\xcf\xe8????\x8b\x4c\x24\x10\x8b\x01\x8b\x40\x0c\x8d\x54\x24\x14\x52\
\x8b\x56\x08\x6a\x00\x52\x8b\x16\x68????\x68\x50\x01\x00\x00\x52\x8b\x51\x04\x52\xff\xd0\
\x5f\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00";
const MODEL_GET_VISIBILITY_WRAPPER: &[u8] =
    b"\x56\x8b\x74\x24\x10\x8b\x46\x04\x57\x8b\x7c\x24\x0c\x50\x57\xe8????\
\x68????\x50\x8b\xcf\xe8????\x8b\x4c\x24\x10\x8b\x01\x8b\x40\x0c\x8d\x54\x24\x14\x52\
\x8b\x16\x6a\x00\x6a\x00\x68????\x68\x51\x01\x00\x00\x52\x8b\x51\x04\x52\xff\xd0\
\x8b\x4c\x24\x18\x8b\x54\x24\x14\x5f\x89\x11\xb8\x01\x00\x00\x00\x5e\xc2\x10\x00";
const MODEL_FREEZE_TRANSFORM_WRAPPER: &[u8] =
    b"\x56\x8b\x74\x24\x10\x8b\x46\x04\x57\x8b\x7c\x24\x0c\x50\x57\xe8????\x68????\
\x50\x8b\xcf\xe8????\x83\x7e\x08\x00\x8b\x4c\x24\x10\x74\x2b\x8b\x01\x8b\x40\x0c\
\x8d\x54\x24\x14\x52\x8b\x16\x6a\x00\x6a\x00\x68????\x68\x74\x01\x00\x00\x52\x8b\x51\
\x04\x52\xff\xd0\x5f\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00\x8b\x11\x8b\x52\x0c\x8d\x44\x24\x14\x50\
\x8b\x06\x6a\x00\x6a\x00\x68????\x68\x73\x01\x00\x00\x50\x8b\x41\x04\x50\xff\xd2\x5f\
\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00";
const MODEL_PLAY_MOTION_WRAPPER: &[u8] =
    b"\x55\x8b\xec\x83\xe4\xf8\x83\xec\x18\x56\x8b\x75\x10\x8b\x46\x04\x57\x8b\x7d\x08\x50\x57\xe8????\
\x68????\x50\x8b\xcf\xe8????\x8b\x46\x08\x8b\x4e\x0c\x8b\x56\
\x10\x89\x44\x24\x10\x8b\x46\x14\x89\x44\x24\x1c\x8d\x44\x24\x0c\x50\x6a\x00\x8d\x44\x24\x18\x50\
\x8b\x06\x68????\x68\x6a\x01\x00\x00\x89\x4c\x24\x2c\x8b\x4d\x0c\x89\x54\x24\x28\x8b\
\x11\x8b\x52\x0c\x50\x8b\x41\x04\x50\xff\xd2\x5f\xb8\x02\x00\x00\x00\x5e\x8b\xe5\x5d\xc2\x10\x00";
const MODEL_MESSAGE_364_WRAPPER: &[u8] =
    b"\x56\x8b\x74\x24\x10\x8b\x46\x04\x57\x8b\x7c\x24\x0c\x50\x57\xe8????\x68????\
\x50\x8b\xcf\xe8????\x8b\x4c\x24\x10\x8b\x01\x8b\x40\x0c\x8d\x54\x24\x14\x52\x8b\x56\x08\
\x6a\x00\x52\x8b\x16\x68????\x68\x6c\x01\x00\x00\x52\x8b\x51\x04\x52\xff\xd0\x5f\
\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00";
const MODEL_MESSAGE_348_WRAPPER: &[u8] =
    b"\x81\xec\x08\x02\x00\x00\xa1????\x33\xc4\x89\x84\x24\x04\x02\x00\x00\x53\x8b\x9c\x24\
\x18\x02\x00\x00\x8b\x43\x04\x56\x8b\xb4\x24\x14\x02\x00\x00\x57\x8b\xbc\x24\x1c\x02\x00\x00\x50\
\x56\xe8????\x8d\x8c\x24\x10\x01\x00\x00\x51\x50\x8b\xce\xe8????\x8b\x43\
\x08\x50\x56\xe8????\x8d\x54\x24\x10\x52\x50\x8b\xce\xe8????\x8b\x07\x8d\
\x4c\x24\x0c\x51\x6a\x00\x8d\x54\x24\x18\x52\x8b\x13\x8d\x8c\x24\x1c\x01\x00\x00\x51\x8b\x4f\x04\
\x68\x5c\x01\x00\x00\x52\x8b\x50\x0c\x51\x8b\xcf\xff\xd2\x8b\x8c\x24\x10\x02\x00\x00\x5f\x5e\x5b\
\x33\xcc\xb8\x02\x00\x00\x00\xe8????\x81\xc4\x08\x02\x00\x00\xc2\x10\x00";
const CAMERA_RELATIVE_POSITION_WRAPPER: &[u8] =
    b"\x55\x8b\xec\x83\xe4\xf8\x81\xec\x8c\x01\x00\x00\xa1????\x33\xc4\x89\x84\x24\x88\x01\
\x00\x00\x8b\x4d\x08\x53\x8b\x5d\x10\x56\x57\x8b\x7d\x0c\x8d\x44\x24\x50\xe8????\x8b\
\x43\x04\x83\xf8\xff\x74\x0b\x50\x51\xe8????\x8b\xf0\xeb\x04\x8b\x74\x24\x10\x8b\x06\
\x8b\x4e\x04\x8b\x56\x08\x89\x44\x24\x14\xd9\x44\x24\x14\xd9\x5c\x24\x20\x89\x4c\x24\x18\xd9\x44\
\x24\x18\x89\x54\x24\x1c\xd9\x5c\x24\x24\x8d\x44\x24\x50\xd9\x44\x24\x1c\x50\xd9\x5c\x24\x2c\x8d\
\x4c\x24\x44\xd9\xe8\x51\xd9\x54\x24\x38\xd9\x54\x24\x3c\xd9\x5c\x24\x40\xe8????\x8b\
\x17\x8b\x52\x0c\x8d\x44\x24\x10\x50\x6a\x00\x8d\x4c\x24\x28\x51\x8b\x0b\x8d\x84\x24\x9c\x00\x00\
\x00\x50\x8b\x47\x04\x68\x57\x01\x00\x00\x51\x50\x8b\xcf\xff\xd2\xd9\x44\x24\x20\x8b\x8c\x24\x94\
\x01\x00\x00\xd9\x1e\xd9\x44\x24\x24\x5f\xd9\x5e\x04\xb8\x02\x00\x00\x00\xd9\x44\x24\x24\xd9\x5e\
\x08\x5e\x5b\x33\xcc\xe8????\x8b\xe5\x5d\xc2\x10\x00";
// Slots 560/561 and 642/643 read the same position, scale and rotation inputs.
// The camera-relative pair negates X and Z before packing the record.
const MODEL_TRANSFORM_SET_PREFIX: &[u8] =
    b"\x55\x8b\xec\x83\xe4\xf8\x81\xec\x9c\x01\x00\x00\xa1????\x33\xc4\x89\x84\x24\
\x98\x01\x00\x00\xd9\xee\x53\x8b\x5d\x10\xd9\x54\x24\x04\xd9\x54\x24\x08\x56\x8b\x75\x08\
\xd9\x5c\x24\x10\xd9\xe8\x57\x8b\x7d\x0c\xd9\x54\x24\x18\xd9\x54\x24\x1c\x8d\x44\x24\x60\
\xd9\x5c\x24\x20\x89\x74\x24\x24\x89\x7c\x24\x28\xe8????\x8b\x43\x08\x83\xf8\
\xff\x74\x1b\x50\x56\xe8????\x8b\x08\x8b\x50\x04\x8b\x40\x08\x89\x4c\x24\x0c\
\x89\x54\x24\x10\x89\x44\x24\x14\x8b\x43\x0c\x83\xf8\xff\x74\x1b\x50\x56\xe8????\
\x8b\x08\x8b\x50\x04\x8b\x40\x08\x89\x4c\x24\x18\x89\x54\x24\x1c\x89\x44\x24\x20\x8b\
\x43\x10\x83\xf8\xff\x74\x1c\x50\x56\xe8????\xb9\x10\x00\x00\x00\x8b\xf0\x8d\
\x7c\x24\x60\xf3\xa5\x8b\x7c\x24\x28\x8b\x74\x24\x24\x8b\x43\x04\x50\x56\xe8????\
\x8d\x8c\x24\xa0\x00\x00\x00\x51\x50\x8b\xce\xe8????";
const MODEL_TRANSFORM_SET_SUFFIX: &[u8] =
    b"\xd9\x44\x24\x0c\xd9\x5c\x24\x30\x8d\x54\x24\x60\xd9\x44\x24\x10\x52\xd9\x5c\x24\x38\x8d\x44\x24\
\x54\xd9\x44\x24\x18\x50\xd9\x5c\x24\x40\xd9\x44\x24\x20\xd9\x5c\x24\x48\xd9\x44\x24\x24\xd9\x5c\
\x24\x4c\xd9\x44\x24\x28\xd9\x5c\x24\x50\xe8????\x8b\x17\x8b\x52\x0c\x8d\x44\x24\x2c\
\x50\x6a\x00\x8d\x4c\x24\x38\x51\x8b\x0b\x8d\x84\x24\xac\x00\x00\x00\x50\x8b\x47\x04\x68?\x01\
\x00\x00\x51\x50\x8b\xcf\xff\xd2\x8b\x8c\x24\xa4\x01\x00\x00\x5f\x5e\x5b\x33\xcc\xb8\x02\x00\x00\
\x00\xe8????\x8b\xe5\x5d\xc2\x10\x00";
const CAMERA_RELATIVE_TRANSFORM_SUFFIX: &[u8] =
    b"\xd9\x44\x24\x0c\xd9\
\xe0\x8d\x54\x24\x60\xd9\x5c\x24\x30\x52\xd9\x44\x24\x14\x8d\x44\x24\x54\xd9\x5c\x24\x38\
\x50\xd9\x44\x24\x1c\xd9\xe0\xd9\x5c\x24\x40\xd9\x44\x24\x20\xd9\x5c\x24\x48\xd9\x44\x24\
\x24\xd9\x5c\x24\x4c\xd9\x44\x24\x28\xd9\x5c\x24\x50\xe8????\x8b\x17\x8b\x52\
\x0c\x8d\x44\x24\x2c\x50\x6a\x00\x8d\x4c\x24\x38\x51\x8b\x0b\x8d\x84\x24\xac\x00\x00\x00\
\x50\x8b\x47\x04\x68?\x01\x00\x00\x51\x50\x8b\xcf\xff\xd2\x8b\x8c\x24\xa4\x01\x00\x00\
\x5f\x5e\x5b\x33\xcc\xb8\x02\x00\x00\x00\xe8????\x8b\xe5\x5d\xc2\x10\x00";
// Four VM output addresses, followed by a display rectangle query and
// conversion of its right/bottom coordinates to width/height.
const LEGACY_DISPLAY_RECTANGLE_WRAPPER: &[u8] =
    b"\x55\x8b\xec\x83\xe4\xf8\x83\xec\x1c\x53\x56\x8b\x75\x10\x8b\x06\x57\x8b\x7d\x08\x50\x57\xe8????\
\x8b\x4e\x04\x51\x57\x8b\xd8\xe8????\x8b\x56\x08\x52\x57\x89\x44\x24\x18\xe8????\
\x89\x44\x24\x14\x8b\x46\x0c\x50\x57\xe8????\x8b\x4d\x0c\x8b\xf8\x8b\x41\x74\x8d\x74\x24\x18\xe8????\
\x8b\x44\x24\x18\x8b\x4c\x24\x1c\x8b\x54\x24\x10\x89\x03\x89\x0a\x8b\x54\x24\x20\x2b\xd0\
\x8b\x44\x24\x14\x89\x10\x8b\x54\x24\x24\x2b\xd1\x89\x17\x5f\x5e\xb8\x02\x00\x00\x00\x5b\x8b\xe5\x5d\xc2\x10\x00";
const LEGACY_DISPLAY_RECTANGLE_GETTER: &[u8] =
    b"\x8b\x0d????\x0f\xb7\x91\x58\x55\x00\x00\x0f\xb7\x89\x5a\x55\x00\x00\x83\xec\x14\x83\x38\x00\
\xc7\x06\x00\x00\x00\x00\xc7\x46\x04\x00\x00\x00\x00\x89\x56\x08\x89\x4e\x0c\x75\x2b\x83\x78\x04\x00\
\x74\x25\x8b\x48\x08\x85\xc9\x74\x1e\x8d\x04\x24\xe8????\x8b\x10\x8b\x48\x04\x89\x16\x8b\x50\x08\
\x8b\x40\x0c\x89\x4e\x04\x89\x56\x08\x89\x46\x0c\x8b\xc6\x83\xc4\x14\xc3";

fn fes_control_command(slot: u32, body: &[u8]) -> Option<u32> {
    let layouts: &[(&[u8], &[u8])] = match slot {
        // Select a named FES entry. The earlier ABIs obtain the target
        // through the formatter and call its message method at +0xc or +0x10.
        450 if body.starts_with(b"\x81\xec\x0c\x01\0\0") => &[
            (b"\x8b\x07\x8d\x4c\x24\x0c\x51\x6a\0\x8d\x4c\x24\x18\x51\x6a\0\x68",
             b"\xff\x33\x8b\xcf\xff\x77\x04\xff\x50\x10\x8b\x44\x24\x0c"),
            (b"\x6a\0\x8d\x54\x24\x18\x52\x8b\x56\x04\x6a\0\x68",
             b"\x51\x52\x8b\xce\xff\xd0\x8b\x4c\x24\x0c\x89\x0b"),
            (b"\x8b\x16\x8b\x52\x10\x8d\x44\x24\x10\x50\x8b\x03\x6a\0\x8d\x4c\x24\x1c\x51\x8b\x4e\x04\x6a\0\x68",
             b"\x50\x51\x8b\xce\xff\xd2\x8b\x44\x24\x10"),
            (b"\x8b\x16\x8b\x52\x0c\x8d\x44\x24\x10\x50\x8b\x03\x6a\0\x8d\x4c\x24\x1c\x51\x8b\x4e\x04\x6a\0\x68",
             b"\x50\x51\x8b\xce\xff\xd2\x8b\x44\x24\x10"),
            (b"\x8b\x16\x8b\x52\x0c\x8d\x44\x24\x0c\x50\x8b\x03\x6a\0\x8d\x4c\x24\x18\x51\x8b\x4e\x04\x6a\0\x68",
             b"\x50\x51\x8b\xce\xff\xd2\x8b\x8c\x24\x14\x01\0\0"),
        ],
        // Copy the FES locals into the current task.
        666 => &[
            (b"\x8b\x4c\x24\x08\x8d\x44\x24\x08\x50\x8b\x44\x24\x10\x6a\0\x8b\x11\x51\x6a\0\x68",
             b"\xff\x30\xff\x71\x04\xff\x52\x10\xb8\x02\0\0\0\xc2\x10\0"),
            (b"\x8b\x4c\x24\x08\x8b\x01\x8b\x40\x10\x8d\x54\x24\x08\x52\x8b\x54\x24\x10\x8b\x12\x6a\0\x51\x6a\0\x68",
             b"\x52\x8b\x51\x04\x52\xff\xd0\xb8\x02\0\0\0\xc2\x10\0"),
            (b"\x8b\x4c\x24\x08\x8b\x01\x8b\x40\x0c\x8d\x54\x24\x08\x52\x8b\x54\x24\x10\x8b\x12\x6a\0\x51\x6a\0\x68",
             b"\x52\x8b\x51\x04\x52\xff\xd0\xb8\x02\0\0\0\xc2\x10\0"),
        ],
        _ => return None,
    };
    let code = &body[..body.len().min(if slot == 450 { 0x80 } else { 0x30 })];
    let mut commands = layouts.iter().flat_map(|&(prefix, suffix)| {
        code.windows(prefix.len() + 4 + suffix.len()).enumerate().filter_map(move |(offset, window)| {
            if slot == 666 && offset != 0 {
                return None;
            }
            let operands = window.strip_prefix(prefix)?.strip_suffix(suffix)?;
            let command = root_clock::read_word(operands, 0)?;
            (256..=700).contains(&command).then_some(command)
        })
    });
    let command = commands.next()?;
    commands.next().is_none().then_some(command)
}

fn immediate_task_message(body: &[u8]) -> Option<u32> {
    if let Some(body) = body.strip_prefix(SINGLE_TARGET_MESSAGE_PREFIX) {
        let code = u32::from_le_bytes(body.get(..4)?.try_into().ok()?);
        return (matches!(code, 355 | 359) && body[4..].starts_with(SINGLE_TARGET_MESSAGE_TAIL))
            .then_some(code);
    }
    let body = body.strip_prefix(IMMEDIATE_TASK_MESSAGE_PREFIX)?;
    let code = u32::from_le_bytes(body.get(..4)?.try_into().ok()?);
    (matches!(code, 338 | 341) && body[4..].starts_with(IMMEDIATE_TASK_MESSAGE_TAIL))
        .then_some(code)
}

fn camera_direction_query<'a>(
    rva: u32,
    body: &[u8],
    code_at: impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<()> {
    let wrapper = body.get(..CAMERA_DIRECTION_WRAPPER.len())?;
    if !CAMERA_DIRECTION_WRAPPER.iter().zip(wrapper)
        .all(|(&expected, &actual)| expected == b'?' || expected == actual)
    {
        return None;
    }
    let resolve_call = |offset: usize| {
        let displacement = i32::from_le_bytes(wrapper[offset + 1..offset + 5].try_into().ok()?);
        rva.checked_add(offset as u32 + 5)?.checked_add_signed(displacement)
    };
    let resolver = resolve_call(0x17)?;
    (resolve_call(0x27) == Some(resolver)
        && code_at(resolver)?.starts_with(CAMERA_VM_ADDRESS_RESOLVER))
        .then_some(())
}

fn model_position_query<'a>(
    rva: u32,
    body: &[u8],
    code_at: impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<()> {
    let wrapper = body.get(..MODEL_POSITION_WRAPPER.len())?;
    if !MODEL_POSITION_WRAPPER.iter().zip(wrapper)
        .all(|(&expected, &actual)| expected == b'?' || expected == actual)
    {
        return None;
    }
    let resolve_call = |offset: usize| {
        let displacement = i32::from_le_bytes(wrapper[offset + 1..offset + 5].try_into().ok()?);
        rva.checked_add(offset as u32 + 5)?.checked_add_signed(displacement)
    };
    let resolver = resolve_call(0x31)?;
    let formatter = code_at(resolve_call(0x3e)?)?;
    let cookie_check = code_at(resolve_call(0xac)?)?;
    let cookie = &wrapper[7..11];
    (resolve_call(0x48) == Some(resolver)
        && code_at(resolver)?.starts_with(CAMERA_VM_ADDRESS_RESOLVER)
        && formatter.starts_with(b"\x81\xec\x60\x06\x00\x00\xa1")
        && formatter.get(7..11) == Some(cookie)
        && cookie_check.starts_with(b"\x3b\x0d")
        && cookie_check.get(2..6) == Some(cookie))
        .then_some(())
}

fn model_local_transform_query<'a>(
    rva: u32,
    body: &[u8],
    message: u32,
    code_at: impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<()> {
    let pattern = MODEL_LOCAL_TRANSFORM_QUERY_WRAPPER;
    let wrapper = body.get(..pattern.len())?;
    if wrapper[0xa0..0xa4] != message.to_le_bytes()
        || !pattern.iter().zip(wrapper).all(|(&expected, &actual)| {
            expected == b'?' || expected == actual
        })
    {
        return None;
    }
    let resolve_call = |offset: usize| {
        let displacement = i32::from_le_bytes(wrapper[offset + 1..offset + 5].try_into().ok()?);
        rva.checked_add(offset as u32 + 5)?.checked_add_signed(displacement)
    };
    let resolver = resolve_call(0x3e)?;
    let formatter = code_at(resolve_call(0x7f)?)?;
    let rotation = code_at(resolve_call(0xf0)?)?;
    let cookie_check = code_at(resolve_call(0x106)?)?;
    let cookie = &wrapper[0xd..0x11];
    ([0x51, 0x64, 0x72].into_iter().all(|offset| resolve_call(offset) == Some(resolver))
        && code_at(resolver)?.starts_with(b"\x55\x8b\xec\x83\xe4\xf8\x51\x8b\x55\x0c\x53\x56\x57\x85\xd2")
        && formatter.starts_with(b"\x81\xec\x60\x06\x00\x00\xa1")
        && formatter.get(7..11) == Some(cookie)
        && rotation.starts_with(b"\xff\x25")
        && rotation.get(6..13) == Some(b"\x55\x8b\xec\x83\xec\x10\x8b")
        && cookie_check.starts_with(b"\x3b\x0d")
        && cookie_check.get(2..6) == Some(cookie))
        .then_some(())
}

fn model_path_wrapper<'a>(
    rva: u32,
    body: &[u8],
    pattern: &[u8],
    resolver_offset: usize,
    formatter_offset: usize,
    path_offsets: &[usize],
    code_at: impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<()> {
    let wrapper = body.get(..pattern.len())?;
    if !pattern.iter().zip(wrapper)
        .all(|(&expected, &actual)| expected == b'?' || expected == actual)
        || path_offsets.windows(2).any(|offsets| {
            wrapper[offsets[0]..offsets[0] + 4] != wrapper[offsets[1]..offsets[1] + 4]
        })
    {
        return None;
    }
    let resolve_call = |offset: usize| {
        let displacement = i32::from_le_bytes(wrapper[offset + 1..offset + 5].try_into().ok()?);
        rva.checked_add(offset as u32 + 5)?.checked_add_signed(displacement)
    };
    let resolver = code_at(resolve_call(resolver_offset)?)?;
    let formatter = code_at(resolve_call(formatter_offset)?)?;
    (resolver.starts_with(CAMERA_VM_ADDRESS_RESOLVER)
        && formatter.starts_with(b"\x81\xec\x60\x06\x00\x00\xa1"))
        .then_some(())
}

fn model_transform_set<'a>(
    rva: u32,
    body: &[u8],
    message: u32,
    code_at: impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<()> {
    let (suffix, quaternion_call, dispatch_code, cookie_call) = if matches!(message, 345 | 347) {
        (CAMERA_RELATIVE_TRANSFORM_SUFFIX, 0x115, 0x139, 0x154)
    } else {
        (MODEL_TRANSFORM_SET_SUFFIX, 0x111, 0x135, 0x150)
    };
    let wrapper = body.get(..MODEL_TRANSFORM_SET_PREFIX.len() + suffix.len())?;
    if !MODEL_TRANSFORM_SET_PREFIX.iter().chain(suffix).zip(wrapper)
        .all(|(&expected, &actual)| expected == b'?' || expected == actual)
        || wrapper[dispatch_code..dispatch_code + 4] != message.to_le_bytes()
    {
        return None;
    }
    let call_target = |offset: usize| {
        let displacement = i32::from_le_bytes(wrapper[offset + 1..offset + 5].try_into().ok()?);
        rva.checked_add(offset as u32 + 5)?.checked_add_signed(displacement)
    };
    let resolver = call_target(0x5d)?;
    let formatter = code_at(call_target(0xd2)?)?;
    let cookie_check = code_at(call_target(cookie_call)?)?;
    let cookie = &wrapper[13..17];
    ([0x80, 0xa3, 0xc2].into_iter().all(|offset| call_target(offset) == Some(resolver))
        && code_at(resolver)?.starts_with(CAMERA_VM_ADDRESS_RESOLVER)
        && code_at(call_target(0x4e)?)?.starts_with(b"\x85\xc0\x75\x0d\xa1")
        && code_at(call_target(quaternion_call)?)?.starts_with(b"\xff\x25")
        && formatter.starts_with(b"\x81\xec\x60\x06\x00\x00\xa1")
        && formatter.get(7..11) == Some(cookie)
        && cookie_check.starts_with(b"\x3b\x0d")
        && cookie_check.get(2..6) == Some(cookie))
        .then_some(())
}

fn camera_relative_position<'a>(
    rva: u32,
    body: &[u8],
    code_at: impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<()> {
    let wrapper = body.get(..CAMERA_RELATIVE_POSITION_WRAPPER.len())?;
    if !CAMERA_RELATIVE_POSITION_WRAPPER.iter().zip(wrapper)
        .all(|(&expected, &actual)| expected == b'?' || expected == actual)
    {
        return None;
    }
    let call_target = |offset: usize| {
        let displacement = i32::from_le_bytes(wrapper[offset + 1..offset + 5].try_into().ok()?);
        rva.checked_add(offset as u32 + 5)?.checked_add_signed(displacement)
    };
    let cookie_check = code_at(call_target(0xdd)?)?;
    (code_at(call_target(0x39)?)?.starts_with(CAMERA_VM_ADDRESS_RESOLVER)
        && code_at(call_target(0x2a)?)?.starts_with(b"\x85\xc0\x75\x0d\xa1")
        && code_at(call_target(0x8a)?)?.starts_with(b"\xff\x25")
        && cookie_check.starts_with(b"\x3b\x0d")
        && cookie_check.get(2..6) == Some(&wrapper[13..17]))
        .then_some(())
}

fn legacy_display_rectangle<'a>(
    rva: u32,
    body: &[u8],
    code_at: impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<()> {
    let matches_code = |body: &[u8], pattern: &[u8]| {
        body.get(..pattern.len()).is_some_and(|bytes|
            pattern.iter().zip(bytes).all(|(&expected, &actual)| expected == b'?' || expected == actual))
    };
    if !matches_code(body, LEGACY_DISPLAY_RECTANGLE_WRAPPER) {
        return None;
    }
    let call_target = |offset: usize| {
        let displacement = i32::from_le_bytes(body.get(offset + 1..offset + 5)?.try_into().ok()?);
        rva.checked_add(offset as u32 + 5)?.checked_add_signed(displacement)
    };
    let resolver = call_target(0x16)?;
    if [0x22, 0x30, 0x3e].into_iter().any(|offset| call_target(offset) != Some(resolver)) {
        return None;
    }
    code_at(resolver)?;
    let getter = call_target(0x4f)?;
    (getter != resolver && matches_code(code_at(getter)?, LEGACY_DISPLAY_RECTANGLE_GETTER))
        .then_some(())
}

// A launcher can sort before the actual game executable in the general scan.
// Use the sibling boot descriptor to find the declared program for native ABI
// discovery, without changing the executable identity used by existing saves.
fn launcher_game_executable<'a>(scan: &'a GameScan, launcher: &Path) -> Option<&'a Path> {
    let directory = launcher.parent()?;
    let descriptor = directory.join("boot.dfn");
    let descriptor = scan.resources.files().iter().find(|file| {
        file.relative_path.to_string_lossy().eq_ignore_ascii_case(&descriptor.to_string_lossy())
    })?;
    if descriptor.size > 64 * 1024 {
        return None;
    }
    let bytes = std::fs::read(scan.root.join(&descriptor.relative_path)).ok()?;
    let text = std::str::from_utf8(&bytes)
        .map(std::borrow::Cow::Borrowed)
        .unwrap_or_else(|_| encoding_rs::SHIFT_JIS.decode_without_bom_handling(&bytes).0);
    let mut targets = text.lines().filter_map(|line| {
        let line = line.trim_start_matches('\u{feff}').trim_start();
        let (key, value) = line.split_once(char::is_whitespace)?;
        if !key.eq_ignore_ascii_case("boot") {
            return None;
        }
        let (target, _) = value.trim_start().strip_prefix('"')?.split_once('"')?;
        Some(target.replace('\\', "/"))
    });
    let target = targets.next()?;
    let path = Path::new(&target);
    if targets.next().is_some()
        || path.components().any(|component| !matches!(component, Component::Normal(_)))
        || !path.extension()?.eq_ignore_ascii_case("exe")
    {
        return None;
    }
    let target = directory.join(path);
    let mut files = scan.resources.files().iter().filter_map(|file| {
        let path = &file.relative_path;
        path.to_string_lossy().eq_ignore_ascii_case(&target.to_string_lossy()).then_some(path.as_path())
    });
    let game = files.next()?;
    (game != launcher && files.next().is_none()).then_some(game)
}

#[derive(Clone, Copy)]
enum CodeSection {
    Executable,
    FlatText(Section),
}

impl CodeSection {
    fn contains_code(self, section: Section) -> bool {
        match self {
            Self::Executable => section.executable(),
            Self::FlatText(text) => section == text,
        }
    }

    fn contains_data(self, section: Section) -> bool {
        match self {
            Self::Executable => !section.executable(),
            Self::FlatText(_) => section.is_readable_data(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderArgumentOrder {
    DurationFirst,
    RecordOrder,
}

#[derive(Debug)]
pub struct StartupCompanionNatives {
    pub native439_continue: bool,
    pub native925_zero: bool,
    pub null_task_factory: NullTaskFactory,
    pub emote_task_family: bool,
    pub state_tree_commands: Option<StateTreeCommands>,
    pub css_restore_command: Option<CssRestoreCommand>,
    pub kc_view_binding: bool,
    pub legacy_view_lifecycle: Option<u32>,
    pub hg_window_blend: Option<HgWindowBlend>,
    pub root_sound_query: Option<u32>,
    pub draw_order: Option<RenderArgumentOrder>,
    pub wipe_order: Option<RenderArgumentOrder>,
    pub message_list_base: Option<u32>,
    pub scene_tool_commands: SceneToolCommands,
    pub fes_control_wrappers: [Option<u32>; 2],
    pub particle_messages: Option<ParticleMessages>,
    pub shared_slots_default: Option<usize>,
    pub raster_messages: Option<RasterMessages>,
    pub raster_finalize_passthrough: bool,
}

/// Supported outcomes and argument layouts recovered from a registered
/// ordinary VM native table in a PE32 image. The executable is read as data only.
#[derive(Debug)]
pub struct OrdinaryNativeTable {
    constants: BTreeMap<u32, ConstantNativeReturn>,
    byte_string_split: bool,
    immediate_task_messages: BTreeMap<u32, u32>,
    camera_direction_query: bool,
    model_position_query: bool,
    model_local_transform_queries: [bool; 2],
    model_visibility: [bool; 2],
    model_clear_overrides: bool,
    model_freeze_transform: bool,
    model_play_motion: bool,
    model_message_364: bool,
    model_message_348: bool,
    model_transform_set: [bool; 4],
    legacy_list_wrapper_query: bool,
    legacy_list_affine: [bool; 2],
    short_motion_properties: [bool; 4],
    camera_relative_position: bool,
    legacy_display_rectangle: bool,
    draw_order: Option<RenderArgumentOrder>,
    wipe_order: Option<RenderArgumentOrder>,
    image_fill_current_pixels: bool,
    image_records: image_records::ImageRecords,
    raster_messages: Option<RasterMessages>,
    raster_finalize_passthrough: bool,
    string_plane_commands: string_plane::StringPlaneCommands,
    configuration_lengths: Option<[usize; 4]>,
    shared_slots_default: Option<usize>,
    story_language_prefixes: bool,
    javascript_save_callbacks: bool,
    native549_returns_vm_identity: bool,
    null_task_factory: NullTaskFactory,
    emote_task_family: bool,
    state_tree_commands: Option<StateTreeCommands>,
    css_restore_command: Option<CssRestoreCommand>,
    kc_view_binding: bool,
    legacy_view_lifecycle: Option<u32>,
    hg_window_blend: Option<HgWindowBlend>,
    root_sound_query: Option<u32>,
    achievement_family: bool,
    effekseer_family: bool,
    tweet_family: bool,
    wallpaper_package_wrapper: bool,
    root_clock_command: Option<u32>,
    task_single_step: bool,
    message_list_base: Option<u32>,
    scene_tool_commands: SceneToolCommands,
    fes_control_wrappers: [Option<u32>; 2],
    particle_messages: Option<ParticleMessages>,
    sequence_resource_category: Option<&'static [u8]>,
}

impl OrdinaryNativeTable {
    /// Read the primary or launcher-declared native table, or associate a
    /// flat-text companion with the encrypted archive supplying the startup script.
    pub fn from_game_scan(scan: &GameScan, startup_entry: Option<&str>) -> Option<Self> {
        Self::from_game_executable(scan)
            .or_else(|| Self::flat_companion_natives(scan, startup_entry?))
    }

    pub fn from_game_executable(scan: &GameScan) -> Option<Self> {
        let primary = &scan.engine.primary_executable.as_ref()?.relative_path;
        let primary_bytes = std::fs::read(scan.root.join(primary)).ok()?;
        if let Some(table) = Self::from_pe(&primary_bytes) {
            return Some(table);
        }
        let game = launcher_game_executable(scan, primary)?;
        std::fs::read(scan.root.join(game)).ok().and_then(|bytes| Self::from_pe(&bytes))
    }

    pub fn flat_companion_natives(scan: &GameScan, entry: &str) -> Option<Self> {
        let resource = scan.resources.resolve(entry).ok().flatten()?;
        Self::from_archive_companion(scan, &resource.location, |bytes| {
            let (image_base, entry_rva) = x86_image_layout(bytes)?;
            let sections = parse_sections(bytes).ok()?;
            Self::from_flat_text(bytes, &sections, image_base, entry_rva)
        })
    }

    /// Recover supported contracts when the same encrypted archive supplies the
    /// effective startup and scene-script programs and identifies one companion.
    pub fn startup_companion_natives(scan: &GameScan, startup_entry: &str) -> Option<StartupCompanionNatives> {
        let startup = scan.resources.resolve(startup_entry).ok().flatten()?;
        let names = scan.resources.category_names(b"kcs", "sscript").ok()?;
        let scene_script = names.iter().find_map(|name| {
            scan.resources.resolve_mode_candidate(name, ResourceMode::LooseFirst).ok().flatten()
        })?;
        let (ResourceLocation::Archive { path: startup_path, name_seed: Some(startup_seed), .. },
            ResourceLocation::Archive { path: scene_path, name_seed: Some(scene_seed), .. }) =
            (&startup.location, &scene_script.location)
        else {
            return None;
        };
        if startup_path != scene_path || startup_seed != scene_seed {
            return None;
        }
        let table = Self::from_archive_companion(scan, &startup.location, Self::from_pe)?;
        Some(StartupCompanionNatives {
            native439_continue: table.constant(439) == Some(ConstantNativeReturn::Continue),
            native925_zero: table.feature_constant(925) == Some(ConstantNativeReturn::Value(0)),
            null_task_factory: table.null_task_factory(),
            emote_task_family: table.emote_task_family(),
            state_tree_commands: table.state_tree_commands(),
            css_restore_command: table.css_restore_command(),
            kc_view_binding: table.kc_view_binding(),
            legacy_view_lifecycle: table.legacy_view_lifecycle(),
            hg_window_blend: table.hg_window_blend(),
            root_sound_query: table.root_sound_query(),
            draw_order: table.render_argument_order(527),
            wipe_order: table.render_argument_order(530),
            message_list_base: table.message_list_base(),
            scene_tool_commands: table.scene_tool_commands(),
            fes_control_wrappers: table.fes_control_wrappers(),
            particle_messages: table.particle_messages(),
            shared_slots_default: table.shared_slots_default(),
            raster_messages: table.raster_messages(),
            raster_finalize_passthrough: table.raster_finalize_passes_through(),
        })
    }

    fn from_archive_companion(
        scan: &GameScan,
        startup: &ResourceLocation,
        table_from_bytes: impl Fn(&[u8]) -> Option<Self>,
    ) -> Option<Self> {
        let primary = &scan.engine.primary_executable.as_ref()?.relative_path;
        let directory = primary.parent()?;
        let ResourceLocation::Archive {
            path,
            name_seed: Some(name_seed),
            ..
        } = startup
        else {
            return None;
        };
        if path.parent()? != scan.root.join(directory) {
            return None;
        }

        let mut companions = scan.resources.files().iter().filter_map(|file| {
            let path = &file.relative_path;
            if path == primary
                || path.parent()? != directory
                || !path.extension()?.eq_ignore_ascii_case("exe")
            {
                return None;
            }
            let bytes = std::fs::read(scan.root.join(path)).ok()?;
            if archive_name_seed_from_pe(&bytes).ok().flatten()? != *name_seed {
                return None;
            }
            table_from_bytes(&bytes)
        });
        let companion = companions.next()?;
        companions.next().is_none().then_some(companion)
    }

    pub fn from_pe(data: &[u8]) -> Option<Self> {
        let (image_base, entry_rva) = x86_image_layout(data)?;
        let sections = parse_sections(data).ok()?;
        if let Some(table) =
            Self::from_sections(data, &sections, image_base, CodeSection::Executable).ok()?
        {
            return Some(table);
        }
        Self::from_flat_text(data, &sections, image_base, entry_rva)
    }

    fn from_flat_text(
        data: &[u8],
        sections: &[Section],
        image_base: u32,
        entry_rva: u32,
    ) -> Option<Self> {
        // Some disk images retain a flat, readable .text mapping without
        // MEM_EXECUTE. Only the entry-point section may supply code evidence.
        let mut entry_sections = sections
            .iter()
            .filter(|section| section.file_bytes_from(data, entry_rva).is_some());
        let text = *entry_sections.next()?;
        if entry_sections.next().is_some() || !text.is_flat_text_code() {
            return None;
        }
        Self::from_sections(data, sections, image_base, CodeSection::FlatText(text))
            .ok()
            .flatten()
    }

    // Distinguish an absent table from contradictory registrations.
    fn from_sections(
        data: &[u8],
        sections: &[Section],
        image_base: u32,
        code_section: CodeSection,
    ) -> Result<Option<Self>, ()> {
        let file_bytes_at = |rva, code| {
            let mut matching = sections.iter().filter_map(|section| {
                section
                    .file_bytes_from(data, rva)
                    .map(|bytes| (bytes, *section))
            });
            let (bytes, section) = matching.next()?;
            let expected = if code {
                code_section.contains_code(section)
            } else {
                code_section.contains_data(section)
            };
            (expected && matching.next().is_none()).then_some(bytes)
        };
        let mut ordinary = None;
        for section in sections
            .iter()
            .filter(|&&section| code_section.contains_code(section))
        {
            let Some(code) = section.file_bytes_from(data, section.virtual_address) else {
                continue;
            };
            for (offset, instructions) in code.windows(REGISTRATION_LEN).enumerate() {
                let table_va = match instructions {
                    // push table; mov ecx,eax; call VM table setter
                    [0x68, t0, t1, t2, t3, 0x8b, 0xc8, 0xe8, a, b, c, d]
                        if matches!(code_section, CodeSection::Executable) =>
                    {
                        let Some(next) = section
                            .virtual_address
                            .checked_add((offset + REGISTRATION_LEN) as u32)
                        else {
                            continue;
                        };
                        let displacement = i32::from_le_bytes([*a, *b, *c, *d]);
                        if !next
                            .checked_add_signed(displacement)
                            .and_then(|target| file_bytes_at(target, true))
                            .is_some_and(|body| body.starts_with(VM_TABLE_SETTER))
                        {
                            continue;
                        }
                        u32::from_le_bytes([*t0, *t1, *t2, *t3])
                    }
                    [0x85, 0xc0, 0x74, ..] => {
                        let Some(table_va) = code
                            .get(offset..offset + INLINE_REGISTRATION_LEN)
                            .and_then(inline_table_va)
                        else {
                            continue;
                        };
                        table_va
                    }
                    _ => continue,
                };
                let Some(table_rva) = table_va.checked_sub(image_base) else {
                    continue;
                };
                if ordinary.as_ref().is_some_and(|(rva, _)| *rva == table_rva) {
                    continue;
                }
                let Some(table) = file_bytes_at(table_rva, false) else {
                    continue;
                };
                let entries: Vec<_> = table
                    .chunks_exact(4)
                    .take(LAST_TRACKED_SLOT + 1)
                    .map_while(|pointer| {
                        let entry =
                            u32::from_le_bytes(pointer.try_into().ok()?).checked_sub(image_base)?;
                        file_bytes_at(entry, true).map(|body| (entry, body))
                    })
                    .collect();
                if entries.len() < MIN_ORDINARY_ENTRIES
                    || entries[0].0 != entries[2].0
                    || entries[0].0 != entries[3].0
                {
                    continue;
                }
                if ordinary.replace((table_rva, entries)).is_some() {
                    return Err(());
                }
            }
        }
        let Some((_, entries)) = ordinary else {
            return Ok(None);
        };
        let byte_string_split = entries.get(41).is_some_and(|&(rva, body)| {
            byte_string_splitter(rva, body, |target| file_bytes_at(target, true)).is_some()
        });
        let fes_control_wrappers = [
            entries.get(450).and_then(|&(_, bytes)| fes_control_command(450, bytes)),
            entries.get(666).and_then(|&(_, bytes)| fes_control_command(666, bytes)),
        ];
        let immediate_task_messages = entries
            .iter()
            .enumerate()
            .filter_map(|(index, (_, body))| {
                immediate_task_message(body).map(|code| (index as u32, code))
            })
            .collect();
        let camera_direction_query = entries.get(661).is_some_and(|&(rva, body)| {
            camera_direction_query(rva, body, |target| file_bytes_at(target, true)).is_some()
        });
        let model_position_query = entries.get(660).is_some_and(|&(rva, body)| {
            model_position_query(rva, body, |target| file_bytes_at(target, true)).is_some()
        });
        let model_local_transform_queries = [(671, 370), (672, 369)].map(|(index, message)| {
            entries.get(index).is_some_and(|&(rva, body)| {
                model_local_transform_query(rva, body, message, |target| file_bytes_at(target, true))
                    .is_some()
            })
        });
        let model_visibility = [
            (550, MODEL_SET_VISIBILITY_WRAPPER, 0x38),
            (551, MODEL_GET_VISIBILITY_WRAPPER, 0x36),
        ].map(|(index, pattern, path)| {
            entries.get(index).is_some_and(|&(rva, body)| {
                model_path_wrapper(rva, body, pattern, 0x0f, 0x1c, &[0x15, path],
                    |target| file_bytes_at(target, true)).is_some()
            })
        });
        let model_clear_overrides = entries.get(687).is_some_and(|&(rva, body)| {
            model_path_wrapper(rva, body, MODEL_CLEAR_OVERRIDES_WRAPPER, 0x0f, 0x1c, &[0x15, 0x36],
                |target| file_bytes_at(target, true)).is_some()
        });
        let model_freeze_transform = entries.get(686).is_some_and(|&(rva, body)| {
            model_path_wrapper(rva, body, MODEL_FREEZE_TRANSFORM_WRAPPER, 0x0f, 0x1c, &[0x15, 0x3c, 0x67],
                |target| file_bytes_at(target, true)).is_some()
        });
        let model_play_motion = entries.get(644).is_some_and(|&(rva, body)| {
            model_path_wrapper(rva, body, MODEL_PLAY_MOTION_WRAPPER, 0x16, 0x23, &[0x1c, 0x4b],
                |target| file_bytes_at(target, true)).is_some()
        });
        let model_message_364 = entries.get(556).is_some_and(|&(rva, body)| {
            model_path_wrapper(rva, body, MODEL_MESSAGE_364_WRAPPER, 0x0f, 0x1c, &[0x15, 0x38],
                |target| file_bytes_at(target, true)).is_some()
        });
        let model_message_348 = entries.get(562).is_some_and(|&(rva, body)| {
            model_path_wrapper(rva, body, MODEL_MESSAGE_348_WRAPPER, 0x31, 0x41, &[],
                |target| file_bytes_at(target, true)).is_some()
        });
        let model_transform_set = [(560, 344), (642, 345), (561, 346), (643, 347)].map(|(index, message)| {
            entries.get(index).is_some_and(|&(rva, body)| {
                model_transform_set(rva, body, message, |target| file_bytes_at(target, true)).is_some()
            })
        });
        // The old 657 wrapper forwards message 269 and returns the borrowed
        // image wrapper written to the local output slot by that receiver.
        let legacy_list_wrapper_query = entries.get(657).is_some_and(|&(_, body)| {
            body.starts_with(b"\x51\x8b\x4c\x24\x0c\x8b\x11\x8b\x52\x0c\x56\x8d\x44\x24\x10\x50\x6a\x00\x8d\x44\x24\x0c\x50\x8b\x44\x24\x20\x8b\x70\x04\x8b\x00\x56\x68\x0d\x01\x00\x00\x50\x8b\x41\x04\x50\xc7\x44\x24\x20\x00\x00\x00\x00\xff\xd2\x8b\x44\x24\x18\x8b\x4c\x24\x04\x89\x08\xb8\x01\x00\x00\x00\x5e\x59\xc2\x10\x00")
        });
        let legacy_list_affine = [626, 627].map(|index| {
            entries.get(index).is_some_and(|&(rva, body)| {
                legacy_layout::list_affine(index, rva, body, |callee| file_bytes_at(callee, true).is_some()).is_some()
            })
        });
        let short_motion_properties = [299, 300, 301, 302].map(|index| {
            entries.get(index).is_some_and(|&(rva, body)| {
                legacy_layout::short_motion(index, rva, body, |callee| file_bytes_at(callee, true).is_some()).is_some()
            })
        });
        let camera_relative_position = entries.get(670).is_some_and(|&(rva, body)| {
            camera_relative_position(rva, body, |target| file_bytes_at(target, true)).is_some()
        });
        let legacy_display_rectangle = entries.get(700).is_some_and(|&(rva, body)| {
            legacy_display_rectangle(rva, body, |target| file_bytes_at(target, true)).is_some()
        });
        let draw_order = entries
            .get(527)
            .and_then(|(_, body)| render_wrapper_order(body, &DRAW_WRAPPERS));
        let wipe_order = entries
            .get(530)
            .and_then(|(_, body)| render_wrapper_order(body, &WIPE_WRAPPERS));
        let image_fill_current_pixels = entries.get(736).is_some_and(|&(rva, body)| {
            image_fill::is_current_pixels_fill(rva, body, image_base, file_bytes_at)
        });
        let image_records = image_records::image_records(&entries, image_base, file_bytes_at);
        let raster_messages = raster::messages(&entries, image_base, file_bytes_at);
        let raster_finalize_passthrough = raster_messages.is_some_and(|messages| {
            raster::finalize_passthrough(&entries, messages, data, sections, image_base, file_bytes_at).is_some()
        });
        let configuration_lengths = configuration::configuration_lengths(&entries, file_bytes_at);
        let shared_slots_default = entries.get(789).and_then(|&(_, getter)| {
            let code = sections.iter().filter_map(|&section| {
                code_section.contains_code(section).then_some(())?;
                section.file_bytes_from(data, section.virtual_address)
            });
            configuration::shared_slots_default(getter, image_base, |rva| file_bytes_at(rva, false), code)
        });
        let native549_returns_vm_identity = entries.get(549).is_some_and(|&(_, slot)| {
            let code = |rva| file_bytes_at(rva, true);
            let code_sections = sections.iter().filter_map(|&section| {
                code_section.contains_code(section).then_some(())?;
                Some((section.virtual_address, code(section.virtual_address)?))
            });
            vm_context::returns_vm_identity(slot, image_base, code, code_sections)
        });
        let javascript_save_callbacks = matches!(code_section, CodeSection::Executable)
            && javascript_save::callbacks(data, sections, image_base, file_bytes_at).is_some();
        let achievement_family = matches!(code_section, CodeSection::Executable)
            && achievements::registered_family(&entries, data, image_base, &file_bytes_at).is_some();
        let effekseer_family = matches!(code_section, CodeSection::Executable)
            && shell_tasks::effekseer_family(&entries, data, sections, image_base, |rva| file_bytes_at(rva, true));
        let tweet_family = matches!(code_section, CodeSection::Executable)
            && shell_tasks::tweet_family(&entries, data, sections, image_base, |rva| file_bytes_at(rva, true));
        let wallpaper_package_wrapper = shell_tasks::wallpaper_package_wrapper(&entries);
        let story_language_prefixes = entries.get(1036).is_some_and(|&entry| {
            let code_sections = sections.iter().filter_map(|&section| {
                let rva = section.virtual_address;
                code_section.contains_code(section).then_some(())?;
                Some((rva, section.file_bytes_from(data, rva)?))
            });
            language::story_prefixes(entry, image_base, file_bytes_at, code_sections).is_some()
        });
        let string_plane_commands = match code_section {
            CodeSection::Executable => string_plane::commands(&entries, data, sections, image_base),
            CodeSection::FlatText(_) => Default::default(),
        };
        let root_clock_command = match code_section {
            CodeSection::Executable => entries.get(577).and_then(|&(_, body)| {
                root_clock::command(body, data, sections, image_base, file_bytes_at)
            }),
            CodeSection::FlatText(_) => None,
        };
        let sequence_resource_category = entries.get(458).and_then(|&(rva, body)| {
            sequence_resource_category(rva, body, image_base, file_bytes_at)
        });
        let message_list_base = message_list::command_base(
            &entries, data, sections, image_base, file_bytes_at,
        );
        let scene_tool_commands = scene_tools::registered(
            &entries, data, sections, image_base, file_bytes_at,
        );
        let particle_messages = particle::registered(
            &entries, data, sections, image_base, |rva| file_bytes_at(rva, true),
        );
        let task_single_step = entries.get(1074).is_some_and(|&entry| {
            supports_single_step(entry, |rva| file_bytes_at(rva, true)).is_some()
        });
        let (null_task_factory, emote_task_family) = entries.get(66).and_then(|&(_, wrapper)| {
            let load = entries.get(906).map(|&(_, body)| body);
            factory::registered_factory(wrapper, load, data, sections, image_base, |rva| {
                file_bytes_at(rva, true)
            })
        }).unwrap_or_default();
        let state_tree_commands = entries.get(688).zip(entries.get(689))
            .and_then(|(&(_, capture), &(_, restore))| state_tree::commands(capture, restore));
        let css_restore_command = entries.get(576).and_then(|&entry| {
            css_restore::registered(entry, data, sections, image_base, |rva| file_bytes_at(rva, true))
        });
        let kc_view_binding = entries.get(543).zip(entries.get(652)).is_some_and(|(&(_, load), &(_, bind))| {
            css_restore::kc_view_binding(load, bind, data, sections, image_base, |rva| file_bytes_at(rva, true)).is_some()
        });
        let legacy_view_lifecycle = entries.get(83)
            .and_then(|&(_, wrapper)| css_restore::legacy_view_lifecycle(wrapper));
        let hg_window_blend = entries.get(648)
            .and_then(|&entry| hg_window::registered(entry, image_base, file_bytes_at));
        let root_sound_query = root_sound::registered(data, sections, image_base, file_bytes_at);
        let constants = match code_section {
            CodeSection::Executable => entries
                .into_iter()
                .enumerate()
                .filter_map(|(index, (_, body))| {
                    constant_stub(body).map(|result| (index as u32, result))
                })
                .collect(),
            CodeSection::FlatText(_) => {
                // Require three independently verified consumers before
                // exposing behavior from this registration.
                if draw_order.is_none() || wipe_order.is_none() || !image_fill_current_pixels {
                    return Ok(None);
                }
                let mut constants = BTreeMap::new();
                // The localized save-resume caller uses slot 439 as a
                // statement. Expose it only if the wrapper writes no result.
                if entries.get(439).and_then(|(_, body)| constant_stub(body))
                    == Some(ConstantNativeReturn::Continue)
                {
                    constants.insert(439, ConstantNativeReturn::Continue);
                }
                constants
            }
        };
        Ok(Some(Self {
            constants,
            byte_string_split,
            immediate_task_messages,
            camera_direction_query,
            model_position_query,
            model_local_transform_queries,
            model_visibility,
            model_clear_overrides,
            model_freeze_transform,
            model_play_motion,
            model_message_364,
            model_message_348,
            model_transform_set,
            legacy_list_wrapper_query,
            legacy_list_affine,
            short_motion_properties,
            camera_relative_position,
            legacy_display_rectangle,
            draw_order,
            wipe_order,
            image_fill_current_pixels,
            image_records,
            raster_messages,
            raster_finalize_passthrough,
            string_plane_commands,
            configuration_lengths,
            shared_slots_default,
            story_language_prefixes,
            javascript_save_callbacks,
            native549_returns_vm_identity,
            null_task_factory,
            emote_task_family,
            state_tree_commands,
            css_restore_command,
            kc_view_binding,
            legacy_view_lifecycle,
            hg_window_blend,
            root_sound_query,
            achievement_family,
            effekseer_family,
            tweet_family,
            wallpaper_package_wrapper,
            root_clock_command,
            task_single_step,
            message_list_base,
            scene_tool_commands,
            fes_control_wrappers,
            particle_messages,
            sequence_resource_category,
        }))
    }

    pub fn null_task_factory(&self) -> NullTaskFactory {
        self.null_task_factory
    }

    pub fn emote_task_family(&self) -> bool {
        self.emote_task_family
    }

    pub fn state_tree_commands(&self) -> Option<StateTreeCommands> {
        self.state_tree_commands
    }

    pub fn css_restore_command(&self) -> Option<CssRestoreCommand> {
        self.css_restore_command
    }

    pub fn kc_view_binding(&self) -> bool {
        self.kc_view_binding
    }

    pub fn legacy_view_lifecycle(&self) -> Option<u32> {
        self.legacy_view_lifecycle
    }

    pub fn hg_window_blend(&self) -> Option<HgWindowBlend> {
        self.hg_window_blend
    }

    pub fn root_sound_query(&self) -> Option<u32> {
        self.root_sound_query
    }

    /// Slot 736 writes the current raw pixels, or clears a persistent
    /// graphics-canvas rectangle when the target is null.
    pub fn image_fill_uses_current_pixels(&self) -> bool {
        self.image_fill_current_pixels
    }

    /// The registered slot 41 splits strings as raw bytes.
    pub fn string_split_uses_bytes(&self) -> bool {
        self.byte_string_split
    }

    pub fn image_producer_703(&self) -> Option<ImageProducer703> {
        self.image_records.producer
    }

    pub fn image_release_704(&self) -> Option<ImageRecordLayout> {
        self.image_records.release
    }

    pub fn raster_messages(&self) -> Option<RasterMessages> {
        self.raster_messages
    }

    pub fn raster_finalize_passes_through(&self) -> bool {
        self.raster_finalize_passthrough
    }

    pub fn message_list_base(&self) -> Option<u32> {
        self.message_list_base
    }

    pub fn scene_tool_commands(&self) -> SceneToolCommands {
        self.scene_tool_commands
    }

    /// Message code sent by the registered two-argument wrapper.
    pub fn immediate_task_message(&self, index: u32) -> Option<u32> {
        self.immediate_task_messages.get(&index).copied()
    }

    pub fn has_camera_direction_query(&self) -> bool {
        self.camera_direction_query
    }

    pub fn has_model_position_query(&self) -> bool {
        self.model_position_query
    }

    pub fn model_local_transform_query_message(&self, index: u32) -> Option<u32> {
        match (index, self.model_local_transform_queries) {
            (671, [true, _]) => Some(370),
            (672, [_, true]) => Some(369),
            _ => None,
        }
    }

    pub fn model_visibility_message(&self, index: u32) -> Option<u32> {
        match (index, self.model_visibility) {
            (550, [true, _]) => Some(336),
            (551, [_, true]) => Some(337),
            _ => None,
        }
    }

    pub fn has_model_clear_overrides(&self) -> bool {
        self.model_clear_overrides
    }

    pub fn has_model_freeze_transform(&self) -> bool {
        self.model_freeze_transform
    }

    pub fn has_legacy_list_wrapper_query(&self) -> bool {
        self.legacy_list_wrapper_query
    }

    pub fn has_legacy_list_affine(&self, index: u32) -> bool {
        (626..=627).contains(&index) && self.legacy_list_affine[(index - 626) as usize]
    }

    pub fn has_short_motion_properties(&self, index: u32) -> bool {
        (299..=302).contains(&index) && self.short_motion_properties[(index - 299) as usize]
    }

    pub fn model_motion_message(&self, index: u32) -> Option<u32> {
        match index {
            556 if self.model_message_364 => Some(364),
            562 if self.model_message_348 => Some(348),
            644 if self.model_play_motion => Some(362),
            _ => None,
        }
    }

    pub fn model_transform_set_message(&self, index: u32) -> Option<u32> {
        match (index, self.model_transform_set) {
            (560, [true, ..]) => Some(344),
            (642, [_, true, ..]) => Some(345),
            (561, [_, _, true, _]) => Some(346),
            (643, [.., true]) => Some(347),
            _ => None,
        }
    }

    pub fn has_camera_relative_position(&self) -> bool {
        self.camera_relative_position
    }

    pub fn has_legacy_display_rectangle(&self) -> bool {
        self.legacy_display_rectangle
    }

    pub fn has_unhandled_task_message_341(&self) -> bool {
        self.immediate_task_messages.values().any(|&code| code == 341)
    }

    pub fn string_plane_has_base_wrappers(&self) -> bool {
        self.string_plane_commands.base
    }

    pub fn string_plane_has_position_wrapper(&self) -> bool {
        self.string_plane_commands.position
    }

    pub fn string_plane_has_alpha_wrapper(&self) -> bool {
        self.string_plane_commands.alpha
    }

    /// The four export lengths. Import uses the same first three blocks.
    pub fn configuration_lengths(&self) -> Option<[usize; 4]> {
        self.configuration_lengths
    }

    pub fn shared_slots_default(&self) -> Option<usize> {
        self.shared_slots_default
    }

    pub fn particle_messages(&self) -> Option<ParticleMessages> {
        self.particle_messages
    }

    pub fn sequence_resource_category(&self) -> Option<&'static [u8]> {
        self.sequence_resource_category
    }

    /// Root command sent by slot 577 when the PE verifies the clock hold ABI.
    pub fn root_clock_command(&self) -> Option<u32> {
        self.root_clock_command
    }

    pub fn task_clock_supports_single_step(&self) -> bool {
        self.task_single_step
    }

    /// Slot 1036 returns main-text and subtitle prefixes in this order.
    pub fn language_query_uses_story_prefixes(&self) -> bool {
        self.story_language_prefixes
    }

    /// The JavaScript task registers the verified CSS and save-notification ABI.
    pub fn javascript_save_callbacks_registered(&self) -> bool {
        self.javascript_save_callbacks
    }

    pub fn native549_returns_vm_identity(&self) -> bool {
        self.native549_returns_vm_identity
    }

    /// Slots 1041–1044 register the Steam achievement cache receiver.
    pub fn has_achievement_family(&self) -> bool {
        self.achievement_family
    }

    pub fn has_effekseer_family(&self) -> bool {
        self.effekseer_family
    }

    pub fn fes_control_wrappers(&self) -> [Option<u32>; 2] {
        self.fes_control_wrappers
    }

    pub fn has_tweet_family(&self) -> bool {
        self.tweet_family
    }

    pub fn has_wallpaper_package_wrapper(&self) -> bool {
        self.wallpaper_package_wrapper
    }

    pub fn render_argument_order(&self, index: u32) -> Option<RenderArgumentOrder> {
        match index {
            527 => self.draw_order,
            530 => self.wipe_order,
            _ => None,
        }
    }

    /// A slot is usable only when its entire function through ret 16 matches
    /// one of the supported instruction sequences.
    pub fn constant(&self, index: u32) -> Option<ConstantNativeReturn> {
        self.constants.get(&index).copied()
    }

    pub fn feature_constant(&self, index: u32) -> Option<ConstantNativeReturn> {
        if Self::is_feature_slot(index) {
            return self.constant(index);
        }
        None
    }

    pub const fn is_feature_slot(index: u32) -> bool {
        matches!(index, 906..=933 | 965 | 966 | 1025 | 1027..=1030 | 1047 | 1048)
    }
}

fn sequence_resource_category<'a>(
    wrapper_rva: u32,
    body: &[u8],
    image_base: u32,
    file_bytes_at: impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> Option<&'static [u8]> {
    // Registered 458 resolves and formats the second argument before building
    // the archive path. These forms place its category in EDX or on the stack.
    if !body.starts_with(b"\x81\xec\x08\x01\x00\x00") {
        return None;
    }
    let (operand, call, ret) =
        if body.get(0x46) == Some(&0x68)
            && body.get(0x4b) == Some(&0xba)
            && body.get(0x50..0x54) == Some(b"\x8d\x4c\x24\x10")
        {
            (0x4c, 0x54, 0xca)
        } else if body.get(0x48) == Some(&0x68)
            && body.get(0x4d) == Some(&0x50)
            && body.get(0x4e) == Some(&0x68)
        {
            (0x49, 0x53, 0xcd)
        } else if body.get(0x3f) == Some(&0x68)
            && body.get(0x44..0x49) == Some(b"\x8d\x4c\x24\x08\x51")
            && body.get(0x4f) == Some(&0x68)
        {
            (0x40, 0x54, 0xce)
        } else {
            return None;
        };
    if body.get(ret..ret + 3) != Some(b"\xc2\x10\x00") || body.get(call) != Some(&0xe8) {
        return None;
    }
    let displacement = i32::from_le_bytes(body.get(call + 1..call + 5)?.try_into().ok()?);
    let callee = wrapper_rva.checked_add((call + 5) as u32)?.checked_add_signed(displacement)?;
    file_bytes_at(callee, true)?;
    let category_va = u32::from_le_bytes(body.get(operand..operand + 4)?.try_into().ok()?);
    match file_bytes_at(category_va.checked_sub(image_base)?, false)? {
        bytes if bytes.starts_with(b"image\0") => Some(b"image"),
        bytes if bytes.starts_with(b"config\0") => Some(b"config"),
        _ => None,
    }
}

fn inline_table_va(instructions: &[u8]) -> Option<u32> {
    // Older constructors publish the ordinary table and VM owner inline.
    let [
        0x85,
        0xc0,
        0x74,
        branch,
        0xc7,
        0x40,
        0x38,
        a,
        b,
        c,
        d,
        0x8b,
        0x43,
        vm,
        0x89,
        0x58,
        0x3c,
        context @ ..,
    ] = instructions
    else {
        return None;
    };
    match (branch, vm, context) {
        (0x1f, 0x60, [0x8b, 0x4b, 0x78, 0x8b, 0x43, 0x60])
        | (0x25 | 0x29, 0x70, [0x8b, 0x8b, 0x88, 0, 0, 0]) => {
            Some(u32::from_le_bytes([*a, *b, *c, *d]))
        }
        _ => None,
    }
}

// Match the complete wrapper through ret 16. X/Y/Z capture the caller byte
// offsets written to mode/depth/duration. # is the low byte of the task
// command, ! is the legacy vtable byte, and ? is a relocated address or rel32.
// The other bytes verify the argument pointer, record stores and dispatch.
const DRAW_WRAPPERS: [(&[u8], &[u8]); 2] = [
    (
        b"\x83\xec\x14\xa1????\x33\xc4\x89\x44\x24\x10\x8b\x4c\x24\x20\x56\x8b\x74\x24\x20\x8b\x11\x85\xd2\x74\x3b\
          \xf3\x0f\x10\x41X\x8b\x41Z\xf3\x0f\x11\x44\x24\x08\xf3\x0f\x10\x41Y\
          \x8d\x4c\x24\x04\x51\x6a\x00\x8d\x4c\x24\x10\x89\x44\x24\x18\x8b\x06\x51\x6a\x00\x68#\x01\x00\x00\
          \x52\xff\x76\x04\x8b\xce\xf3\x0f\x11\x44\x24\x28\xff\x50\x10\
          \x8b\x4c\x24\x14\xb8\x02\x00\x00\x00\x5e\x33\xcc\xe8????\x83\xc4\x14\xc2\x10\x00",
        b"\x76\x78\x7b\x7d\x80\x86",
    ),
    (
        b"\x8b\x44\x24\x0c\x8b\x10\x83\xec\x0c\x85\xd2\x74\x3a\
          \x8b\x48Z\xd9\x40X\x56\xd9\x5c\x24\x04\xd9\x40Y\x8d\x74\x24\x1c\x56\xd9\x5c\x24\x0c\
          \x6a\x00\x8d\x74\x24\x0c\x56\x6a\x00\x68#\x01\x00\x00\x89\x4c\x24\x20\
          \x8b\x4c\x24\x2c\x8b\x01\x8b\x40!\x52\x8b\x51\x04\x52\xff\xd0\
          \x5e\xb8\x02\x00\x00\x00\x83\xc4\x0c\xc2\x10\x00",
        b"\x35\x46\x4c\x5d\x61\x63\x68\x6f\x74",
    ),
];

const WIPE_WRAPPERS: [(&[u8], &[u8]); 4] = [
    (
        b"\x83\xec\x14\x56\x8b\x74\x24\x24\x83\x3e\x00\x8b\x4e\x10\x74\x53\x85\xc9\x74\x4f\
          \xf3\x0f\x10\x46X\x8b\x46Z\xf3\x0f\x11\x44\x24\x04\xf3\x0f\x10\x46Y\x89\x44\x24\x0c\xf3\x0f\x11\x44\x24\x08\
          \xe8????\x8b\x4c\x24\x20\x8d\x54\x24\x24\x52\x6a\x00\x8d\x54\x24\x0c\x89\x44\x24\x18\x8b\x01\x52\x6a\x00\x68#\x01\x00\x00\
          \xff\x36\xc7\x44\x24\x2c\x00\x00\x00\x00\xff\x71\x04\xff\x50\x10\
          \xb8\x02\x00\x00\x00\x5e\x83\xc4\x14\xc2\x10\x00",
        b"\x79\x7b\x7e\x80\x83\x89",
    ),
    (
        b"\x8b\x44\x24\x0c\x8b\x10\x8b\x48\x10\x83\xec\x10\x85\xd2\x74\x44\x85\xc9\x74\x40\
          \xd9\x40X\x56\x8b\x70Z\xd9\x5c\x24\x04\xd9\x40Y\x89\x74\x24\x0c\x8d\x74\x24\x20\xd9\x5c\x24\x08\
          \x8b\x01\x8b\x4c\x24\x1c\x56\x6a\x00\x8d\x74\x24\x0c\x56\x6a\x00\x68#\x01\x00\x00\x89\x44\x24\x24\
          \x8b\x01\x8b\x40!\x52\x8b\x51\x04\x52\xff\xd0\x5e\xb8\x02\x00\x00\x00\x83\xc4\x10\xc2\x10\x00",
        b"\x38\x49\x4f\x60\x64\x66",
    ),
    (
        b"\x83\xec\x14\x56\x8b\x74\x24\x24\x83\x3e\x00\x8b\x4e\x10\x74\x4f\x85\xc9\x74\x4b\
          \xd9\x46X\x8b\x46Z\xd9\x5c\x24\x04\x89\x44\x24\x0c\xd9\x46Y\xd9\x5c\x24\x08\
          \xe8????\x8b\x4c\x24\x20\x8b\x11\x8b\x52\x10\x89\x44\x24\x10\
          \x8d\x44\x24\x24\x50\x6a\x00\x8d\x44\x24\x0c\x50\x8b\x06\x6a\x00\x68#\x01\x00\x00\
          \x50\x8b\x41\x04\x50\xc7\x44\x24\x30\x00\x00\x00\x00\xff\xd2\xb8\x02\x00\x00\x00\x5e\x83\xc4\x14\xc2\x10\x00",
        b"\x72\x77",
    ),
    (
        b"\x83\xec\x10\x56\x8b\x74\x24\x20\x83\x3e\x00\x8b\x4e\x10\x74\x47\x85\xc9\x74\x43\
          \xd9\x46X\x8b\x46Z\xd9\x5c\x24\x04\x89\x44\x24\x0c\xd9\x46Y\xd9\x5c\x24\x08\
          \xe8????\x8b\x4c\x24\x1c\x8b\x11\x8b\x52\x10\x89\x44\x24\x10\
          \x8d\x44\x24\x20\x50\x6a\x00\x8d\x44\x24\x0c\x50\x8b\x06\x6a\x00\x68#\x01\x00\x00\
          \x50\x8b\x41\x04\x50\xff\xd2\xb8\x02\x00\x00\x00\x5e\x83\xc4\x10\xc2\x10\x00",
        b"\x6b",
    ),
];

fn render_wrapper_order(body: &[u8], wrappers: &[(&[u8], &[u8])]) -> Option<RenderArgumentOrder> {
    for &(pattern, commands) in wrappers {
        let Some(bytes) = body.get(..pattern.len()) else {
            continue;
        };
        let mut offsets = [0; 3];
        let mut command = 0;
        let valid = pattern
            .iter()
            .zip(bytes)
            .all(|(&expected, &byte)| match expected {
                b'?' => true,
                b'#' => {
                    command = byte;
                    commands.contains(&command)
                }
                b'!' => byte == if command <= 0x49 { 0x0c } else { 0x10 },
                b'X'..=b'Z' => {
                    offsets[usize::from(expected - b'X')] = byte;
                    true
                }
                _ => byte == expected,
            });
        if valid {
            return match offsets {
                [8, 12, 4] => Some(RenderArgumentOrder::DurationFirst),
                [4, 8, 12] => Some(RenderArgumentOrder::RecordOrder),
                _ => None,
            };
        }
    }
    None
}

fn constant_stub(body: &[u8]) -> Option<ConstantNativeReturn> {
    match body {
        [0xb8, 0x02, 0, 0, 0, 0xc2, 0x10, 0, ..] => Some(ConstantNativeReturn::Continue),
        [
            0x8b,
            0x44,
            0x24,
            0x10,
            0xc7,
            0,
            a,
            b,
            c,
            d,
            0xb8,
            0x01,
            0,
            0,
            0,
            0xc2,
            0x10,
            0,
            ..,
        ] => Some(ConstantNativeReturn::Value(u32::from_le_bytes([
            *a, *b, *c, *d,
        ]))),
        _ => None,
    }
}

fn byte_string_splitter<'a>(
    wrapper_rva: u32,
    body: &[u8],
    code_at: impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<()> {
    let body = body.get(..STRING_SPLIT_WRAPPER.len())?;
    if !STRING_SPLIT_WRAPPER
        .iter()
        .zip(body)
        .all(|(&expected, &actual)| expected == b'?' || expected == actual)
    {
        return None;
    }

    let call_target = |offset: usize| {
        let displacement = i32::from_le_bytes(body.get(offset + 1..offset + 5)?.try_into().ok()?);
        wrapper_rva
            .checked_add(u32::try_from(offset + 5).ok()?)?
            .checked_add_signed(displacement)
    };
    let resolver = call_target(0x10)?;
    if resolver != call_target(0x1c)? || resolver != call_target(0x28)? {
        return None;
    }
    code_at(resolver)?;

    let splitter = call_target(0x30)?;
    (resolver != splitter && code_at(splitter)?.starts_with(BYTE_STRING_SPLITTER)).then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u32 = 0x40_0000;
    const CODE_RVA: u32 = 0x1000;
    const TABLE_RVA: u32 = 0x4000;
    const COFF: usize = 0x84;
    const OPTIONAL: usize = COFF + 20;
    const SECTIONS: usize = OPTIONAL + 0xe0;
    const CODE: usize = 0x200;
    const TABLE: usize = 0x400;
    const TABLE_BYTES: usize = (LAST_TRACKED_SLOT + 1) * 4;
    const REGISTER: usize = 0x80;

    fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn set_slot(data: &mut [u8], index: usize, entry_va: u32) {
        write_u32(data, TABLE + index * 4, entry_va);
    }

    fn set_registration(data: &mut [u8], offset: usize, table_va: u32) {
        data[CODE + offset..CODE + offset + REGISTRATION_LEN]
            .copy_from_slice(b"\x68\0\0\0\0\x8b\xc8\xe8\0\0\0\0");
        write_u32(data, CODE + offset + 1, table_va);
        let displacement = -(offset as i32 + REGISTRATION_LEN as i32);
        data[CODE + offset + 8..CODE + offset + REGISTRATION_LEN]
            .copy_from_slice(&displacement.to_le_bytes());
    }

    fn set_rel32_call(data: &mut [u8], wrapper: usize, call: usize, target: usize) {
        let next = CODE_RVA + wrapper as u32 + call as u32 + 5;
        let target = CODE_RVA + target as u32;
        let displacement = (target as i64 - next as i64) as i32;
        let start = CODE + wrapper + call + 1;
        data[start..start + 4].copy_from_slice(&displacement.to_le_bytes());
    }

    fn install_bytewise_string_splitter(data: &mut [u8]) -> (usize, usize) {
        let wrapper_offset = 0x90;
        let resolver_offset = 0x1a0;
        let splitter_offset = 0x100;
        let mut wrapper = STRING_SPLIT_WRAPPER
            .iter()
            .map(|&byte| if byte == b'?' { 0 } else { byte })
            .collect::<Vec<_>>();
        for call in [0x10, 0x1c, 0x28, 0x30] {
            wrapper[call] = 0xe8;
        }
        let wrapper_start = CODE + wrapper_offset;
        data[wrapper_start..wrapper_start + wrapper.len()].copy_from_slice(&wrapper);
        for call in [0x10, 0x1c, 0x28] {
            set_rel32_call(data, wrapper_offset, call, resolver_offset);
        }
        set_rel32_call(data, wrapper_offset, 0x30, splitter_offset);
        let splitter_start = CODE + splitter_offset;
        data[splitter_start..splitter_start + BYTE_STRING_SPLITTER.len()]
            .copy_from_slice(BYTE_STRING_SPLITTER);
        data[CODE + resolver_offset..CODE + resolver_offset + 3]
            .copy_from_slice(b"\x8b\x01\xc3");
        set_slot(data, 41, BASE + CODE_RVA + wrapper_offset as u32);
        (splitter_start, wrapper_start)
    }

    fn sample_pe() -> Vec<u8> {
        let mut bytes = vec![0; TABLE + TABLE_BYTES * 2];
        bytes[..2].copy_from_slice(b"MZ");
        write_u32(&mut bytes, 0x3c, 0x80);
        bytes[0x80..COFF].copy_from_slice(b"PE\0\0");
        bytes[COFF..COFF + 2].copy_from_slice(&0x014c_u16.to_le_bytes());
        bytes[COFF + 2..COFF + 4].copy_from_slice(&2_u16.to_le_bytes());
        bytes[COFF + 16..COFF + 18].copy_from_slice(&0xe0_u16.to_le_bytes());
        bytes[OPTIONAL..OPTIONAL + 2].copy_from_slice(&0x010b_u16.to_le_bytes());
        write_u32(&mut bytes, OPTIONAL + 28, BASE);
        for (header, rva, raw, size, flags) in [
            (SECTIONS, CODE_RVA, CODE, TABLE - CODE, 0x6000_0020),
            (
                SECTIONS + 40,
                TABLE_RVA,
                TABLE,
                TABLE_BYTES * 2,
                0xc000_0040,
            ),
        ] {
            write_u32(&mut bytes, header + 8, size as u32);
            write_u32(&mut bytes, header + 12, rva);
            write_u32(&mut bytes, header + 16, size as u32);
            write_u32(&mut bytes, header + 20, raw as u32);
            write_u32(&mut bytes, header + 36, flags);
        }
        bytes[CODE..CODE + VM_TABLE_SETTER.len()].copy_from_slice(VM_TABLE_SETTER);
        bytes[CODE + 0x20..CODE + 0x28].copy_from_slice(b"\xb8\x02\0\0\0\xc2\x10\0");
        bytes[CODE + 0x40..CODE + 0x52]
            .copy_from_slice(b"\x8b\x44\x24\x10\xc7\0\x33\x01\xd0\xae\xb8\x01\0\0\0\xc2\x10\0");
        bytes[CODE + 0x60..CODE + 0x62].copy_from_slice(b"\xeb\xbe");
        bytes[TABLE - 4..TABLE].copy_from_slice(b"\xb8\x02\0\0");
        for index in 0..=LAST_TRACKED_SLOT {
            set_slot(&mut bytes, index, BASE + CODE_RVA + 0x20);
        }
        set_slot(&mut bytes, 925, BASE + CODE_RVA + 0x40);
        set_slot(&mut bytes, 926, BASE + CODE_RVA + 0x60);
        set_slot(&mut bytes, 927, BASE + CODE_RVA + (TABLE - CODE - 4) as u32);
        set_registration(&mut bytes, REGISTER, BASE + TABLE_RVA);
        bytes
    }

    #[test]
    fn ordinary_table_uses_complete_stubs_and_the_actual_dword() {
        let mut pe = sample_pe();
        pe.extend_from_slice(b"unrelated overlay");
        let table = OrdinaryNativeTable::from_pe(&pe).unwrap();
        assert_eq!(
            table.feature_constant(925),
            Some(ConstantNativeReturn::Value(0xaed0_0133))
        );
        assert_eq!(table.constant(439), Some(ConstantNativeReturn::Continue));
        assert_eq!(table.feature_constant(439), None);
        for index in [926, 927, 1049] {
            assert_eq!(table.feature_constant(index), None);
        }
    }

    #[test]
    fn native41_byte_split_mode_requires_its_registered_wrapper_and_helper() {
        let mut pe = sample_pe();
        let (splitter, _) = install_bytewise_string_splitter(&mut pe);
        let table = OrdinaryNativeTable::from_pe(&pe).unwrap();
        assert!(table.string_split_uses_bytes());

        pe[splitter + 0x39] ^= 1;
        let table = OrdinaryNativeTable::from_pe(&pe).unwrap();
        assert!(!table.string_split_uses_bytes());

        let mut pe = sample_pe();
        let (_, wrapper) = install_bytewise_string_splitter(&mut pe);
        pe[wrapper + 0x08] ^= 1;
        let table = OrdinaryNativeTable::from_pe(&pe).unwrap();
        assert!(!table.string_split_uses_bytes());
    }

    #[test]
    fn native41_byte_split_mode_comes_from_installed_pe_structure() -> std::io::Result<()> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        for (path, expected) in [
            ("cs2_full_v401/system/cs2_open.exe", true),
            ("Yukikoi Melt/YukikoiMelt.exe", false),
        ] {
            let path = root.join(path);
            if !path.is_file() {
                return Ok(());
            }
            let mut pe = std::fs::read(path)?;
            let table = OrdinaryNativeTable::from_pe(&pe).expect("ordinary registered table");
            assert_eq!(table.string_split_uses_bytes(), expected);
            if expected {
                pe.extend_from_slice(b"unrelated overlay");
                let table = OrdinaryNativeTable::from_pe(&pe).expect("overlay keeps table");
                assert!(table.string_split_uses_bytes());
            }
        }
        Ok(())
    }

    #[test]
    fn fes_control_wrappers_come_from_the_registered_argument_layout() -> std::io::Result<()> {
        let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        for (file, expected) in [
            ("cs2_full_v401/system/cs2_open.exe", [Some(330), Some(342)]),
            ("Yukikoi Melt/YukikoiMelt.exe", [Some(341), Some(353)]),
            ("\u{30c4}\u{30ca}\u{30d0}\u{30f3} \u{3089}\u{3076}\u{307f}\u{304f}\u{3059}/cs2.exe", [Some(286), None]),
            ("\u{7070}\u{8272}\u{7684}\u{8ff7}\u{5bab}/Grisaia2.exe", [Some(313), Some(325)]),
            ("\u{3046}\u{305f}\u{3066}\u{3081}\u{3050}\u{308a}/UTATEMEGURI.exe", [Some(298), Some(310)]),
            ("\u{795d}\u{798f}\u{306e}\u{30ab}\u{30f3}\u{30d1}\u{30cd}\u{30e9}/cs2.exe", [Some(292), Some(304)]),
            ("\u{7a81}\u{7136}\u{4e4b}\u{95f4}\u{53d1}\u{73b0}\u{6211}\u{5df2}\u{604b}\u{4e0a}\u{4f60}/cs2_cn.exe", [Some(308), Some(320)]),
        ] {
            let path = games.join(file);
            if !path.is_file() {
                continue;
            }
            let mut pe = std::fs::read(path)?;
            for extra in [b"".as_slice(), b"unrelated overlay"] {
                pe.extend_from_slice(extra);
                let table = OrdinaryNativeTable::from_pe(&pe).expect("registered ordinary table");
                assert_eq!(table.fes_control_wrappers(), expected, "{file}");
            }
        }
        Ok(())
    }

    #[test]
    fn fes_control_wrappers_follow_the_effective_startup_companion() -> crate::RuntimeResult<()> {
        let folder = "\u{7a81}\u{7136}\u{4e4b}\u{95f4}\u{53d1}\u{73b0}\u{6211}\u{5df2}\u{604b}\u{4e0a}\u{4f60}";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../games").join(folder);
        if !path.is_dir() {
            return Ok(());
        }
        let scan = crate::GameRoot::open(path)?.scan()?;
        let entry = "main.kcs";
        let primary = OrdinaryNativeTable::from_game_scan(&scan, Some(entry))
            .map(|table| table.fes_control_wrappers());
        let companion = OrdinaryNativeTable::startup_companion_natives(&scan, entry)
            .map(|table| table.fes_control_wrappers);
        assert_eq!(primary.or(companion), Some([Some(308), Some(320)]));
        Ok(())
    }

    #[test]
    fn immediate_task_message_is_recognized_from_its_registered_wrapper() {
        let mut pe = sample_pe();
        let wrapper = 0x180;
        let start = CODE + wrapper;
        let command = start + IMMEDIATE_TASK_MESSAGE_PREFIX.len();
        let tail = command + 4;
        pe[start..command].copy_from_slice(IMMEDIATE_TASK_MESSAGE_PREFIX);
        pe[tail..tail + IMMEDIATE_TASK_MESSAGE_TAIL.len()].copy_from_slice(IMMEDIATE_TASK_MESSAGE_TAIL);
        set_slot(&mut pe, 546, BASE + CODE_RVA + wrapper as u32);
        for (code, expected) in [(341u32, Some(341)), (338, Some(338)), (339, None)] {
            pe[command..tail].copy_from_slice(&code.to_le_bytes());
            let table = OrdinaryNativeTable::from_pe(&pe).unwrap();
            assert_eq!(table.immediate_task_message(546), expected);
            assert_eq!(table.immediate_task_message(545), None);
        }

        pe[command..tail].copy_from_slice(&338u32.to_le_bytes());
        pe[tail] ^= 1;
        let table = OrdinaryNativeTable::from_pe(&pe).unwrap();
        assert_eq!(table.immediate_task_message(546), None);
    }

    #[test]
    fn legacy_3d_immediate_messages_are_identified_from_the_registered_wrappers() -> std::io::Result<()> {
        let game = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(concat!(
            "../../../games/",
            "\u{795d}\u{798f}\u{306e}\u{30ab}\u{30f3}\u{30d1}\u{30cd}\u{30e9}/cs2.exe",
        ));
        if !game.is_file() {
            return Ok(());
        }

        let pe = std::fs::read(game)?;
        let table = OrdinaryNativeTable::from_pe(&pe).expect("Campanella ordinary native table");
        assert_eq!(table.immediate_task_message(546), Some(341));
        assert_eq!(table.immediate_task_message(548), Some(338));
        assert_eq!(table.immediate_task_message(581), Some(355));
        assert_eq!(table.immediate_task_message(673), Some(359));
        assert_eq!(table.immediate_task_message(545), None);
        assert!(table.has_camera_direction_query());
        assert!(table.has_model_position_query());
        assert_eq!(table.model_local_transform_query_message(671), Some(370));
        assert_eq!(table.model_local_transform_query_message(672), Some(369));
        assert_eq!(table.model_visibility_message(550), Some(336));
        assert_eq!(table.model_visibility_message(551), Some(337));
        assert!(table.has_model_clear_overrides());
        assert!(table.has_model_freeze_transform());
        assert_eq!(table.model_motion_message(556), Some(364));
        assert_eq!(table.model_motion_message(562), Some(348));
        assert_eq!(table.model_motion_message(644), Some(362));
        assert_eq!(table.model_transform_set_message(560), Some(344));
        assert_eq!(table.model_transform_set_message(642), Some(345));
        assert_eq!(table.model_transform_set_message(561), Some(346));
        assert_eq!(table.model_transform_set_message(643), Some(347));
        assert!(table.has_camera_relative_position());
        Ok(())
    }

    #[test]
    fn registered_camera_direction_requires_the_same_vm_resolver_for_both_outputs() {
        let mut pe = sample_pe();
        let wrapper = 0xb0;
        let resolver = 0x170;
        let start = CODE + wrapper;
        pe[start..start + CAMERA_DIRECTION_WRAPPER.len()].copy_from_slice(CAMERA_DIRECTION_WRAPPER);
        let start = CODE + resolver;
        pe[start..start + CAMERA_VM_ADDRESS_RESOLVER.len()]
            .copy_from_slice(CAMERA_VM_ADDRESS_RESOLVER);
        set_rel32_call(&mut pe, wrapper, 0x17, resolver);
        set_rel32_call(&mut pe, wrapper, 0x27, resolver);
        set_slot(&mut pe, 661, BASE + CODE_RVA + wrapper as u32);
        assert!(OrdinaryNativeTable::from_pe(&pe).unwrap().has_camera_direction_query());

        set_rel32_call(&mut pe, wrapper, 0x27, resolver + 1);
        assert!(!OrdinaryNativeTable::from_pe(&pe).unwrap().has_camera_direction_query());
        set_rel32_call(&mut pe, wrapper, 0x27, resolver);
        pe[CODE + wrapper + 0x6a] ^= 1;
        assert!(!OrdinaryNativeTable::from_pe(&pe).unwrap().has_camera_direction_query());
    }

    #[test]
    fn registered_legacy_slot_700_queries_the_display_rectangle() -> std::io::Result<()> {
        use std::path::Path;
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let old = games.join("\u{3046}\u{305f}\u{3066}\u{3081}\u{3050}\u{308a}/UTATEMEGURI.exe");
        if old.is_file() {
            if old.with_file_name("boot.dfn").is_file() {
                let scan = crate::GameRoot::open(old.parent().unwrap()).unwrap().scan().unwrap();
                assert!(OrdinaryNativeTable::from_game_scan(&scan, Some("kcs.int/main.kcs"))
                    .expect("ordinary table for the installed game")
                    .has_legacy_display_rectangle());
            }
            let mut bytes = std::fs::read(old)?;
            let registered = |pe: &[u8]| OrdinaryNativeTable::from_pe(pe).expect("ordinary table");
            assert!(registered(&bytes).has_legacy_display_rectangle());
            bytes.extend_from_slice(b"overlay");
            assert!(registered(&bytes).has_legacy_display_rectangle());
            let wrapper = bytes.windows(LEGACY_DISPLAY_RECTANGLE_WRAPPER.len())
                .position(|bytes| LEGACY_DISPLAY_RECTANGLE_WRAPPER.iter().zip(bytes)
                    .all(|(&expected, &actual)| expected == b'?' || expected == actual))
                .expect("registered legacy wrapper");
            bytes[wrapper + 0x23] ^= 1;
            assert!(!registered(&bytes).has_legacy_display_rectangle());
        }
        let modern = games.join("Yukikoi Melt/YukikoiMelt.exe");
        if modern.is_file() {
            assert!(!OrdinaryNativeTable::from_pe(&std::fs::read(modern)?)
                .expect("ordinary table").has_legacy_display_rectangle());
        }
        Ok(())
    }

    #[test]
    fn registered_legacy_affine_and_motion_layouts_survive_pe_overlays() -> std::io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let legacy = [
            "\u{3046}\u{305f}\u{3066}\u{3081}\u{3050}\u{308a}/UTATEMEGURI.exe",
            "\u{30c4}\u{30ca}\u{30d0}\u{30f3} \u{3089}\u{3076}\u{307f}\u{304f}\u{3059}/cs2.exe",
            "\u{795d}\u{798f}\u{306e}\u{30ab}\u{30f3}\u{30d1}\u{30cd}\u{30e9}/cs2.exe",
        ];
        let check = |table: OrdinaryNativeTable, expected| {
            for index in 626..=627 {
                assert_eq!(table.has_legacy_list_affine(index), expected, "list {index}");
            }
            for index in 299..=302 {
                assert_eq!(table.has_short_motion_properties(index), expected, "motion {index}");
            }
        };
        for relative in legacy {
            let path = games.join(relative);
            if !path.is_file() {
                continue;
            }
            let scan = crate::GameRoot::open(path.parent().unwrap()).unwrap().scan().unwrap();
            check(OrdinaryNativeTable::from_game_executable(&scan).expect(relative), true);
            let mut pe = std::fs::read(path)?;
            pe.extend_from_slice(b"overlay");
            check(OrdinaryNativeTable::from_pe(&pe).expect(relative), true);
        }
        let modern = games.join("Yukikoi Melt/YukikoiMelt.exe");
        if modern.is_file() {
            let pe = std::fs::read(modern)?;
            check(OrdinaryNativeTable::from_pe(&pe).expect("modern table"), false);
        }
        check(OrdinaryNativeTable::from_pe(&sample_pe()).unwrap(), false);
        Ok(())
    }

    #[test]
    fn ordinary_table_rejects_wrong_pe_or_registration() {
        let original = sample_pe();
        for (offset, value) in [
            (COFF, 0x64),
            (OPTIONAL, 0x0b ^ 1),
            (CODE + 7, 0x90),
            (CODE + REGISTER + 6, 0xc9),
        ] {
            let mut pe = original.clone();
            pe[offset] = value;
            assert!(OrdinaryNativeTable::from_pe(&pe).is_none());
        }
        let mut pe = original.clone();
        set_registration(&mut pe, REGISTER, BASE - 1);
        assert!(OrdinaryNativeTable::from_pe(&pe).is_none());
        let mut pe = original;
        set_slot(&mut pe, 17, BASE + TABLE_RVA);
        assert!(OrdinaryNativeTable::from_pe(&pe).is_none());
    }

    #[test]
    fn native1074_uses_the_registered_slot_in_installed_pe_samples() -> std::io::Result<()> {
        let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        for (relative, table_rva) in [
            ("\u{767d}\u{663c}\u{5922}\u{306e}\u{9752}\u{5199}\u{771f}/cs2.exe", 0x6250e8),
            ("Yukikoi Melt/YukikoiMelt.exe", 0x6270e8),
        ] {
            let path = games.join(relative);
            if !path.is_file() {
                continue;
            }
            let mut pe = std::fs::read(path)?;
            let read_table = |pe: &[u8]| OrdinaryNativeTable::from_pe(pe).expect("ordinary table");
            assert!(read_table(&pe).task_clock_supports_single_step(), "{relative}");

            pe.extend_from_slice(b"unrelated overlay");
            assert!(read_table(&pe).task_clock_supports_single_step(), "{relative}");

            let slots = parse_sections(&pe).unwrap().into_iter().find_map(|section| {
                section.file_bytes_from(&pe, table_rva + 1073 * 4)
            }).expect("file-backed ordinary table");
            let prior_slot = slots.as_ptr() as usize - pe.as_ptr() as usize;
            pe.copy_within(prior_slot..prior_slot + 4, prior_slot + 4);
            assert!(!read_table(&pe).task_clock_supports_single_step(), "{relative}");
        }
        Ok(())
    }

    #[test]
    fn legacy_native439_uses_the_registered_constant_in_installed_pe_samples() -> std::io::Result<()> {
        let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        for relative in [
            "\u{6226}\u{56fd}\u{5929}\u{4f7f}\u{30b8}\u{30d6}\u{30ea}\u{30fc}\u{30eb}/Djibril5.exe",
            "\u{7070}\u{8272}\u{7684}\u{679c}\u{5b9e}/Grisaia.exe",
        ] {
            let path = games.join(relative);
            if !path.is_file() {
                continue;
            }
            let mut pe = std::fs::read(path)?;
            pe.extend_from_slice(b"unrelated overlay");
            let table = OrdinaryNativeTable::from_pe(&pe).expect(relative);
            assert_eq!(table.constant(439), Some(ConstantNativeReturn::Continue), "{relative}");
        }
        Ok(())
    }

    #[test]
    fn ordinary_table_rejects_ambiguous_tables_but_accepts_repeated_registration() {
        let mut pe = sample_pe();
        set_registration(&mut pe, REGISTER + 0x10, BASE + TABLE_RVA);
        assert!(OrdinaryNativeTable::from_pe(&pe).is_some());
        pe.copy_within(TABLE..TABLE + TABLE_BYTES, TABLE + TABLE_BYTES);
        set_registration(
            &mut pe,
            REGISTER + 0x10,
            BASE + TABLE_RVA + TABLE_BYTES as u32,
        );
        assert!(OrdinaryNativeTable::from_pe(&pe).is_none());
    }

    #[test]
    fn sequence_resource_category_comes_from_registered_wrapper() -> std::io::Result<()> {
        let samples = [
            ("\u{30c4}\u{30ca}\u{30d0}\u{30f3} \u{3089}\u{3076}\u{307f}\u{304f}\u{3059}/cs2.exe", &b"image"[..], 0x47800, 0x4c),
            ("ISLAND/ISLAND.exe", &b"config"[..], 0x114290, 0x49),
            ("\u{7070}\u{8272}\u{7684}\u{4e50}\u{56ed}v1/Grisaia3.exe", &b"config"[..], 0xcf260, 0x40),
        ];
        let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        for (relative, category, wrapper, operand) in samples {
            let path = games.join(relative);
            if !path.is_file() {
                continue;
            }
            let mut pe = std::fs::read(path)?;
            assert_eq!(
                OrdinaryNativeTable::from_pe(&pe).unwrap().sequence_resource_category(),
                Some(category),
                "{relative}",
            );
            let (base, _) = x86_image_layout(&pe).unwrap();
            let sections = parse_sections(&pe).unwrap();
            let offset_at = |rva| {
                sections.iter().find_map(|section| {
                    section.file_bytes_from(&pe, rva)
                        .map(|bytes| bytes.as_ptr() as usize - pe.as_ptr() as usize)
                }).unwrap()
            };
            let operand = offset_at(wrapper + operand);
            let category_va = u32::from_le_bytes(pe[operand..operand + 4].try_into().unwrap());
            let category_at = offset_at(category_va - base);
            pe[category_at..category_at + 6].copy_from_slice(b"image\0");
            assert_eq!(
                OrdinaryNativeTable::from_pe(&pe).unwrap().sequence_resource_category(),
                Some(&b"image"[..]),
            );
            pe[category_at] = b'x';
            assert_eq!(OrdinaryNativeTable::from_pe(&pe).unwrap().sequence_resource_category(), None);
        }
        Ok(())
    }

    #[test]
    fn flat_text_requires_inline_registration_and_verified_render_and_fill() {
        let mut pe = sample_pe();
        pe[SECTIONS..SECTIONS + 8].copy_from_slice(b".text\0\0\0");
        write_u32(&mut pe, SECTIONS + 12, CODE as u32);
        write_u32(&mut pe, SECTIONS + 36, 0xc000_0020);
        write_u32(&mut pe, OPTIONAL + 16, CODE as u32);
        for index in 0..=LAST_TRACKED_SLOT {
            set_slot(&mut pe, index, BASE + CODE as u32 + 0x20);
        }
        assert!(OrdinaryNativeTable::from_pe(&pe).is_none());

        pe[CODE + REGISTER..CODE + REGISTER + INLINE_REGISTRATION_LEN].copy_from_slice(
            b"\x85\xc0\x74\x1f\xc7\x40\x38\0\0\0\0\x8b\x43\x60\x89\x58\x3c\x8b\x4b\x78\x8b\x43\x60",
        );
        write_u32(&mut pe, CODE + REGISTER + 7, BASE + TABLE_RVA);
        assert!(OrdinaryNativeTable::from_pe(&pe).is_none());
    }

    #[test]
    fn localized_flat_text_exposes_only_verified_native_slots()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::path::Path;

        let relative_game = Path::new(concat!(
            "../../../games/",
            "\u{7a81}\u{7136}\u{4e4b}\u{95f4}\u{53d1}\u{73b0}\u{6211}\u{5df2}\u{604b}\u{4e0a}\u{4f60}",
        ));
        let game = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_game);
        let localized = game.join("cs2_cn.exe");
        if !localized.is_file() {
            return Ok(());
        }
        let pe = std::fs::read(localized)?;
        let table = OrdinaryNativeTable::from_pe(&pe).unwrap();
        for index in [527, 530] {
            assert_eq!(
                table.render_argument_order(index),
                Some(RenderArgumentOrder::DurationFirst)
            );
        }
        assert!(table.image_fill_uses_current_pixels());
        assert_eq!(table.constant(0), None);
        assert_eq!(table.constant(429), None);
        assert_eq!(table.constant(439), Some(ConstantNativeReturn::Continue));

        // Redirect 439 to the adjacent nonconstant wrapper. The ordinary
        // table remains valid, but 439 loses its verified continuation.
        let mut changed = pe.clone();
        const ORDINARY_TABLE_OFFSET: usize = 0x361a78;
        let slot_438 = ORDINARY_TABLE_OFFSET + 438 * 4;
        changed.copy_within(slot_438..slot_438 + 4, slot_438 + 4);
        let changed_table = OrdinaryNativeTable::from_pe(&changed).unwrap();
        assert_eq!(changed_table.constant(439), None);

        let (_, entry_rva) = x86_image_layout(&pe).unwrap();
        let sections = parse_sections(&pe).unwrap();
        let text = sections
            .into_iter()
            .find(|section| section.is_flat_text_code())
            .unwrap();
        assert!(text.file_bytes_from(&pe, entry_rva).is_some());

        // The localized image maps these verified instructions 1:1 to disk.
        for changed_offset in [0x53989, 0x79fa0, 0x7a080, 0x777d0, 0xf1800 + 0xa5] {
            let mut changed = pe.clone();
            changed[changed_offset] ^= 1;
            assert!(
                OrdinaryNativeTable::from_pe(&changed).is_none(),
                "{changed_offset:#x}"
            );
        }
        let packed = game.join("cs2.exe");
        if packed.is_file() {
            assert!(OrdinaryNativeTable::from_pe(&std::fs::read(packed)?).is_none());

            for game in [game.canonicalize()?.as_path(), relative_game] {
                let scan = crate::GameRoot::open(game)?.scan()?;
                assert_eq!(
                    scan.engine
                        .primary_executable
                        .as_ref()
                        .unwrap()
                        .relative_path,
                    Path::new("cs2.exe")
                );
                assert!(OrdinaryNativeTable::from_game_scan(&scan, None).is_none());
                assert!(OrdinaryNativeTable::from_game_scan(&scan, Some("startup.xml")).is_none());
                let table = OrdinaryNativeTable::from_game_scan(&scan, Some("kcs.int/main.kcs"))
                    .expect("startup archive binds the localized companion");
                for index in [527, 530] {
                    assert_eq!(
                        table.render_argument_order(index),
                        Some(RenderArgumentOrder::DurationFirst)
                    );
                }
                assert!(table.image_fill_uses_current_pixels());
                assert_eq!(table.constant(0), None);
                assert_eq!(table.constant(439), Some(ConstantNativeReturn::Continue));
            }
        }
        Ok(())
    }

    #[test]
    fn ordinary_table_recovers_both_render_slots_independently() {
        let mut pe = sample_pe();
        pe[CODE + REGISTER..CODE + REGISTER + INLINE_REGISTRATION_LEN].copy_from_slice(
            b"\x85\xc0\x74\x1f\xc7\x40\x38\0\0\0\0\x8b\x43\x60\x89\x58\x3c\x8b\x4b\x78\x8b\x43\x60",
        );
        write_u32(&mut pe, CODE + REGISTER + 7, BASE + TABLE_RVA);
        let draw = b"\x8b\x44\x24\x0c\x8b\x10\x83\xec\x0c\x85\xd2\x74\x3a\x8b\x48\x04\xd9\x40\x08\
            \x56\xd9\x5c\x24\x04\xd9\x40\x0c\x8d\x74\x24\x1c\x56\xd9\x5c\x24\x0c\x6a\x00\x8d\x74\x24\x0c\x56\x6a\x00\
            \x68\x68\x01\x00\x00\x89\x4c\x24\x20\x8b\x4c\x24\x2c\x8b\x01\x8b\x40\x10\x52\x8b\x51\x04\x52\xff\xd0\
            \x5e\xb8\x02\x00\x00\x00\x83\xc4\x0c\xc2\x10\x00";
        let draw_offset = 0xa0;
        pe[CODE + draw_offset..CODE + draw_offset + draw.len()].copy_from_slice(draw);
        set_slot(&mut pe, 527, BASE + CODE_RVA + draw_offset as u32);
        let (pattern, commands) = WIPE_WRAPPERS[0];
        let mut wipe: Vec<_> = pattern
            .iter()
            .map(|&byte| match byte {
                b'X' => 4,
                b'Y' => 8,
                b'Z' => 12,
                b'#' => commands[0],
                b'?' => 0x7f,
                _ => byte,
            })
            .collect();
        let wipe_offset = 0x120;
        pe[CODE + wipe_offset..CODE + wipe_offset + wipe.len()].copy_from_slice(&wipe);
        set_slot(&mut pe, 530, BASE + CODE_RVA + wipe_offset as u32);
        let table = OrdinaryNativeTable::from_pe(&pe).unwrap();
        assert_eq!(
            table.render_argument_order(527),
            Some(RenderArgumentOrder::DurationFirst)
        );
        assert_eq!(
            table.render_argument_order(530),
            Some(RenderArgumentOrder::RecordOrder)
        );

        pe[CODE + draw_offset + 44] = 0xff;
        let table = OrdinaryNativeTable::from_pe(&pe).unwrap();
        assert_eq!(table.render_argument_order(527), None);
        assert_eq!(
            table.render_argument_order(530),
            Some(RenderArgumentOrder::RecordOrder)
        );

        assert_eq!(
            render_wrapper_order(&wipe[..wipe.len() - 1], &WIPE_WRAPPERS),
            None
        );
        let command_offset = pattern.iter().position(|&byte| byte == b'#').unwrap();
        wipe[command_offset] = 0x68; // Draw command, through the Wipe wrapper shape.
        assert_eq!(render_wrapper_order(&wipe, &WIPE_WRAPPERS), None);
        pe[CODE + REGISTER + 16] ^= 1; // Remove the inline owner publication.
        assert!(OrdinaryNativeTable::from_pe(&pe).is_none());
    }
}
