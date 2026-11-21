//! CST scene loading/localization and history variable snapshots.
//! Original binaries are used only as static byte evidence, never executed.
use std::borrow::Cow;
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use cs2_formats::{ConfigXml, CstlTable};
use cs2_runtime::{ResourceIndex, ResourceLocation};
use cs2_runtime::resource_router::ResourceMode;
use cs2_script::{CstDocument, NativeReturn, Result, ScriptError, Vm};
use cs2_script::cst_history::{HistorySnapshot, history_blocks, prepend_history_snapshot};

use super::{LogicEvent, LoggingHost, ScriptTextEncoding, native_state_error, native_state_error_owned, read_u32, trim_nul,
    expand_kcs_format, signed_cst_index, write_kcs_terminated};
use super::resource_preload::SourcedResource;
use super::particle_plugin_digest::sha256;
use super::{HostPhase, measure_host_operation, measure_host_phase};

#[path = "happiness_route_data.rs"]
pub(crate) mod happiness_route_data;
#[path = "newton_route_data.rs"]
pub(crate) mod newton_route_data;

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;
pub(super) const NEWTON_PE: &str = "3b359fc2d737d5ceed454d6b65e6e5b5e6dfb4a24cf88ee361df86e454203f18";
pub(super) const NEWTON_LOCALIZED_PE: &str = "6f75ae3877e018de1982a68f1089c127870d895d748c6290dd2ee85cd8daa4de";
const NEKOPARA_PE_SHA256: &str = "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc";
const MAX_BODY_SOURCES: usize = 64;
const MAX_SOURCE_OBSERVATIONS: usize = 32;
static NEXT_BODY_LOAD: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CstBodySource {
    pub(super) context_handle: u32,
    pub(super) load_id: u64,
    pub(super) group: i32,
    pub(super) absolute_record: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CstSourceProbe {
    Eye08,
    Eden,
    Kanmi,
    KanmiSc02,
    KanmiSc03,
    Djibril10,
    August,
    Kamikaze,
    Yukikoi,
    Kokonatsu,
    KokonatsuC02,
    Grisaia14,
    Campanella,
    CampanellaLocalized,
    Rebellions,
    RebellionsSecond,
    RebellionsThird,
    RebellionsLater,
    Hatsumira12,
    Hatsumira12PostOp,
    Happiness02,
    HappinessSecond,
    HappinessThird,
    Utate05,
    Newton16,
    Newton16B,
    Sudden19,
}

impl CstSourceProbe {
    fn happiness_scene(self) -> Option<&'static str> {
        match self {
            Self::Happiness02 => Some("0_01_01"),
            Self::HappinessSecond => Some("0_01_02"),
            Self::HappinessThird => Some("0_01_03"),
            _ => None,
        }
    }

    pub(super) fn legacy_continuation_page(self, group: i32) -> Option<(i32, &'static [usize])> {
        if self == Self::Newton16B {
            return newton_route_data::SECOND.get(usize::try_from(group).ok()?).copied();
        }
        let happiness_pages = match self {
            Self::Happiness02 if group >= 3 => Some(happiness_route_data::FIRST),
            Self::HappinessSecond => Some(happiness_route_data::SECOND),
            Self::HappinessThird => Some(happiness_route_data::THIRD),
            _ => None,
        };
        if let Some(pages) = happiness_pages {
            return usize::try_from(group).ok().and_then(|group| pages.get(group)).copied();
        }
        match (self, group) {
            (Self::Yukikoi, 2) => Some((70, &[69])),
            (Self::Yukikoi, 3) => Some((73, &[72])),
            (Self::Yukikoi, 4) => Some((85, &[84])),
            (Self::Yukikoi, 5) => Some((87, &[86])),
            (Self::Yukikoi, 6) => Some((99, &[98])),
            (Self::Yukikoi, 7) => Some((102, &[101])),
            (Self::Yukikoi, 8) => Some((104, &[103])),
            (Self::Yukikoi, 9) => Some((107, &[106])),
            (Self::Yukikoi, 10) => Some((123, &[118])),
            (Self::Yukikoi, 11) => Some((126, &[125])),
            (Self::Yukikoi, 12) => Some((128, &[127])),
            (Self::Yukikoi, 13) => Some((130, &[129])),
            (Self::Yukikoi, 14) => Some((132, &[131])),
            (Self::Yukikoi, 15) => Some((135, &[134])),
            (Self::Yukikoi, 16) => Some((157, &[146])),
            (Self::Yukikoi, 17) => Some((169, &[168])),
            (Self::Yukikoi, 18) => Some((172, &[171])),
            (Self::Yukikoi, 19) => Some((184, &[183])),
            (Self::Yukikoi, 20) => Some((187, &[186])),
            (Self::Yukikoi, 21) => Some((199, &[198])),
            (Self::Yukikoi, 22) => Some((202, &[201])),
            (Self::Yukikoi, 23) => Some((219, &[213])),
            (Self::Utate05, 0) => Some((49, &[48])),
            (Self::Utate05, 1) => Some((54, &[53])),
            (Self::Utate05, 2) => Some((58, &[57])),
            (Self::Utate05, 3) => Some((62, &[61])),
            (Self::Utate05, 4) => Some((66, &[65])),
            (Self::Utate05, 5) => Some((94, &[73])),
            (Self::Utate05, 6) => Some((98, &[97])),
            (Self::Utate05, 7) => Some((107, &[103])),
            (Self::Utate05, 8) => Some((110, &[109])),
            (Self::Utate05, 9) => Some((118, &[113])),
            (Self::Utate05, 10) => Some((121, &[120])),
            (Self::Utate05, 11) => Some((140, &[136])),
            (Self::Utate05, 12) => Some((161, &[152])),
            (Self::Utate05, 13) => Some((164, &[163])),
            (Self::Utate05, 14) => Some((168, &[167])),
            (Self::Utate05, 15) => Some((171, &[170])),
            (Self::Utate05, 16) => Some((177, &[176])),
            (Self::Utate05, 17) => Some((180, &[179])),
            (Self::Utate05, 18) => Some((189, &[185])),
            (Self::Utate05, 19) => Some((192, &[191])),
            (Self::Utate05, 20) => Some((195, &[194])),
            (Self::Utate05, 21) => Some((218, &[210])),
            (Self::Utate05, 22) => Some((220, &[219])),
            (Self::Utate05, 23) => Some((222, &[221])),
            (Self::Kokonatsu, 3) => Some((112, &[111])),
            (Self::Kokonatsu, 4) => Some((118, &[113])),
            (Self::Kokonatsu, 5) => Some((127, &[122])),
            (Self::Kokonatsu, 6) => Some((132, &[131])),
            (Self::Kokonatsu, 7) => Some((135, &[134])),
            (Self::Kokonatsu, 8) => Some((137, &[136])),
            (Self::Kokonatsu, 9) => Some((140, &[139])),
            (Self::Newton16, 3) => Some((40, &[39])),
            (Self::Newton16, 4) => Some((42, &[41])),
            (Self::Newton16, 5) => Some((44, &[43])),
            (Self::Newton16, 6) => Some((47, &[46])),
            (Self::Newton16, 7) => Some((49, &[48])),
            (Self::Newton16, 8) => Some((54, &[51, 52, 53])),
            (Self::Newton16, 9) => Some((57, &[55, 56])),
            (Self::Sudden19, 3) => Some((14, &[13])),
            (Self::Sudden19, 4) => Some((30, &[21])),
            (Self::Sudden19, 5) => Some((42, &[31])),
            (Self::Sudden19, 6) => Some((50, &[49])),
            (Self::Sudden19, 7) => Some((52, &[51])),
            (Self::Sudden19, 8) => Some((55, &[54])),
            (Self::Sudden19, 9) => Some((58, &[57])),
            (Self::Kokonatsu, 10) => Some((147, &[141])),
            (Self::Kamikaze, 3) => Some((74, &[73])),
            (Self::Kamikaze, 4) => Some((84, &[83])),
            (Self::Kamikaze, 5) => Some((90, &[89])),
            (Self::Kamikaze, 6) => Some((93, &[92])),
            (Self::Kamikaze, 7) => Some((110, &[109])),
            (Self::Kamikaze, 8) => Some((154, &[146])),
            (Self::Kamikaze, 9) => Some((156, &[155])),
            (Self::Kamikaze, 10) => Some((160, &[159])),
            (Self::Kamikaze, 11) => Some((183, &[165])),
            (Self::Kamikaze, 12) => Some((186, &[185])),
            (Self::Kamikaze, 13) => Some((190, &[189])),
            (Self::Newton16, 10) => Some((62, &[61])),
            (Self::Newton16, 11) => Some((66, &[65])),
            (Self::Newton16, 12) => Some((71, &[70])),
            (Self::Newton16, 13) => Some((77, &[76])),
            (Self::Newton16, 14) => Some((81, &[80])),
            (Self::Newton16, 15) => Some((84, &[83])),
            (Self::Newton16, 16) => Some((86, &[85])),
            (Self::Newton16, 17) => Some((91, &[90])),
            (Self::Newton16, 18) => Some((93, &[92])),
            (Self::Newton16, 19) => Some((96, &[95])),
            (Self::Newton16, 20) => Some((99, &[98])),
            (Self::Newton16, 21) => Some((101, &[100])),
            (Self::Newton16, 22) => Some((108, &[107])),
            (Self::Newton16, 23) => Some((114, &[113])),
            (Self::Newton16, 24) => Some((120, &[119])),
            (Self::Newton16, 25) => Some((124, &[123])),
            (Self::Newton16, 26) => Some((127, &[125, 126])),
            (Self::Newton16, 27) => Some((131, &[129, 130])),
            (Self::Newton16, 28) => Some((135, &[134])),
            (Self::Newton16, 29) => Some((138, &[137])),
            (Self::Newton16, 30) => Some((144, &[143])),
            (Self::Newton16, 31) => Some((147, &[146])),
            (Self::Newton16, 32) => Some((153, &[152])),
            (Self::Newton16, 33) => Some((155, &[154])),
            (Self::Newton16, 34) => Some((158, &[157])),
            (Self::Newton16, 35) => Some((165, &[164])),
            (Self::Newton16, 36) => Some((168, &[167])),
            (Self::Newton16, 37) => Some((177, &[176])),
            (Self::Newton16, 38) => Some((180, &[179])),
            (Self::Newton16, 39) => Some((187, &[185, 186])),
            (Self::Newton16, 40) => Some((190, &[189])),
            (Self::Newton16, 41) => Some((199, &[198])),
            (Self::Newton16, 42) => Some((204, &[202, 203])),
            (Self::Newton16, 43) => Some((208, &[207])),
            (Self::Newton16, 44) => Some((215, &[213, 214])),
            (Self::Newton16, 45) => Some((218, &[217])),
            (Self::Newton16, 46) => Some((226, &[223, 224, 225])),
            (Self::Newton16, 47) => Some((229, &[228])),
            (Self::Newton16, 48) => Some((231, &[230])),
            (Self::Newton16, 49) => Some((233, &[232])),
            (Self::Newton16, 50) => Some((235, &[234])),
            (Self::Newton16, 51) => Some((245, &[242, 243, 244])),
            (Self::Newton16, 52) => Some((247, &[246])),
            (Self::Newton16, 53) => Some((249, &[248])),
            (Self::Newton16, 54) => Some((251, &[250])),
            (Self::Newton16, 55) => Some((256, &[255])),
            (Self::Newton16, 56) => Some((258, &[257])),
            (Self::Sudden19, 10) => Some((67, &[60])),
            (Self::Sudden19, 11) => Some((74, &[68])),
            (Self::Sudden19, 12) => Some((76, &[75])),
            (Self::Sudden19, 13) => Some((78, &[77])),
            (Self::Sudden19, 14) => Some((80, &[79])),
            (Self::Sudden19, 15) => Some((82, &[81])),
            (Self::Sudden19, 16) => Some((91, &[90])),
            (Self::Sudden19, 17) => Some((95, &[94])),
            (Self::Sudden19, 18) => Some((98, &[97])),
            (Self::Sudden19, 19) => Some((110, &[109])),
            (Self::Sudden19, 20) => Some((117, &[116])),
            (Self::Sudden19, 21) => Some((120, &[119])),
            (Self::Sudden19, 22) => Some((127, &[126])),
            (Self::Sudden19, 23) => Some((130, &[129])),
            (Self::Sudden19, 24) => Some((145, &[136])),
            (Self::Sudden19, 25) => Some((152, &[147])),
            (Self::Sudden19, 26) => Some((171, &[154])),
            (Self::Sudden19, 27) => Some((179, &[178])),
            (Self::Sudden19, 28) => Some((182, &[181])),
            (Self::Sudden19, 29) => Some((184, &[183])),
            (Self::Sudden19, 30) => Some((186, &[185])),
            (Self::Sudden19, 31) => Some((188, &[187])),
            (Self::Sudden19, 32) => Some((190, &[189])),
            (Self::Sudden19, 33) => Some((193, &[192])),
            (Self::Sudden19, 34) => Some((200, &[199])),
            (Self::Sudden19, 35) => Some((203, &[202])),
            (Self::KokonatsuC02, 0) => Some((47, &[46])),
            (Self::KokonatsuC02, 1) => Some((49, &[48])),
            (Self::KokonatsuC02, 2) => Some((51, &[50])),
            (Self::KokonatsuC02, 3) => Some((53, &[52])),
            (Self::KokonatsuC02, 4) => Some((55, &[54])),
            _ => None,
        }
    }

    fn accepts(self, native: u32, group: i32, record: usize, kind: u8) -> bool {
        if let Some((_, body)) = self.legacy_continuation_page(group) {
            return kind == 0x20 && body.contains(&record) && match self {
                Self::Kokonatsu | Self::KokonatsuC02 | Self::Kamikaze | Self::Utate05 => native == 327,
                Self::Yukikoi => matches!(native, 327 | 1033 | 1037 | 1038),
                _ => true,
            };
        }
        if self == Self::Djibril10 {
            return native == 327 && kind == 0x20 && matches!((group, record), (0, 3) | (1, 13 | 14));
        }
        if self == Self::Eye08 {
            return native == 327 && kind == 0x20
                && (matches!((group, record),
                    (0, 143) | (1, 147) | (2, 149) | (3, 165) | (4, 169) | (5, 173) | (6, 179))
                    || ((7..=27).contains(&group) && record == 183 + 4 * (group - 7) as usize));
        }
        matches!((self, group, record, kind),
            (Self::Eden, 4, 101, 0x21) | (Self::Eden, 4, 102, 0x20)
            | (Self::Eden, 5, 106, 0x20)
            | (Self::Eden, 6, 108, 0x20) | (Self::Eden, 7, 110, 0x20)
            | (Self::Eden, 8, 112, 0x20) | (Self::Eden, 9, 114, 0x20)
            | (Self::Eden, 10, 118, 0x20) | (Self::Eden, 11, 120, 0x20)
            | (Self::Eden, 12, 138, 0x20)
            | (Self::Eden, 13, 150, 0x20) | (Self::Eden, 14, 162, 0x20)
            | (Self::Eden, 15, 173, 0x20) | (Self::Eden, 16, 185, 0x20)
            | (Self::Eden, 17, 187, 0x20)
            | (Self::Kamikaze, 0, 56, 0x20) | (Self::Kamikaze, 1, 61, 0x20)
            | (Self::Kamikaze, 2, 67, 0x20))
            || native == 327 && kind == 0x20
                && match self {
                    Self::Kanmi => Some("sc01"),
                    Self::KanmiSc02 => Some("sc02"),
                    Self::KanmiSc03 => Some("sc03"),
                    _ => None,
                }.is_some_and(|scene| {
                    let wait = usize::try_from(group).ok()
                        .and_then(|group| crate::HeadlessPlayer::host_kanmi_scene_waits(scene).get(group));
                    wait.and_then(|wait| crate::HeadlessPlayer::host_kanmi_body_record(scene, group, *wait))
                        == Some(record)
                    || (scene, group, record) == ("sc02", 101, 555)
                })
            || self == Self::Yukikoi && matches!(native, 327 | 1033 | 1037 | 1038)
                && matches!((group, record, kind), (0, 53, 0x20) | (1, 66, 0x20))
            || self == Self::Kokonatsu && native == 327
                && matches!((group, record, kind), (0, 61, 0x20) | (1, 98, 0x20) | (2, 109, 0x20))
            || self == Self::Grisaia14 && native == 327
                && matches!((group, record, kind),
                    (0, 5, 0x20) | (1, 7, 0x20) | (2, 77, 0x20) | (3, 79, 0x20)
                    | (4, 81, 0x20) | (5, 114, 0x20)
                    | (6, 123, 0x20) | (7, 125, 0x20) | (8, 129, 0x20))
            || matches!(self, Self::Campanella | Self::CampanellaLocalized)
                && native == 327 && kind == 0x20
                && crate::story_trace::story_trace_18::page_records(self == Self::CampanellaLocalized, group)
                    .is_some_and(|(_, records)| records.contains(&record))
            || matches!(self, Self::RebellionsThird | Self::RebellionsLater)
                && native == 327 && kind == 0x20
            || matches!(self, Self::Rebellions | Self::RebellionsSecond)
                && native == 327 && kind == 0x20
                && usize::try_from(group).ok()
                    .and_then(|group| {
                        let scene = match self {
                            Self::Rebellions => "A_01_01",
                            Self::RebellionsSecond => "A_01_02",
                            _ => return None,
                        };
                        crate::HeadlessPlayer::host_rebellions_scene_waits(scene).get(group)
                    })
                    .is_some_and(|&wait| record + 1 == wait as usize)
            || self == Self::Hatsumira12 && native == 327 && kind == 0x20
                && crate::HeadlessPlayer::host_hatsumira_12_op001_records(group)
                    .is_some_and(|(_, body)| record == body)
            || self == Self::Hatsumira12PostOp && native == 327 && kind == 0x20
                && crate::HeadlessPlayer::host_hatsumira_12_post_op_records(group)
                    .is_some_and(|(_, body)| record == body)
            || kind == 0x20 && matches!((self, group, record),
                (Self::Happiness02, 0, 100) | (Self::Happiness02, 1, 103 | 104 | 122)
                | (Self::Happiness02, 2, 124)
                | (Self::Newton16, 0, 15) | (Self::Newton16, 1, 23)
                | (Self::Newton16, 2, 25 | 26) | (Self::Newton16, 3, 39)
                | (Self::Sudden19, 0, 7) | (Self::Sudden19, 1, 9) | (Self::Sudden19, 2, 11))
            || self == Self::August && native == 1037
                && matches!((group, record, kind), (0, 78, 0x21) | (0, 79 | 80, 0x20)
                    | (1, 83, 0x21) | (1, 84 | 85, 0x20)
                    | (2, 88, 0x21) | (2, 89, 0x20) | (3, 93, 0x20) | (4, 96, 0x20))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CstSourceRecordObservation {
    pub(super) source: CstBodySource,
    pub(super) frame: u64,
    pub(super) probe: CstSourceProbe,
    pub(super) native: u32,
    pub(super) kind: u8,
    pub(super) stored_codec: ScriptTextEncoding,
    pub(super) output_codec: ScriptTextEncoding,
    pub(super) raw_bytes: usize,
    pub(super) output_bytes: usize,
    pub(super) body_tagged: bool,
    pub(super) localized: bool,
}

struct CstSourceProbeLoad {
    probe: CstSourceProbe,
    load_id: u64,
    records: Vec<CstSourceRecordObservation>,
}

pub(super) fn join_body_sources(prefix: &[CstBodySource], suffix: &[CstBodySource]) -> Option<Vec<CstBodySource>> {
    if prefix.len() + suffix.len() > MAX_BODY_SOURCES { return None; }
    if let (Some(previous), Some(next)) = (prefix.last(), suffix.first())
        && (previous.context_handle != next.context_handle || previous.load_id != next.load_id
            || previous.group != next.group || previous.absolute_record >= next.absolute_record)
    {
        return None;
    }
    Some(prefix.iter().chain(suffix).copied().collect())
}

pub(super) fn body_control_only(mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        if let Some(rest) = bytes.strip_prefix(b"\\n").or_else(|| bytes.strip_prefix(b"\\r")) {
            bytes = rest;
        } else if let Some(rest) = bytes.strip_prefix(b"\\p") {
            bytes = if matches!(rest.first(), Some(b'c' | b'l' | b'r')) { &rest[1..] } else { rest };
        } else {
            return false;
        }
    }
    true
}

#[derive(Clone)]
struct CstByteOrigin {
    bytes: Vec<u8>,
    codec: ScriptTextEncoding,
    body: Vec<CstBodySource>,
}

#[derive(Default)]
pub(super) struct CstTextOrigins {
    returned: VecDeque<((u32, u32), CstByteOrigin)>,
    shared: BTreeMap<u32, CstByteOrigin>,
    body_loads: BTreeMap<u32, u64>,
    source_probes: BTreeMap<u32, CstSourceProbeLoad>,
    neko_scene_loads: BTreeMap<u32, NekoSceneLoad>,
    happiness_cst_calls: BTreeMap<u32, HappinessCstCall>,
}

struct HappinessCstCall {
    caller: u32,
    frame: u64,
    loaded_at: u64,
    outer: CstSourceProbeLoad,
}

#[derive(Clone, Copy)]
struct NekoSceneLoad {
    member: &'static str,
    id: u64,
    loaded_at: u64,
    helper: Option<(u64, NekoHelper)>,
}

fn neko_cst_resource(archive: &str, name: &str) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    include_str!("neko_chapter_resources.txt").lines().find_map(|line| {
        let mut fields = line.split_ascii_whitespace();
        let actual_archive = fields.next()?;
        let member = fields.next()?;
        if !archive.eq_ignore_ascii_case(actual_archive) || !name.eq_ignore_ascii_case(member) {
            return None;
        }
        Some((actual_archive, member, fields.next()?, fields.next()?))
    })
}

