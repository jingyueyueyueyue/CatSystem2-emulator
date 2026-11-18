//! Scene-message strings are VM lifetime allocations. Consuming a packet
//! does not invalidate pointers which script code may have copied elsewhere.
//! Persist this ledger together with the VM group and task message queues.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cs2_runtime::scene_tools::SceneMessageMemorySnapshot;
use cs2_script::{Result, Vm};

use super::super::NativeTaskStore;
use super::{HeadlessPlayer, SceneToolStore, native_state_error};

pub(crate) struct PreparedSceneMessageMemory(SceneMessageMemorySnapshot);

impl SceneToolStore {
    pub(super) fn check_memory_growth(&self, additional_bytes: usize, additional_items: usize) -> Result<()> {
        let mut bytes = self.leased_bytes.saturating_add(additional_bytes);
        let mut items = self.string_leases.values().map(BTreeMap::len).sum::<usize>()
            .saturating_add(additional_items);
        for table in self.vm_sources.values() {
            items = items.saturating_add(table.records().len());
            bytes = bytes.saturating_add(table.records().len().saturating_mul(68));
        }
        for string in self.strings.values().flatten().filter(|string| string.address.is_none()) {
            items = items.saturating_add(1);
            bytes = bytes.saturating_add(string.bytes.len()+2);
        }
        for selection in self.selections.values() {
            items = items.saturating_add(selection.nodes.len());
            bytes = bytes.saturating_add(selection.nodes.len().saturating_mul(4))
                .saturating_add(selection.name.len());
        }
        if bytes > 64*1024*1024 || items > 1_000_000 {
            return Err(native_state_error("scene-message/source allocations exceed their group bounds"));
        }
        Ok(())
    }

    fn memory_snapshot(&self) -> Result<SceneMessageMemorySnapshot> {
        let saved = SceneMessageMemorySnapshot {
            version: 1,
            next_generation: self.next_vm_generation,
            generations: self.vm_generations.clone(),
            leases: self.string_leases.clone(),
            pending: self.strings.iter().map(|(&id, strings)|
                (id, strings.iter().cloned().collect())).collect(),
            sources: self.vm_sources.clone(),
            selections: self.selections.clone(),
        };
        saved.validate().map_err(|_| native_state_error("scene-message memory snapshot is invalid"))?;
        Ok(saved)
    }

    /// A replacement VM must not inherit a queued address or an unfinished
    /// scene-context broadcast from the old interpreter.
    pub(crate) fn check_vm_replacement<'a>(
        &self, id: u32, packets: impl IntoIterator<Item = &'a [u32; 4]>,
    ) -> Result<()> {
        if self.has_strings(id) || self.selections.contains_key(&id) {
            return Err(native_state_error(
                "finish the scene-string queue and context broadcast before replacing its VM",
            ));
        }
        let Some(leases) = self.string_leases.get(&id) else { return Ok(()); };
        if packets.into_iter().any(|packet|
            packet[1] == 0xffff_0010 && leases.contains_key(&packet[3]))
        {
            return Err(native_state_error(
                "consume the queued scene string before replacing its owning VM",
            ));
        }
        Ok(())
    }
}

impl HeadlessPlayer {
    pub(crate) fn check_scene_message_vm_replacement(&self, task: u32) -> anyhow::Result<()> {
        let store = &self.data_services.tasks;
        let record = store.records.get(&task);
        let root_packets = self.pending_messages.iter()
            .filter(|_| task == self.current_task_id);
        store.scene_tools.check_vm_replacement(
            task,
            root_packets.chain(record.into_iter().flat_map(|record| record.queued.iter()))
                .chain(record.into_iter().flat_map(|record| record.ordinary.iter())
                    .flat_map(cs2_runtime::OrdinaryTask::queued_messages)),
        )?;
        if store.root_dispatch.is_some_and(|request| request.target == task) {
            anyhow::bail!("finish the synchronous root message before replacing its VM");
        }
        if record.is_some_and(|record| record.active || record.loading.is_some()
            || record.ordinary.as_ref().is_some_and(|ordinary|
                ordinary.has_pending_update() || ordinary.has_pending_command()))
        {
            anyhow::bail!("finish the task execution continuation before replacing its VM");
        }
        if self.scene_message_lease_count(task) != 0 {
            let vm = if task == self.current_task_id { self.vm.as_ref() }
                else { self.task_vms.get(&task).or_else(|| record
                    .and_then(|record| record.ordinary.as_ref())
                    .and_then(|ordinary| ordinary.vm.as_ref())) };
            if vm.is_some_and(Vm::has_pending_execution) {
                anyhow::bail!("finish the scene-message VM continuation before replacing its owner");
            }
        }
        Ok(())
    }

    /// These leases remain live even after a message handler returns. Actual
    /// region bytes and aliases are captured by VmSnapshotGroup, not copied
    /// into a second host buffer or rebound to another task's local address.
    pub fn scene_message_memory_snapshot(&self) -> Result<SceneMessageMemorySnapshot> {
        let store = &self.data_services.tasks;
        if store.records.values().any(|record| record.active) {
            return Err(native_state_error("capture scene-message memory outside active task execution"));
        }
        let saved = store.scene_tools.memory_snapshot()?;
        let mut programs = BTreeMap::new();
        for &id in saved.generations.keys().chain(saved.sources.keys())
            .chain(saved.leases.keys()).chain(saved.pending.keys()) {
            let vm = if id == self.current_task_id {
                self.vm.as_ref()
            } else { self.task_vms.get(&id).or_else(||
                store.records.get(&id).and_then(|record| record.ordinary.as_ref())
                    .and_then(|task| task.vm.as_ref()))
            }.ok_or_else(|| native_state_error("scene-message memory owner has no actual VM"))?;
            programs.insert(id, vm);
        }
        for (&id, strings) in &saved.pending {
            let queue = store.records.get(&id).ok_or_else(||
                native_state_error("pending scene-string owner disappeared"))?;
            if strings.iter().any(|string| string.before > queue.queued.len()) {
                return Err(native_state_error("pending scene-string position exceeds its actual task queue"));
            }
        }
        validate_storage(&saved, &programs)?;
        validate_task_owners(&saved, |id| store.records.contains_key(&id))?;
        Ok(saved)
    }

