use super::HeadlessPlayer;
use super::fes_driver::FesDriverWait;
use super::particle_plugin_digest::sha256;
use super::story_trace_09::{enter_mapping, live_child, quiet_root_at};
use cs2_runtime::message_object::{ButtonSource, InputRouteReceipt, RoutedPhysicalButton};
use cs2_runtime::{InputKey, ResourceLocation};
use cs2_script::FesDocument;

const PE_SHA256: &str = "72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8";
const EYE_FES_SHA256: &str = "d50f5ee259007e5ec7472e22ee25f8668d39b5cd577330d9811585f5d2d9d5b5";
const EYE_FES_SIZE: usize = 1379;
const EYE_POINT: [i32; 2] = [640, 360];
const EYE_SCENE: &str = "c01_01";
const ROOT_WAIT: u32 = 0x32f;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Eye08Body {
    pub flow: u32,
    pub script: u32,
    pub owner: u32,
    pub plane: u32,
    pub string: u32,
    pub generation: u64,
    pub marked: u64,
    pub reveal: u64,
    pub opaque: usize,
}

impl Eye08Body {
    pub fn same_tasks(self, next: Self) -> bool {
        (self.flow, self.script, self.owner, self.plane, self.string)
            == (next.flow, next.script, next.owner, next.plane, next.string)
    }
}

