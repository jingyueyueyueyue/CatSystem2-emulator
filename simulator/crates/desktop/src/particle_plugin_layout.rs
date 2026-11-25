//! SDK2 effect models identified by their registered object layouts.

use cs2_runtime::particle_plugin_pe::ParticlePluginPe;
use super::family::profiles::SpriteProfile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Layout {
    Rain,
    AtlasRain,
    ScaledRain,
    Burst,
    Common,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DarkField {
    SingleCenter,
    ThreeCenters,
    Rising,
    Ground,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PathEffect {
    Note,
    Oko,
    Sweat,
    Sigh,
    Energy,
    Kirakira,
    Warukira,
    Shy,
    Steam,
    SteamAligned,
    Hadou,
    Thunder,
    Firebase,
    Firebase2,
    Firebase3,
    Firebase4,
    Fireball,
    Fireball2,
    Slash,
    Volt,
    Volt2,
    Volt3,
    Volt4,
    Windball,
    Concent,
    Hotaru,
    Dash,
    Rising3,
    Rising4,
    Paper,
    HotaruMove,
    HotaruStay,
    Splash,
    Rubble,
    Tiles,
    Exprode,
    Star,
    Mirror,
    PrismOrbit,
    Protect,
    RoundY,
    RoundZ,
    StarLegacy,
    PrismPath,
    Spark,
    SparkEx,
    Erase,
    EraseAtlas,
    BaselineSpark,
    ASpark,
    Arrow,
    FallSpark,
    Radiate,
    Seed,
    SeedRot,
    Protect2,
}

const DIRECT_RELEASE: &[u8] =
    b"\x8b\x4c\x24\x04\x85\xc9\x74\x08\x8b\x01\x8b\x10\x6a\x01\xff\xd2\xc3";
const CONSTRUCTOR: &[u8] =
    b"\x6a?\xe8????\x33\xc9\x83\xc4\x04\x3b\xc1\x74?\x8b\x54\x24\x04\x89\x50\x04\x89\x48\x0c\x89\x48\x10";
const STACK_FRAME_RELEASE: &[u8] =
    b"\x55\x8b\xec\x8b\x4d\x08\x85\xc9\x74\x06\x8b\x01\x6a\x01\xff\x10\x5d\xc3";
const STACK_FRAME_FACTORY: &[u8] =
    b"\x55\x8b\xec\x68????\xe8????\x83\xc4\x04\x85\xc0\x74\x0c\xff\x75\x08\x8b\xc8\xe8";
const SEH_FACTORY: &[u8] =
    b"\x68????\xe8????\x83\xc4\x04\x89\x45\xf0\xc7\x45\xfc\0\0\0\0\x85\xc0\x74\x19\xff\x75\x08\x8b\xc8\xe8";

pub(super) fn detect(pe: &ParticlePluginPe<'_>) -> Option<Layout> {
    common_layout(pe).or_else(|| direct_layout(pe))
}

pub(super) fn path_effect(pe: &ParticlePluginPe<'_>) -> Option<PathEffect> {
    if !matches!(pe.version(), 0x20000..=0x20002) {
        return None;
    }
    let release = pe.release()?;
    let factory = pe.constructor()?;
    if release.starts_with(DIRECT_RELEASE) {
        return legacy_path_effect(pe, factory).or_else(|| legacy_pool_effect(pe, factory));
    }
    if pe.version() == 0x20000 || !release.starts_with(STACK_FRAME_RELEASE) {
        return None;
    }
    if matches_at(factory, 0, STACK_FRAME_FACTORY) {
        let size = word(factory, 4)?;
        return simple_path_effect(pe, size).or_else(|| fire_path_effect(pe, size))
            .or_else(|| stack_frame_pool_effect(pe, size));
    }
    if matches_at(factory, 0x23, SEH_FACTORY) {
        let size = word(factory, 0x24)?;
        return if size == 0x35c {
            first_firebase(pe)
        } else {
            steam_path_effect(pe, size).or_else(|| ribbon_path_effect(pe, size))
                .or_else(|| hotaru_wander_effect(pe, size))
        };
    }
    inline_path_effect(pe, factory).or_else(|| hotaru_flight_effect(pe, factory))
        .or_else(|| inline_frame_pool_effect(pe, factory))
}

fn inline_path_effect(pe: &ParticlePluginPe<'_>, factory: &[u8]) -> Option<PathEffect> {
    let (table_offset, effect) = if matches_at(factory, 0, b"\x55\x8b\xec\x6a\x60\xe8????\x8b\xc8\x83\xc4\x04\x85\xc9")
        && matches_at(factory, 0x37, b"\xc7\x01????\xc7\x41\x40\0\0\0\0")
    {
        (0x39, PathEffect::Concent)
    } else if matches_at(factory, 0, b"\x55\x8b\xec\x6a?\xe8????\x83\xc4\x04\x85\xc0\x74\x31")
        && matches_at(factory, 0x33, b"\xc7\0????\xc7\x40\x3c\0\0\0\0")
    {
        (0x35, match factory[4] {
            0x58 => PathEffect::Dash,
            0x40 => PathEffect::Rising4,
            _ => return None,
        })
    } else {
        return None;
    };
    let table = word(factory, table_offset)?;
    let [destroy, start, update, finish] = pe.callbacks(table)?;
    if !matches_at(destroy, 0, b"\x55\x8b\xec\x56\x8b\xf1\x8b\x46\x3c\xc7\x06")
        || word(destroy, 0x0b)? != table || !finish.starts_with(b"\xc2\x04\0")
    {
        return None;
    }
    let recognized = match effect {
        PathEffect::Concent => {
            path_records(start, 0x0f, 0x07, 0x3c, 0x2ac)
                && matches_at(start, 0x39, b"\xf3\x0f\x2c\x47\x14\x51")
                && matches_at(update, 0x4c, b"\xff\x50\x08\x33\xd2\xb9\x0a\0\0\0\xf7\xf1\x8b\x0d????\x85\xd2\x75\x03\x83\xc1\x0a")
        }
        PathEffect::Dash => {
            // Both allocation and zeroing multiply the record count by 80.
            matches_at(start, 0x0f, b"\x83\x7e\x3c\0\x75\x28\x8b\x07\x8d\x04\x80\xc1\xe0\x04\x50\xe8????\x89\x46\x3c\x83\xc4\x04\x8b\x0f\x8d\x0c\x89\xc1\xe1\x04")
                && matches_at(update, 0x29, b"\x8b\x0d????\x41\x66\x0f\x6e\xc0\xf3\x0f\xe6\xc0\xc1\xe8\x1f\x3b\x4f\x38\x0f\x47\x4f\x38")
        }
        PathEffect::Rising4 => {
            indexed_path_records_by(start, 0x03, 0x3c, 0x2b0)
                && matches_at(start, 0x40, b"\x83\xf8\x01\x75\x3e\x83\x7e\x34\0\x89\x46\x10")
                && matches_at(update, 0x68, b"\x8b\x4f\x08\x83\xe9\0\x0f\x84????\x49\x0f\x85????\x66\x0f\x6e\x07")
        }
        _ => return None,
    };
    recognized.then_some(effect)
}

fn legacy_path_effect(pe: &ParticlePluginPe<'_>, factory: &[u8]) -> Option<PathEffect> {
    if !matches_at(factory, 0, CONSTRUCTOR)
        || !matches_at(factory, 0x1d, b"\x89\x48\x34\x89\x48\x38\xc7\0????\x89\x48\x3c\xc3")
    {
        return None;
    }
    let table = word(factory, 0x25)?;
    let [destroy, start, update, finish] = pe.callbacks(table)?;
    if !matches_at(destroy, 0, b"\x56\x8b\xf1\x8b\x46\x3c\xc7\x06")
        || word(destroy, 8)? != table || !finish.starts_with(b"\xc2\x04\0")
    {
        return None;
    }
    if factory[1] == 0x40
        && matches_at(start, 0x08, b"\x83\x7e\x3c\0\x75\x27\x8b\x45\0\x69\xc0\xb0\x02\0\0\x50\xe8????\x89\x46\x3c\x8b\x4d\0\x69\xc9\xb0\x02\0\0")
        && matches_at(update, 0x5f, b"\x8b\x41\x08\x83\xe8\0\x89\x4c\x24\x10\x89\x5c\x24\x18\x0f\x84????\x83\xe8\x01\x0f\x85")
    {
        Some(PathEffect::Rising3)
    } else if factory[1] == 0x5c
        && matches_at(start, 0x0b, b"\x39\x5e\x3c\x75\x24\x8b\x07\x69\xc0\xb4\x02\0\0\x50\xe8????\x89\x46\x3c\x8b\x0f\x69\xc9\xb4\x02\0\0")
        && matches_at(start, 0x6f, b"\xb9\x3d\0\0\0\x2b\xc8\x83\xf9\x01\x89\x4e\x4c")
        && matches_at(update, 0x3a, b"\x8b\x42\x08\xff\xd0\x33\xd2\xf7\x76\x4c\x85\xd2\x75\x06\x8b\x4e\x50\x01\x4e\x40")
    {
        Some(PathEffect::Paper)
    } else {
        None
    }
}

fn legacy_pool_effect(pe: &ParticlePluginPe<'_>, factory: &[u8]) -> Option<PathEffect> {
    if pe.version() != 0x20001 {
        return None;
    }
    if matches_at(factory, 0x21, b"\x68????\xe8????\x83\xc4\x04\x89\x44\x24\x04\xc7\x44\x24\x10\0\0\0\0\x85\xc0\x74\x1b\x8b\x4c\x24\x18\x51\x50\xe8") {
        let constructor = pe.constructor_call(0x44)?;
        if !matches_at(constructor, 0x46, b"\x6a\x14\x6a\x20\x8d\x7e?\x89\x5c\x24\x28\x57\xc7\x06") {
            return None;
        }
        let table = word(constructor, 0x54)?;
        let [destroy, start, update, finish] = pe.callbacks(table)?;
        if !matches_at(destroy, 0, b"\x56\x8b\xf1\x56\xe8????\xf6\x44\x24\x08\x01")
            || !finish.starts_with(b"\xc2\x04\0")
        {
            return None;
        }
        return match word(factory, 0x22)? {
            0x308 if constructor[0x4c] == 0x48
                && matches_at(start, 0x11, b"\x39\x5e\x3c\x75\x27\x8b\x07\x69\xc0\xbc\x02\0\0\x50\xe8????\x89\x46\x3c\x8b\x0f\x69\xc9\xbc\x02\0\0")
                && matches_at(update, 0x40, b"\x8b\x43\x38\x3b\xc8\x7e\x03\x89\x43\x40\x39\x73\x40") => Some(PathEffect::Splash),
            0x2e0 if constructor[0x4c] == 0x4c
                && matches_at(start, 0x0d, b"\x83\xbe\xdc\x02\0\0\0\x57\x8b\x7d\x08\x75\x2b\x8b\x07\x69\xc0\xac\x02\0\0\x50\xe8????\x89\x86\xdc\x02\0\0\x8b\x0f\x69\xc9\xac\x02\0\0")
                && matches_at(update, 0x42, b"\x8b\x43\x38\x3b\xc8\x76\x03\x89\x43\x3c\x89\x74\x24\x1c\x39\x73\x3c") => Some(PathEffect::Rubble),
            0x300 if constructor[0x4c] == 0x48
                && matches_at(start, 0x1b, b"\x8b\x07\x69\xc0\xbc\x02\0\0\x50\xe8????\x89\x46\x3c\x8b\x0f\x69\xc9\xbc\x02\0\0")
                && matches_at(update, 0x35, b"\x83\x43\x40\x0a\xd9\x5c\x24\x18\x8b\x4b\x40") => Some(PathEffect::BaselineSpark),
            0x300 if constructor[0x4c] == 0x6c
                && matches_at(start, 0x17, b"\x8b\x07\x69\xc0\xb4\x02\0\0\x50\xe8????\x89\x46\x68\x8b\x0f\x69\xc9\xb4\x02\0\0")
                && matches_at(update, 0x3d, b"\xff\xd0\x33\xd2\xb9\x0a\0\0\0\xf7\xf1\x85\xd2\x75\x06\xff\x86\xfc\x02\0\0") => Some(PathEffect::ASpark),
            0x580 if constructor[0x4c] == 0x5c
                && matches_at(start, 0x17, b"\x8b\x07\x69\xc0\xb4\x02\0\0\x50\xe8????\x89\x46\x58\x8b\x0f\x69\xc9\xb4\x02\0\0")
                && matches_at(start, 0x26d, b"\xd9\x47\x0c\xd9\x1c\x24\xe8")
                && matches_at(update, 0x42, b"\xd9\x44\x24\x1c\xe8????\x89\x44\x24\x24\x8b\x4e\x34") => {
                    // The atlas path chooses a tile before taking its first float sample.
                    let spawn = pe.start_call(table, 0x273)?;
                    if !matches_at(spawn, 0x36, b"\x8b\x4e\x08\x8b\x11\x8b\x42?\xff\xd0") {
                        return None;
                    }
                    match spawn[0x3d] {
                        0x0c => Some(PathEffect::Erase),
                        0x08 => Some(PathEffect::EraseAtlas),
                        _ => None,
                    }
                }
            _ => None,
        };
    }

    let (table_offset, storage) = if matches_at(factory, 0, CONSTRUCTOR)
        && matches_at(factory, 0x1d, b"\x89\x48\x34\x89\x48\x38\xc7\0????\x89\x48?\xc3")
    {
        match (factory[1], factory[0x2b]) {
            (0x60, 0x5c) | (0x40, 0x3c) | (0x5c, 0x54) => (0x25, factory[0x2b]),
            _ => return None,
        }
    } else if matches_at(factory, 0, b"\x6a?\xe8????\x33\xc9\x83\xc4\x04\x3b\xc1\x74\x31\xd9\xee")
        && matches_at(factory, 0x25, b"\xc7\0????\xd9\x50\x3c\xd9\x50\x40\xd9\x50\x44\xd9\x50\x48\xd9\x50\x4c\xd9\x58\x50\x89\x48?")
    {
        match (factory[1], factory[0x3f]) {
            (0x68, 0x64) | (0x60, 0x5c) | (0x74, 0x70) => (0x27, factory[0x3f]),
            _ => return None,
        }
    } else {
        return None;
    };
    let table = word(factory, table_offset)?;
    let [destroy, start, update, finish] = pe.callbacks(table)?;
    if !matches_at(destroy, 0, &[0x56, 0x8b, 0xf1, 0x8b, 0x46, storage, 0xc7, 0x06])
        || word(destroy, 8)? != table || !finish.starts_with(b"\xc2\x04\0")
    {
        return None;
    }
    if table_offset == 0x27 {
        return match storage {
            0x70 if matches_at(start, 0x19, b"\x69\xc0\xac\x02\0\0\x50\xe8????\x89\x46\x70\x8b\x0f\x69\xc9\xac\x02\0\0")
                && matches_at(update, 0x31, b"\xff\x47\x6c\xd9\x5c\x24\x1c\x8b\x4f\x6c\x8b\x47\x38\x3b\xc8")
                => Some(PathEffect::PrismPath),
            0x5c if matches_at(start, 0x10, b"\x69\xc0\xbc\x02\0\0\x50\xe8????\x89\x46\x5c\x8b\x0b\x69\xc9\xbc\x02\0\0")
                && matches_at(update, 0x3d, b"\xff\xd0\x33\xd2\xb9\x0a\0\0\0\xf7\xf1\x85\xd2\x75\x06\xff\x05")
                => Some(PathEffect::Arrow),
            0x5c | 0x64 if matches_at(start, 0x17, b"\x69\xc0\xc4\x02\0\0\x50\xe8????\x89\x46?\x8b\x0f\x69\xc9\xc4\x02\0\0")
                && start[0x25] == storage
            && matches_at(start, 0x42, b"\xb8\x25\x49\x92\x24\xf7\xe3\x8b\xc3\x2b\xc2\xd1\xe8")
                && matches_at(update, 0x45, b"\xd9\x44\x24\x50\xe8????\x89\x44\x24\x50\x89\x7c\x24\x4c\x8b\x4e\x34")
                => Some(if storage == 0x64 { PathEffect::Star } else { PathEffect::StarLegacy }),
            _ => None,
        };
    }
    if storage == 0x54 {
        return (matches_at(start, 0x13, b"\x69\xc0\xb4\x02\0\0\x50\xe8????\x89\x46\x54\x8b\x0f\x69\xc9\xb4\x02\0\0")
            && matches_at(update, 0x3d, b"\xff\xd0\x33\xd2\xf7\x76\x44\x85\xd2\x75\x06\x8b\x4e\x48\x01\x4e\x58"))
            .then_some(PathEffect::FallSpark);
    }
    if storage == 0x3c {
        if !matches_at(start, 0x17, b"\x69\xc0\xb4\x02\0\0\x50\xe8????\x89\x46\x3c\x8b\x0b\x69\xc9\xb4\x02\0\0")
            || !matches_at(start, 0x6a, b"\x8d\x43\x14\xd9\x43\x0c\xd9\x1c\x24\x50\x8b\xc7\xe8")
            || !matches_at(update, 0x3e, b"\x8b\x42\x08\xff\xd0\xa8\x01\x75\x06\xff\x05")
        {
            return None;
        }
        let spawn = pe.start_call(table, 0x76)?;
        if matches_at(spawn, 0x75f, b"\xd9\x05????\x8b\x55\x08\x83\xec\x08\xd9\x5c\x24\x04\x8b\xfb\xd9\x02")
            && pe.read_only_data(word(spawn, 0x761)?)?.starts_with(&400_f32.to_le_bytes())
        {
            return Some(PathEffect::Spark);
        }
        return (matches_at(spawn, 0x75f, b"\x8b\x45\x08\xd9\0\x83\xec\x08\xdc\x0d????\x8b\xfb\xd9\x5c\x24\x20")
            && pe.read_only_data(word(spawn, 0x769)?)?.starts_with(&(f64::from(0.55_f32)).to_le_bytes()))
            .then_some(PathEffect::SparkEx);
    }
    if matches_at(start, 0x08, b"\x83\x7e\x5c\0\x75\x25\x8b\x03\x69\xc0\x3c\x03\0\0\x50\xe8????\x89\x46\x5c\x8b\x0b\x69\xc9\x3c\x03\0\0")
        && matches_at(start, 0x60, b"\xd9\x43\x14\xd9\x5e\x3c\xd9\x43\x18\xd9\x5e\x40\xd9\x43\x1c\xdc\x35????")
        && matches_at(update, 0x4f, b"\x89\x44\x24\x14\x8b\x4c\x24\x0c\xd9\xee\x8b\x57\x34\xc1\xe1\x04")
    {
        Some(PathEffect::Tiles)
    } else if matches_at(start, 0x0e, b"\x8b\x07\x8d\x04\x80\x03\xc0\x03\xc0\x50\xe8????\x89\x46\x5c\x8b\x0f\x8d\x0c\x89\x03\xc9\x03\xc9")
        && matches_at(update, 0x31, b"\x8b\x0d????\xd9\x5c\x24\x10\x8b\x43\x38\x83\xc1\x0a\x89\x0d????\x3b\xc8\x76\x05\xa3")
        && word(update, 0x33)? == word(update, 0x43)?
        && word(update, 0x33)? == word(update, 0x4c)?
    {
        Some(PathEffect::Exprode)
    } else {
        None
    }
}

fn inline_frame_pool_effect(pe: &ParticlePluginPe<'_>, factory: &[u8]) -> Option<PathEffect> {
    let (table_offset, storage) = if pe.version() == 0x20002
        && matches_at(factory, 0, b"\x55\x8b\xec\x6a?\xe8????\x8b\xc8\x83\xc4\x04\x85\xc9\x74\x48\x8b\x45\x08\x89\x41\x04")
        && matches_at(factory, 0x37, b"\xc7\x01????\xc7\x41?\0\0\0\0")
    {
        match (factory[4], factory[0x3f]) {
            (0x60, 0x44) => (0x39, 0x3c),
            (0x64, 0x48) => (0x39, 0x60),
            _ => return None,
        }
    } else if pe.version() == 0x20001
        && matches_at(factory, 0, b"\x55\x8b\xec\x68\x80\0\0\0\xe8????\x8b\xc8\x83\xc4\x04\x85\xc9\x74\x48\x8b\x45\x08\x89\x41\x04")
        && matches_at(factory, 0x3a, b"\xc7\x01????\xc7\x41\x40\0\0\0\0")
    {
        (0x3c, 0x7c)
    } else {
        return None;
    };
    let table = word(factory, table_offset)?;
    let [destroy, start, update, finish] = pe.callbacks(table)?;
    if !matches_at(destroy, 0, &[0x55, 0x8b, 0xec, 0x56, 0x8b, 0xf1, 0x8b, 0x46, storage, 0xc7, 0x06])
        || word(destroy, 0x0b)? != table || !finish.starts_with(b"\xc2\x04\0")
    {
        return None;
    }
    match storage {
        0x3c if path_records(start, 0x0f, 0x07, 0x3c, 0x2b4)
            && matches_at(update, 0x43, b"\x66\x0f\x5a\xc0\x39\x46\x40\x76\x03\x89\x46\x40") =>
            Some(PathEffect::Radiate),
        0x7c if matches_at(start, 0x0a, b"\x83\x7e\x7c\0\x75\x27\x8b\x03\x8d\x04\xc0\xc1\xe0\x03\x50\xe8????\x89\x46\x7c\x8b\x0b\x8d\x0c\xc9\xc1\xe1\x03")
            && matches_at(update, 0xa1, b"\x8d\x47\x18\x50\x8d\x45\xa0\x50\xe8????\x83\xec\x08") =>
            Some(PathEffect::Protect2),
        0x60 if matches_at(start, 0x0a, b"\x83\x7e\x60\0\x75\x1b\x6b\x07?\x50\xe8????\x89\x46\x60\x6b\x0f?")
            && start[0x12] == start[0x1e]
            && matches_at(update, 0xbc, b"\x83\x7f?\0\x74\x07\xc7\x47\x08\x03\0\0\0") =>
            match (start[0x12], update[0xbe]) {
                (0x68, 0x64) => Some(PathEffect::Seed),
                (0x6c, 0x68) => Some(PathEffect::SeedRot),
                _ => None,
            },
        _ => None,
    }
}

fn stack_frame_pool_effect(pe: &ParticlePluginPe<'_>, size: u32) -> Option<PathEffect> {
    if size == 0xbc && pe.version() == 0x20002 {
        let [start, update] = stack_frame_effect_callbacks(pe, 0x40, 0x3c)?;
        if !matches_at(start, 0x0a, b"\x83\x7e\x3c\0\x75\x1b\x6b\x07\x34\x50\xe8????\x89\x46\x3c\x6b\x0f\x34")
            || !matches_at(update, 0x23, b"\xff\x81\x94\0\0\0\x66\x0f\x6e\xc0")
        {
            return None;
        }
        let table = word(pe.constructor_call(0x19)?, 0x29)?;
        let spawn = pe.update_call(table, 0xce)?;
        // The orbit callbacks read their parameters from different records.
        return if matches_at(spawn, 0x8a, b"\x8b\x5d\x08\x8d\x49\0\x8b\x4e\x08\x8b\x01\xff\x50\x08\x33\xd2\xb9\x03\0\0\0\xf7\xf1") {
            Some(PathEffect::Mirror)
        } else if matches_at(spawn, 0x87, b"\x83\xc6\x0d\x8d\x9b\0\0\0\0\x8b\x4f\x08\x8b\x01\xff\x50\x08\x33\xd2\xb9\x03\0\0\0\xf7\xf1") {
            Some(PathEffect::PrismOrbit)
        } else {
            None
        };
    }
    if size != 0xe0 {
        return None;
    }
    let constructor = pe.constructor_call(0x19)?;
    if !matches_at(constructor, 0x2a, b"\xc7\x01")
        || !matches_at(constructor, 0x55, b"\xc7\x81\x84\0\0\0\0\0\0\0\xc7\x81\x88\0\0\0\0\0\0\0")
    {
        return None;
    }
    let table = word(constructor, 0x2c)?;
    let [destroy, start, update, finish] = pe.callbacks(table)?;
    if !matches_at(destroy, 0, b"\x55\x8b\xec\x56\x8b\xf1\x8b\x86\xb8\0\0\0\xc7\x06")
        || word(destroy, 0x0e)? != table || !finish.starts_with(b"\xc2\x04\0")
        || !matches_at(start, 0x0a, b"\x83\xbe\xb8\0\0\0\0\x75\x24\x69\x07\xcc\0\0\0\x50\xe8????\x89\x86\xb8\0\0\0\x69\x0f\xcc\0\0\0")
    {
        return None;
    }
    if pe.version() == 0x20002 {
        return matches_at(update, 0x22, b"\x6a\x01\x6a\x01\xff\x77\x04\x8b\x01\xff\x90\x88\0\0\0\x8b\x4f\x08\x6a\x01\x6a\x02\xff\x77\x04\x8b\x01\xff\x90\x88\0\0\0")
            .then_some(PathEffect::Protect);
    }
    if !matches_at(start, 0x131, b"\x33\xff\x39\x7e\x38\x76\x0e\x57\x8b\xce\xe8")
        || !matches_at(update, 0x21, b"\x2b\x45\x0c\x89\x54\x24\x28\x66\x0f\x6e\xc0\xf3\x0f\xe6\xc0")
    {
        return None;
    }
    let spawn = pe.start_call(table, 0x13b)?;
    // Each rotation writes its own initial angles into the shared record.
    if matches_at(spawn, 0x0ae, b"\xc7\x46\x58\xcd\xcc\xcc\xbd\xc7\x46\x5c\xdb\x0f\x49\x40\xc7\x46\x60\xdb\x0f\x89\x3f") {
        Some(PathEffect::RoundY)
    } else if matches_at(spawn, 0x98, b"\xc7\x46\x58\xdb\x0f\xc9\xbf\xc7\x46\x5c\0\0\0\0\xc7\x46\x60\0\0\0\0") {
        Some(PathEffect::RoundZ)
    } else {
        None
    }
}

fn hotaru_wander_effect(pe: &ParticlePluginPe<'_>, size: u32) -> Option<PathEffect> {
    if size != 0x328 {
        return None;
    }
    let constructor = pe.constructor_call(0x43)?;
    if !matches_at(constructor, 0x68, b"\xc7\x07????\xe8????\xc7\x86\x88\x02\0\0\x14\0\0\0")
        || !matches_at(constructor, 0x93, b"\xc7\x87\x90\0\0\0\0\0\0\0")
    {
        return None;
    }
    let table = word(constructor, 0x6a)?;
    let [destroy, start, update, finish] = pe.callbacks(table)?;
    (matches_at(destroy, 0x29, b"\xc7\x06????\xc7\x45\xfc\0\0\0\0\x8b\x86\x90\0\0\0")
        && word(destroy, 0x2b)? == table && finish.starts_with(b"\xc2\x04\0")
        && matches_at(start, 0x0a, b"\x83\xbe\x90\0\0\0\0\x75\x24\x69\x07\xc4\x02\0\0\x50\xe8????\x89\x86\x90\0\0\0\x69\x0f\xc4\x02\0\0")
        && matches_at(start, 0xe8, b"\xb9\x3d\0\0\0\xc7\x86\x1c\x03\0\0\x14\0\0\0\xc7\x86\x24\x03\0\0\x01\0\0\0")
        && matches_at(update, 0x92, b"\x8b\x41\x08\x83\xe8\0\x0f\x84????\x48\x0f\x84????\x48\x74\x1a"))
        .then_some(PathEffect::Hotaru)
}

fn hotaru_flight_effect(pe: &ParticlePluginPe<'_>, factory: &[u8]) -> Option<PathEffect> {
    if !matches_at(factory, 0x24, b"\x68????\xe8????\x8b\xf0\x83\xc4\x04\x89\x75\xf0")
        || !matches_at(factory, 0x6d, b"\x6a\x14\x6a\x0c\x8d\x4e?\xc6\x45\xfc\x01\x51\xc7\x06????\xe8????\xc7\x46\x3c\0\0\0\0")
    {
        return None;
    }
    let (storage, stride, knot_count, effect) = match (word(factory, 0x25)?, factory[0x73]) {
        (0x148, 0x58) => (0x58, 0x2b0, 0x54, PathEffect::HotaruMove),
        (0x14c, 0x5c) => (0x5c, 0x540, 0x58, PathEffect::HotaruStay),
        _ => return None,
    };
    let table = word(factory, 0x7b)?;
    let [destroy, start, update, finish] = pe.callbacks(table)?;
    if !matches_at(destroy, 0x29, b"\xc7\x06????\xc7\x45\xfc\0\0\0\0\x8b\x46\x3c")
        || word(destroy, 0x2b)? != table || !matches_at(destroy, 0x56, &[0x8d, 0x46, storage])
        || !finish.starts_with(b"\xc2\x04\0") || !path_records(start, 0x0f, 0x07, 0x3c, stride)
    {
        return None;
    }
    let recognized = match effect {
        PathEffect::HotaruMove => matches_at(start, 0xea, &[0x3b, 0x5e, knot_count, 0x7c, 0xe1])
            && matches_at(update, 0x90, b"\xc7\x44\x24\x04\0\0\x34\x43\x66\x0f\x6e\xc0"),
        PathEffect::HotaruStay => matches_at(start, 0xfa, &[0x3b, 0x5e, knot_count, 0x7c, 0xe1])
            && matches_at(update, 0x89, b"\x83\xe8\x02\x74\x34\x8b\x45\x08"),
        _ => return None,
    };
    recognized.then_some(effect)
}

fn path_records(start: &[u8], offset: usize, input: u8, storage: u8, stride: u32) -> bool {
    let object = if input == 0x07 { 0x7e } else { 0x7f };
    matches_at(start, offset, b"\x83??\0\x75\x24\x69?????\x50\xe8????\x89??\x83\xc4\x04\x69?????")
        && start[offset + 1] == object && start[offset + 0x13] == object - 0x38
        && start[offset + 2] == storage && start[offset + 0x14] == storage
        && start[offset + 7] == input && start[offset + 0x19] == input + 8
        && word(start, offset + 8) == Some(stride) && word(start, offset + 0x1a) == Some(stride)
}

fn ribbon_path_effect(pe: &ParticlePluginPe<'_>, size: u32) -> Option<PathEffect> {
    let constructor = pe.constructor_call(0x43)?;
    let table_offset = match size {
        0x34c | 0x358 | 0x374 => 0x62,
        0x370 if matches_at(constructor, 0x56, b"\xc7\x07????\x68????\xc7\x47\x3c\0\0\0\0") => 0x58,
        0x370 => 0x5d,
        _ => return None,
    };
    if !matches_at(constructor, table_offset - 2, b"\xc7\x07") {
        return None;
    }
    let table = word(constructor, table_offset)?;
    let [destroy, start, update, finish] = pe.callbacks(table)?;
    if !matches_at(destroy, 0x29, b"\xc7\x06????\xc7\x45\xfc\0\0\0\0\x8b\x46?")
        || word(destroy, 0x2b)? != table
        || !finish.starts_with(b"\xc2\x04\0")
    {
        return None;
    }
    let storage = destroy[0x38];
    if table_offset == 0x58 {
        // Slash uses the same pool stride as Volt4, with integral control knots.
        return (storage == 0x78
            && path_records(start, 0x11, 0x03, storage, 0x2cc)
            && matches_at(start, 0x157, b"\xf3\x0f\x2c\x43\x3c\x89\x87\x40\x03\0\0")
            && matches_at(update, 0x29d, b"\x69\x83\x2c\x03\0\0\xcc\x02\0\0"))
            .then_some(PathEffect::Slash);
    }
    let (stride, effect) = match (size, storage) {
        (0x358, 0x78) => (0x2cc, PathEffect::Fireball),
        (0x34c, 0x74) => (0x2c4, PathEffect::Fireball2),
        (0x34c, 0x6c) => (0x2d8, volt_spawn_effect(pe, table, update)?),
        (0x370, 0x78) => (0x2cc, PathEffect::Volt4),
        (0x374, 0x78) => (0x2e0, PathEffect::Windball),
        _ => return None,
    };
    if !path_records(start, 0x13, 0x06, storage, stride) {
        return None;
    }
    let recognized = match effect {
        PathEffect::Fireball => matches_at(update, 0x1e, b"\x83\x87\x2c\x03\0\0\x02\x66\x0f\x6e\xc0"),
        PathEffect::Fireball2 => matches_at(update, 0x19, b"\xff\x87\x20\x03\0\0\x66\x0f\x6e\xc0"),
        PathEffect::Volt4 => matches_at(start, 0xa6, b"\x8b\x46\x3c")
            && matches_at(update, 0x3be, b"\x69\x83\x2c\x03\0\0\xcc\x02\0\0"),
        PathEffect::Windball => matches_at(update, 0x16, b"\x8d\x4f\x7c\x2b\x45\x0c\x66\x0f\x6e\x97\x2c\x03\0\0"),
        _ => true,
    };
    recognized.then_some(effect)
}

fn volt_spawn_effect(pe: &ParticlePluginPe<'_>, table: u32, update: &[u8]) -> Option<PathEffect> {
    if !matches_at(update, 0x1e, b"\xff\x86\x20\x03\0\0\x66\x0f\x6e\xc0") {
        return None;
    }
    let spawn = pe.update_call(table, 0x100)?;
    // The first two variants write different phase lengths. The third instead
    // selects one of four atlas columns through the SDK random callback.
    if matches_at(spawn, 0, b"\x55\x8b\xec\x8b\x41\x34\x8b\x55\x08") {
        if matches_at(spawn, 0x96, b"\xc7\x86\xd0\x02\0\0\x03\0\0\0\xc7\x86\xd4\x02\0\0\x1e\0\0\0") {
            Some(PathEffect::Volt)
        } else if matches_at(spawn, 0x96, b"\xc7\x86\xd0\x02\0\0\x05\0\0\0\xc7\x86\xd4\x02\0\0\x14\0\0\0") {
            Some(PathEffect::Volt2)
        } else {
            None
        }
    } else if matches_at(spawn, 0x29, b"\x8b\x4e\x08\x8b\x01\xff\x50\x08\x8b\xd0\x83\xe2\x03\x8b\x0c\xd5") {
        Some(PathEffect::Volt3)
    } else {
        None
    }
}

fn first_firebase(pe: &ParticlePluginPe<'_>) -> Option<PathEffect> {
    let constructor = pe.constructor_call(0x43)?;
    if !matches_at(constructor, 0x63, b"\xc7\x07????\x6a\x14\xc7\x47\x3c\0\0\0\0") {
        return None;
    }
    let [start, update] = seh_effect_callbacks(pe, word(constructor, 0x65)?, 0x7c)?;
    (indexed_path_records(start, 0x7c, 0x2d0)
        && matches_at(start, 0x9a, b"\xc7\x86\x34\x03\0\0\0\0\0\0")
        && matches_at(update, 0x16, b"\x2b\x45\x0c\xff\x83\x30\x03\0\0\x66\x0f\x6e\xc0"))
        .then_some(PathEffect::Firebase)
}

fn fire_path_effect(pe: &ParticlePluginPe<'_>, size: u32) -> Option<PathEffect> {
    let (initial_field, storage, stride, effect) = match size {
        0xa0 => (0x3c, 0x68, 0x2ac, PathEffect::Hadou),
        0xa8 => (0x3c, 0x70, 0x2b4, PathEffect::Thunder),
        0xb8 => (0x58, 0x54, 0x2d8, PathEffect::Firebase2),
        0xbc => (0x5c, 0x58, 0x2e0, PathEffect::Firebase3),
        0xc0 => (0x60, 0x5c, 0x2e0, PathEffect::Firebase4),
        _ => return None,
    };
    let [start, update] = stack_frame_effect_callbacks(pe, initial_field, storage)?;
    if !indexed_path_records(start, storage, stride) {
        return None;
    }
    let recognized = match effect {
        PathEffect::Hadou => {
            matches_at(start, 0xa4, b"\xf3\x0f\x2c\x47\x28\xb9\x01\0\0\0\x85\xc0\x0f\x48\xc1")
                && matches_at(update, 0x34, b"\xf7\x77\x54\x66\x0f\x5a\xc8\x85\xd2\x75\x03\xff\x47\x6c")
        }
        PathEffect::Thunder => {
            matches_at(start, 0x86, b"\xf3\x0f\x2c\x47\x24\xb9\x01\0\0\0\x85\xc0\x0f\x48\xc1")
                && matches_at(update, 0x19, b"\xff\x47\x7c\x66\x0f\x6e\xc0\xf3\x0f\xe6\xc0\xc1\xe8\x1f\xf6\x47\x7c\x01")
        }
        PathEffect::Firebase2 => {
            matches_at(start, 0x83, b"\xf3\x0f\x2c\x47\x28\x89\x46\x48\xf3\x0f\x2c\x47\x2c\x89\x46\x4c")
                && matches_at(update, 0x5e, b"\xe8????\xf3\x0f\x2c\x45\x10\x01\x46\x70\x5e\x5d\xc2\x0c\0")
        }
        PathEffect::Firebase3 => {
            matches_at(start, 0x83, b"\x8b\x47\x28\x89\x46\x48\xf3\x0f\x2c\x47\x2c\x89\x46\x4c")
                && matches_at(start, 0xdb, b"\xc7\x86\x90\0\0\0\x03\0\0\0")
                && matches_at(update, 0x5e, b"\xe8????\xf3\x0f\x2c\x45\x10\x01\x46\x74\x5e\x5d\xc2\x0c\0")
        }
        PathEffect::Firebase4 => {
            matches_at(start, 0xa1, b"\xf3\x0f\x2c\x47\x38\x89\x46\x4c\xf3\x0f\x10\x4f\x3c")
                && matches_at(start, 0xe6, b"\xc7\x86\x94\0\0\0\x03\0\0\0")
                && matches_at(update, 0x5e, b"\xe8????\xf3\x0f\x2c\x45\x10\x01\x46\x78\x5e\x5d\xc2\x0c\0")
        }
        _ => return None,
    };
    recognized.then_some(effect)
}

fn indexed_path_records(start: &[u8], storage: u8, stride: u32) -> bool {
    indexed_path_records_by(start, 0x07, storage, stride)
}

fn indexed_path_records_by(start: &[u8], input: u8, storage: u8, stride: u32) -> bool {
    matches_at(start, 0x0a, b"\x83\x7e?\0\x75\x21\x69?????\x50\xe8????\x89\x46?\x69?????")
        && start[0x0c] == storage && start[0x1e] == storage
        && start[0x11] == input && start[0x20] == input + 8
        && word(start, 0x12) == Some(stride) && word(start, 0x21) == Some(stride)
}

fn seh_effect_callbacks<'a>(
    pe: &ParticlePluginPe<'a>, table: u32, storage: u8,
) -> Option<[&'a [u8]; 2]> {
    let [destroy, start, update, finish] = pe.callbacks(table)?;
    (matches_at(destroy, 0x29, b"\xc7\x06????\xc7\x45\xfc\0\0\0\0\x8b\x46?")
        && word(destroy, 0x2b)? == table && destroy[0x38] == storage
        && finish.starts_with(b"\xc2\x04\0")).then_some([start, update])
}

fn simple_path_effect(pe: &ParticlePluginPe<'_>, object_size: u32) -> Option<PathEffect> {
    let (storage, stride) = match object_size {
        0x98 => (0x68, 0x2b8),
        0x80 => (0x54, 0x2a8),
        0x84 => (0x5c, 0x2b8),
        0xa0 => (0x60, 0x2a8),
        0x9c => (0x64, 0x534),
        _ => return None,
    };
    let first_initialized_field = match storage {
        0x60 => 0x64,
        0x64 => 0x68,
        _ => 0x3c,
    };
    let [start, update] = stack_frame_effect_callbacks(pe, first_initialized_field, storage)?;
    if storage == 0x64 {
        return sparkle_path_effect(start, update);
    }
    if !matches_at(start, 0x0f, b"\x83\x7e?\0\x75\x24\x69\x07????\x50\xe8????\x89\x46?\x83\xc4\x04\x69\x0f")
        || start[0x11] != storage
        || start[0x23] != storage
        || word(start, 0x17)? != stride
        || word(start, 0x29)? != stride
    {
        return None;
    }

    if storage == 0x5c {
        // Sigh takes a direction before the scale. Its update applies
        // the difference between the current and initial source position.
        return (matches_at(start, 0x64, b"\xf3\x0f\x2c\x47\x1c\xf3\x0f\x10\x05????\x89\x46\x48\xf3\x0f\x10\x4f\x20")
            && matches_at(update, 0x42, b"\xf3\x0f\x10\x46\x24\xf3\x0f\x5c\x46\x78\xf3\x0f\x11\x46\x60"))
            .then_some(PathEffect::Sigh);
    }
    if storage == 0x60 {
        // Energy reads two additional XY knots into adjacent XYZ records.
        return (matches_at(start, 0x5e, b"\x8b\x47\x18\x89\x46\x68\xc7\x46\x6c\0\0\0\0\x8b\x47\x1c\x89\x46\x70\x8b\x47\x20\x89\x46\x74\xc7\x46\x78\0\0\0\0")
            && matches_at(update, 0x48, b"\xf3\x0f\x10\x46\x24\xf3\x0f\x5c\x86\x94\0\0\0\xf3\x0f\x11\x46\x7c"))
            .then_some(PathEffect::Energy);
    }
    if storage == 0x54 {
        // Both paths share their base allocator. Sweat starts one emitter
        // with a trail pool and branches on its private alpha input.
        return (matches_at(start, 0x39, b"\x8b\x07\x51\xff\x76\x54\x8b\xce\x50\x50\xe8")
            && matches_at(start, 0xa2, b"\xf3\x0f\x10\x47\x10\x0f\x2f\x05????\x76\x09\xc7\x46\x58\0\0\x80\x3f\xeb\x07\xc7\x46\x58\0\0\0\x3f")
            && matches_at(update, 0x42, b"\xf3\x0f\x10\x47\x24\xf3\x0f\x5c\x47\x74\xf3\x0f\x11\x47\x5c"))
            .then_some(PathEffect::Sweat);
    }

    // Note and Oko share the update. Note supplies an explicit partition
    // to the allocator and sets a different divisor in the field it reads.
    if !matches_at(update, 0x65, b"\xff\x50\x08\x33\xd2\xf7\x73\x54\x85\xd2") {
        return None;
    }
    if matches_at(start, 0x39, b"\xf3\x0f\x2c\x47\x14\x51\x8b\xce\xff\x76\x68\x50\xff\x37\xe8")
        && matches_at(start, 0x55, b"\x8b\x47\x18\x89\x46\x3c\x8b\x47\x1c\x89\x46\x40")
        && matches_at(start, 0xb3, b"\xc7\x46\x54\x33\0\0\0")
    {
        Some(PathEffect::Note)
    } else if matches_at(start, 0x39, b"\x51\xff\x76\x68\x51\xff\x37\x8b\xce\xe8")
        && matches_at(start, 0x50, b"\x8b\x47\x14\x89\x46\x3c\x8b\x47\x18\x89\x46\x40")
        && matches_at(start, 0xa6, b"\xc7\x46\x54\x03\0\0\0")
    {
        Some(PathEffect::Oko)
    } else {
        None
    }
}

fn sparkle_path_effect(start: &[u8], update: &[u8]) -> Option<PathEffect> {
    if !matches_at(start, 0x0a, b"\x83\x7e\x64\0\x75\x21\x69\x03\x34\x05\0\0\x50\xe8????\x89\x46\x64\x69\x0b\x34\x05\0\0")
        || !matches_at(update, 0x65, b"\xff\x50\x08\x33\xd2\xf7\x77\x4c\x85\xd2\x75\x06\x8b\x47\x50\x01\x47\x74")
    {
        return None;
    }
    // Both updates consume the same divisor and step. Kirakira sets
    // additive blending and uses two as the divisor, Warukira uses five.
    if matches_at(start, 0xb5, b"\xc7\x46\x4c\x02\0\0\0\xc7\x46\x50\x01\0\0\0\x89\x4e\x54\xc7\x46\x10\x01\0\0\0") {
        Some(PathEffect::Kirakira)
    } else if matches_at(start, 0xb5, b"\xc7\x46\x4c\x05\0\0\0\xc7\x46\x50\x01\0\0\0\x89\x4e\x54\x85\xc0\x0f\x48\xc1") {
        Some(PathEffect::Warukira)
    } else {
        None
    }
}

fn steam_path_effect(pe: &ParticlePluginPe<'_>, object_size: u32) -> Option<PathEffect> {
    let (effect, storage, stride, initial_path_size, update_offset, update_register) =
        match object_size {
            0x328 => (PathEffect::Steam, 0x84, 0x2bc, 0x288, 0x41, 0x77),
            0x354 => (PathEffect::Shy, 0x84, 0x2c4, 0x290, 0x41, 0x77),
            0x334 => (PathEffect::SteamAligned, 0x88, 0x2c4, 0x290, 0x59, 0x76),
            _ => return None,
        };
    let constructor = pe.constructor_call(0x43)?;
    if !matches_at(constructor, 0x68, b"\xc7\x07????\xe8????\xc7\x86????\x14\0\0\0")
        || word(constructor, 0x75)? != initial_path_size
    {
        return None;
    }
    let table = word(constructor, 0x6a)?;
    let [destroy, start, update, finish] = pe.callbacks(table)?;
    if !matches_at(destroy, 0x29, b"\xc7\x06????\xc7\x45\xfc\0\0\0\0\x8b\x86?\0\0\0")
        || word(destroy, 0x2b)? != table
        || destroy[0x38] != storage
        || !matches_at(start, 0x10, b"\x83\xbf?\0\0\0\0\x75\x27\x69\x06????\x50\xe8????\x89\x87?\0\0\0\x83\xc4\x04\x69\x0e")
        || start[0x12] != storage
        || start[0x27] != storage
        || word(start, 0x1b)? != stride
        || word(start, 0x30)? != stride
        || !matches_at(update, update_offset, &[0xff, 0x50, 0x08, 0x33, 0xd2, 0xf7, update_register, 0x78, 0x85, 0xd2])
        || !finish.starts_with(b"\xc2\x04\0")
    {
        return None;
    }
    // Steam can preseed half of the grid. Shy reads the source position
    // instead. Aligned steam consumes the additional blend and motion inputs.
    let recognized = match effect {
        PathEffect::Steam => {
            matches_at(start, 0x419, b"\x89\x87\x80\0\0\0\x0f\x84????\x85\xc0\x0f\x84????\x8b\x47\x38\xbe\0\0\0\0\xd1\xe8\x89\x87\x24\x03\0\0")
                && matches_at(update, 0x192, b"\xf3\x0f\x2c\x81\xb8\x02\0\0\x2b\x71\x04\x88\x42\x0f")
        }
        PathEffect::Shy => {
            matches_at(start, 0x46c, b"\x89\x87\x80\0\0\0\xf3\x0f\x10\x42\x04\xf3\x0f\x59\x05????\xf3\x0f\x11\x87\x48\x03\0\0")
                && matches_at(update, 0x1af, b"\xf3\x0f\x2c\x82\xc0\x02\0\0\x8b\x32\x2b\x72\x04\x88\x41\x0f")
        }
        PathEffect::SteamAligned => {
            matches_at(start, 0x418, b"\xf3\x0f\x2c\x42\x5c\x85\xc0\x74\x03\x89\x77\x10\x83\x7f\x34\0\xf3\x0f\x2c\x42\x60\x89\x87\x84\0\0\0")
                && matches_at(update, 0x2f6, b"\xf3\x0f\x2c\x87\xc0\x02\0\0\x88\x41\x0f")
        }
        _ => return None,
    };
    recognized.then_some(effect)
}

fn stack_frame_effect_callbacks<'a>(
    pe: &ParticlePluginPe<'a>, initialized_field: u8, storage: u8,
) -> Option<[&'a [u8]; 2]> {
    let constructor = pe.constructor_call(0x19)?;
    let field_initializer = if initialized_field < 0x80 {
        matches_at(constructor, 0x2d, &[0xc7, 0x41, initialized_field, 0, 0, 0, 0])
    } else {
        matches_at(constructor, 0x2d, &[0xc7, 0x81, initialized_field, 0, 0, 0, 0, 0, 0, 0])
    };
    if !matches_at(constructor, 0x27, b"\xc7\x01") || !field_initializer {
        return None;
    }
    let table = word(constructor, 0x29)?;
    let [destroy, start, update, finish] = pe.callbacks(table)?;
    (matches_at(destroy, 0, &[0x55, 0x8b, 0xec, 0x56, 0x8b, 0xf1, 0x8b, 0x46, storage, 0xc7, 0x06])
        && word(destroy, 0x0b)? == table
        && finish.starts_with(b"\xc2\x04\0")).then_some([start, update])
}

