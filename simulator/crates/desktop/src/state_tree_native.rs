//! Original 688/689 -> live kcTask3DView transform-tree receiver.
//! This is separate from the explicitly installed portable whole-host graph.
use cs2_formats::StateTree;
use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_runtime::model_state::{LegacyModelState, ModelStateNode};
use cs2_runtime::model_state::{decode_local_transform, encode_local_transform, IDENTITY_MODEL_TRANSFORM};
use cs2_script::{NativeReturn, Result, Vm};

use super::super::{LoggingHost, native_state_error, read_u32};
use super::{NativeTaskStore, OwnedTaskBuffers};

const LEGACY: &str = "659d5f069f3c97f69a61e12449cf1510b45e48a10649b9ac6977c2b93d6d28ad";

// Fixed ordinary tables, complete wrapper branch/7-push evidence:
// archive/2026-11/20261117_C_state_wrapper_batch.json. These describe the
// wrapper, never a positive kind43 constructor or a portable graph receiver.
// Each row is (SHA256, capture, restore, captured-tree destructor command).
const CURRENT_WRAPPERS: &[(&str, u32, u32, u32)] = &[
    ("0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f", 434, 435, 436),
    ("c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc", 439, 440, 441),
    ("474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1", 429, 430, 431),
    ("63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976", 445, 446, 447),
    ("c47c9d883126f641ce07d92eb57b6ac9833fe7495996001c12c68c55b99974d3", 431, 432, 433),
    ("4ccb48ad4bb6bfb5a15a45b8e1f2c2e62072cfad9f1487f869561a473770ed94", 439, 440, 441),
    ("c48d383c335c5b283bb0c17bd9f14b2b8bfef4a2e3d635365157b1c69498df78", 429, 430, 431),
    ("c2af2ee9baf4b26a3c2cf8a1c867f15f92b54ad1ac5fac381858046a47818f6c", 415, 416, 417),
    ("ac226c8455eff72114d6b1b9929fae01ac49ba90b8ba579f049f179c4c6dd8d5", 408, 409, 410),
    ("3b359fc2d737d5ceed454d6b65e6e5b5e6dfb4a24cf88ee361df86e454203f18", 436, 437, 438),
    ("6f75ae3877e018de1982a68f1089c127870d895d748c6290dd2ee85cd8daa4de", 436, 437, 438),
    ("1f3288d5fd5408bee4d395faab708aca78c9eeaa6c172dc06dcce09c844ba9d5", 445, 446, 447),
    ("f6af7437475b8c066be973245aa79688bea510ca1192264add4f1914ebee4c0e", 401, 402, 403),
    ("012c71635932c241cde4df986377516279a50b907166757e89ea7b86ff3f813f", 403, 404, 405),
    ("9f3a5ad0ed22074420685f1e2a3bc92436d68289c7bac854d5f7d7331dc3ef67", 397, 398, 399),
    ("7bd9f6dcc7d62868d4ad9cc68d5580452e71bd907540ec4f993d960d051d2c45", 429, 430, 431),
    ("a540b1dec1185a63cc68c4d283b6bc99bcce8de219e1bead3b8d113577de4027", 431, 432, 433),
    ("cfb7f74ac3674962a8d982fe7e0ee53b3306fb47362653db794ad6dcb0fcb35d", 439, 440, 441),
    ("f36f63798eb27b1eb6742c2fe7c3a13f172a249ee6623a12fc79fd8c3dee3cfe", 415, 416, 417),
    ("b70a802a74c49a5d6f429114f81d878f83e25a4acbcb2778c6f4d5f68528de7b", 408, 409, 410),
    ("a099ac1cee7af60c8f32869485c9df4aab344b5eb91c1018ae8bb7df2c1f39f3", 420, 421, 422),
    ("5869b7d03a636c200f5042f0c3fc37878a362615bf724f58d47aec5eb8b2d44c", 445, 446, 447),
];

#[derive(Clone, Copy)]
pub(super) struct OriginalStateTreeAbi {
    sha: &'static str,
    // A wrapper's target DWORD is type-independent. Only the independently
    // recovered concrete receiver may assign a task kind here.
    receiver_kind: Option<u32>,
    capture: u32,
    restore: u32,
    release: u32,
}

