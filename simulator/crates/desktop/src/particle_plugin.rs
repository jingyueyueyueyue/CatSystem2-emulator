//! Independent SHA-identified SDK2 effect models. No PE code is loaded or
//! called. Evidence: 20261117_D_ptclpi_* and20261117_D_ppi_*.
use cs2_script::Result;
use super::particle_vm::{ParticleCanvas, ParticleQuad, ParticleRandomSource,
    ParticleRenderSnapshot, ParticleVertex};
#[path = "particle_family.rs"]
mod family;
#[path = "particle_common.rs"]
mod common;
#[path = "particle_plugin_state.rs"]
pub(crate) mod checkpoint;

const RAIN_V2: &str = "85cd9abde12a2a580051cbc2c008fb62933f61e1571b1ba8a510af90017aba0f";
// SDK2.1 rain2: all328 instructions of10d0..1467 match the SDK2 model
// after local RVA relocation. Extra SDK callbacks are never called here.
const RAIN_V21: &str = "623e65424090b7f94b3bc4974715d32c60bc738dd06a0740430ccdfee59bd4e5";
//Public sample type1/version0x10000 is rejected by the fixed SDK2 host
//before MakeInstance (Yukikoi14e7a6..14e86f). This is a compatibility
//rejection, never an implementation of its rain algorithm.
const FIXED_HOST_REJECTED_SDK1: &str = "90266315c608b53e081fc2fa33a799178e245f621b9d49f56ee502b5bb3bb758";
const MAX_STORAGE: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
struct RainDrop {
    position: [f32; 3],
    velocity: [f32; 3],
    rotation: f32,
}

#[derive(Debug)]
pub(super) struct ParticlePlugin {
    digest: String,
    drops: Vec<RainDrop>,
    field: Option<super::particle_rain_field::RainField>,
    burst: Option<super::particle_burst::ParticleBurst>,
    family: Option<family::Family>,
    common: Option<common::CommonParticles>,
    source_position: [f32; 3],
    image_size: [f32; 2],
    canvas: ParticleCanvas,
    active: bool,
    initialized: bool,
    origin: u32,
    previous_time: u32,
    elapsed: u32,
    previous_elapsed: u32,
    submitted: bool,
}

impl ParticlePlugin {
    pub(super) fn load(bytes: &[u8], count: u32, image_size: [f32; 2], canvas: ParticleCanvas) -> Result<Self> {
        // Identity covers the complete resource, not its name/export metadata.
        // The public SDK sample is version1 and is deliberately not SDK2 rain.
        let digest = super::particle_plugin_digest::sha256(bytes);
        if digest == FIXED_HOST_REJECTED_SDK1 {
            return Err(super::native_state_error("fixed SDK2 particle host rejects type1/version0x10000 before MakeInstance"));
        }
        let field = match digest.as_str() {
            super::particle_rain_field::ATLAS_SHA => Some(super::particle_rain_field::RainFieldKind::Atlas),
            super::particle_rain_field::SCALED_SHA => Some(super::particle_rain_field::RainFieldKind::Scaled),
            _ => None,
        };
        let burst = digest == super::particle_burst::SHA;
        let common = matches!(digest.as_str(),common::COMMON_SHA|common::COMMON_LEGACY_SHA);
        let described = family::Family::supports(&digest);
        if digest != RAIN_V2 && digest != RAIN_V21 && field.is_none() && !burst && !described && !common {
            return Err(super::native_state_error("particle plugin SHA/type/version has no independent implementation"));
        }
        // Include SDK/current/saved/render snapshots in the allocation bound.
        let bytes_per_particle = if described || common { 2048 } else { 512 };
        if (count as usize).checked_mul(bytes_per_particle).is_none_or(|n| n > MAX_STORAGE) {
            return Err(super::native_state_error("particle plugin grid exceeds64 MiB"));
        }
        let family = family::Family::new(&digest,count,image_size)?;
        Ok(Self { digest, drops: if field.is_none() && !burst && !described && !common { vec![RainDrop::default(); count as usize] } else { Vec::new() },
            family, source_position: [0.;3],
            common:common.then(||common::CommonParticles::new(count,image_size)),
            burst: burst.then(|| super::particle_burst::ParticleBurst::new(count,image_size)),
            field: field.map(|kind| super::particle_rain_field::RainField::new(kind,count,image_size)), image_size, canvas,
            active: false, initialized: false, origin: 0, previous_time: 0, elapsed: 0,
            previous_elapsed: u32::MAX, submitted: false })
    }

