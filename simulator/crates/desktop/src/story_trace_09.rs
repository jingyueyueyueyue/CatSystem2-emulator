use cs2_runtime::message_object::{ButtonSource, DeviceBinding, InputButtonState};
use cs2_script::{FesProgram, KcsProgram};

use crate::cst_context_native::{CstBodySource, CstSourceProbe, CstSourceRecordObservation};
use crate::fes_driver::FesDriverWait;
use crate::particle_plugin_digest::sha256;
use crate::{HeadlessPlayer, ScriptTextEncoding};

use super::{current_committed_body, submitted_body_pixels};

const WINDOW: &str = "meswnd";
const SCENE: &str = "sc01";
const SUCCESSOR: &str = "sc02";
const POST_OPENING: &str = "sc03";
const SC03_WAITS: [i32; 6] = [7, 10, 20, 27, 29, 36];
const SC01_WAITS: [i32; 294] = [
    41, 43, 45, 47, 49, 51, 53, 78, 80, 83, 85, 87,
    89, 91, 93, 96, 98, 100, 102, 118, 120, 122, 124, 126,
    128, 130, 132, 134, 141, 144, 146, 152, 154, 161, 164, 179,
    182, 189, 196, 199, 208, 211, 214, 221, 223, 225, 228, 235,
    242, 245, 249, 256, 258, 260, 263, 270, 277, 280, 284, 287,
    294, 297, 304, 306, 308, 315, 318, 325, 328, 335, 353, 363,
    366, 376, 379, 386, 389, 396, 399, 406, 409, 416, 420, 423,
    425, 427, 429, 431, 434, 441, 444, 447, 454, 456, 458, 461,
    468, 475, 478, 485, 492, 495, 502, 505, 512, 519, 522, 529,
    532, 535, 542, 544, 546, 548, 552, 555, 562, 565, 572, 579,
    582, 585, 592, 595, 603, 610, 613, 620, 627, 630, 637, 643,
    645, 647, 665, 667, 675, 678, 681, 688, 695, 698, 705, 712,
    715, 722, 725, 732, 735, 742, 745, 747, 749, 751, 758, 761,
    768, 771, 778, 781, 791, 793, 796, 806, 809, 816, 819, 826,
    833, 836, 838, 840, 847, 850, 857, 860, 867, 870, 872, 890,
    895, 897, 899, 901, 904, 907, 925, 927, 929, 931, 933, 935,
    937, 939, 942, 944, 946, 949, 951, 953, 955, 958, 962, 964,
    966, 969, 971, 983, 993, 1002, 1004, 1029, 1031, 1033, 1035, 1037,
    1049, 1051, 1054, 1058, 1061, 1063, 1065, 1067, 1070, 1072, 1076, 1087,
    1097, 1100, 1112, 1114, 1117, 1121, 1124, 1128, 1130, 1134, 1136, 1138,
    1142, 1145, 1149, 1152, 1156, 1159, 1164, 1167, 1171, 1174, 1178, 1182,
    1185, 1189, 1192, 1196, 1198, 1200, 1203, 1207, 1210, 1214, 1217, 1220,
    1223, 1227, 1230, 1232, 1235, 1239, 1242, 1246, 1248, 1256, 1259, 1261,
    1263, 1267, 1270, 1274, 1277, 1285, 1288, 1292, 1295, 1299, 1303, 1306,
    1314, 1316, 1318, 1320, 1322, 1324,
];
const SC02_WAITS: [i32; 265] = [
    10, 12, 15, 22, 29, 32, 39, 42, 49, 52, 55, 57,
    64, 67, 75, 78, 81, 96, 109, 112, 115, 125, 132, 135,
    145, 147, 149, 156, 159, 166, 169, 176, 183, 186, 193, 196,
    203, 206, 213, 220, 223, 226, 233, 236, 243, 250, 257, 260,
    267, 270, 277, 284, 287, 289, 291, 293, 300, 303, 310, 313,
    320, 323, 325, 328, 335, 338, 348, 351, 361, 363, 365, 367,
    396, 404, 407, 416, 418, 420, 427, 430, 437, 440, 447, 450,
    457, 460, 467, 470, 477, 480, 489, 508, 516, 519, 523, 526,
    530, 533, 540, 543, 547, 562,
    565, 569, 573, 577, 581, 588, 594, 598, 601, 605, 608, 610,
    612, 616, 623, 627, 630, 634, 638, 645, 649, 653, 656, 661,
    663, 665, 667, 670, 677, 680, 687, 690, 694, 698, 705, 707,
    722, 741, 743, 746, 748, 750, 753, 755, 780, 783, 790, 793,
    800, 803, 810, 813, 820, 823, 830, 833, 840, 847, 865, 867,
    870, 872, 874, 876, 879, 881, 883, 885, 903, 905, 925, 929,
    932, 936, 940, 943, 947, 951, 953, 955, 957, 960, 985, 987,
    989, 991, 993, 1007, 1023, 1025, 1032, 1035, 1037, 1039, 1042, 1044,
    1052, 1058, 1061, 1080, 1087, 1105, 1107, 1109, 1112, 1120, 1124, 1127,
    1130, 1138, 1146, 1154, 1157, 1159, 1167, 1175, 1179, 1182, 1190, 1194,
    1197, 1205, 1209, 1213, 1217, 1221, 1223, 1226, 1229, 1237, 1241, 1244,
    1247, 1250, 1258, 1262, 1270, 1278, 1281, 1283, 1285, 1287, 1290, 1298,
    1306, 1309, 1312, 1320, 1324, 1327, 1335, 1339, 1341, 1343, 1345, 1348,
    1351, 1359, 1371, 1373, 1375, 1377, 1379,
];
const ROOT_WAIT: u32 = 0x0cae;
const ENTER: u8 = 13;
const LOGICAL_ENTER: usize = 2;
const BOTTOM_BOUNDS: [i32; 4] = [-100000, -100000, 100000, 100000];
const BOTTOM_PRIORITY: i32 = 6;
const SSCRIPT_CODE: &str = "7a068090a8d37c7a446a74e98448458e3d6ed75d94bb1a00022eaeb07d7a3921";
const SSCRIPT_DATA: &str = "cda42ae0bdd55920fd91baf7c5267995cf9b497fca4fe4530ef0469adbe14a14";
const FLOW_FES: &str = "8a22b583e1e6dca573c2cbc56097a2134a5f3a3c21a4075fea077bd1c1fa5fea";
const TITLE_FES: &str = "b737c7557f43be7c399173cd181532dc6af01cbe4f6ebf5cbda3df5861ab40fb";
const SAVELOAD_FES: &str = "ca732386c6cbf52c978f10db32c3ad71636ff046d4d2415baaef26ceeedd2289";
const SAVE_PAGE: (i32, i32) = (25, 147);
const NEW_GAME: [i32; 2] = [140, 354];
const TITLE_REGION: u32 = 14;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct KanmiPage {
    pub(super) frame: u64,
    pub(super) scene: &'static str,
    pub(super) group: i32,
    pub(super) record: i32,
    pub(super) window: &'static str,
    pub(super) owner: u32,
    pub(super) receiver: u32,
    pub(super) region: u32,
    pub(super) source: CstBodySource,
    pub(super) composed_prefix: Option<usize>,
    pub(super) source_epoch: u64,
    pub(super) source_frame: u64,
}