impl OriginalStateTreeAbi {
    fn for_sha(sha: &str) -> Option<Self> {
        match sha {
            // Wrappers 53a90/54660; factory18d30, view handler1ef20.
            LEGACY => Some(Self { sha: LEGACY, receiver_kind: Some(42), capture: 374, restore: 375, release: 376 }),
            super::super::css_model::UTATE_SHA => Some(Self {
                sha: super::super::css_model::UTATE_SHA, receiver_kind: Some(42),
                capture: 380, restore: 381, release: 382,
            }),
            // Original null-CSS and missing-record branches are still real
            // wrapper behavior. Positive objects need their own receiver.
            _ => CURRENT_WRAPPERS.iter().find(|row| row.0 == sha).map(|&(sha, capture, restore, release)|
                Self { sha, receiver_kind: None, capture, restore, release }),
        }
    }
}

impl NativeTaskStore {
    pub(super) fn bind_original_state_tree_profile(&mut self, sha: &str) {
        self.original_state_tree_abi = OriginalStateTreeAbi::for_sha(sha);
    }

    pub(crate) fn model_state_tree_for_owner(&self, owner: u32, handle: u32) -> Result<StateTree> {
        self.model_state_trees.get(&handle).filter(|(id, _)| *id == owner)
            .map(|(_, tree)| tree.clone()).ok_or_else(|| native_state_error(
                "model restore tree is absent or belongs to another receiver"))
    }

    pub(crate) fn release_model_state_tree(&mut self, owner: u32, handle: u32) -> Result<()> {
        if handle == 0 { return Ok(()); }
        if self.model_state_trees.get(&handle).is_none_or(|(id, _)| *id != owner) {
            return Err(native_state_error("model-tree release handle is absent or belongs to another receiver"));
        }
        self.model_state_trees.remove(&handle);
        Ok(())
    }
}