    pub(super) fn start(&mut self, parameters: &[u8; 128], viewport: [u32; 2], resources:&super::ResourceIndex,
        random: &mut impl ParticleRandomSource) -> Result<()> {
        //14ee4f queries the actual canvas at start, not the plugin filename or
        // a synthetic default. A resize between load/start is observable.
        self.canvas.viewport = viewport;
        self.origin = 0; self.previous_time = 0;
        self.elapsed = 0; self.previous_elapsed = u32::MAX; self.submitted = false;
        if let Some(common) = self.common.as_mut() {common.start(parameters,viewport,resources,random)?;}
        else if let Some(family) = self.family.as_mut() { family.start(parameters,viewport,resources,random)?; }
        else if let Some(burst) = self.burst.as_mut() { burst.start(parameters,random)?; }
        else if let Some(field) = self.field.as_mut() { field.start(parameters,viewport,random); }
        else { for drop in &mut self.drops {
            *drop = RainDrop::default();
            respawn(drop, self.canvas.viewport, parameters, random);
        } }
        self.active = true; self.initialized = true;
        Ok(())
    }

    // SDK finish (rain RVA12f0) is a real no-op; storage is retained until
    // release/reload destroys this object. Stop still removes the draw callback.
    pub(super) fn stop(&mut self) { self.active = false; }
    pub(super) fn active(&self) -> bool { self.active }

    //14e5f0 creates a fresh instance and initializes canonical quads; it does
    //NOT call SDK start. Preserve the task's existing active bit on reload
    //without inventing a start callback or consuming any random samples.
    pub(super) fn retain_loaded_active(&mut self, active: bool) { self.active = active; }

    //14f490 fills the SDK source before each update; +24/+28/+2c are the
    //source's3D coordinates. Explicit detach zeros them; a failed lookup
    //retains the last values, matching the same portable source service as VM.
    pub(super) fn refresh_source(&mut self, id: i32, source: Option<super::particle_vm::ParticleSource>) {
        if id == -1 { self.source_position = [0.;3]; }
        else if let Some(source) = source { self.source_position = source.position_3d; }
    }

    pub(super) fn reset_extent(&mut self, image_size: [f32; 2]) -> Result<()> {
        if self.active { return Err(super::native_state_error("bind plugin extent before start")); }
        self.image_size = image_size; self.submitted = false;
        if let Some(field) = self.field.as_mut() { field.reset_extent(image_size); }
        if let Some(burst) = self.burst.as_mut() { burst.reset_extent(image_size); }
        if let Some(family) = self.family.as_mut() { family.reset_extent(image_size); }
        if let Some(common) = self.common.as_mut() {common.reset_extent(image_size);}
        Ok(())
    }

    pub(super) fn update(&mut self, now: u32, parameters: &[u8; 128], random: &mut impl ParticleRandomSource) {
        if !self.active { return; }
        // Same DWORD task clock/sentinel as14e424. First update has delta1,
        // including when the scheduler's actual tick is0; never substitute ms.
        if self.origin == 0 {
            self.origin = now; self.elapsed = 0; self.previous_elapsed = u32::MAX;
        } else { self.elapsed = self.elapsed.wrapping_add(now.wrapping_sub(self.previous_time)); }
        self.previous_time = now;
        let delta = self.elapsed.wrapping_sub(self.previous_elapsed) as f32;
        if !self.initialized { self.previous_elapsed = self.elapsed; return; }
        if let Some(common) = self.common.as_mut() {
            common.update(delta,self.source_position,random);
            self.previous_elapsed=self.elapsed;self.submitted=true;return;
        }
        if let Some(family) = self.family.as_mut() {
            family.update(delta,parameters,self.source_position,random);
            //SDK04 clears active in the owning task, including before its
            //afterimage pool drains. Storage still belongs to release/reload.
            if family.finish_requested() {self.active=false;}
            self.previous_elapsed = self.elapsed; self.submitted = true; return;
        }
        if let Some(burst) = self.burst.as_mut() {
            burst.update(delta,parameters);
            self.previous_elapsed = self.elapsed; self.submitted = true; return;
        }
        if let Some(field) = self.field.as_mut() {
            field.update(delta,random);
            self.previous_elapsed = self.elapsed; self.submitted = true; return;
        }
        let limit = self.canvas.viewport[1] as f64 + 200.;
        for drop in &mut self.drops {
            for axis in 0..3 {
                drop.position[axis] = (f64::from(drop.velocity[axis]) * f64::from(delta)
                    + f64::from(drop.position[axis])) as f32;
            }
            if f64::from(drop.position[1]) > limit { respawn(drop, self.canvas.viewport, parameters, random); }
        }
        self.previous_elapsed = self.elapsed; self.submitted = true;
    }