pub(super) fn sprite_effect(pe: &ParticlePluginPe<'_>) -> Option<SpriteProfile> {
    if !matches!(pe.version(), 0x20001 | 0x20002) || !pe.release()?.starts_with(STACK_FRAME_RELEASE) {
        return None;
    }
    let factory = pe.constructor()?;
    if matches_at(factory, 0x23, SEH_FACTORY) && word(factory, 0x24)? == 0x2d4 {
        return film_noise(pe);
    }
    if !matches_at(factory, 0, STACK_FRAME_FACTORY) {
        return None;
    }
    let size = word(factory, 4)?;
    match size {
        0x88 => {
            let [start, update] = stack_frame_effect_callbacks(pe, 0x40, 0x60)?;
            (matches_at(start, 0x0a, b"\x83\x7e\x60\0\x75\x25\x8b\x03\x8d\x04\xc0\xc1\xe0\x02\x50\xe8????\x89\x46\x60\x8b\x0b\x8d\x0c\xc9\xc1\xe1\x02")
                && matches_at(update, 0x155, b"\xc7\x47\x08\x0a\0\0\0\xe9????\x8b\x47\x10\x83\xec\x14"))
                .then_some(SpriteProfile::Asease)
        }
        0x8c => reaction_offset_effect(pe),
        0x90 => {
            let [start, update] = stack_frame_effect_callbacks(pe, 0x3c, 0x64)?;
            if !matches_at(start, 0x0f, b"\x83\x7e\x64\0\x75\x28\x8b\x07\x8d\x04\x80\xc1\xe0\x03\x50\xe8????\x89\x46\x64\x83\xc4\x04\x8b\x0f\x8d\x0c\x89\xc1\xe1\x03") {
                return None;
            }
            reaction_phase(pe, update, 0x91, 4, 0)?.starts_with(b"\x8b\x37\x83\xec\x14")
                .then_some(SpriteProfile::Exclamation)
        }
        0x94 => double_reaction_effect(pe),
        0x9c => compact_reaction_effect(pe),
        0xa4 => {
            if let Some([start, update]) = stack_frame_effect_callbacks(pe, 0x3c, 0x6c) {
                return (matches_at(start, 0x0a, b"\x83\x7e\x6c\0\x75\x1b\x6b\x07\x34\x50\xe8????\x89\x46\x6c\x6b\x0f\x34")
                    && matches_at(update, 0x39, b"\x7e\x15\xff\x46\x70\x8b\x46\x38\xc7\x46\x78\0\0\0\0"))
                    .then_some(SpriteProfile::Silent);
            }
            let [start, update] = stack_frame_effect_callbacks(pe, 0x80, 0x7c)?;
            if !matches_at(start, 0x0a, b"\x83\x7e\x7c\0\x75\x1f\x8b\x03\xc1\xe0\x05\x50\xe8????\x89\x46\x7c\x8b\x0b\xc1\xe1\x05") {
                return None;
            }
            reaction_phase(pe, update, 0xc0, 4, 0)?.starts_with(b"\x8b\x41\x14\x83\xec")
                .then_some(SpriteProfile::FlowerBurst)
        }
        0xac => {
            let [start, update] = stack_frame_effect_callbacks(pe, 0x3c, 0x74)?;
            (matches_at(start, 0x0a, b"\x83\x7e\x74\0\x75\x21\x69\x07\xb4\x02\0\0\x50\xe8????\x89\x46\x74\x69\x0f\xb4\x02\0\0")
                && matches_at(start, 0x31, b"\xf3\x0f\x2c\x47\x14\x51\x8b\xce\xff\x76\x74\x50")
                && matches_at(update, 0x35, b"\x8b\xc1\xf7\x77\x58\x66\x0f\x5a\xc8\x85\xd2\x75\x03\xff\x47\x78"))
                .then_some(SpriteProfile::Ring)
        }
        0xb0 => {
            let [start, update] = stack_frame_effect_callbacks(pe, 0x3c, 0x78)?;
            (matches_at(start, 0x0a, b"\x83\x7e\x78\0\x75\x1f\x8b\x07\xc1\xe0\x06\x50\xe8????\x89\x46\x78\x8b\x0f\xc1\xe1\x06")
                && matches_at(update, 0x39, b"\x3b\x86\x88\0\0\0\x7e\x18\xff\x46\x7c\x8b\x46\x38\xc7\x86\x84\0\0\0"))
                .then_some(SpriteProfile::Down)
        }
        0xc4 | 0xc8 => digital_reaction_effect(pe, size),
        _ => None,
    }
}