impl HeadlessPlayer {
    pub fn host_kanmi_scene_waits(scene: &str) -> &'static [i32] {
        match scene {
            SCENE => &SC01_WAITS,
            SUCCESSOR => &SC02_WAITS,
            POST_OPENING => &SC03_WAITS,
            _ => &[],
        }
    }

    pub fn host_kanmi_body_record(scene: &str, group: i32, wait: i32) -> Option<usize> {
        let expected = *Self::host_kanmi_scene_waits(scene).get(usize::try_from(group).ok()?)?;
        if expected != wait {
            return None;
        }
        Some(match (scene, group) {
            (SUCCESSOR, 101) => 560,
            (SUCCESSOR, 201) => 1076,
            (SUCCESSOR, 202) => 1083,
            (SUCCESSOR, 203) => 1102,
            _ => wait as usize - 1,
        })
    }

    pub fn host_kanmi_sc02_loaded_after(&self, dispatched: u64) -> bool {
        self.cst_contexts.values().any(|context| {
            context.story.scene == SUCCESSOR && context.story.loaded > dispatched
        })
    }

    pub fn host_kanmi_sc02_branch_after(&self, dispatched: u64) -> Option<(u64, &str, u64)> {
        let mut events = self.events().iter().rev().take_while(|entry| entry.frame > dispatched);
        let first_page = observe(self, self.state.clock.frame)?;
        if (first_page.scene, first_page.group, first_page.record) != (POST_OPENING, 0, 7) {
            return None;
        }
        let movie = events.find(|entry| entry.kind == "kanmi-film-end" && entry.detail.ends_with(" natural=true"))?;
        let branch = events.find(|entry| entry.kind == "kanmi-op-branch-load")?;
        let scene = branch.detail.strip_prefix("scene=")?.split_once(' ')?.0;
        (branch.frame <= movie.frame && movie.frame <= first_page.source_frame)
            .then_some((branch.frame, scene, movie.frame))
    }

    /// Title owner for the exact Kanmi New Game route at a completed frame.
    pub fn host_kanmi_exact_title_owner(&self) -> Option<u32> {
        title_owner(self, self.state.clock.frame)
    }

    /// Recheck the live target and press order immediately before dispatch.
    pub fn host_kanmi_exact_new_game_target(&mut self) -> Option<(u32, u32)> {
        let owner = self.host_kanmi_exact_title_owner()?;
        let mask = self.masked_input_region_at(NEW_GAME).ok()??;
        let button = self.input_button_state(TITLE_REGION)?;
        // The hit query checks the registered bitmap or the live rectangle.
        if mask[0] != TITLE_REGION || mask[8] == 0
            || button.receiver != Some(owner) || button.show == 0
            || self.mouse_press_candidate_regions(NEW_GAME).ok()? != [3, TITLE_REGION, 23, 7]
        {
            return None;
        }
        let (receiver, target) = self.with_host(|host| {
            let scope = host.enter_task_values(owner)?;
            let main_bank = host.shared_values.get(541) == Some(&0);
            host.leave_task_values(scope);
            if !main_bank {
                eprintln!("Kanmi New Game bank mismatch: owner={owner} expected $541=0");
                return Ok(None);
            }
            Ok(crate::fes_task_platform::button_target_lookup(host, TITLE_REGION))
        }).ok()??;
        let target = target?;
        (receiver == owner && target.name.as_slice() == b"btn1" && target.bank == 0
            && live_child(self, TITLE_REGION, owner, 6))
            .then_some((TITLE_REGION, owner))
    }

    /// Text owner, keyboard region, receiver, CST handle, load ID, body
    /// record, source epoch and source frame for the exact committed page.
    pub fn host_kanmi_exact_body(&self) -> Option<(u32, u32, u32, u32, u64, usize, u64, u64, Option<usize>)> {
        let page = observe(self, self.state.clock.frame)?;
        Some((page.owner, page.region, page.receiver, page.source.context_handle, page.source.load_id,
            page.source.absolute_record, page.source_epoch, page.source_frame, page.composed_prefix))
    }

    pub fn host_kanmi_save_point(&mut self, anchor: (u32, u64, u32, u64), slot: bool)
        -> anyhow::Result<Option<([i32; 2], u32, u32)>>
    {
        let Some((owner, name, bank)) = self.kanmi_save_control(anchor, slot)? else { return Ok(None); };
        self.kanmi_menu_button(owner, name, bank)
    }

    pub fn host_kanmi_load_point(&mut self, slot: bool) -> anyhow::Result<Option<([i32; 2], u32, u32)>> {
        let frame = self.state.clock.frame;
        if quiet_root(self, frame).is_none() || !self.cst_contexts.is_empty() {
            return Ok(None);
        }
        if !slot {
            let Some(owner) = title_owner(self, frame) else { return Ok(None); };
            return self.kanmi_menu_button(owner, "btn1", 1);
        }
        let Some(owner) = self.kanmi_saveload_panel(frame, 1)? else { return Ok(None); };
        let flow_ready = {
            let mut flows = self.data_services.tasks.fes.story_runtimes().filter(|(_, name, _, _)| {
                name.strip_suffix(b".fes").unwrap_or(name) == b"flow"
            });
            let Some((flow, _, runtime, _)) = flows.next() else { return Ok(None); };
            let Some(driver) = runtime.driver.as_ref() else { return Ok(None); };
            flows.next().is_none() && live_child(self, owner, flow, 34) && live_child(self, flow, 0, 34)
                && settled_poll(driver.interpreter.current_block(), b"LOAD_WAIT",
                    driver.has_pending_handler() || driver.initialization_pending(), driver.waiting(), frame)
                && verified_title_program(self, "flow.fes", FLOW_FES, &driver.interpreter.program)
        };
        if !flow_ready {
            return Ok(None);
        }
        self.kanmi_menu_button(owner, "btn_data", 0)
    }

    fn kanmi_menu_button(&mut self, owner: u32, name: &str, bank: i32)
        -> anyhow::Result<Option<([i32; 2], u32, u32)>>
    {
        let Some(runtime) = self.data_services.tasks.fes.runtime(owner) else { return Ok(None); };
        let Some(button) = runtime.media.get(&format!("{name}[{bank}]")).filter(|button|
            button.kind == "BUTTON" && button.enabled) else { return Ok(None); };
        let visible = if name == "btn_data" {
            // saveload draws the card on SAVEPANEL data; btn_data is its hit region.
            runtime.media.get(&format!("data[{bank}]")).is_some_and(|panel|
                panel.kind == "SAVEPANEL" && panel.visible && panel.opacity > 0 && panel.image.is_some())
        } else {
            button.visible && button.opacity > 0
        };
        if !visible { return Ok(None); }
        let mut regions = self.input_task.regions.iter().filter(|entry| {
            self.host_neko_save_target(owner, entry.id(), name, bank)
        });
        let Some(entry) = regions.next() else { return Ok(None); };
        let region = entry.id();
        let [left, top, right, bottom] = entry.bounds();
        if regions.next().is_some() || !live_child(self, region, owner, 6)
            || left >= right || top >= bottom || entry.words[8] == 0 || entry.words[9] == 0
        {
            return Ok(None);
        }
        let point = [left.midpoint(right), top.midpoint(bottom)];
        let hit = self.masked_input_region_at(point)?;
        Ok((hit.is_some_and(|words| words[0] == region && words[8] != 0 && words[9] != 0)
            && self.mouse_press_candidate_regions(point)?.contains(&region)).then_some((point, region, owner)))
    }

    fn kanmi_save_control(&self, (context, load, body_owner, epoch): (u32, u64, u32, u64), slot: bool)
        -> anyhow::Result<Option<(u32, &'static str, i32)>>
    {
        let frame = self.state.clock.frame;
        if !self.host_neko_slot_zero_absent() || self.host_story_trace_profile() != Some(super::StoryTraceProfile::KanmiObservation)
            || self.frame_in_progress() || !self.state.input.focused || !self.state.input.pressed.is_empty()
            || self.data_services.host_pause.paused || self.data_services.host_quit_requested
            || !self.pending_messages.is_empty() || !self.pending_fes_events.is_empty()
            || !self.data_services.pending_host_input.is_empty() || !self.movie_playbacks.is_empty()
        {
            return Ok(None);
        }
        if slot {
            let retained = self.cst_contexts.get(&context).is_some_and(|context| {
                context.story.is_scene(SUCCESSOR) && context.story.settled_body_record(frame)
                    .is_some_and(|(_, group, record)| (group, record) == SAVE_PAGE)
            });
            if !retained || !self.data_services.fes_text.story_body_sources(body_owner, epoch, &self.data_services.cst_text_origins)
                .is_some_and(|sources| matches!(sources, [source]
                    if (source.context_handle, source.load_id, source.group, source.absolute_record)
                        == (context, load, SAVE_PAGE.0, SAVE_PAGE.1 as usize - 1)))
            {
                return Ok(None);
            }
            Ok(self.kanmi_saveload_panel(frame, 0)?.map(|owner| (owner, "btn_data", 0)))
        } else {
            let Some(page) = observe(self, frame) else { return Ok(None); };
            if (page.scene, page.group, page.record) != (SUCCESSOR, SAVE_PAGE.0, SAVE_PAGE.1)
                || (page.source.context_handle, page.source.load_id, page.owner, page.source_epoch) != (context, load, body_owner, epoch)
            {
                return Ok(None);
            }
            Ok(Some((body_owner, "btn", 6)))
        }
    }

    fn kanmi_saveload_panel(&self, frame: u64, mode: i32) -> anyhow::Result<Option<u32>> {
        let mut windows = self.data_services.tasks.fes.story_runtimes().filter(|(_, name, _, _)| {
            name.strip_suffix(b".fes").unwrap_or(name) == b"saveload"
        });
        let Some((owner, _, runtime, _)) = windows.next() else { return Ok(None); };
        let Some(driver) = runtime.driver.as_ref() else { return Ok(None); };
        let locals = driver.interpreter.local_values();
        if windows.next().is_some() || !settled_poll(driver.interpreter.current_block(), b"WAIT",
            driver.has_pending_handler() || driver.initialization_pending(), driver.waiting(), frame)
            || locals.get(100) != Some(&0) || locals.get(110) != Some(&mode)
            || !verified_title_program(self, "saveload.fes", SAVELOAD_FES, &driver.interpreter.program)
        {
            return Ok(None);
        }
        let snapshot = self.shared_numeric_state_snapshot()?;
        let root = if snapshot.active_identity == 0 { Some(&snapshot.active_values) } else { snapshot.inactive_values.get(&0) };
        Ok(root.is_some_and(|root| root.get(800) == Some(&(mode as u32)) && root.get(870).is_some_and(|occupied| {
            (*occupied != 0) == (mode == 1)
        })).then_some(owner))
    }
}