#[derive(Clone, Copy)]
enum NekoHelper {
    GetAttM2,
    GetAtt,
    ReturnedToGetAttM2,
}

impl NekoSceneLoad {
    fn resume(self, member: &str, pc: Option<usize>, frame: u64) -> Option<(u64, u64)> {
        if member != self.member {
            return None;
        }
        match (self.helper, pc) {
            (None, Some(0x14464 | 0x14e91 | 0x14f97 | 0x15178)) => Some((self.id, self.loaded_at)),
            (Some((entered, NekoHelper::ReturnedToGetAttM2)), Some(0x14f97 | 0x15178))
            | (Some((entered, NekoHelper::GetAttM2)), Some(0x15178))
                if entered == frame => Some((self.id, self.loaded_at)),
            _ => None,
        }
    }

    fn helper_step(self, pc: Option<usize>, frame: u64) -> Option<(Self, &'static str, &'static str)> {
        let stage = match (self.helper, pc) {
            (None, Some(0x14e91)) => NekoHelper::GetAttM2,
            (Some((entered, NekoHelper::GetAttM2)), Some(0x14e91)) if entered == frame => NekoHelper::GetAtt,
            (Some((entered, NekoHelper::GetAtt)), Some(0x14f97)) if entered == frame => NekoHelper::ReturnedToGetAttM2,
            _ => return None,
        };
        let (name, digest) = match stage {
            NekoHelper::GetAttM2 | NekoHelper::ReturnedToGetAttM2 => (
                "sub_getattm2.cst",
                "d628630ac899d079b8c1aadadf04bfdfb28aacac7b1004e0da91c2121a91a725",
            ),
            NekoHelper::GetAtt => (
                "sub_getatt.cst",
                "aee228f0ddc6d71ec21c22cabd9aa5a1f2110f80731beeac06676210db36995f",
            ),
        };
        Some((Self { helper: Some((frame, stage)), ..self }, name, digest))
    }
}

impl CstTextOrigins {
    pub(super) fn eye08_load(&self, handle: u32) -> Option<u64> {
        let load = self.source_probes.get(&handle)?;
        (load.probe == CstSourceProbe::Eye08 && self.body_loads.get(&handle) == Some(&load.load_id))
            .then_some(load.load_id)
    }

    /// CSS restores replace shared bytes. Future reads must still be able to
    /// tag text from the CST contexts that were loaded before the restore.
    pub(super) fn clear_after_shared_strings_restore(&mut self) {
        self.returned.clear();
        self.shared.clear();
        self.happiness_cst_calls.clear();
        for load in self.source_probes.values_mut() {
            load.records.clear();
        }
    }

    pub(super) fn source_record_observations(&self, handle: u32) -> &[CstSourceRecordObservation] {
        self.source_probes.get(&handle)
            .filter(|load| self.body_loads.get(&handle) == Some(&load.load_id))
            .map_or(&[], |load| load.records.as_slice())
    }

    fn note_source_record(&mut self, observation: CstSourceRecordObservation) -> bool {
        let source = observation.source;
        let Some(load) = self.source_probes.get_mut(&source.context_handle) else { return false; };
        if load.load_id != source.load_id || load.probe != observation.probe
            || self.body_loads.get(&source.context_handle) != Some(&source.load_id)
            || !load.probe.accepts(observation.native, source.group, source.absolute_record, observation.kind)
        {
            return false;
        }
        // Keep the earliest matching read so a later wait-marker lookup cannot
        // overwrite the provenance of a body that was already queued.
        if load.records.iter().any(|entry| {
            *entry == CstSourceRecordObservation { frame: entry.frame, ..observation }
        }) {
            return false;
        }
        // Happiness prefetches raw records before requesting their localized variants.
        let capacity = if load.probe.happiness_scene().is_some() {
            MAX_SOURCE_OBSERVATIONS * 2
        } else {
            MAX_SOURCE_OBSERVATIONS
        };
        if load.records.len() == capacity {
            load.records.remove(0);
        }
        load.records.push(observation);
        true
    }

    /// Refresh native 278's encoding. Only a proven no-op preserves the input body tag.
    pub(super) fn record(&mut self, task: u32, address: u32, bytes: &[u8], codec: ScriptTextEncoding) {
        let body = self.returned_origin(task, address, bytes)
            .filter(|origin| origin.codec == codec && self.body_is_current(&origin.body)
                && !bytes.windows(4).any(|marker| marker == b"$str"))
            .map_or_else(Vec::new, |origin| origin.body.clone());
        self.record_with_body(task, address, bytes, codec, body);
    }

    fn record_with_body(&mut self, task: u32, address: u32, bytes: &[u8], codec: ScriptTextEncoding,
        body: Vec<CstBodySource>,
    ) {
        let key = (task, address);
        self.returned.retain(|(old, ..)| *old != key);
        if self.returned.len() == 64 {
            self.returned.pop_front();
        }
        self.returned.push_back((key, CstByteOrigin { bytes: bytes.to_vec(), codec, body }));
    }

    pub(super) fn invalidate_returned(&mut self, task: u32, address: u32) {
        self.returned.retain(|(key, ..)| *key != (task, address));
    }

    fn returned_origin(&self, task: u32, address: u32, bytes: &[u8]) -> Option<&CstByteOrigin> {
        self.returned.iter().find_map(|(key, origin)| {
            (*key == (task, address) && origin.bytes == bytes).then_some(origin)
        })
    }

    pub(super) fn returned(&self, task: u32, address: u32, bytes: &[u8]) -> Option<ScriptTextEncoding> {
        self.returned_origin(task, address, bytes).map(|origin| origin.codec)
    }

    pub(super) fn returned_body(&self, task: u32, address: u32, bytes: &[u8]) -> Option<&[CstBodySource]> {
        let origin = self.returned_origin(task, address, bytes)?;
        self.body_is_current(&origin.body).then_some(origin.body.as_slice())
    }

    pub(super) fn publish(&mut self, task: u32, address: u32, id: u32, bytes: &[u8]) {
        if let Some(origin) = self.returned_origin(task, address, bytes)
            .filter(|origin| !bytes.is_ascii() || !origin.body.is_empty())
        {
            let origin = CstByteOrigin { bytes: bytes.to_vec(), codec: origin.codec, body: origin.body.clone() };
            self.shared.insert(id, origin);
        } else {
            self.shared.remove(&id);
        }
    }

    pub(super) fn shared(&self, id: u32, bytes: &[u8]) -> Option<ScriptTextEncoding> {
        self.shared.get(&id).and_then(|origin| (origin.bytes == bytes && !bytes.is_ascii()).then_some(origin.codec))
    }

    pub(super) fn shared_body(&self, id: u32, bytes: &[u8]) -> Option<&[CstBodySource]> {
        let origin = self.shared.get(&id)?;
        (origin.bytes == bytes && self.body_is_current(&origin.body)).then_some(origin.body.as_slice())
    }

    pub(super) fn body_is_current(&self, sources: &[CstBodySource]) -> bool {
        !sources.is_empty() && sources.iter().all(|source|
            self.body_loads.get(&source.context_handle) == Some(&source.load_id))
    }

    pub(super) fn set_shared_encoding(&mut self, id: u32, bytes: &[u8], codec: ScriptTextEncoding) {
        if bytes.is_ascii() {
            self.shared.remove(&id);
        } else {
            self.shared.insert(id, CstByteOrigin { bytes: bytes.to_vec(), codec, body: Vec::new() });
        }
    }

    pub(super) fn invalidate_shared(&mut self, id: u32) {
        self.shared.remove(&id);
    }

    pub(super) fn note_shared_string_write(
        &mut self, destination: u32, source: Option<u32>, bytes: &[u8],
    ) {
        let copied = source.and_then(|id| self.shared.get(&id))
            .filter(|origin| origin.bytes == bytes).cloned();
        if let Some(origin) = copied {
            self.shared.insert(destination, origin);
        } else {
            self.invalidate_shared(destination);
        }
    }

    fn capture_raw_copy(&self, task: u32, target: u32, source: u32, append: bool, vm: &Vm)
        -> Option<CstRawCopy>
    {
        if !self.returned.iter().any(|((owner, address), _)|
            *owner == task && (*address == source || append && *address == target))
        {
            return None;
        }
        let suffix = vm.read_string(source).ok()?;
        let prefix = if append { vm.read_string(target).ok()? } else { Vec::new() };
        let prefix_codec = self.returned(task, target, &prefix);
        let suffix_codec = self.returned(task, source, &suffix);
        // Hatsumira's sscript appends the color-default command and the body
        // separately to its message rebuild cache at native 15/next PC 0xeae.
        let hatsumira_color_reset = append && target == 0x23af8 && vm.instruction_pointer == 0xeae
            && (prefix == b"\\cd" || suffix == b"\\cd")
            && vm.program_code_sha256() == Some(super::story_trace::story_trace_12::SSCRIPT_CODE)
            && [target, source].into_iter().zip([&prefix, &suffix]).any(|(address, bytes)| {
                self.returned_origin(task, address, bytes).is_some_and(|origin| {
                    origin.codec == ScriptTextEncoding::Cp932 && self.body_is_current(&origin.body)
                        && matches!(origin.body.as_slice(), [tag] if self.source_probes
                            .get(&tag.context_handle).is_some_and(|load|
                                matches!(load.probe, CstSourceProbe::Hatsumira12 | CstSourceProbe::Hatsumira12PostOp)
                                    && load.load_id == tag.load_id))
                })
            });
        let body_part = |address, bytes: &[u8]| {
            if let Some(origin) = self.returned_origin(task, address, bytes)
                && self.body_is_current(&origin.body)
            {
                return Some(origin.body.as_slice());
            }
            (body_control_only(bytes) || hatsumira_color_reset && bytes == b"\\cd").then_some(&[][..])
        };
        let body = body_part(target, &prefix).zip(body_part(source, &suffix))
            .and_then(|(prefix, suffix)| join_body_sources(prefix, suffix)).unwrap_or_default();
        let codec = match (prefix.is_ascii(), suffix.is_ascii()) {
            (true, false) => suffix_codec?,
            (false, true) => prefix_codec?,
            (false, false) if prefix_codec == suffix_codec => prefix_codec?,
            (true, true) if !body.is_empty() => suffix_codec.or(prefix_codec)?,
            _ => return None,
        };
        Some(CstRawCopy { prefix, suffix, codec, body })
    }

    fn finish_copy(&mut self, task: u32, target: u32, copy: Option<CstRawCopy>, vm: &Vm) {
        self.invalidate_returned(task, target);
        if let Some(copy) = copy
            && let Ok(bytes) = vm.read_string(target)
            && copy.prefix.iter().chain(&copy.suffix).eq(bytes.iter())
        {
            self.record_with_body(task, target, &bytes, copy.codec, copy.body);
        }
    }
}

struct CstRawCopy {
    prefix: Vec<u8>,
    suffix: Vec<u8>,
    codec: ScriptTextEncoding,
    body: Vec<CstBodySource>,
}

const SSCRIPT_SPEAKER_COPY_NEXT_PC: usize = 0x1476;
const SSCRIPT_SPEAKER_SCRATCH: u32 = 0x8000_176c;

fn capture_sscript_speaker_copy(
    source: u32,
    vm: &Vm,
    origins: &CstTextOrigins,
) -> Option<CstRawCopy> {
    const SSCRIPT_CODE_SHA256: &str = "3024880f72c31051d071f6cfb6c1b4125086de3b355691a903293e4dfb826fa6";
    if vm.instruction_pointer != SSCRIPT_SPEAKER_COPY_NEXT_PC {
        return None;
    }
    let offset = source.checked_sub(SSCRIPT_SPEAKER_SCRATCH)?;
    if matches!(offset, 9 | 12) {
        // The AUGUST handler copies the bytes after the fullwidth @.
        let (group, record, length, digest) = if offset == 9 {
            (1, 83, 18, "8fc31bbe2d569dce62fa4e759f0cd6bb32bce9f64fc5138a1120d2bfc6be4e19")
        } else {
            (2, 88, 21, "60542969ed192aa25f9dee45c5b19f614d90d943216107867d4e022b01fa3efc")
        };
        let current_read = origins.source_probes.keys().any(|&handle| {
            origins.source_record_observations(handle).iter().any(|entry| {
                entry.probe == CstSourceProbe::August
                    && entry.native == 1037
                    && (entry.source.group, entry.source.absolute_record) == (group, record)
                    && entry.kind == 0x21
                    && entry.stored_codec == ScriptTextEncoding::Cp932
                    && entry.output_codec == ScriptTextEncoding::Utf8
                    && entry.output_bytes == length
                    && !entry.localized
            })
        });
        if !current_read
            || sha256(&vm.read_string(SSCRIPT_SPEAKER_SCRATCH).ok()?) != digest
        {
            return None;
        }
    } else if !matches!(offset, 0 | 3) {
        return None;
    }
    let suffix = vm.read_string(source).ok()?;
    if suffix.is_ascii() || std::str::from_utf8(&suffix).is_err()
        || vm.program_code_sha256()? != SSCRIPT_CODE_SHA256
    {
        return None;
    }
    Some(CstRawCopy { prefix: Vec::new(), suffix, codec: ScriptTextEncoding::Utf8, body: Vec::new() })
}