type EyeExitPoll = Box<dyn FnMut(&HeadlessPlayer) -> anyhow::Result<bool>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EyeButton {
    region: u32,
    owner: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EyePhase {
    AwaitingPress,
    PressVerified,
    ExitSeen,
    Released,
    Rejected,
}

impl HeadlessPlayer {
    pub fn host_replay_eye08_body(&self, group: i32, after: u64) -> Result<Eye08Body, &'static str> {
        let (wait, body) = match group {
            0 => (144, 143),
            1 => (148, 147),
            2 => (150, 149),
            3 => (166, 165),
            4 => (170, 169),
            5 => (174, 173),
            6 => (180, 179),
            7..=27 => {
                let wait = 184 + 4 * (group - 7);
                (wait, (wait - 1) as usize)
            }
            _ => return Err("08 body group is outside the route"),
        };
        let frame = self.state.clock.frame;
        quiet_root_at(self, frame, ROOT_WAIT).ok_or("08 root VM or device state is unsettled")?;
        let (context, load, loaded) = self.host_replay_eye08_cst_load().ok_or("08 CST load differs")?;
        let state = self.cst_contexts.get(&context).ok_or("08 CST context missing")?;
        let (marked, actual_group, actual_wait) = state.story.settled_body_record(frame)
            .ok_or("08 executed CST input is missing or pending")?;
        if (actual_group, actual_wait) != (group, wait) || marked <= after {
            return Err("08 executed CST input differs");
        }

        let services = &self.data_services;
        let tasks = &services.tasks;
        let mut runtimes = tasks.fes.story_runtimes();
        let (flow, name, runtime, wait_key) = runtimes.next().ok_or("08 flow FES missing")?;
        let driver = runtime.driver.as_ref().ok_or("08 flow FES driver missing")?;
        if runtimes.next().is_some() || name.strip_suffix(b".fes").unwrap_or(name) != b"flow"
            || !live_child(self, flow, 0, 34)
            || !settled_poll(driver.interpreter.current_block(), b"GAME_WAIT", wait_key,
                driver.has_pending_handler() || driver.initialization_pending(), driver.waiting(), frame)
        {
            return Err("08 flow GAME_WAIT is unsettled");
        }

        let unique_child = |parent: u32, kind: u32, name: &str| {
            let mut nodes = self.scene_nodes.values().filter(|node| {
                node.name == name && live_child(self, node.id, parent, kind)
            });
            let node = nodes.next()?;
            nodes.next().is_none().then_some(node.id)
        };
        let mut windows = self.scene_nodes.values().filter(|node| {
            node.name == "meswnd" && live_child(self, node.id, node.parent, 12)
        });
        let window = windows.next().ok_or("08 ordinary meswnd missing")?;
        let (owner, script) = (window.id, window.parent);
        if windows.next().is_some() || !live_child(self, script, flow, 0)
            || self.task_vms.get(&script).is_none_or(|vm| vm.program().is_none())
            || tasks.task_clock(script).is_none_or(|clock| clock.paused != 0)
        {
            return Err("08 ordinary meswnd ancestry differs");
        }
        let plane = unique_child(owner, 2, "message main").ok_or("08 message main plane differs")?;
        let string = unique_child(plane, 45, "string").ok_or("08 kind-45 string differs")?;
        if services.messages.legacy_string_kind(string) != Some(45) {
            return Err("08 ordinary string binding differs");
        }

        let document = state.document.as_ref().ok_or("08 CST document missing")?;
        let raw = document.records.get(body).ok_or("08 CST body missing")?.text.as_slice();
        if !services.cst_text_origins.source_record_observations(context).iter().any(|entry| {
            (entry.source.context_handle, entry.source.load_id, entry.source.group,
                entry.source.absolute_record) == (context, load, group, body)
                && entry.native == 327 && entry.kind == 0x20 && entry.body_tagged && !entry.localized
                && entry.raw_bytes == raw.len() && entry.output_bytes == raw.len()
                && entry.stored_codec == entry.output_codec
                && (loaded..=marked).contains(&entry.frame)
        }) {
            return Err("08 original CST body read missing");
        }

        let object = services.messages.object(string).ok_or("08 string object missing")?;
        let surface = services.message_device.surface(string as i32).ok_or("08 string surface missing")?;
        if !object.prepared() || object.remaining_all().ok() != Some(0)
            || surface.generation != object.surface_generation() || surface.dirty.is_some()
        {
            return Err("08 ordinary body is still drawing or unsubmitted");
        }
        let appended = object.raw_appends().concat();
        let mut submitted = appended.as_slice();
        while let Some(core) = submitted.strip_suffix(b"\\n").or_else(|| submitted.strip_suffix(b"\n")) {
            submitted = core;
        }
        if submitted != raw.splitn(2, |&byte| byte == 0).next().unwrap_or_default() {
            return Err("08 ordinary append bytes differ from current CST body");
        }
        let completion = format!("index=69 arguments=16 bytes [{:08x}0d0000000000000000000000]", owner.swap_bytes());
        let reveal = self.events.iter().rev().take_while(|event| event.frame >= marked)
            .find(|event| event.frame < frame && event.kind == "native-call" && event.detail.starts_with(&completion))
            .ok_or("08 current body reveal-complete receipt missing")?.frame;
        let node = self.scene_nodes.get(&string).ok_or("08 string node missing")?;
        let projected = self.project_message_plane(node).map_err(|_| "08 string projection failed")?;
        let ((matched, opaque), _, clipping) = self.message_plane_rgb_probe(object, &projected, surface.submitted)
            .ok_or("08 string surface cannot be sampled")?;
        if opaque == 0 || matched != opaque || clipping != (0, 0) {
            return Err("08 string pixels do not match the final composition");
        }
        Ok(Eye08Body { flow, script, owner, plane, string, generation: surface.generation, marked, reveal, opaque })
    }

    pub fn host_replay_eye08_wait_diagnostics(&self) -> String {
        let mut windows = Vec::new();
        for window in self.scene_nodes.values().filter(|node|
            node.name == "meswnd" && live_child(self, node.id, node.parent, 12))
        {
            let script = self.task_vms.get(&window.parent).map(|vm|
                (vm.instruction_pointer, vm.waiting_for_message(), vm.has_pending_execution()));
            let paused = self.data_services.tasks.task_clock(window.parent).map(|clock| clock.paused);
            let mut strings = Vec::new();
            for plane in self.scene_nodes.values().filter(|node|
                node.name == "message main" && live_child(self, node.id, window.id, 2))
            {
                for string in self.scene_nodes.values().filter(|node|
                    node.name == "string" && live_child(self, node.id, plane.id, 45))
                {
                    let object = self.data_services.messages.object(string.id)
                        .map(|object| (object.prepared(), object.remaining_all().ok(), object.surface_generation()));
                    let surface = self.data_services.message_device.surface(string.id as i32)
                        .map(|surface| (surface.generation, surface.dirty.is_some()));
                    strings.push((plane.id, string.id, object, surface));
                }
            }
            windows.push((window.id, window.parent, script, paused, strings));
        }
        format!("08 ordinary wait windows(owner,script,(pc,waiting,pending),paused,strings(plane,string,(prepared,remaining,generation),(generation,dirty)))={windows:?}")
    }

    pub fn host_replay_eye08_enter(&mut self, body: Eye08Body) -> anyhow::Result<Option<(u32, Vec<(u32, u32)>)>> {
        let input = &self.data_services.messages;
        if !input.input_frame_complete || !input.key_capture.is_empty()
            || input.typed_physical_owners.contains_key(&(ButtonSource::Keyboard, 13))
            || self.device_key_scan_state().records().iter().flatten().any(|key| key.mask != 0)
            || !enter_mapping(self.device_key_bindings(), &self.input_task.key_enabled)
            || (0..80).any(|key| input.device_keys.forced_queries(key).ok() != Some(0))
        {
            return Ok(None);
        }
        let mut bottoms = self.scene_nodes.values().filter(|node| {
            node.name == "bottom button" && live_child(self, node.id, body.script, 6)
        });
        let Some(bottom_node) = bottoms.next() else { return Ok(None) };
        if bottoms.next().is_some() {
            return Ok(None);
        }
        let expected = bottom_node.id;
        self.with_host(|host| {
            let mut candidates = Vec::new();
            let mut bottom = None;
            for region in host.input_task.regions.iter().copied() {
                if !host.device_press_region_eligible(region, ButtonSource::Keyboard.mask(), [0, 0])? {
                    continue;
                }
                let id = region.id();
                let Some(receiver) = host.effective_input_region_receiver(id) else { continue };
                if ![0, body.flow, body.script, body.owner].contains(&receiver)
                    || candidates.iter().any(|&(prior, _)| prior == id)
                {
                    return Ok(None);
                }
                if id == expected {
                    if receiver != body.script {
                        return Ok(None);
                    }
                    if region.words[11] != 0 || !host.data_services.messages.input_buttons.get(&id)
                        .is_some_and(|button| button.execution_checkpoint().3 && button.state.show != 0)
                        || region.words[8] == 0 || bottom.replace(id).is_some()
                    {
                        return Ok(None);
                    }
                }
                candidates.push((id, receiver));
            }
            Ok(bottom.map(|region| (region, candidates)))
        })
    }

    /// After the pointer move commits, arm a left down for the next frame and
    /// pair it with one up. Poll after every complete frame.
    /// A true result ends this step, not the scene. The caller owns all input and the timeout.
    pub fn host_prepare_eye08_click(&mut self) -> anyhow::Result<Option<EyeExitPoll>> {
        let frame = self.state.clock.frame;
        if !self.data_services.story_trace
            || [self.state.input.cursor_x, self.state.input.cursor_y] != EYE_POINT
            || !self.data_services.messages.key_capture.is_empty()
        {
            return Ok(None);
        }
        let Some((_, owner)) = observe(self, frame) else { return Ok(None) };
        let Some(scene) = scene_identity(self) else { return Ok(None) };
        let parent = self.task_parents[&owner];
        let flow = self.task_parents[&parent];
        let Some(eye_button) = live_auxiliary(self, owner) else { return Ok(None) };
        let Some(flow_button) = live_auxiliary(self, flow) else { return Ok(None) };
        if eye_button.region == flow_button.region
            || self.mouse_press_candidate_regions(EYE_POINT)? != [eye_button.region, flow_button.region]
        {
            return Ok(None);
        }

        let mut phase = EyePhase::AwaitingPress;
        let mut last_frame = frame;
        Ok(Some(Box::new(move |player| {
            if matches!(phase, EyePhase::Released | EyePhase::Rejected) {
                anyhow::ensure!(phase == EyePhase::Released, "08 eyecatch observation already rejected");
                return Ok(true);
            }
            if player.frame_in_progress() || player.state.clock.frame == last_frame {
                return Ok(false);
            }
            let previous = phase;
            phase = EyePhase::Rejected;
            let now = player.state.clock.frame;
            anyhow::ensure!(last_frame.checked_add(1) == Some(now), "08 eyecatch poll missed a frame");
            anyhow::ensure!(scene_identity(player) == Some(scene)
                && player.state.input.focused && !player.data_services.host_pause.paused
                && !player.data_services.host_quit_requested && player.movie_playbacks.is_empty()
                && [player.state.input.cursor_x, player.state.input.cursor_y] == EYE_POINT
                && live_auxiliary(player, flow) == Some(flow_button), "08 eyecatch scene or flow changed");
            anyhow::ensure!(
                matches!(player.state.input.pressed.as_slice(), [] | [InputKey::MouseButton(0)]),
                "08 eyecatch received another held input"
            );
            let mut next = match validate_press(player.host_input_route_receipts(), now, eye_button, flow_button)? {
                true if previous == EyePhase::AwaitingPress => EyePhase::PressVerified,
                true => anyhow::bail!("08 eyecatch received another press"),
                false if previous == EyePhase::AwaitingPress => {
                    anyhow::bail!("08 eyecatch left down was not routed on the next frame")
                }
                false => previous,
            };
            let tasks = &player.data_services.tasks;
            if let Some(runtime) = tasks.fes.runtime(owner) {
                anyhow::ensure!(
                    player.task_parents.get(&owner) == Some(&parent)
                        && player.task_parents.get(&parent) == Some(&flow),
                    "08 eyecatch owner rebound"
                );
                let driver = runtime.driver.as_ref().ok_or_else(|| anyhow::anyhow!("08 eyecatch lost its driver"))?;
                match (next, driver.interpreter.current_block()) {
                    (EyePhase::AwaitingPress | EyePhase::PressVerified, b"WAIT") => {}
                    (EyePhase::PressVerified | EyePhase::ExitSeen, b"EXIT") => next = EyePhase::ExitSeen,
                    // The pinned eye script executes exit 0, then next SLEEP.
                    // Still wait for the owner and its input region to disappear.
                    (EyePhase::ExitSeen, b"SLEEP") if runtime.exit_code == Some(0) => {}
                    _ => anyhow::bail!("08 eyecatch reached an unverified block"),
                }
                anyhow::ensure!(runtime.exit_code.is_none_or(|code| code == 0), "08 eyecatch exited with an error");
            } else {
                anyhow::ensure!(next == EyePhase::ExitSeen, "08 eyecatch removed without a verified press and EXIT");
                if !player.scene_nodes.contains_key(&owner)
                    && !player.task_parents.contains_key(&owner)
                    && tasks.message_task_state(owner).is_none()
                    && !player.scene_nodes.contains_key(&eye_button.region)
                    && !player.task_parents.contains_key(&eye_button.region)
                    && tasks.message_task_state(eye_button.region).is_none()
                    && !player.input_task.regions.iter().any(|region| region.id() == eye_button.region)
                    && player.state.input.pressed.is_empty()
                    && player.data_services.messages.key_capture.is_empty()
                    && player.data_services.pending_host_input.is_empty()
                {
                    next = EyePhase::Released;
                }
            }
            last_frame = now;
            phase = next;
            Ok(phase == EyePhase::Released)
        })))
    }
}

