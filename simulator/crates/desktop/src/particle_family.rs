//! Portable semantic interpreter for verified SDK2 effect families. The
//! descriptors contain recovered parameters, never instructions/PE pointers.
//! All models share SDK quad projection and the owner's real task clock/RNG.
use cs2_script::Result;
use super::super::particle_vm::{ParticleCanvas, ParticleQuad, ParticleRandomSource,
    ParticleRenderSnapshot, ParticleVertex};
#[path = "particle_family_profiles.rs"]
pub(super) mod profiles;
#[path = "particle_family_path.rs"]
mod paths;
#[path = "particle_family_sprite.rs"]
mod sprites;
#[path = "particle_family_state.rs"]
pub(crate) mod checkpoint;
use profiles::{FieldProfile, Model};

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Particle {
    clock: u32,
    phase_origin: u32,
    phase: u32,
    position: [f32; 3],
    velocity: [f32; 2],
    displacement: [f32; 2],
    rotation: f32,
    angular_step: f32,
    scale: [f32; 2],
    uv_origin: [f32; 2],
    uv_span: f32,
    alpha: u8,
    durations: [i32; 3],
    total_duration: i32,
}
impl Default for Particle {
    fn default() -> Self {
        Self { clock: 0, phase_origin: 0, phase: 0, position: [0.;3],
            velocity: [0.;2], displacement: [0.;2], rotation: 0., angular_step: 0.,
            scale: [1.;2], uv_origin: [0.;2], uv_span: 1., alpha: 255,
            durations: [0;3], total_duration: 0 }
    }
}

#[derive(Debug)]
pub(super) struct Family {
    profile: profiles::Profile,
    path: Option<paths::PathFamily>,
    sprite: Option<sprites::SpriteFamily>,
    particles: Vec<Particle>,
    image: [f32; 2],
    viewport_half: [f32; 2],
    render_mode: u32,
    counter: i32,
    active_count: usize,
    center: [f32; 2],
    field_centers: [[f32;2];3],
    field_scale_parameter: f32,
    gravity: f32,
    horizontal: f32,
    peak_alpha: i32,
    initial_scale: f32,
    question_durations: [i32; 5],
    question_rotation: f32,
}

impl Family {
    pub(super) fn uses_family(pe: &cs2_runtime::particle_plugin_pe::ParticlePluginPe<'_>, sha256: &str) -> bool {
        Self::identify(pe, sha256).is_some()
    }

    pub(super) fn load(pe: &cs2_runtime::particle_plugin_pe::ParticlePluginPe<'_>, sha256: &str,
        count: u32, image: [f32; 2]) -> Result<Option<Self>> {
        let Some(profile) = Self::identify(pe, sha256) else { return Ok(None); };
        Self::new(profile, count, image).map(Some)
    }

    fn identify(pe: &cs2_runtime::particle_plugin_pe::ParticlePluginPe<'_>, sha256: &str) -> Option<profiles::Profile> {
        use super::plugin_layout::{self, DarkField};

        if plugin_layout::is_sakura(pe) {
            Some(profiles::SAKURA_PROFILE)
        } else if let Some(oscillation) = plugin_layout::question_oscillation(pe) {
            Some(if oscillation { profiles::QUESTION_PROFILE } else { profiles::QUESTION_A_PROFILE })
        } else if let Some(field) = plugin_layout::dark_field(pe) {
            Some(match field {
                DarkField::SingleCenter => profiles::DARK_PROFILE,
                DarkField::ThreeCenters => profiles::DARK_TRI_PROFILE,
                DarkField::Rising => profiles::DARK_RISING_PROFILE,
                DarkField::Ground => profiles::DARK_GROUND_PROFILE,
            })
        } else if let Some(effect) = plugin_layout::path_effect(pe) {
            Some(*profiles::path_effect(effect))
        } else if let Some(effect) = plugin_layout::sprite_effect(pe) {
            Some(profiles::Profile { sha256: "", model: Model::Sprite(effect) })
        } else {
            profiles::identify(sha256).copied()
        }
    }