pub(super) fn verified_newton_text_profile(resources: &ResourceIndex) -> bool {
    let Some(config) = resources.find("config/startup.xml").filter(|candidate| candidate.size == 39_216)
    else { return false; };
    let Ok(config) = resources.read_candidate(config) else { return false; };
    if sha256(&config) != "8019386a793415a0520a7182fa8c9b4e4e4bec20e9e4caaf6e509f503ce64f3f" {
        return false;
    }
    let Ok(names) = resources.category_names(b"kcs", "sscript") else { return false; };
    let Some(script) = names.iter().find_map(|name| resources.find(name))
        .filter(|candidate| candidate.size == 145_552) else { return false; };
    resources.read_candidate(script).is_ok_and(|bytes|
        sha256(&bytes) == "1f581def5d97d2f8a5ecb465f4ee5803fa30788d7c8fba0852b4b26ce769c5c4")
}

fn newton_loose_scene(root: &Path, source: &SourcedResource) -> bool {
    let ResourceLocation::File { path } = &source.source.location else { return false; };
    let Ok(relative) = path.strip_prefix(root) else { return false; };
    relative.parent().is_some_and(|parent| parent.eq(Path::new("scene")))
        && relative.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("cst"))
}

// Decrypted CST member hashes from the installed Rebellions Episode A patch.
// Keep the names in ascending ASCII order for lookup.
const REBELLIONS_GBK_SCENES: &[(&str, &str)] = &[
    ("a_01_01.cst", "92de81ed6532c41611106715b4d8185cd5210ff0b9a177586e1d4d9f1b839176"),
    ("a_01_02.cst", "40e144527644238f1a97f8b1a1e2eda841eecebd7f62909e2108ed4af1deab10"),
    ("a_01_03.cst", "7b314b3e706ad3e31b39fe86387745947e99e180b18f0873e7f3d70d655407fb"),
    ("a_01_04.cst", "658d38d147381aefb1aa32b7e097d3d7e79870986c62d1f48cc6a78cfe3e8ae1"),
    ("a_01_05.cst", "8cbf0595182715252c1b64a1a6525471ea61f2b254a0b7edd3606e19c0d72fa1"),
    ("a_01_06.cst", "f878c9cf53c1bcb0828ee47f0574a7d4ec0f0bbee0c2008140b3e79df8a6e0b0"),
    ("a_01_07.cst", "538cb47cc62a0800b0d2d68e42ac6efeeafa0600022f80ded4d1b0600b01034b"),
    ("a_01_08.cst", "c8a0b6ae7b3404afd590c44c30cb6e7883d312ba653b050c4d81d6ae483f2d2b"),
    ("a_01_09.cst", "4049ab16b2c53d474b115b3f4452d9f4f0fab24bc5bc2d3e9ac0c05104900fed"),
    ("a_01_10.cst", "05cf3ce1ad2eccacf2b3d11360721584b13653789b7b1aeb20b1eea30922f04d"),
    ("a_01_11.cst", "ef833a73a00fd571eb86b85f4b1832679c337c4abca3356a014c80e1688ab76e"),
    ("a_01_12.cst", "e626d690593e76b8101b35f1d5f424c441602b22cd77f3962a5f0a3aaf58ea1a"),
    ("a_01_13.cst", "59d256aa0c08faac4673be1f35cdbb36b1d0938f2df5c1d5bcd5d24fd46795e4"),
    ("a_01_14.cst", "4079685764dca951254041963c011411682e7b4edad557afbcd23e81aab20303"),
    ("a_01_15.cst", "4a6d14e1d25853e4aa271e743d20824de3e226d2c81b1282350473497329c54e"),
    ("a_01_16.cst", "aa8e40f07506c7a4e823c5e59f4a5ec59d76ec0d3746a673809e2170cea5724b"),
    ("a_01_17.cst", "dd01c8606e392468ccc209362d80341e5a3a862bb7a0aacd82a4c2c3c8ebe211"),
    ("a_01_18.cst", "d0b7564c3ee9408d58f63a3ec78cdc333a8ff1e9985bf37b741110ecb8e210eb"),
    ("a_01_19.cst", "8de7b939b6ddd77e4683c30a6ddd5861d83545156b5f78dde5dc0fb8c8035e60"),
    ("a_01_20.cst", "51b76821e0db96f2a006ad8db08a00f717a6191a1c8063c80b93293ebc9bfc1e"),
    ("a_02_01.cst", "916dcad559f9c7e624cd5165bb1c513e7401daf2499fdbb393ea0481c26fb71a"),
    ("a_02_02.cst", "7def2eec1158c0a52d2c85964dd7d22ac2d1b443d25858531a8b95fe0c998316"),
    ("a_02_03.cst", "fcf0cb78a4cc4406b6f24b5ea41033dd9c28467adc8256a6cbcd597fd692db41"),
    ("a_02_04.cst", "4166b71d1bc6537944863d6798fb2dfd0d60b8ce7a05dd7419339795f88871b2"),
    ("a_02_05.cst", "84dcdd248fe39648670c3620cd6b62708423d4f08726755ed0720e908009570c"),
    ("a_02_06.cst", "ec16eadbc91ca77befc95ac59cf9bc991571de6f5869ab7ffe46e286d4d6bcf2"),
    ("a_02_07.cst", "2e7e69c32e2f5e9b85c2cfd5a2195e079bb7d770c0b1e53f977e279e2c0619dd"),
    ("a_02_08.cst", "3f33a6e53d7fb24a4944b8d4d0f055b32ec58e8aa7916e44aed295daea9801ca"),
    ("a_02_09.cst", "d7cea84428cd947124579db5a52d54a6b0d7cfadcacd3905c18dcb2c1433e884"),
    ("a_02_10.cst", "13ef207af4d8f19c4f3663ec20f2675717aaf4f83d4b0866c084e9410c9a2202"),
    ("a_02_11.cst", "c74794fe1dfd731e25b0a18187ce86041459fa8a8d8c3154308b88ea55833fa7"),
    ("a_02_12.cst", "4d32f5d7f823b6febac33d978fff227cfe33d8feb4f5669416d995a835413fa7"),
    ("a_03_01.cst", "ccc9d849aa057c5372ded29d9e823683b262a886f94a64ae13f23cbad4098916"),
    ("a_03_02.cst", "c2ebf026e72ee00dcf31d960ceb82147c7ec8ed144f36423fb98e870efae5d73"),
    ("a_03_03.cst", "77b2c9f68edb678b8a85fb8c9df7b4666c9c5c104f414b34cde3aabdfff20d5e"),
    ("a_03_03_1.cst", "7a8884696ce79044cd211d2b74c4df966c8e4542d3976b0e48b758d3f8ebe1dd"),
    ("a_03_03_h.cst", "e61dd6b003e98f1fc9355d77bfb0824fffefc45ce2bd62694efb9b0dd6f3e48c"),
    ("a_03_04.cst", "4455b83ff382e00f625481b86884f95f83f71fb6347881f333b11754c4ef4837"),
    ("a_03_05.cst", "6f516344ee9998b7196266fccee2d3b7f82ed67d1e963b8244fc26aeda41fd0a"),
    ("a_03_06.cst", "c774a5fa8fd9ac3edf0b28bb070aeb66e26d184a348595a713b7fd20c1370be2"),
    ("a_03_07.cst", "7dac265e147511d764f01d1a98ab7e8ea3cc350bb61096bd74ecd8c2da59d311"),
    ("a_03_08.cst", "66c69294df3080e3de174f1e08a9cb8c3584a93d6fae9df73869f29d9df93754"),
    ("a_03_09.cst", "84f43537d3e889872f7466d0e9d464fb2058f1fde2d39112e133b8fb3cbaefdb"),
    ("a_03_10.cst", "a6afed80642525f26b567d42d964f793ae6491c6122fc451bf7d430ad8f65242"),
    ("a_04_01.cst", "da5c95fa6276372644ec43cb004c78eb8d34fa41a914c098721ded71d0865fa5"),
    ("a_04_02.cst", "e51f10c0f4fc1da69e40558fecb298719881eabe6443cb822517e966d741f0ce"),
    ("a_04_03.cst", "33b257cda45982eb21b61ead13ef55909ce22d64483cafa1b71a307dfd1492e2"),
    ("a_04_04.cst", "14016e1e4d2cbc894a758ea23e03ed7ca4a56ec60d02bdac65c1d308fecd50c5"),
    ("a_04_05.cst", "b57dd988b2092f0b624f083ce264ee1bccb6522a329229f8b624418cc8e860a9"),
    ("a_04_06.cst", "df8ce942ef57d8ca892ccb0aa34fb836be8f6c3824a0c246e711c1869af9f0c3"),
    ("a_04_07.cst", "7284cd3ddce53a4e74f8ba29beb70f3bde46c14c8ffdc674371a1aa807555515"),
    ("a_04_08.cst", "8fad2ac7e535dd92dc8f3ca14808228cc40c45af2949c8df64d53623564301cb"),
    ("a_04_09.cst", "bda5af05ac363cf1b97959fcc5463248ce7b49510cd6d000de9e1c1fb2bc8e81"),
    ("a_04_10.cst", "12b065e360c26ebd8bf24863dd5336f9276377e1be8a5708649cccd610bbf34b"),
    ("a_04_11.cst", "f9932b2f6c6af723fc50a94f9cacfa38cf4f3970d4fdacc5af7f66ec60964f05"),
    ("a_04_12.cst", "883d495a4c6e524b7528a5570276b9b825e830b3b4b042699d95f70dee1f082d"),
    ("a_04_13.cst", "01883673cf3d2d351f9e59c467e608920214d42acb0a771dec8e48a31d0907c5"),
    ("a_04_14.cst", "6e1e5215ae970bee60ab47c75cf4bcae3870658b239c1dd76f8fbecd47c9d471"),
    ("a_04_15.cst", "1be9768e6f100efba7e548d3035c29284a65c3efa013eb3c7df76269b870e1a2"),
    ("a_04_15_1.cst", "a643309408f759bee1015f3b4abcf9e4562877d8e517bd5c536a489830addfd7"),
    ("a_04_15_h.cst", "0231b41e1eea95c2385bacfcecbe108e04b3f4484336ec40cb767dd832743f1e"),
    ("a_05_01.cst", "89bb82648e14de51016b9d32fe4825d343903d951aa8ce6d5bbee27cf1c91dae"),
    ("a_05_02.cst", "fcb7fbe35b5aeb9d222ff250b885063bf56710abeb0d80dc0084524ec87fcf82"),
    ("a_05_03.cst", "60e2ff097d1e9ef0010009a5969d487d98a61b2474802a0f4e0cdca19987d952"),
    ("a_05_04.cst", "adfc192064a5dd5258e3dbf4fbbbdd461bab70122e406a384ffe4bd7526eb452"),
    ("a_05_05.cst", "98c9de93d7c982411cf2cb4f700d5c477f107eb57f982195de75d55f629ce514"),
    ("a_05_06.cst", "3953943225aeba2f6041ed40f2f974ef899d85e7e174ebd3a3f06e2a822b3f58"),
    ("a_05_07.cst", "6431432c933a9a2d94f1dff3f9eab487ff9bb68eddabc1b15c781482aa96731b"),
    ("a_05_08.cst", "6bcd94400793f94bccc86c3ffa763fe8a3b9358ff2764d2e2127689eaa3986f3"),
    ("a_05_09.cst", "b741c65be0365c677297554319ddc19e61bf602913b40908c5ae3b2b8bb55259"),
    ("a_05_09_h.cst", "413c60846acb8a0204de6cee8d358a38d7b11219318657b8037722ea3221b792"),
    ("a_05_09_h_1.cst", "348091bac0de21ffba4789e27b1d731bcdbc9eb7c56782a325b39d7dbc32d334"),
    ("a_06_01.cst", "50add71cb16fb3081dfe9f306e7a803e1af8ee313d52c9874f30b40e629c181e"),
    ("a_06_02.cst", "8ad119f16970f7a9d6e9372684789c977d911902d32afcd0dc6ac9d2528f54e7"),
    ("a_06_03.cst", "64895c8bfac1da953c35d9c2edc43133fa05aa88f2a852abb22fcb65cbd2886c"),
    ("a_06_04.cst", "05c5f1e5f0598cb9d359958cb4ded55d85c4270bf2f1e0f5d4730318954503e3"),
    ("a_06_05.cst", "8eee1d5facc08005bcdb9145a34c57ba673ba00df3e58c741515a4f7843d31b5"),
    ("a_06_06.cst", "63cb4e5f3fca02e11de32db57404eb47e18e5df6b1e121ce322ecca364aadaa0"),
    ("a_06_07.cst", "b7d44e2a859c515deaad0e2b167eb6735f594178bc40951ae8b6a5f309b013c8"),
    ("a_06_08.cst", "a0f08bea2f4e6672c9c29148c81cc8908bda2312cc39132efed9450956478e10"),
    ("a_06_09.cst", "146a41472503dec09a5fe9dc2d0c8c4066e3becceb53b171e07fc688c01e6fd5"),
];

pub(super) fn rebellions_gbk_member_digest(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    let index = REBELLIONS_GBK_SCENES.binary_search_by_key(&name.as_str(), |(key, _)| *key).ok()?;
    Some(REBELLIONS_GBK_SCENES[index].1)
}

fn verified_gbk_scene(root: &Path, resource: &SourcedResource) -> bool {
    let relative_matches = |path: &Path, expected: &str| {
        path.strip_prefix(root).ok().is_some_and(|relative|
            relative.to_string_lossy().replace('\\', "/").eq_ignore_ascii_case(expected))
    };
    let digest = match &resource.source.location {
        ResourceLocation::File { path } if relative_matches(path, "scene/com_001A.cst") =>
            "91e7241ee4dc6b0c36d697913cf22bb9649d7ea5b924b88e199a3bcb6e8f4e5b",
        ResourceLocation::Archive { path, .. } if relative_matches(path, "update03.int")
            && resource.source.name.eq_ignore_ascii_case("prologue.cst") =>
            "4f713b09acbe41fd0d19e072eb3d5f89d3ee34854904aa8300640f0ba00e05e8",
        ResourceLocation::Archive { path, .. } if relative_matches(path, "scene.int")
            && resource.source.name.eq_ignore_ascii_case("aft_002.cst") =>
            "0d5670492bf927f91d358bcc4b0d857bac7ebd786c3dafa7b5c56fd638cb098a",
        ResourceLocation::Archive { path, .. } if relative_matches(path, "scene.int")
            && resource.source.name.eq_ignore_ascii_case("BLAOP1_END.cst") =>
            "c09bcb90152977cd9d21b68ec019d627ccc80136036503a5ce376a2ae6020561",
        ResourceLocation::Archive { path, .. } if relative_matches(path, "scene.int") => {
            let Some(digest) = rebellions_gbk_member_digest(&resource.source.name) else { return false; };
            digest
        }
        ResourceLocation::Archive { path, .. } if relative_matches(path, "update01.int")
            && resource.source.name.eq_ignore_ascii_case("op.cst") =>
            "2596c7c430dabed8e1e3144c50c4b8672b59b1d452e4b92b7e18cf0c9f409817",
        ResourceLocation::Archive { path, .. } if relative_matches(path, "update05.int")
            && resource.source.name.eq_ignore_ascii_case("common00.cst") =>
            "dea050b2a240e487c5ceed1aca21cfa6e1fb985b71a3f856b1a5aea0f92a51c3",
        ResourceLocation::Archive { path, .. } if relative_matches(path, "update05.int")
            && resource.source.name.eq_ignore_ascii_case("common01.cst") =>
            "80bf853d65d55a8683c3860ab4453973404da781bcca092589a8283d22fa4ac1",
        _ => return false,
    };
    sha256(&resource.bytes) == digest
}

fn require_gbk_records(document: &CstDocument) -> Result<()> {
    for record in &document.records {
        if encoding_rs::GBK.decode_without_bom_handling_and_without_replacement(&record.text).is_none() {
            return Err(native_state_error("verified GBK scene contains a malformed record"));
        }
    }
    Ok(())
}

pub(super) fn decode_cst_dialogue(bytes: &[u8], codec: ScriptTextEncoding) -> Result<Cow<'_, str>> {
    let bytes = trim_nul(bytes);
    let decoded = match codec {
        ScriptTextEncoding::Cp932 => encoding_rs::SHIFT_JIS.decode_without_bom_handling_and_without_replacement(bytes),
        ScriptTextEncoding::Gbk => encoding_rs::GBK.decode_without_bom_handling_and_without_replacement(bytes),
        ScriptTextEncoding::Utf8 => std::str::from_utf8(bytes).ok().map(Cow::Borrowed),
    };
    decoded.ok_or_else(|| native_state_error_owned(format!("CST dialogue has invalid {codec:?} source bytes")))
}

type CstCommandRepair = (usize, usize, usize, &'static [u8], &'static [u8]);