fn film_noise(pe: &ParticlePluginPe<'_>) -> Option<SpriteProfile> {
    let constructor = pe.constructor_call(0x43)?;
    if !matches_at(constructor, 0x65, b"\xc7\x07????\xe8????\xc7\x86\x88\x02\0\0\x14\0\0\0") {
        return None;
    }
    let [start, update] = seh_effect_callbacks(pe, word(constructor, 0x67)?, 0x3c)?;
    // Start establishes three active layers. Update grows them every
    // 30 RNG ticks and dispatches distinct backgrounds and scratches.
    (matches_at(start, 0x0a, b"\x83\x7f\x3c\0\x75\x25\x8b\x03\x8d\x04\x80\xc1\xe0\x02\x50\xe8????\x89\x47\x3c\x8b\x0b\x8d\x0c\x89\xc1\xe1\x02")
        && matches_at(start, 0x4c, b"\xc7\x87\xd0\x02\0\0\x03\0\0\0")
        && matches_at(update, 0x38, b"\xff\x50\x08\x33\xd2\xb9\x1e\0\0\0\xf7\xf1\x85\xd2\x75\x06\xff\x86\xd0\x02\0\0")
        && matches_at(update, 0x73, b"\x83\xf8\x01\x76\x1b\x83\xf8\x02"))
        .then_some(SpriteProfile::FilmNoise)
}

