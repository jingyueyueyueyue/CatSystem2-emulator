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
        // Recover the model from the same resource used by live loading, then
        // bind the saved state to it without running a start callback.
        let mut plugin = ParticlePlugin::load(bytes, count, image_size, self.canvas)?;
        plugin.family.as_mut().context("saved PPI family absent")?
            .restore_checkpoint(count, image_size, self.family)?;
        plugin.source_position = self.source_position;
        plugin.active = self.active;
        plugin.initialized = self.initialized;
        plugin.origin = self.origin;
        plugin.previous_time = self.previous_time;
        plugin.elapsed = self.elapsed;
        plugin.previous_elapsed = self.previous_elapsed;
        plugin.submitted = self.submitted;
        Ok(plugin)
    }
}
