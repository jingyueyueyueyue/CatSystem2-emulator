//! Typed task receiver and task-owned numeric areas. Implemented from the
//! documented command contracts; no process pointers or reference code.
use std::collections::{BTreeMap, BTreeSet};

use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_script::{NativeReturn, Result};

use super::{LoggingHost, NativeTaskStore, TaskValueScope, ordinary_kind};
use crate::{native_state_error, read_u32};

const MAX_VALUES: usize = 1_000_000;
const MAX_RECORD_VALUES: usize = 64 * 1024 * 1024 / 4;
const UNBOUND_AREA: u64 = 1 << 63;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SharedNumericBackupSnapshot {
    pub(crate) area: u64,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Default)]
pub(super) struct SharedValueAreas {
    // Area zero is the desktop's visible expression buffer. Other areas are
    // moved into that buffer while their task executes, then moved back.
    active: u64,
    next: u64,
    initialized: BTreeSet<u64>,
    inactive: BTreeMap<u64, Vec<u32>>,
    /// One fixed backup per shared identity, not a stack per invoking task.
    /// The original 609/610 copy only `slot_count` bytes, not DWORD bytes.
    backups: BTreeMap<u64, Vec<u8>>,
    local_regions: BTreeMap<u64, BTreeMap<u32, Vec<u32>>>,
}

impl SharedValueAreas {
    pub(super) fn host_binding(&self) -> Option<u64> {
        self.initialized.contains(&0).then_some(0)
    }
}

impl NativeTaskStore {
    pub(crate) fn shared_numeric_state_snapshot(
        &self, active_values: &[u32],
    ) -> Result<cs2_script::shared_numeric_snapshot::SharedNumericStateSnapshot> {
        if self.records.values().any(|record| record.active) {
            return Err(native_state_error("cannot snapshot numeric bindings inside an active task callback"));
        }
        let state = cs2_script::shared_numeric_snapshot::SharedNumericStateSnapshot {
            version: 1, next_identity: self.shared_areas.next,
            active_identity: self.shared_areas.active,
            initialized: self.shared_areas.initialized.clone(),
            active_values: active_values.to_vec(), inactive_values: self.shared_areas.inactive.clone(),
            backups: self.shared_areas.backups.clone(),
            task_bindings: self.records.iter().map(|(&id, record)| (id, record.shared_area)).collect(),
            local_regions: self.shared_areas.local_regions.clone(),
        };
        state.validate()?;
        Ok(state)
    }

    /// Rebuilt task records must exist before installing their shared bindings.
    /// Validate the complete group and task identities before modifying either
    /// the active host buffer or any object's binding.
    pub(crate) fn restore_shared_numeric_state(
        &mut self, active_values: &mut Vec<u32>,
        state: cs2_script::shared_numeric_snapshot::SharedNumericStateSnapshot,
    ) -> Result<()> {
        state.validate()?;
        if self.records.values().any(|record| record.active)
            || self.records.len() != state.task_bindings.len()
            || self.records.keys().ne(state.task_bindings.keys())
        {
            return Err(native_state_error("numeric state restore requires the matching inactive task graph"));
        }
        for (&id, &binding) in &state.task_bindings {
            self.records.get_mut(&id).unwrap().shared_area = binding;
        }
        *active_values = state.active_values;
        self.shared_areas = SharedValueAreas {
            active: state.active_identity, next: state.next_identity,
            initialized: state.initialized, inactive: state.inactive_values, backups: state.backups,
            local_regions: state.local_regions,
        };
        Ok(())
    }

    /// Save each shared identity once, alongside that identity's numeric data
    /// and task bindings in the complete host object graph.
    pub(crate) fn shared_numeric_backup_snapshots(&self) -> Vec<SharedNumericBackupSnapshot> {
        self.shared_areas.backups.iter().map(|(&area, bytes)|
            SharedNumericBackupSnapshot { area, bytes: bytes.clone() }).collect()
    }

    pub(crate) fn supports(index: u32) -> bool {
        matches!(index, 209..=212 | 240 | 250 | 609 | 610 | 658 | 659 | 694 | 1074)
    }
}