const REBELLIONS_DATE_CONDITIONS: [(&[u8], &[u8]); 2] = [
    (br"if (( #80 > 0 )&&(( #81 < 1 )||( \81 > 19 )) #81 = #9 / 10000",
         b"if (( #80 > 0 )&&(( #81 < 1 )||( #81 > 19 ))) #81 = #9 / 10000"),
    (br"if (( #80 > 0 )&&(( #81 < 1 )||( \81 > 19 )) #9 = #81 * 10000 + #80",
         b"if (( #80 > 0 )&&(( #81 < 1 )||( #81 > 19 ))) #9 = #81 * 10000 + #80"),
];

const KANMI_POSITIONS: &[CstCommandRepair] = &[
    (207, 3, 975, b"eg 1 pos %3[-18] %4[-10]", b"eg 1 pos -18 -10"),
    (208, 2, 986, b"eg 1 pos %3[-18] %4[-10]", b"eg 1 pos -18 -10"),
    (209, 2, 996, b"eg 1 pos %3[-18] %4[-10]", b"eg 1 pos -18 -10"),
];

fn rebellions_date_repairs(document: &CstDocument) -> Vec<CstCommandRepair> {
    let mut repairs = Vec::new();
    for (group, range) in document.inputs.iter().enumerate() {
        let Some(records) = document.input_records(group) else { continue; };
        for (first, pair) in records.windows(2).enumerate() {
            if !pair.iter().zip(REBELLIONS_DATE_CONDITIONS).all(|(record, (original, _))|
                record.kind == 0x30 && record.text.as_slice() == original)
            {
                continue;
            }
            for (offset, (original, repaired)) in REBELLIONS_DATE_CONDITIONS.into_iter().enumerate() {
                let ordinal = first + offset;
                repairs.push((group, ordinal, range.start + ordinal, original, repaired));
            }
        }
    }
    repairs
}

fn repair_scene_commands(document: &mut CstDocument, repairs: &[CstCommandRepair],
    scene: &str, handle: u32, frame: u64, events: &mut Vec<LogicEvent>,
) {
    if !repairs.iter().all(|(group, ordinal, absolute, original, _)| {
        document.inputs.get(*group).is_some_and(|range|
            range.start.checked_add(*ordinal) == Some(*absolute) && range.contains(absolute))
            && document.records.get(*absolute).is_some_and(|record|
                record.kind == 0x30 && record.text.as_slice() == *original)
    }) {
        return;
    }
    for &(group, ordinal, absolute, original, repaired) in repairs {
        document.records[absolute].text = repaired.to_vec();
        events.push(LogicEvent { frame, kind: "cst-command-compat".into(),
            detail: format!("scene={scene} handle={handle} group={group} record={absolute} ordinal={ordinal} original={:?} repaired={:?}",
                String::from_utf8_lossy(original), String::from_utf8_lossy(repaired)),
        });
    }
}