fn quiet_root(player: &HeadlessPlayer, frame: u64) -> Option<()> {
    player.verified_kanmi_runtime_pe_sha256()?;
    quiet_root_at(player, frame, ROOT_WAIT)
}

pub(super) fn quiet_root_at(player: &HeadlessPlayer, frame: u64, root_wait: u32) -> Option<()> {
    let services = &player.data_services;
    let input = &services.messages;
    if !services.story_trace
        || player.loaded.as_ref()?.name != "kcs.int/main.kcs"
        || player.state.clock.frame != frame
        || player.frame_in_progress()
        || !player.state.input.focused
        || !player.state.input.pressed.is_empty()
        || services.host_pause.paused
        || services.host_quit_requested
        || !player.movie_playbacks.is_empty()
        || !player.pending_messages.is_empty()
        || !player.pending_fes_events.is_empty()
        || services.tasks.pending_root_dispatch().is_some()
        || !services.pending_host_input.is_empty()
        || player.message_wait_address != Some(root_wait)
        || !player.vm.as_ref()?.waiting_for_message()
        || !player.input_task.alive
        || !input.device_keys_loaded
        || !input.device_foreground
        || !input.physical_buttons.is_empty()
        || !input.physical_edges.is_empty()
        || !input.pending_device_buttons.is_empty()
        || input.key_dispatch.is_some()
        || input.key_selection.is_some()
        || !input.button_dispatches.is_empty()
        || !input.button_active.is_empty()
    {
        return None;
    }
    Some(())
}