fn scene_identity(player: &HeadlessPlayer) -> Option<(u32, u64)> {
    let mut scenes = player.cst_contexts.iter().filter(|(_, context)| !context.story.scene.is_empty());
    let (&id, context) = scenes.next()?;
    (scenes.next().is_none() && context.story.scene == EYE_SCENE && context.document.is_some())
        .then_some((id, context.story.loaded))
}

fn live_auxiliary(player: &HeadlessPlayer, owner: u32) -> Option<EyeButton> {
    let tasks = &player.data_services.tasks;
    let region = tasks.fes.platform.auxiliary_button(owner)?;
    let button = player.scene_nodes.get(&region)?;
    (matches!(tasks.message_task_state(owner), Some((34, 0, _)))
        && matches!(tasks.message_task_state(region), Some((6, 0, _)))
        && button.kind == 6 && button.name == "low button" && button.initialized && button.parent == owner
        && player.task_parents.get(&region) == Some(&owner)
        && player.input_button_state(region).is_none_or(|state| state.receiver == Some(owner)))
        .then_some(EyeButton { region, owner })
}

fn validate_press(receipts: &[InputRouteReceipt], frame: u64, eye: EyeButton, flow: EyeButton) -> anyhow::Result<bool> {
    let mut presses = receipts.iter().filter(|receipt| receipt.frame == frame && receipt.pressed != 0);
    let Some(first) = presses.next() else { return Ok(false) };
    let matches = |receipt: &InputRouteReceipt, target: EyeButton, key| {
        receipt.region == target.region && receipt.receiver == target.owner && receipt.pressed == 1
            && !receipt.repeated && receipt.key == key && receipt.group == 0
            && receipt.raw_mask == ButtonSource::Mouse.mask()
            && receipt.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }]
    };
    anyhow::ensure!(matches(first, eye, 0), "08 eyecatch press did not reach its owner first");
    if let Some(second) = presses.next() {
        anyhow::ensure!(first.response != 0 && matches(second, flow, 0),
            "08 eyecatch primary press chain differs");
        if let Some(alias) = presses.next() {
            // This profile also maps Mouse1 to logical key 2 when key 0 passes through.
            anyhow::ensure!(first.response == 1 && second.response == 1
                && matches(alias, eye, 2) && alias.response == 1
                && presses.next().is_some_and(|last| matches(last, flow, 2) && last.response == 1)
                && presses.next().is_none(), "08 eyecatch Mouse1 alias chain differs");
        }
    }
    Ok(true)
}