fn reaction_offset_effect(pe: &ParticlePluginPe<'_>) -> Option<SpriteProfile> {
    let [start, update] = stack_frame_effect_callbacks(pe, 0x3c, 0x60)?;
    if !matches_at(start, 0x0f, b"\x83\x7e\x60\0\x75\x1e\x6b\x07\x2c\x50\xe8????\x89\x46\x60\x83\xc4\x04\x6b\x0f\x2c") {
        return None;
    }
    if matches_at(update, 0x264, b"\xc7\x47\x08\x0a\0\0\0\xe9????\x8b\x37\x83\xec\x14\x2b\x77\x04") {
        Some(SpriteProfile::Common)
    } else {
        reaction_phase(pe, update, 0x84, 3, 0)?.starts_with(b"\x8b\x55\x0c\x83\xec")
            .then_some(SpriteProfile::Dis)
    }
}

fn compact_reaction_effect(pe: &ParticlePluginPe<'_>) -> Option<SpriteProfile> {
    if let Some([start, update]) = stack_frame_effect_callbacks(pe, 0x3c, 0x70) {
        if !matches_at(start, 0x0a, b"\x83\x7e\x70\0\x75\x25\x8b\x07\x8d\x04\x40\xc1\xe0\x03\x50\xe8????\x89\x46\x70\x8b\x0f\x8d\x0c\x49\xc1\xe1\x03") {
            return None;
        }
        return reaction_phase(pe, update, 0x93, 4, 0)?.starts_with(b"\x8b\x31\x83\xec\x14")
            .then_some(SpriteProfile::Angry);
    }
    let [start, update] = stack_frame_effect_callbacks(pe, 0x3c, 0x6c)?;
    if matches_at(start, 0x0a, b"\x83\x7e\x6c\0\x75\x1b\x6b\x07\x34\x50\xe8????\x89\x46\x6c\x6b\x0f\x34") {
        return reaction_phase(pe, update, 0x9b, 4, 0)?.starts_with(b"\x8b\x31\x83\xec\x14")
            .then_some(SpriteProfile::Kyupin);
    }
    if !matches_at(start, 0x0a, b"\x83\x7e\x6c\0\x75\x21\x69\x07\xac\x02\0\0\x50\xe8????\x89\x46\x6c\x69\x0f\xac\x02\0\0") {
        return None;
    }
    // Notice ends in phase 5. Surprise has a four-position shake table
    // and enters its permanent hold by assigning phase 10.
    if matches_at(update, 0x197, b"\xc7\x47\x08\x05\0\0\0\x89\x47\x04") {
        reaction_phase(pe, update, 0x7d, 4, 4).map(|_| SpriteProfile::Notice)
    } else if matches_at(update, 0x113, b"\x83\xf9\x03\x77?\xff\x24\x8d????\xf3\x0f\x10\x46\x3c")
        && matches_at(update, 0x188, b"\xc7\x43\x08\x0a\0\0\0")
    {
        let shake = pe.code_at_address(word(update, 0x11b)?)?;
        pe.code_at_address(word(shake, 12)?)?;
        Some(SpriteProfile::Surprise)
    } else {
        None
    }
}