fn title_owner(player: &HeadlessPlayer, frame: u64) -> Option<u32> {
    quiet_root(player, frame)?;
    if !player.cst_contexts.is_empty() {
        return None;
    }
    let tasks = &player.data_services.tasks;
    if tasks.fes.runtimes().count() != 2 {
        return None;
    }

    let mut flow = None;
    let mut title = None;
    for (id, name, runtime, wait_key) in tasks.fes.story_runtimes() {
        let name = name.strip_suffix(b".fes").unwrap_or(name);
        let (block, member, digest, owner) = match name {
            b"flow" => (b"TITLE_WAIT".as_slice(), "flow.fes", FLOW_FES, &mut flow),
            b"title" => (b"WAIT".as_slice(), "title.fes", TITLE_FES, &mut title),
            _ => return None,
        };
        let driver = runtime.driver.as_ref()?;
        if wait_key || owner.replace(id).is_some()
            || !settled_poll(driver.interpreter.current_block(), block,
                driver.has_pending_handler() || driver.initialization_pending(), driver.waiting(), frame)
            || !verified_title_program(player, member, digest, &driver.interpreter.program)
        {
            return None;
        }
        if name == b"title" && !runtime.media.get("btn1[0]").is_some_and(|layer| {
            layer.kind == "BUTTON" && layer.enabled && layer.visible && layer.opacity != 0
        }) {
            return None;
        }
    }
    let flow = flow?;
    let title = title?;
    (live_child(player, flow, 0, 34) && live_child(player, title, flow, 34)).then_some(title)
}