impl LoggingHost<'_> {
    fn state_tree_abi(&self) -> Option<OriginalStateTreeAbi> {
        self.data_services.tasks.original_state_tree_abi.or_else(||
            self.data_services.native_profile.map(|p| p.pe_sha256)
                .or(self.data_services.css_task_pe_sha256).and_then(OriginalStateTreeAbi::for_sha))
    }

    pub(super) fn original_state_tree_native(&mut self, index: u32, args: &[u8]) -> Result<NativeReturn> {
        if !matches!(index, 688 | 689) { return Err(native_state_error("invalid original state-tree native")); }
        let abi = self.state_tree_abi().ok_or_else(|| native_state_error(
            "original 688/689 need an exact PE state-tree wrapper descriptor"))?;
        let css = read_u32(args, 0)?;
        if css == 0 { return Ok(NativeReturn::Value(0)); }
        let record = read_u32(args, 4)?;
        // Lookup and decode before touching the model. Missing records are
        // the original early return, not a missing-receiver success.
        let file = self.css_handles.get(&css)
            .ok_or_else(|| native_state_error("688/689 has an invalid/wrong-type CSS handle"))?;
        let restore = if index == 689 {
            let bytes = file.try_record(record)
                .map_err(|_| native_state_error("native689 CSS record scan is malformed"))?;
            let Some(bytes) = bytes else { return Ok(NativeReturn::Value(0)); };
            Some(bytes.to_vec())
        } else { None };
        let target = read_u32(args, 8)?;
        let kind = self.data_services.tasks.records.get(&target).map(|record| record.kind);
        let receiver_exists = if abi.sha == LEGACY {
            self.data_services.particle_external.canvases.contains_key(&target)
        } else if abi.sha == super::super::css_model::UTATE_SHA {
            self.data_services.css_models.utate_models.contains_key(&target)
        } else { false };
        if abi.receiver_kind != Some(42) || kind != abi.receiver_kind || !receiver_exists
        {
            return Err(self.task_error(target, cs2_runtime::RuntimeError::Host(format!(
                "original native{index} has no recovered positive-target state-tree receiver: PE={} target={target} kind={kind:?} proven_receiver_kind={:?} kind43_factory={:?} commands={}/{}/{}",
                abi.sha, abi.receiver_kind,
                cs2_runtime::native_factory::exact_factory_result(abi.sha, 43),
                abi.capture, abi.restore, abi.release,
            ))));
        }
        let command = TaskCommand { target: target as i32, sender: self.current_task_id as i32,
            code: if index == 688 { abi.capture } else { abi.restore }, data: [0; 3] };
        if let Some(bytes) = restore {
            let tree = StateTree::decode(&bytes).map_err(|_| native_state_error(
                "native689 original model state-tree decode failed"))?;
            // A virtual typed tree identity replaces only the wrapper pointer.
            // It is never a task number, native address or fabricated VM region.
            let handle = self.retain_model_tree(target, tree)?;
            let result = self.receive_task_command(TaskCommand { data: [0, handle, 0], ..command },
                &mut TaskCommandBuffers::default(), &mut 0, 4096);
            self.data_services.tasks.model_state_trees.remove(&handle);
            let outcome = result?.ok_or_else(|| native_state_error("native689 model target became unavailable"))?;
            if !outcome.done || outcome.propagate {
                return Err(native_state_error("native689 original model command was not consumed"));
            }
            // 54660 ignores the message output and writes1 after dispatch.
            return Ok(NativeReturn::Value(1));
        }
        let mut handle = u32::MAX;
        let outcome = self.receive_task_command(command, &mut TaskCommandBuffers::default(), &mut handle, 4096)?;
        if outcome.is_none_or(|outcome| !outcome.done || outcome.propagate) || handle == u32::MAX {
            // 1fab2 does not write output when no model is loaded. Do not
            // turn an uninitialized native pointer slot into DWORD zero.
            return Err(native_state_error("native688 capture did not produce a loaded-model tree"));
        }
        // 53a90: a recovered receiver may explicitly return a null tree. An
        // untouched output slot above is a different, unrecovered condition.
        if handle == 0 { return Ok(NativeReturn::Value(0)); }
        let result: Result<NativeReturn> = (|| {
            let (_, tree) = self.data_services.tasks.model_state_trees.get(&handle)
                .ok_or_else(|| native_state_error("native688 returned an invalid model-tree identity"))?;
            let bytes = tree.encode().map_err(|_| native_state_error(
                "native688 original state-tree serialization failed"))?;
            self.css_handles.get_mut(&css)
                .ok_or_else(|| native_state_error("native688 CSS owner disappeared during capture"))?
                .append(record, &bytes)
                .map_err(|_| native_state_error("native688 CSS append failed"))?;
            Ok(NativeReturn::Value(1))
        })();
        // 53a90 sends376 to the same receiver after freeing its serialized
        // bytes, including the zero-size/error path. Do not bypass that
        // owner's destructor by just dropping an entry in the handle map.
        // 54660's decoded restore temporary above is instead destroyed
        // directly by the wrapper; it never sends376.
        let release = self.receive_task_command(
            TaskCommand { code: abi.release, data: [0, handle, 0], ..command },
            &mut TaskCommandBuffers::default(), &mut 0, 4096,
        ).and_then(|outcome| {
            if outcome.is_none_or(|outcome| !outcome.done || outcome.propagate)
                || self.data_services.tasks.model_state_trees.contains_key(&handle)
            {
                return Err(native_state_error("native688 owner did not release its captured model tree"));
            }
            Ok(())
        });
        match (result, release) {
            (result, Ok(())) => result,
            (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(release)) => Err(self.task_error(target,
                cs2_runtime::RuntimeError::Host(format!(
                    "native688 capture/save failed: {error}; owner release also failed: {release}",
                )))),
        }
    }

    pub(super) fn receive_model_state_command(
        &mut self, command: TaskCommand, buffers: &mut TaskCommandBuffers<'_>, output: &mut u32,
    ) -> Result<Option<TaskCommandOutcome>> {
        let id = command.target as u32;
        if !model_command(command.code) || !self.legacy_particle_canvas(42)
            || self.data_services.tasks.records.get(&id).is_none_or(|record| record.kind != 42)
            || !self.data_services.particle_external.canvases.contains_key(&id)
        { return Ok(None); }
        match command.code {
            344 | 346 | 371 | 372 => {
                let canvas = &self.data_services.particle_external.canvases[&id];
                if canvas.loaded {
                    let path = command_path(buffers.data0)?;
                    if let Some(found) = canvas.resolve_object(path) {
                        // Parse/validate before mutating either live channel.
                        let values = if matches!(command.code, 344 | 346) {
                            Some(decode_local_transform(buffers.data1.as_deref().ok_or_else(||
                                native_state_error("model local transform requires its owned data1 record"))?)
                                .map_err(|_| native_state_error("model local transform record is truncated"))?)
                        } else { None };
                        // Only a primary freeze without a saved bank reads the
                        // sampled model source, just as the draw consumer does.
                        let sampled_base = if command.code == 371 {
                            match found {
                                Some(index) if canvas.nodes[index].overrides.primary.is_none() =>
                                    Some(canvas.source_transform(index).map_err(|error|
                            crate::native_state_error_owned(format!("model primary freeze source: {error}")))?),
                                _ => None,
                            }
                        } else { None };
                        let canvas = self.data_services.particle_external.canvases.get_mut(&id).unwrap();
                        let (local, saved, base) = match found {
                            Some(index) => {
                                let node = &mut canvas.nodes[index];
                                (&mut node.local_transforms, &mut node.overrides,
                                    sampled_base.unwrap_or(node.base_transform))
                            }
                            None => (&mut canvas.root_local_transforms, &mut canvas.root_overrides,
                                IDENTITY_MODEL_TRANSFORM),
                        };
                        match command.code {
                            344 => local.primary = values.unwrap(),
                            346 => local.secondary = values.unwrap(),
                            371 => local.freeze_primary(saved, base),
                            372 => local.freeze_secondary(saved),
                            _ => unreachable!(),
                        }
                    }
                }
            }
            369 | 370 => {
                if let Some(bytes) = self.model_local_query(id, command.code, buffers.data0)? {
                    buffers.data1.as_deref_mut().and_then(|target| target.get_mut(..48))
                        .ok_or_else(|| native_state_error("model query needs 48 writable data1 bytes"))?
                        .copy_from_slice(&bytes);
                }
            }
            373 => {
                let canvas = self.data_services.particle_external.canvases.get_mut(&id).unwrap();
                if canvas.loaded {
                    if let Some(found) = canvas.resolve_object(command_path(buffers.data0)?) {
                        match found {
                            Some(index) => canvas.nodes[index].overrides = Default::default(),
                            None => canvas.root_overrides = Default::default(),
                        }
                    }
                }
            }
            374 => {
                let canvas = &self.data_services.particle_external.canvases[&id];
                if canvas.loaded {
                    let tree = live_state(canvas)?.capture().map_err(|error| self.task_error(id, error))?;
                    let handle = self.retain_model_tree(id, tree)?;
                    *output = handle;
                }
            }
            375 => {
                if self.data_services.particle_external.canvases[&id].loaded {
                    let (_, tree) = self.data_services.tasks.model_state_trees.get(&command.data[1])
                        .filter(|(owner, _)| *owner == id)
                        .ok_or_else(|| native_state_error("model restore requires its live typed tree handle"))?;
                    let mut prepared = live_state(&self.data_services.particle_external.canvases[&id])?;
                    prepared.restore(tree).map_err(|error| self.task_error(id, error))?;
                    let (root, nodes) = prepared.into_overrides();
                    let canvas = self.data_services.particle_external.canvases.get_mut(&id).unwrap();
                    // All parsing/path validation precedes this single commit.
                    canvas.root_overrides = root;
                    for (node, state) in canvas.nodes.iter_mut().zip(nodes) { node.overrides = state; }
                }
            }
            376 => {
                if command.data[1] != 0 {
                    let handle = command.data[1];
                    if self.data_services.tasks.model_state_trees.get(&handle).is_none_or(|(owner, _)| *owner != id) {
                        return Err(native_state_error("model-tree release handle is absent or belongs to another receiver"));
                    }
                    self.data_services.tasks.model_state_trees.remove(&handle);
                }
            }
            _ => unreachable!(),
        }
        Ok(Some(TaskCommandOutcome { done: true, propagate: false }))
    }

    pub(super) fn dispatch_model_state_message(
        &mut self, target: u32, packet: [u32; 4], queued: bool, output: &mut u32, mut vm: Option<&mut Vm>,
    ) -> Result<Option<TaskCommandOutcome>> {
        if !model_command(packet[1]) { return Ok(None); }
        if queued && matches!(packet[1], 369 | 370) {
            return Err(native_state_error("queued model query cannot retain a VM output address"));
        }
        let command = TaskCommand { target: target as i32, sender: packet[0] as i32,
            code: packet[1], data: [packet[2], packet[3], 0] };
        let mut buffers = if let Some(saved) = self.pending_owned_task_buffers(command) { saved }
        else if model_path_command(packet[1]) && self.data_services.particle_external.canvases.get(&target)
            .is_some_and(|canvas| canvas.loaded)
        {
            let source = vm.as_deref().ok_or_else(|| native_state_error("raw model transform needs its caller VM"))?;
            let mut path = source
                .read_string(packet[2])?;
            path.push(0);
            let found = self.data_services.particle_external.canvases[&target].resolve_object(&path[..path.len()-1]).is_some();
            let record = if matches!(packet[1], 344 | 346) && found { Some(source.read(packet[3], 48)?) }
                else if matches!(packet[1], 369 | 370) { Some(vec![0;48]) } else { None };
            OwnedTaskBuffers::new(Some(path), record)?
        } else { OwnedTaskBuffers::default() };
        if queued {
            return Ok(self.queue_task_command_buffers(command, &buffers, output)?
                .then_some(TaskCommandOutcome { done: true, propagate: false }));
        }
        let result = self.route_task_command_buffers(command, &mut buffers, output, 4096);
        if buffers.data1_write_length() != 0 {
            vm.as_deref_mut().ok_or_else(|| native_state_error("model query output requires its caller VM"))?
                .write(packet[3], &buffers.data1().unwrap()[..buffers.data1_write_length()])?;
        }
        result
    }

    // Owned queries explicitly mark equal-byte output too. Missing paths and
    // unloaded models leave the destination untouched, just like1f2f9/1f3d5.
    pub(super) fn receive_owned_model_query(&mut self, command: TaskCommand, buffers: &mut OwnedTaskBuffers)
        -> Result<Option<TaskCommandOutcome>>
    {
        let id = command.target as u32;
        if !matches!(command.code, 369 | 370) || !self.legacy_particle_canvas(42)
            || self.data_services.tasks.records.get(&id).is_none_or(|record| record.kind != 42)
            || !self.data_services.particle_external.canvases.contains_key(&id) { return Ok(None); }
        if let Some(bytes) = self.model_local_query(id, command.code, buffers.data0())? {
            buffers.write_data1(&bytes)?;
        }
        Ok(Some(TaskCommandOutcome { done: true, propagate: false }))
    }

    fn model_local_query(&self, id: u32, code: u32, path: Option<&[u8]>) -> Result<Option<[u8; 48]>> {
        let canvas = &self.data_services.particle_external.canvases[&id];
        if !canvas.loaded { return Ok(None); }
        let Some(found) = canvas.resolve_object(command_path(path)?) else { return Ok(None); };
        let local = match found { Some(index) => &canvas.nodes[index].local_transforms,
            None => &canvas.root_local_transforms };
        Ok(Some(encode_local_transform(if code == 369 { &local.primary } else { &local.secondary })))
    }

    pub(crate) fn retain_model_tree(&mut self, owner: u32, tree: StateTree) -> Result<u32> {
        let handles = &self.data_services.tasks.model_state_trees;
        let bytes = tree_bytes(&tree).and_then(|initial| handles.values().try_fold(initial,
            |total, (_, tree)| total.checked_add(tree_bytes(tree)?)));
        if handles.len() >= 4096 || bytes.is_none_or(|bytes| bytes > 128 * 1024 * 1024) {
            return Err(native_state_error("retained model state trees exceed 4096 handles or 128 MiB"));
        }
        if *self.next_xml_handle == 0 || *self.next_xml_handle == u32::MAX {
            return Err(native_state_error("model state tree object identities exhausted"));
        }
        let handle = self.allocate_handle();
        self.data_services.tasks.model_state_trees.insert(handle, (owner, tree));
        Ok(handle)
    }
}