impl LoggingHost<'_> {
    fn cst_source_probe(&self, resource: &SourcedResource) -> Option<CstSourceProbe> {
        let root = self.resources.root();
        if self.data_services.newton_legacy_gbk
            && matches!(self.css_task_sha(), Some(NEWTON_PE | NEWTON_LOCALIZED_PE))
            && let Some(member) = resource.source.name.rsplit(['/', '\\']).next()
            && (member.eq_ignore_ascii_case("com_001A.cst")
                || member.eq_ignore_ascii_case("com_001B.cst"))
            && newton_loose_scene(root, resource)
            && verified_gbk_scene(root, resource)
        {
            if member.eq_ignore_ascii_case("com_001A.cst") {
                return Some(CstSourceProbe::Newton16);
            }
            if member.eq_ignore_ascii_case("com_001B.cst")
                && sha256(&resource.bytes) == newton_route_data::SECOND_SHA256
            {
                return Some(CstSourceProbe::Newton16B);
            }
        }
        if self.css_task_sha() == Some("72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8")
            && matches!(&resource.source.location, ResourceLocation::File { path }
                if path.strip_prefix(root).is_ok_and(|path| path == Path::new("scene").join("c01_01.cst")))
            && sha256(&resource.bytes) == "942ca2dbd98382b7196ba97f8b38709d672ce5d9c97d62068a9e3b423dafdee2"
        {
            return Some(CstSourceProbe::Eye08);
        }
        let ResourceLocation::Archive { path, .. } = &resource.source.location else { return None; };
        let archive = path.strip_prefix(self.resources.root()).ok()?.to_str()?;
        use crate::story_trace::{story_trace_10, story_trace_12, story_trace_14, story_trace_18};
        if self.css_task_sha() == Some(story_trace_14::PE)
            && archive.eq_ignore_ascii_case("UPDATE01.INT")
            && resource.source.name.eq_ignore_ascii_case("op.cst")
            && sha256(&resource.bytes) == story_trace_14::OP_CST
        {
            return Some(CstSourceProbe::Grisaia14);
        }
        if archive.eq_ignore_ascii_case("update05.int")
            && resource.source.name.eq_ignore_ascii_case("common00.cst")
            && verified_gbk_scene(root, resource)
            && self.verified_sudden_sound_loop_source().unwrap_or(false)
        {
            return Some(CstSourceProbe::Sudden19);
        }
        if self.css_task_sha() == Some(story_trace_18::PE)
            && archive.eq_ignore_ascii_case("update00.int")
            && resource.source.name.eq_ignore_ascii_case("0001_00.cst")
            && sha256(&resource.bytes) == story_trace_18::LOCALIZED_CST
        {
            return Some(CstSourceProbe::CampanellaLocalized);
        }
        if !archive.eq_ignore_ascii_case("scene.int") {
            return None;
        }
        if self.css_task_sha() == Some(story_trace_10::PE)
            && resource.source.name.eq_ignore_ascii_case("OP01.cst")
            && sha256(&resource.bytes) == story_trace_10::CST
        {
            return Some(CstSourceProbe::Djibril10);
        }
        if self.data_services.particle_message_pe_sha256
            == Some("5869b7d03a636c200f5042f0c3fc37878a362615bf724f58d47aec5eb8b2d44c")
        {
            let pinned_scenes = [
                ("0_01_01.cst", happiness_route_data::FIRST_SHA256, CstSourceProbe::Happiness02),
                ("0_01_02.cst", happiness_route_data::SECOND_SHA256, CstSourceProbe::HappinessSecond),
                ("0_01_03.cst", happiness_route_data::THIRD_SHA256, CstSourceProbe::HappinessThird),
            ];
            if let Some((_, digest, probe)) = pinned_scenes.into_iter()
                .find(|(name, _, _)| resource.source.name.eq_ignore_ascii_case(name))
            {
                return (sha256(&resource.bytes) == digest).then_some(probe);
            }
        }
        if resource.source.name.eq_ignore_ascii_case("00_01.cst")
            && sha256(&resource.bytes) == "306ac319a2eb7589dba849bd5391ae0d189f3d851bdeb775288156a3f6f163c5"
            && self.verified_utate_sound_loop_source().unwrap_or(false)
        {
            return Some(CstSourceProbe::Utate05);
        }
        if self.css_task_sha() == Some(story_trace_18::PE)
            && resource.source.name.eq_ignore_ascii_case("0001_00.cst")
            && sha256(&resource.bytes) == story_trace_18::CST
        {
            return Some(CstSourceProbe::Campanella);
        }
        if let Some(probe) = crate::story_trace::legacy_story_wait::cst_probe_04_06(
            self.css_task_sha(), &resource.source.name, &resource.bytes,
        ) {
            return Some(probe);
        }
        if self.css_task_sha() == Some(story_trace_12::PE)
            && resource.source.name.eq_ignore_ascii_case("op001.cst")
            && sha256(&resource.bytes) == story_trace_12::CST
        {
            return Some(CstSourceProbe::Hatsumira12);
        }
        if self.css_task_sha() == Some(story_trace_12::PE)
            && matches!(&resource.source.location, ResourceLocation::Archive { path, .. }
                if path.strip_prefix(root).ok().is_some_and(|relative| relative == Path::new("scene.int")))
            && resource.source.name.eq_ignore_ascii_case("OPMOVIE_END.cst")
            && sha256(&resource.bytes) == story_trace_12::POST_OP_CST
        {
            return Some(CstSourceProbe::Hatsumira12PostOp);
        }
        if self.css_task_sha() == Some("c2af2ee9baf4b26a3c2cf8a1c867f15f92b54ad1ac5fac381858046a47818f6c")
            && resource.source.name.eq_ignore_ascii_case("BLAOP1_END.cst")
            && sha256(&resource.bytes) == "c09bcb90152977cd9d21b68ec019d627ccc80136036503a5ce376a2ae6020561"
        {
            Some(CstSourceProbe::Eden)
        } else if self.data_services.particle_message_pe_sha256
            == Some("c264b2465889c78723079a694d2654ef1091253db657a735e92f58b75031aa26")
            && resource.source.name.eq_ignore_ascii_case("sc01.cst")
            && sha256(&resource.bytes) == "7c0d796d069db416291effc217aa3664ba449f23ce43b88a11a5d13cfff1a55e"
        {
            Some(CstSourceProbe::Kanmi)
        } else if self.data_services.particle_message_pe_sha256
            == Some("c264b2465889c78723079a694d2654ef1091253db657a735e92f58b75031aa26")
            && matches!(&resource.source.location, ResourceLocation::Archive { path, .. }
                if path.file_name().is_some_and(|name| name.eq_ignore_ascii_case("scene.int")))
            && resource.source.name.eq_ignore_ascii_case("sc02.cst")
            && sha256(&resource.bytes) == "8284a2ad28f30b0f82008ae443b7dea6f811427d8c2f3d5314d6467e89364b10"
        {
            Some(CstSourceProbe::KanmiSc02)
        } else if self.data_services.particle_message_pe_sha256
            == Some("c264b2465889c78723079a694d2654ef1091253db657a735e92f58b75031aa26")
            && matches!(&resource.source.location, ResourceLocation::Archive { path, .. }
                if path.file_name().is_some_and(|name| name.eq_ignore_ascii_case("scene.int")))
            && resource.source.name.eq_ignore_ascii_case("sc03.cst")
            && sha256(&resource.bytes) == "68952bbbf0273ba4f1a1d7b77a38304fa65ce71a536d33d140393c22a996c209"
        {
            Some(CstSourceProbe::KanmiSc03)
        } else if self.css_task_sha() == Some("4ccb48ad4bb6bfb5a15a45b8e1f2c2e62072cfad9f1487f869561a473770ed94")
            && resource.source.name.eq_ignore_ascii_case("00_001.cst")
            && sha256(&resource.bytes) == "9e4efc626b072ff3de336665e3f23bb993e9da4f79f9a59e5bee5ba183b8fe78"
        {
            Some(CstSourceProbe::August)
        } else if self.css_task_sha() == Some("f6af7437475b8c066be973245aa79688bea510ca1192264add4f1914ebee4c0e")
            && resource.source.name.eq_ignore_ascii_case("com01.cst")
            && sha256(&resource.bytes) == "bccd3463db8d0f173101609a1ad6e57f4edc5d7f2c084763a17826863d8ce086"
        {
            Some(CstSourceProbe::Kamikaze)
        } else if resource.source.name.as_bytes().get(..2).is_some_and(|head| head.eq_ignore_ascii_case(b"A_"))
            && rebellions_gbk_member_digest(&resource.source.name).is_some()
            && verified_gbk_scene(self.resources.root(), resource)
            && self.resources.resolve_plain_file("REBELLIONS.exe").ok().flatten()
                .filter(|candidate| candidate.size == 4_350_976)
                .and_then(|candidate| self.resources.read_candidate_bounded(candidate, 4_350_976).ok().flatten())
                .is_some_and(|bytes| sha256(&bytes) == "a099ac1cee7af60c8f32869485c9df4aab344b5eb91c1018ae8bb7df2c1f39f3")
        {
            if resource.source.name.eq_ignore_ascii_case("A_01_01.cst") {
                Some(CstSourceProbe::Rebellions)
            } else if resource.source.name.eq_ignore_ascii_case("A_01_02.cst") {
                Some(CstSourceProbe::RebellionsSecond)
            } else if resource.source.name.eq_ignore_ascii_case("A_01_03.cst") {
                Some(CstSourceProbe::RebellionsThird)
            } else {
                Some(CstSourceProbe::RebellionsLater)
            }
        } else {
            None
        }
    }

    pub(super) fn cst_string_copy_native(&mut self, index: u32, args: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        let packet = if args.len() >= 8 { args } else { vm.native_argument_storage(8)? };
        let target = read_u32(packet, 0)?;
        let source = read_u32(packet, 4)?;
        let task = self.current_task_id;
        let copy = if matches!(index, 13 | 15) {
            self.data_services.cst_text_origins.capture_raw_copy(task, target, source, index == 15, vm)
        } else { None };
        let speaker_copy = if copy.is_none() && index == 13 {
            capture_sscript_speaker_copy(source, vm, &self.data_services.cst_text_origins)
        } else { None };
        let trace_len = speaker_copy.as_ref().map(|source| source.suffix.len());
        let color_reset = copy.as_ref().and_then(|copy| {
            (index == 15 && target == 0x23af8 && vm.instruction_pointer == 0xeae
                && (copy.prefix == b"\\cd" || copy.suffix == b"\\cd"))
                .then(|| copy.body.first().copied()).flatten()
        });
        let result = self.string_copy_native(index, args, vm);
        let origins = &mut self.data_services.cst_text_origins;
        origins.finish_copy(task, target, if result.is_ok() { copy.or(speaker_copy) } else { None }, vm);
        if self.data_services.story_trace && let Some(source) = color_reset
            && let Ok(bytes) = vm.read_string(target)
            && origins.returned_origin(task, target, &bytes).is_some_and(|origin| origin.body == [source])
        {
            self.events.push(LogicEvent {
                frame: self.frame, kind: "hatsumira-body-cache-color-reset".into(),
                detail: format!("task={task} target={target:#x} bytes={} source={source:?}", bytes.len()),
            });
        }
        if self.data_services.story_trace && let Some(length) = trace_len
            && let Ok(bytes) = vm.read_string(target)
            && origins.returned(task, target, &bytes) == Some(ScriptTextEncoding::Utf8)
        {
            self.events.push(super::LogicEvent {
                frame: self.frame,
                kind: "cst-text-sourced-copy".into(),
                detail: format!(
                    "task={task} native=13 next_pc={:#x} scratch_offset={} bytes={length} source=sscript-speaker/Utf8",
                    vm.instruction_pointer, source - SSCRIPT_SPEAKER_SCRATCH,
                ),
            });
        }
        result
    }

    pub(super) fn copy_cst_attached_data_native981(&mut self, args: &[u8]) -> Result<NativeReturn> {
        let destination = read_u32(args, 0)?;
        if destination == 0 { return Ok(NativeReturn::Continue); }
        let source = read_u32(args, 4)?;
        if source == 0 { return Ok(NativeReturn::Continue); }
        let destination_context = self.cst_contexts.get(&destination)
            .ok_or_else(|| native_state_error("native 981 invalid/wrong-type destination context"))?;
        let source_context = self.cst_contexts.get(&source)
            .ok_or_else(|| native_state_error("native 981 invalid/wrong-type source context"))?;
        if source == destination {
            // The original frees the same buffer it subsequently reads.
            if destination_context.registrations.iter().chain(&destination_context.history)
                .any(|entry| !entry.data.is_empty())
            {
                return Err(native_state_error("native 981 self-copy would read freed attached data"));
            }
            return Ok(NativeReturn::Continue);
        }
        let mut total = 0usize;
        let mut collect = |source: &[super::CstRegistration], count: usize| -> Result<Vec<Vec<u8>>> {
            source.iter().take(count).map(|entry| {
                total = total.checked_add(entry.data.len()).filter(|&total| total <= 256 * 1024 * 1024)
                    .ok_or_else(|| native_state_error("native 981 attached data exceeds 256 MiB"))?;
                Ok(entry.data.clone())
            }).collect()
        };
        let registrations = collect(&source_context.registrations, destination_context.registrations.len())?;
        let history = collect(&source_context.history, destination_context.history.len())?;
        let destination = self.cst_contexts.get_mut(&destination).unwrap();
        // Positional zip over BOTH tables. Names, values and string records
        // are untouched, and unmatched destination suffixes survive.
        for (entry, data) in destination.registrations.iter_mut().zip(registrations) { entry.data = data; }
        for (entry, data) in destination.history.iter_mut().zip(history) { entry.data = data; }
        Ok(NativeReturn::Continue)
    }

    pub(super) fn cst_option_native1064(&self, args: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        let handle = read_u32(args, 0)?;
        if handle == 0 { return Ok(NativeReturn::Value(0)); }
        let context = self.cst_contexts.get(&handle)
            .ok_or_else(|| native_state_error("native 1064 invalid/wrong-type CST context"))?;
        let target = read_u32(args, 4)?;
        match vm.read(target, 1) {
            Err(ScriptError::MemoryOutOfBounds { .. }) => return Ok(NativeReturn::Value(0)),
            other => { other?; }
        }
        let ordinal = read_u32(args, 8)?;
        let language = formatted(vm, read_u32(args, 12)?, 64)?;
        vm.write(target, &[0])?;
        let Some(table) = context.localization.as_ref().filter(|_|
            context.languages.binary_search_by(|name| name.as_slice().cmp(language.as_slice())).is_ok())
        else { return Ok(NativeReturn::Value(0)); };
        let Some(column) = table.languages.iter().position(|name| name.as_bytes() == language)
        else { return Ok(NativeReturn::Value(0)); };
        let count = context.document.as_ref().map_or(0, |document| document.inputs.len() as u32);
        let input = count.wrapping_sub(1).wrapping_add(ordinal);
        let mut text = Vec::new();
        if let Some(message) = table.messages.get(input as usize).and_then(|row| row.get(column)) {
            // Stop at the first empty fragment, including an initial U+001F.
            for fragment in message.content.split('\u{1f}').take_while(|fragment| !fragment.is_empty()) {
                let fragment = trim_nul(fragment.as_bytes());
                if text.len().checked_add(fragment.len()).is_none_or(|length| length >= MAX_BYTES) {
                    return Err(native_state_error("native 1064 localized option exceeds 64 MiB"));
                }
                text.extend_from_slice(fragment);
            }
        }
        write_kcs_terminated(vm, target, &text, 1)?;
        Ok(NativeReturn::Value(text.len() as u32))
    }

    pub(super) fn cst_container_native(&mut self, index: u32, args: &[u8]) -> Result<NativeReturn> {
        let handle = read_u32(args, 0)?;
        if handle == 0 {
            return Ok(if matches!(index, 979 | 980) {
                NativeReturn::Value(0)
            } else { NativeReturn::Continue });
        }
        if index == 993 {
            let source = read_u32(args, 4)?;
            if source == 0 { return Ok(NativeReturn::Continue); }
            if args.len() != 8 { return Err(native_state_error("native 993 requires two CST contexts")); }
            if !self.cst_contexts.contains_key(&handle) || !self.cst_contexts.contains_key(&source) {
                return Err(native_state_error("native 993 invalid/wrong-type CST context"));
            }
            // 0x68b877 clears the destination BEFORE enumerating the source.
            // Self-copy therefore clears the history rather than cloning it.
            let history = if source == handle { Vec::new() } else {
                self.cst_contexts[&source].history.iter().map(|entry| {
                    let mut name = [0; 32];
                    let length = entry.name.iter().position(|&byte| byte == 0)
                        .ok_or_else(|| native_state_error("native 993 source name has no terminator"))?;
                    let text = &entry.name[..length];
                    name[..text.len()].copy_from_slice(text);
                    Ok(super::CstRegistration { name, value: entry.value,
                        attachments: Vec::new(), data: Vec::new() })
                }).collect::<Result<Vec<_>>>()?
            };
            self.cst_contexts.get_mut(&handle).unwrap().history = history;
            return Ok(NativeReturn::Continue);
        }
        let context = self.cst_contexts.get(&handle)
            .ok_or_else(|| native_state_error("CST container operation has an invalid/wrong-type context"))?;
        if index == 980 {
            if args.len() != 4 { return Err(native_state_error("native 980 requires one context")); }
            let count = context.document.as_ref().map_or(0, |document| {
                document.records.iter().filter(|record| record.kind == 0x20).count() as u32
            });
            return Ok(NativeReturn::Value(count));
        }
        let value = read_u32(args, 4)?;
        if args.len() != 8 { return Err(native_state_error("native 978/979 requires two DWORDs")); }
        if index == 978 {
            if value == 0 { return Ok(NativeReturn::Continue); }
            let bytes = self.css_handles.get(&value)
                .ok_or_else(|| native_state_error("native 978 invalid/wrong-type CSS container"))?
                .snapshot().map_err(|_| native_state_error("native 978 CSS snapshot is malformed"))?;
            if let Some(last) = self.cst_contexts.get_mut(&handle).unwrap().history.last_mut() {
                // Same actual storage as 991's compressed variable chain.
                // Do not tag it or create an independent shadow attachment.
                last.data = bytes;
            }
            return Ok(NativeReturn::Continue);
        }
        if index == 979 {
            let Some(entry) = context.history.get(value as usize).filter(|entry| !entry.data.is_empty())
            else { return Ok(NativeReturn::Value(0)); };
            let file = cs2_formats::CssFile::from_snapshot(&entry.data)
                .map_err(|_| native_state_error("native 979 history attachment is not a valid CSS snapshot"))?;
            let handle = self.allocate_handle();
            self.css_handles.insert(handle, file);
            return Ok(NativeReturn::Value(handle));
        }
        Err(cs2_script::ScriptError::UnimplementedNativeCall { index })
    }

    pub(super) fn load_cst_context(&mut self, handle: u32, address: u32, vm: &Vm) -> Result<NativeReturn> {
        let name = formatted(vm, address, 260)?;
        let Some((resolved, resource)) = self.cst_resource_with_source(b"scene", &name)? else {
            return Ok(NativeReturn::Value(0));
        };
        let bytes = &resource.bytes;
        if bytes.is_empty() { return Ok(NativeReturn::Value(0)); }
        let signature = bytes.get(..8) == Some(b"CatScene");
        let valid_length = if signature && bytes.len() >= 16 {
            let packed = read_u32(bytes, 8)? as usize;
            let plain = read_u32(bytes, 12)? as usize;
            16usize.checked_add(if packed == 0 { plain } else { packed }) == Some(bytes.len())
        } else { false };
        // A wrong signature keeps the old scene; a recognized container with
        // rejected outer length clears only its scene, not its history/position.
        let mut encoding = self.text_encoding;
        let loaded = if signature && valid_length {
            let mut document = measure_host_operation(HostPhase::ScriptParse, || CstDocument::parse(bytes))?;
            let source_encoding = self.cst_scene_encoding_override(&resource);
            if let Some(source_encoding) = source_encoding {
                require_gbk_records(&document)?;
                encoding = source_encoding;
            }
            match vm.program_code_sha256().filter(|_| {
                matches!(&resource.source.location, ResourceLocation::Archive { path, .. }
                    if path.strip_prefix(self.resources.root()).ok().is_some_and(|relative|
                        relative.as_os_str().eq_ignore_ascii_case("scene.int")))
            }) {
                Some("508090f8d1f1716e69c42101e298802f5b0d706474161b4790a1a084883a2ccc")
                    if source_encoding == Some(ScriptTextEncoding::Gbk)
                        && rebellions_gbk_member_digest(&resource.source.name).is_some()
                        && let Some((member, _)) = resource.source.name.rsplit_once('.') =>
                {
                    let repairs = rebellions_date_repairs(&document);
                    repair_scene_commands(&mut document, &repairs, &member.to_ascii_uppercase(),
                        handle, self.frame, self.events);
                }
                Some("7a068090a8d37c7a446a74e98448458e3d6ed75d94bb1a00022eaeb07d7a3921")
                    if resource.source.name.eq_ignore_ascii_case("sc01.cst")
                        && matches!(self.cst_source_probe(&resource), Some(CstSourceProbe::Kanmi)) =>
                {
                    repair_scene_commands(&mut document, KANMI_POSITIONS, "sc01", handle, self.frame, self.events);
                }
                _ => {}
            }
            Some(document)
        } else { None };
        let success = loaded.is_some();
        let probe = (self.data_services.story_trace && signature && success)
            .then(|| self.cst_source_probe(&resource)).flatten();
        let happiness_probe = probe.and_then(CstSourceProbe::happiness_scene).is_some();
        let mut happiness_return = None;
        if signature {
            let happiness_member = if success && self.data_services.story_trace
                && self.data_services.particle_message_pe_sha256
                    == Some("5869b7d03a636c200f5042f0c3fc37878a362615bf724f58d47aec5eb8b2d44c")
                && matches!(&resource.source.location, ResourceLocation::Archive { path, .. }
                    if path.file_name().is_some_and(|name| name.eq_ignore_ascii_case("scene.int")))
                && (happiness_probe || resource.source.name == "sub_getatt.cst")
                && vm.program_code_sha256() == Some("4076e8aaaeda6f9f9da88731643b9e5ad32999353d9b93fd1b1282c980b77fb9")
            {
                Some(resource.source.name.as_str())
            } else {
                None
            };

            let origins = &mut self.data_services.cst_text_origins;
            let returned = origins.happiness_cst_calls.remove(&handle);
            let old_id = origins.body_loads.remove(&handle);
            let old_probe = origins.source_probes.remove(&handle);
            let caller = self.current_task_id;
            // The helper owns a separate load while the outer scene is inactive.
            if happiness_member == Some("sub_getatt.cst")
                && sha256(bytes) == "9cc32f979688a24b5c5ea1230e334d1b265b26856d0d798424c0b826f3062666"
                && returned.is_none()
                && let Some(outer) = old_probe
                && Some(outer.load_id) == old_id
                && let Some(outer_scene) = outer.probe.happiness_scene()
                && let Some(context) = self.cst_contexts.get(&handle).filter(|c| c.story.is_scene(outer_scene))
            {
                let load_id = outer.load_id;
                origins.happiness_cst_calls.insert(handle, HappinessCstCall {
                    caller, frame: self.frame, loaded_at: context.story.loaded_at(), outer,
                });
                self.events.push(LogicEvent {
                    frame: self.frame,
                    kind: "happiness-cst-helper".into(),
                    detail: format!("handle={handle} caller={caller} load_id={load_id} pc={:#x}", vm.instruction_pointer),
                });
            } else if happiness_member.is_some() && happiness_probe
                && let Some(returned) = returned
                && probe == Some(returned.outer.probe)
                && (returned.caller, returned.frame) == (caller, self.frame)
                && self.cst_contexts.get(&handle).is_some_and(|c| c.story.is_scene("sub_getatt"))
            {
                happiness_return = Some(returned);
            }
        }
        {
            let context = self.cst_contexts.get_mut(&handle)
                .ok_or_else(|| native_state_error("CST context handle is invalid"))?;
            if signature {
                context.document = loaded;
                if success {
                    context.position = [0, 0];
                    context.encoding = encoding;
                }
            }
            context.localization = None;
        }
        let neko_trace = self.trace_neko_cstl();
        let neko_archive = match &resource.source.location {
            ResourceLocation::Archive { path, .. } if neko_trace => path.file_name().and_then(|name| name.to_str()),
            _ => None,
        };
        let neko_resource = neko_archive.and_then(|archive| neko_cst_resource(archive, &resource.source.name));
        let neko_main = neko_resource.filter(|&(_, _, _, table_digest)| table_digest != "-");
        let mut neko_main_table = false;
        let mut adjacent = resolved;
        if let Some(dot) = adjacent.rfind('.').filter(|&dot| adjacent[..dot]
            .rfind(['/', '\\']).is_none_or(|slash| slash < dot))
        {
            adjacent.truncate(dot);
        }
        adjacent.push_str(".cstl");
        if let Some(SourcedResource { bytes, source }) = self.cst_read_resource(&adjacent)? {
            if !bytes.is_empty() {
                let (table, _) = measure_host_operation(HostPhase::ScriptParse, || CstlTable::parse_prefix(&bytes))
                    .map_err(|_| native_state_error("CST adjacent localization table is malformed"))?;
                if success && (neko_trace || happiness_probe) {
                    let origin = match &source.location {
                        ResourceLocation::File { .. } => source.name.replace('\\', "/"),
                        ResourceLocation::Archive { path, .. } => format!("{}::{}",
                            path.file_name().unwrap_or(path.as_os_str()).to_string_lossy(), source.name),
                    };
                    let digest = sha256(&bytes);
                    if neko_trace {
                        neko_main_table = neko_main.is_some_and(|(archive, member, _, expected)|
                            origin.eq_ignore_ascii_case(&format!("{archive}::{member}l")) && digest == expected);
                    }
                    self.events.push(LogicEvent {
                        frame: self.frame,
                        kind: if happiness_probe { "happiness-cstl-load" } else { "neko-cstl-load" }.into(),
                        detail: format!("handle={handle} requested={adjacent:?} source={origin:?} sha256={digest} columns={:?}",
                            table.languages),
                    });
                }
                self.cst_contexts.get_mut(&handle).unwrap().localization = Some(table);
            }
        }
        if self.data_services.story_trace && signature {
            let mut loaded_at = self.frame;
            let previous_neko = self.data_services.cst_text_origins.neko_scene_loads.remove(&handle);
            if success {
                if self.css_task_sha() == Some(crate::story_trace::story_trace_18::PE)
                    && resource.source.name.eq_ignore_ascii_case("0001_00.cst")
                    && matches!(&resource.source.location, ResourceLocation::Archive { path, .. }
                        if path.strip_prefix(self.resources.root()).is_ok_and(|relative|
                            relative.as_os_str().eq_ignore_ascii_case("update00.int")))
                {
                    let digest = sha256(bytes);
                    self.events.push(LogicEvent {
                        frame: self.frame,
                        kind: "campanella-cn-cst-load".into(),
                        detail: format!("handle={handle} source=update00.int::0001_00.cst sha256={digest}"),
                    });
                }
                let neko_origin = neko_resource.map(|(archive, member, expected, _)|
                    (archive, member, expected, sha256(&resource.bytes)));
                let pinned_member = neko_origin.as_ref()
                    .filter(|(_, _, expected, actual)| neko_main_table && actual == expected)
                    .map(|&(_, member, _, _)| member);
                let neko_pc = (neko_archive.is_some_and(|archive| archive.eq_ignore_ascii_case("scene.int"))
                    || neko_resource.is_some()) && vm.program_code_sha256()
                    == Some("bf9933a33675e5e44d34f7486d54aa6b53bb3f32a74c2fe0393b5a43efc6f4ad");
                let neko_pc = neko_pc.then_some(vm.instruction_pointer);
                let origins = &mut self.data_services.cst_text_origins;
                origins.body_loads.retain(|handle, _| self.cst_contexts.contains_key(handle));
                origins.source_probes.retain(|handle, _| self.cst_contexts.contains_key(handle));
                origins.neko_scene_loads.retain(|handle, _| self.cst_contexts.contains_key(handle));
                origins.happiness_cst_calls.retain(|handle, call|
                    call.frame == self.frame && self.cst_contexts.contains_key(handle));
                let preserved = pinned_member.and_then(|member| {
                    previous_neko.and_then(|scene| scene.resume(member, neko_pc, self.frame))
                });
                if preserved.is_none()
                    && let Some((_, member, _, digest)) = &neko_origin
                    && *member != "01.cst"
                {
                    self.events.push(LogicEvent {
                        frame: self.frame, kind: "neko-boundary-load".into(),
                        detail: format!("handle={handle} member={member} sha256={digest}"),
                    });
                }
                let (load_id, source_loaded_at) = happiness_return.as_ref()
                    .map(|returned| (returned.outer.load_id, returned.loaded_at))
                    .or(preserved).unwrap_or_else(|| {
                        (NEXT_BODY_LOAD.fetch_add(1, Ordering::Relaxed), self.frame)
                    });
                origins.body_loads.insert(handle, load_id);
                if let Some(member) = pinned_member {
                    loaded_at = source_loaded_at;
                    origins.neko_scene_loads.insert(handle, NekoSceneLoad { member, id: load_id, loaded_at, helper: None });
                } else if let Some((scene, name, digest)) = previous_neko
                    .and_then(|scene| scene.helper_step(neko_pc, self.frame))
                    && resource.source.name.eq_ignore_ascii_case(name)
                    && sha256(&resource.bytes) == digest
                {
                    // The helper documents keep their own read IDs through the nested call.
                    origins.neko_scene_loads.insert(handle, scene);
                    let kind = match scene.helper {
                        Some((_, NekoHelper::GetAttM2)) => "neko-cst-helper",
                        _ => "neko-cst-helper-nested",
                    };
                    self.events.push(LogicEvent {
                        frame: self.frame,
                        kind: kind.into(),
                        detail: format!("handle={handle} load_id={load_id} outer_load_id={} pc={neko_pc:?} source=\"scene.int::{name}\"", scene.id),
                    });
                }
                if let Some(returned) = happiness_return {
                    loaded_at = source_loaded_at;
                    let observations = returned.outer.records.len();
                    origins.source_probes.insert(handle, returned.outer);
                    self.events.push(LogicEvent {
                        frame: self.frame,
                        kind: "happiness-cst-return".into(),
                        detail: format!("handle={handle} caller={} load_id={load_id} observations={observations} pc={:#x}",
                            returned.caller, vm.instruction_pointer),
                    });
                } else if let Some(probe) = probe {
                    origins.source_probes.insert(handle, CstSourceProbeLoad {
                        probe, load_id, records: Vec::new(),
                    });
                }
                if matches!(probe, Some(CstSourceProbe::KanmiSc02 | CstSourceProbe::KanmiSc03)) {
                    let digest = sha256(&resource.bytes);
                    let scene = if probe == Some(CstSourceProbe::KanmiSc02) { "sc02" } else { "sc03" };
                    self.events.push(LogicEvent {
                        frame: self.frame,
                        kind: "kanmi-scene-load".into(),
                        detail: format!("handle={handle} load_id={load_id} scene={scene} sha256={digest}"),
                    });
                }
                if self.data_services.story_trace
                    && self.data_services.particle_message_pe_sha256
                        == Some("c264b2465889c78723079a694d2654ef1091253db657a735e92f58b75031aa26")
                    && matches!(&resource.source.location, ResourceLocation::Archive { path, .. }
                        if path.file_name().is_some_and(|name| name.eq_ignore_ascii_case("scene.int")))
                {
                    let member = resource.source.name.to_ascii_lowercase();
                    let digest = match member.as_str() {
                        "sc02_01.cst" => Some("6643eaccfaf2898448a8bfbe3527916d1980b4111658502d6f47dd1b23f7958d"),
                        "sc02_02.cst" => Some("57471b4c2cbafeeaf7d2e02bc7a2a5bf1685ace512636e77baea973523b2fd87"),
                        _ => None,
                    };
                    if let Some(digest) = digest.filter(|digest| sha256(&resource.bytes) == *digest) {
                        let scene = member.trim_end_matches(".cst");
                        self.events.push(LogicEvent {
                            frame: self.frame, kind: "kanmi-op-branch-load".into(),
                            detail: format!("scene={scene} handle={handle} load_id={load_id} sha256={digest}"),
                        });
                    }
                }
                if matches!(probe, Some(CstSourceProbe::RebellionsSecond | CstSourceProbe::RebellionsThird | CstSourceProbe::RebellionsLater)) {
                    let digest = sha256(bytes);
                    let member = &resource.source.name;
                    let scene = member.get(..member.len().saturating_sub(4))
                        .unwrap_or_default().to_ascii_uppercase();
                    self.events.push(LogicEvent {
                        frame: self.frame,
                        kind: "rebellions-scene-load".into(),
                        detail: format!("handle={handle} load_id={load_id} scene={scene} sha256={digest}"),
                    });
                }
                if let Some((archive, member, _, digest)) = neko_origin.filter(|_| neko_main.is_some()) {
                    self.events.push(LogicEvent {
                        frame: self.frame,
                        kind: "neko-cst-origin".into(),
                        detail: format!("handle={handle} load_id={load_id} source=\"{archive}::{member}\" sha256={digest}"),
                    });
                    self.events.push(LogicEvent {
                        frame: self.frame,
                        kind: "neko-cst-reload".into(),
                        detail: format!("handle={handle} load_id={load_id} pc={neko_pc:?} preserved={}",
                            preserved.is_some()),
                    });
                }
            }
            let context = self.cst_contexts.get_mut(&handle).unwrap();
            context.story = if success {
                // Re-probe the wait marker. A verified reload keeps the birth frame of the
                // body load, which may already have supplied text before the helper ran.
                super::story_trace::CstStoryPosition::loaded(&name, loaded_at)
            } else {
                Default::default()
            };
        }
        Ok(NativeReturn::Value(u32::from(success)))
    }

    pub(super) fn read_cst_record(
        &mut self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let handle = read_u32(args, 0)?;
        let context = self.cst_contexts.get(&handle)
            .ok_or_else(|| native_state_error("CST context handle is invalid"))?;
        let target = read_u32(args, 4)?;
        // Ordinary start rejection returns zero without touching the buffer;
        // shared storage lock failures remain real diagnostics.
        match vm.read(target, 1) {
            Err(ScriptError::MemoryOutOfBounds { .. }) => return Ok(NativeReturn::Value(0)),
            other => { other?; }
        }
        let input = signed_cst_index(read_u32(args, 8)?, context.position[0]);
        let ordinal = signed_cst_index(read_u32(args, 12)?, context.position[1]);
        let Some(document) = &context.document else { return Ok(NativeReturn::Value(0)); };
        let Some(records) = usize::try_from(input).ok().and_then(|input| document.input_records(input))
        else { return Ok(NativeReturn::Value(0)); };
        let Some(record) = usize::try_from(ordinal).ok().and_then(|ordinal| records.get(ordinal))
        else { return Ok(NativeReturn::Value(0)); };
        let language = if matches!(index, 1033 | 1038) {
            Some(formatted(vm, read_u32(args, 16)?, 64)?)
        } else { None };
        let bound = language.as_deref().and_then(|language| {
            if context.languages.binary_search_by(|name| name.as_slice().cmp(language)).is_err() {
                return None;
            }
            let table = context.localization.as_ref()?;
            let column = table.languages.iter().position(|name| name.as_bytes() == language)?;
            Some((table, column))
        });
        let localized = bound.filter(|_| matches!(record.kind, 0x20 | 0x21));
        let codec = if localized.is_some() || matches!(index, 1037 | 1038) {
            ScriptTextEncoding::Utf8
        } else {
            context.encoding
        };
        let text = if let Some((table, column)) = localized {
            let message = table.messages.get(input as usize).and_then(|row| row.get(column));
            if record.kind == 0x21 {
                message.map_or(&b""[..], |message| message.name.as_bytes()).to_vec()
            } else {
                let fragment = records[..ordinal as usize].iter().filter(|r| r.kind == 0x20).count();
                message.and_then(|message| message.content.split('\u{1f}').nth(fragment))
                    .unwrap_or("").as_bytes().to_vec()
            }
        } else if matches!(index, 1037 | 1038) {
            if matches!(record.kind, 0x20 | 0x21) {
                decode_cst_dialogue(&record.text, context.encoding).map_err(|error|
                    native_state_error_owned(format!("native {index} input={input} record={ordinal}: {error}")))?
                    .into_owned().into_bytes()
            } else {
                context.encoding.decode(&record.text).into_bytes()
            }
        } else { record.text.clone() };
        let text = trim_nul(&text);
        write_kcs_terminated(vm, target, text, 1)?;
        if self.data_services.story_trace && index == 327 && record.kind == 0x30 {
            let repaired = match target {
                0x65c72 if context.encoding == ScriptTextEncoding::Gbk
                    && context.story.scene_name().get(..2).is_some_and(|prefix| prefix.eq_ignore_ascii_case("A_"))
                    && REBELLIONS_DATE_CONDITIONS.iter().any(|(_, repaired)| text == *repaired) =>
                {
                    Some((context.story.scene_name(), document.inputs[input as usize].start + ordinal as usize))
                }
                0x797f0 if context.story.is_scene("sc01") => KANMI_POSITIONS.iter()
                    .find(|(group, record, _, _, repaired)|
                        *group as i32 == input && *record as i32 == ordinal && text == *repaired)
                    .map(|(_, _, absolute, _, _)| ("sc01", *absolute)),
                _ => None,
            };
            if let Some((scene, absolute)) = repaired {
                self.events.push(LogicEvent { frame: self.frame, kind: "cst-command-compat-read".into(),
                    detail: format!("scene={scene} handle={handle} group={input} record={absolute} native={index} ordinal={ordinal} command={:?}",
                        String::from_utf8_lossy(text)),
                });
            }
        }
        let trace_neko_group = (0..=2).contains(&input)
            || context.story.is_scene("01") && ((3..=35).contains(&input)
                || (36..=491).contains(&input) && record.kind == 0x20 && target == 0x00062c6b)
            || (context.story.is_scene("01_02") || context.story.is_scene("H01") || context.story.is_scene("h01")
                || context.story.is_scene("02"))
                && record.kind == 0x20 && target == 0x00062c6b;
        let trace_happiness_group = (0..=2).contains(&input)
            && self.data_services.cst_text_origins.source_probes.get(&handle)
                .and_then(|load| load.probe.happiness_scene()).is_some();
        if index == 1038 && matches!(record.kind, 0x20 | 0x21)
            && (trace_happiness_group || trace_neko_group && self.trace_neko_cstl())
        {
            let languages = &self.data_services.language.languages;
            let ui = languages.prefix(0).map(String::from_utf8_lossy);
            let main = languages.prefix(1).map(String::from_utf8_lossy);
            let requested = language.as_deref().map(String::from_utf8_lossy);
            let column = localized.map(|(_, column)| column);
            let source = if column.is_some() { "cstl" } else { "raw-cst" };
            self.events.push(LogicEvent {
                frame: self.frame,
                kind: if trace_happiness_group { "happiness-cstl-read" } else { "neko-cstl-read" }.into(),
                detail: format!("handle={handle} input={input} ordinal={ordinal} kind={:#04x} destination={target:#010x} ui={ui:?} main={main:?} requested={requested:?} source={source} column={column:?} bytes={} sha256={}",
                    record.kind, text.len(), sha256(text)),
            });
        }
        if matches!(record.kind, 0x20 | 0x21) {
            let main_prefix = self.data_services.language.languages.prefix(1);
            let origins = &mut self.data_services.cst_text_origins;
            let load = origins.body_loads.get(&handle).copied().filter(|_| {
                record.kind == 0x20 && !body_control_only(text)
                    && language.as_deref().is_none_or(|requested|
                        main_prefix == Some(requested)
                            || (main_prefix.is_none() && requested.is_empty() && localized.is_none()))
            });
            let absolute_record = document.inputs[input as usize].start + ordinal as usize;
            if let Some(load_id) = load {
                origins.record_with_body(self.current_task_id, target, text, codec, vec![CstBodySource {
                    context_handle: handle, load_id, group: input, absolute_record,
                }]);
            } else {
                origins.record_with_body(self.current_task_id, target, text, codec, Vec::new());
            }
            if let Some(&(probe, load_id)) = origins.source_probes.get(&handle)
                .map(|load| (load.probe, load.load_id)).as_ref()
            {
                let observation = CstSourceRecordObservation {
                    source: CstBodySource { context_handle: handle, load_id, group: input, absolute_record },
                    frame: self.frame, probe, native: index, kind: record.kind,
                    stored_codec: context.encoding, output_codec: codec,
                    raw_bytes: record.text.len(), output_bytes: text.len(),
                    body_tagged: load == Some(load_id), localized: localized.is_some(),
                };
                if origins.note_source_record(observation) {
                    self.events.push(LogicEvent {
                        frame: self.frame, kind: "cst-source-record".into(), detail: format!("{observation:?}"),
                    });
                }
            }
        } else {
            self.data_services.cst_text_origins.invalidate_returned(self.current_task_id, target);
        }
        Ok(NativeReturn::Value(text.len() as u32))
    }

    pub(super) fn trace_neko_cstl(&self) -> bool {
        self.data_services.story_trace
            && self.data_services.native_profile.is_some_and(|profile| profile.pe_sha256 == NEKOPARA_PE_SHA256)
    }

    pub(super) fn cst_snapshot_native(
        &mut self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let handle = read_u32(args, 0)?;
        if handle == 0 {
            return Ok(if index == 992 { NativeReturn::Value(0) } else { NativeReturn::Continue });
        }
        if !self.cst_contexts.contains_key(&handle) {
            return Err(native_state_error("CST snapshot context handle is invalid"));
        }
        let expected = if index == 992 { 16 } else { 12 };
        if args.len() != expected { return Err(native_state_error("CST snapshot argument count is invalid")); }
        if index == 994 {
            return self.trim_cst_snapshots(handle, read_u32(args, 4)?, read_u32(args, 8)?);
        }
        if index == 991 && self.cst_contexts[&handle].history.is_empty() {
            return Ok(NativeReturn::Continue);
        }
        let (requested, position, outputs) = if index == 992 {
            let history_address = read_u32(args, 4)?;
            let position_address = read_u32(args, 8)?;
            (vm.read_u32(history_address)?, vm.read_u32(position_address)?,
                Some((history_address, position_address)))
        } else { (0, read_u32(args, 4)?, None) };
        let config = formatted(vm, read_u32(args, if index == 992 { 12 } else { 8 })?, 1024)?;
        let strings = self.cst_snapshot_string_ids(&config)?;
        if index == 991 {
            self.ensure_fes_settings()
                .map_err(|_| native_state_error("CST snapshot flag settings are unavailable"))?;
            let mut numbers = Vec::new();
            for [start, count] in self.data_services.fes_settings.cst_local_flag_ranges() {
                let mut id = start;
                let end = start.wrapping_add(count) as i32;
                while (id as i32) < end {
                    if numbers.len() >= MAX_ITEMS {
                        return Err(native_state_error("CST snapshot numeric ranges exceed one million values"));
                    }
                    let value = self.shared_values_for(self.current_task_id)
                        .and_then(|values| values.get(id as usize)).copied().unwrap_or(0);
                    numbers.push((id, value));
                    id = id.wrapping_add(1);
                }
            }
            let strings = strings.into_iter().map(|id| {
                (id, self.string_values.get(&id).cloned().unwrap_or_default())
            }).collect::<BTreeMap<_, _>>();
            let context = self.cst_contexts.get_mut(&handle).unwrap();
            let last = context.history.last_mut().unwrap();
            let data = prepend_history_snapshot(&HistorySnapshot { position, numbers, strings }, &last.data)?;
            last.data = data;
            return Ok(NativeReturn::Continue);
        }
        if (requested as i32) < 0 { return Ok(NativeReturn::Value(0)); }
        let history = &self.cst_contexts[&handle].history;
        let mut found = None;
        if !history.is_empty() {
            for actual in (0..=usize::min(requested as usize, history.len() - 1)).rev() {
                let blocks = history_blocks(&history[actual].data)?;
                if let Some(block) = blocks.into_iter().find(|block| {
                    actual as u32 != requested || block.position as i32 <= position as i32
                }) {
                    found = Some((actual as u32, block));
                    break;
                }
            }
        }
        let Some((actual, block)) = found else { return Ok(NativeReturn::Value(0)); };
        let (history_address, position_address) = outputs.unwrap();
        // Preserve the original output order, including overlapping outputs,
        // before applying saved numeric and configured string variables.
        vm.write(history_address, &actual.to_le_bytes())?;
        vm.write(position_address, &block.position.to_le_bytes())?;
        let snapshot = block.decode()?;
        if let Some(values) = self.shared_values_for_mut(self.current_task_id) {
            for (id, value) in snapshot.numbers {
                if let Some(target) = values.get_mut(id as usize) { *target = value; }
            }
        }
        for id in strings {
            self.data_services.cst_text_origins.invalidate_shared(id);
            self.string_values.insert(id, snapshot.strings.get(&id).cloned().unwrap_or_default());
        }
        Ok(NativeReturn::Value(1))
    }

    fn trim_cst_snapshots(&mut self, handle: u32, requested: u32, position: u32) -> Result<NativeReturn> {
        let context = self.cst_contexts.get_mut(&handle).unwrap();
        if !context.history.is_empty() {
            let selected = usize::min(requested as usize, context.history.len() - 1);
            let data = &context.history[selected].data;
            if let Some(block) = history_blocks(data)?.into_iter()
                .find(|block| (block.position as i32) < position as i32)
            {
                if block.offset != 0 {
                    let remaining = data[block.offset..].to_vec();
                    // 0x509260 updates the LAST history item before truncation.
                    context.history.last_mut().unwrap().data = remaining;
                }
            }
            context.history[selected].value = position;
        }
        let delete = requested.wrapping_add(1);
        if delete != u32::MAX {
            let start = if (delete as i32) < 0 { 0 } else { delete as usize };
            context.history.truncate(start);
        }
        Ok(NativeReturn::Continue)
    }

    fn cst_snapshot_string_ids(&self, name: &[u8]) -> Result<Vec<u32>> {
        let Some((_, bytes)) = self.cst_resource(b"config", name)? else { return Ok(Vec::new()); };
        if bytes.is_empty() { return Ok(Vec::new()); }
        let xml = ConfigXml::parse_runtime(&bytes)
            .map_err(|_| native_state_error("CST snapshot configuration XML is malformed"))?;
        let text = trim_nul(xml.text(&[xml.root.as_slice(), b"localstr"]).unwrap_or_default());
        if text.len() >= 2048 { return Err(native_state_error("CST snapshot localstr exceeds 2047 bytes")); }
        Ok(text.split(|&byte| byte == b',').filter(|part| !part.is_empty())
            .map(|part| integer_prefix(part).map_or(0, |(_, value)| value)).collect())
    }

    pub(super) fn cst_resource(&self, category: &[u8], name: &[u8]) -> Result<Option<(String, Vec<u8>)>> {
        Ok(self.cst_resource_with_source(category, name)?
            .map(|(name, resource)| (name, resource.bytes)))
    }

    pub(super) fn cst_scene_encoding_override(&self, resource: &SourcedResource) -> Option<ScriptTextEncoding> {
        let gbk_archive = matches!(&resource.source.location, ResourceLocation::Archive { path, .. }
            if self.resources.resource_router().is_some_and(|router|
                router.is_island_chinese_scene(self.resources, path)
                    || router.is_daydream_gbk_scene_archive(self.resources, path)));
        (gbk_archive || verified_gbk_scene(self.resources.root(), resource)
            || self.data_services.newton_legacy_gbk && newton_loose_scene(self.resources.root(), resource))
            .then_some(ScriptTextEncoding::Gbk)
    }

    pub(super) fn cst_resource_with_source(&self, category: &[u8], name: &[u8]) -> Result<Option<(String, SourcedResource)>> {
        let name = ScriptTextEncoding::Cp932.decode(name);
        let candidates = self.resources.category_names(category, &name)
            .map_err(|_| native_state_error("CST resource category resolution failed"))?;
        for name in candidates {
            if let Some(resource) = self.cst_read_resource(&name)? { return Ok(Some((name, resource))); }
        }
        Ok(None)
    }

    fn cst_read_resource(&self, name: &str) -> Result<Option<SourcedResource>> {
        let _timing = measure_host_phase(HostPhase::ScriptRead);
        if let Some(resource) = self.data_services.ordinary.cached_resource_with_source(name) {
            if resource.bytes.len() > MAX_BYTES { return Err(native_state_error("CST cached resource exceeds 64 MiB")); }
            return Ok(Some(resource.clone()));
        }
        let Some(found) = self.resources.resolve_mode_utf16(
            &name.encode_utf16().collect::<Vec<_>>(), ResourceMode::LooseFirst,
        ).map_err(|_| native_state_error("CST resource routing failed"))? else { return Ok(None); };
        if found.value.size > MAX_BYTES as u64 { return Err(native_state_error("CST resource exceeds 64 MiB")); }
        let bytes = self.resources.read_candidate(found.value)
            .map_err(|_| native_state_error("CST resource read failed"))?;
        if bytes.len() > MAX_BYTES { return Err(native_state_error("CST resource exceeds 64 MiB")); }
        Ok(Some(SourcedResource { source: found.value.clone(), bytes }))
    }
}

