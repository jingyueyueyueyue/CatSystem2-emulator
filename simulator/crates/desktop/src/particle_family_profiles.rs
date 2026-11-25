//! Exact resource identities and semantic differences recovered from PE data.
//! RVAs and complete method/constant comparisons are archived as
//! 20261117_D_ppi_{*_start,*_core,*_spawn,*_diff,identity_classification}.
//! These are independent effect descriptions, not original executable code.

#[derive(Clone, Copy, Debug)]
pub(super) struct FieldProfile {
    pub triple_centers: bool,
    pub gravity_parameter: usize,
    pub horizontal_parameter: Option<usize>,
    pub alpha_parameter: usize,
    pub scale_parameter: usize,
    pub blend_parameter: usize,
    pub emission_period: i32,
    pub speed_random: f32,
    pub speed_base: f32,
    pub hold: i32,
    pub fadeout_random: f32,
    pub fadeout_base: f32,
    pub fadeout_damping_below: Option<f32>,
    pub reset_position_on_respawn: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) enum PathKind { Note, Trail, Sigh, Shy, Kirakira, Oko, Warukira, Concent, ConcentOriginal, Hotaru, Energy, Steam, SteamAligned, Dash, Rising3, Rising4, Sakura, Paper, Volt, Volt2, Volt3, Volt4, Fireball, Fireball2, Slash, Hadou, HotaruMove, HotaruStay, Windball, Splash, Tiles, Mirror, Exprode, Firebase, Firebase2, Firebase3, Firebase4, Spark, SparkEx, Rubble, RoundY, RoundZ, Erase, EraseAtlas, Star, StarLegacy, PrismPath, PrismOrbit, PrismTrail, Seed, SeedRot, Radiate, Dust, Twinkle, FloatingSpark, ASpark, Thunder, FallSpark, Arrow, Protect, Protect2, FirePattern, Pond, Vortex, BaselineSpark, Torch, SwordSpark, CreateSquare, Rolling, Smoke, Ice, Ice2, CircleLight, CircleLightTuned, Happy, Photon, RotatingBackground, BlurStrips, Message, MessageRandom, Dark, Invisi, Bubble, Number, Heart, WcFall, EarlySquare, Damage, RadialFlare, WindTrail, Ribbon, Light, Cube, StreamArrow, TextWords }