pub(super) fn is_profile(pe: &str, startup: Option<&str>) -> bool {
    pe == PE_SHA256 && startup == Some("kcs.int/main.kcs")
}

pub(super) fn observe(player: &HeadlessPlayer, frame: u64) -> Option<(&'static str, u32)> {
    let pe = &player.scan.engine.primary_executable.as_ref()?.sha256;
    if !is_profile(pe, player.default_entry())
        || player.loaded.as_ref()?.name != "kcs.int/main.kcs"
        || player.state.clock.frame != frame
        || player.frame_in_progress()
        || !player.state.input.focused
        || !player.state.input.pressed.is_empty()
        || player.data_services.host_pause.paused
        || player.data_services.host_quit_requested
        || !player.movie_playbacks.is_empty()
        || !player.pending_messages.is_empty()
        || !player.pending_fes_events.is_empty()
        || !player.data_services.pending_host_input.is_empty()
        || player.message_wait_address.is_none()
        || !player.vm.as_ref().is_some_and(|vm| vm.waiting_for_message())
    {
        return None;
    }

    let tasks = &player.data_services.tasks;
    let mut flow = None;
    let mut eye = None;
    for (owner, name, runtime, wait_key) in tasks.fes.story_runtimes() {
        let name = name.strip_suffix(b".fes").unwrap_or(name);
        let (slot, expected_block) = match name {
            b"flow" => (&mut flow, b"GAME_WAIT".as_slice()),
            b"eyecatch" => (&mut eye, b"WAIT".as_slice()),
            _ => return None,
        };
        let driver = runtime.driver.as_ref()?;
        if !matches!(tasks.message_task_state(owner), Some((34, 0, _)))
            || !settled_poll(
                driver.interpreter.current_block(),
                expected_block,
                wait_key,
                driver.has_pending_handler() || driver.initialization_pending(),
                driver.waiting(),
                frame,
            )
            || slot.replace(owner).is_some()
        {
            return None;
        }
    }
    let flow = flow?;
    let owner = eye?;
    let node = player.scene_nodes.get(&owner)?;
    let parent = node.parent;
    let region = tasks.fes.platform.auxiliary_button(owner)?;
    let button = player.scene_nodes.get(&region)?;
    if node.kind != 34
        || node.name != "child fes"
        || player.task_parents.get(&owner) != Some(&parent)
        || player.task_parents.get(&parent) != Some(&flow)
        || !matches!(tasks.message_task_state(parent), Some((0, 0, _)))
        || !matches!(tasks.message_task_state(region), Some((6, 0, _)))
        || button.kind != 6
        || button.name != "low button"
        || !button.initialized
        || button.parent != owner
        || player.task_parents.get(&region) != Some(&owner)
        || player.input_button_state(region).is_some_and(|button| button.receiver != Some(owner))
    {
        return None;
    }
    let mut registrations = player.input_task.regions.iter().filter(|entry| entry.id() == region);
    let registered = registrations.next()?.words;
    if registrations.next().is_some()
        || !eye_region(&registered, region)
        || player.masked_input_region_at(EYE_POINT).ok().flatten() != Some(registered)
    {
        return None;
    }

    let resources = player.resources();
    let names = resources.category_names(b"fes", "eyecatch").ok()?;
    let resource = names.iter().find_map(|name| resources.find(name))?;
    if !matches!(&resource.location, ResourceLocation::Archive { path, .. } if path == &resources.root().join("fes.int")) {
        return None;
    }
    let bytes = resources.read_candidate_bounded(resource, EYE_FES_SIZE).ok()??;
    if bytes.len() != EYE_FES_SIZE || sha256(&bytes) != EYE_FES_SHA256 {
        return None;
    }
    let document = FesDocument::parse(&bytes).ok()?;
    (tasks.fes.runtime(owner)?.document == document).then_some(("eyecatch", owner))
}