fn digital_reaction_effect(pe: &ParticlePluginPe<'_>, size: u32) -> Option<SpriteProfile> {
    let [start, update] = stack_frame_effect_callbacks(pe, 0x50, 0x3c)?;
    let orbit = size == 0xc8;
    let recognized = if orbit {
        matches_at(start, 0x0a, b"\x83\x7e\x3c\0\x75\x25\x8b\x03\x8d\x04\x40\xc1\xe0\x04\x50\xe8????\x89\x46\x3c\x8b\x0b\x8d\x0c\x49\xc1\xe1\x04")
            && matches_at(update, 0x123, b"\x83\x7f\x08\0\x74?\x66\x0f\x6e\x86\x80\0\0\0\x0f\x5b\xc0\x0f\x2f\x86\x84")
    } else {
        matches_at(start, 0x0a, b"\x83\x7e\x3c\0\x75\x1b\x6b\x03\x2c\x50\xe8????\x89\x46\x3c\x6b\x0b\x2c")
            && matches_at(update, 0x11d, b"\x83\x7f\x08\0\x74?\x66\x0f\x6e\x83\x80\0\0\0\x0f\x5b\xc0\x0f\x2f\x83\x84")
    };
    recognized.then_some(SpriteProfile::Digital { orbit })
}

fn double_reaction_effect(pe: &ParticlePluginPe<'_>) -> Option<SpriteProfile> {
    let [start, update] = stack_frame_effect_callbacks(pe, 0x3c, 0x64)?;
    if !matches_at(start, 0x0f, b"\x83\x7e\x64\0\x75\x34\x8b\x07\x8d\x14\xc5\0\0\0\0\x2b\xd0\xc1\xe2\x02\x52\xe8????\x89\x46\x64") {
        return None;
    }
    if reaction_phase(pe, update, 0x13d, 3, 2).is_some_and(|phase| {
        phase.starts_with(b"\xf3\x0f\x10\x47\x3c\xf3\x0f\x58\x44\x24\x10\xeb")
    }) {
        Some(SpriteProfile::Double { shake_by_index: false, loops: true })
    } else if reaction_phase(pe, update, 0x13f, 3, 2).is_some_and(|phase| {
        phase.starts_with(b"\xf3\x0f\x10\x47\x3c\xf3\x0f\x58\x44\x24\x0c\xf3\x0f\x11\x46\x6c\xf3\x0f\x10\x47\x40\xf3\x0f\x5c\x44\x24\x0c")
    }) {
        Some(SpriteProfile::Double { shake_by_index: true, loops: false })
    } else {
        None
    }
}

