//! Bind a private checkpoint to the same fixed SHA descriptor as live load.
use super::*;
use anyhow::{Context, Result, bail};
use cs2_runtime::graph_serde::{Deserialize, Serialize};
pub(crate) use paths::checkpoint::ModuleEmissionState;

pub(crate) const SAKURA_SHA: &str = "154233ec97c89dcf98b8054fc60e5ddceb58f814dd3a2ce50ce9fe994698d892";
// Existing scene inventory: Question238+40, dark_tri28, wc_dark6, wc_darkund2.
// Other zero-reference Field/Question variants stay outside this delivery.
fn supports_particles(sha: &str) -> bool {
    matches!(sha,
        "f9c6468d0196fe58533b0a35a109f5c4a3ecc66b4775ac396c334cd180861a82"
        | "de9de8d2b26be6e1896dd4d078eac461778cd15fe019497f5bf1cb240cbe741e"
        | "bddb1bf517e00dd04625b2f312eb3b7a085b51f65342e710e37348dc5d5ec5d4"
        | "fa9e19b5ae623f3fe83f7b93c2afb467ad7574d0e5a610c925ee34d03e2d4186"
        | "75b5eba8dfb90f4fb92f103776577de2a782d37c53a70a0196935ff93ecce851")
}
// Every Sprite descriptor here has a real producer in the retained usage
// inventory. Identity still comes from PROFILES' complete SHA, never a name or
// a serialized profile supplied by the save file.
pub(crate) fn supports(sha: &str) -> bool {
    sha == SAKURA_SHA || profiles::identify(sha).is_some_and(|p| match p.model {
        Model::Sprite(_) => true,
        Model::Path(profile) => paths::checkpoint::supports_core(profile.kind),
        Model::Field(_) | Model::Question { .. } => supports_particles(sha),
    })
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
        if !supports(self.profile.sha256) {
            bail!("PPI family private checkpoint is not implemented for this SHA");
        }
        match self.profile.model {
            Model::Path(_) if self.profile.sha256 == SAKURA_SHA && self.sprite.is_none() && self.particles.is_empty() => Ok(FamilyState::Sakura(
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

    pub(crate) fn prepare_checkpoint(sha: &str, count: u32, image: [f32; 2], state: FamilyState) -> Result<Self> {
        if !supports(sha) { bail!("PPI private checkpoint SHA has no admitted state codec"); }
        // Constructor only allocates canonical storage. No start/random call.
        let mut family = Self::new(sha, count, image)?.context("saved PPI descriptor absent")?;
        match state {
            FamilyState::Sakura(state) if sha == SAKURA_SHA => {
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
            FamilyState::Particles(state) if supports_particles(sha) => {
                state.validate(family.particles.len(), image)?;
                family.particles = state.particles; family.viewport_half = state.viewport_half;
                family.render_mode = state.render_mode; family.counter = state.counter;
                family.active_count = state.active_count; family.center = state.center;
                family.field_centers = state.field_centers; family.field_scale_parameter = state.field_scale_parameter;
                family.gravity = state.gravity; family.horizontal = state.horizontal;
                family.peak_alpha = state.peak_alpha; family.initial_scale = state.initial_scale;
                family.question_durations = state.question_durations; family.question_rotation = state.question_rotation;
            }
            _ => bail!("PPI checkpoint state variant differs from its SHA descriptor"),
        }
        Ok(family)
    }
}