fn verified_title_program(player: &HeadlessPlayer, member: &str, digest: &str, program: &FesProgram) -> bool {
    let resources = player.resources();
    resources.category_names(b"fes", member).ok()
        .and_then(|names| names.iter().find_map(|name| resources.find(name)))
        .and_then(|candidate| resources.read_candidate(candidate).ok())
        .filter(|bytes| sha256(bytes) == digest)
        .and_then(|bytes| FesProgram::parse(&bytes).ok())
        .is_some_and(|winner| winner == *program)
}

pub(super) fn observe(player: &HeadlessPlayer, frame: u64) -> Option<KanmiPage> {
    quiet_root(player, frame)?;
    let services = &player.data_services;
    let input = &services.messages;
    if input.key_capture.contains_key(&(LOGICAL_ENTER as u32))
        || input.typed_physical_owners.contains_key(&(ButtonSource::Keyboard, ENTER))
        || input.device_keys.forced_queries(LOGICAL_ENTER as u32).ok()? != 0
        || input.device_keys.forced_queries(0).ok()? != 0
        || !enter_mapping(player.device_key_bindings(), &player.input_task.key_enabled)
    {
        return None;
    }

    let mut contexts = player.cst_contexts.iter().filter(|(_, context)| context.document.is_some());
    let (&handle, context) = contexts.next()?;
    if contexts.next().is_some() || context.localization.is_some()
        || context.encoding != ScriptTextEncoding::Cp932 {
        return None;
    }
    let (marked, group, record) = context.story.settled_body_record(frame)?;
    let (scene, probe) = match context.story.scene.as_str() {
        SCENE => (SCENE, CstSourceProbe::Kanmi),
        SUCCESSOR => (SUCCESSOR, CstSourceProbe::KanmiSc02),
        POST_OPENING => (POST_OPENING, CstSourceProbe::KanmiSc03),
        _ => return None,
    };
    let absolute_record = HeadlessPlayer::host_kanmi_body_record(scene, group, record)?;
    let document = context.document.as_ref()?;
    let group_range = document.inputs.get(usize::try_from(group).ok()?)?;
    let wait_record = usize::try_from(record).ok()?;
    if !group_range.contains(&absolute_record)
        || !group_range.contains(&wait_record)
        || document.records.get(absolute_record)?.kind != 0x20
        || document.records.get(wait_record)?.kind != 0x02
    {
        return None;
    }

    let tasks = &services.tasks;
    let mut flow = None;
    let mut owner = None;
    for (id, name, runtime, wait_key) in tasks.fes.story_runtimes() {
        if !matches!(tasks.message_task_state(id), Some((34, 0, _))) {
            return None;
        }
        let driver = runtime.driver.as_ref()?;
        let pending = driver.has_pending_handler() || driver.initialization_pending();
        let block = driver.interpreter.current_block();
        match name {
            b"flow" | b"flow.fes" => {
                if !settled_poll(block, b"GAME_WAIT", pending, driver.waiting(), frame)
                    || wait_key || flow.replace(id).is_some()
                {
                    return None;
                }
            }
            b"meswnd" | b"meswnd.fes" => {
                let locals = driver.interpreter.local_values();
                let prompt = runtime.media.get("keywait")?;
                if !settled_poll(block, b"WAIT", pending, driver.waiting(), frame)
                    || !message_ready(wait_key, locals.get(100).copied(), locals.get(52).copied(),
                        locals.get(101).copied(), prompt.visible, prompt.opacity)
                    || driver.bounded_message_fes_source != Some(true)
                    || owner.replace(id).is_some()
                {
                    return None;
                }
            }
            _ => return None,
        }
    }
    let flow = flow?;
    let owner = owner?;
    if tasks.fes.runtimes().count() != 2 {
        return None;
    }
    let receiver = *player.task_parents.get(&owner)?;
    if !live_child(player, flow, 0, 34)
        || !live_child(player, receiver, flow, 0)
        || !live_child(player, owner, receiver, 34)
        || !tasks.task_clock(receiver).is_some_and(|clock| clock.paused == 0)
    {
        return None;
    }

    let generation = services.fes_text.story_body_generation(owner)?;
    let origins = &services.cst_text_origins;
    let sources = services.fes_text.story_body_sources(owner, generation.source_epoch, origins)?;
    let multi_body = (scene, group, record) == (SUCCESSOR, 101, 562);
    let (source, composed_prefix) = match sources {
        [source] => (*source, None),
        [prefix, source] if multi_body && prefix.absolute_record == 555 && prefix.load_id == source.load_id =>
            (*source, Some(*prefix)),
        _ => return None,
    };
    let has_read = |source| origins.source_record_observations(handle).iter().any(|entry|
        exact_source(entry, probe, source, context.story.loaded, generation.source_frame));
    if source.context_handle != handle || source.group != group || source.absolute_record != absolute_record
        || !has_read(source)
        || multi_body && (!group_range.contains(&555) || !group_range.contains(&561)
            || document.records.get(555)?.kind != 0x20
            || document.records.get(561)?.text.as_slice() != br"\n"
            || !has_read(CstBodySource { absolute_record: 555, ..source }))
        || composed_prefix.is_some_and(|prefix| !has_read(prefix))
        || !current_committed_body(generation, context.story.loaded, marked, frame)
        || !body_and_prompt_submitted(player, owner)
    {
        return None;
    }

    let mut bottom_buttons = player.scene_nodes.iter().filter(|(_, node)| node.name == "bottom button");
    let (&region, _) = bottom_buttons.next()?;
    if bottom_buttons.next().is_some() || !live_child(player, region, receiver, 6) {
        return None;
    }
    let mut registrations = player.input_task.regions.iter().filter(|entry| entry.id() == region);
    let registration = registrations.next()?;
    let button = input.input_buttons.get(&region)?;
    if registrations.next().is_some()
        || !bottom_registration(&registration.words, &button.state, region, receiver, button.execution_checkpoint().3)
        || input.input_masks.contains_key(&region)
        || !sscript_program(player.task_vms.get(&receiver)?.program()?)
    {
        return None;
    }

    Some(KanmiPage {
        frame, scene, group, record, window: WINDOW, owner, receiver, region,
        source, composed_prefix: composed_prefix.map(|prefix| prefix.absolute_record),
        source_epoch: generation.source_epoch, source_frame: generation.source_frame,
    })
}