fn reaction_phase<'a>(
    pe: &ParticlePluginPe<'a>, update: &[u8], offset: usize, limit: u8, index: usize,
) -> Option<&'a [u8]> {
    if !matches_at(update, offset, b"\x83\xf8?\x0f\x87????\xff\x24\x85????")
        || update[offset + 2] != limit || index > usize::from(limit)
    {
        return None;
    }
    let table = pe.code_at_address(word(update, offset + 12)?)?;
    pe.code_at_address(word(table, index * 4)?)
}

pub(super) fn is_sakura(pe: &ParticlePluginPe<'_>) -> bool {
    (|| {
        if pe.version() != 0x20001
            || !pe.release()?.starts_with(STACK_FRAME_RELEASE)
        {
            return None;
        }
        let constructor = pe.constructor()?;
        if !matches_at(constructor, 0, b"\x55\x8b\xec\x6a\x54\xe8????\x83\xc4\x04\x85\xc0\x74\x31")
            || !matches_at(constructor, 0x33, b"\xc7\0????\xc7\x40\x3c\0\0\0\0")
        {
            return None;
        }
        let table = word(constructor, 0x35)?;
        let [destroy, start, update, finish] = pe.callbacks(table)?;
        // The model's start allocates 0x2b4-byte paths and reads the first
        // partition count. Its update uses 20-tick trajectory samples.
        if !matches_at(destroy, 0, b"\x55\x8b\xec\x56\x8b\xf1\x8b\x46\x3c\xc7\x06")
            || word(destroy, 0x0b)? != table
            || !matches_at(start, 0x0d, b"\x83\x7e\x3c\0\x75\x21\x69\x07\xb4\x02\0\0\x50\xe8????\x89\x46\x3c")
            || !matches_at(start, 0x34, b"\xf3\x0f\x2c\x47\x14\x51\x8b\xce\xff\x76\x3c\x50")
            || !matches_at(update, 0x19, b"\x83\x7f\x34\0\x0f\x84????\x8b\x45\x10\x2b\x45\x0c")
            || !matches_at(update, 0x160, b"\xd8\x35????\x8b\x44\x24\x10\xd8\x40\x78")
            || !finish.starts_with(b"\xc2\x04\0")
        {
            return None;
        }
        let interval = pe.read_only_data(word(update, 0x162)?)?;
        interval.starts_with(&20_f32.to_le_bytes()).then_some(())
    })().is_some()
}