fn settled_poll(
    block: &[u8], expected_block: &[u8], wait_key: bool, pending: bool,
    wait: Option<&FesDriverWait>, frame: u64,
) -> bool {
    block == expected_block && !wait_key && !pending
        && matches!(wait, Some(FesDriverWait::Frame { after }) if *after == frame)
}

fn eye_region(words: &[u32; 15], region: u32) -> bool {
    words[0] == region && words[2] & ButtonSource::Mouse.mask() != 0 && words[3..7] == [0, 0, 800, 600]
        && words[8] != 0 && words[11] == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eye08_checks_the_full_fresh_left_press_chain() {
        let eye = EyeButton { region: 67, owner: 61 };
        let flow = EyeButton { region: 6, owner: 5 };
        let receipt = |target: EyeButton| InputRouteReceipt {
            frame: 5705, key: 0, group: 0, raw_mask: 2, pressed: 1, repeated: false,
            region: target.region, receiver: target.owner, response: 1,
            physical: vec![RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }],
        };
        let first = receipt(eye);
        let second = receipt(flow);
        let eye_alias = InputRouteReceipt { key: 2, ..first.clone() };
        let flow_alias = InputRouteReceipt { key: 2, ..second.clone() };
        assert!(validate_press(&[first.clone(), second.clone()], 5705, eye, flow).unwrap());
        assert!(validate_press(&[first.clone(), second.clone(), eye_alias.clone(), flow_alias.clone()],
            5705, eye, flow).unwrap());
        assert!(validate_press(&[first.clone(), second.clone(), flow_alias, eye_alias.clone()],
            5705, eye, flow).is_err());
        assert!(validate_press(&[first.clone(), second.clone(), eye_alias], 5705, eye, flow).is_err());
        assert!(!validate_press(&[first.clone()], 5706, eye, flow).unwrap());
        assert!(validate_press(&[second.clone(), first.clone()], 5705, eye, flow).is_err());
        assert!(validate_press(&[first.clone(), first.clone()], 5705, eye, flow).is_err());
        assert!(validate_press(&[InputRouteReceipt { response: 0, ..first.clone() }, second], 5705, eye, flow).is_err());
        assert!(validate_press(&[InputRouteReceipt { repeated: true, ..first }], 5705, eye, flow).is_err());
    }

    #[test]
    fn eye08_profile_requires_exact_pe_and_startup() {
        assert!(is_profile(PE_SHA256, Some("kcs.int/main.kcs")));
        assert!(!is_profile(PE_SHA256, Some("main.kcs")));
        assert!(!is_profile(PE_SHA256, None));
        assert!(!is_profile("", Some("kcs.int/main.kcs")));
    }

    #[test]
    fn eye08_requires_the_current_idle_driver_poll() {
        let wait = FesDriverWait::Frame { after: 5701 };
        assert!(settled_poll(b"WAIT", b"WAIT", false, false, Some(&wait), 5701));
        assert!(!settled_poll(b"EXIT", b"WAIT", false, false, Some(&wait), 5701));
        assert!(!settled_poll(b"WAIT", b"WAIT", true, false, Some(&wait), 5701));
        assert!(!settled_poll(b"WAIT", b"WAIT", false, true, Some(&wait), 5701));
        assert!(!settled_poll(b"WAIT", b"WAIT", false, false, None, 5701));
        assert!(!settled_poll(b"WAIT", b"WAIT", false, false, Some(&wait), 5702));
    }

    #[test]
    fn eye08_region_requires_the_live_id_and_unmasked_enabled_rectangle() {
        let mut words = [811, 2, 7, 0, 0, 800, 600, 1, 1, 1, 0, 0, 0, 0, 1];
        assert!(eye_region(&words, 811));
        assert!(!eye_region(&words, 67));
        words[8] = 0;
        assert!(!eye_region(&words, 811));
        words[8] = 1;
        words[11] = 1;
        assert!(!eye_region(&words, 811));
        words[11] = 0;
        words[2] = ButtonSource::Keyboard.mask();
        assert!(!eye_region(&words, 811));
    }
}
