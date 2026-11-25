//! The distinct44-byte rain state machines, recovered from fixed SDK2.1 PEs.
//! This is not the SDK2 rain algorithm and never executes a plugin binary.
use super::particle_vm::{ParticleCanvas, ParticleQuad, ParticleRandomSource,
    ParticleRenderSnapshot, ParticleVertex};

#[derive(Clone, Copy, Debug)]
pub(super) enum RainFieldKind { Atlas, Scaled }

#[derive(Clone, Debug, Default)]
struct Drop {
    clock: u32,
    phase_origin: u32,
    phase: u32,
    position: [f32;3],
    velocity: [f32;3],
    lifetime: f32,
    alpha_target: f32,
    alpha: u8,
    rotation: f32,
    scale: f32,
    uv_origin: [f32;2],
}

#[derive(Debug, Default)]
struct Configuration {
    lifetime: i32,
    lifetime_jitter: i32,
    angle: f32,
    angle_jitter: f32,
    birth_period: u32,
    birth_increment: u32,
    scale: f32,
    center_x: f32,
    width: f32,
}

#[derive(Debug)]
pub(super) struct RainField {
    kind: RainFieldKind,
    drops: Vec<Drop>,
    config: Configuration,
    image: [f32;2],
    viewport: [u32;2],
    active_count: u32,
    render_mode: u32,
}

impl RainField {
    pub(super) fn new(kind: RainFieldKind, count: u32, image: [f32;2]) -> Self {
        Self { kind, drops: vec![Drop::default();count as usize], config: Configuration::default(),
            image, viewport: [0;2], active_count: 0, render_mode: 0 }
    }

    pub(super) fn reset_extent(&mut self, image: [f32;2]) { self.image = image; }

    pub(super) fn start(&mut self, p: &[u8;128], viewport: [u32;2], random: &mut impl ParticleRandomSource) {
        let word = |i: usize| f32::from_le_bytes(p[i*4..i*4+4].try_into().unwrap());
        self.viewport = viewport;
        self.config = Configuration {
            lifetime: trunc_i32(f64::from(word(0))).max(1),
            lifetime_jitter: trunc_i32(f64::from(word(1))).max(0),
            angle: ((f64::from(word(2))/100.) as f32).clamp(-1.,1.),
            angle_jitter: ((f64::from(word(3))/100.) as f32).clamp(-0.2,0.2),
            birth_period: 61i32.wrapping_sub(trunc_i32(f64::from(word(4)))).clamp(1,60) as u32,
            birth_increment: trunc_i32(f64::from(word(5))) as u32,
            scale: (f64::from(word(7))/100.) as f32,
            center_x: word(8), width: word(9),
        };
        //350e:11c1 only sets the bit; a later start with0 does not clear it.
        if matches!(self.kind,RainFieldKind::Scaled) && trunc_i32(f64::from(word(10))) != 0 { self.render_mode = 1; }
        self.active_count = if trunc_i32(f64::from(word(6))) != 0 { self.drops.len() as u32 / 2 } else { 0 };
        for (i,drop) in self.drops.iter_mut().enumerate() {
            *drop = Drop::default();
            spawn(self.kind, &self.config, viewport, drop, random);
            if i < self.active_count as usize {
                //1690/16f0 pre-age XY only; the44-byte clock is truncated.
                drop.clock = trunc_i32(sample(random)*f64::from(drop.lifetime)) as u32;
                for axis in 0..2 {
                    drop.position[axis] = (f64::from(drop.velocity[axis]) * f64::from(drop.clock as i32)
                        + f64::from(drop.position[axis])) as f32;
                }
            }
        }
    }

    pub(super) fn update(&mut self, delta: f32, random: &mut impl ParticleRandomSource) {
        //SDK callback+8 is the shared INTEGER stream. It is consumed once per
        //Body even at zero count, independently of all floating spawn samples.
        if random.next_integer() % self.config.birth_period == 0 {
            self.active_count = self.active_count.wrapping_add(self.config.birth_increment);
        }
        self.active_count = self.active_count.min(self.drops.len() as u32);
        let tick = trunc_i32(f64::from(delta)) as u32;
        for drop in &mut self.drops[..self.active_count as usize] {
            match drop.phase {
                0 => {
                    //The PE deliberately adds velocity once perBody. Delta is
                    //used for the phase clock below, not this XY displacement.
                    for axis in 0..2 { drop.position[axis] =
                        (f64::from(drop.velocity[axis])+f64::from(drop.position[axis])) as f32; }
                    let elapsed = drop.clock.wrapping_sub(drop.phase_origin) as i32 as f32;
                    //Shared callback1b0410: linear0..target over EXACTLY2ticks.
                    let alpha = if elapsed <= 0. { 0. } else if elapsed >= 2. { drop.alpha_target }
                        else { elapsed / 2. * drop.alpha_target };
                    drop.alpha = trunc_i32(f64::from(alpha)) as u8;
                    if f64::from(drop.position[1]) > f64::from(self.viewport[1])+200. {
                        drop.phase = 2; drop.phase_origin = drop.clock;
                    }
                }
                1 => {
                    for axis in 0..3 { drop.position[axis] =
                        (f64::from(drop.velocity[axis])+f64::from(drop.position[axis])) as f32; }
                    if drop.clock as i32 > self.config.lifetime { drop.phase = 10; drop.phase_origin = drop.clock; }
                }
                _ => spawn(self.kind, &self.config, self.viewport, drop, random),
            }
            drop.clock = drop.clock.wrapping_add(tick);
        }
    }