pub(super) fn question_oscillation(pe: &ParticlePluginPe<'_>) -> Option<bool> {
    if !matches!(pe.version(), 0x20001 | 0x20002)
        || !pe.release()?.starts_with(STACK_FRAME_RELEASE)
        || !matches_at(pe.constructor()?, 0,
            b"\x55\x8b\xec\x68\x94\0\0\0\xe8????\x83\xc4\x04\x85\xc0\x74\x0c\xff\x75\x08\x8b\xc8\xe8")
    {
        return None;
    }
    let constructor = pe.constructor_call(0x19)?;
    if !matches_at(constructor, 0x27, b"\xc7\x01????\xc7\x41\x3c\0\0\0\0") {
        return None;
    }
    let table = word(constructor, 0x29)?;
    let [destroy, start, update, finish] = pe.callbacks(table)?;
    if !matches_at(destroy, 0, b"\x55\x8b\xec\x56\x8b\xf1\x8b\x46\x64\xc7\x06")
        || word(destroy, 0x0b)? != table
        || !matches_at(start, 0x0a,
            b"\x83\x7e\x64\0\x75\x25\x8b\x07\x8d\x04\x80\xc1\xe0\x02\x50\xe8????\x89\x46\x64")
        || !matches_at(update, 0x93, b"\x83\xf8\x04\x0f\x87????\xff\x24\x85????")
        || !finish.starts_with(b"\xc2\x04\0")
    {
        return None;
    }
    // The five-state jump table lies inside the update code section.
    let phases = pe.code_at_address(word(update, 0x9f)?)?;
    let phase = |index: usize| pe.code_at_address(word(phases, index * 4)?);
    let hold = phase(3)?;
    if !phase(0)?.starts_with(b"\x8b\x33\x83\xec\x14\x2b\x73\x04")
        || !phase(1)?.starts_with(b"\x8b\x33\x83\xec\x14\x2b\x73\x04")
        || !phase(2)?.starts_with(b"\x8b\x33\x83\xec\x14\x2b\x73\x04")
        || !phase(4)?.starts_with(b"\x66\x0f\x6e\x47\x60")
    {
        return None;
    }
    if matches_at(hold, 0,
        b"\x8b\x13\x8b\xc2\x2b\x43\x04\x3b\x47\x5c\x0f\x8e????\xc7\x43\x08\x04\0\0\0")
    {
        Some(true)
    } else if hold.starts_with(b"\xf3\x0f\x10\x47\x3c\xf3\x0f\x58\x47\x6c\x8b\x45\xf4") {
        Some(false)
    } else {
        None
    }
}

pub(super) fn dark_field(pe: &ParticlePluginPe<'_>) -> Option<DarkField> {
    if !matches!(pe.version(), 0x20001 | 0x20002)
        || !pe.release()?.starts_with(STACK_FRAME_RELEASE)
    {
        return None;
    }
    let factory = pe.constructor()?;
    if pe.version() == 0x20002
        && matches_at(factory, 0x24, b"\x68\xd4\0\0\0\xe8????\x8b\xf0")
    {
        return three_center_field(pe).then_some(DarkField::ThreeCenters);
    }
    if !matches_at(factory, 0,
        b"\x55\x8b\xec\x68\xb4\0\0\0\xe8????\x83\xc4\x04\x85\xc0\x74\x0c\xff\x75\x08\x8b\xc8\xe8")
    {
        return None;
    }
    let constructor = pe.constructor_call(0x19)?;
    if !matches_at(constructor, 0x27, b"\xc7\x01????\xc7\x41\x54\0\0\0\0") {
        return None;
    }
    let table = word(constructor, 0x29)?;
    let [destroy, start, update, finish] = pe.callbacks(table)?;
    if !matches_at(destroy, 0, b"\x55\x8b\xec\x56\x8b\xf1\x8b\x46\x50\xc7\x06")
        || word(destroy, 0x0b)? != table
        || !matches_at(start, 0x0a, b"\x83\x7e\x50\0\x75\x21\x69\x07????\x50\xe8????\x89\x46\x50\x69\x0f")
        || !matches_at(update, 0x54, b"\xac\0\0\0\xf3\x0f\x11\x86\x94\0\0\0\xe8????\xf3\x0f\x2c\x45\x10\x01\x46\x6c")
        || !finish.starts_with(b"\xc2\x04\0")
    {
        return None;
    }
    let stride = word(start, 0x12)?;
    let core = pe.update_call(table, 0x60)?;
    if word(start, 0x21)? != stride
        || !matches_at(core, 0x09, b"\x8b\x43\x70\x3b\x43\x74\x7e\x15\xff\x43\x7c")
    {
        return None;
    }
    // This variant zeroes the horizontal parameter. The others read it
    // from start +0x1c, and the update compares the vertical speed.
    if matches_at(start, 0x48, b"\x8b\x47\x14\x0f\x57\xc9\x89\x46\x54\x8b\x47\x18\x89\x46\x58\xc7\x46\x60\0\0\0\0") {
        return (pe.version() == 0x20001 && stride == 0x2d8
            && matches_at(core, 0x8f, b"\xf3\x0f\x10\x43\x64\x83\xec\x14\xf3\x0f\x58\x87\xac\x02\0\0"))
            .then_some(DarkField::SingleCenter);
    }
    if !matches_at(start, 0x48, b"\x8b\x47\x14\x89\x46\x54\x8b\x47\x18\x89\x46\x58\xf3\x0f\x10\x47\x1c\xf3\x0f\x5e\x05????\xf3\x0f\x11\x46\x60")
        || !matches_at(core, 0x83, b"\xf3\x0f\x10\x43\x64\xf3\x0f\x58\x87????\x8b\x37\xf3\x0f\x10\x0d????\x2b\x77\x04\x0f\x2f\xc8")
    {
        return None;
    }
    let threshold = word(pe.read_only_data(word(core, 0x96)?)?, 0)?;
    match (pe.version(), stride, word(core, 0x8c)?, f32::from_bits(threshold)) {
        (0x20001, 0x2d8, 0x2ac, 740.) => Some(DarkField::Rising),
        (0x20002, 0x2e0, 0x2b4, 800.) => Some(DarkField::Ground),
        _ => None,
    }
}

