//! Versions observed at actual render and scene submission boundaries.
use std::{collections::BTreeMap, time::Instant};
use cs2_runtime::emote::{CpuPlayer, CpuPlayerRenderState, CpuViewport, EmoteFrameIdentity, EmoteRasterFrame};
use crate::perf_detail;

#[derive(Default)]
pub(super) struct Progress {
    entries: BTreeMap<u32, Entry>,
    rotation: usize,
}

#[derive(Default)]
struct Entry {
    input: Option<CpuPlayerRenderState>,
    input_rev: u64,
    rendered_rev: u64,
    surface_input_rev: u64,
    scene_rev: u64,
    surface: Option<EmoteFrameIdentity>,
    surface_rev: u64,
    scene_surface_rev: u64,
    renders: u64,
    scene_calls: u64,
    changed: Option<Instant>,
    surface_changed: Option<Instant>,
    known_input: bool,
    motion_time: u64,
}

impl Progress {
    pub(super) fn remove(&mut self, task: u32) { self.entries.remove(&task); }

    pub(super) fn request(&mut self, task: u32, player: Option<(&CpuPlayer, CpuViewport)>) {
        if !perf_detail::enabled() || (self.entries.len() >= 16 && !self.entries.contains_key(&task)) { return; }
        let entry = self.entries.entry(task).or_default();
        entry.known_input = false;
        if let Some((player, viewport)) = player && let Ok(variables) = player.effective_variables() {
            entry.known_input = true;
            entry.motion_time = player.frame.to_bits();
            if !entry.input.as_ref().is_some_and(|old| old.matches(player, viewport, &variables)) {
                entry.input = Some(player.render_state_snapshot(viewport, &variables));
                entry.input_rev = entry.input_rev.saturating_add(1);
                entry.changed = Some(Instant::now());
            }
        }
    }

    pub(super) fn rendered(&mut self, task: u32, frame: &EmoteRasterFrame) {
        if !perf_detail::enabled() { return; }
        let Some(entry) = self.entries.get_mut(&task) else { return; };
        if !entry.surface.as_ref().is_some_and(|old| old.matches(frame)) {
            entry.surface = Some(frame.diagnostic_identity());
            entry.surface_rev = entry.surface_rev.saturating_add(1);
            entry.surface_input_rev = entry.input_rev;
            entry.surface_changed = Some(Instant::now());
        }
        entry.rendered_rev = entry.input_rev;
        entry.renders = entry.renders.saturating_add(1);
    }

    pub(super) fn scene_applied(&mut self, task: u32, frame: &EmoteRasterFrame) {
        if !perf_detail::enabled() { return; }
        if let Some(entry) = self.entries.get_mut(&task) {
            let matches = entry.surface.as_ref().is_some_and(|identity| identity.matches(frame));
            entry.scene_rev = if matches { entry.surface_input_rev } else { 0 };
            entry.scene_surface_rev = if matches { entry.surface_rev } else { 0 };
            entry.scene_calls = entry.scene_calls.saturating_add(1);
        }
    }

    pub(super) fn trace(&mut self, state: &cs2_runtime::emote::EmoteServices) -> (usize, usize) {
        let mut kept = 0;
        let start = self.rotation % self.entries.len().max(1);
        self.rotation = self.rotation.wrapping_add(1);
        for (&task, entry) in self.entries.iter().cycle().skip(start).take(self.entries.len()) {
            let Some(task_state) = state.state(task) else { continue; };
            if kept == 4 { break; }
            let tick = task_state.last_completed_tick;
            perf_detail::visual_note("visual_pose", format!(
                "task={task} task_now={} increment={} imports={} input_known={} pose_in={} render_in={} surface_input={} scene_input={} surface_rev={} surface_scene={} input_idle_ms={} surface_idle_ms={} renders={} scene_calls={} motion_bits={:x}",
                tick.map_or(0, |tick| tick.task_elapsed), tick.map_or(0, |tick| tick.increment),
                task_state.successful_resource_imports, u8::from(entry.known_input), entry.input_rev, entry.rendered_rev,
                entry.surface_input_rev, entry.scene_rev, entry.surface_rev, entry.scene_surface_rev,
                entry.changed.map_or(0, |at| at.elapsed().as_millis()),
                entry.surface_changed.map_or(0, |at| at.elapsed().as_millis()), entry.renders, entry.scene_calls, entry.motion_time));
            kept += 1;
        }
        (state.task_ids().count(), kept)
    }
}
