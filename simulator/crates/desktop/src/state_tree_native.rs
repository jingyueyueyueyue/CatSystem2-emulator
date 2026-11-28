//! Original 688/689 -> live kcTask3DView transform-tree receiver.
//! This is separate from the explicitly installed portable whole-host graph.
use cs2_formats::StateTree;
use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_runtime::model_state::{LegacyModelState, ModelStateNode};
use cs2_runtime::model_state::{decode_local_transform, encode_local_transform, IDENTITY_MODEL_TRANSFORM};
use cs2_runtime::native_table::StateTreeCommands;
use cs2_script::{NativeReturn, Result, Vm};

use super::super::{LoggingHost, native_state_error, read_u32};
use super::{NativeTaskStore, OwnedTaskBuffers};

const PARTICLE_TREE_COMMANDS: StateTreeCommands =
    StateTreeCommands { capture: 374, restore: 375, release: 376 };
const CSS_TREE_COMMANDS: StateTreeCommands =
    StateTreeCommands { capture: 380, restore: 381, release: 382 };

impl NativeTaskStore {
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
    pub(crate) fn registered_state_tree_commands(&self) -> Option<StateTreeCommands> {
        self.data_services.ordinary_native_table.as_ref()
            .and_then(|table| table.state_tree_commands())
            .or_else(|| self.data_services.startup_companion_natives.as_ref()
                .and_then(|companion| companion.state_tree_commands))
    }