#[derive(Clone, Copy, Debug)]
pub(super) struct PathProfile {
    pub kind: PathKind,
    pub center_parameter: usize,
    pub scale_parameter: usize,
    pub duration_parameter: usize,
    pub partition_parameter: Option<usize>,
    pub direction_parameter: Option<usize>,
    pub emission_divisor: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(in crate::particle_plugin) enum SpriteProfile { Notice, Asease, Common, Surprise, Dis, Angry, Kyupin,
    Double { shake_by_index: bool, loops: bool }, Exclamation, Down, Silent, FlowerBurst,
    Digital { orbit: bool }, FilmNoise, Ring }

#[derive(Clone, Copy, Debug)]
pub(super) enum Model {
    Field(FieldProfile),
    Question { oscillates_after_hold: bool },
    Path(PathProfile),
    Sprite(SpriteProfile),
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Profile {
    pub sha256: &'static str,
    pub model: Model,
}

const RISING: FieldProfile = FieldProfile {
    triple_centers: false,
    gravity_parameter: 3, horizontal_parameter: Some(2), alpha_parameter: 4,
    scale_parameter: 5, blend_parameter: 6, emission_period: 1,
    speed_random: 0.5, speed_base: 1.5, hold: 120,
    fadeout_random: 100., fadeout_base: 120.,
    fadeout_damping_below: Some(740.), reset_position_on_respawn: false,
};

const NOTE: PathProfile = PathProfile {
    kind: PathKind::Note, center_parameter: 1, scale_parameter: 4,
    duration_parameter: 5, partition_parameter: Some(0),
    direction_parameter: Some(3), emission_divisor: Some(51),
};
const SWEAT: PathProfile = PathProfile {
    kind: PathKind::Trail, center_parameter: 0, scale_parameter: 2,
    duration_parameter: 3, partition_parameter: None,
    direction_parameter: None, emission_divisor: None,
};
const SIGH: PathProfile = PathProfile {
    kind: PathKind::Sigh, center_parameter: 0, scale_parameter: 3,
    duration_parameter: 4, partition_parameter: None,
    direction_parameter: Some(2), emission_divisor: None,
};
const SHY: PathProfile = PathProfile {
    kind: PathKind::Shy, center_parameter: 11, scale_parameter: 4,
    duration_parameter: 8, partition_parameter: None,
    direction_parameter: None, emission_divisor: None,
};
const KIRAKIRA: PathProfile = PathProfile {
    kind: PathKind::Kirakira, center_parameter: 0, scale_parameter: 2,
    duration_parameter: 5, partition_parameter: None,
    direction_parameter: None, emission_divisor: Some(2),
};
const OKO: PathProfile = PathProfile {
    kind: PathKind::Oko, center_parameter: 0, scale_parameter: 2,
    duration_parameter: 3, partition_parameter: None,
    direction_parameter: None, emission_divisor: Some(3),
};
const WARUKIRA: PathProfile = PathProfile {
    kind: PathKind::Warukira, center_parameter: 0, scale_parameter: 2,
    duration_parameter: 5, partition_parameter: None,
    direction_parameter: None, emission_divisor: Some(5),
};
const CONCENT: PathProfile = PathProfile {
    kind: PathKind::Concent, center_parameter: 3, scale_parameter: 2,
    duration_parameter: 1, partition_parameter: Some(0),
    direction_parameter: None, emission_divisor: Some(10),
};
const HOTARU: PathProfile = PathProfile {
    kind: PathKind::Hotaru, center_parameter: 0, scale_parameter: 4,
    duration_parameter: 7, partition_parameter: None,
    direction_parameter: Some(12), emission_divisor: None,
};
const ENERGY: PathProfile = PathProfile {
    kind: PathKind::Energy, center_parameter: 0, scale_parameter: 4,
    duration_parameter: 8, partition_parameter: None,
    direction_parameter: None, emission_divisor: None,
};
const STEAM: PathProfile = PathProfile {kind:PathKind::Steam,..SHY};
const WC_STEAM: PathProfile = PathProfile {kind:PathKind::SteamAligned,..SHY};
const DASH: PathProfile = PathProfile {kind:PathKind::Dash,center_parameter:0,
    scale_parameter:2,duration_parameter:1,partition_parameter:Some(0),
    direction_parameter:Some(3),emission_divisor:None};
const RISING3: PathProfile = PathProfile {kind:PathKind::Rising3,
    center_parameter:0,scale_parameter:3,duration_parameter:4,
    partition_parameter:None,direction_parameter:None,emission_divisor:None};
const RISING4: PathProfile = PathProfile {kind:PathKind::Rising4,..RISING3};
const SAKURA: PathProfile = PathProfile {kind:PathKind::Sakura,
    center_parameter:0,scale_parameter:1,duration_parameter:3,
    partition_parameter:Some(0),direction_parameter:None,emission_divisor:Some(10)};
const PAPER: PathProfile = PathProfile {kind:PathKind::Paper,scale_parameter:2,
    duration_parameter:1,..SAKURA};
const VOLT: PathProfile = PathProfile {kind:PathKind::Volt,center_parameter:0,
    scale_parameter:10,duration_parameter:6,partition_parameter:None,
    direction_parameter:Some(8),emission_divisor:None};
const VOLT2: PathProfile = PathProfile {kind:PathKind::Volt2,..VOLT};
const VOLT3: PathProfile = PathProfile {kind:PathKind::Volt3,..VOLT};
const VOLT4: PathProfile = PathProfile {kind:PathKind::Volt4,scale_parameter:13,..VOLT};
const FIREBALL: PathProfile = PathProfile {kind:PathKind::Fireball,center_parameter:1,
    scale_parameter:11,duration_parameter:7,partition_parameter:None,
    direction_parameter:None,emission_divisor:None};
const FIREBALL2: PathProfile = PathProfile {kind:PathKind::Fireball2,scale_parameter:10,..FIREBALL};
const SLASH: PathProfile = PathProfile {kind:PathKind::Slash,scale_parameter:13,
    duration_parameter:9,..VOLT};
const HADOU: PathProfile = PathProfile {kind:PathKind::Hadou,center_parameter:1,
    scale_parameter:3,duration_parameter:7,partition_parameter:Some(0),
    direction_parameter:None,emission_divisor:None};
const HOTARU_MOVE: PathProfile = PathProfile {kind:PathKind::HotaruMove,
    center_parameter:6,scale_parameter:2,duration_parameter:0,
    partition_parameter:None,direction_parameter:None,emission_divisor:None};
const HOTARU_STAY: PathProfile = PathProfile {kind:PathKind::HotaruStay,center_parameter:7,..HOTARU_MOVE};
const WINDBALL: PathProfile = PathProfile {kind:PathKind::Windball,..VOLT};
const SPLASH: PathProfile = PathProfile {kind:PathKind::Splash,scale_parameter:2,
    duration_parameter:9,..VOLT};
const TILES: PathProfile = PathProfile {kind:PathKind::Tiles,scale_parameter:2,
    duration_parameter:7,..VOLT};
const MIRROR: PathProfile = PathProfile {kind:PathKind::Mirror,scale_parameter:6,
    duration_parameter:10,..VOLT};
const EXPRODE: PathProfile = PathProfile {kind:PathKind::Exprode,scale_parameter:3,
    duration_parameter:4,..VOLT};
const FIREBASE2: PathProfile = PathProfile {kind:PathKind::Firebase2,scale_parameter:4,
    duration_parameter:5,..VOLT};
const FIREBASE: PathProfile = PathProfile {kind:PathKind::Firebase,scale_parameter:2,..FIREBASE2};
const FIREBASE3: PathProfile = PathProfile {kind:PathKind::Firebase3,duration_parameter:6,..FIREBASE2};
const FIREBASE4: PathProfile = PathProfile {kind:PathKind::Firebase4,..FIREBASE3};
const SPARK: PathProfile = PathProfile {kind:PathKind::Spark,scale_parameter:4,duration_parameter:0,..VOLT};
const SPARK_EX: PathProfile = PathProfile {kind:PathKind::SparkEx,..SPARK};
const RUBBLE: PathProfile = PathProfile {kind:PathKind::Rubble,scale_parameter:6,duration_parameter:7,..VOLT};
const ROUND_Y: PathProfile = PathProfile {kind:PathKind::RoundY,scale_parameter:2,..VOLT};
const ROUND_Z: PathProfile = PathProfile {kind:PathKind::RoundZ,..ROUND_Y};
const ERASE: PathProfile = PathProfile {kind:PathKind::Erase,scale_parameter:13,duration_parameter:15,..VOLT};
const ERASE_ATLAS: PathProfile = PathProfile {kind:PathKind::EraseAtlas,..ERASE};
const STAR: PathProfile = PathProfile {kind:PathKind::Star,scale_parameter:4,duration_parameter:5,..VOLT};
const STAR_LEGACY: PathProfile = PathProfile {kind:PathKind::StarLegacy,..STAR};
const PRISM_PATH: PathProfile = PathProfile {kind:PathKind::PrismPath,scale_parameter:4,duration_parameter:6,..VOLT};
const PRISM_TRAIL: PathProfile = PathProfile {kind:PathKind::PrismTrail,partition_parameter:Some(0),duration_parameter:1,..PRISM_PATH};
const PRISM_ORBIT: PathProfile = PathProfile {kind:PathKind::PrismOrbit,scale_parameter:6,duration_parameter:10,..VOLT};
const SEED: PathProfile = PathProfile {kind:PathKind::Seed,scale_parameter:2,duration_parameter:3,..VOLT};
const SEED_ROT: PathProfile = PathProfile {kind:PathKind::SeedRot,..SEED};
const RADIATE: PathProfile = PathProfile {kind:PathKind::Radiate,center_parameter:2,scale_parameter:4,duration_parameter:1,partition_parameter:Some(0),..VOLT};
const DUST: PathProfile = PathProfile {kind:PathKind::Dust,scale_parameter:4,duration_parameter:6,..VOLT};
const TWINKLE: PathProfile = PathProfile {kind:PathKind::Twinkle,..DUST};
const FLOATING_SPARK: PathProfile = PathProfile {kind:PathKind::FloatingSpark,..DUST};
const A_SPARK: PathProfile = PathProfile {kind:PathKind::ASpark,center_parameter:1,scale_parameter:5,duration_parameter:7,..DUST};
const THUNDER: PathProfile = PathProfile {kind:PathKind::Thunder,center_parameter:1,scale_parameter:3,duration_parameter:5,partition_parameter:Some(0),..DUST};
const FALL_SPARK: PathProfile = PathProfile {kind:PathKind::FallSpark,scale_parameter:2,duration_parameter:1,partition_parameter:Some(0),..DUST};
const ARROW: PathProfile = PathProfile {kind:PathKind::Arrow,center_parameter:1,scale_parameter:5,duration_parameter:6,partition_parameter:Some(0),..DUST};
const PROTECT: PathProfile = PathProfile {kind:PathKind::Protect,center_parameter:0,scale_parameter:2,..DUST};
const PROTECT2: PathProfile = PathProfile {kind:PathKind::Protect2,duration_parameter:6,..PROTECT};
const FIRE_PATTERN: PathProfile = PathProfile {kind:PathKind::FirePattern,scale_parameter:2,duration_parameter:3,..DUST};
const POND: PathProfile = PathProfile {kind:PathKind::Pond,center_parameter:0,scale_parameter:7,..DUST};
const VORTEX: PathProfile = PathProfile {kind:PathKind::Vortex,center_parameter:1,scale_parameter:3,partition_parameter:Some(0),..DUST};
const BASELINE_SPARK: PathProfile = PathProfile {kind:PathKind::BaselineSpark,scale_parameter:6,duration_parameter:13,..DUST};
const TORCH: PathProfile = PathProfile {kind:PathKind::Torch,scale_parameter:2,duration_parameter:4,..DUST};
const SWORD_SPARK: PathProfile = PathProfile {kind:PathKind::SwordSpark,scale_parameter:2,duration_parameter:6,..DUST};
const CREATE_SQUARE: PathProfile = PathProfile {kind:PathKind::CreateSquare,scale_parameter:2,duration_parameter:5,..DUST};
const CONCENT_O: PathProfile = PathProfile {kind:PathKind::ConcentOriginal,center_parameter:2,scale_parameter:4,..CONCENT};
const ROLLING: PathProfile = PathProfile {kind:PathKind::Rolling,center_parameter:1,scale_parameter:5,duration_parameter:6,partition_parameter:Some(0),..DUST};
const SMOKE: PathProfile = PathProfile {kind:PathKind::Smoke,center_parameter:1,scale_parameter:3,duration_parameter:0,..DUST};
const ICE: PathProfile = PathProfile {kind:PathKind::Ice,scale_parameter:2,duration_parameter:1,partition_parameter:Some(0),..DUST};
const ICE2: PathProfile = PathProfile {kind:PathKind::Ice2,..ICE};
const CIRCLE_LIGHT: PathProfile = PathProfile {kind:PathKind::CircleLight,scale_parameter:2,..DUST};
const CIRCLE_LIGHT_TUNED: PathProfile = PathProfile {kind:PathKind::CircleLightTuned,..CIRCLE_LIGHT};
const HAPPY: PathProfile = PathProfile {kind:PathKind::Happy,scale_parameter:2,..DUST};
const PHOTON: PathProfile = PathProfile {kind:PathKind::Photon,center_parameter:1,scale_parameter:6,partition_parameter:Some(0),..DUST};
const ROTATING_BACKGROUND: PathProfile = PathProfile {kind:PathKind::RotatingBackground,center_parameter:1,scale_parameter:3,..DUST};
const BLUR_STRIPS: PathProfile = PathProfile {kind:PathKind::BlurStrips,center_parameter:1,scale_parameter:4,..DUST};
const MESSAGE: PathProfile = PathProfile {kind:PathKind::Message,center_parameter:0,scale_parameter:2,..DUST};
const MESSAGE_RANDOM: PathProfile = PathProfile {kind:PathKind::MessageRandom,..MESSAGE};
const DARK: PathProfile = PathProfile {kind:PathKind::Dark,scale_parameter:6,..DUST};
const INVISI: PathProfile = PathProfile {kind:PathKind::Invisi,scale_parameter:2,duration_parameter:7,..DUST};
const BUBBLE: PathProfile = PathProfile {kind:PathKind::Bubble,scale_parameter:7,..DUST};
const NUMBER: PathProfile = PathProfile {kind:PathKind::Number,scale_parameter:2,duration_parameter:4,..DUST};
const HEART: PathProfile = PathProfile {kind:PathKind::Heart,duration_parameter:2,..DUST};
const TEXT_WORDS: PathProfile = PathProfile {kind:PathKind::TextWords,..DUST};
const STREAM_ARROW: PathProfile = PathProfile {kind:PathKind::StreamArrow,..DUST};
const CUBE: PathProfile = PathProfile {kind:PathKind::Cube,..DUST};
const LIGHT: PathProfile = PathProfile {kind:PathKind::Light,..DUST};
const RIBBON: PathProfile = PathProfile {kind:PathKind::Ribbon,partition_parameter:Some(0),..DUST};
const WIND_TRAIL: PathProfile = PathProfile {kind:PathKind::WindTrail,partition_parameter:Some(0),..DUST};
const RADIAL_FLARE: PathProfile = PathProfile {kind:PathKind::RadialFlare,..DUST};
const DAMAGE: PathProfile = PathProfile {kind:PathKind::Damage,..DUST};
const EARLY_SQUARE: PathProfile = PathProfile {kind:PathKind::EarlySquare,..CREATE_SQUARE};
const WC_FALL: PathProfile = PathProfile {kind:PathKind::WcFall,scale_parameter:3,duration_parameter:5,..DUST};

pub(super) const PROFILES: &[Profile] = &[
    //wc_text01:2b40/4600/5340/5800 consumes actual plist words,
    //six phases, quarter-atlas and owned per-word afterimages.
    Profile { sha256: "4ac0ed356b4bf5be68191d3d66b78bffb39df4454d13fa3070395ceb74668cfe",
        model: Model::Path(TEXT_WORDS) },
    //wc_arrow2370/2890/3190/3540: real baseline/depth velocity,
    //three alpha phases and emission gate;1120 creates no child records.
    Profile { sha256: "4ee128491413bd59e21d91ccede4674bc2380d7a5a616c0e377a69534d4cc9b4",
        model: Model::Path(STREAM_ARROW) },
    //o_1501011220/13d0/1d80: three real solids, six-face geometry,
    //lighting/culling, phase clocks and host-consumed SDK7c depth bias.
    Profile { sha256: "1c336dd230bf4d48c5ea2c32427bb43d5dc91dbc21de7ec132f139f96b385e1d",
        model: Model::Path(CUBE) },
    //wc_light1540/1660/1710/1bb0/2560/2710: main pulse, live
    //mode-triggered fades, independent radial sprites;1120 children0.
    Profile { sha256: "f3ca38b7872106b135df16b9178b1663979eb68e6fb96a9d2d9ce75a37ca9470",
        model: Model::Path(LIGHT) },
    //s_0401031210/14a0/2380: dual-baseline parent ribbon, real
    //four-knot child paths, actual atlas halves, spin and timed recycling.
    Profile { sha256: "edfeadddb51cfa123a75010ad601eacdb68630cde36bb67fa9c60653d452ba0e",
        model: Model::Path(RIBBON) },
    //prism10a0/1140/17b0/1ba0: independent warm start, live duration,
    //random alpha and call-count child pool; not prism3's Hermite model.
    Profile { sha256: "a52b9e01131618b990865f9b9ef9ca20b871435bfbe4b7aed774c4385748cf95",
        model: Model::Path(PRISM_TRAIL) },
    //a_wind11e0/1380/1950: six-knot paths, transparent parents and
    //real randomized segment submissions into a timed existing child pool.
    Profile { sha256: "2f1dd4383294b71ee4305e62da34717408564a0ffafb2072731b85dba7cc37ad",
        model: Model::Path(WIND_TRAIL) },
    //a_08010111c0/1270/1af0: quarter-atlas radial launch, per-byte
    //RGB decay, real XY motion/scale and fade/reseed;2020 childCount0.
    Profile { sha256: "f26ada1fbe2b9fdd43e5def7621b117b02d6c2be381b1c57d0c1e29231ef7ba7",
        model: Model::Path(RADIAL_FLARE) },
    //wc_damage1c60/1db0/2320:16x16 texture pieces, private corner
    //velocity, random RGB and three actual fade states; no respawn.
    Profile { sha256: "f9e3cab30f82306ba2ddc71c246fa8c49544e41cce5191cd5929e045f0d4361d",
        model: Model::Path(DAMAGE) },
    //s_0801021070/1120/1920: fixed X/Y rotations,80tick entrance,
    //absolute120tick terminal and zero children; independent module counter.
    Profile { sha256: "51839a9d998b29c1700131a7dd5380eabd3402dea6045f928a36019288f31918",
        model: Model::Path(EARLY_SQUARE) },
    //13a0/14c0/1740: quadrant flakes with cos/sin eased vertical fall;
    //four-call activation, literal phase5 reuse, retained phase origin.
    Profile { sha256: "fe8881db32deb4d0ccda007cdecbafb01b7c9df3fcbae2cea49ab3201358492a",
        model: Model::Path(WC_FALL) },
    //1380/1470/1890: fifteen individual heart atlas pieces plus the
    //independent background slot, delayed shrink/hold/scatter/fade.
    Profile { sha256: "29daedaadbd055e3242c985a1e784a2fb07bd345f4a593de4816e29c14711af6",
        model: Model::Path(HEART) },
    //13e0/1550/17c0: four-way atlas, moving trigonometric emitter,
    //real random velocity/alpha/lifetime and emission completion.
    Profile { sha256: "1844f74fc147efc8a6ba37232cbfc65bca35f66fe8564ba8f6e19720233ce11f",
        model: Model::Path(BUBBLE) },
    //13e0/1510/1880: sixteen-row random glyphs and bright colors,
    //two timed display states; private velocity is never integrated.
    Profile { sha256: "49b06fc5d37252234e8f25ad9ddf4ca3cffb97fd38bf36df033abdaac5ac8140",
        model: Model::Path(NUMBER) },
    //13d0/1540/17f0: quarter atlas, independent radial factors,
    //absolute first transition and relative fade; SDK scale grows in place.
    Profile { sha256: "d2c40530d72c1cbdb152582af6c353bb8f129b57b9f9a3e4000edae1f881005e",
        model: Model::Path(INVISI) },
    Profile { sha256: "a58f167065a032458639736e402408f4dbb6b48f4e60de1ab3c0a1143edc11ef",
        model: Model::Path(DARK) },
    Profile { sha256: "16f22f4b6c79b5e9ad9c170de44706838946403f770e5fcb8544119bc85fd5a3",
        model: Model::Path(MESSAGE) },
    Profile { sha256: "d89d42799316165f3df2c5303e3606767a91534e72576247bc2553a46c4cfdb8",
        model: Model::Path(MESSAGE_RANDOM) },
    Profile { sha256: "037f059afeac87a0974ebdacd023df7b96591f6567fbb93b28c1da22db58491c",
        model: Model::Path(ROTATING_BACKGROUND) },
    Profile { sha256: "ee4c3f91688794c30dae4947fa5b1fc7a5c0a240e5640bc6d595fc738595bd91",
        model: Model::Path(BLUR_STRIPS) },
    //1070/1140/1410: probabilistic directional burst, absolute fade end,
    //ten-attempt cone sampling and the live p0 respawn sentinel.
    Profile { sha256: "5d8481e8a9f58d631ac630f502f50a927e2e53e48e6bd8e342b88c811504543b",
        model: Model::Path(HAPPY) },
    //1070/1180/1460: ellipsoid normalization, two-knot inward flight,
    //per-instance emission and completion/restart. a_photon/photon alias.
    Profile { sha256: "0c5f361cff47927384a8d1c8eb8f322dd60510963d9962e06afb74b1bdb38e14",
        model: Model::Path(PHOTON) },
    //11a0/1240/1440 versus11a0/1280/1480: shared circular placement,
    //but fixed180/full-UV versus configurable150/quarter-UV hold/fade.
    Profile { sha256: "aec7008652dc87f3e7d7c90bf4a40fbba8fd7e53d356a462de53a7ca57ed3654",
        model: Model::Path(CIRCLE_LIGHT) },
    Profile { sha256: "495af92e38378bc20c094cf2e05163c0d133a2c71180f05d13940a94106f0ea2",
        model: Model::Path(CIRCLE_LIGHT_TUNED) },
    //10b0/11f0/1940: actual ten-knot folded fall, attached children,
    //prewarming, spin and stop/respawn. ice2 p6 is alpha, not blend;
    //its144a jumps directly to phase5 instead of ice's flash2/3/4.
    Profile { sha256: "3af77041d4c053d13c3b28cf342972ff06c187de721bfb13ec00c188cb532acf",
        model: Model::Path(ICE) },
    Profile { sha256: "8abc13c57199383dcb95b8eb7599c2d0b722881eceb9577ab4b4507cdd9024e8",
        model: Model::Path(ICE2) },
    //10b0/1160/1630: true float clocks, three emitters, separate CRT/SDK
    //random streams, four-knot smoke, growth and parent-driven reseeding.
    Profile { sha256: "e7090cf8a4c631fff57229123e103c5e4c009e6cf9dfc39d9cbf4713ee74e851",
        model: Model::Path(SMOKE) },
    //1090/1160/18c0: distributed travel, fade-in/out and real four-knot
    //shrinking trails; emitting a child changes the parent's spin step.
    Profile { sha256: "da3dc13eb05b4e7360549535663d474a54e759164819b58f5613fd3763d9048b",
        model: Model::Path(ROLLING) },
    //1070/1120/1750: original rectangle-to-center stream at Z0,
    //random .1..15 scale, quantized .6..8 duration and real segment pool.
    Profile { sha256: "621941872d7aeb0575a7415af69cba43163f39148858a7c68ee1cf8b34e0837e",
        model: Model::Path(CONCENT_O) },
    //1070/1130/1930: fixed atlas, real transformed quad, two absolute
    //clock alpha stages, persistent module activation and terminal2.
    Profile { sha256: "1e0d8267b8a7714bffd8b5f3242013d9e4530bd4d3d26de3b8fa4dbc632c7186",
        model: Model::Path(CREATE_SQUARE) },
    //1070/1110/1860: actual radial3D shards, milliradian face matrices,
    //three integrator stages, per-face lighting and terminal3; no respawn.
    Profile { sha256: "13f33daed17e24e42438e195b06282e492643511fce9eb433e3e61902713346b",
        model: Model::Path(SWORD_SPARK) },
    //2050/21c0/2720/3150: prewarmed five-knot folded stream and respawn.
    Profile { sha256: "d1ec0bbd80346fe945b181ef8275812f44b17af1c38f212c13f6ea6a1297fbed",
        model: Model::Path(TORCH) },
    //11e0/13f0/1700: three-knot baseline, +/-10 degree six-knot drift,
    //per-call random hold alpha, absolute hold and terminal3 persistence.
    //26a0/29b0/32f0: actual eleven-knot joined ribbon geometry,
    //shared matrix consumer,180/220 breathing, edge alpha and UV scroll.
    Profile { sha256: "518c4dfc3eb9d0b672b1eaa8957691f52973e87b937cbe61c4f2034932929a41",
        model: Model::Path(VORTEX) },
    //1c60/1e30/2510: actual matrix geometry, radial quarter-atlas pool,
    //depth bias, persistent rotation and exact absolute-clock hold branch.
    Profile { sha256: "2b9334726ef25abb1a02533551546717e73f64af9d70bf55615b935ba7068b6b",
        model: Model::Path(POND) },
    //1a30/1c10/1ef0: actual16-frame atlas, two activations/call,
    //strict interval progression and terminal2 cumulative source offsets.
    Profile { sha256: "f783b6b4e276c8cb149270316e87ca6b08c80150cd8b5045b77f789fbb189770",
        model: Model::Path(FIRE_PATTERN) },
    //1ce0/1e70/28e0: groups26, actual25 curved faces, vertex alpha,
    //normal lighting, cumulative cos-UV drift and terminal9 persistence.
    //1370/1440/18d0: independent sprite radial spread, quarter atlas,
    //sine/cosine trajectory, mutable target and cumulative rotation.
    //1090/1150/1790: absolute-clock main arrow, four-knot child pool,
    //module-shared activation and exact 2x2 lower-half atlas table10d10.
    //10b0/1230/1930: ten-knot path, CRT first step, SDK later steps,
    //terminal10/restart p6 and a real reusable six-call afterimage pool.
    //13e0/1510/1ab0: CRT atlas, grouped afterimages, SDK04 at1a81
    //clears host active after all parents enter terminal9; no respawn.
    //11e0/1440/1940, helper2450 childCount0: baseline p1..4,
    //three-half atlas, independent vertex greens, +/-50-degree path.
    //2130/2410/29b0: six-knot core, actual p0..3 baseline and p17..22
    //random three-axis spin; p16 keeps the original sticky render mode.
    Profile { sha256: "76f5294c4ad8e3c35c4954496cd9a695216381f341edefa20977fda5c4129120",
        model: Model::Path(FLOATING_SPARK) },
    // Both2130/2320: six-knot path and three-phase lifetime; dust2830,
    // twinkle2970 has its own atlas/random peak and SDK78 wave consumption.
    Profile { sha256: "431411cb40d0c49f61b900de7f9005be9e625b03b4afa22313d28f1db512bc28",
        model: Model::Path(DUST) },
    Profile { sha256: "1a472d45d4b79bdc253202af8b7037e2ea8bbc75bc888ee134537cc0756fd3c3",
        model: Model::Path(TWINKLE) },
    //20e0/21b0/27c0: p0 partitions the real SDK grid; two-point random
    //duration parent plus one captured segment per call and reusable trail.
    //13e0/14a0; seed1990/1cb0, seedrot1a00/1e70. Shared eased
    //transform core, distinct atlas/RNG/depth/mode2 and phase2 tables.
    //13c0/1540/1c30 and13c0/1560/1d70,1d30: independently
    //consumed layouts, fixed vs animated atlas, velocity and terminal alpha.
    // exprode1070/1160/1660: shared module emission count, real
    // 25-frame atlas, randomized ascent and final one-shot disappearance.
    // wc_mirror13a0/1530/1b20: real XZ orbit, inward XYZ
    // orientation, two colored vertices and randomized three-stage lives.
    // s_0801031070/1210/16b0: four real atlas pieces and XYZ
    // affine geometry; SDK+7c bias changes with p8 blend mode.
    // s_020402/splash11e0/13b0/16c0: fixed three-knot seed path,
    // branching randomized paths and one-shot alpha/rotation stages.
    // filmnoise1470/1520, three spawn helpers1840/1a60/1c20:
    // alternating backgrounds, short-lived scratch and random speckles.
];

// Partitioned rolling petals, fourteen-knot paths and frame-aged trails.
pub(super) const SAKURA_PROFILE: Profile = Profile { sha256: "", model: Model::Path(SAKURA) };
pub(super) const QUESTION_PROFILE: Profile =
    Profile { sha256: "", model: Model::Question { oscillates_after_hold: true } };
pub(super) const QUESTION_A_PROFILE: Profile =
    Profile { sha256: "", model: Model::Question { oscillates_after_hold: false } };

pub(super) const DARK_PROFILE: Profile = Profile {
    sha256: "",
    model: Model::Field(DARK_FIELD),
};
pub(super) const DARK_TRI_PROFILE: Profile = Profile {
    sha256: "",
    model: Model::Field(FieldProfile {
        triple_centers: true,
        gravity_parameter: 9,
        alpha_parameter: 10,
        scale_parameter: 11,
        blend_parameter: 12,
        ..DARK_FIELD
    }),
};
pub(super) const DARK_RISING_PROFILE: Profile = Profile {
    sha256: "",
    model: Model::Field(RISING),
};
pub(super) const DARK_GROUND_PROFILE: Profile = Profile {
    sha256: "",
    model: Model::Field(FieldProfile { fadeout_damping_below: Some(800.), ..RISING }),
};

const DARK_FIELD: FieldProfile = FieldProfile {
    triple_centers: false,
    gravity_parameter: 2,
    horizontal_parameter: None,
    alpha_parameter: 3,
    scale_parameter: 4,
    blend_parameter: 5,
    emission_period: 3,
    speed_random: f32::from_bits(0x3ccc_cccd),
    speed_base: 0.75,
    hold: 30,
    fadeout_random: 20.,
    fadeout_base: 60.,
    fadeout_damping_below: None,
    reset_position_on_respawn: true,
};

pub(super) fn identify(sha256: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|profile| profile.sha256 == sha256)
}

pub(super) fn path_effect(effect: super::super::plugin_layout::PathEffect) -> &'static Profile {
    use super::super::plugin_layout::PathEffect;