    pub(super) fn snapshot(&self, opacity: u32) -> ParticleRenderSnapshot {
        if let Some(common) = self.common.as_ref() {
            return common.snapshot(self.canvas,self.active && self.initialized,self.submitted,self.elapsed,self.previous_elapsed,opacity);
        }
        if let Some(family) = self.family.as_ref() {
            return family.snapshot(self.canvas,self.active && self.initialized,self.submitted,self.elapsed,self.previous_elapsed,opacity);
        }
        if let Some(burst) = self.burst.as_ref() {
            return burst.snapshot(self.canvas,self.active && self.initialized,self.submitted,self.elapsed,self.previous_elapsed,opacity);
        }
        if let Some(field) = self.field.as_ref() {
            return field.snapshot(self.canvas,self.active && self.initialized,self.submitted,self.elapsed,self.previous_elapsed,opacity);
        }
        let [w,h] = self.image_size;
        let positions = [[0.,0.],[w,0.],[0.,h],[w,h]];
        let uv = [[0.,0.],[1.,0.],[0.,1.],[1.,1.]];
        ParticleRenderSnapshot { active: self.active && self.initialized, frame_complete: true,
            elapsed: self.elapsed, previous_elapsed: self.previous_elapsed,
            current_quad: self.drops.len() as u32, render_mode: 0, opacity, canvas: self.canvas,
            quads: self.drops.iter().enumerate().map(|(i, drop)| {
                //14e200 subtracts pivot, then scale/XYZ rotations/translation.
                // Rain changes only Z. Bake that matrix into the vertices and
                // leave ordinary projector offsets zero (it subtracts them).
                // MatrixRotationZ stores its sin/cos as float before SSE
                // matrix multiplication (Yuki33af6..33b71).
                let (sin,cos) = f64::from(drop.rotation).sin_cos();
                let (sin,cos) = (sin as f32,cos as f32);
                let vertices = std::array::from_fn(|n| {
                    let [mut x,mut y] = positions[n];
                    if self.submitted {
                        x -= w * 0.5; y -= h * 0.5;
                        let a = x; let b = y;
                        x = a * cos - b * sin;
                        y = a * sin + b * cos;
                        x += drop.position[0]; y += drop.position[1];
                    }
                    ParticleVertex { position: [x,y,if self.submitted { drop.position[2] } else { 0. }],
                        uv: uv[n], ..Default::default() }
                });
                ParticleQuad { index: i as u32, vertices, offset: [0.;4], depth_bias:0., flag: 1 }
            }).collect() }
    }
}

fn respawn(drop: &mut RainDrop, viewport: [u32;2], parameters: &[u8;128], random: &mut impl ParticleRandomSource) {
    let sample = |r: &mut _| f64::from(f32::from_bits(ParticleRandomSource::next_float_bits(r)));
    drop.position[0] = (2. * f64::from(viewport[0]) * sample(random) - 0.5 * f64::from(viewport[0])) as f32;
    drop.position[1] = (-2. * f64::from(viewport[1]) * sample(random)) as f32;
    let word = |i: usize| f32::from_le_bytes(parameters[i * 4..i * 4 + 4].try_into().unwrap());
    let radians = |degrees: f32| (f64::from(degrees) * f64::from(std::f32::consts::PI) / 180.) as f32;
    let min = radians(word(1)); let max = radians(word(2));
    let angle = (f64::from(min) + sample(random) * (f64::from(max) - f64::from(min))) as f32;
    let speed = word(0); let (sin,cos) = f64::from(angle).sin_cos();
    let (sin,cos) = (sin as f32,cos as f32);
    // SDK callback+5c rotates clockwise. The plugin argument slot is reused:
    //1070:1141/114f and1390:13c6/13d4 read NEGATIVE ANGLE, not speed.
    drop.velocity = [speed * sin, speed * cos, 0.];
    drop.rotation = -angle;
}