    /// Public VM detach is an ownership transfer. The caller must retain this
    /// metadata along with that VM, or restore it as part of its complete task
    /// graph. This query lets shells refuse dropping a live lease silently.
    pub fn scene_message_lease_count(&self, task: u32) -> usize {
        self.data_services.tasks.scene_tools.string_leases.get(&task)
            .map_or(0, BTreeMap::len)
    }

    /// A bare VM attach/detach must retain this state with the same owner.
    /// This also covers source tables and not-yet-allocated names.
    pub fn scene_message_vm_has_state(&self, task: u32) -> bool {
        let store = &self.data_services.tasks.scene_tools;
        store.vm_generations.contains_key(&task) || store.vm_sources.contains_key(&task)
            || store.has_strings(task) || store.selections.contains_key(&task)
    }
}

impl NativeTaskStore {
    /// Pair with dropping/replacing the actual VM. The player-level entry
    /// switch checks its separate pending-message queue before this call.
    pub(crate) fn retire_scene_message_vm(&mut self, id: u32) -> Result<()> {
        if self.records.get(&id).is_some_and(|record| record.active
            || record.ordinary.as_ref().is_some_and(|task|
                task.has_pending_update() || task.has_pending_command()))
        {
            return Err(native_state_error("finish the scene-message task continuation before retirement"));
        }
        let packets = self.records.get(&id).into_iter().flat_map(|record| record.queued.iter())
            .chain(self.records.get(&id).into_iter().flat_map(|record| record.ordinary.iter())
                .flat_map(cs2_runtime::OrdinaryTask::queued_messages));
        self.scene_tools.check_vm_replacement(id, packets)?;
        self.scene_tools.release_vm(id);
        Ok(())
    }

    /// Call after every VM is reconstructed but before any live host mutation.
    /// The caller also validates base task identities and its root message
    /// queue in the complete graph transaction.
    pub(crate) fn prepare_scene_message_memory_restore(
        saved: &SceneMessageMemorySnapshot,
        programs: &BTreeMap<u32, Vm>,
        rebuilt_owners: &BTreeSet<u32>,
        task_queue_lengths: &BTreeMap<u32, usize>,
    ) -> Result<PreparedSceneMessageMemory> {
        saved.validate().map_err(|_| native_state_error("saved scene-message memory is invalid"))?;
        let borrowed = programs.iter().map(|(&id, vm)| (id, vm)).collect();
        validate_storage(saved, &borrowed)?;
        validate_task_owners(saved, |id| rebuilt_owners.contains(&id))?;
        for (&id, strings) in &saved.pending {
            let length = task_queue_lengths.get(&id).ok_or_else(||
                native_state_error("saved scene-string owner has no rebuilt task message queue"))?;
            if strings.iter().any(|string| string.before > *length) {
                return Err(native_state_error("saved scene-string position exceeds its rebuilt message queue"));
            }
        }
        Ok(PreparedSceneMessageMemory(saved.clone()))
    }

    pub(crate) fn commit_scene_message_memory_restore(
        &mut self, prepared: PreparedSceneMessageMemory,
    ) {
        debug_assert!(validate_task_owners(&prepared.0, |id| self.records.contains_key(&id)).is_ok());
        let saved = prepared.0;
        let store = &mut self.scene_tools;
        store.next_vm_generation = saved.next_generation;
        store.vm_generations = saved.generations;
        store.leased_bytes = saved.leases.values().flat_map(BTreeMap::values)
            .map(|lease| lease.length as usize).sum();
        store.string_leases = saved.leases;
        store.strings = saved.pending.into_iter()
            .map(|(id, pending)| (id, VecDeque::from(pending))).collect();
        store.vm_sources = saved.sources;
        store.selections = saved.selections;
        store.changed();
    }
}

fn validate_task_owners(
    saved: &SceneMessageMemorySnapshot, mut exists: impl FnMut(u32) -> bool,
) -> Result<()> {
    for &id in saved.generations.keys().chain(saved.pending.keys())
        .chain(saved.sources.keys()).chain(saved.selections.keys())
    {
        if !exists(id) {
            return Err(native_state_error("scene-message snapshot refers to an absent task owner"));
        }
    }
    Ok(())
}

fn validate_storage(saved: &SceneMessageMemorySnapshot, programs: &BTreeMap<u32, &Vm>) -> Result<()> {
    for &id in saved.generations.keys().chain(saved.sources.keys()).chain(saved.pending.keys()) {
        if !programs.contains_key(&id) {
            return Err(native_state_error("scene-message snapshot owner is missing from its actual VM group"));
        }
    }
    for (&id, leases) in &saved.leases {
        let vm = programs.get(&id)
            .ok_or_else(|| native_state_error("scene string lease lost its VM owner"))?;
        let regions: BTreeMap<_, _> = vm.region_snapshots().into_iter()
            .map(|region| (region.base, region.bytes.len())).collect();
        for (&base, lease) in leases {
            if regions.get(&base).copied() != Some(lease.length as usize) {
                return Err(native_state_error(
                    "scene string lease must match an exact allocation in its owning VM",
                ));
            }
        }
    }
    Ok(())
}