    pub(super) fn snapshot(&self, canvas: ParticleCanvas, active: bool, submitted: bool,
        elapsed: u32, previous_elapsed: u32, opacity: u32) -> ParticleRenderSnapshot {
        let [w,h] = self.image;
        let positions = [[0.,0.],[w,0.],[0.,h],[w,h]];
        let base_uv = [[0.,0.],[1.,0.],[0.,1.],[1.,1.]];
        ParticleRenderSnapshot { active, frame_complete: true, elapsed, previous_elapsed,
            current_quad: self.drops.len() as u32, render_mode: self.render_mode,
            opacity, canvas, quads: self.drops.iter().enumerate().map(|(i,d)| {
                let (sin,cos) = f64::from(d.rotation).sin_cos(); let (sin,cos) = (sin as f32,cos as f32);
                let vertices = std::array::from_fn(|v| {
                    let [mut x,mut y] = positions[v]; let mut uv = base_uv[v];
                    if submitted {
                        x = (x-w*0.5)*d.scale; y = (y-h*0.5)*d.scale;
                        let [a,b] = [x,y]; x = a*cos-b*sin+d.position[0]; y = a*sin+b*cos+d.position[1];
                        if matches!(self.kind,RainFieldKind::Atlas) {
                            uv = [uv[0]*0.5+d.uv_origin[0],uv[1]*0.5+d.uv_origin[1]];
                        }
                    }
                    ParticleVertex { position: [x,y,if submitted { d.position[2] } else { 0. }],
                        color: if submitted { 0x00ff_ffff | u32::from(d.alpha)<<24 } else { u32::MAX },
                        uv, ..Default::default() }
                });
                ParticleQuad { index:i as u32, vertices, offset:[0.;4],depth_bias:0.,flag:1 }
            }).collect() }
    }
}

fn spawn(kind: RainFieldKind, c: &Configuration, viewport: [u32;2], d: &mut Drop,
    random: &mut impl ParticleRandomSource) {
    match kind {
        RainFieldKind::Atlas => {
            let i = (random.next_integer() & 3) as usize;
            //Read-only6ab38b table RVA11d10, four exact UV pairs.
            const UV: [[f32;2];4] = [[0.,0.],[0.5,0.],[0.,0.5],[0.5,0.5]];
            d.uv_origin = UV[i]; d.scale = 1.;
            let width = trunc_i32(f64::from(viewport[0])*2.799999952316284) as f32;
            d.position[0] = (sample(random)*f64::from(width) - (f64::from(width)-f64::from(viewport[0]))*0.5) as f32;
        }
        RainFieldKind::Scaled => {
            d.scale = (sample(random)*f64::from(c.scale)) as f32;
            d.position[0] = (sample(random)*f64::from(c.width)+f64::from(c.center_x)-f64::from(c.width)*0.5) as f32;
        }
    }
    d.position[1] = -100.; d.position[2] = (sample(random)*10.) as f32;
    d.alpha = 0;
    d.rotation = (sample(random)*f64::from(c.angle_jitter)-f64::from(c.angle_jitter)*0.5+f64::from(c.angle)) as f32;
    d.clock = 0; d.phase_origin = 0; d.phase = 0;
    d.lifetime = (sample(random)*f64::from(c.lifetime_jitter)+f64::from(c.lifetime)) as f32;
    d.alpha_target = (random.next_integer()%70+100) as f32;
    //Local1b30 first quantizes its angle through _ftol(angle*1000)/1000.
    //The SDK rotation field itself remains unquantized for the host renderer.
    let angle = (f64::from(trunc_i32(f64::from(d.rotation)*1000.))/1000.) as f32;
    let (sin,cos) = f64::from(angle).sin_cos();
    let scale = (1020./f64::from(d.lifetime)) as f32;
    d.velocity = [(f64::from(-(sin as f32))*f64::from(scale)) as f32,
        (f64::from(cos as f32)*f64::from(scale)) as f32,0.];
}

fn sample(random: &mut impl ParticleRandomSource) -> f64 { f64::from(f32::from_bits(random.next_float_bits())) }
//MSVC _ftol2's indefinite result differs from Rust's saturating `as` on NaN/
//overflow. Preserve the DWORD and its later signed/unsigned comparisons.
fn trunc_i32(value: f64) -> i32 {
    if !value.is_finite() || value >= 2147483648. || value < -2147483648. { i32::MIN }
    else { value.trunc() as i32 }
}