    match effect {
        PathEffect::Note => &Profile { sha256: "", model: Model::Path(NOTE) },
        PathEffect::Oko => &Profile { sha256: "", model: Model::Path(OKO) },
        PathEffect::Sweat => &Profile { sha256: "", model: Model::Path(SWEAT) },
        PathEffect::Sigh => &Profile { sha256: "", model: Model::Path(SIGH) },
        PathEffect::Energy => &Profile { sha256: "", model: Model::Path(ENERGY) },
        PathEffect::Kirakira => &Profile { sha256: "", model: Model::Path(KIRAKIRA) },
        PathEffect::Warukira => &Profile { sha256: "", model: Model::Path(WARUKIRA) },
        PathEffect::Shy => &Profile { sha256: "", model: Model::Path(SHY) },
        PathEffect::Steam => &Profile { sha256: "", model: Model::Path(STEAM) },
        PathEffect::SteamAligned => &Profile { sha256: "", model: Model::Path(WC_STEAM) },
        PathEffect::Hadou => &Profile { sha256: "", model: Model::Path(HADOU) },
        PathEffect::Thunder => &Profile { sha256: "", model: Model::Path(THUNDER) },
        PathEffect::Firebase => &Profile { sha256: "", model: Model::Path(FIREBASE) },
        PathEffect::Firebase2 => &Profile { sha256: "", model: Model::Path(FIREBASE2) },
        PathEffect::Firebase3 => &Profile { sha256: "", model: Model::Path(FIREBASE3) },
        PathEffect::Firebase4 => &Profile { sha256: "", model: Model::Path(FIREBASE4) },
        PathEffect::Fireball => &Profile { sha256: "", model: Model::Path(FIREBALL) },
        PathEffect::Fireball2 => &Profile { sha256: "", model: Model::Path(FIREBALL2) },
        PathEffect::Slash => &Profile { sha256: "", model: Model::Path(SLASH) },
        PathEffect::Volt => &Profile { sha256: "", model: Model::Path(VOLT) },
        PathEffect::Volt2 => &Profile { sha256: "", model: Model::Path(VOLT2) },
        PathEffect::Volt3 => &Profile { sha256: "", model: Model::Path(VOLT3) },
        PathEffect::Volt4 => &Profile { sha256: "", model: Model::Path(VOLT4) },
        PathEffect::Windball => &Profile { sha256: "", model: Model::Path(WINDBALL) },
        PathEffect::Concent => &Profile { sha256: "", model: Model::Path(CONCENT) },
        PathEffect::Hotaru => &Profile { sha256: "", model: Model::Path(HOTARU) },
        PathEffect::Dash => &Profile { sha256: "", model: Model::Path(DASH) },
        PathEffect::Rising3 => &Profile { sha256: "", model: Model::Path(RISING3) },
        PathEffect::Rising4 => &Profile { sha256: "", model: Model::Path(RISING4) },
        PathEffect::Paper => &Profile { sha256: "", model: Model::Path(PAPER) },
        PathEffect::HotaruMove => &Profile { sha256: "", model: Model::Path(HOTARU_MOVE) },
        PathEffect::HotaruStay => &Profile { sha256: "", model: Model::Path(HOTARU_STAY) },
        PathEffect::Splash => &Profile { sha256: "", model: Model::Path(SPLASH) },
        PathEffect::Rubble => &Profile { sha256: "", model: Model::Path(RUBBLE) },
        PathEffect::Tiles => &Profile { sha256: "", model: Model::Path(TILES) },
        PathEffect::Exprode => &Profile { sha256: "", model: Model::Path(EXPRODE) },
        PathEffect::Star => &Profile { sha256: "", model: Model::Path(STAR) },
        PathEffect::Mirror => &Profile { sha256: "", model: Model::Path(MIRROR) },
        PathEffect::PrismOrbit => &Profile { sha256: "", model: Model::Path(PRISM_ORBIT) },
        PathEffect::Protect => &Profile { sha256: "", model: Model::Path(PROTECT) },
        PathEffect::RoundY => &Profile { sha256: "", model: Model::Path(ROUND_Y) },
        PathEffect::RoundZ => &Profile { sha256: "", model: Model::Path(ROUND_Z) },
        PathEffect::StarLegacy => &Profile { sha256: "", model: Model::Path(STAR_LEGACY) },
        PathEffect::PrismPath => &Profile { sha256: "", model: Model::Path(PRISM_PATH) },
        PathEffect::Spark => &Profile { sha256: "", model: Model::Path(SPARK) },
        PathEffect::SparkEx => &Profile { sha256: "", model: Model::Path(SPARK_EX) },
        PathEffect::Erase => &Profile { sha256: "", model: Model::Path(ERASE) },
        PathEffect::EraseAtlas => &Profile { sha256: "", model: Model::Path(ERASE_ATLAS) },
        PathEffect::BaselineSpark => &Profile { sha256: "", model: Model::Path(BASELINE_SPARK) },
        PathEffect::ASpark => &Profile { sha256: "", model: Model::Path(A_SPARK) },
        PathEffect::Arrow => &Profile { sha256: "", model: Model::Path(ARROW) },
        PathEffect::FallSpark => &Profile { sha256: "", model: Model::Path(FALL_SPARK) },
        PathEffect::Radiate => &Profile { sha256: "", model: Model::Path(RADIATE) },
        PathEffect::Seed => &Profile { sha256: "", model: Model::Path(SEED) },
        PathEffect::SeedRot => &Profile { sha256: "", model: Model::Path(SEED_ROT) },
        PathEffect::Protect2 => &Profile { sha256: "", model: Model::Path(PROTECT2) },
    }
}
