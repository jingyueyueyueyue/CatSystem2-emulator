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
pub(super) enum SpriteProfile { Notice, Asease, Common, Surprise, Dis, Angry, Kyupin,
    Double { shake_by_index: bool, loops: bool }, Exclamation, Down, Silent, FlowerBurst,
    Digital { orbit: bool }, FilmNoise, Ring }

#[derive(Clone, Copy, Debug)]
pub(super) enum Model {
    Field(FieldProfile),
    Question { oscillates_after_hold: bool },
    Path(PathProfile),
    Sprite(SpriteProfile),
}

#[derive(Debug)]
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
    Profile { sha256: "cf89ee777749a2c2283fda0de8f2c17b00c6c0f14d322fc2bb48c5d60890c989",
        model: Model::Path(BASELINE_SPARK) },
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
    Profile { sha256: "83eda4dc223d074bd8411bbb0e06ac0f06430b03016fc118190870d87f2e7970",
        model: Model::Path(PROTECT) },
    //1370/1440/18d0: independent sprite radial spread, quarter atlas,
    //sine/cosine trajectory, mutable target and cumulative rotation.
    Profile { sha256: "de7f4171590d5171d2f24407bfa26c814a82af4e25eed4e88b638ba6dfc588d2",
        model: Model::Path(PROTECT2) },
    //1090/1150/1790: absolute-clock main arrow, four-knot child pool,
    //module-shared activation and exact 2x2 lower-half atlas table10d10.
    Profile { sha256: "dc936c427712811478627ce24ca121faed48a5bd9259bd66a549f374f9f8b66f",
        model: Model::Path(ARROW) },
    //10b0/1230/1930: ten-knot path, CRT first step, SDK later steps,
    //terminal10/restart p6 and a real reusable six-call afterimage pool.
    Profile { sha256: "287be05745459d69f5d93b41f1767cf4bca538a8d1025cb050c3d24aa33ac9ad",
        model: Model::Path(FALL_SPARK) },
    //13e0/1510/1ab0: CRT atlas, grouped afterimages, SDK04 at1a81
    //clears host active after all parents enter terminal9; no respawn.
    Profile { sha256: "d27cab387eeb841b122f2a99870b87ed05aa465d4dddf68cb8d3a10328a06e71",
        model: Model::Path(THUNDER) },
    //11e0/1440/1940, helper2450 childCount0: baseline p1..4,
    //three-half atlas, independent vertex greens, +/-50-degree path.
    Profile { sha256: "583e8604a5f271ffde32d68e871f2e667ef9a638e28c0bea2772ea528672a747",
        model: Model::Path(A_SPARK) },
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
    Profile { sha256: "734ab4e40be8d972cce301f614c203cbe7cdab23ba00d76d9100261f353564f3",
        model: Model::Path(RADIATE) },
    //13e0/14a0; seed1990/1cb0, seedrot1a00/1e70. Shared eased
    //transform core, distinct atlas/RNG/depth/mode2 and phase2 tables.
    Profile { sha256: "bae261fba6166363aff578c76a2b8350423991d81535043711796bd8aed2a049",
        model: Model::Path(SEED) },
    Profile { sha256: "daacc29cb39f7c4844dad30362544dd952f595b19a57395cd696ef1bf163cc75",
        model: Model::Path(SEED_ROT) },
    Profile { sha256: "8b8bbe1635eb4993cbe24c280586a187fd50946b5a3c5be8e7d4abd9482ef1a4",
        model: Model::Path(PRISM_ORBIT) },
    Profile { sha256: "37e24660497c4a3959147bfdd66af5372f439e13544887c69c7100cedaaa0f09",
        model: Model::Path(PRISM_PATH) },
    Profile { sha256: "d05283f4aaa1966976f787f87289613787463d959f1dcefb7655a5cf44144561",
        model: Model::Path(STAR) },
    Profile { sha256: "34be97b7b052bbdfa67dd70bf48e5c79d386cc1e22e69410bc1a8671aa87ff07",
        model: Model::Path(STAR_LEGACY) },
    Profile { sha256: "2794bd1ae6deb7c7eeeddf4c54bf5864919bd1f3515730f03786b42d851daa72",
        model: Model::Path(ERASE_ATLAS) },
    Profile { sha256: "9b4eecb872ff8fe89795e470696de27a7750c34b06a7d0687025a75e6da2d13c",
        model: Model::Path(ERASE) },
    Profile { sha256: "c7ba77f27d1539e20325808949f61a27e26110bb20b7a80aab6a319b13909a36",
        model: Model::Path(FIREBASE) },
    Profile { sha256: "33f5204837d4cfb92bb6c5d52ff94ed956e1600fd5f96a2b6b8a43c0bb921e4a",
        model: Model::Path(ROUND_Y) },
    Profile { sha256: "8a15830795d59628ea2228d2f1e9a8b9ce8238eba87c45490f92c2ec451d7277",
        model: Model::Path(ROUND_Z) },
    Profile { sha256: "e79cf1aae2c1f0e63f267201a352fe09bb7b4ceb7bd25eadbc3d96a9a3a3460d",
        model: Model::Path(RUBBLE) },
    Profile { sha256: "4b7bd73d6135eb161f858e9c7e8261c64542aaab8321ba013bd0355ba8706c5a",
        model: Model::Path(SPARK) },
    Profile { sha256: "bfe589c4957394b262cf5f5fc0a7a6f371848817e61a2158a602cb2db520d1a7",
        model: Model::Path(SPARK_EX) },
    //13c0/1540/1c30 and13c0/1560/1d70,1d30: independently
    //consumed layouts, fixed vs animated atlas, velocity and terminal alpha.
    Profile { sha256: "5b59b5aeb0b27dabaead59ff7230b931d75f252e9ed298e18d82f996290ddc52",
        model: Model::Path(FIREBASE2) },
    Profile { sha256: "3c9e37cf4d806a7f72c1bd839bce9f41646519d8796449485d1049849ced1a88",
        model: Model::Path(FIREBASE3) },
    Profile { sha256: "999020d2a82d9271333f62579b9588b8138e8857f6607f49d00c1acfdc8be3eb",
        model: Model::Path(FIREBASE4) },
    // exprode1070/1160/1660: shared module emission count, real
    // 25-frame atlas, randomized ascent and final one-shot disappearance.
    Profile { sha256: "b91ed75154730c2807b8d8d42ec37f9a5674d7e34bbc0e7bd9fd83c756b3fb4b",
        model: Model::Path(EXPRODE) },
    // wc_mirror13a0/1530/1b20: real XZ orbit, inward XYZ
    // orientation, two colored vertices and randomized three-stage lives.
    Profile { sha256: "d0d41a95f6a7eaf2f9fd470867ed14a87f162b8c8e0ac32f7ea33ae5fde20726",
        model: Model::Path(MIRROR) },
    // s_0801031070/1210/16b0: four real atlas pieces and XYZ
    // affine geometry; SDK+7c bias changes with p8 blend mode.
    Profile { sha256: "5ba106cbf4d823e7a5b0d8809f289271c03d9bdac47285198e67944691dcbe88",
        model: Model::Path(TILES) },
    // s_020402/splash11e0/13b0/16c0: fixed three-knot seed path,
    // branching randomized paths and one-shot alpha/rotation stages.
    Profile { sha256: "8b0d66e708a5d55dafd409837f7b75b08e888a38282fcf7f65ed050ab2d6df7d",
        model: Model::Path(SPLASH) },
    // wc_windball2480/2a10/3450/3780: dynamic pool boundary,
    // two atlas tiles, independently aged radial movement and envelopes.
    Profile { sha256: "1551ff4d125a1d204a3e1a02eeefb6a950a8e42f99b5356dc4e6bdfdc868c225",
        model: Model::Path(WINDBALL) },
    // hotaru_stay2110/2260/31a0: shared path/trail core plus live p5
    // change -> eight-knot exit producer2a14..3109 and phase3 consumer.
    Profile { sha256: "a1273efb9217c2b2786cb3582dfffe24ee56f236266781e5a68a3378c2be139e",
        model: Model::Path(HOTARU_STAY) },
    // hotaru_move2080/21b0/27d0: one path parent, count-1 live
    // segment trails, explicit knots, no random emitter or implicit loop.
    Profile { sha256: "b44453ca2dc29903ffd8e2cb3212400cb692e5e6e1815ad230d82eda863ad918",
        model: Model::Path(HOTARU_MOVE) },
    // hadou13e0/1530/1a80: partitioned ring growth, three stages,
    // periodic emission and40-update low-alpha trails, reused on respawn.
    Profile { sha256: "18a9829b45f3ab4fd3cf0196878fe89fde9bec282a2b7d7335d5e38267301f9a",
        model: Model::Path(HADOU) },
    // wc_slash22d0/2640/2ca0/2d80: explicit control knot, real
    // ribbon vertices and40px splitting, phase3 remains in its pool.
    Profile { sha256: "567d5cf7e75b897451cb085187da24b98f6150f3da755d5e952b293beec5bc0f",
        model: Model::Path(SLASH) },
    // fireball2480/29c0/2fd0 and fireball22460/2960/2d80 share the
    // path core, with distinct motion, atlas, phase spans and alpha units.
    Profile { sha256: "bec4de69845f47e471adf3e7dbc9aca983cd52db72e83271a9404c22dded7a66",
        model: Model::Path(FIREBALL) },
    Profile { sha256: "7c382a711c5ca5066a42a9f654afdbe186a584f53ca0f169c3e69cf981b4fbd3",
        model: Model::Path(FIREBALL2) },
    // volt4:2470/2a80/30b0/3190. Actual segment vertices and compact
    // swap-removal pool; never substituted with the sprite-emitter model.
    Profile { sha256: "14b14f685d9af57656d7de71c6b70603eead9ad87186dd7bbc8fb63e0449a644",
        model: Model::Path(VOLT4) },
    // Shared2480/29a0 with distinct spawn2cb0/2ce0/2db0. The three
    // fixed PEs use different time/alpha types, birth positions and atlas.
    Profile { sha256: "1b3185f2ef94e37ce071712db1309ed943759e834b56931b1dde65886a80e5bf",
        model: Model::Path(VOLT) },
    Profile { sha256: "1f9538bebe4ca154a6da1f56402147cf3ea3163d8b27f9be1c0ef3bdf95ce60f",
        model: Model::Path(VOLT2) },
    Profile { sha256: "56ed7435d6191d979a0b388117fdb3bf8b9822b990b7ba5ab71e893f8e1ab4ec",
        model: Model::Path(VOLT3) },
    // paper10b0/1230/1860: fourteen-knot colored stream. First segment
    // distance comes from module CRT rand, later ones from SDK MT integers.
    Profile { sha256: "eb024cd6f8a5775ee30f974912d374a9755a04c03ba91fcfebedf07ffa49bcd8",
        model: Model::Path(PAPER) },
    // sakura1ff0/2160/2800: partitioned rolling petals, fourteen-knot
    // maximum path, XYZ rotation and independent frame-aged soft trails.
    Profile { sha256: "154233ec97c89dcf98b8054fc60e5ddceb58f814dd3a2ce50ce9fe994698d892",
        model: Model::Path(SAKURA) },
    // rising3:1070/1100/1720 and rising4:1fa0/2030/2680. Both
    // partition one parent and count-1 reusable diffusion trails. Only the
    // older phase0 branches to phase1; rising4's21f4 only rotates in place.
    Profile { sha256: "5e0196bf42bd0837bf76ce04e64e77e32c218e335497fc7a96404ec5fff5361e",
        model: Model::Path(RISING3) },
    Profile { sha256: "c87ddc357d116570a6167e720c9b093713382db677aebc52820ef04e576bfc42",
        model: Model::Path(RISING4) },
    // dash1390/1460/1a00: divided parent/trail grid, constant-step
    // line travel, shared emission counter17110 and segment geometry.
    Profile { sha256: "2c680ed461b97d615d162588a221a812734414b68c2773e597affef9d1b049ca",
        model: Model::Path(DASH) },
    // wc_steam2770/2c90/35d0/3cd0: mean-chord tangents1e50,
    // sticky p18 integer blend flag and optional p19 motion orientation.
    Profile { sha256: "5265655bec679ebf7e6537b93e44ff52d3aa7f3a147e80318fb036c21862a334",
        model: Model::Path(WC_STEAM) },
    // steam21e0/26e0/2c20/3320: equal disassembly+constants in the two
    // exact identities, childCount0 in1120. Reusable branching stream paths.
    Profile { sha256: "940362f102707ae6098ea8517d1f7940e6499dbf62e9369050100b5158e989bd",
        model: Model::Path(STEAM) },
    Profile { sha256: "dea3f0313a42e371485676033102aa37c3caeae97bd3300c62bee151ec49064a",
        model: Model::Path(STEAM) },
    // energy2120/22a0/26c0: three-point trajectory, clock starts at p8,
    // per-vertex MT colors and phase2 damping. 1120 sets childCount0.
    Profile { sha256: "5ce2689feaec351bb633bf02ac0387b2f57a5a9f3e334090d73b53752c99c9be",
        model: Model::Path(ENERGY) },
    // dark_tri1500/16c0/core1780/1cc0: the wc_dark integrator with
    // index%3 centers. Only a changed p11 refreshes p0/1,p3/4,p6/7.
    Profile { sha256: "bddb1bf517e00dd04625b2f312eb3b7a085b51f65342e710e37348dc5d5ec5d4",
        model: Model::Field(FieldProfile {
            triple_centers: true, gravity_parameter: 9, horizontal_parameter: None,
            alpha_parameter: 10, scale_parameter: 11, blend_parameter: 12,
            emission_period: 3, speed_random: f32::from_bits(0x3ccc_cccd),
            speed_base: 0.75, hold: 30, fadeout_random: 20., fadeout_base: 60.,
            fadeout_damping_below: None, reset_position_on_respawn: true,
        }) },
    // hotaru2130/2340/2990/3610: no children in1120; six-point maximum
    // wandering path, pulsed alpha and optionally staggered initial clocks.
    Profile { sha256: "4dd7f14831be2a6d394e132f6e3854bb803501aa9b0989ae97ff4f3cdc1801cd",
        model: Model::Path(HOTARU) },
    // concent1fd0/2090/2660: random rectangle -> center at Z500,
    // chord-oriented segment trails and a module-shared emission counter.
    Profile { sha256: "069aa47909418337c6457923a55fcf7d54e1bb8914c8a045cf1136bc97a58e74",
        model: Model::Path(CONCENT) },
    // ring13e0/1520/1a30: divided main/child pool, periodic activation,
    // absolute-clock size envelope, alpha entrance/exit and one-shot trails.
    Profile { sha256: "1bab19a92effece581e665a6a39c338a3829c070d10d18d3905dda7a23925a3a",
        model: Model::Sprite(SpriteProfile::Ring) },
    // filmnoise1470/1520, three spawn helpers1840/1a60/1c20:
    // alternating backgrounds, short-lived scratch and random speckles.
    Profile { sha256: "7aff22d2b0389c32cfa6c4edd06e3ab43d4ce5f30a53c27f933f73dba9bb2da8",
        model: Model::Sprite(SpriteProfile::FilmNoise) },
    // digi13e0/1550/17d0 and digiring13e0/1560/18e0: shared moving
    // center, biased atlas, random RGB/size/alpha/lifetime; distinct motion.
    Profile { sha256: "e2a6044a983c3074dd75bb8fe2bd0884f2d7e6b691524adbb4e4812be86850cc",
        model: Model::Sprite(SpriteProfile::Digital { orbit: false }) },
    Profile { sha256: "418b15c6971f19ee8abeabc7870b3347609ee694b0b0f9b7a4ef382400e0a3cb",
        model: Model::Sprite(SpriteProfile::Digital { orbit: true }) },
    // ohanaburst1380/14a0/1b10, five float-duration stages and reseed.
    Profile { sha256: "edc067e7cd7051920bba27b51df38d4d0b396a8d641dd69bc9513b0778da3369",
        model: Model::Sprite(SpriteProfile::FlowerBurst) },
    // oko2080/21d0/24f0: alternating side streams, randomized integer
    // durations, two-point path stops before the fadeout stage completes.
    Profile { sha256: "1972b79a62760a6c4bbd28cc2c0f09d15f95b2ae6c52f789ed896a56287ec035",
        model: Model::Path(OKO) },
    // warukira2040/2180/2590: full-texture cone paths, index parity,
    // modulus5 emission and stronger triangular squash. No child partition.
    Profile { sha256: "aaddc494561e30df9056908a162009b3879982a26b44049f36ae57ab70649948",
        model: Model::Path(WARUKIRA) },
    // double1420/1580/18f0 and1900: same entrance/geometry; revised
    // variant selects shake by grid index and never advances phase1.
    Profile { sha256: "2739555cad427fdba36f27fb22487b0fce145e9a83029a7a145fbe2bedbbce0f",
        model: Model::Sprite(SpriteProfile::Double { shake_by_index: false, loops: true }) },
    Profile { sha256: "5d207cb0eeb89891369c0f72aab080cf808ef2b71127aba360b693baaa059e22",
        model: Model::Sprite(SpriteProfile::Double { shake_by_index: true, loops: false }) },
    // exclamation1420/1550/1c70: two excursions, squash/stretch,
    // alpha and stable looping Y-axis swing; five configured durations.
    Profile { sha256: "de23455414987fa6ba5978aa6b0882bb57a2aa0c3017d7f59fa0f670f9846b88",
        model: Model::Sprite(SpriteProfile::Exclamation) },
    // down13b0/1520/1c70+1d80 and silent13a0/1500/1bf0+1d70:
    // first quad is the main glyph, remaining quads staggered decorations.
    Profile { sha256: "447d653b6db379a9f560a3a359fb483807b5852badb9ae0507ba4ecff8a5277c",
        model: Model::Sprite(SpriteProfile::Down) },
    Profile { sha256: "d369d7d50dcee6516407f3ad8f2257fe17c8388b1cdb924663827c1dc422ffe0",
        model: Model::Sprite(SpriteProfile::Silent) },
    // High-use scene producers. shy2370/2860/2e10 has no children in1120;
    // kirakira2040/2180/28b0 partitions the grid in half with one child each.
    Profile { sha256: "39bf02953903ad0b7e3e94e2af8eab6d41c4ef7869bfae98ac831551603cfbca",
        model: Model::Path(SHY) },
    Profile { sha256: "7b868e2a1ba46620337aeef5038df7dc4c61c4c5d86787e3483bd6546d6b0dd1",
        model: Model::Path(KIRAKIRA) },
    // wc_dark SDK2.1: start13c0, update1530/core15b0, spawn1a50,
    // finish1a40. UV origins150b0 = [(0,0),(.5,0),(0,.5),(.5,.5)].
    Profile { sha256: "fa9e19b5ae623f3fe83f7b93c2afb467ad7574d0e5a610c925ee34d03e2d4186",
        model: Model::Field(FieldProfile {
            triple_centers: false,
            gravity_parameter: 2, horizontal_parameter: None, alpha_parameter: 3,
            scale_parameter: 4, blend_parameter: 5, emission_period: 3,
            speed_random: f32::from_bits(0x3ccc_cccd), speed_base: 0.75,
            hold: 30, fadeout_random: 20., fadeout_base: 60.,
            fadeout_damping_below: None, reset_position_on_respawn: true,
        }) },
    // wc_darkund SDK2.1: start13c0, update1540/core15c0,
    // spawn1aa0, finish1a90. Phase2 damps velocityY when it is below740.
    Profile { sha256: "75b5eba8dfb90f4fb92f103776577de2a782d37c53a70a0196935ff93ecce851",
        model: Model::Field(RISING) },
    // wc_ground SDK2.2: same semantic operations with private offsets+8
    // (record2e0 rather than2d8) and a different phase2 threshold800.
    // start13c0, update1540/core15c0, spawn1aa0, finish1a90.
    Profile { sha256: "25be67c1b0639fa3f0c8ece326fc9abc6f070b4e8fa34d0f92c284c64b7702b1",
        model: Model::Field(FieldProfile { fadeout_damping_below: Some(800.), ..RISING }) },
    // Both question identities have identical full method regions, including
    // all five jump-table bodies, constants and spawn bytes. SDK2.1 / SDK2.2.
    // start1390, update14c0, table1998, finish19b0, spawn19c0.
    Profile { sha256: "f9c6468d0196fe58533b0a35a109f5c4a3ecc66b4775ac396c334cd180861a82",
        model: Model::Question { oscillates_after_hold: true } },
    Profile { sha256: "de9de8d2b26be6e1896dd4d078eac461778cd15fe019497f5bf1cb240cbe741e",
        model: Model::Question { oscillates_after_hold: true } },
    // question_a SDK2.2: table1978 phase3->1931 (clock/position only),
    // not the ordinary question's18bd wait-to-phase4 producer. Phase4 exists
    // in the PE but is unreachable from start through this public lifecycle.
    // start1390, update14c0, finish1990, spawn19a0.
    Profile { sha256: "3a412a08bf5e94e50cff7302af620574da7e6503d4d8a039087ad744a7b09594",
        model: Model::Question { oscillates_after_hold: false } },
    // em_note SDK2.1/2.2: start2070/update21c0/finish24d0/spawn24e0.
    // Full method, spawn and path helper regions match, including constants.
    // Four random knots, chord-distance clock, phase0/1/2/10 respawn.
    Profile { sha256: "1694c7df3333180e9ee93cf533729604472c425f3c0ffb9121534ed000190695",
        model: Model::Path(NOTE) },
    Profile { sha256: "58bd95795fd711379de18e13084112069ea048a88e095052001bca279fdb8a4e",
        model: Model::Path(NOTE) },
    // em_sweat SDK2.1/2.2: start2010/update2130/finish2550/spawn2560.
    // One main path; the rest of the SDK grid is its reusable trail pool.
    Profile { sha256: "f4d7ac621f9caa492842163251111522ce79243f1d84b62e3b3f6b504f3ebd3d",
        model: Model::Path(SWEAT) },
    Profile { sha256: "cb791b30afea7024ca4ef833a23977d2913edfa7a7cd630bb05c5a64b13e44b8",
        model: Model::Path(SWEAT) },
    // Actual scene particle3 producers: notice734 / asease836 static records.
    // notice start13b0/update14f0 table16dc/finish16f0/spawn1700.
    // Five visible/hidden phases then stable hold; mirror is geometry, not UV.
    Profile { sha256: "fa95fceeb172bd53894154f26820f0691f8d304613e9c432ad11b0b5e212fe03",
        model: Model::Sprite(SpriteProfile::Notice) },
    // asease start13d0/update14c0/finish1750/spawn1760. Private36B.
    // RNG gates new particles; random cone position and scale reinitialize
    // on phase10. Two randomized private times are unused by the update.
    Profile { sha256: "1f6dd7f3e84b831fb0b947ab5ef2d6eeb57ba8118fff5ab9df87469699c92a3a",
        model: Model::Sprite(SpriteProfile::Asease) },
    // common start1420/update1550/finish19d0/spawn19e0. Two eased
    // excursions then a stable hold; CRT c1b0=cos, c370=sin (fcos fallback).
    Profile { sha256: "0a2e3b242202e591593b8ab371ed8956bdc3fbdcb42a4c12f73af6d9ae8f98d2",
        model: Model::Sprite(SpriteProfile::Common) },
    // suprise start13b0/update14f0/finish1860/spawn1870. Rotating
    // scale3->1 entrance, then a four-position shake and stable hold.
    Profile { sha256: "96d268e1876c6f9dd4fb6578632f3f05f7271a7143657c81dea06c626803f33c",
        model: Model::Sprite(SpriteProfile::Surprise) },
    // dis start1420/update1540/table1bc8/finish1be0/spawn1bf0.
    // Main and subordinate use separate atlas halves; only index1 consumes
    // the phase2 jitter/atlas RNG. p6 is stored but the public update skips it.
    Profile { sha256: "9b1e199e26217702659acc11e5ed16b594172f39a289d41e237a21867e258426",
        model: Model::Sprite(SpriteProfile::Dis) },
    // Both angry identities have byte-identical1120..1ee4 and constants:
    // start13c0/update1510/table1c9c/finish1cb0/spawn1cc0. One parent
    // partitions the existing grid into its own reusable29tick trail pool.
    Profile { sha256: "5c0c3c13f28a46828d493be147e99597f1b74a078538d91930d70b44c3013c96",
        model: Model::Sprite(SpriteProfile::Angry) },
    Profile { sha256: "384f720f7f2a061a5080c1d98d5015edc3f24bcc046d7272d1d29a1133446ae7",
        model: Model::Sprite(SpriteProfile::Angry) },
    // sigh start20e0/update21f0/finish28b0/spawn28c0. Three fixed
    // Hermite knots; one parent plus existing grid tail; left/right atlas.
    Profile { sha256: "afc0eb43679ca0fb1c9456b0a437f62eb562b0499ad00fe8854ffd7b85c031d4",
        model: Model::Path(SIGH) },
    // kyupin start1410/update1550/finish1e20/spawn1e30. A main
    // five-phase scale pulse and bounded random scatter pool; helper20b0
    // consumes the child's age and the actual child count as its duration.
    Profile { sha256: "d23634d199719cad9a1e7d49981becb878a9d7251941d0a09c74572855f66ee9",
        model: Model::Sprite(SpriteProfile::Kyupin) },
];

pub(super) fn identify(sha256: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|profile| profile.sha256 == sha256)
}
