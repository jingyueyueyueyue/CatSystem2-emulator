//! Private storage for observed path producers. The two admitted module-shared
//! emission counters are captured once with the entire graph, not per instance.
use super::*;
use anyhow::{Result, bail};

/// These counters live across start/stop/reload, including a graph with no
/// currently loaded instance. Prepare owns their values without writing the
/// live module; only the enclosing graph's final commit may install them.
#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct ModuleEmissionState { concent: u32, dash: u32 }

impl ModuleEmissionState {
    pub(crate) fn capture() -> Self {
        Self { concent: CONCENT_ACTIVE.load(Ordering::Relaxed), dash: DASH_ACTIVE.load(Ordering::Relaxed) }
    }

    pub(crate) fn commit(self) {
        CONCENT_ACTIVE.store(self.concent, Ordering::Relaxed);
        DASH_ACTIVE.store(self.dash, Ordering::Relaxed);
    }
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct SakuraState {
    drops: Vec<Drop>, image: [f32; 2], viewport_half: [f32; 2],
    main_count: usize, active_count: usize, child_count: usize,
    falling: falling::FallingConfig,
}

pub(crate) fn supports_core(kind: PathKind) -> bool {
    matches!(kind, PathKind::Note | PathKind::Trail | PathKind::Sigh | PathKind::Shy
        | PathKind::Kirakira | PathKind::Oko | PathKind::Warukira | PathKind::Hotaru
        | PathKind::Energy | PathKind::Steam | PathKind::SteamAligned
        | PathKind::Concent | PathKind::Dash | PathKind::Rising3 | PathKind::Rising4)
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct CorePathState {
    kind: PathKind, drops: Vec<Drop>, image: [f32; 2], viewport_half: [f32; 2],
    center: [f32; 3], scale: f32, factor: f32, durations: [i32; 3], direction: i32,
    main_count: usize, active_count: usize, child_count: usize,
    shy: ShyConfig, hotaru: HotaruConfig, radius: f32, angle: f32, stream_render_mode: u32,
    concent_duration: f32, concent_spread: [f32; 2], dash: DashConfig,
}

impl CorePathState {
    fn validate(&self) -> Result<()> {
        // Concent/Dash start repartitions the pool without resetting its last
        // observed active count. Their next update replaces it from the
        // shared module counter before consuming the parent pool.
        let active_limit = if matches!(self.kind, PathKind::Concent | PathKind::Dash) {
            self.drops.len()
        } else { self.main_count };
        if !supports_core(self.kind)
            || self.drops.len().checked_mul(2048).is_none_or(|n| n > 64 * 1024 * 1024)
            || self.main_count > self.drops.len()
            || self.active_count > active_limit
            || self.main_count.checked_mul(self.child_count).is_none_or(|n| n > self.drops.len() - self.main_count)
            || self.kind == PathKind::Kirakira && self.main_count > self.drops.len() - self.main_count
            || self.drops.iter().any(|d| d.path.count > 20) || self.shy.baseline.count > 20 {
            bail!("path checkpoint descriptor/pool/trajectory storage is invalid");
        }
        Ok(())
    }
}

impl PathFamily {
    pub(crate) fn capture_core(&self) -> Result<CorePathState> {
        let state = CorePathState { kind: self.profile.kind, drops: self.drops.clone(),
            image: self.image, viewport_half: self.viewport_half, center: self.center,
            scale: self.scale, factor: self.factor, durations: self.durations, direction: self.direction,
            main_count: self.main_count, active_count: self.active_count, child_count: self.child_count,
            shy: self.shy.clone(), hotaru: self.hotaru.clone(), radius: self.radius,
            angle: self.angle, stream_render_mode: self.stream_render_mode,
            concent_duration: self.concent_duration, concent_spread: self.concent_spread, dash: self.dash.clone() };
        state.validate()?;
        Ok(state)
    }

    pub(crate) fn restore_core(&mut self, state: CorePathState) -> Result<()> {
        state.validate()?;
        if self.profile.kind != state.kind || self.drops.len() != state.drops.len() || self.image != state.image {
            bail!("path checkpoint differs from its exact descriptor/grid/extent");
        }
        // These selected live algorithms consume only this shared storage.
        // Keep every path knot, random result and parent's captured geometry;
        // never call spawn/start or rebuild trails from the completed frame.
        self.drops = state.drops; self.viewport_half = state.viewport_half; self.center = state.center;
        self.scale = state.scale; self.factor = state.factor; self.durations = state.durations;
        self.direction = state.direction; self.main_count = state.main_count;
        self.active_count = state.active_count; self.child_count = state.child_count;
        self.shy = state.shy; self.hotaru = state.hotaru; self.radius = state.radius;
        self.angle = state.angle; self.stream_render_mode = state.stream_render_mode;
        self.concent_duration = state.concent_duration; self.concent_spread = state.concent_spread;
        self.dash = state.dash;
        Ok(())
    }

    pub(crate) fn capture_sakura(&self) -> Result<SakuraState> {
        if self.profile.kind != PathKind::Sakura { bail!("path family has no private checkpoint codec"); }
        let state = SakuraState { drops: self.drops.clone(), image: self.image, viewport_half: self.viewport_half,
            main_count: self.main_count, active_count: self.active_count, child_count: self.child_count,
            falling: self.falling.clone() };
        state.validate()?;
        Ok(state)
    }

    pub(crate) fn restore_sakura(&mut self, state: SakuraState) -> Result<()> {
        if self.profile.kind != PathKind::Sakura || self.drops.len() != state.drops.len() || self.image != state.image {
            bail!("Sakura checkpoint differs from its exact descriptor/grid/extent");
        }
        state.validate()?;
        self.drops = state.drops; self.viewport_half = state.viewport_half;
        self.main_count = state.main_count; self.active_count = state.active_count;
        self.child_count = state.child_count; self.falling = state.falling;
        Ok(())
    }
}

impl SakuraState {
    fn validate(&self) -> Result<()> {
        if self.drops.len().checked_mul(2048).is_none_or(|n| n > 64 * 1024 * 1024)
            || self.main_count > self.drops.len() || self.active_count > self.main_count
            || self.main_count.checked_mul(self.child_count).is_none_or(|n| n > self.drops.len() - self.main_count)
            || self.drops.iter().any(|d| d.path.count > 20) {
            bail!("Sakura checkpoint parent/trail/path storage is invalid");
        }
        Ok(())
    }
}