    pub(super) fn original_state_tree_native(&mut self, index: u32, args: &[u8]) -> Result<NativeReturn> {
        if !matches!(index, 688 | 689) { return Err(native_state_error("invalid original state-tree native")); }
        let commands = self.registered_state_tree_commands().ok_or_else(|| native_state_error(
            "original 688/689 need a registered state-tree wrapper"))?;
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
        let receiver_exists = (commands == PARTICLE_TREE_COMMANDS
            && self.legacy_particle_canvas(42)
            && self.data_services.particle_external.canvases.contains_key(&target))
            || (commands == CSS_TREE_COMMANDS
                && self.data_services.css_models.utate_models.contains_key(&target));
        let StateTreeCommands { capture, restore: restore_command, release } = commands;
        if kind != Some(42) || !receiver_exists {
            return Err(self.task_error(target, cs2_runtime::RuntimeError::Host(format!(
                "original native{index} has no recovered positive-target state-tree receiver: target={target} kind={kind:?} commands={capture}/{restore_command}/{release}",
            ))));
        }
        let command = TaskCommand { target: target as i32, sender: self.current_task_id as i32,
            code: if index == 688 { capture } else { restore_command }, data: [0; 3] };
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
            TaskCommand { code: release, data: [0, handle, 0], ..command },
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
            344..=347 | 371 | 372 => {
                let canvas = &self.data_services.particle_external.canvases[&id];
                if canvas.loaded {
                    let path = command_path(buffers.data0)?;
                    if let Some(found) = canvas.resolve_object(path) {
                        // Parse/validate before mutating either live channel.
                        let values = if (344..=347).contains(&command.code) {
                            let mut values = decode_local_transform(buffers.data1.as_deref().ok_or_else(||
                                native_state_error("model local transform requires its owned data1 record"))?)
                                .map_err(|_| native_state_error("model local transform record is truncated"))?;
                            if matches!(command.code, 345 | 347) {
                                let position = [values[0], values[1], values[2]].map(f32::from_bits);
                                values[..3].copy_from_slice(&canvas.camera_relative_position(position).map(f32::to_bits));
                            }
                            Some(values)
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
                        let canvas = self.data_services.particle_external.canvases.get_mut(&id)
                            .ok_or_else(|| native_state_error("live model canvas disappeared"))?;
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
                            344 | 345 => local.primary = values.ok_or_else(||
                                native_state_error("primary model transform was not decoded"))?,
                            346 | 347 => local.secondary = values.ok_or_else(||
                                native_state_error("secondary model transform was not decoded"))?,
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
                let canvas = self.data_services.particle_external.canvases.get_mut(&id)
                    .ok_or_else(|| native_state_error("live model canvas disappeared"))?;
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
                    let canvas = self.data_services.particle_external.canvases.get_mut(&id)
                        .ok_or_else(|| native_state_error("live model canvas disappeared"))?;
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
            let record = if (344..=347).contains(&packet[1]) && found { Some(source.read(packet[3], 48)?) }
                else if matches!(packet[1], 369 | 370) { Some(vec![0;48]) } else { None };
            OwnedTaskBuffers::new(Some(path), record)?
        } else { OwnedTaskBuffers::default() };
        if queued {
            return Ok(self.queue_task_command_buffers(command, &buffers, output)?
                .then_some(TaskCommandOutcome { done: true, propagate: false }));
        }
        let result = self.route_task_command_buffers(command, &mut buffers, output, 4096)?;
        if buffers.data1_write_length() != 0 {
            let bytes = buffers.data1().and_then(|data| data.get(..buffers.data1_write_length()))
                .ok_or_else(|| native_state_error("model query lost its written data1 bytes"))?;
            vm.as_deref_mut().ok_or_else(|| native_state_error("model query output requires its caller VM"))?
                .write(packet[3], bytes)?;
        }
        Ok(result)
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

fn model_command(code: u32) -> bool { matches!(code, 344..=347 | 369..=376) }
fn model_path_command(code: u32) -> bool { matches!(code, 344..=347 | 369..=373) }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HeadlessPlayer, tests::TempDir};
    use cs2_formats::CssFile;
    use std::path::Path;

    #[test]
    #[ignore = "requires the installed Campanella engine"]
    fn native_state_tree_registered_legacy_model_roundtrip() -> anyhow::Result<()> {
        let game = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../games/\u{795d}\u{798f}\u{306e}\u{30ab}\u{30f3}\u{30d1}\u{30cd}\u{30e9}");
        let scratch = TempDir::new();
        let mut player = HeadlessPlayer::open_with_save_root(game, &scratch.0)?;
        player.with_host(|host| {
            assert_eq!(host.registered_state_tree_commands(), Some(PARTICLE_TREE_COMMANDS));
            host.data_services.native_profile = None;
            host.data_services.css_task_pe_sha256 = None;
            let target = host.create_task_object(42, 0, b"3D View", b"")?;
            let canvas = host.data_services.particle_external.canvases.get_mut(&target)
                .ok_or_else(|| anyhow::anyhow!("factory did not install its model canvas"))?;
            canvas.loaded = true;
            host.css_handles.insert(1, CssFile::new(1000));
            let args = [1, 500, target].map(u32::to_le_bytes).concat();
            assert_eq!(host.portable_object_graph_native(688, &args)?, NativeReturn::Value(1));
            assert!(host.data_services.tasks.model_state_trees.is_empty());
            assert!(host.css_handles[&1].try_record(500)?.is_some());
            assert_eq!(host.portable_object_graph_native(689, &args)?, NativeReturn::Value(1));
            assert!(host.data_services.tasks.model_state_trees.is_empty());
            Ok(())
        })
    }

    #[test]
    #[ignore = "requires the installed V401 and Happiness!2 games"]
    fn native_state_tree_desktop_registered_wrapper_dispatch() -> anyhow::Result<()> {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let v401 = workspace.join("games/cs2_full_v401");
        let happiness = workspace.join("games/Happiness！2 樱花盛典");
        for (game, expected_capture) in [(v401, 434), (happiness, 445)] {
            let scratch = TempDir::new();
            let mut player = HeadlessPlayer::open_with_save_root(game, &scratch.0)?;
            let mut css = CssFile::new(1000);
            css.append(42, b"existing CSS record")?;
            let missing = [1, 17, 0].map(u32::to_le_bytes).concat();
            let present = [1, 42, 0].map(u32::to_le_bytes).concat();
            player.with_host(|host| {
                assert_eq!(host.registered_state_tree_commands(), Some(StateTreeCommands {
                    capture: expected_capture,
                    restore: expected_capture + 1,
                    release: expected_capture + 2,
                }));
                host.css_handles.insert(1, css.clone());
                for index in [688, 689] {
                    assert_eq!(host.portable_object_graph_native(index, &0u32.to_le_bytes())?, NativeReturn::Value(0));
                    let error = host.portable_object_graph_native(index, &present).unwrap_err();
                    assert!(error.to_string().contains("no recovered positive-target state-tree receiver"));
                    let event = host.events.last().unwrap();
                    assert!(event.detail.contains("no recovered positive-target state-tree receiver"));
                }
                assert_eq!(host.portable_object_graph_native(689, &missing)?, NativeReturn::Value(0));
                assert_eq!(host.css_handles.get(&1), Some(&css));

                host.data_services.ordinary_native_table = None;
                host.data_services.startup_companion_natives = None;
                let error = host.portable_object_graph_native(689, &missing).unwrap_err();
                assert!(error.to_string().contains("need a registered state-tree wrapper"));
                assert_eq!(host.css_handles.get(&1), Some(&css));
                Ok(())
            })?;
        }
        Ok(())
    }
}
