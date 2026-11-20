use super::HeadlessPlayer;
use super::fes_driver::FesDriverWait;
use super::particle_plugin_digest::sha256;
use cs2_runtime::ResourceLocation;
use cs2_runtime::message_object::{ButtonSource, InputRouteReceipt, RoutedPhysicalButton};
use cs2_script::FesDocument;

const PE_SHA256: &str = "72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8";
const EYE_FES_SHA256: &str = "d50f5ee259007e5ec7472e22ee25f8668d39b5cd577330d9811585f5d2d9d5b5";
const EYE_FES_SIZE: usize = 1379;
const EYE_POINT: [i32; 2] = [640, 360];
const EYE_SCENE: &str = "c01_01";

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
    /// After moving to the eye, arm a single left click; poll after every complete frame.
    /// A true result ends this step, not the scene. The caller owns all input and the timeout.
    pub fn host_prepare_eye08_click(&mut self) -> anyhow::Result<Option<EyeExitPoll>> {
        let frame = self.state.clock.frame;
        if !self.data_services.story_trace
            || [self.state.input.cursor_x, self.state.input.cursor_y] != EYE_POINT
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
            let mut next = match validate_press(player.host_input_route_receipts(), now, eye_button, flow_button)? {
                true if previous == EyePhase::AwaitingPress => EyePhase::PressVerified,
                true => anyhow::bail!("08 eyecatch received another press"),
                false => previous,
            };
            let tasks = &player.data_services.tasks;
            if let Some(runtime) = tasks.fes.runtime(owner) {
                anyhow::ensure!(player.task_parents.get(&owner) == Some(&parent), "08 eyecatch owner rebound");
                let driver = runtime.driver.as_ref().ok_or_else(|| anyhow::anyhow!("08 eyecatch lost its driver"))?;
                match (next, driver.interpreter.current_block()) {
                    (EyePhase::AwaitingPress | EyePhase::PressVerified, b"WAIT") => {}
                    (EyePhase::PressVerified | EyePhase::ExitSeen, b"EXIT") => next = EyePhase::ExitSeen,
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
    let matches = |receipt: &InputRouteReceipt, target: EyeButton| {
        receipt.region == target.region && receipt.receiver == target.owner && receipt.pressed == 1
            && !receipt.repeated && receipt.key == 0 && receipt.group == 0
            && receipt.raw_mask == ButtonSource::Mouse.mask()
            && receipt.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }]
    };
    anyhow::ensure!(matches(first, eye), "08 eyecatch press did not reach its owner first");
    if let Some(second) = presses.next() {
        anyhow::ensure!(first.response != 0 && matches(second, flow) && presses.next().is_none(),
            "08 eyecatch press reached another receiver");
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
        assert!(validate_press(&[first.clone(), second.clone()], 5705, eye, flow).unwrap());
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
