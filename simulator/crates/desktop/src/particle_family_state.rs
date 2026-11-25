//! Bind a private checkpoint to the model of the verified resource.
use super::*;
use anyhow::{Context, Result, bail};
use cs2_runtime::graph_serde::{Deserialize, Serialize};
pub(crate) use paths::checkpoint::ModuleEmissionState;

// Admit private codecs for the models exercised by the retained scene inventory.
fn supports(profile: &profiles::Profile) -> bool {
    match profile.model {
        Model::Sprite(_) => true,
        Model::Path(path) => path.kind == profiles::PathKind::Sakura || paths::checkpoint::supports_core(path.kind),
        // The two center layouts and the 740-threshold rising model
        // have checkpoints. The 800-threshold model has no retained scene.
        Model::Field(field) => matches!(field.fadeout_damping_below, None | Some(740.)),
        Model::Question { oscillates_after_hold } => oscillates_after_hold,
    }
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) enum FamilyState {
    Sakura(paths::checkpoint::SakuraState),
    Reaction(sprites::SpriteFamily),
    CorePath(paths::checkpoint::CorePathState),
    Particles(ParticleFamilyState),
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct ParticleFamilyState {
    particles: Vec<Particle>, image: [f32; 2], viewport_half: [f32; 2],
    render_mode: u32, counter: i32, active_count: usize, center: [f32; 2],
    field_centers: [[f32; 2]; 3], field_scale_parameter: f32,
    gravity: f32, horizontal: f32, peak_alpha: i32, initial_scale: f32,
    question_durations: [i32; 5], question_rotation: f32,
}

impl ParticleFamilyState {
    fn validate(&self, count: usize, image: [f32; 2]) -> Result<()> {
        if self.particles.len() != count || self.image != image || self.active_count > count
            || count.checked_mul(2048).is_none_or(|n| n > 64 * 1024 * 1024) {
            bail!("field/question checkpoint differs from its exact model/grid/extent");
        }
        Ok(())
    }
}

impl Family {
    pub(crate) fn capture_checkpoint(&self) -> Result<FamilyState> {
        if !supports(&self.profile) {
            bail!("PPI model has no private checkpoint codec");
        }
        match self.profile.model {
            Model::Path(path) if path.kind == profiles::PathKind::Sakura && self.sprite.is_none() && self.particles.is_empty() => Ok(FamilyState::Sakura(
                self.path.as_ref().context("Sakura path absent")?.capture_sakura()?)),
            Model::Sprite(_) if self.path.is_none() && self.particles.is_empty() => Ok(FamilyState::Reaction(
                self.sprite.as_ref().context("reaction sprite state absent")?.capture_checkpoint()?)),
            Model::Path(profile) if paths::checkpoint::supports_core(profile.kind) && self.sprite.is_none() && self.particles.is_empty() =>
                Ok(FamilyState::CorePath(self.path.as_ref().context("core path state absent")?.capture_core()?)),
            Model::Field(_) | Model::Question { .. } if self.path.is_none() && self.sprite.is_none() => {
                let state = ParticleFamilyState { particles: self.particles.clone(), image: self.image,
                    viewport_half: self.viewport_half, render_mode: self.render_mode, counter: self.counter,
                    active_count: self.active_count, center: self.center, field_centers: self.field_centers,
                    field_scale_parameter: self.field_scale_parameter, gravity: self.gravity,
                    horizontal: self.horizontal, peak_alpha: self.peak_alpha, initial_scale: self.initial_scale,
                    question_durations: self.question_durations, question_rotation: self.question_rotation };
                state.validate(self.particles.len(), self.image)?;
                Ok(FamilyState::Particles(state))
            }
            _ => bail!("PPI private checkpoint consumer differs from its exact descriptor"),
        }
    }

    pub(crate) fn restore_checkpoint(&mut self, count: u32, image: [f32; 2], state: FamilyState) -> Result<()> {
        if !supports(&self.profile) { bail!("PPI model has no private checkpoint codec"); }
        let family = self;
        match state {
            FamilyState::Sakura(state) if matches!(family.profile.model, Model::Path(path)
                if path.kind == profiles::PathKind::Sakura) => {
                family.path.as_mut().context("saved Sakura path absent")?.restore_sakura(state)?;
            }
            FamilyState::Reaction(state) => {
                let profile = match family.profile.model {
                    Model::Sprite(profile) => profile,
                    _ => bail!("reaction checkpoint rebound a different model"),
                };
                state.validate_checkpoint(profile, count, image)?;
                family.sprite = Some(state);
            }
            FamilyState::CorePath(state) => {
                family.path.as_mut().context("saved core path model absent")?.restore_core(state)?;
            }
            FamilyState::Particles(state) if matches!(family.profile.model, Model::Field(_) | Model::Question { .. }) => {
                state.validate(family.particles.len(), image)?;
                family.particles = state.particles; family.viewport_half = state.viewport_half;
                family.render_mode = state.render_mode; family.counter = state.counter;
                family.active_count = state.active_count; family.center = state.center;
                family.field_centers = state.field_centers; family.field_scale_parameter = state.field_scale_parameter;
                family.gravity = state.gravity; family.horizontal = state.horizontal;
                family.peak_alpha = state.peak_alpha; family.initial_scale = state.initial_scale;
                family.question_durations = state.question_durations; family.question_rotation = state.question_rotation;
            }
            _ => bail!("PPI checkpoint state variant differs from its resource model"),
        }
        Ok(())
    }
}