    fn new(profile: profiles::Profile, count: u32, image: [f32;2]) -> Result<Self> {
        if matches!(profile.model,Model::Path(profiles::PathProfile {kind:profiles::PathKind::Star|profiles::PathKind::StarLegacy,..}))
            && count/7>1 && count/(count/7)>7 {
            return Err(super::super::native_state_error("star emitter count exceeds the recovered seven-color table"));
        }
        if count<18 && matches!(profile.model, Model::Path(profiles::PathProfile {kind:profiles::PathKind::Cube,..})) {
            return Err(super::super::native_state_error("o_150101 requires its eighteen six-face solid records"));
        }
        if count>4 && matches!(profile.model, Model::Path(profiles::PathProfile {kind:profiles::PathKind::Tiles,..})) {
            return Err(super::super::native_state_error("s_080103 grid exceeds its four-entry atlas and geometry table"));
        }
        if count==0 && matches!(profile.model, Model::Path(profiles::PathProfile {kind:profiles::PathKind::Windball,..})) {
            return Err(super::super::native_state_error("windball grid0 has no first active particle"));
        }
        if count == 0 && matches!(profile.model, Model::Field(_) | Model::Sprite(profiles::SpriteProfile::Down | profiles::SpriteProfile::Silent)) {
            return Err(super::super::native_state_error("particle model count0 would dereference an absent first particle"));
        }
        if count > 4 && matches!(profile.model, Model::Sprite(profiles::SpriteProfile::Silent)) {
            return Err(super::super::native_state_error("silent plugin grid exceeds its recovered four-entry atlas table"));
        }
        let path = if let Model::Path(p) = profile.model {Some(paths::PathFamily::new(p,count,image))} else {None};
        let sprite = if let Model::Sprite(p) = profile.model {Some(sprites::SpriteFamily::new(p,count,image))} else {None};
        Ok(Self { profile, particles: if path.is_some() || sprite.is_some() {Vec::new()} else {vec![Particle::default(); count as usize]}, path,sprite,
            image, viewport_half: [0.;2], render_mode: 0, counter: 0, active_count: 0,
            center: [0.;2], field_centers: [[0.;2];3], field_scale_parameter: 0.,
            gravity: 0., horizontal: 0., peak_alpha: 0, initial_scale: 1.,
            question_durations: [0;5], question_rotation: 0. })
    }

    pub(super) fn reset_extent(&mut self, image: [f32;2]) {
        self.image = image;
        if let Some(path) = self.path.as_mut() {path.reset_extent(image);}
        if let Some(sprite) = self.sprite.as_mut() {sprite.reset_extent(image);}
    }