fn exact_source(entry: &CstSourceRecordObservation, probe: CstSourceProbe, source: CstBodySource,
    loaded: u64, source_frame: u64,
) -> bool {
    entry.probe == probe && entry.source == source && source.load_id != 0
        && entry.native == 327 && entry.kind == 0x20 && entry.body_tagged && !entry.localized
        && entry.stored_codec == ScriptTextEncoding::Cp932 && entry.output_codec == ScriptTextEncoding::Cp932
        && entry.raw_bytes != 0 && entry.raw_bytes == entry.output_bytes
        && loaded <= entry.frame && entry.frame <= source_frame
}

pub(super) fn live_child(player: &HeadlessPlayer, id: u32, parent: u32, kind: u32) -> bool {
    player.task_parents.get(&id) == Some(&parent)
        && player.scene_nodes.get(&id).is_some_and(|node| node.parent == parent && node.kind == kind)
        && player.data_services.tasks.message_task_state(id).is_some_and(|(live_kind, deleted, _)|
            live_kind == kind && deleted == 0)
}

pub(super) fn settled_poll(block: &[u8], expected: &[u8], pending: bool, wait: Option<&FesDriverWait>, frame: u64) -> bool {
    block == expected && !pending
        && matches!(wait, Some(FesDriverWait::Frame { after }) if *after == frame)
}