#[cfg(test)]
mod source_encoding_tests {
    use super::*;
    use cs2_script::{CstRecord, Host};

    #[test]
    fn neko_scene_body_provenance_follows_only_verified_reload_edges() {
        let scene = NekoSceneLoad { member: "01.cst", id: 3, loaded_at: 1500, helper: None };
        assert_eq!(scene.resume("01.cst", Some(0x14464), 1536), Some((3, 1500)));
        assert_eq!(scene.resume("02.cst", Some(0x14464), 1536), None);
        assert_eq!(scene.resume("01.cst", Some(0x141e0), 1536), None);
        assert_eq!(scene.resume("01.cst", Some(0xb1a58), 1536), None);
        let (helper, name, _) = scene.helper_step(Some(0x14e91), 1536).unwrap();
        assert_eq!(name, "sub_getattm2.cst");
        assert_eq!(helper.resume("01.cst", Some(0x14f97), 1536), None);
        assert_eq!(helper.resume("01.cst", Some(0x15178), 1536), Some((3, 1500)));
        assert_eq!(helper.resume("02.cst", Some(0x15178), 1536), None);
        assert_eq!(helper.resume("01.cst", Some(0x15178), 1537), None);
        assert_eq!(helper.resume("01.cst", Some(0x14464), 1536), None);
        assert_eq!(helper.resume("01.cst", None, 1536), None);
        assert!(helper.helper_step(Some(0x14e91), 1537).is_none());
        let (nested, name, _) = helper.helper_step(Some(0x14e91), 1536).unwrap();
        assert_eq!(name, "sub_getatt.cst");
        assert_eq!(nested.resume("01.cst", Some(0x14f97), 1536), None);
        assert!(nested.helper_step(Some(0x14e91), 1536).is_none());
        assert!(nested.helper_step(Some(0x14f97), 1537).is_none());
        let (returned, name, _) = nested.helper_step(Some(0x14f97), 1536).unwrap();
        assert_eq!(name, "sub_getattm2.cst");
        assert_eq!(returned.resume("01.cst", Some(0x14f97), 1536), Some((3, 1500)));
        assert_eq!(returned.resume("01.cst", Some(0x14f97), 1537), None);
    }