    pub(super) fn start(&mut self, p: &[u8;128], viewport: [u32;2], resources:&super::super::ResourceIndex, random: &mut impl ParticleRandomSource) -> Result<()> {
        if matches!(self.profile.model, Model::Path(profiles::PathProfile {kind:profiles::PathKind::Message,..}))
            && !(0..32).contains(&trunc(word(p,10))) {
            return Err(super::super::native_state_error("wc_message atlas index lies outside the recovered32-row table"));
        }
        if matches!(self.profile.model, Model::Path(profiles::PathProfile {kind:profiles::PathKind::HotaruStay,..}))
            && trunc(word(p,6))>12 {
            return Err(super::super::native_state_error("hotaru_stay knot list exceeds its 128-byte parameter block"));
        }
        if matches!(self.profile.model, Model::Path(profiles::PathProfile {kind:profiles::PathKind::HotaruMove,..}))
            && trunc(word(p,5))>13 {
            return Err(super::super::native_state_error("hotaru_move knot list exceeds its 128-byte parameter block"));
        }
        if matches!(self.profile.model, Model::Path(profiles::PathProfile {kind:profiles::PathKind::Hadou,..}))
            && trunc(word(p,5))==0 {
            return Err(super::super::native_state_error("hadou emission period is zero (PE1530 divides by this parameter)"));
        }
        if matches!(self.profile.model, Model::Path(profiles::PathProfile {kind:profiles::PathKind::Concent,..}))
            && (trunc(word(p,5))==0 || trunc(word(p,6))==0) {
            return Err(super::super::native_state_error("concent rectangle has a zero integer random divisor"));
        }
        if matches!(self.profile.model, Model::Path(profiles::PathProfile {kind:profiles::PathKind::ConcentOriginal,..}))
            && (trunc(word(p,4))==0 || trunc(word(p,5))==0) {
            return Err(super::super::native_state_error("concent_o rectangle has a zero integer random divisor"));
        }
        self.viewport_half = viewport.map(|n| n as f32 * 0.5);
        self.center = [word(p,0), word(p,1)];
        match self.profile.model {
            Model::Sprite(_) => {
                if let Some(sprite) = self.sprite.as_mut() {sprite.start(p,viewport,random)?;}
            }
            Model::Path(_) => {
                if let Some(path) = self.path.as_mut() {
                    if matches!(self.profile.model,Model::Path(profiles::PathProfile {kind:profiles::PathKind::TextWords,..})) {path.load_text_config(p,resources)?;}
                    path.start(p,viewport,random);
                }
            }
            Model::Field(profile) => {
                self.field_centers = std::array::from_fn(|i| {
                    let base = if profile.triple_centers {i*3} else {0};
                    [word(p,base),word(p,base+1)]
                });
                self.field_scale_parameter = word(p,profile.scale_parameter);
                self.gravity = word(p,profile.gravity_parameter) / 1000.;
                self.horizontal = profile.horizontal_parameter.map_or(0., |i| word(p,i) / 1000.);
                self.peak_alpha = trunc(word(p,profile.alpha_parameter));
                self.initial_scale = word(p,profile.scale_parameter) / 100.;
                // ucomiss/lahf/test44/jnp: only ordered equality skips the
                // sticky write. NaN follows the nonzero branch in the PE.
                if word(p,profile.blend_parameter) != 0. { self.render_mode = 1; }
                self.active_count = 1; self.counter = 0;
                for d in &mut self.particles {
                    //1120 clears private records on EVERY start. SDK transform
                    //storage remains separate; spawn does not set its position.
                    spawn_field(d,profile,self.initial_scale,self.horizontal,random);
                }
            }
            Model::Question { .. } => {
                let scale = word(p,3) / 100.;
                self.initial_scale = if 0.001 > scale { 0.001 } else { scale };
                self.question_durations = std::array::from_fn(|i| trunc(word(p,i+4)));
                self.question_rotation = if trunc(word(p,2)) == 0 { -0.5 } else { 0.5 };
                self.active_count = self.particles.len();
                for d in &mut self.particles {
                    *d = Particle::default(); d.position = [self.center[0],self.center[1],0.];
                    d.scale = [self.initial_scale;2]; d.rotation = self.question_rotation;
                    d.alpha = 0;
                }
            }
        }
        Ok(())
    }