fn model_command(code: u32) -> bool { matches!(code, 344 | 346 | 369..=376) }
fn model_path_command(code: u32) -> bool { matches!(code, 344 | 346 | 369..=373) }
fn command_path(bytes: Option<&[u8]>) -> Result<&[u8]> {
    let bytes = bytes.ok_or_else(|| native_state_error("model command requires its owned object path"))?;
    let end = bytes.iter().position(|&byte| byte == 0)
        .filter(|&end| end < 128)
        .ok_or_else(|| native_state_error("model command path lacks its bounded terminator"))?;
    Ok(&bytes[..end])
}

fn live_state(canvas: &crate::particle_canvas::ParticleCanvas) -> Result<LegacyModelState> {
    LegacyModelState::from_live_nodes(canvas.root_overrides.clone(), canvas.nodes.iter().map(|node| ModelStateNode {
        name: node.name.clone(), parent: node.parent, alive: node.alive, overrides: node.overrides.clone(),
    }).collect()).map_err(|_| native_state_error("live model transform graph is invalid or excessive"))
}

fn tree_bytes(tree: &StateTree) -> Option<usize> {
    let mut pending = tree.roots.iter().collect::<Vec<_>>();
    let mut total = 0usize;
    while let Some(node) = pending.pop() {
        total = total.checked_add(12)?.checked_add(node.payload.len())?;
        pending.extend(&node.children);
    }
    Some(total)
}