fn message_ready(wait_key: bool, input: Option<i32>, animation: Option<i32>, prompt: Option<i32>, visible: bool, opacity: u8) -> bool {
    wait_key && input.is_some_and(|value| value != 0) && animation.is_some_and(|value| value != 0)
        && prompt == Some(1) && visible && opacity != 0
}

pub(super) fn body_and_prompt_submitted(player: &HeadlessPlayer, owner: u32) -> bool {
    let body_key = (owner, "\0text:737472[0]".to_owned());
    let prompt_key = (owner, "keywait".to_owned());
    let mut lists = player.data_services.tasks.fes.platform.lists.values().filter(|list|
        list.layers.contains_key(&body_key) || list.layers.contains_key(&prompt_key));
    let Some(list) = lists.next() else { return false; };
    if lists.next().is_some() || !list.enabled || !list.submit {
        return false;
    }
    let Some(output) = player.image_handles.get(&list.output) else { return false; };
    [body_key, prompt_key].iter().all(|key| list.layers.get(key)
        .is_some_and(|layer| submitted_body_pixels(layer, output)))
}

pub(super) fn bottom_registration(words: &[u32; 15], state: &InputButtonState, region: u32, receiver: u32, bound: bool) -> bool {
    bound && words[0] == region && words[1] as i32 == BOTTOM_PRIORITY
        && words[2] & ButtonSource::Keyboard.mask() != 0
        && words[3..7].iter().map(|value| *value as i32).eq(BOTTOM_BOUNDS)
        && words[8] != 0 && words[9] != 0 && words[11] == 0
        && state.id == region && state.receiver == Some(receiver) && state.show != 0
        && state.full_screen && state.rectangle == BOTTOM_BOUNDS
        && state.priority == BOTTOM_PRIORITY && state.parameter == 0
}

pub(super) fn enter_mapping(bindings: &[DeviceBinding; 80], scan_enabled: &[bool; 80]) -> bool {
    let keyboard = ButtonSource::Keyboard.index();
    scan_enabled[LOGICAL_ENTER] && bindings[LOGICAL_ENTER].enabled
        && bindings[LOGICAL_ENTER].codes[keyboard][0] == ENTER
        && bindings.iter().enumerate().all(|(logical, binding)| {
            !binding.enabled || !scan_enabled[logical] || binding.codes[keyboard].iter().enumerate()
                .all(|(group, &code)| code != ENTER || group == 0 && matches!(logical, 0 | LOGICAL_ENTER))
        })
}

