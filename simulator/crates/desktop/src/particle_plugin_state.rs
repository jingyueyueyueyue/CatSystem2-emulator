//! Private SDK state, not a baked last-frame substitute for the effect.
use super::*;
use anyhow::{Context, Result, bail};
use cs2_runtime::graph_serde::{Deserialize, Serialize};
pub(crate) use family::checkpoint::ModuleEmissionState;

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct PluginState {
    digest: String, family: family::checkpoint::FamilyState,
    source_position: [f32; 3], image_size: [f32; 2], canvas: ParticleCanvas,
    active: bool, initialized: bool, origin: u32, previous_time: u32,
    elapsed: u32, previous_elapsed: u32, submitted: bool,
}

impl ParticlePlugin {
    pub(crate) fn checkpoint_digest(&self) -> &str { &self.digest }

    pub(crate) fn capture_checkpoint(&self) -> Result<PluginState> {
        if !family::checkpoint::supports(&self.digest) || !self.drops.is_empty()
            || self.field.is_some() || self.burst.is_some() || self.common.is_some() {
            bail!("PPI private checkpoint is not implemented for this SHA (live model is retained)");
        }
        Ok(PluginState { digest: self.digest.clone(),
            family: self.family.as_ref().context("saved PPI family absent")?.capture_checkpoint()?,
            source_position: self.source_position, image_size: self.image_size, canvas: self.canvas,
            active: self.active, initialized: self.initialized, origin: self.origin, previous_time: self.previous_time,
            elapsed: self.elapsed, previous_elapsed: self.previous_elapsed, submitted: self.submitted })
    }
}

impl PluginState {
    pub(crate) fn prepare(self, bytes: &[u8], count: u32, image_size: [f32; 2]) -> Result<ParticlePlugin> {
        let digest = super::super::particle_plugin_digest::sha256(bytes);
        if digest != self.digest || image_size != self.image_size {
            bail!("saved PPI source identity or texture extent changed");
        }
        let family = family::Family::prepare_checkpoint(&digest, count, image_size, self.family)?;
        Ok(ParticlePlugin { digest, drops: Vec::new(), field: None, burst: None, common: None, family: Some(family),
            source_position: self.source_position, image_size, canvas: self.canvas, active: self.active,
            initialized: self.initialized, origin: self.origin, previous_time: self.previous_time,
            elapsed: self.elapsed, previous_elapsed: self.previous_elapsed, submitted: self.submitted })
    }
}