    pub(super) fn update(&mut self, delta: f32, p: &[u8;128], source_position: [f32;3], random: &mut impl ParticleRandomSource) {
        let tick = trunc(delta) as u32;
        let offset = [source_position[0]-self.viewport_half[0],source_position[1]-self.viewport_half[1]];
        match self.profile.model {
            Model::Sprite(_) => {
                if let Some(sprite) = self.sprite.as_mut() {sprite.update(delta,source_position,random);}
            }
            Model::Path(_) => {
                if let Some(path) = self.path.as_mut() {path.update(delta,p,source_position,random);}
            }
            Model::Field(profile) => {
                if profile.triple_centers && self.field_scale_parameter != word(p,profile.scale_parameter) {
                    self.field_scale_parameter = word(p,profile.scale_parameter);
                    self.initial_scale = self.field_scale_parameter/100.;
                    self.field_centers = std::array::from_fn(|i| [word(p,i*3),word(p,i*3+1)]);
                }
                if self.counter > profile.emission_period {
                    self.active_count = (self.active_count+1).min(self.particles.len());
                    self.counter = 0;
                }
                for (index,d) in self.particles.iter_mut().take(self.active_count).enumerate() {
                    let center = self.field_centers[index%3];
                    if d.phase <= 2 {
                        let elapsed = d.clock.wrapping_sub(d.phase_origin) as i32;
                        d.velocity[1] = self.gravity + d.velocity[1];
                        if d.phase == 2 && profile.fadeout_damping_below.is_some_and(|limit| limit > d.velocity[1]) {
                            d.velocity[1] *= f32::from_bits(0x3f4c_cccd);
                        }
                        d.displacement[0] = d.velocity[0] + d.displacement[0];
                        d.displacement[1] += d.velocity[1];
                        d.position = [center[0]+d.displacement[0],center[1]+d.displacement[1],0.];
                        d.scale = [lerp(self.initial_scale,self.initial_scale*2.,d.total_duration as f32,d.clock as i32 as f32);2];
                        d.rotation = d.angular_step + d.rotation;
                        if d.phase == 0 { d.alpha = trunc(lerp(0.,self.peak_alpha as f32,d.durations[0] as f32,elapsed as f32)) as u8; }
                        if d.phase == 2 { d.alpha = trunc(lerp(self.peak_alpha as f32,0.,d.durations[2] as f32,elapsed as f32)) as u8; }
                        if elapsed > d.durations[d.phase as usize] {
                            d.phase += 1; d.phase_origin = d.clock;
                        }
                    } else {
                        if profile.reset_position_on_respawn { d.position[..2].copy_from_slice(&center); }
                        spawn_field(d,profile,self.initial_scale,self.horizontal,random);
                    }
                    d.position[0] += offset[0]; d.position[1] += offset[1];
                    d.clock = d.clock.wrapping_add(tick);
                }
                self.counter = self.counter.wrapping_add(1);
            }
            Model::Question { oscillates_after_hold } => {
                for d in &mut self.particles {
                    let elapsed = d.clock.wrapping_sub(d.phase_origin) as i32;
                    let t = elapsed as f32; let scale = self.initial_scale;
                    let next = match d.phase {
                        0 => {
                            d.scale = [lerp(scale,scale*f32::from_bits(0x3ecc_cccd),self.question_durations[0] as f32,t),
                                lerp(scale,scale*1.5,self.question_durations[0] as f32,t)];
                            d.alpha = trunc(lerp(0.,128.,self.question_durations[0] as f32,t)) as u8;
                            elapsed > self.question_durations[0]
                        }
                        1 => {
                            d.scale = [lerp((f64::from(scale)*0.4) as f32,scale*f32::from_bits(0x4019_999a),self.question_durations[1] as f32,t),
                                lerp(scale*1.5,scale*0.5,self.question_durations[1] as f32,t)];
                            d.alpha = trunc(lerp(128.,255.,self.question_durations[1] as f32,t)) as u8;
                            elapsed > self.question_durations[1]
                        }
                        2 => {
                            //17fa deliberately discards the vertical-motion
                            //interpolant. Preserve that observed no-op.
                            d.scale = [lerp(scale*f32::from_bits(0x4019_999a),scale,self.question_durations[2] as f32,t),
                                lerp(scale*0.5,scale,self.question_durations[2] as f32,t)];
                            elapsed > self.question_durations[2]
                        }
                        3 => oscillates_after_hold && elapsed > self.question_durations[3],
                        4 => {
                            let angle = (f32::from_bits(0x40c9_0fdb) / self.question_durations[4] as f32)*t;
                            // SDK cosine returns a float before x87 division50.
                            let cosine = f64::from(angle).cos() as f32;
                            d.rotation += (f64::from(cosine)/50.) as f32;
                            if elapsed > self.question_durations[4] {
                                d.rotation = self.question_rotation; d.phase = 3; d.phase_origin = d.clock;
                            }
                            false
                        }
                        _ => false,
                    };
                    if next { d.phase += 1; d.phase_origin = d.clock; }
                    d.position = [self.center[0]+offset[0],offset[1]+self.center[1],0.];
                    d.clock = d.clock.wrapping_add(tick);
                }
            }
        }
    }

    pub(super) fn finish_requested(&self)->bool {
        self.path.as_ref().is_some_and(|path|path.finish_requested())
    }