fn sscript_program(program: &KcsProgram) -> bool {
    program.code.len() == 919272 && program.data.len() == 497540 && program.stack_size == 10240
        && sha256(&program.code) == SSCRIPT_CODE && sha256(&program.data) == SSCRIPT_DATA
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kanmi_guard_resolves_body_records_for_exact_scene_waits() {
        let source_record = HeadlessPlayer::host_kanmi_body_record;
        assert_eq!(source_record("sc01", 0, 41), Some(40));
        assert_eq!(source_record("sc01", 1, 43), Some(42));
        assert_eq!(source_record("sc02", 0, 41), None);
        assert_eq!(source_record("sc01", 1, 41), None);
        assert_eq!(source_record("sc01", 1, 42), None);
        assert_eq!(source_record("sc01", 2, 45), Some(44));
        assert_eq!(source_record("sc01", 3, 47), Some(46));
        assert_eq!(source_record("sc01", 4, 49), Some(48));
        assert_eq!(source_record("sc01", 5, 51), Some(50));
        assert_eq!(source_record("sc01", 6, 53), Some(52));
        assert_eq!(source_record("sc01", 7, 78), Some(77));
        assert_eq!(source_record("sc01", 8, 80), Some(79));
        assert_eq!(source_record("sc01", 7, 55), None);
    }

    #[test]
    fn kanmi_guard_rejects_stale_or_altered_body_provenance() {
        let source = CstBodySource { context_handle: 155, load_id: 9, group: 1, absolute_record: 42 };
        let observation = CstSourceRecordObservation {
            source, frame: 910, probe: CstSourceProbe::Kanmi, native: 327, kind: 0x20,
            stored_codec: ScriptTextEncoding::Cp932, output_codec: ScriptTextEncoding::Cp932,
            raw_bytes: 44, output_bytes: 44, body_tagged: true, localized: false,
        };
        assert!(exact_source(&observation, CstSourceProbe::Kanmi, source, 448, 911));
        assert!(!exact_source(&observation, CstSourceProbe::Kanmi, source, 448, 909));
        assert!(!exact_source(&observation, CstSourceProbe::Kanmi, CstBodySource { load_id: 10, ..source }, 448, 911));
        assert!(!exact_source(&observation, CstSourceProbe::Kanmi, CstBodySource { absolute_record: 40, ..source }, 448, 911));
        assert!(!exact_source(&CstSourceRecordObservation { localized: true, ..observation }, CstSourceProbe::Kanmi, source, 448, 911));
        assert!(!exact_source(&CstSourceRecordObservation { body_tagged: false, ..observation }, CstSourceProbe::Kanmi, source, 448, 911));
        assert!(!exact_source(&CstSourceRecordObservation { probe: CstSourceProbe::Eden, ..observation }, CstSourceProbe::Kanmi, source, 448, 911));
    }

    #[test]
    fn kanmi_guard_requires_settled_wait_and_visible_prompt() {
        let wait = FesDriverWait::Frame { after: 900 };
        assert!(settled_poll(b"WAIT", b"WAIT", false, Some(&wait), 900));
        assert!(!settled_poll(b"MES_SHOW", b"WAIT", false, Some(&wait), 900));
        assert!(!settled_poll(b"WAIT", b"WAIT", true, Some(&wait), 900));
        assert!(!settled_poll(b"WAIT", b"WAIT", false, Some(&wait), 901));
        assert!(message_ready(true, Some(1), Some(1), Some(1), true, 255));
        assert!(!message_ready(true, Some(1), Some(1), Some(1), true, 0));
        assert!(!message_ready(true, Some(1), Some(1), Some(1), false, 255));
        assert!(!message_ready(true, Some(1), Some(1), Some(0), true, 255));
        assert!(!message_ready(false, Some(1), Some(1), Some(1), true, 255));
    }

    #[test]
    fn kanmi_guard_requires_the_bound_live_keyboard_region() {
        let region = 760;
        let receiver = 58;
        let mut words = [region, 6, 7, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 1];
        words[3..7].copy_from_slice(&BOTTOM_BOUNDS.map(|value| value as u32));
        let mut state = InputButtonState::new(region);
        state.receiver = Some(receiver);
        state.rectangle = BOTTOM_BOUNDS;
        state.full_screen = true;
        state.priority = 6;
        assert!(bottom_registration(&words, &state, region, receiver, true));
        assert!(!bottom_registration(&words, &state, region, receiver, false));
        assert!(!bottom_registration(&words, &state, region, 8, true));
        assert!(!bottom_registration(&words, &state, 60, receiver, true));
        words[8] = 0;
        assert!(!bottom_registration(&words, &state, region, receiver, true));
        words[8] = 1;
        words[2] = ButtonSource::Mouse.mask();
        assert!(!bottom_registration(&words, &state, region, receiver, true));
        words[2] = ButtonSource::Keyboard.mask();
        words[11] = 1;
        assert!(!bottom_registration(&words, &state, region, receiver, true));
    }

    #[test]
    fn kanmi_guard_rejects_disabled_moved_or_ambiguous_enter_bindings() {
        let mut bindings = [DeviceBinding::default(); 80];
        let mut enabled = [true; 80];
        let keyboard = ButtonSource::Keyboard.index();
        bindings[2].enabled = true;
        bindings[2].codes[keyboard][0] = ENTER;
        bindings[0] = bindings[2];
        assert!(enter_mapping(&bindings, &enabled));
        enabled[2] = false;
        assert!(!enter_mapping(&bindings, &enabled));
        enabled[2] = true;
        bindings[2].codes[keyboard][1] = ENTER;
        assert!(!enter_mapping(&bindings, &enabled));
        bindings[2].codes[keyboard][1] = 0;
        bindings[4] = bindings[2];
        assert!(!enter_mapping(&bindings, &enabled));
        bindings[4].enabled = false;
        assert!(enter_mapping(&bindings, &enabled));
        bindings[2].codes[keyboard][0] = 32;
        assert!(!enter_mapping(&bindings, &enabled));
    }
}