    #[test]
    fn august_source_observation_keeps_two_pages_of_legacy_reads_from_the_live_load() {
        let mut origins = CstTextOrigins::default();
        let context_handle = 139;
        let load_id = 3;
        let probe = CstSourceProbe::August;
        origins.body_loads.insert(context_handle, load_id);
        origins.source_probes.insert(context_handle, CstSourceProbeLoad {
            probe, load_id, records: Vec::new(),
        });
        let speaker = CstSourceRecordObservation {
            source: CstBodySource { context_handle, load_id, group: 0, absolute_record: 78 },
            frame: 1309, probe, native: 1037, kind: 0x21,
            stored_codec: ScriptTextEncoding::Cp932, output_codec: ScriptTextEncoding::Utf8,
            raw_bytes: 12, output_bytes: 18, body_tagged: false, localized: false,
        };
        let first_body = CstSourceRecordObservation {
            source: CstBodySource { absolute_record: 79, ..speaker.source },
            kind: 0x20, raw_bytes: 58, output_bytes: 87, body_tagged: true, ..speaker
        };
        let second_body = CstSourceRecordObservation {
            source: CstBodySource { absolute_record: 80, ..first_body.source },
            output_bytes: 86, ..first_body
        };
        assert!(!origins.note_source_record(CstSourceRecordObservation { native: 327, ..first_body }));
        let successor = CstSourceRecordObservation {
            source: CstBodySource { group: 1, absolute_record: 84, ..first_body.source },
            frame: 1500, ..first_body
        };
        assert!(!origins.note_source_record(CstSourceRecordObservation {
            source: CstBodySource { absolute_record: 86, ..successor.source }, ..successor
        }));
        for observation in [speaker, first_body, second_body, successor] {
            assert!(origins.note_source_record(observation));
        }
        let repeat = CstSourceRecordObservation { frame: 1310, ..first_body };
        assert!(!origins.note_source_record(repeat));
        assert_eq!(origins.source_record_observations(context_handle), &[speaker, first_body, second_body, successor]);
        origins.body_loads.insert(context_handle, load_id + 1);
        assert!(origins.source_record_observations(context_handle).is_empty());
        assert!(!origins.note_source_record(second_body));
    }

    #[test]
    fn cst_text_origin_is_scoped_to_the_producing_task_and_current_bytes() {
        let mut origins = CstTextOrigins::default();
        let gbk = b"\xb7\xc7";
        let utf8 = b"\xe9\x9d\x9e";
        origins.record(4, 0x100, gbk, ScriptTextEncoding::Gbk);
        origins.publish(4, 0x100, 1000, gbk);
        assert_eq!(origins.shared(1000, gbk), Some(ScriptTextEncoding::Gbk));
        assert_eq!(origins.shared(1000, utf8), None);
        origins.publish(5, 0x100, 1000, gbk);
        assert_eq!(origins.shared(1000, gbk), None);
        origins.record(4, 0x100, utf8, ScriptTextEncoding::Utf8);
        origins.publish(4, 0x100, 1000, utf8);
        assert_eq!(origins.shared(1000, utf8), Some(ScriptTextEncoding::Utf8));
        origins.invalidate_returned(4, 0x100);
        origins.publish(4, 0x100, 1000, utf8);
        assert_eq!(origins.shared(1000, utf8), None);
    }

    #[test]
    fn cst_dialogue_decodes_ambiguous_bytes_only_with_declared_codec() {
        let bytes = b"\xc2\xa9\0\x81";
        assert_eq!(decode_cst_dialogue(bytes, ScriptTextEncoding::Cp932).unwrap(), "\u{ff82}\u{ff69}");
        assert_eq!(decode_cst_dialogue(bytes, ScriptTextEncoding::Gbk).unwrap(), "\u{6f0f}");
        assert_eq!(decode_cst_dialogue(bytes, ScriptTextEncoding::Utf8).unwrap(), "\u{a9}");
        for codec in [ScriptTextEncoding::Cp932, ScriptTextEncoding::Gbk, ScriptTextEncoding::Utf8] {
            assert!(decode_cst_dialogue(b"\x81", codec).is_err());
        }
    }

    #[test]
    fn cst_text_origins_survive_message_window_cache_republication() -> Result<()> {
        let task = 8;
        let input = 0x8000_0100;
        let cache = 0x8000_0200;
        let output = 0x8000_0300;
        let mut vm = Vm::new();
        vm.bind_external(vec![0; 4096])?;
        for codec in [ScriptTextEncoding::Utf8, ScriptTextEncoding::Gbk] {
            let mut origins = CstTextOrigins::default();
            let body = b"\xc2\xa9";
            vm.write(input, b"\xc2\xa9\0")?;
            vm.write(cache, b"\\n\\pc\0")?;
            origins.record(task, input, body, codec);
            let append = origins.capture_raw_copy(task, cache, input, true, &vm);
            let combined = b"\\n\\pc\xc2\xa9";
            vm.write(cache, b"\\n\\pc\xc2\xa9\0")?;
            origins.finish_copy(task, cache, append, &vm);

            origins.publish(task, input, 1000, b"");
            let copy = origins.capture_raw_copy(task, output, cache, false, &vm);
            vm.write(output, b"\\n\\pc\xc2\xa9\0")?;
            origins.finish_copy(task, output, copy, &vm);
            origins.publish(task, output, 1000, combined);
            assert_eq!(origins.shared(1000, combined), Some(codec));
            let decoded = crate::fes_text::decode_sourced_text(b"$str1000", combined,
                ScriptTextEncoding::Cp932, ScriptTextEncoding::Cp932, false,
                |id| (combined, origins.shared(id, combined))).unwrap().unwrap();
            let expected = if codec == ScriptTextEncoding::Utf8 { "\u{a9}" } else { "\u{6f0f}" };
            assert_eq!(decoded, format!("\\n\\pc{expected}"));

            assert!(origins.capture_raw_copy(task + 1, output, cache, false, &vm).is_none());
            let copy = origins.capture_raw_copy(task, output, cache, false, &vm);
            vm.write(output, b"changed\0")?;
            origins.finish_copy(task, output, copy, &vm);
            assert_eq!(origins.returned(task, output, combined), None);
            origins.record(task, input, body, ScriptTextEncoding::Cp932);
            assert!(origins.capture_raw_copy(task, cache, input, true, &vm).is_none());
        }
        Ok(())
    }

    #[test]
    fn cst_body_origins_bind_ordered_publish_to_the_current_load() -> Result<()> {
        let task = 8;
        let first = 0x8000_0100;
        let second = 0x8000_0200;
        let cache = 0x8000_0300;
        let mut vm = Vm::new();
        vm.bind_external(vec![0; 4096])?;
        let mut origins = CstTextOrigins::default();
        let first_tag = CstBodySource { context_handle: 9, load_id: 42, group: 1, absolute_record: 20 };
        let second_tag = CstBodySource { absolute_record: 21, ..first_tag };
        origins.body_loads.insert(first_tag.context_handle, first_tag.load_id);
        for (address, text, written, tag) in [
            (first, b"ABC".as_slice(), b"ABC\0".as_slice(), first_tag),
            (second, b"DEF", b"DEF\0", second_tag),
        ] {
            vm.write(address, written)?;
            origins.record_with_body(task, address, text, ScriptTextEncoding::Utf8, vec![tag]);
            origins.record(task, address, text, ScriptTextEncoding::Utf8);
        }
        vm.write(cache, b"\\r\\pc\0")?;
        let copy = origins.capture_raw_copy(task, cache, first, true, &vm);
        vm.write(cache, b"\\r\\pcABC\0")?;
        origins.finish_copy(task, cache, copy, &vm);
        let copy = origins.capture_raw_copy(task, cache, second, true, &vm);
        vm.write(cache, b"\\r\\pcABCDEF\0")?;
        origins.finish_copy(task, cache, copy, &vm);
        let combined = b"\\r\\pcABCDEF";
        origins.publish(task, cache, 1000, combined);
        assert_eq!(origins.shared_body(1000, combined), Some([first_tag, second_tag].as_slice()));
        assert_eq!(origins.shared(1000, combined), None);
        origins.publish(task + 1, cache, 1000, combined);
        assert!(origins.shared_body(1000, combined).is_none());
        origins.publish(task, cache, 1000, combined);
        origins.body_loads.insert(first_tag.context_handle, 43);
        assert!(origins.shared_body(1000, combined).is_none());
        Ok(())
    }

    #[test]
    fn css_shared_string_restore_keeps_live_cst_loads_for_new_reads() {
        let mut origins = CstTextOrigins::default();
        let tag = CstBodySource { context_handle: 9, load_id: 42, group: 24, absolute_record: 216 };
        let bytes = b"\xc2\xa9";
        origins.body_loads.insert(tag.context_handle, tag.load_id);
        origins.record_with_body(8, 0x100, bytes, ScriptTextEncoding::Utf8, vec![tag]);
        origins.publish(8, 0x100, 1000, bytes);
        assert_eq!(origins.shared_body(1000, bytes), Some([tag].as_slice()));

        origins.clear_after_shared_strings_restore();
        assert!(origins.returned(8, 0x100, bytes).is_none());
        assert!(origins.shared(1000, bytes).is_none());
        assert!(origins.shared_body(1000, bytes).is_none());
        assert!(origins.body_is_current(&[tag]));

        origins.record_with_body(8, 0x100, bytes, ScriptTextEncoding::Utf8, vec![tag]);
        origins.publish(8, 0x100, 1000, bytes);
        assert_eq!(origins.shared_body(1000, bytes), Some([tag].as_slice()));
    }

    #[test]
    fn cst_body_origins_drop_replaced_expansions_and_codec_changes() {
        let mut origins = CstTextOrigins::default();
        let tag = CstBodySource { context_handle: 9, load_id: 42, group: 0, absolute_record: 17 };
        origins.body_loads.insert(tag.context_handle, tag.load_id);
        for (input, output, codec) in [
            (b"$str1000".as_slice(), b"$str1000".as_slice(), ScriptTextEncoding::Utf8),
            (b"A$str1000", b"ABC", ScriptTextEncoding::Utf8),
            (b"ABC", b"ABC", ScriptTextEncoding::Cp932),
        ] {
            origins.record_with_body(8, 0x100, input, ScriptTextEncoding::Utf8, vec![tag]);
            origins.record(8, 0x100, output, codec);
            origins.publish(8, 0x100, 1000, output);
            assert!(origins.shared_body(1000, output).is_none());
        }
    }

    #[test]
    fn sidecar9_verified_codec_validation_is_strict() {
        let mut document = CstDocument {
            inputs: vec![0..1],
            records: vec![CstRecord { kind: 0x20, text: vec![0xb7, 0xc7] }],
        };
        assert!(require_gbk_records(&document).is_ok());
        assert_eq!(ScriptTextEncoding::Gbk.decode(&document.records[0].text), "\u{975e}");
        let cp932 = encoding_rs::SHIFT_JIS.decode_without_bom_handling_and_without_replacement(&document.records[0].text).unwrap();
        assert_ne!(cp932, "\u{975e}");
        document.records[0].text.pop();
        assert!(require_gbk_records(&document).is_err());
    }

