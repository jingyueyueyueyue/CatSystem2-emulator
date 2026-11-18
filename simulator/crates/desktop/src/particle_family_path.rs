//! Independent SDK2 path effects. Only exact identities in PROFILES enter
//! this model. PE data is never loaded as code. Shared path helpers recover
//! polyline-distance timing and the original cubic endpoint/tangent basis.
use super::profiles::{PathKind, PathProfile};
use cs2_runtime::graph_serde::{Deserialize, Serialize};
#[path = "particle_path_state.rs"]
pub(crate) mod checkpoint;
use std::sync::atomic::{AtomicU32,Ordering};
use super::super::super::particle_vm::{ParticleCanvas, ParticleQuad,
    ParticleRandomSource, ParticleRenderSnapshot, ParticleVertex};
#[path = "particle_family_falling.rs"]
mod falling;
#[path = "particle_family_volt.rs"]
mod volt;
#[path = "particle_family_fire.rs"]
mod fire;
#[path = "particle_family_wave.rs"]
mod wave;
#[path = "particle_family_flight.rs"]
mod flight;
#[path = "particle_family_wind.rs"]
mod wind;
#[path = "particle_family_splash.rs"]
mod splash;
#[path = "particle_family_tiles.rs"]
mod tiles;
#[path = "particle_family_orbit.rs"]
mod orbit;
#[path = "particle_family_explosion.rs"]
mod explosion;
#[path = "particle_family_firebase.rs"]
mod firebase;
#[path = "particle_family_spark.rs"]
mod spark;
#[path = "particle_family_rubble.rs"]
mod rubble;
#[path = "particle_family_round.rs"]
mod round;
#[path = "particle_family_erase.rs"]
mod erase;
#[path = "particle_family_star.rs"]
mod star;
#[path = "particle_family_prism.rs"]
mod prism;
#[path = "particle_family_seed.rs"]
mod seed;
#[path = "particle_family_radiate.rs"]
mod radiate;
#[path = "particle_family_floating.rs"]
mod floating;
#[path = "particle_family_thunder.rs"]
mod thunder;
#[path = "particle_family_fall_spark.rs"]
mod fall_spark;
#[path = "particle_family_arrow.rs"]
mod arrow;
#[path = "particle_family_protect.rs"]
mod protect;
#[path = "particle_family_fire_pattern.rs"]
mod fire_pattern;
#[path = "particle_family_matrix.rs"]
mod matrix;
#[path = "particle_family_pond.rs"]
mod pond;
#[path = "particle_family_vortex.rs"]
mod vortex;
#[path = "particle_family_baseline_spark.rs"]
mod baseline_spark;
#[path = "particle_family_torch.rs"]
mod torch;
#[path = "particle_family_shard.rs"]
mod shard;
#[path = "particle_family_rolling.rs"]
mod rolling;
#[path = "particle_family_smoke.rs"]
mod smoke;
#[path = "particle_family_ice.rs"]
mod ice;
#[path = "particle_family_circle.rs"]
mod circle;
#[path = "particle_family_glow.rs"]
mod glow;
#[path = "particle_family_background.rs"]
mod background;
#[path = "particle_family_messages.rs"]
mod messages;
#[path = "particle_family_atlas_emitters.rs"]
mod atlas_emitters;
#[path = "particle_family_damage.rs"]
mod damage;
#[path = "particle_family_wind_trail.rs"]
mod wind_trail;
#[path = "particle_family_ribbon.rs"]
mod ribbon;
#[path = "particle_family_light.rs"]
mod light;
#[path = "particle_family_text.rs"]
mod text;
#[path = "particle_family_arrow_stream.rs"]
mod arrow_stream;
#[path = "particle_family_cube.rs"]
mod cube;

// concent's150ac is initialized to1 in PE data and is not reset by start
// or finish. Instances of this model share that module emission state.
static CONCENT_ACTIVE: AtomicU32=AtomicU32::new(1);
// Original concent_o has a separate module counter at10d0c.
static CONCENT_ORIGINAL_ACTIVE: AtomicU32=AtomicU32::new(1);
static DASH_ACTIVE: AtomicU32=AtomicU32::new(1);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Trajectory {
    points: [[f32; 3]; 20],
    tangents: [[f32; 3]; 20],
    lengths: [f32; 20],
    count: usize,
    length: f32,
    duration: f32,
}

impl Trajectory {
    fn append(&mut self, point: [f32; 3]) {
        let n = self.count;
        if n != 0 { self.lengths[n] = length(sub(self.points[n-1],point)); }
        self.length += self.lengths[n];
        self.points[n] = point; self.count += 1;
    }

    // Helper18f0, including separate endpoint and centered interior tangents.
    // Duration is total task ticks, while segment selection uses chord length.
    fn finish(&mut self, duration: f32, tangent_size: f32) {
        if duration == 0. || self.count < 2 { return; }
        let n = self.count;
        self.tangents[0] = scale(unit(sub(self.points[1],self.points[0])),tangent_size);
        for i in 1..n-1 {
            let incoming = sub(self.points[i-1],self.points[i]);
            let outgoing = sub(self.points[i+1],self.points[i]);
            self.tangents[i] = scale(unit(sub(outgoing,incoming)),tangent_size);
        }
        self.tangents[n-1] = scale(unit(sub(self.points[n-1],self.points[n-2])),tangent_size);
        self.duration = duration;
    }

    fn at(&self, clock: u32) -> [f32; 3] {
        self.at_time(clock as i32 as f32)
    }

    // energy18c0 chooses mean chord length for the tangent magnitude;
    // steam19d0 instead receives the magnitude explicitly.
    fn finish_average(&mut self,duration: f32) {
        let tangent=if self.count>1 {self.length/(self.count-1) as f32} else {0.};
        self.finish(duration,tangent);
    }