fn three_center_field(pe: &ParticlePluginPe<'_>) -> bool {
    (|| {
        let constructor = pe.constructor_call(0x56)?;
        if !matches_at(constructor, 0x64, b"\xc7\x06????\xe8????\xc7\x86\x80\0\0\0\0\0\0\0") {
            return None;
        }
        let table = word(constructor, 0x66)?;
        let [destroy, start, update, finish] = pe.callbacks(table)?;
        let core = pe.update_call(table, 0x9d)?;
        // The update chooses one of the three centers by grid index modulo 3.
        (matches_at(destroy, 0x29, b"\xc7\x06????\xc7\x45\xfc\0\0\0\0\x8b\x46\x58")
            && word(destroy, 0x2b)? == table
            && matches_at(start, 0x0a, b"\x83\x7e\x58\0\x75\x2d\x8b\x07\x33\xc9\xba\xdc\x02\0\0\xf7\xe2")
            && matches_at(start, 0x28, b"\x89\x46\x58\x69\x0f\xdc\x02\0\0")
            && matches_at(start, 0x63, b"\x8b\x47\x1c\x89\x46\x68\x8b\x47\x20\x89\x46\x6c\x8b\x47\x24\x89\x46\x74\x8b\x47\x28\x89\x46\x78")
            && matches_at(update, 0x96, b"\x8d\x45\x10\x50\x51\x8b\xce\xe8????\xf3\x0f\x2c\x45\x10")
            && matches_at(core, 0x6d, b"\xb8\x56\x55\x55\x55\xf7\xea")
            && finish.starts_with(b"\xc2\x04\0")).then_some(())
    })().is_some()
}

fn direct_layout(pe: &ParticlePluginPe<'_>) -> Option<Layout> {
    let constructor = pe.constructor()?;
    if !pe.release()?.starts_with(DIRECT_RELEASE) || !matches_at(constructor, 0, CONSTRUCTOR) {
        return None;
    }
    let (layout, storage_offset, table_offset) = match (pe.version(), constructor[1]) {
        (0x20000 | 0x20001, 0x38) => (Layout::Rain, 0x34, 0x1f),
        (0x20001, 0x60) => (Layout::AtlasRain, 0x58, 0x25),
        (0x20001, 0x6c) => (Layout::ScaledRain, 0x64, 0x25),
        (0x20001, 0x40) => (Layout::Burst, 0x3c, 0x25),
        _ => return None,
    };
    if !matches_at(constructor, table_offset - 2, b"\xc7\0????")
        || !(if layout == Layout::Rain {
            matches_at(constructor, table_offset + 4, b"\x89\x48\x34\xc3")
        } else {
            matches_at(constructor, 0x1d, b"\x89\x48\x34\x89\x48\x38")
        })
    {
        return None;
    }
    let table = constructor.get(table_offset..table_offset + 4)?.try_into().ok()?;
    let [destroy, start, update, finish] = pe.callbacks(u32::from_le_bytes(table))?;
    if !matches_at(destroy, 0, &[0x56, 0x8b, 0xf1, 0x8b, 0x46, storage_offset, 0xc7, 0x06])
        || destroy.get(8..12)? != table
        || !finish.starts_with(b"\xc2\x04\0")
    {
        return None;
    }
    let recognized = match layout {
        // These allocations describe 12-, 44-, and 28-byte private records.
        // Verify the model's stride and its update's distinct consumer.
        Layout::Rain => matches_at(start, 0x0e,
            b"\x39\x6f\x34\x75\x1d\x8b\x06\x8d\x04\x40\x03\xc0\x03\xc0")
            && matches_at(update, 0xb9,
                b"\xd9\x5c\x24\x08\xd9\x45\x08\xd9\x5c\x24\x04\xd9\x45\x04\xd9\x1c\x24\xe8????")
            && matches_at(update, 0x140, b"\xff\xdd\xd8\x5b\x5e\x5d\x5f\x83\xc4\x10\xc2\x0c\0"),
        Layout::AtlasRain => matches_at(start, 0x13,
            b"\x75\x22\x8b\x07\x6b\xc0\x2c\x50\xe8????\x89\x46\x58\x8b\x0f\x6b\xc9\x2c")
            && matches_at(update, 0xaf, b"\x8b\x44\x24\x1c\xd9\x1c\x24\x53\xe8????")
            && matches_at(update, 0x18c, b"\x8b\x4c\x24\x1c\x89\x56\x1c\x8b\x40\x08"),
        Layout::ScaledRain => matches_at(start, 0x13,
            b"\x75\x22\x8b\x03\x6b\xc0\x2c\x50\xe8????\x89\x47\x64\x8b\x0b\x6b\xc9\x2c")
            && matches_at(update, 0xaf, b"\xe8????\xe9????")
            && matches_at(update, 0x184, b"\x8b\x44\x24\x1c\x89\x4e\x1c\x8b\x53\x08"),
        Layout::Burst => matches_at(start, 0x0e,
            b"\x8b\x07\x8d\x0c\xc5\0\0\0\0\x2b\xc8\x03\xc9\x03\xc9\x51")
            && matches_at(update, 0x3d, b"\x24\x18\x55\x57\xe8????\xdd\x05")
            && matches_at(update, 0x229, b"\x0f\x82????\x5f\xdd\xd8\x5d\x5e\x5b\x59\xc2\x0c\0"),
        Layout::Common => unreachable!("common layout has a separate constructor"),
    };
    recognized.then_some(layout)
}

fn common_layout(pe: &ParticlePluginPe<'_>) -> Option<Layout> {
    if pe.version() != 0x20002
        || !pe.release()?.starts_with(STACK_FRAME_RELEASE)
    {
        return None;
    }
    let factory = pe.constructor()?;
    let (constructor_call, table_offset, destination, enabled_offset) = if matches_at(factory, 0x23,
        b"\x68\x98\x01\0\0\xe8????\x83\xc4\x04\x89\x45\xf0\xc7\x45\xfc\0\0\0\0\x85\xc0\x74\x19\xff\x75\x08\x8b\xc8\xe8")
    {
        (0x43, 0x54, 0x06, 0x50)
    } else if matches_at(factory, 0,
        b"\x55\x8b\xec\x68\x90\x01\0\0\xe8????\x83\xc4\x04\x85\xc0\x74\x0c\xff\x75\x08\x8b\xc8\xe8")
    {
        (0x19, 0x29, 0x01, 0x48)
    } else {
        return None;
    };
    let constructor = pe.constructor_call(constructor_call)?;
    if !matches_at(constructor, table_offset - 2, &[0xc7, destination])
        || !matches_at(constructor, table_offset + 4, &[0xc7, destination | 0x80, 0x04, 0x01, 0, 0, 0, 0, 0, 0])
    {
        return None;
    }
    let [_, start, update, finish] = pe.callbacks(word(constructor, table_offset)?)?;
    // Start formats the real scalar configuration name into a 128-byte buffer.
    let config = pe.read_only_data(word(start, 0xd7)?)?;
    if !matches_at(start, 0xd4, b"\x56\x57\x68????\x68\x80\0\0\0\x50\xe8")
        || !config.starts_with(b"ptcl%d%03d.plist\0")
        || !matches_at(update, 0x14, b"\x83\x7e\x34\0\x0f\x84????\x83\xbe")
        || !matches_at(update, 0x20, &[enabled_offset, 0x01, 0, 0, 0, 0x0f, 0x84])
        || !matches_at(update, 0x2b, b"\x8b\x45\x10\x2b\x45\x0c\x66\x0f\x6e\xc0\xf3\x0f\xe6\xc0")
        || !finish.starts_with(b"\xc2\x04\0")
    {
        return None;
    }
    Some(Layout::Common)
}

fn word(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?))
}

fn matches_at(body: &[u8], offset: usize, pattern: &[u8]) -> bool {
    body.get(offset..offset + pattern.len()).is_some_and(|bytes| {
        bytes.iter().zip(pattern).all(|(&byte, &expected)| expected == b'?' || byte == expected)
    })
}