impl LoggingHost<'_> {
    /// Restore after rebuilding numeric area identities/bindings. Validate the
    /// complete group before replacing any existing backup storage.
    pub(crate) fn restore_shared_numeric_backups(
        &mut self, snapshots: Vec<SharedNumericBackupSnapshot>,
    ) -> Result<()> {
        let areas = &self.data_services.tasks.shared_areas;
        let mut restored = BTreeMap::new();
        for snapshot in snapshots {
            if !areas.initialized.contains(&snapshot.area) {
                return Err(native_state_error("numeric backup references an unknown shared identity"));
            }
            let count = if snapshot.area == areas.active {
                self.shared_values.len()
            } else {
                areas.inactive.get(&snapshot.area)
                    .ok_or_else(|| native_state_error("numeric backup has no restored numeric storage"))?.len()
            };
            if snapshot.bytes.len() != count || restored.insert(snapshot.area, snapshot.bytes).is_some() {
                return Err(native_state_error("numeric backup has a duplicate identity or wrong byte length"));
            }
        }
        if restored.len() != areas.initialized.len() {
            return Err(native_state_error("numeric backup snapshot omits a live shared identity"));
        }
        self.data_services.tasks.shared_areas.backups = restored;
        Ok(())
    }

    pub(crate) fn enter_task_values(&mut self, id: u32) -> Result<TaskValueScope> {
        self.ensure_task_record(id)?;
        let fes_locals = self.scene_nodes.get(&id).filter(|node| node.kind == 34)
            .map(|node| node.fes_parameters);
        let store = &mut self.data_services.tasks;
        let area = store.records[&id].shared_area;
        if let Some(area) = area {
            // The original shared service's named local map uses the actual
            // task object as key, not its creation-context text. Re-entering
            // an existing object selects its retained 512-slot region.
            let locals = store.shared_areas.local_regions.entry(area).or_default()
                .entry(id).or_insert_with(|| vec![0; 512]);
            if let Some(values) = fes_locals {
                for (target, value) in locals.iter_mut().zip(values) { *target = value as u32; }
            }
        }
        let previous_area = store.shared_areas.active;
        if area == Some(previous_area)
            || (area.is_none() && previous_area == (UNBOUND_AREA | u64::from(id)))
            || (id == 0 && previous_area == 0)
        {
            return Ok(TaskValueScope { previous_area, swapped: false });
        }
        let values = match area {
            Some(area) => store.shared_areas.inactive.remove(&area)
                .ok_or_else(|| native_state_error("task shared area is executing in another scope"))?,
            None => Vec::new(),
        };
        let saved_values = std::mem::replace(self.shared_values, values);
        store.shared_areas.inactive.insert(previous_area, saved_values);
        store.shared_areas.active = area.unwrap_or(UNBOUND_AREA | u64::from(id));
        Ok(TaskValueScope { previous_area, swapped: true })
    }

    pub(crate) fn leave_task_values(&mut self, scope: TaskValueScope) {
        if scope.swapped {
            let store = &mut self.data_services.tasks;
            let previous = store.shared_areas.inactive.remove(&scope.previous_area)
                .expect("the enclosing task numeric area remains registered");
            let values = std::mem::replace(self.shared_values, previous);
            let active = store.shared_areas.active;
            if store.shared_areas.initialized.contains(&active) {
                store.shared_areas.inactive.insert(active, values);
            }
            store.shared_areas.active = scope.previous_area;
        }
    }

    pub(crate) fn release_task_values(&mut self, area: Option<u64>) {
        let Some(area) = area else { return };
        let store = &mut self.data_services.tasks;
        let records = &store.records;
        if let Some(regions) = store.shared_areas.local_regions.get_mut(&area) {
            regions.retain(|id, _| records.get(id).is_some_and(|record| record.shared_area == Some(area)));
        }
        if store.records.values().any(|record| record.shared_area == Some(area)) {
            return;
        }
        store.shared_areas.inactive.remove(&area);
        store.shared_areas.initialized.remove(&area);
        store.shared_areas.backups.remove(&area);
        store.shared_areas.local_regions.remove(&area);
        if store.shared_areas.active == area {
            self.shared_values.clear();
        }
    }

    pub(crate) fn initialize_task_values(&mut self, count: u32) -> Result<()> {
        let id = self.current_task_id;
        self.ensure_task_record(id)?;
        if self.data_services.tasks.records[&id].shared_area.is_some() {
            return Ok(());
        }
        let count = count as usize;
        if count > MAX_VALUES {
            return Err(native_state_error("shared numeric area exceeds one million slots"));
        }
        let mut values = Vec::new();
        values.try_reserve_exact(count)
            .map_err(|_| native_state_error("shared numeric area allocation failed"))?;
        values.resize(count, 0);
        let store = &mut self.data_services.tasks;
        let area = if id == 0 && store.shared_areas.active == 0 {
            0
        } else {
            let area = store.shared_areas.next.checked_add(1)
                .filter(|&area| area < UNBOUND_AREA)
                .ok_or_else(|| native_state_error("shared numeric area identifiers exhausted"))?;
            store.shared_areas.next = area;
            area
        };
        store.shared_areas.initialized.insert(area);
        // Initial backup bytes were not initialized by the original
        // constructor. Deterministic zeros avoid reading host allocator data.
        store.shared_areas.backups.insert(area, vec![0; count]);
        store.shared_areas.local_regions.entry(area).or_default().insert(id, vec![0; 512]);
        store.records.get_mut(&id).unwrap().shared_area = Some(area);
        store.shared_areas.active = area;
        *self.shared_values = values;
        Ok(())
    }

    pub(crate) fn shared_values_for(&self, id: u32) -> Option<&[u32]> {
        let store = &self.data_services.tasks;
        let record = store.records.get(&id)?;
        if !ordinary_kind(record.kind) || record.deletion != 0 { return None; }
        let area = record.shared_area?;
        if store.shared_areas.active == area {
            Some(self.shared_values.as_slice())
        } else {
            store.shared_areas.inactive.get(&area).map(Vec::as_slice)
        }
    }

    pub(crate) fn current_local_values(&self) -> Option<&[u32]> {
        let store = &self.data_services.tasks;
        let area = store.records.get(&self.current_task_id)?.shared_area?;
        store.shared_areas.local_regions.get(&area)?.get(&self.current_task_id).map(Vec::as_slice)
    }

    pub(crate) fn current_local_values_mut(&mut self) -> Option<&mut Vec<u32>> {
        let store = &mut self.data_services.tasks;
        let area = store.records.get(&self.current_task_id)?.shared_area?;
        store.shared_areas.local_regions.get_mut(&area)?.get_mut(&self.current_task_id)
    }

    /// FES expressions own an i32 interpreter buffer; its task service exposes
    /// the same bits through 658/659 and 666. Call after interpreter execution
    /// or a FES parameter restore, including while the driver is taken out.
    pub(crate) fn sync_fes_local_region(&mut self, id: u32, values: &[i32]) {
        let store = &mut self.data_services.tasks;
        let Some(area) = store.records.get(&id).and_then(|record| record.shared_area) else { return; };
        let locals = store.shared_areas.local_regions.entry(area).or_default()
            .entry(id).or_insert_with(|| vec![0; 512]);
        for (target, &value) in locals.iter_mut().zip(values) { *target = value as u32; }
    }

    pub(crate) fn commit_expression_locals(&mut self, values: &[i32]) {
        if let Some(locals) = self.current_local_values_mut() {
            for (target, &value) in locals.iter_mut().zip(values) { *target = value as u32; }
        }
        if let Some(node) = self.scene_nodes.get_mut(&self.current_task_id)
            .filter(|node| node.kind == 34)
        {
            node.fes_parameters.copy_from_slice(values);
        }
    }

    pub(crate) fn copy_fes_locals_to_task(
        &mut self, source: u32, destination: u32, values: &[i32; 512],
    ) -> bool {
        let store = &mut self.data_services.tasks;
        let Some(area) = store.records.get(&source).and_then(|record| record.shared_area) else {
            return false;
        };
        if store.records.get(&destination).and_then(|record| record.shared_area) != Some(area) {
            return false;
        }
        let Some(regions) = store.shared_areas.local_regions.get_mut(&area) else { return false; };
        if !regions.contains_key(&source) { return false; }
        let Some(target) = regions.get_mut(&destination) else { return false; };
        if target.len() != 512 { return false; }
        for (target, &value) in target.iter_mut().zip(values) { *target = value as u32; }
        if let Some(node) = self.scene_nodes.get_mut(&destination).filter(|node| node.kind == 34) {
            node.fes_parameters.copy_from_slice(values);
        }
        true
    }

    pub(crate) fn shared_values_for_mut(&mut self, id: u32) -> Option<&mut Vec<u32>> {
        let store = &mut self.data_services.tasks;
        let record = store.records.get(&id)?;
        if !ordinary_kind(record.kind) || record.deletion != 0 { return None; }
        let area = record.shared_area?;
        if store.shared_areas.active == area {
            Some(&mut *self.shared_values)
        } else {
            store.shared_areas.inactive.get_mut(&area)
        }
    }

    pub(crate) fn task_values_native(&mut self, index: u32, args: &[u8]) -> Result<NativeReturn> {
        if index == 1074 {
            let digest = self.data_services.native_profile.map_or("", |profile| profile.pe_sha256);
            if !cs2_runtime::native_task_clock_profile::exact_single_step_native(digest) {
                return Err(native_state_error("task single-step native needs its exact PE clock descriptor"));
            }
            self.ensure_task_record(self.current_task_id)?;
            let record = self.data_services.tasks.records.get_mut(&self.current_task_id).unwrap();
            record.clock.single_step = 1;
            if let Some(task) = record.ordinary.as_mut() {
                task.clock.single_step = 1;
            }
            return Ok(NativeReturn::Continue);
        }
        let scope = self.enter_task_values(self.current_task_id)?;
        let result = self.task_values_native_inner(index, args);
        self.leave_task_values(scope);
        result
    }

    fn task_values_native_inner(&mut self, index: u32, args: &[u8]) -> Result<NativeReturn> {
        if matches!(index, 609 | 610) {
            return self.shared_numeric_backup_native(index);
        }
        if matches!(index, 658 | 659) && self.current_local_values().is_none() {
            return Ok(if index == 658 { NativeReturn::Value(0) } else { NativeReturn::Continue });
        }
        let first = read_u32(args, 0)?;
        match index {
            658 => Ok(NativeReturn::Value(self.current_local_values()
                .and_then(|values| values.get(first as usize)).copied().unwrap_or(0))),
            659 => {
                let value = read_u32(args, 4)?;
                if let Some(slot) = self.current_local_values_mut()
                    .and_then(|values| values.get_mut(first as usize)) { *slot = value; }
                if let Some(parameter) = self.scene_nodes.get_mut(&self.current_task_id)
                    .filter(|node| node.kind == 34)
                    .and_then(|node| node.fes_parameters.get_mut(first as usize))
                {
                    *parameter = value as i32;
                }
                Ok(NativeReturn::Continue)
            }
            209 => {
                self.initialize_task_values(first)?;
                Ok(NativeReturn::Continue)
            }
            210 => Ok(NativeReturn::Value(
                self.shared_values.get(first as usize).copied().unwrap_or(0),
            )),
            211 => {
                let value = read_u32(args, 4)?;
                if let Some(slot) = self.shared_values.get_mut(first as usize) { *slot = value; }
                Ok(NativeReturn::Continue)
            }
            212 => {
                *self.current_parameter = first;
                Ok(NativeReturn::Continue)
            }
            694 => {
                let value = read_u32(args, 4)?;
                self.receive_task_command(
                    TaskCommand { target: first as i32, sender: self.current_task_id as i32,
                        code: 29, data: [0, value, 0] },
                    &mut TaskCommandBuffers::default(), &mut 0, 4096,
                )?;
                Ok(NativeReturn::Continue)
            }
            240 | 250 => {
                // A null CSS handle skips every later argument and query.
                if first == 0 { return Ok(NativeReturn::Value(0)); }
                let record = read_u32(args, 4)?;
                let target = read_u32(args, 8)?;
                let start = read_u32(args, 12)? as usize;
                let mut available = [0; 4];
                let mut output = 0;
                self.receive_task_command(
                    TaskCommand { target: target as i32, sender: self.current_task_id as i32,
                        code: 177, data: [0, 1, 0] },
                    &mut TaskCommandBuffers { data0: None, data1: Some(&mut available) },
                    &mut output, 4096,
                )?;
                if u32::from_le_bytes(available) == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                if index == 240 {
                    let count = read_u32(args, 16)? as usize;
                    if count > MAX_RECORD_VALUES {
                        return Err(native_state_error("shared numeric save range exceeds 64 MiB"));
                    }
                    let values = self.shared_values_for(target).ok_or_else(||
                        native_state_error("command 177 returned an unavailable shared numeric area"))?;
                    let begin = start.min(values.len());
                    let end = begin.saturating_add(count).min(values.len());
                    let bytes: Vec<_> = values[begin..end].iter()
                        .flat_map(|value| value.to_le_bytes()).collect();
                    self.css_handles.get_mut(&first)
                        .ok_or_else(|| native_state_error("CSS container handle is invalid"))?
                        .append(record, &bytes)
                        .map_err(|_| native_state_error("shared numeric record append failed"))?;
                } else {
                    let Some(bytes) = self.css_handles.get(&first)
                        .ok_or_else(|| native_state_error("CSS container handle is invalid"))?
                        .try_record(record)
                        .map_err(|_| native_state_error("shared numeric CSS record storage is malformed"))?
                        .map(ToOwned::to_owned)
                    else { return Ok(NativeReturn::Value(0)); };
                    let values = self.shared_values_for_mut(target).ok_or_else(||
                        native_state_error("command 177 returned an unavailable shared numeric area"))?;
                    let begin = start.min(values.len());
                    for (slot, bytes) in values[begin..].iter_mut().zip(bytes.chunks_exact(4)) {
                        *slot = u32::from_le_bytes(bytes.try_into().unwrap());
                    }
                }
                Ok(NativeReturn::Value(1))
            }
            _ => Err(native_state_error("native has no task value/control contract")),
        }
    }

    fn shared_numeric_backup_native(&mut self, index: u32) -> Result<NativeReturn> {
        let store = &mut self.data_services.tasks;
        let Some(area) = store.records.get(&self.current_task_id).and_then(|record| record.shared_area)
        else { return Ok(NativeReturn::Continue); };
        // task_values_native entered this task's area before reaching here.
        if store.shared_areas.active != area {
            return Err(native_state_error("numeric backup task binding is not active"));
        }
        let count = self.shared_values.len();
        let backup = store.shared_areas.backups.get_mut(&area)
            .ok_or_else(|| native_state_error("numeric backup storage is absent for its shared area"))?;
        if backup.len() != count {
            return Err(native_state_error("numeric backup length differs from its shared area"));
        }
        // Both original methods pass the unscaled DWORD slot count directly
        // as memcpy's BYTE length (0x686000/0x686020). Preserve partial DWORDs.
        if index == 609 {
            for (offset, output) in backup.iter_mut().enumerate() {
                *output = self.shared_values[offset / 4].to_le_bytes()[offset % 4];
            }
        } else {
            for (offset, &byte) in backup.iter().enumerate() {
                let target = &mut self.shared_values[offset / 4];
                let mut bytes = target.to_le_bytes();
                bytes[offset % 4] = byte;
                *target = u32::from_le_bytes(bytes);
            }
        }
        Ok(NativeReturn::Continue)
    }

    /// Addressed receiver only. Unknown commands retain the base-class
    /// propagation response; callers decide whether to traverse descendants.
    pub(crate) fn receive_task_command(
        &mut self, command: TaskCommand, buffers: &mut TaskCommandBuffers<'_>,
        output: &mut u32, budget: usize,
    ) -> Result<Option<TaskCommandOutcome>> {
        let id = command.target as u32;
        if !self.task_is_reachable(id) {
            return Ok(None);
        }
        if let Some(outcome) = self.receive_top_clock_command(command)? {
            return Ok(Some(outcome));
        }
        self.ensure_task_record(id)?;
        if let Some(outcome) = self.receive_root_sound_query(command, output)? {
            return Ok(Some(outcome));
        }
        let consumed = TaskCommandOutcome { done: true, propagate: false };
        let (kind, deletion) = {
            let record = &self.data_services.tasks.records[&id];
            (record.kind, record.deletion)
        };
        // A root handler may delete its sender/itself before completing a
        // synchronous packet. Consume that exact retained acknowledgement
        // before busy/deletion guards; never replay or abandon the handler.
        if command.code == 173 && ordinary_kind(kind)
            && self.data_services.tasks.root_dispatch.is_some_and(|request| request.target == id)
        {
            let bytes = buffers.data1.as_deref_mut().and_then(|bytes| bytes.get_mut(..16))
                .ok_or_else(|| native_state_error("retained root packet requires 16 data1 bytes"))?;
            let mut packet = [0; 4];
            for (word, bytes) in packet.iter_mut().zip(bytes.chunks_exact(4)) {
                *word = u32::from_le_bytes(bytes.try_into().unwrap());
            }
            if self.data_services.tasks.root_dispatch.is_some_and(|request| request.message == packet) {
                let done = self.dispatch_root_message_request(id, &mut packet, output)?;
                if done {
                    for (bytes, word) in bytes.chunks_exact_mut(4).zip(packet) {
                        bytes.copy_from_slice(&word.to_le_bytes());
                    }
                }
                return Ok(Some(TaskCommandOutcome { done, propagate: false }));
            }
        }
        // JS save notifications target the registered parent, which can be
        // an ordinary child KCS rather than the player's external root. A
        // synthetic budget yield must retain that VM's actual message call
        // even if a later notification marks the receiving task deleted.
        if command.code == 173 && ordinary_kind(kind)
            && self.data_services.tasks.records[&id].message_owner == Some(self.current_task_id)
        {
            let bytes = buffers.data1.as_deref_mut().and_then(|bytes| bytes.get_mut(..16))
                .ok_or_else(|| native_state_error("retained KCS packet requires 16 data1 bytes"))?;
            let mut packet = [0; 4];
            for (word, bytes) in packet.iter_mut().zip(bytes.chunks_exact(4)) {
                *word = u32::from_le_bytes(bytes.try_into().unwrap());
            }
            let done = self.dispatch_ordinary_message(id, &mut packet, output, budget)?;
            if done {
                for (bytes, word) in bytes.chunks_exact_mut(4).zip(packet) {
                    bytes.copy_from_slice(&word.to_le_bytes());
                }
            }
            return Ok(Some(TaskCommandOutcome { done, propagate: false }));
        }
        if deletion != 0 { return Ok(Some(consumed)); }
        if let Some(outcome) = self.receive_legacy_plane_input91(command)? {
            return Ok(Some(outcome));
        }
        if matches!(kind, 2 | 65) {
            if let Some(outcome) = self.receive_movie_plane_command(command, buffers, output)? {
                return Ok(Some(outcome));
            }
        }
        if let Some(outcome) = self.receive_css_model_command(command, buffers, output)? {
            return Ok(Some(outcome));
        }
        if let Some(outcome) = self.receive_css_task_command(command, output)? {
            return Ok(Some(outcome));
        }
        if kind == 54 {
            if let Some(outcome) = self.receive_emote_geometry(command, buffers.data1.as_deref())? {
                return Ok(Some(outcome));
            }
        }
        if kind == 61 {
            if let Some(outcome) = self.receive_effekseer_transform(
                command, buffers.data1.as_deref(), output, budget,
            )? {
                return Ok(Some(outcome));
            }
        }
        if kind == 62 {
            if let Some(outcome) = self.receive_tweet_command(command, buffers.data1.as_deref(), output, budget)? {
                return Ok(Some(outcome));
            }
        }
        if kind == 16 {
            if let Some(outcome) = self.receive_particle_command(command, buffers, output)? {
                return Ok(Some(outcome));
            }
        }
        if matches!(kind, 18 | 25) {
            if let Some(outcome) = self.receive_raster_command(command, buffers, output)? {
                return Ok(Some(outcome));
            }
        }
        if super::ordinary_kind(kind) {
            if let Some(outcome) = self.receive_scene_selection_command(command, buffers, output, budget)? {
                return Ok(Some(outcome));
            }
        }
        if matches!(kind, 32 | 33) {
            if let Some(outcome) = self.receive_scene_tool_command(command, buffers, budget)? {
                return Ok(Some(outcome));
            }
        }
        if kind == 2 && matches!(command.code, 46 | 79 | 80 | 84..=86)
            && !self.movie_host.as_ref().is_some_and(|movie|movie.plane_target_handle(id).is_some())
        {
            if self.dispatch_plane_command(command, &mut crate::plane_native::PlaneCommandBuffers {
                data0:None, data1:buffers.data1.as_deref_mut(),
            }, output)? {
                return Ok(Some(consumed));
            }
        }
        if super::ordinary_kind(kind) {
            if let Some(outcome) = self.message_list_parent_selection(command, budget)? {
                return Ok(Some(outcome));
            }
        }
        if kind == 47 {
            if let Some(outcome) = self.receive_message_list_command(command, budget)? {
                return Ok(Some(outcome));
            }
        }
        if matches!(kind, 45 | 46) {
            if let Some(outcome) = self.receive_export_task_command(command, buffers, output)? {
                return Ok(Some(outcome));
            }
        }
        if self.legacy_particle_canvas(kind) {
            if let Some(outcome) = self.receive_model_state_command(command, buffers, output)? { return Ok(Some(outcome)); }
            if let Some(outcome) = self.receive_particle_canvas_command(command, buffers, output)? { return Ok(Some(outcome)); }
        } else if super::RenderTaskStore::supports_kind(kind) && !self.legacy_css_model(kind) {
            if let Some(outcome) = self.receive_render_task_command(command, buffers, output)? {
                return Ok(Some(outcome));
            }
        }
        if kind == 55 {
            if let Some(outcome) = self.receive_javascript_command(command, buffers, output, budget)
                .map_err(|cause| self.task_error(id, cause))?
            {
                return Ok(Some(outcome));
            }
        }
        if matches!(kind, 41 | 42) && self.data_services.fes_controls.owns_lower(id) {
            if let Some(outcome) = self.receive_lower_task_command(command, buffers, output, budget)? {
                return Ok(Some(outcome));
            }
        }
        match command.code {
            13 => *output = kind,
            17 => {}
            23 => *output = self.data_services.tasks.records[&id].property,
            24 => { self.data_services.tasks.write_task_property(id, command.data[1])?; }
            29 => {
                let record = self.data_services.tasks.records.get_mut(&id).unwrap();
                record.clock.paused = command.data[1];
                if let Some(task) = record.ordinary.as_mut() {
                    task.clock.paused = command.data[1];
                }
            }
            25 | 26 if ordinary_kind(kind) || kind == 34 => {
                if command.data[1] != 0 {
                    let bytes = buffers.data1.as_deref_mut()
                        .ok_or_else(|| native_state_error("task context requires data1 buffer"))?;
                    if command.code == 25 {
                        let context = &self.data_services.tasks.records[&id].context;
                        let length = context.len() + 1;
                        let target = bytes.get_mut(..length)
                            .ok_or_else(|| native_state_error("task context output buffer is too small"))?;
                        target[..context.len()].copy_from_slice(context);
                        target[context.len()] = 0;
                    } else {
                        let end = bytes.iter().position(|&byte| byte == 0)
                            .ok_or_else(|| native_state_error("task context input is not terminated"))?;
                        if end > 31 { return Err(native_state_error("task context exceeds 31 bytes")); }
                        let record = self.data_services.tasks.records.get_mut(&id).unwrap();
                        record.context = bytes[..end].to_vec();
                        if let Some(task) = record.ordinary.as_mut() {
                            task.set_creation_context(&record.context)
                                .expect("the context length and termination were validated");
                        }
                    }
                }
            }
            177 if ordinary_kind(kind) => {
                if self.shared_values_for(id).is_some() {
                    let bytes = buffers.data1.as_deref_mut()
                        .and_then(|bytes| bytes.get_mut(..4))
                        .ok_or_else(|| native_state_error("task shared query requires four data1 bytes"))?;
                    bytes.copy_from_slice(&1u32.to_le_bytes());
                }
            }
            20 | 21 if ordinary_kind(kind) => {
                self.data_services.tasks.records.get_mut(&id).unwrap().queued.push_back([
                    command.sender as u32,
                    if command.code == 20 { 0xffff_0012 } else { 0xffff_0013 },
                    0, 0,
                ]);
                *output = 1;
            }
            173 | 174 if ordinary_kind(kind) => {
                let bytes = buffers.data1.as_deref_mut()
                    .and_then(|bytes| bytes.get_mut(..16))
                    .ok_or_else(|| native_state_error("ordinary task message requires 16 data1 bytes"))?;
                let mut packet = [0; 4];
                for (word, bytes) in packet.iter_mut().zip(bytes.chunks_exact(4)) {
                    *word = u32::from_le_bytes(bytes.try_into().unwrap());
                }
                let done = if command.code == 174 {
                    self.data_services.tasks.records.get_mut(&id).unwrap().queued.push_back(packet);
                    *output = 1;
                    true
                } else if self.data_services.tasks.records[&id].active {
                    *output = 0;
                    true
                } else if self.data_services.tasks.external_tasks.contains(&id) {
                    self.dispatch_root_message_request(id, &mut packet, output)?
                } else {
                    self.dispatch_ordinary_message(id, &mut packet, output, budget)?
                };
                if done && command.code == 173 {
                    for (bytes, word) in bytes.chunks_exact_mut(4).zip(packet) {
                        bytes.copy_from_slice(&word.to_le_bytes());
                    }
                }
                return Ok(Some(TaskCommandOutcome { done, propagate: false }));
            }
            19 if ordinary_kind(kind) => {
                let languages = self.data_services.language.languages.clone();
                self.data_services.tasks.notify_configuration_command(
                    id, command, &languages, self.task_vms.contains_key(&id),
                )?;
            }
            19 if kind == 34 => return Ok(Some(TaskCommandOutcome {
                done: self.notify_fes_configuration(id, budget)?, propagate: false,
            })),
            19 if kind == 13 => {
                if !self.dispatch_sound_task_command(id, command, buffers, output)? {
                    return Err(native_state_error("sound configuration receiver was not available"));
                }
            }
            3 if id == 0 => return Err(native_state_error(
                "root deletion requires the player's root lifecycle",
            )),
            3 if self.data_services.tasks.records[&id].active => {
                self.data_services.tasks.records.get_mut(&id).unwrap().deletion = 1;
            }
            3 if ordinary_kind(kind) && !self.data_services.tasks.external_tasks.contains(&id) => {
                let done = self.with_ordinary_task(id, |host, task| {
                    task.request_delete(host, budget).map_err(|error| host.task_error(id, error))
                })?;
                if done { self.data_services.tasks.records.get_mut(&id).unwrap().deletion = 1; }
                return Ok(Some(TaskCommandOutcome { done, propagate: false }));
            }
            3 => { self.request_ordinary_task_delete(id)?; }
            _ if kind == 34 => {
                return Ok(Some(match self.receive_fes_command(command, buffers, output, budget)? {
                    Some(done) => TaskCommandOutcome {
                        done, propagate: done && command.code == 348,
                    },
                    None => TaskCommandOutcome { done: true, propagate: true },
                }));
            }
            _ if ordinary_kind(kind) => {
                if self.data_services.tasks.records[&id].active
                    || self.data_services.tasks.external_tasks.contains(&id)
                {
                    if matches!(command.code, 4 | 6 | 9 | 10 | 18 | 20..=22 | 30 | 31 | 64 | 65
                        | 161..=176 | 452)
                    {
                        return Err(native_state_error("task command requires its suspended external VM"));
                    }
                    return Ok(Some(TaskCommandOutcome { done: true, propagate: true }));
                }
                return self.with_ordinary_task(id, |host, task| {
                    task.dispatch_control_with_buffers(kind, command, output, buffers, host, budget)
                        .map(Some).map_err(|error| host.task_error(id, error))
                });
            }
            _ => return Ok(Some(TaskCommandOutcome { done: true, propagate: true })),
        }
        Ok(Some(consumed))
    }
}