    fn at_time(&self, now: f32) -> [f32; 3] {
        if self.count < 2 || now <= 0. { return self.points[0]; }
        let now = if now < self.duration { now } else { self.duration };
        let distance = (now / self.duration) * self.length;
        let mut origin = 0.;
        for i in 1..self.count {
            let end = self.lengths[i] + origin;
            if end > distance {
                let t = (distance-origin)/self.lengths[i];
                let t2 = t*t; let t3 = t2*t;
                return std::array::from_fn(|axis| {
                    let p = self.points[i-1][axis]; let q = self.points[i][axis];
                    let a = self.tangents[i-1][axis]; let b = self.tangents[i][axis];
                    //12c0 multiplies the four input rows by this stored basis.
                    let cubic = ((p*2. + q*(-2.)) + a) + b;
                    let square = ((p*(-3.) + q*3.) + a*(-2.)) + b*(-1.);
                    let linear = ((p*0. + q*0.) + a) + b*0.;
                    let constant = ((p + q*0.) + a*0.) + b*0.;
                    ((t3*cubic + t2*square) + linear*t) + constant
                });
            }
            origin = end;
        }
        self.points[self.count-1]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Drop {
    clock: u32,
    phase_origin: u32,
    phase: u32,
    atlas_frame: u32,
    position: [f32; 3],
    captured: [f32; 3],
    scale: f32,
    stretch: [f32; 2],
    rotation: f32,
    rotation_xy: [f32;2],
    angular_step: f32,
    pivot: [f32; 2],
    uv: [[f32;2];4],
    alpha: u8,
    depth_bias: f32,
    saved_alpha: f32,
    vertex_rgb: Option<[u32;4]>,
    vertex_alpha: Option<[u8;4]>,
    flag: u32,
    motion: [f32;3],
    motion_origin: [f32;3],
    motion_limit: f32,
    centered_geometry: bool,
    geometry: Option<[[f32;3];4]>,
    durations: [i32; 3],
    path: Trajectory,
}
impl Default for Drop {
    fn default() -> Self {
        Self { clock: 0, phase_origin: 0, phase: 0, atlas_frame:0, position: [0.;3], captured: [0.;3],
            scale: 1., stretch: [1.;2], rotation: 0., rotation_xy:[0.;2], angular_step: 0., pivot: [0.;2],uv:[[0.,0.],[1.,0.],[0.,1.],[1.,1.]],alpha: 255, saved_alpha: 255.,
            vertex_rgb: None, vertex_alpha:None,flag:1,depth_bias:0., motion:[0.;3],motion_origin:[0.;3],motion_limit:0.,
            centered_geometry: false, geometry:None, durations: [0;3], path: Trajectory::default() }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct ShyConfig {
    distance: f32,
    distance_range: f32,
    angle: f32,
    angle_range: f32,
    size_range: f32,
    alpha: f32,
    alpha_range: f32,
    normal: [f32;3],
    baseline: Trajectory,
    divisor: u32,
    step: usize,
    // Start stores p17, but neither2860 nor2e10 reads it.
    _behavior: i32,
    average_tangents: bool,
    align_motion: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct HotaruConfig {
    size_range: f32,
    radius: f32,
    horizontal: [f32;2],
    vertical: f32,
    direction: i32,
    divisor: u32,
    step: u32,
}

#[derive(Clone,Debug,Default,Serialize,Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct DashConfig {speed: f32,angle: f32,distance: f32,spread: f32,alpha: u8}
impl Default for HotaruConfig {
    fn default() -> Self {Self {size_range:0.,radius:0.,horizontal:[0.;2],vertical:0.,direction:0,divisor:1,step:0}}
}

#[derive(Debug)]
pub(super) struct PathFamily {
    profile: PathProfile,
    drops: Vec<Drop>,
    image: [f32; 2],
    viewport_half: [f32; 2],
    center: [f32; 3],
    scale: f32,
    factor: f32,
    durations: [i32; 3],
    direction: i32,
    main_count: usize,
    active_count: usize,
    shy: ShyConfig,
    radius: f32,
    angle: f32,
    concent_duration: f32,
    concent_spread: [f32;2],
    child_count: usize,
    hotaru: HotaruConfig,
    stream_render_mode: u32,
    dash: DashConfig,
    falling: falling::FallingConfig,
    volt: volt::VoltConfig,
    fire: fire::FireConfig,
    wave: wave::WaveConfig,
    flight: flight::FlightConfig,
    wind: wind::WindConfig,
    splash: splash::SplashConfig,
    tiles: tiles::TilesConfig,
    orbit: orbit::OrbitConfig,
    explosion: explosion::ExplosionConfig,
    firebase: firebase::FirebaseConfig,
    firebase_stream: firebase::FirebaseStreamConfig,
    rubble: rubble::RubbleConfig,
    round: round::RoundConfig,
    erase: erase::EraseConfig,
    star: star::StarConfig,
    prism_trail: prism::PrismTrailConfig,
    prism: prism::PrismConfig,
    prism_orbit: prism::PrismOrbitConfig,
    seed: seed::SeedConfig,
    radiate: radiate::RadiateConfig,
    floating: floating::FloatingConfig,
    thunder: thunder::ThunderConfig,
    fall_spark: fall_spark::FallSparkConfig,
    arrow: arrow::ArrowConfig,
    protect: protect::ProtectConfig,
    fire_pattern: fire_pattern::FirePatternConfig,
    pond: pond::PondConfig,
    vortex: vortex::VortexConfig,
    baseline_spark: baseline_spark::BaselineSparkConfig,
    torch: torch::TorchConfig,
    shard: shard::ShardConfig,
    smoke: smoke::SmokeConfig,
    glow: glow::GlowConfig,
    background: background::BackgroundConfig,
    messages: messages::MessageConfig,
    atlas_emitter: atlas_emitters::AtlasEmitterConfig,
    damage: damage::DamageConfig,
    wind_trail: wind_trail::WindTrailConfig,
    ribbon:ribbon::RibbonConfig,
    light:light::LightConfig,
    cube:cube::CubeConfig,
    stream_arrow:arrow_stream::StreamArrowConfig,
    text:text::TextConfig,
}
impl PathFamily {
    pub(super) fn new(profile: PathProfile, count: u32, image: [f32;2]) -> Self {
        Self { profile, text:text::TextConfig::default(), stream_arrow:arrow_stream::StreamArrowConfig::default(), cube:cube::CubeConfig::default(), light:light::LightConfig::default(), ribbon:ribbon::RibbonConfig::default(), prism_trail:prism::PrismTrailConfig::default(), wind_trail:wind_trail::WindTrailConfig::default(), damage:damage::DamageConfig::default(), atlas_emitter:atlas_emitters::AtlasEmitterConfig::default(),torch:torch::TorchConfig::default(),baseline_spark:baseline_spark::BaselineSparkConfig::default(),vortex:vortex::VortexConfig::default(),pond:pond::PondConfig::default(),fire_pattern:fire_pattern::FirePatternConfig::default(),protect:protect::ProtectConfig::default(),arrow:arrow::ArrowConfig::default(),fall_spark:fall_spark::FallSparkConfig::default(),thunder:thunder::ThunderConfig::default(),seed:seed::SeedConfig::default(),radiate:radiate::RadiateConfig::default(),floating:floating::FloatingConfig::default(), drops: vec![Drop::default();count as usize], image,
            shard:shard::ShardConfig::default(),smoke:smoke::SmokeConfig::default(),glow:glow::GlowConfig::default(),background:background::BackgroundConfig::default(),messages:messages::MessageConfig::default(),viewport_half: [0.;2], center: [0.;3], scale: 1., factor: 0.5,
            durations: [0;3], direction: 0, main_count: 0, active_count: 0,
            shy: ShyConfig::default(), radius: 0., angle: 0.,concent_duration:0.,concent_spread:[0.;2],child_count:0,
            hotaru:HotaruConfig::default(),stream_render_mode:0,dash:DashConfig::default(),falling:falling::FallingConfig::default(),volt:volt::VoltConfig::default(),fire:fire::FireConfig::default(),wave:wave::WaveConfig::default(),flight:flight::FlightConfig::default(),wind:wind::WindConfig::default(),splash:splash::SplashConfig::default(),tiles:tiles::TilesConfig::default(),orbit:orbit::OrbitConfig::default(),explosion:explosion::ExplosionConfig::default(),firebase:firebase::FirebaseConfig::default(),firebase_stream:firebase::FirebaseStreamConfig::default(),rubble:rubble::RubbleConfig::default(),round:round::RoundConfig::default(),erase:erase::EraseConfig::default(),star:star::StarConfig::default(),prism:prism::PrismConfig::default(),prism_orbit:prism::PrismOrbitConfig::default() }
    }
    pub(super) fn reset_extent(&mut self, image: [f32;2]) { self.image = image; }

    pub(super) fn start(&mut self, p: &[u8;128], viewport: [u32;2], random: &mut impl ParticleRandomSource) {
        let profile = self.profile;
        if profile.kind==PathKind::Dark {self.start_dark(p,random);return;}
        if profile.kind==PathKind::Happy {self.start_happy(p,random);return;}
        if profile.kind==PathKind::Photon {self.start_photon(p,random);return;}
        if matches!(profile.kind,PathKind::CircleLight|PathKind::CircleLightTuned) {self.start_circle_light(p,random);return;}
        if matches!(profile.kind,PathKind::Ice|PathKind::Ice2) {self.start_ice(p,random);return;}
        if profile.kind==PathKind::Smoke {self.start_smoke(p,random);return;}
        if profile.kind==PathKind::Rolling {self.start_rolling(p,random);return;}
        if profile.kind==PathKind::CreateSquare {self.start_square(p);return;}
        if profile.kind==PathKind::EarlySquare {self.start_early_square(p);return;}
        if profile.kind==PathKind::SwordSpark {self.start_shard(p,random);return;}
        self.viewport_half = viewport.map(|v| v as f32*0.5);
        if profile.kind==PathKind::TextWords {self.start_text(p,random);return;}
        if profile.kind==PathKind::StreamArrow {self.start_stream_arrow(p,random);return;}
        if profile.kind==PathKind::Cube {self.start_cube(p);return;}
        if profile.kind==PathKind::Light {self.start_light(p,viewport,random);return;}
        if profile.kind==PathKind::Ribbon {self.start_ribbon(p,random);return;}
        if profile.kind==PathKind::WindTrail {self.start_wind_trail(p,random);return;}
        if profile.kind==PathKind::Damage {self.start_damage(p,random);return;}
        if profile.kind==PathKind::RadialFlare {self.start_radial_flare(p,random);return;}
        if profile.kind==PathKind::WcFall {self.start_wc_fall(p,random);return;}
        if profile.kind==PathKind::Heart {self.start_heart(p);return;}
        if profile.kind==PathKind::Bubble {self.start_bubble(p,random);return;}
        if profile.kind==PathKind::Number {self.start_number(p,random);return;}
        if profile.kind==PathKind::Invisi {self.start_invisi(p,random);return;}
        if matches!(profile.kind,PathKind::Message|PathKind::MessageRandom) {self.start_message(p,viewport,random);return;}
        if profile.kind==PathKind::RotatingBackground {self.start_rotbg(p);return;}
        if profile.kind==PathKind::BlurStrips {self.start_blur(p,random);return;}
        if profile.kind==PathKind::Torch {self.start_torch(p,random);return;}
        if profile.kind==PathKind::BaselineSpark {self.start_baseline_spark(p,random);return;}
        if profile.kind==PathKind::Vortex {self.start_vortex(p);return;}
        if profile.kind==PathKind::Pond {self.start_pond(p,random);return;}
        if profile.kind==PathKind::FirePattern {self.start_fire_pattern(p,random);return;}
        if matches!(profile.kind,PathKind::Protect|PathKind::Protect2) {self.start_protect(p,random);return;}
        if profile.kind==PathKind::Arrow {self.start_arrow(p,random);return;}
        if profile.kind==PathKind::Thunder {self.start_thunder(p,random);return;}
        if profile.kind==PathKind::FallSpark {self.start_fall_spark(p,random);return;}
        if matches!(profile.kind,PathKind::Dust|PathKind::Twinkle|PathKind::FloatingSpark|PathKind::ASpark) {self.start_floating(p,random);return;}
        if profile.kind==PathKind::Radiate {self.start_radiate(p,random);return;}
        if matches!(profile.kind,PathKind::Seed|PathKind::SeedRot) {self.start_seed(p,random);return;}
        if profile.kind==PathKind::PrismTrail {self.start_prism_trail(p,random);return;}
        if profile.kind==PathKind::PrismOrbit {self.start_prism_orbit(p,random);return;}
        if profile.kind==PathKind::PrismPath {self.start_prism(p,random);return;}
        if matches!(profile.kind,PathKind::Star|PathKind::StarLegacy) {self.start_star(p,random);return;}
        if matches!(profile.kind,PathKind::Erase|PathKind::EraseAtlas) {self.start_erase(p,random);return;}
        if profile.kind==PathKind::Firebase {self.start_firebase_stream(p,random);return;}
        if matches!(profile.kind,PathKind::RoundY|PathKind::RoundZ) {self.start_round(p,random);return;}
        if profile.kind==PathKind::Rubble {self.start_rubble(p,random);return;}
        if matches!(profile.kind,PathKind::Spark|PathKind::SparkEx) {self.start_spark(p,random);return;}
        if matches!(profile.kind,PathKind::Firebase2|PathKind::Firebase3|PathKind::Firebase4) {self.start_firebase(p);return;}
        if profile.kind==PathKind::Exprode {self.start_explosion(p,random);return;}
        if profile.kind==PathKind::Mirror {self.start_orbit(p,random);return;}
        if profile.kind==PathKind::Tiles {self.start_tiles(p);return;}
        if profile.kind==PathKind::Splash {self.start_splash(p,random);return;}
        if profile.kind==PathKind::Windball {self.start_wind(p,random);return;}
        if matches!(profile.kind,PathKind::HotaruMove|PathKind::HotaruStay) {self.start_flight(p);return;}
        if profile.kind==PathKind::Hadou {self.start_wave(p);return;}
        if profile.kind==PathKind::Slash {self.start_slash_ribbon(p);return;}
        if matches!(profile.kind,PathKind::Fireball|PathKind::Fireball2) {self.start_fire(p,random);return;}
        if profile.kind==PathKind::Volt4 {self.start_volt_ribbon(p);return;}
        if matches!(profile.kind,PathKind::Volt|PathKind::Volt2|PathKind::Volt3) {self.start_volt(p,random);return;}
        if profile.kind==PathKind::Sakura {self.start_sakura(p,random);return;}
        if profile.kind==PathKind::Paper {self.start_paper(p,random);return;}
        if matches!(profile.kind,PathKind::Rising3|PathKind::Rising4) {self.start_rising(p,random);return;}
        if profile.kind==PathKind::Dash {self.start_dash(p,random);return;}
        if profile.kind==PathKind::Energy {self.start_energy(p,random);return;}
        if profile.kind==PathKind::Hotaru {self.start_hotaru(p,random);return;}
        if matches!(profile.kind,PathKind::Concent|PathKind::ConcentOriginal) {self.start_concent(p,random);return;}
        if matches!(profile.kind,PathKind::Shy|PathKind::Steam|PathKind::SteamAligned) {self.start_shy(p,random);return;}
        if profile.kind==PathKind::Kirakira {self.start_kirakira(p,random);return;}
        if profile.kind==PathKind::Warukira {self.start_warukira(p,random);return;}
        if profile.kind==PathKind::Oko {self.start_oko(p,random);return;}
        self.center = [word(p,profile.center_parameter),word(p,profile.center_parameter+1),0.];
        let scale_parameter = word(p,profile.scale_parameter)/100.;
        self.scale = if 0.001 > scale_parameter { 0.001 } else { scale_parameter };
        self.factor = if self.image[1] > 256. {1.} else {0.5};
        self.direction = profile.direction_parameter.map_or(0,|i|trunc(word(p,i)));
        self.durations = std::array::from_fn(|i| {
            if profile.kind != PathKind::Note && i == 2 {0}
            else {trunc(word(p,profile.duration_parameter+i))}
        });
        //1120 partitions WITHIN the original grid. Unused remainder quads
        //retain SDK storage; their transforms are not invented or hidden.
        let total = self.drops.len();
        let divisor = profile.partition_parameter.map_or(total as u32,|i|trunc(word(p,i)) as u32);
        self.main_count = if divisor == 0 {total} else {total/(divisor as usize)};
        self.active_count = if profile.kind == PathKind::Note {1.min(self.main_count)} else {self.main_count};
        for d in &mut self.drops {
            d.clock = 0; d.phase_origin = 0; d.phase = 0;
            d.path = Trajectory::default(); d.captured = [0.;3];
        }
        for d in self.drops.iter_mut().take(self.main_count) {
            if profile.kind == PathKind::Note {
                spawn_note(d,self.center,self.scale,self.factor,self.direction,self.durations,random);
            } else if profile.kind == PathKind::Trail {
                d.pivot = self.image.map(|v|v*0.5); d.position = self.center;
                d.scale = self.scale; d.alpha = 0;
                d.path.append(self.center);
                let y = (self.scale*150.)*self.factor + self.center[1];
                d.path.append([self.center[0],y,0.]);
                d.path.finish(self.durations[0].wrapping_add(self.durations[1]) as f32,10.);
            } else {
                d.pivot=self.image.map(|v|v*0.5);d.position=self.center;d.scale=self.scale;d.alpha=0;
                d.uv=sigh_uv(0,self.direction!=0);d.path.append(self.center);
                let angle=if self.direction==0 {-2.2} else {2.2};
                let step=sigh_direction(angle);
                let target=add(self.center,scale(step,self.scale*100.));
                d.path.append(target);
                let step=sigh_direction(angle*f32::from_bits(0x3f59_999a));
                let step=scale(step,self.scale*100.).map(|n|n/f32::from_bits(0x3f7d_70a4));
                d.path.append(add(target,step));
                d.path.finish(self.durations[0].wrapping_add(self.durations[1]) as f32,25.);
            }
        }
        if profile.kind == PathKind::Trail && self.main_count != 0 {
            let main = self.drops[0].clone();
            for d in self.drops.iter_mut().skip(1) {
                d.pivot = main.pivot; d.position = main.position; d.scale = main.scale; d.alpha = 0;
            }
        }
        if profile.kind==PathKind::Sigh && self.main_count!=0 {
            let main=self.drops[0].clone();
            for d in self.drops.iter_mut().skip(1) {
                d.pivot=main.pivot;d.position=main.position;d.scale=main.scale;d.alpha=0;
                d.uv=sigh_uv(1,self.direction!=0);
            }
        }
    }

    pub(super) fn update(&mut self, delta: f32, p: &[u8;128], source: [f32;3], random: &mut impl ParticleRandomSource) {
        let offset = [source[0]-self.viewport_half[0],source[1]-self.viewport_half[1]];
        let tick = trunc(delta) as u32;
        if self.profile.kind==PathKind::TextWords {self.update_text(tick,offset);return;}
        if self.profile.kind==PathKind::StreamArrow {self.update_stream_arrow(tick,random);return;}
        if self.profile.kind==PathKind::Cube {self.update_cube(tick);return;}
        if self.profile.kind==PathKind::Light {self.update_light(tick,p,offset,random);return;}
        if self.profile.kind==PathKind::Ribbon {self.update_ribbon(tick,random);return;}
        if self.profile.kind==PathKind::WindTrail {self.update_wind_trail(tick,random);return;}
        if self.profile.kind==PathKind::Damage {self.update_damage(tick,p,offset,random);return;}
        if self.profile.kind==PathKind::RadialFlare {self.update_radial_flare(tick,random);return;}
        if self.profile.kind==PathKind::WcFall {self.update_wc_fall(tick,offset,random);return;}
        if self.profile.kind==PathKind::Heart {self.update_heart(tick,offset);return;}
        if self.profile.kind==PathKind::Bubble {self.update_bubble(delta,tick,offset,random);return;}
        if self.profile.kind==PathKind::Number {self.update_number(tick,offset,random);return;}
        if self.profile.kind==PathKind::Invisi {self.update_invisi(tick,offset,random);return;}
        if self.profile.kind==PathKind::Dark {self.update_dark(tick,p,random);return;}
        if matches!(self.profile.kind,PathKind::Message|PathKind::MessageRandom) {self.update_message(tick,p,offset,random);return;}
        if self.profile.kind==PathKind::RotatingBackground {self.update_rotbg(tick,p);return;}
        if self.profile.kind==PathKind::BlurStrips {self.update_blur(tick,p,offset,random);return;}
        if self.profile.kind==PathKind::Happy {self.update_happy(tick,p,random);return;}
        if self.profile.kind==PathKind::Photon {self.update_photon(tick,random);return;}
        if matches!(self.profile.kind,PathKind::CircleLight|PathKind::CircleLightTuned) {self.update_circle_light(tick,random);return;}
        if matches!(self.profile.kind,PathKind::Ice|PathKind::Ice2) {self.update_ice(tick,p,random);return;}
        if self.profile.kind==PathKind::Smoke {self.update_smoke(tick,p,random);return;}
        if self.profile.kind==PathKind::Rolling {self.update_rolling(tick,p,random);return;}
        if self.profile.kind==PathKind::CreateSquare {self.update_square(tick,p);return;}
        if self.profile.kind==PathKind::EarlySquare {self.update_early_square(tick);return;}
        if self.profile.kind==PathKind::SwordSpark {self.update_shard(tick,p);return;}
        if self.profile.kind==PathKind::Torch {self.update_torch(tick,offset,random);return;}
        if self.profile.kind==PathKind::BaselineSpark {self.update_baseline_spark(tick,random);return;}
        if self.profile.kind==PathKind::Vortex {self.update_vortex(tick);return;}
        if self.profile.kind==PathKind::Pond {self.update_pond(tick,offset,random);return;}
        if self.profile.kind==PathKind::FirePattern {self.update_fire_pattern(tick,offset,random);return;}
        if matches!(self.profile.kind,PathKind::Protect|PathKind::Protect2) {self.update_protect(tick);return;}
        if self.profile.kind==PathKind::Thunder {self.update_thunder(tick,offset);return;}
        if self.profile.kind==PathKind::Arrow {self.update_arrow(tick,random);return;}
        if self.profile.kind==PathKind::FallSpark {self.update_fall_spark(tick,random);return;}
        if matches!(self.profile.kind,PathKind::Dust|PathKind::Twinkle|PathKind::FloatingSpark|PathKind::ASpark) {self.update_floating(tick,random);return;}
        if self.profile.kind==PathKind::Radiate {self.update_radiate(tick,random);return;}
        if matches!(self.profile.kind,PathKind::Seed|PathKind::SeedRot) {self.update_seed(tick,p,random);return;}
        if self.profile.kind==PathKind::PrismTrail {self.update_prism_trail(tick,p,random);return;}
        if self.profile.kind==PathKind::PrismOrbit {self.update_prism_orbit(tick,offset,random);return;}
        if self.profile.kind==PathKind::PrismPath {self.update_prism(tick,random);return;}
        if matches!(self.profile.kind,PathKind::Star|PathKind::StarLegacy) {self.update_star(tick,random);return;}
        if matches!(self.profile.kind,PathKind::Erase|PathKind::EraseAtlas) {self.update_erase(tick);return;}
        if self.profile.kind==PathKind::Firebase {self.update_firebase_stream(tick,random);return;}
        if matches!(self.profile.kind,PathKind::RoundY|PathKind::RoundZ) {self.update_round(tick,random);return;}
        if self.profile.kind==PathKind::Rubble {self.update_rubble(tick);return;}
        if matches!(self.profile.kind,PathKind::Spark|PathKind::SparkEx) {self.update_spark(tick,p,random);return;}
        if matches!(self.profile.kind,PathKind::Firebase2|PathKind::Firebase3|PathKind::Firebase4) {self.update_firebase(tick,offset);return;}
        if self.profile.kind==PathKind::Exprode {self.update_explosion(tick,random);return;}
        if self.profile.kind==PathKind::Mirror {self.update_orbit(tick,offset,random);return;}
        if self.profile.kind==PathKind::Tiles {self.update_tiles(tick);return;}
        if self.profile.kind==PathKind::Splash {self.update_splash(tick,random);return;}
        if self.profile.kind==PathKind::Windball {self.update_wind(tick,offset,random);return;}
        if matches!(self.profile.kind,PathKind::HotaruMove|PathKind::HotaruStay) {self.update_flight(tick,p,random);return;}
        if self.profile.kind==PathKind::Hadou {self.update_wave(tick,offset);return;}
        if matches!(self.profile.kind,PathKind::Fireball|PathKind::Fireball2) {self.update_fire(tick,offset,random);return;}
        if matches!(self.profile.kind,PathKind::Volt4|PathKind::Slash) {self.update_volt_ribbon(tick);return;}
        if matches!(self.profile.kind,PathKind::Volt|PathKind::Volt2|PathKind::Volt3) {self.update_volt(tick,offset,random);return;}
        if self.profile.kind==PathKind::Sakura {self.update_sakura(tick,random);return;}
        if self.profile.kind==PathKind::Paper {self.update_paper(tick,random);return;}
        if matches!(self.profile.kind,PathKind::Rising3|PathKind::Rising4) {self.update_rising(tick,p,random);return;}
        if self.profile.kind==PathKind::Dash {self.update_dash(tick,random);return;}
        if self.profile.kind==PathKind::Energy {self.update_energy(tick,random);return;}
        if self.profile.kind==PathKind::Hotaru {self.update_hotaru(tick,random);return;}
        if matches!(self.profile.kind,PathKind::Concent|PathKind::ConcentOriginal) {self.update_concent(tick,random);return;}
        if matches!(self.profile.kind,PathKind::Shy|PathKind::Steam|PathKind::SteamAligned) {self.update_shy(tick,offset,random);return;}
        if self.profile.kind==PathKind::Kirakira {self.update_kirakira(tick,offset,random);return;}
        if self.profile.kind==PathKind::Warukira {self.update_warukira(tick,offset,random);return;}
        if self.profile.kind==PathKind::Sigh {
            self.update_sigh(tick,offset);return;
        }
        if let Some(divisor) = self.profile.emission_divisor {
            if random.next_integer()%divisor == 0 {self.active_count += 1;}
            self.active_count = self.active_count.min(self.main_count);
        }
        for (index,d) in self.drops.iter_mut().take(self.active_count).enumerate() {
            let elapsed = d.clock.wrapping_sub(d.phase_origin) as i32;
            let streamed=matches!(self.profile.kind,PathKind::Note|PathKind::Oko);
            if streamed && d.phase > 2 {
                if self.profile.kind==PathKind::Oko {spawn_oko(d,index,self.center,self.scale,self.factor,self.durations,random);}
                else {spawn_note(d,self.center,self.scale,self.factor,self.direction,self.durations,random);}
            } else {
                let point = d.path.at(d.clock);
                d.position[..2].copy_from_slice(&point[..2]);
                if streamed {
                    if d.phase == 0 {d.alpha = trunc(super::lerp(0.,255.,d.durations[0] as f32,elapsed as f32)) as u8;}
                    if d.phase == 2 {d.alpha = trunc(super::lerp(255.,0.,d.durations[2] as f32,elapsed as f32)) as u8;}
                    if elapsed > d.durations[d.phase as usize] {
                        d.phase = if d.phase == 2 {10} else {d.phase+1}; d.phase_origin = d.clock;
                    }
                } else if d.phase <= 1 {
                    if d.phase == 0 {d.alpha = trunc(super::lerp(0.,255.,self.durations[0] as f32,elapsed as f32)) as u8;}
                    if elapsed > self.durations[d.phase as usize] {
                        d.phase = if d.phase == 1 {10} else {1}; d.phase_origin = d.clock;
                    }
                }
            }
            // Trail capture precedes the parent's source translation.
            if streamed {
                d.position[0] += offset[0]; d.position[1] += offset[1];
                d.clock = d.clock.wrapping_add(tick);
            }
        }
        if self.profile.kind == PathKind::Trail && self.main_count != 0 {
            let (parents,children) = self.drops.split_at_mut(self.main_count);
            let main = &mut parents[0];
            if main.phase <= 1 {
                if let Some(d) = children.iter_mut().find(|d|d.phase == 0) {
                    d.captured = main.position; d.position = main.position;
                    d.position[2] -= 1.; d.phase = 1;
                }
            }
            for d in children.iter_mut().filter(|d|d.phase != 0) {
                d.position[..2].copy_from_slice(&d.captured[..2]);
                d.alpha = trunc(super::lerp(30.,0.,22.,d.clock as i32 as f32)) as u8;
                d.clock = d.clock.wrapping_add(tick);
                if (d.clock as i32) > 25 {d.alpha = 0; d.clock = 0; d.phase = 0;}
                d.position[0] += offset[0]; d.position[1] += offset[1];
            }
            main.position[0] += offset[0]; main.position[1] += offset[1];
            main.clock = main.clock.wrapping_add(tick);
        }
    }

    fn start_rising(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        // 1b30 / 1120 receives count as both grid count and divisor.
        // Private storage is cleared on every start; SDK rotation survives it.
        self.main_count=usize::from(!self.drops.is_empty());
        self.child_count=self.drops.len().saturating_sub(self.main_count);
        let Some((main,trail))=self.drops.split_first_mut() else {return;};
        main.clock=0;main.phase_origin=0;main.phase=0;main.path=Trajectory::default();
        main.pivot=self.image.map(|n|n*0.5);
        main.uv=[[0.,0.5],[0.5,0.5],[0.,1.],[0.5,1.]];
        main.scale=word(p,3)/100.;main.stretch=[1.;2];main.alpha=255;
        main.position=std::array::from_fn(|i|word(p,i));main.captured=main.position;
        main.angular_step=rising_spin(random);
        for child in trail {
            child.clock=0;child.phase_origin=0;child.phase=0;child.path=Trajectory::default();
            child.angular_step=0.;child.position=main.position;
            child.pivot=main.pivot;child.uv=main.uv;child.alpha=0;
        }
    }

    fn update_rising(&mut self,tick: u32,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        let moving=self.profile.kind==PathKind::Rising3;
        let Some((main,trail))=self.drops.split_first_mut() else {return;};
        if main.phase==0 {
            main.rotation+=main.angular_step;
            // rising4's public phase0 deliberately has no transition. Do not
            // invent one from the otherwise-present phase1 body.
            if moving && main.clock as i32 as f32>word(p,4) {
                main.phase=1;main.phase_origin=main.clock;
            }
        } else if main.phase==1 {
            let start=main.phase_origin as i32 as f32;
            let duration=word(p,5);
            for axis in 0..3 {
                main.position[axis]=super::lerp(word(p,axis),word(p,axis+6),duration,
                    main.clock as i32 as f32-start);
            }
            main.rotation+=main.angular_step;
            if main.clock as i32 as f32>start+duration {main.phase=2;main.phase_origin=main.clock;}
        }
        if main.phase<=2 {
            if let Some(child)=trail.iter_mut().find(|d|d.phase==0) {
                child.clock=0;child.position=main.position;child.rotation=main.rotation;
                child.path=Trajectory::default();child.path.append(main.position);
                let target=std::array::from_fn(|axis|sample(random)*500.+main.position[axis]-250.);
                child.path.append(target);child.path.finish(self.child_count as f32,100.);
                child.pivot=main.pivot;child.scale=main.scale;child.stretch=main.stretch;
                // Both PEs write the next spin into the PARENT private record.
                // The child's own spin remains the zero from cleared storage.
                main.angular_step=rising_spin(random);child.phase=1;
            }
        }
        for child in trail.iter_mut().filter(|d|d.phase!=0) {
            child.scale*=f32::from_bits(0x3f66_6666);
            child.position=child.path.at(child.clock);child.rotation+=child.angular_step;
            child.alpha=trunc(super::lerp(255.,0.,self.child_count as f32,child.clock as i32 as f32)) as u8;
            child.clock=child.clock.wrapping_add(tick);
            if child.clock as i32>=self.child_count as i32 {child.clock=0;child.phase=0;}
        }
        main.clock=main.clock.wrapping_add(tick);main.captured=main.position;
    }

    fn start_dash(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;self.scale=word(p,2)/100.;
        self.dash=DashConfig {speed:word(p,1),angle:word(p,3),distance:word(p,4),
            spread:word(p,5),alpha:trunc(word(p,6)) as u8};
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            let trail=&mut children[index*self.child_count..(index+1)*self.child_count];
            spawn_dash(d,trail,index,self.image,self.scale,&self.dash,random);
        }
    }

    fn update_dash(&mut self,tick: u32,random: &mut impl ParticleRandomSource) {
        let active=DASH_ACTIVE.load(Ordering::Relaxed).wrapping_add(1).min(self.main_count as u32);
        DASH_ACTIVE.store(active,Ordering::Relaxed);self.active_count=active as usize;
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().take(self.active_count).enumerate() {
            let trail=&mut children[index*self.child_count..(index+1)*self.child_count];
            if d.phase==0 {
                d.motion_origin=add(d.motion_origin,d.motion);d.position=d.motion_origin;d.alpha=self.dash.alpha;
                if length(sub(d.position,d.path.points[0]))>d.motion_limit {d.phase=2;d.phase_origin=d.clock;}
            } else {spawn_dash(d,trail,index,self.image,self.scale,&self.dash,random);}
            if d.phase==0 {
                if let Some(child)=trail.iter_mut().find(|c|c.phase==0) {submit_segment(d,child);}
            }
            advance_segment_trail(trail,tick);
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }

    fn start_energy(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        self.center=[word(p,0),word(p,1),0.];
        self.scale=word(p,4)/100.;if 0.001>self.scale {self.scale=0.001;}
        let range=word(p,5)/100.;self.radius=if 0.001>range {0.001} else {range};
        self.durations=std::array::from_fn(|i|trunc(word(p,8+i)).max(0));
        self.main_count=self.drops.len();self.active_count=self.main_count;
        for d in &mut self.drops {
            d.pivot=self.image.map(|v|v*0.5);d.stretch=[1.;2];d.position=self.center;
            let atlas=random.next_integer()%3;
            d.uv=std::array::from_fn(|i|[(atlas>>1) as f32*0.5+(i&1) as f32*0.5,
                (atlas&1) as f32*0.5+(i>>1) as f32*0.5]);
            d.scale=(sample(random)*2.-1.)*self.radius+self.scale;
            d.alpha=0;d.vertex_rgb=None;
            let direction=normalize_nonzero([sample(random)*2.-1.,sample(random)*2.-1.,0.]);
            let x=(sample(random)*word(p,6)+word(p,6))*direction[0];
            let y=(sample(random)*word(p,6)+word(p,6)*0.5)*direction[1];
            d.path=Trajectory::default();d.path.append(self.center);
            d.path.append(add(self.center,[x,y,0.]));
            d.path.append([word(p,2),word(p,3),0.]);
            let total=self.durations.iter().fold(0_i32,|a,&b|a.wrapping_add(b));
            d.path.finish_average(total as f32);
            // p7 and p11/12 are stored in the PE but unused by22a0/26c0.
            d.clock=self.durations[0] as u32;d.phase_origin=0;d.phase=0;
        }
    }

    fn update_energy(&mut self,tick: u32,random: &mut impl ParticleRandomSource) {
        for d in &mut self.drops {
            if d.phase>2 {continue;}
            let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
            d.position=d.path.at(d.clock);
            if d.phase==1 {d.alpha=trunc(super::lerp(0.,255.,self.durations[1] as f32,elapsed as f32)) as u8;}
            if d.phase==2 {
                d.scale*=f32::from_bits(0x3f7f_3b64);
                d.alpha=trunc(super::lerp(255.,0.,self.durations[2] as f32,elapsed as f32)) as u8;
            }
            d.vertex_rgb=Some(std::array::from_fn(|_| {
                (random.next_integer()%200+56)|((random.next_integer()%200+56)<<8)
                    |((random.next_integer()%200+56)<<16)
            }));
            if elapsed>self.durations[d.phase as usize] {d.phase+=1;d.phase_origin=d.clock;}
            // No source-offset addition, reseed or child allocation in this PE.
            d.clock=d.clock.wrapping_add(tick);
        }
    }

    fn start_hotaru(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        self.scale=word(p,4)/100.;if 0.01>self.scale {self.scale=0.01;}
        let mut size_range=word(p,5)/100.;if 0.01>size_range {size_range=0.01;}
        self.durations=std::array::from_fn(|i|trunc(word(p,i+7)).max(0));
        self.hotaru=HotaruConfig {size_range,radius:word(p,6),horizontal:[word(p,10)/100.,word(p,11)/100.],
            vertical:word(p,13)/100.,direction:trunc(word(p,12)),
            divisor:61_i32.wrapping_sub(trunc(word(p,14))).clamp(1,60) as u32,step:trunc(word(p,15)) as u32};
        self.main_count=self.drops.len();let stagger=trunc(word(p,16))!=0;
        self.active_count=if stagger {self.main_count/2} else {1.min(self.main_count)};
        for (index,d) in self.drops.iter_mut().enumerate() {
            d.pivot=self.image.map(|n|n*0.5);
            spawn_hotaru(d,self.scale,&self.hotaru,self.durations,random);
            if stagger&&index<self.main_count/2 {
                let total=self.durations[0].wrapping_add(self.durations[1]).wrapping_add(self.durations[2]);
                d.clock=trunc(sample(random)*(total as f32)) as u32;
                if d.clock as i32>self.durations[0] {
                    d.phase+=1;d.phase_origin=d.phase_origin.wrapping_add(self.durations[0] as u32);d.alpha=255;
                }
                //3610 compares absolute clock separately against duration1;
                //it does not compare against the cumulative entrance+hold.
                if d.clock as i32>self.durations[1] {d.phase+=1;d.phase_origin=d.phase_origin.wrapping_add(self.durations[1] as u32);}
            }
        }
    }

    fn update_hotaru(&mut self,tick: u32,random: &mut impl ParticleRandomSource) {
        if random.next_integer()%self.hotaru.divisor==0 {
            self.active_count=((self.active_count as u32).wrapping_add(self.hotaru.step) as usize).min(self.main_count);
        }
        for d in self.drops.iter_mut().take(self.active_count) {
            if d.phase>2 {spawn_hotaru(d,self.scale,&self.hotaru,self.durations,random);}
            else {
                d.position=d.path.at(d.clock);
                let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
                let now=if d.phase==0 {d.clock as i32} else {elapsed};
                let base=trunc(match d.phase {
                    0=>super::lerp(0.,d.saved_alpha,self.durations[0] as f32,now as f32),
                    1=>d.saved_alpha,
                    _=>super::lerp(d.saved_alpha,0.,self.durations[2] as f32,now as f32),
                }) as f32;
                let wave=super::sprites::triangle(d.clock as i32 as f32,80.);
                d.alpha=trunc((wave*base)*0.3+base*0.7) as u8;
                if now>self.durations[d.phase as usize] {d.phase+=1;d.phase_origin=d.clock;}
            }
            //1120 explicitly assigns childCount0; its follow-child code is
            //unreachable. 2340 also never adds source/viewport offsets.
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }

    fn start_concent(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        let original=self.profile.kind==PathKind::ConcentOriginal;
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;
        self.center=if original {[word(p,2),word(p,3),0.]} else {[word(p,3),word(p,4),500.]};
        self.scale=word(p,2)/1000.;self.concent_duration=word(p,1);
        self.concent_spread=if original {[word(p,4),word(p,5)]} else {[word(p,5),word(p,6)]};
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            d.pivot=self.image.map(|n|n*0.5);
            let trail=&mut children[index*self.child_count..(index+1)*self.child_count];
            spawn_concent(d,trail,self.center,self.scale,self.concent_spread,self.concent_duration,original,random);
        }
    }

    fn update_concent(&mut self,tick: u32,random: &mut impl ParticleRandomSource) {
        let original=self.profile.kind==PathKind::ConcentOriginal;
        let counter=if original {&CONCENT_ORIGINAL_ACTIVE} else {&CONCENT_ACTIVE};
        let mut active=counter.load(Ordering::Relaxed);
        if random.next_integer()%10==0 {active=active.wrapping_add(10);}
        active=active.min(self.main_count as u32);counter.store(active,Ordering::Relaxed);
        self.active_count=active as usize;
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().take(self.active_count).enumerate() {
            let trail=&mut children[index*self.child_count..(index+1)*self.child_count];
            if d.phase<=1 {
                d.position=d.path.at(d.clock);
                //1120 calls SDK50 on fraction*duration; revised2090 stores
                //the already scaled float threshold in its private record.
                let boundary=if d.phase==0 {if original {trunc((d.saved_alpha*self.concent_duration+0.5).floor()) as f32} else {d.saved_alpha}}
                    else {self.concent_duration};
                if d.clock as i32 as f32>boundary {d.phase=2;d.phase_origin=d.clock;}
            } else {spawn_concent(d,trail,self.center,self.scale,self.concent_spread,self.concent_duration,original,random);}
            if d.phase==0 {
                if let Some(child)=trail.iter_mut().find(|c|c.phase==0) {
                    submit_segment(d,child);
                }
            }
            advance_segment_trail(trail,tick);
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
            //2090 does not add source/viewport offsets to either pool.
        }
    }

    fn start_oko(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        self.center=[word(p,0),word(p,1),0.];self.scale=word(p,2)/100.;
        if 0.001>self.scale {self.scale=0.001;}
        self.factor=if self.image[1]>256. {1.} else {0.5};
        self.durations=std::array::from_fn(|i|trunc(word(p,i+3)));
        self.main_count=self.drops.len();self.active_count=1.min(self.main_count);
        for (index,d) in self.drops.iter_mut().enumerate() {
            d.pivot=self.image.map(|n|n*0.5);d.stretch=[1.;2];
            spawn_oko(d,index,self.center,self.scale,self.factor,self.durations,random);
        }
    }

    fn start_warukira(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        self.center=[word(p,0),word(p,1),0.];self.scale=word(p,2)/100.;
        if 0.001>self.scale {self.scale=0.001;}
        self.radius=word(p,3);self.angle=word(p,4);
        self.durations=std::array::from_fn(|i|trunc(word(p,i+5)).max(0));
        self.main_count=self.drops.len();self.active_count=1.min(self.main_count);
        for (index,d) in self.drops.iter_mut().enumerate() {
            d.pivot=self.image.map(|n|n*0.5);
            spawn_cone(d,self.center,self.scale,self.radius,self.angle,self.durations,Some(index),random);
        }
    }

    fn update_warukira(&mut self,tick: u32,offset: [f32;2],random: &mut impl ParticleRandomSource) {
        if random.next_integer()%5==0 {self.active_count=(self.active_count+1).min(self.main_count);}
        for (index,d) in self.drops.iter_mut().take(self.active_count).enumerate() {
            let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
            if d.phase>2 {
                spawn_cone(d,self.center,self.scale,self.radius,self.angle,self.durations,Some(index),random);
            } else {
                d.position=d.path.at(d.clock);
                if d.phase!=0 {
                    let wave=super::sprites::triangle(d.clock as i32 as f32,30.);
                    d.stretch=[(d.scale+wave/5.)/d.scale,(d.scale-wave/10.)/d.scale];
                    d.alpha=trunc(if d.phase==1 {super::lerp(0.,255.,self.durations[1] as f32,elapsed as f32)}
                        else {super::lerp(255.,0.,self.durations[2] as f32,elapsed as f32)}) as u8;
                }
                if (if d.phase==0 {d.clock as i32} else {elapsed})>self.durations[d.phase as usize] {
                    d.phase+=1;d.phase_origin=d.clock;
                }
            }
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
        }
    }

    fn start_shy(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        let steam=matches!(self.profile.kind,PathKind::Steam|PathKind::SteamAligned);
        let revised=self.profile.kind==PathKind::SteamAligned;
        if revised && trunc(word(p,18))!=0 {self.stream_render_mode=1;}
        let positive=|v: f32|if 0.>v {0.} else {v};
        let angle=|v: f32|v.clamp(-std::f32::consts::PI,std::f32::consts::PI);
        self.scale=word(p,4)/100.;if 0.01>self.scale {self.scale=0.01;}
        self.durations=std::array::from_fn(|i|trunc(word(p,i+8)).max(0));
        let from=[word(p,11),word(p,12),0.];let to=[word(p,13),word(p,14),0.];
        let mut baseline=Trajectory::default();baseline.append(from);baseline.append(to);baseline.finish(100.,1.);
        //25a6 subtracts endpoint2 from endpoint1 before rotating by pi/2.
        let mut normal=sub(from,to);if !steam && length(normal)==0. {normal=[0.,-1.,0.];}
        normal=normalize_nonzero(rotate_radians(normal,std::f32::consts::FRAC_PI_2));
        self.shy=ShyConfig {distance:if 10.>word(p,0) {10.} else {word(p,0)},
            distance_range:positive(word(p,1)),angle:angle(word(p,2)/100.),
            angle_range:angle(word(p,3)/100.),size_range:positive(word(p,5)/100.),
            alpha:positive(word(p,6)),alpha_range:positive(word(p,7)),normal,baseline,
            divisor:61_i32.wrapping_sub(trunc(word(p,15))).clamp(1,60) as u32,
            step:trunc(word(p,16)).max(1) as usize,_behavior:trunc(word(p,17)),
            average_tangents:revised,align_motion:revised && trunc(word(p,19))!=0};
        self.main_count=self.drops.len();self.active_count=if steam && self.shy._behavior!=0 {self.main_count/2} else {1.min(self.main_count)};
        for (index,d) in self.drops.iter_mut().enumerate() {
            d.path=Trajectory::default();d.stretch=[1.;2];
            d.pivot=self.image.map(|n|n*0.5);
            spawn_stream(d,&self.shy,self.scale,self.durations,steam,random);
            if steam && self.shy._behavior!=0 && index<self.active_count {
                let total=self.durations.iter().fold(0_i32,|a,&b|a.wrapping_add(b));
                d.clock=trunc((f64::from(sample(random))*f64::from(total as f32)) as f32) as u32;
                if d.clock as i32>self.durations[0] {d.phase+=1;d.phase_origin=d.phase_origin.wrapping_add(self.durations[0] as u32);d.alpha=128;}
                //3320 compares absolute clock to p9, not p8+p9.
                if d.clock as i32>self.durations[1] {d.phase+=1;d.phase_origin=d.phase_origin.wrapping_add(self.durations[1] as u32);}
            }
        }
    }

    fn update_shy(&mut self,tick: u32,offset: [f32;2],random: &mut impl ParticleRandomSource) {
        let steam=matches!(self.profile.kind,PathKind::Steam|PathKind::SteamAligned);
        if random.next_integer()%self.shy.divisor==0 {
            self.active_count=self.active_count.saturating_add(self.shy.step).min(self.main_count);
        }
        for d in self.drops.iter_mut().take(self.active_count) {
            let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
            if d.phase<=2 {
                d.alpha=trunc(match d.phase {
                    0=>super::lerp(0.,d.saved_alpha,self.durations[0] as f32,d.clock as i32 as f32),
                    1=>d.saved_alpha,
                    _=>super::lerp(d.saved_alpha,0.,self.durations[2] as f32,elapsed as f32),
                }) as u8;
                d.position=d.path.at(d.clock);if !steam {d.position[1]-=(d.clock as i32 as f32)*0.1;}
                if self.shy.align_motion {
                    let v=normalize_nonzero(sub(d.captured,d.position));
                    let angle=f64::from(-v[1]).acos() as f32;
                    d.rotation=if -v[0]>0. {-angle} else {angle};
                }
                let advance=if d.phase==0 {d.clock as i32>=self.durations[0]}
                    else if d.phase==1 {elapsed>=self.durations[1]} else {elapsed>self.durations[2]};
                if advance {d.phase+=1;d.phase_origin=d.clock;}
            } else if steam {spawn_stream(d,&self.shy,self.scale,self.durations,true,random);}
            //1120 assigns zero child records; the generic trail branch is
            //unreachable for these identities. Shy holds at phase3; steam
            //reseeds and does not apply the owning source's translation.
            if !steam {d.position[0]+=offset[0];d.position[1]+=offset[1];}
            d.captured=d.position;
            d.clock=d.clock.wrapping_add(tick);
        }
    }

    fn start_kirakira(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        self.center=[word(p,0),word(p,1),0.];self.scale=word(p,2)/100.;
        if 0.001>self.scale {self.scale=0.001;}
        self.radius=word(p,3);self.angle=word(p,4);
        self.durations=std::array::from_fn(|i|trunc(word(p,i+5)).max(0));
        self.main_count=self.drops.len()/2;self.active_count=1.min(self.main_count);
        for d in &mut self.drops {d.clock=0;d.phase_origin=0;d.phase=0;d.path=Trajectory::default();d.stretch=[1.;2];}
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            d.pivot=self.image.map(|n|n*0.5);
            spawn_kirakira(d,self.center,self.scale,self.radius,self.angle,self.durations,random);
            let child=&mut children[index];child.position=d.position;child.position[2]+=1.;
            child.pivot=d.pivot;child.alpha=0;
        }
    }

    fn update_kirakira(&mut self,tick: u32,offset: [f32;2],random: &mut impl ParticleRandomSource) {
        if random.next_integer()%2==0 {self.active_count=(self.active_count+1).min(self.main_count);}
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().take(self.active_count).enumerate() {
            let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
            if d.phase>2 {
                spawn_kirakira(d,self.center,self.scale,self.radius,self.angle,self.durations,random);
                children[index].clock=0;children[index].phase=0;
            } else {
                d.position=d.path.at(d.clock);
                if d.phase!=0 {
                    let wave=super::sprites::triangle(d.clock as i32 as f32,30.);
                    d.stretch=[(d.scale+wave/20.)/d.scale,(d.scale-wave/40.)/d.scale];
                    d.alpha=trunc(if d.phase==1 {super::lerp(0.,255.,self.durations[1] as f32,elapsed as f32)}
                        else {super::lerp(255.,0.,self.durations[2] as f32,elapsed as f32)}) as u8;
                }
                if (if d.phase==0 {d.clock as i32} else {elapsed})>self.durations[d.phase as usize] {
                    d.phase+=1;d.phase_origin=d.clock;
                }
            }
            let child=&mut children[index];
            if child.phase==0 {child.phase=1;child.uv=sigh_uv(1,false);}
            child.position[..2].copy_from_slice(&d.position[..2]);child.pivot=d.pivot;
            child.scale=d.scale;child.stretch=d.stretch;child.clock=child.clock.wrapping_add(1);
            child.alpha=if d.phase==1 {d.alpha} else {d.alpha>>1};
            child.position[0]+=offset[0];child.position[1]+=offset[1];
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
        }
    }

    fn update_sigh(&mut self,tick: u32,offset: [f32;2]) {
        if self.main_count==0 {return;}
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        let main=&mut parents[0];
        let point=main.path.at(main.clock);main.position[..2].copy_from_slice(&point[..2]);
        if main.phase<=1 {
            let elapsed=main.clock.wrapping_sub(main.phase_origin) as i32;
            if main.phase==0 {main.alpha=trunc(super::lerp(0.,255.,self.durations[0] as f32,elapsed as f32)) as u8;}
            let direction=normalize_nonzero(sub(point,self.center));
            // SDK+28 is acos; the dot against(0,1,0) is directionY,
            //with the perpendicular sign selecting clockwise/counterclockwise.
            let angle=f64::from(direction[1]).acos() as f32;
            let angle=if direction[0]>0. {-angle} else {angle};
            main.rotation=if self.direction==0 {angle-f32::from_bits(0x3f4c_cccd)} else {angle+f32::from_bits(0x3f4c_cccd)};
            if elapsed>self.durations[main.phase as usize] {
                main.phase=if main.phase==0 {1} else {10};main.phase_origin=main.clock;
            }
        }
        if main.phase!=10 {
            if let Some(d)=children.iter_mut().find(|d|d.phase==0) {
                d.captured=main.position;d.position=main.position;d.position[2]-=1.;
                d.scale=main.scale;d.rotation=main.rotation;d.phase=1;
            }
        }
        for d in children.iter_mut().filter(|d|d.phase!=0) {
            d.position[..2].copy_from_slice(&d.captured[..2]);
            d.alpha=trunc(super::lerp(30.,0.,20.,d.clock as i32 as f32)) as u8;
            d.scale*=f32::from_bits(0x3f80_a3d7);d.clock=d.clock.wrapping_add(tick);
            if d.clock as i32>20 {d.alpha=0;d.clock=0;d.phase=0;}
            d.position[0]+=offset[0];d.position[1]+=offset[1];
        }
        main.position[0]+=offset[0];main.position[1]+=offset[1];main.clock=main.clock.wrapping_add(tick);
    }

    pub(super) fn snapshot(&self, canvas: ParticleCanvas, active: bool, submitted: bool,
        elapsed: u32, previous_elapsed: u32, opacity: u32) -> ParticleRenderSnapshot {
        let [w,h] = self.image;
        ParticleRenderSnapshot { active, frame_complete: true, elapsed, previous_elapsed,
            current_quad: self.drops.len() as u32,
            render_mode: if self.profile.kind==PathKind::Mirror {self.orbit_render_mode()}
                else if matches!(self.profile.kind,PathKind::Protect|PathKind::Protect2|PathKind::FirePattern|PathKind::Pond|PathKind::Vortex) {self.stream_render_mode}
                else if self.profile.kind==PathKind::PrismOrbit {self.prism_orbit_render_mode()}
                else if self.profile.kind==PathKind::Firebase {self.firebase_stream_render_mode()}
                else if matches!(self.profile.kind,PathKind::Firebase2|PathKind::Firebase3|PathKind::Firebase4) {self.firebase_render_mode()}
                else if self.profile.kind==PathKind::Tiles {self.tiles_render_mode()}
                else if self.profile.kind==PathKind::Splash {1}
                else if matches!(self.profile.kind,PathKind::PrismPath|PathKind::PrismTrail) {1}
                else if self.profile.kind==PathKind::Radiate {1}
                else if self.profile.kind==PathKind::Thunder {1}
                else if self.profile.kind==PathKind::FallSpark {1}
                else if self.profile.kind==PathKind::Arrow {1}
                else if self.profile.kind==PathKind::BaselineSpark {1}
                else if self.profile.kind==PathKind::Torch {1}
                else if self.profile.kind==PathKind::ConcentOriginal {1}
                else if self.profile.kind==PathKind::SwordSpark {self.stream_render_mode}
                else if self.profile.kind==PathKind::CreateSquare {1}
                else if self.profile.kind==PathKind::EarlySquare {1}
                else if matches!(self.profile.kind,PathKind::WindTrail|PathKind::Ribbon|PathKind::Light) {1}
                else if self.profile.kind==PathKind::Rolling {1}
                else if self.profile.kind==PathKind::Smoke {self.stream_render_mode}
                else if self.profile.kind==PathKind::Ice {self.stream_render_mode}
                else if matches!(self.profile.kind,PathKind::CircleLight|PathKind::CircleLightTuned) {1}
                else if self.profile.kind==PathKind::Photon {1}
                else if self.profile.kind==PathKind::Heart {1}
                else if self.profile.kind==PathKind::BlurStrips {self.stream_render_mode}
                else if matches!(self.profile.kind,PathKind::Bubble|PathKind::Number) {self.stream_render_mode}
                else if matches!(self.profile.kind,PathKind::Message|PathKind::MessageRandom) {self.stream_render_mode}
                else if matches!(self.profile.kind,PathKind::Dust|PathKind::Twinkle|PathKind::ASpark) {1}
                else if self.profile.kind==PathKind::FloatingSpark {self.stream_render_mode}
                else if matches!(self.profile.kind,PathKind::Star|PathKind::StarLegacy) {1}
                else if matches!(self.profile.kind,PathKind::Erase|PathKind::EraseAtlas) {1}
                else if matches!(self.profile.kind,PathKind::RoundY|PathKind::RoundZ) {1}
                else if self.profile.kind==PathKind::Windball {self.wind_render_mode()}
                else if matches!(self.profile.kind,PathKind::Fireball|PathKind::Fireball2) {self.fire_render_mode()}
                else if matches!(self.profile.kind,PathKind::Volt|PathKind::Volt2|PathKind::Volt3|PathKind::Volt4|PathKind::Slash) {self.volt_render_mode()}
                else if matches!(self.profile.kind,PathKind::SteamAligned|PathKind::StreamArrow|PathKind::TextWords) {self.stream_render_mode}
                else if matches!(self.profile.kind,PathKind::Shy|PathKind::Steam|PathKind::Kirakira|PathKind::Concent|PathKind::Hotaru|PathKind::Energy|PathKind::Dash|PathKind::Rising3|PathKind::Rising4|PathKind::Hadou|PathKind::HotaruMove|PathKind::HotaruStay) {1} else {0}, opacity, canvas,
            quads: self.drops.iter().enumerate().map(|(index,d)| {
                let positions = if submitted && d.centered_geometry {
                    // note252b..2567: vertex2.Y is half WIDTH, no pivot write.
                    [[-w*0.5,-h*0.5],[w*0.5,-h*0.5],[-w*0.5,w*0.5],[w*0.5,h*0.5]]
                } else {[[0.,0.],[w,0.],[0.,h],[w,h]]};
                let uv = if submitted {d.uv} else {[[0.,0.],[1.,0.],[0.,1.],[1.,1.]]};
                let (s,c) = f64::from(d.rotation).sin_cos(); let (s,c) = (s as f32,c as f32);
                let (sx,cx)=f64::from(d.rotation_xy[0]).sin_cos();let (sx,cx)=(sx as f32,cx as f32);
                let (sy,cy)=f64::from(d.rotation_xy[1]).sin_cos();let (sy,cy)=(sy as f32,cy as f32);
                let vertices = std::array::from_fn(|i| {
                    let [mut x,mut y] = positions[i];let mut z=0.;
                    if submitted {
                        if let Some(geometry)=d.geometry {[x,y,z]=geometry[i];}
                        x = (x-d.pivot[0])*(d.scale*d.stretch[0]); y = (y-d.pivot[1])*(d.scale*d.stretch[1]);
                        if d.rotation_xy != [0.;2] {
                            let b=y;y=b*cx-z*sx;z=b*sx+z*cx;
                            let a=x;x=a*cy+z*sy;z=-a*sy+z*cy;
                        }
                        let [a,b] = [x,y]; x = a*c-b*s+d.position[0]; y = a*s+b*c+d.position[1];
                    }
                    ParticleVertex { position: [x,y,if submitted {z+d.position[2]} else {0.}], uv: uv[i],
                        color: if submitted {d.vertex_rgb.map_or(0x00ff_ffff,|c|c[i])|(u32::from(d.vertex_alpha.map_or(d.alpha,|a|a[i]))<<24)} else {u32::MAX}, ..Default::default() }
                });
                ParticleQuad {index:index as u32,vertices,offset:[0.;4],depth_bias:if submitted {d.depth_bias} else {0.},flag:if submitted {d.flag} else {1}}
            }).collect() }
    }
}

fn rising_spin(random: &mut impl ParticleRandomSource) -> f32 {
    sample(random)*f32::from_bits(0x4049_0fdb)-f32::from_bits(0x3fc9_0fdb)
}

fn submit_segment(d: &Drop,child: &mut Drop) {
    let difference=sub(d.position,d.captured);let distance=length(difference);
    child.pivot=d.pivot;child.scale=d.scale;child.stretch=d.stretch;
    if distance!=0. {
        let direction=normalize_nonzero(difference);let angle=f64::from(direction[1]).acos() as f32;
        child.rotation=if difference[0]>0. {-angle} else {angle};
        child.position=scale(add(d.position,d.captured),0.5);
        child.stretch[1]*=distance*0.7;
    } else {child.position=d.position;}
    child.phase=1;
}

fn advance_segment_trail(trail: &mut [Drop],tick: u32) {
    let duration=trail.len() as u32;
    for child in trail.iter_mut().filter(|c|c.phase!=0) {
        child.alpha=trunc(super::lerp(255.,0.,duration as f32,child.clock as i32 as f32)) as u8;
        child.clock=child.clock.wrapping_add(tick);
        if child.clock>=duration {child.clock=0;child.phase=0;child.alpha=0;}
    }
}

fn spawn_dash(d: &mut Drop,trail: &mut [Drop],index: usize,image: [f32;2],size: f32,
    c: &DashConfig,random: &mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);
    let atlas=random.next_integer()&3;
    d.uv=std::array::from_fn(|i|[atlas as f32*0.25+(i&1) as f32*0.25,(i>>1) as f32]);
    d.scale=1.;d.stretch=[(sample(random)*size)*0.8+size*0.2,1.];d.alpha=0;
    let mut speed=c.speed;let mut spread=0.;
    for level in 1..6 {
        spread+=(sample(random)*0.9+0.1)*((level as f32*0.1)*c.spread);
        speed*=(sample(random)*0.1)+0.85;
        if random.next_integer()%5==0 {break;}
    }
    spread=if index&1==0 {c.distance*0.5-spread} else {spread-c.distance*0.5};
    //17110 does not use viewport size:640/360 and texture height are
    //actual1e21/1e36 constants/inputs, not guessed fallback dimensions.
    let radians=((f64::from(c.angle)*f64::from(std::f32::consts::PI))/180.) as f32;
    d.rotation=radians;
    let start=add(rotate_radians([spread,-c.distance,0.],radians),[640.,360.,0.]);
    let end=add(rotate_radians([spread,c.distance+image[1],0.],radians),[640.,360.,0.]);
    let difference=sub(end,start);d.motion_limit=length(difference);
    d.motion=scale(normalize_nonzero(difference),speed);d.motion_origin=start;
    d.path=Trajectory::default();d.path.append(start);
    //1a00 seeds private movement separately from SDK transform position.
    d.clock=0;d.phase=0;d.captured=d.position;
    for child in trail {child.clock=0;child.phase=0;child.position=d.position;child.alpha=0;}
}

fn spawn_hotaru(d: &mut Drop,size: f32,c: &HotaruConfig,durations: [i32;3],random: &mut impl ParticleRandomSource) {
    let atlas=random.next_integer()&3;
    d.uv=std::array::from_fn(|i|[(atlas&1) as f32*0.5+(i&1) as f32*0.5,(atlas>>1) as f32*0.5+(i>>1) as f32*0.5]);
    d.scale=(f64::from(sample(random))*f64::from(c.size_range)+f64::from(size)) as f32;d.alpha=0;
    //These are the plugin's actual layout constants, not fallback extents.
    d.position=[(f64::from(sample(random))*1480.-100.) as f32,(f64::from(sample(random))*920.-100.) as f32,0.];
    d.clock=0;d.phase_origin=0;d.phase=0;d.saved_alpha=(random.next_integer()%100+156) as f32;
    let x=(f64::from(sample(random))*f64::from(c.horizontal[1])-f64::from(c.horizontal[0])) as f32;
    let y=match c.direction {
        1=>-(sample(random)*c.vertical+0.05),
        2=>(f64::from(sample(random))*f64::from(c.vertical)-f64::from(c.vertical*0.5)) as f32,
        _=>(f64::from(sample(random))*f64::from(c.vertical)+f64::from(0.05_f32)) as f32,
    };
    let direction=normalize_nonzero([x,y,0.]);
    let distance=(f64::from(sample(random))*f64::from(c.radius)+f64::from(c.radius)) as f32;
    let mut point=add(d.position,scale(direction,distance));
    let forward=normalize_nonzero(sub(point,d.position));
    d.path=Trajectory::default();d.path.append(d.position);d.path.append(point);
    for _ in 0..4 {
        //The recovered angles really are -360/+360 degrees. Keep the
        //coordinate-span sampling and both RNG draws even for zero spans.
        let low=rotate_degrees(forward,-360.);let high=rotate_degrees(forward,360.);
        let span_x=f64::from(high[0]-low[0]).powf(2.).sqrt() as f32;
        let x=(f64::from(sample(random))*f64::from(span_x)-f64::from(span_x*0.5)) as f32;
        let span_y=f64::from(high[1]-low[1]).powf(2.).sqrt() as f32;
        let y=(f64::from(sample(random))*f64::from(span_y)-f64::from(span_y*0.5)) as f32;
        let direction=normalize_nonzero([forward[0]+x,forward[1]+y,0.]);
        let distance=(f64::from(sample(random))*f64::from(c.radius)+f64::from(c.radius)) as f32;
        point=add(point,scale(direction,distance));d.path.append(point);
        if random.next_integer()%15==0 {break;}
    }
    d.path.finish(durations[0].wrapping_add(durations[1]).wrapping_add(durations[2]) as f32,c.radius*1.25);
}

fn spawn_concent(d: &mut Drop,trail: &mut [Drop],center: [f32;3],size: f32,
    spread: [f32;2],duration: f32,original:bool,random: &mut impl ParticleRandomSource) {
    d.scale=if original {(f64::from(sample(random))*f64::from(0.05f32)+f64::from(0.1f32)) as f32}
        else {(sample(random)*size)*0.2+size*0.8};d.alpha=0;
    d.position=center;
    for i in 0..2 {d.position[i]=(random.next_integer()%(trunc(spread[i]) as u32)) as f32-spread[i]*0.5+center[i];}
    d.clock=0;d.phase_origin=0;d.phase=0;d.captured=d.position;
    d.path=Trajectory::default();d.path.append(d.position);d.path.append(center);d.path.finish(duration,100.);
    d.saved_alpha=if original {(f64::from(sample(random))*f64::from(0.2f32)+f64::from(0.6f32)) as f32}
        else {(f64::from(sample(random))*(f64::from(duration)*0.2)+f64::from(duration*0.6)) as f32};
    for child in trail {
        child.clock=0;child.phase=0;child.position=d.position;child.alpha=0;
    }
}

fn spawn_stream(d: &mut Drop,c: &ShyConfig,size: f32,durations: [i32;3],steam: bool,random: &mut impl ParticleRandomSource) {
    d.uv=std::array::from_fn(|i| {
        //The atlas selector is sampled once, below; fill canonical UV first.
        [(i&1) as f32*0.5,(i>>1) as f32*0.5]
    });
    let atlas=random.next_integer()&3;
    for uv in &mut d.uv {uv[0]+=(atlas&1) as f32*0.5;uv[1]+=(atlas>>1) as f32*0.5;}
    d.scale=(sample(random)*c.size_range-c.size_range*0.5)+size;d.alpha=0;
    let mut point=c.baseline.at_time(sample(random)*100.);d.position=point;
    d.path=Trajectory::default();d.path.append(point);
    let angle=if steam {(sample(random)*c.angle_range-c.angle_range*0.5)+c.angle}
        else {(sample(random)*std::f32::consts::PI-std::f32::consts::FRAC_PI_2)+c.angle};
    let distance=(sample(random)*c.distance_range+c.distance_range*0.5)+c.distance;
    point=add(point,scale(rotate_radians(c.normal,angle),distance));d.path.append(point);
    for level in (8..=10).rev() {
        if random.next_integer()%5==0 {continue;}
        let angle=(sample(random)*c.angle_range-c.angle_range*0.5)+c.angle;
        let distance=(sample(random)*c.distance_range+c.distance_range*0.5)+c.distance;
        let distance=if steam {distance} else {(f64::from(distance)*(f64::from(level)*0.09)) as f32};
        point=add(point,scale(rotate_radians(c.normal,angle),distance));d.path.append(point);
    }
    let duration=durations[0].wrapping_add(durations[1]).wrapping_add(durations[2]) as f32;
    if c.average_tangents {d.path.finish_average(duration);} else {d.path.finish(duration,duration*0.4);}
    d.saved_alpha=(sample(random)*c.alpha_range-c.alpha_range*0.5)+c.alpha;
    d.clock=0;d.phase_origin=0;d.phase=0;d.captured=d.position;
}

fn spawn_kirakira(d: &mut Drop,center: [f32;3],size: f32,radius: f32,angle: f32,
    durations: [i32;3],random: &mut impl ParticleRandomSource) {
    spawn_cone(d,center,size,radius,angle,durations,None,random);
}
fn spawn_cone(d: &mut Drop,center: [f32;3],size: f32,radius: f32,angle: f32,
    durations: [i32;3],warukira_index: Option<usize>,random: &mut impl ParticleRandomSource) {
    d.uv=if warukira_index.is_some() {[[0.,0.],[1.,0.],[0.,1.],[1.,1.]]}
        else {std::array::from_fn(|i|[(i&1) as f32*0.5,0.5+(i>>1) as f32*0.5])};
    d.position=center;
    d.scale=if warukira_index.is_some() {(sample(random)*size)*0.5+size*0.5}
        else {(f64::from(sample(random))*f64::from(size)+f64::from(size*0.01)) as f32};
    d.stretch=[1.;2];d.alpha=0;
    let sign=if warukira_index.map_or_else(||random.next_integer()&1==0,|i|i&1==0) {1.} else {-1.};
    let low=rotate_degrees([sign,0.,0.],-angle);let high=rotate_degrees([sign,0.,0.],angle);
    let span_x=f64::from(high[0]-low[0]).powf(2.).sqrt() as f32;
    let x=(f64::from(sample(random))*f64::from(span_x)-f64::from(span_x*0.5)+f64::from(sign)) as f32;
    let span_y=f64::from(high[1]-low[1]).powf(2.).sqrt() as f32;
    let y=(f64::from(sample(random))*f64::from(span_y)-f64::from(span_y*0.5)) as f32;
    let direction=normalize_nonzero([x,y,0.]);
    let distance=(f64::from(sample(random))*f64::from(radius)+f64::from(radius)) as f32;
    d.path=Trajectory::default();d.path.append(center);d.path.append(add(center,scale(direction,distance)));
    d.path.finish(durations[0].wrapping_add(durations[1]).wrapping_add(durations[2]) as f32,1.);
    d.clock=0;d.phase_origin=0;d.phase=0;
}

fn spawn_oko(d: &mut Drop,index: usize,center: [f32;3],size: f32,factor: f32,
    durations: [i32;3],random: &mut impl ParticleRandomSource) {
    d.centered_geometry=false;d.uv=[[0.,0.],[1.,0.],[0.,1.],[1.,1.]];
    let r=sample(random)*size;
    d.scale=(f64::from(r)*0.4+f64::from(size*0.6)) as f32;
    d.durations=durations.map(|time| {let time=time as f32;trunc((sample(random)*time)*0.5+time*0.5)});
    d.rotation=(f64::from(sample(random))*0.5+0.5) as f32;
    if index&1==0 {d.rotation=-d.rotation;}
    let x=(((sample(random)*200.+350.)*size)*factor)*(if index&1==0 {-1.} else {1.});
    let y=((sample(random)*130.+120.)*size)*factor;
    d.position=[center[0]+x,center[1]-y,0.];d.alpha=0;
    let distance=(sample(random)*100.+100.)*(factor*size);
    let step=sigh_direction(d.rotation);
    d.path=Trajectory::default();d.path.append(d.position);d.path.append(add(d.position,scale(step,distance)));
    d.path.finish(d.durations[0].wrapping_add(d.durations[1]) as f32,10.);
    d.clock=0;d.phase_origin=0;d.phase=0;
}

fn rotate_radians(a: [f32;3],angle: f32) -> [f32;3] {
    let angle=trunc(angle*1000.) as f32/1000.;
    let (s,c)=f64::from(angle).sin_cos();let (s,c)=(s as f32,c as f32);
    [a[0]*c+a[1]*(-s),a[0]*s+a[1]*c,a[2]]
}

fn sigh_direction(angle: f32) -> [f32;3] {
    let angle=trunc(angle*1000.) as f32/1000.;
    let (s,c)=f64::from(angle).sin_cos();[s as f32,-(c as f32),0.]
}
fn sigh_uv(column: u32,mirrored: bool) -> [[f32;2];4] {
    std::array::from_fn(|i| {
        let x=if mirrored {1-(i&1)} else {i&1};
        [column as f32*0.5+x as f32*0.5,(i>>1) as f32*0.5]
    })
}

fn spawn_note(d: &mut Drop, center: [f32;3], size: f32, factor: f32, direction: i32,
    durations: [i32;3], random: &mut impl ParticleRandomSource) {
    d.centered_geometry = true; d.position = center;
    let r = sample(random)*size;
    d.scale = (f64::from(r)*0.4 + f64::from(size*f32::from_bits(0x3f19_999a))) as f32;
    d.durations = durations.map(|time| {
        let t = time as f32; let r = sample(random)*t; trunc(r*0.5 + t*0.5)
    });
    d.rotation = (f64::from(sample(random))-0.5) as f32; d.alpha = 0;
    let direction = normalize_nonzero([if direction == 0 {-1.} else {1.},-1.,0.]);
    let amplitude = (size*80.)*factor;
    let distance = (f64::from(sample(random))*f64::from(amplitude)+f64::from(amplitude)) as f32;
    let mut target = add(center,scale(direction,distance));
    // The stored direction is normalized again after float target subtraction.
    let forward = normalize_nonzero(sub(target,center));
    d.path = Trajectory::default();
    for _ in 0..4 {
        let minus = rotate_degrees(forward,-50.); let plus = rotate_degrees(forward,50.);
        // Native pow(x,2),sqrt returns independent coordinate spans. This
        //samples a rectangle around the fixed forward vector, not an angle.
        let span_x = f64::from(plus[0]-minus[0]).powf(2.).sqrt() as f32;
        let jitter_x = (f64::from(sample(random))*f64::from(span_x)-f64::from(span_x*0.5)) as f32;
        let span_y = f64::from(plus[1]-minus[1]).powf(2.).sqrt() as f32;
        let jitter_y = (f64::from(sample(random))*f64::from(span_y)-f64::from(span_y*0.5)) as f32;
        let step = normalize_nonzero([forward[0]+jitter_x,forward[1]+jitter_y,0.]);
        let distance = (f64::from(sample(random))*f64::from(amplitude)+f64::from(amplitude)) as f32;
        target = add(target,scale(step,distance)); d.path.append(target);
    }
    let duration = d.durations[0].wrapping_add(d.durations[1]).wrapping_add(d.durations[2]);
    d.path.finish(duration as f32,(size*120.)*factor);
    d.clock = 0; d.phase_origin = 0; d.phase = 0;
}
fn sample(random: &mut impl ParticleRandomSource) -> f32 { f32::from_bits(random.next_float_bits()) }
fn word(p: &[u8;128],i: usize) -> f32 {super::word(p,i)}
fn trunc(v: f32) -> i32 {super::trunc(v)}
fn sub(a: [f32;3],b: [f32;3]) -> [f32;3] {std::array::from_fn(|i|a[i]-b[i])}
fn add(a: [f32;3],b: [f32;3]) -> [f32;3] {std::array::from_fn(|i|a[i]+b[i])}
fn scale(a: [f32;3],v: f32) -> [f32;3] {a.map(|x|x*v)}
fn length(a: [f32;3]) -> f32 {f64::from((a[0]*a[0]+a[1]*a[1])+a[2]*a[2]).sqrt() as f32}
fn unit(a: [f32;3]) -> [f32;3] {let n=length(a); a.map(|v|v/n)}
fn normalize_nonzero(a: [f32;3]) -> [f32;3] {let n=length(a); if n>0. {a.map(|v|v/n)} else {a}}
fn rotate_degrees(a: [f32;3],degrees: f32) -> [f32;3] {
    let radians = ((f64::from(degrees)*f64::from(std::f32::consts::PI))/180.) as f32;
    let (s,c) = f64::from(radians).sin_cos(); let (s,c) = (s as f32,c as f32);
    let x = c*a[0]; let y = c*a[1];
    [(f64::from(x)-f64::from(s)*f64::from(a[1])) as f32,
     (f64::from(y)+f64::from(s)*f64::from(a[0])) as f32,0.]
}