    pub(super) fn snapshot(&self, canvas: ParticleCanvas, active: bool, submitted: bool,
        elapsed: u32, previous_elapsed: u32, opacity: u32) -> ParticleRenderSnapshot {
        if let Some(path) = self.path.as_ref() {
            return path.snapshot(canvas,active,submitted,elapsed,previous_elapsed,opacity);
        }
        if let Some(sprite) = self.sprite.as_ref() {
            return sprite.snapshot(canvas,active,submitted,elapsed,previous_elapsed,opacity);
        }
        let [w,h] = self.image;
        ParticleRenderSnapshot { active, frame_complete: true, elapsed, previous_elapsed,
            current_quad: self.particles.len() as u32, render_mode: self.render_mode, opacity, canvas,
            quads: self.particles.iter().enumerate().map(|(index,d)| {
                let positions = if submitted && matches!(self.profile.model,Model::Question { .. }) {
                    //19c0: the third vertex's Y is half WIDTH, not half height.
                    //Only pivotY is changed; pivotX retains canonical zero.
                    [[-w*0.5,-h*0.5],[w*0.5,-h*0.5],[-w*0.5,w*0.5],[w*0.5,h*0.5]]
                } else { [[0.,0.],[w,0.],[0.,h],[w,h]] };
                let pivot = match self.profile.model { Model::Field(_) => [w*0.5,h*0.5],Model::Question { .. } => [0.,h*0.5],Model::Path(_) | Model::Sprite(_) => [0.;2] };
                let base_uv = [[0.,0.],[1.,0.],[0.,1.],[1.,1.]];
                let (s,c) = f64::from(d.rotation).sin_cos(); let (s,c) = (s as f32,c as f32);
                let vertices = std::array::from_fn(|i| {
                    let [mut x,mut y] = positions[i]; let mut uv = base_uv[i];
                    if submitted {
                        x = (x-pivot[0])*d.scale[0]; y = (y-pivot[1])*d.scale[1];
                        let [a,b] = [x,y]; x = a*c-b*s+d.position[0]; y = a*s+b*c+d.position[1];
                        uv = [uv[0]*d.uv_span+d.uv_origin[0],uv[1]*d.uv_span+d.uv_origin[1]];
                    }
                    ParticleVertex { position: [x,y,if submitted {d.position[2]} else {0.}], uv,
                        color: if submitted {0x00ff_ffff | (u32::from(d.alpha)<<24)} else {u32::MAX}, ..Default::default() }
                });
                ParticleQuad { index: index as u32, vertices, offset: [0.;4], depth_bias:0., flag: 1 }
            }).collect() }
    }
}

fn spawn_field(d: &mut Particle, profile: FieldProfile, scale: f32, horizontal: f32,
    random: &mut impl ParticleRandomSource) {
    // Integer and floating MT streams are distinct; call order is observable.
    d.uv_origin = [[0.,0.],[0.5,0.],[0.,0.5],[0.5,0.5]][(random.next_integer()&3) as usize];
    d.uv_span = 0.5; d.displacement = [0.;2]; d.scale = [scale;2];
    d.rotation = (sample(random)*f64::from(f32::from_bits(0x40c9_0fdb))) as f32;
    let spin = f32::from_bits(random.next_float_bits());
    d.angular_step = ((spin*2.)-1.)*f32::from_bits(0x3c23_d70a);
    let x_sign = (sample(random)*2.-1.) as f32;
    let x = sample(random)*f64::from(x_sign);
    d.velocity[0] = if profile.horizontal_parameter.is_some() { (x*f64::from(horizontal)) as f32 } else {x as f32};
    d.velocity[1] = if profile.horizontal_parameter.is_some() { -sample(random) as f32 } else {
        let y_sign = (sample(random)*2.-1.) as f32;
        (sample(random)*f64::from(y_sign)) as f32
    };
    let length_squared = d.velocity[0]*d.velocity[0]+d.velocity[1]*d.velocity[1];
    let length = f64::from(length_squared).sqrt() as f32;
    if length > 0. { for v in &mut d.velocity { *v /= length; } }
    let speed = (sample(random)*f64::from(profile.speed_random)+f64::from(profile.speed_base)) as f32;
    for v in &mut d.velocity { *v *= speed; }
    d.phase = 0; d.clock = 0; d.phase_origin = 0;
    let fadein = f32::from_bits(random.next_float_bits());
    d.durations[0] = trunc(((fadein*2.)-1.)*15.+30.);
    d.durations[1] = profile.hold;
    let fadeout = f32::from_bits(random.next_float_bits());
    d.durations[2] = trunc(((fadeout*2.)-1.)*profile.fadeout_random+profile.fadeout_base);
    d.total_duration = d.durations[0].wrapping_add(d.durations[1]).wrapping_add(d.durations[2]);
    d.alpha = 0;
}
fn word(p: &[u8;128], i: usize) -> f32 { f32::from_le_bytes(p[i*4..i*4+4].try_into().unwrap()) }
fn sample(random: &mut impl ParticleRandomSource) -> f64 { f64::from(f32::from_bits(random.next_float_bits())) }
fn trunc(v: f32) -> i32 { if !v.is_finite() || !(-2147483648.0..2147483648.0).contains(&v) {i32::MIN} else {v.trunc() as i32} }
fn lerp(from: f32, to: f32, duration: f32, now: f32) -> f32 {
    // Yuki SDK callback1b0410..1b047e: SSE fraction, difference, product,
    // then start; keep the float store boundaries, including signed clocks.
    if now <= 0. { from } else if now >= duration { to } else { (now/duration)*(to-from)+from }
}