    fn args(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    #[test]
    #[ignore = "requires the installed Grisaia Eden V1 patch and an isolated profile path"]
    fn grisaia_eden_blaop1_end_publishes_gbk_name_and_body() -> anyhow::Result<()> {
        let game = std::env::var_os("CS2_EDEN_TEXT_TEST_ROOT").expect("installed game root required");
        let profile = std::env::var_os("CS2_EDEN_TEXT_TEST_PROFILE").expect("isolated profile required");
        let mut player = crate::HeadlessPlayer::open_with_save_root(
            game, Path::new(&profile).join("eden-blaop1-end"),
        )?;
        let mut vm = Vm::new();
        vm.bind_external(vec![0; 4096])?;
        let scene = b"BLAOP1_END.cst";
        let scene_name = 0x8000_0100;
        let destination = 0x8000_0200;
        vm.write(scene_name, b"BLAOP1_END.cst\0")?;
        player.with_host(|host| {
            let (_, winner) = host.cst_resource_with_source(b"scene", scene)?.unwrap();
            assert!(verified_gbk_scene(host.resources.root(), &winner));
            let mut changed = winner.clone();
            changed.bytes[0] ^= 1;
            assert!(!verified_gbk_scene(host.resources.root(), &changed));
            changed.source.location = ResourceLocation::File {
                path: host.resources.root().join("scene/BLAOP1_END.cst"),
            };
            changed.bytes = winner.bytes;
            assert!(!verified_gbk_scene(host.resources.root(), &changed));

            let NativeReturn::Value(handle) = host.call(323, &[], &mut vm)? else {
                anyhow::bail!("no CST context handle");
            };
            assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
            assert_eq!(host.cst_contexts[&handle].encoding, ScriptTextEncoding::Gbk);
            for (ordinal, id, kind, length) in [(10, 1001, 0x21, 4), (11, 1000, 0x20, 97)] {
                let packet = args(&[handle, destination, 4, ordinal]);
                let record = &host.cst_contexts[&handle].document.as_ref().unwrap()
                    .input_records(4).unwrap()[ordinal as usize];
                assert_eq!(record.kind, kind);
                assert_eq!(record.text.len(), length);
                let expected = decode_cst_dialogue(&record.text, ScriptTextEncoding::Gbk)?.into_owned();
                if kind == 0x20 {
                    assert!(decode_cst_dialogue(&record.text, ScriptTextEncoding::Cp932).is_err());
                }
                host.read_cst_record(327, &packet, &mut vm)?;
                host.call(276, &args(&[id, destination]), &mut vm)?;
                let raw = vm.read_string(destination)?;
                assert_eq!(host.data_services.cst_text_origins.shared(id, &raw), Some(ScriptTextEncoding::Gbk));
                host.read_cst_record(1037, &packet, &mut vm)?;
                assert_eq!(vm.read_string(destination)?, expected.as_bytes());
            }
            Ok(())
        })
    }

    #[test]
    #[ignore = "requires the installed Rebellions A-line patch and an isolated profile path"]
    fn rebellions_a01_gbk_uses_exact_winning_member() -> anyhow::Result<()> {
        let game = std::env::var_os("CS2_REBELLIONS_TEST_GAME").expect("installed game root required");
        let profile = std::env::var_os("CS2_SIDECAR9_PROFILE").expect("isolated profile required");
        let mut player = crate::HeadlessPlayer::open_with_save_root(
            game, Path::new(&profile).join("rebellions-a01-encoding"),
        )?;
        let mut vm = Vm::new();
        vm.bind_external(vec![0; 4096])?;
        let scene_name = 0x8000_0100;
        let destination = 0x8000_0200;
        player.with_host(|host| {
            let NativeReturn::Value(handle) = host.call(323, &[], &mut vm)? else {
                anyhow::bail!("no CST context handle");
            };
            let (_, patched) = host.cst_resource_with_source(b"scene", b"A_01_01.cst")?.unwrap();
            let (_, later) = host.cst_resource_with_source(b"scene", b"A_01_03.cst")?.unwrap();
            let (_, japanese) = host.cst_resource_with_source(b"scene", b"C_01_01.cst")?.unwrap();
            assert_eq!(host.cst_scene_encoding_override(&patched), Some(ScriptTextEncoding::Gbk));
            assert_eq!(host.cst_scene_encoding_override(&later), Some(ScriptTextEncoding::Gbk));
            assert_eq!(host.cst_scene_encoding_override(&japanese), None);

            let mut same_name = patched.clone();
            same_name.bytes = japanese.bytes;
            assert_eq!(host.cst_scene_encoding_override(&same_name), None);
            let mut loose = patched;
            loose.source.location = ResourceLocation::File {
                path: host.resources.root().join("scene/A_01_01.cst"),
            };
            assert_eq!(host.cst_scene_encoding_override(&loose), None);

            for (name, ordinal, codec, expected) in [
                (b"A_01_01.cst\0", 12, ScriptTextEncoding::Gbk,
                    "\u{2015}\u{2015}\u{4ed6}\u{505a}\u{4e86}\u{4e00}\u{4e2a}\u{68a6}\u{3002}"),
                (b"C_01_01.cst\0", 16, ScriptTextEncoding::Cp932,
                    "\u{2015}\u{2015}\u{5f7c}\u{306f}\u{3001}\u{5922}\u{3092}\u{898b}\u{3066}\u{3044}\u{305f}\u{3002}"),
            ] {
                vm.write(scene_name, name)?;
                assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
                assert_eq!(host.cst_contexts[&handle].encoding, codec);
                let read_args = args(&[handle, destination, 0, ordinal]);
                host.read_cst_record(327, &read_args, &mut vm)?;
                let raw = vm.read_string(destination)?;
                host.call(278, &args(&[destination]), &mut vm)?;
                host.call(276, &args(&[1000, destination]), &mut vm)?;
                assert_eq!(host.data_services.cst_text_origins.shared(1000, &raw), Some(codec));
                host.read_cst_record(1037, &read_args, &mut vm)?;
                assert_eq!(vm.read_string(destination)?, expected.as_bytes());
            }
            Ok(())
        })
    }

    #[test]
    #[ignore = "requires the pinned installed Cyanotype game and an isolated profile path"]
    fn cyanotype_speaker_copy_publishes_utf8_to_fes() -> anyhow::Result<()> {
        let game = std::env::var_os("CS2_SIDECAR9_CYANOTYPE").expect("installed game root required");
        let profile = std::env::var_os("CS2_SIDECAR9_PROFILE").expect("isolated profile required");
        let run_id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let save_root = Path::new(&profile).join(format!("cyanotype-f711-{}-{run_id}", std::process::id()));
        let mut player = crate::HeadlessPlayer::open_with_save_root(game, save_root)?;
        player.enable_host_window_requests();
        player.initialize_general_window_host(1)?;
        player.set_host_input_update_rate(60);
        player.enable_host_system_open_queue();
        player.enable_host_tweet_queue();
        player.enable_host_dialog_queue();
        player.enable_host_story_trace();
        player.load_startup()?;
        player.apply_host_input(cs2_runtime::InputEvent::FocusChanged(true))?;
        for frame in 1..=711 {
            let mut advanced = false;
            for _ in 0..128 {
                advanced = anyhow::Context::with_context(player.advance_host_frame(20_000), || {
                    let error = player.events().iter().rev().find(|event| event.kind == "task-error");
                    format!("Cyanotype f{frame}: {}", error.map_or("no task error event", |event| event.detail.as_str()))
                })?;
                if advanced { break; }
            }
            anyhow::ensure!(advanced, "Cyanotype stalled at f{frame}");
            player.limit_host_diagnostics(10_000, 1_000);
        }
        assert_eq!(player.host_story_trace().2, "prologue");
        let mut events = player.events().iter().filter(|event| event.frame == 711);
        assert!(events.any(|event| event.kind == "cst-text-sourced-copy"
            && event.detail.ends_with("source=sscript-speaker/Utf8")));
        assert!(events.any(|event| event.kind == "fes-text-sourced-queued"
            && event.detail.contains("cst=[Utf8]")));
        player.with_host(|host| {
            assert!(host.data_services.language.languages.prefix(1).is_none());
            let (owner, _, _, _) = host.data_services.tasks.fes.story_runtimes()
                .find(|(_, name, _, _)| *name == b"cinema.fes")
                .ok_or_else(|| anyhow::anyhow!("Cyanotype f711 cinema receiver missing"))?;
            let text = &host.data_services.fes_text;
            let generation = text.story_body_generation(owner)
                .ok_or_else(|| anyhow::anyhow!("Cyanotype f711 cinema body generation missing"))?;
            assert_eq!(generation.source_frame, 711);
            let sources = text.story_body_sources(owner, generation.source_epoch, &host.data_services.cst_text_origins)
                .ok_or_else(|| anyhow::anyhow!("Cyanotype f711 cinema body source missing"))?;
            let [source] = sources else { anyhow::bail!("Cyanotype f711 expected one cinema body source"); };
            assert_eq!((source.group, source.absolute_record), (0, 17));
            let (_, bytes) = host.cst_resource(b"scene", b"prologue")?
                .ok_or_else(|| anyhow::anyhow!("Cyanotype prologue resource missing"))?;
            let prologue = CstDocument::parse(&bytes)?;
            assert_eq!(host.cst_contexts[&source.context_handle].document.as_ref(), Some(&prologue));
            Ok(())
        })
    }

    fn check_scene(game: &str, filename: &[u8], records: &[usize], fallback: &str) -> anyhow::Result<()> {
        let root = std::env::var_os(game).expect("installed game root required");
        let profile = std::env::var_os("CS2_SIDECAR9_PROFILE").expect("isolated profile required");
        let mut player = crate::HeadlessPlayer::open_with_save_root(root, Path::new(&profile).join(game))?;
        let mut vm = Vm::new();
        vm.bind_external(vec![0; 4096])?;
        let scene_name = 0x8000_0100;
        let category = 0x8000_0200;
        let unbound_language = 0x8000_0300;
        let destination = 0x8000_0400;
        vm.write(scene_name, filename)?;
        vm.write(category, b"scene\0")?;
        vm.write(unbound_language, b"sidecar9-unbound\0")?;
        let (handle, virtual_name) = player.with_host(|host| {
            let NativeReturn::Value(handle) = host.call(323, &[], &mut vm)? else {
                anyhow::bail!("no CST context handle");
            };
            let (virtual_name, winner) = host.cst_resource_with_source(b"scene", trim_nul(filename))?.unwrap();
            assert!(verified_gbk_scene(host.resources.root(), &winner));
            let mut changed = winner.clone();
            changed.bytes[0] ^= 1;
            assert!(!verified_gbk_scene(host.resources.root(), &changed));
            let mut moved = winner.clone();
            match &mut moved.source.location {
                ResourceLocation::File { path } => *path = host.resources.root().join("other/com_001A.cst"),
                ResourceLocation::Archive { path, .. } => *path = host.resources.root().join("update02.int"),
            }
            assert!(!verified_gbk_scene(host.resources.root(), &moved));
            if matches!(&winner.source.location, ResourceLocation::Archive { .. }) {
                moved.source.location = winner.source.location.clone();
                moved.source.name = "other.cst".into();
                assert!(!verified_gbk_scene(host.resources.root(), &moved));
            }

            assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
            let context = &host.cst_contexts[&handle];
            assert_eq!(context.encoding, ScriptTextEncoding::Gbk);
            let document = context.document.as_ref().unwrap().clone();
            for &record in records {
                let input = document.inputs.iter().position(|range| range.contains(&record)).unwrap();
                let ordinal = record - document.inputs[input].start;
                let read_args = args(&[handle, destination, input as u32, ordinal as u32]);
                let raw = &document.records[record].text;
                let expected = encoding_rs::GBK.decode_without_bom_handling_and_without_replacement(raw).unwrap();
                assert_ne!(expected.as_ref(), ScriptTextEncoding::Cp932.decode(raw));
                host.read_cst_record(327, &read_args, &mut vm)?;
                assert_eq!(&vm.read_string(destination)?, raw);
                host.call(278, &args(&[destination]), &mut vm)?;
                host.call(276, &args(&[1000, destination]), &mut vm)?;
                assert_eq!(host.data_services.cst_text_origins.shared(1000, raw), Some(ScriptTextEncoding::Gbk));
                host.read_cst_record(1037, &read_args, &mut vm)?;
                assert_eq!(vm.read_string(destination)?, expected.as_bytes());
                host.call(276, &args(&[1000, destination]), &mut vm)?;
                assert_eq!(host.data_services.cst_text_origins.shared(1000, expected.as_bytes()), Some(ScriptTextEncoding::Utf8));
                let localized_args = args(&[handle, destination, input as u32, ordinal as u32, unbound_language]);
                assert_eq!(host.read_cst_record(1038, &localized_args, &mut vm)?,
                    NativeReturn::Value(expected.len() as u32));
                assert_eq!(vm.read_string(destination)?, expected.as_bytes());
            }
            host.call(435, &args(&[scene_name, category, 1]), &mut vm)?;
            host.call(436, &[], &mut vm)?;
            let cached = host.data_services.ordinary.cached_resource_with_source(&virtual_name).unwrap();
            assert_eq!(cached.source, winner.source);
            assert_eq!(cached.bytes, winner.bytes);
            Ok((handle, virtual_name))
        })?;

        let resources = &mut player.scan.resources;
        let mut router = resources.resource_router().unwrap().clone();
        if fallback == "scene.int" {
            let prefix: Vec<_> = "scene.int".encode_utf16().collect();
            for slot in router.slots.iter_mut().filter(|slot| slot.prefix == prefix) {
                slot.loose_enabled = false;
            }
        } else {
            router.patches.retain(|mount| resources.archives()[mount.archive].path != Path::new("update03.int"));
        }
        resources.set_resource_router(router)?;

        player.with_host(|host| {
            let candidate = host.resources.resolve(&virtual_name)?.unwrap();
            let ResourceLocation::Archive { path, .. } = &candidate.location else {
                anyhow::bail!("expected archive fallback");
            };
            assert_eq!(path.strip_prefix(host.resources.root())?, Path::new(fallback));
            assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
            assert_eq!(host.cst_contexts[&handle].encoding, ScriptTextEncoding::Gbk);
            host.call(434, &[], &mut vm)?;
            assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
            let context = &host.cst_contexts[&handle];
            assert_eq!(context.encoding, ScriptTextEncoding::Cp932);
            let document = context.document.as_ref().unwrap();
            let record = &document.records[records[0]];
            assert!(record.text.iter().any(|byte| !byte.is_ascii()));
            let expected = encoding_rs::SHIFT_JIS.decode_without_bom_handling_and_without_replacement(&record.text).unwrap().into_owned();
            let input = document.inputs.iter().position(|range| range.contains(&records[0])).unwrap();
            let ordinal = records[0] - document.inputs[input].start;
            let localized_args = args(&[handle, destination, input as u32, ordinal as u32, unbound_language]);
            assert_eq!(host.read_cst_record(1038, &localized_args, &mut vm)?,
                NativeReturn::Value(expected.len() as u32));
            assert_eq!(vm.read_string(destination)?, expected.as_bytes());
            Ok(())
        })
    }

    #[test]
    #[ignore = "requires the pinned installed Newton game and an isolated profile path"]
    fn sidecar9_newton_origin_cache_and_fallback() -> anyhow::Result<()> {
        let root = std::env::var_os("CS2_SIDECAR9_NEWTON").expect("installed game root required");
        let scan = cs2_runtime::GameRoot::open(root)?.scan()?;
        assert!(verified_newton_text_profile(&scan.resources));
        check_scene("CS2_SIDECAR9_NEWTON", b"com_001A.cst\0", &[15, 23], "scene.int")
    }

    #[test]
    #[ignore = "requires the installed ISLAND game and an isolated profile path"]
    fn island_chinese_choices_and_ending_use_resolved_scene_codec() -> anyhow::Result<()> {
        let root = std::env::var_os("CS2_ISLAND_TEXT_TEST_ROOT").expect("installed game root required");
        let profile = std::env::var_os("CS2_SIDECAR9_PROFILE").expect("isolated profile required");
        let mut player = crate::HeadlessPlayer::open_with_save_root(root, Path::new(&profile).join("island-text"))?;
        let mut vm = Vm::new();
        vm.bind_external(vec![0; 16384])?;
        let scene_name = 0x8000_0100;
        let destination = 0x8000_0400;
        player.with_host(|host| {
            let NativeReturn::Value(handle) = host.call(323, &[], &mut vm)? else {
                anyhow::bail!("no CST context handle");
            };
            vm.write(scene_name, b"com01.cst\0")?;
            assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
            let raw = b"\\n\\pc\xc6\xf0\xb3\xf5\xa3\xac\xcc\xfd\xb5\xbd\xc1\xcb\xd2\xbb\xcb\xbf\xc9\xf9\xcf\xec\xa1\xa3";
            let expected = "\\n\\pc\u{8d77}\u{521d}\u{ff0c}\u{542c}\u{5230}\u{4e86}\u{4e00}\u{4e1d}\u{58f0}\u{54cd}\u{3002}";
            let read_args = args(&[handle, destination, 0, 53]);
            host.read_cst_record(327, &read_args, &mut vm)?;
            host.call(278, &args(&[destination]), &mut vm)?;
            host.call(276, &args(&[1000, destination]), &mut vm)?;
            let published = &host.string_values[&1000];
            assert_eq!(published.as_slice(), raw);
            let origin = host.data_services.cst_text_origins.shared(1000, published);
            assert_eq!(origin, Some(ScriptTextEncoding::Gbk));
            assert_eq!(super::super::fes_text::decode_sourced_text(
                b"$str1000", published, ScriptTextEncoding::Cp932, ScriptTextEncoding::Cp932, false, |id| {
                    assert_eq!(id, 1000);
                    (published.as_slice(), origin)
                },
            )?.as_deref(), Some(expected));
            host.read_cst_record(1037, &read_args, &mut vm)?;
            assert_eq!(vm.read_string(destination)?, expected.as_bytes());
            for (filename, kind) in [(b"com01_selC.cst\0".as_slice(), 0x30), (b"BAD_02.cst\0", 0x20)] {
                vm.write(scene_name, filename)?;
                assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
                let context = &host.cst_contexts[&handle];
                assert_eq!(context.encoding, ScriptTextEncoding::Gbk);
                let document = context.document.as_ref().unwrap();
                let (input, ordinal, expected) = document.inputs.iter().enumerate().find_map(|(input, range)| {
                    document.records[range.clone()].iter().enumerate().find_map(|(ordinal, record)| {
                        if record.kind == kind && !record.text.is_ascii() {
                            let text = encoding_rs::GBK.decode_without_bom_handling_and_without_replacement(&record.text)?;
                            Some((input, ordinal, text.into_owned()))
                        } else { None }
                    })
                }).expect("expected translated record");
                host.read_cst_record(1037, &args(&[handle, destination, input as u32, ordinal as u32]), &mut vm)?;
                assert_eq!(vm.read_string(destination)?, expected.as_bytes());
            }
            Ok(())
        })
    }

    #[test]
    #[ignore = "requires the pinned installed Cyanotype game and an isolated profile path"]
    fn sidecar9_cyanotype_origin_cache_and_fallback() -> anyhow::Result<()> {
        check_scene("CS2_SIDECAR9_CYANOTYPE", b"prologue.cst\0", &[17], "update02.int")
    }
}

fn formatted(vm: &Vm, address: u32, capacity: usize) -> Result<Vec<u8>> {
    let bytes = expand_kcs_format(vm, &vm.read_string(address)?, 0)?;
    let text = trim_nul(&bytes);
    if text.len() >= capacity { return Err(native_state_error("CST formatted name exceeds fixed buffer")); }
    Ok(text.to_vec())
}

/// Untrimmed, signed decimal/hex prefix with DWORD wrapping. The caller
/// decides whether an unparseable value means a missing marker or numeric zero.
pub(super) fn integer_prefix(bytes: &[u8]) -> Option<(usize, u32)> {
    let negative = bytes.first() == Some(&b'-');
    let mut cursor = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let hex = bytes.get(cursor..)?.starts_with(b"0x") || bytes.get(cursor..)?.starts_with(b"0X");
    if hex { cursor += 2; }
    let start = cursor;
    let mut value = 0u32;
    while let Some(digit) = bytes.get(cursor).and_then(|byte| match *byte {
        b'0'..=b'9' => Some(u32::from(*byte - b'0')),
        b'a'..=b'f' if hex => Some(u32::from(*byte - b'a' + 10)),
        b'A'..=b'F' if hex => Some(u32::from(*byte - b'A' + 10)),
        _ => None,
    }) {
        value = value.wrapping_mul(if hex { 16 } else { 10 }).wrapping_add(digit);
        cursor += 1;
    }
    if cursor == start || (!hex && bytes.get(cursor) == Some(&b'.')) { return None; }
    Some((cursor, if negative { value.wrapping_neg() } else { value }))
}
