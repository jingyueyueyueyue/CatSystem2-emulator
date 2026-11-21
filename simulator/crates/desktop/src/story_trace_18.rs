use cs2_runtime::message_object::ButtonSource;

use super::HeadlessPlayer;
use super::story_trace_09::{
    bottom_registration, enter_mapping, live_child, quiet_root_at, settled_poll,
};
use crate::ScriptTextEncoding;
use crate::cst_context_native::CstSourceProbe;

pub(crate) const PE: &str = "659d5f069f3c97f69a61e12449cf1510b45e48a10649b9ac6977c2b93d6d28ad";
pub(crate) const CST: &str = "22d8aef073b8d125bad43817274c77765d0665bfd3224598f1f5fb0ae3905e62";
pub(crate) const LOCALIZED_CST: &str = "a46748a49c7970fc13a19f0b3522b59523a3be78a51bb19ee0ba0932e7d86b27";
const ROOT_WAIT: u32 = 0x59f;
const TITLE_REGION: u32 = 11;

pub(crate) fn page_records(localized: bool, group: i32) -> Option<(i32, &'static [usize])> {
    const ORIGINAL: [(i32, &[usize]); 10] = [
        (90, &[88, 89]), (95, &[92]), (98, &[96, 97]), (100, &[99]), (112, &[102]),
        (115, &[113, 114]), (118, &[116, 117]), (121, &[119, 120]), (124, &[122, 123]),
        (126, &[125]),
    ];
    const LOCALIZED: [(i32, &[usize]); 10] = [
        (94, &[92]), (99, &[96]), (102, &[100]), (104, &[103]), (116, &[106]),
        (119, &[117]), (122, &[120]), (125, &[123]), (128, &[126]), (130, &[129]),
    ];
    let pages = if localized { &LOCALIZED } else { &ORIGINAL };
    pages.get(usize::try_from(group).ok()?).copied()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CampanellaWait {
    pub context: u32,
    pub load: u64,
    pub script: u32,
    pub owner: u32,
    pub plane: u32,
    pub string: u32,
    pub generation: u64,
    pub marked: u64,
    pub opaque: usize,
}

impl CampanellaWait {
    pub fn same_instance(self, other: Self) -> bool {
        (
            self.context,
            self.load,
            self.script,
            self.owner,
            self.plane,
            self.string,
        ) == (
            other.context,
            other.load,
            other.script,
            other.owner,
            other.plane,
            other.string,
        )
    }
}

impl HeadlessPlayer {
    fn campanella_quiet(&self, title_owner: Option<u32>) -> Result<u64, &'static str> {
        let frame = self.state.clock.frame;
        quiet_root_at(self, frame, ROOT_WAIT).ok_or("root VM or common input state is unsettled")?;
        if self.host_story_pe_sha256() != Some(PE) || self.default_entry() != Some("kcs.int/main.kcs") {
            return Err("Campanella PE or entry differs");
        }
        let services = &self.data_services;
        let input = &services.messages;
        if !input.input_frame_complete {
            return Err("input scan has not completed the frame");
        }
        let records = self.device_key_scan_state().records();
        if records.iter().flatten().any(|key| key.mask & 15 != 0) {
            return Err("logical key scan still has an active device mask");
        }
        // A removed title button cannot receive KeyUp. Only its recorded
        // Mouse1 ownership may remain after the scanned release.
        let title_retired = title_owner.is_some_and(|owner| {
            records[0][0].mask == 0 && records[0][0].count == 0
                && services.tasks.fes.runtime(owner).is_none()
                && services.tasks.fes.platform.region_target(TITLE_REGION).is_none()
                && !live_child(self, TITLE_REGION, owner, 6)
        });
        if input.key_capture.iter().any(|(&key, &region)| {
            !(title_retired && key == 0 && region == TITLE_REGION)
        }) {
            return Err("logical key capture is still owned");
        }
        if input.typed_physical_owners.iter().any(|(&(source, code), &(key, group, owner))| {
            !(title_retired && Some(owner) == title_owner
                && (source, code, key, group) == (ButtonSource::Mouse, 1, 0, 0))
        }) {
            return Err("physical button ownership is still held");
        }
        Ok(frame)
    }

    pub fn host_campanella_input_diagnostics(&self, title_owner: u32) -> String {
        let services = &self.data_services;
        let input = &services.messages;
        let scan: Vec<_> = input.key_scan.records().iter().enumerate()
            .flat_map(|(key, groups)| groups.iter().enumerate().map(move |(group, state)| (key, group, state)))
            .filter(|(_, _, state)| state.mask != 0 || state.count != 0)
            .take(12)
            .map(|(key, group, state)| (key, group, state.mask, state.count, state.consumed))
            .collect();
        let title_fes = services.tasks.fes.runtime(title_owner).is_some();
        let title_region = services.tasks.fes.platform.region_target(TITLE_REGION).map(|(owner, _)| owner);
        format!(
            "18 input {} title_owner={title_owner} title_fes={title_fes} title_region_owner={title_region:?} \
             device_loaded={} device_foreground={} host_pressed={} physical={:?} edges={:?} \
             queued(host,device,fes)=({},{},{}) dispatch(key,select,button,active)=({},{},{},{}) \
             capture={:?} owners={:?} scan(key,group,mask,count,consumed)={scan:?}",
            self.host_tick_state(), input.device_keys_loaded, input.device_foreground,
            self.state.input.pressed.len(), input.physical_buttons, input.physical_edges,
            services.pending_host_input.len(), input.pending_device_buttons.len(), self.pending_fes_events.len(),
            input.key_dispatch.is_some(), input.key_selection.is_some(),
            input.button_dispatches.len(), input.button_active.len(), input.key_capture,
            input.typed_physical_owners,
        )
    }

    pub fn host_campanella_title(&self) -> Option<u32> {
        let frame = self.campanella_quiet(None).ok()?;
        if !self.cst_contexts.is_empty() {
            return None;
        }
        let tasks = &self.data_services.tasks;
        let mut flow = None;
        let mut title = None;
        for (owner, name, runtime, waiting) in tasks.fes.story_runtimes() {
            let name = name.strip_suffix(b".fes").unwrap_or(name);
            let driver = runtime.driver.as_ref()?;
            let (expected, slot) = match name {
                b"flow" => (b"TITLE_WAIT".as_slice(), &mut flow),
                b"title" => (b"WAIT".as_slice(), &mut title),
                _ => return None,
            };
            if waiting
                || driver.initialization_pending()
                || slot.replace(owner).is_some()
                || !settled_poll(
                    driver.interpreter.current_block(),
                    expected,
                    driver.has_pending_handler(),
                    driver.waiting(),
                    frame,
                )
            {
                return None;
            }
        }
        let flow = flow?;
        let title = title?;
        (live_child(self, flow, 0, 34)
            && live_child(self, title, flow, 34)
            && live_child(self, TITLE_REGION, title, 6)
            && tasks.fes.platform.region_target(TITLE_REGION).is_some_and(|(owner, target)| {
                owner == title && target.is_some_and(|target| target.name.as_slice() == b"btn_a_base")
            })
            && self.input_button_state(TITLE_REGION).is_none_or(|button| {
                button.receiver == Some(title) && button.show != 0
            }))
            .then_some(title)
    }

    pub fn host_campanella_localized_entry(
        &self, title_owner: u32, title_frame: u64,
    ) -> Result<(u32, u64, u64), &'static str> {
        let (body, wait) = self.host_campanella_wait(0, title_owner)?;
        let loaded = self.cst_contexts[&body.context].story.loaded;
        if wait != 94 || loaded <= title_frame {
            return Err("localized CST load did not follow the title press");
        }
        Ok((body.context, loaded, body.marked))
    }

    pub fn host_campanella_wait(&self, group: i32, title_owner: u32) -> Result<(CampanellaWait, i32), &'static str> {
        let frame = self.campanella_quiet(Some(title_owner))?;
        let services = &self.data_services;
        let tasks = &services.tasks;
        let mut runtimes = tasks.fes.story_runtimes();
        let (flow, name, runtime, waiting) = runtimes.next().ok_or("flow FES missing")?;
        let driver = runtime.driver.as_ref().ok_or("flow FES driver missing")?;
        if runtimes.next().is_some()
            || name.strip_suffix(b".fes").unwrap_or(name) != b"flow"
            || waiting
            || driver.initialization_pending()
            || !settled_poll(
                driver.interpreter.current_block(),
                b"GAME_WAIT",
                driver.has_pending_handler(),
                driver.waiting(),
                frame,
            )
            || !live_child(self, flow, 0, 34)
        {
            return Err("flow FES has not settled in GAME_WAIT");
        }

        let mut contexts = self.cst_contexts.iter();
        let (&context, state) = contexts.next().ok_or("CST context missing")?;
        let (marked, actual_group, actual_wait) = state.story.settled_body_record(frame)
            .ok_or("executed CST input marker missing or pending")?;
        if contexts.next().is_some()
            || !state.story.is_scene("0001_00")
            || actual_group != group
            || state.localization.is_some()
            || state.encoding != ScriptTextEncoding::Cp932
        {
            return Err("active CST page differs");
        }
        let (expected, probe) = [
            (false, CstSourceProbe::Campanella),
            (true, CstSourceProbe::CampanellaLocalized),
        ].into_iter().find_map(|(localized, probe)| {
            let (wait, records) = page_records(localized, group)?;
            (wait == actual_wait).then_some((records, probe))
        }).ok_or("CST input record is outside the guarded route")?;
        let origins = services
            .cst_text_origins
            .source_record_observations(context);
        // Startup reads these records before MesWnd2 draws them. The CST
        // observations identify the source bytes for this load. The executed
        // input marker, live string contents and reveal receipt prove this page.
        let mut load = None;
        for &record in expected {
            let entry = origins.iter().find(|entry| {
                entry.source.context_handle == context
                    && entry.source.group == group
                    && entry.source.absolute_record == record
                    && entry.source.load_id != 0
                    && load.is_none_or(|id| id == entry.source.load_id)
                    && (state.story.loaded..=marked).contains(&entry.frame)
                    && entry.probe == probe
                    && entry.native == 327
                    && entry.kind == 0x20
                    && entry.stored_codec == ScriptTextEncoding::Cp932
                    && entry.output_codec == ScriptTextEncoding::Cp932
                    && entry.raw_bytes != 0
                    && entry.raw_bytes == entry.output_bytes
                    && entry.body_tagged
                    && !entry.localized
            }).ok_or("current load has no matching CST body observation")?;
            load = Some(entry.source.load_id);
        }

        let mut windows = self
            .scene_nodes
            .values()
            .filter(|node| node.name == "meswnd" && live_child(self, node.id, node.parent, 12));
        let window = windows.next().ok_or("live ordinary meswnd missing")?;
        let owner = window.id;
        let script = window.parent;
        if windows.next().is_some()
            || !live_child(self, script, flow, 0)
            || self.task_vms.get(&script).is_none_or(|vm| vm.program().is_none())
            || tasks.task_clock(script).is_none_or(|clock| clock.paused != 0)
        {
            return Err("ordinary meswnd or script ancestry differs");
        }
        let mut planes = self
            .scene_nodes
            .values()
            .filter(|node| node.name == "message main" && live_child(self, node.id, owner, 2));
        let plane = planes.next().ok_or("live message main plane missing")?.id;
        if planes.next().is_some() {
            return Err("multiple live message main planes");
        }
        let mut strings = self
            .scene_nodes
            .values()
            .filter(|node| node.name == "string" && live_child(self, node.id, plane, 45));
        let string = strings.next().ok_or("live kind-45 string missing")?.id;
        if strings.next().is_some() || services.messages.legacy_string_kind(string) != Some(45) {
            return Err("kind-45 string binding differs");
        }
        let object = services.messages.object(string).ok_or("kind-45 object missing")?;
        let surface = services.message_device.surface(string as i32)
            .ok_or("kind-45 surface missing")?;
        if !object.prepared() || object.remaining_all().ok() != Some(0) {
            return Err("kind-45 body reveal incomplete");
        }
        if surface.generation != object.surface_generation() {
            return Err("kind-45 surface generation differs");
        }
        if surface.dirty.is_some() {
            return Err("kind-45 surface still has an unsubmitted rectangle");
        }

        // Tie the current ordinary text object to the exact bytes of this page.
        let document = state.document.as_ref().ok_or("CST document missing")?;
        let mut body = Vec::new();
        for &record in expected {
            let text = &document.records.get(record).ok_or("CST body record missing")?.text;
            body.extend_from_slice(text.splitn(2, |&byte| byte == 0).next().unwrap_or_default());
        }
        let appended = object.raw_appends().concat();
        let mut submitted_body = appended.as_slice();
        while let Some(core) = submitted_body
            .strip_suffix(b"\\n")
            .or_else(|| submitted_body.strip_suffix(b"\n"))
        {
            submitted_body = core;
        }
        if submitted_body != body.as_slice() {
            return Err("kind-45 append bytes differ from the current CST body");
        }

        let reveal_finished = format!(
            "index=69 arguments=16 bytes [{:08x}0d0000000000000000000000]",
            owner.swap_bytes()
        );
        if !self
            .events
            .iter()
            .rev()
            .take_while(|event| event.frame >= marked)
            .any(|event| {
                event.frame < frame
                    && event.kind == "native-call"
                    && event.detail.starts_with(&reveal_finished)
            })
        {
            return Err("current page native69 reveal-complete receipt missing");
        }
        let projected = self
            .project_message_plane(self.scene_nodes.get(&string).ok_or("kind-45 node missing")?)
            .map_err(|_| "kind-45 projection failed")?;
        let ((matched, opaque), origin, clipping) = self
            .message_plane_rgb_probe(object, &projected, surface.submitted)
            .ok_or("kind-45 projection cannot be sampled")?;
        if opaque == 0 || matched != opaque || (group == 0 && origin != [200, 487]) || clipping != (0, 0) {
            return Err("kind-45 body pixels differ in the final composition");
        }
        Ok((CampanellaWait {
            context,
            load: load.ok_or("CST body load missing")?,
            script,
            owner,
            plane,
            string,
            generation: surface.generation,
            marked,
            opaque,
        }, actual_wait))
    }

    /// Report the eligible keyboard regions in the same order as device dispatch.
    pub fn host_campanella_enter(
        &mut self,
        body: CampanellaWait,
    ) -> anyhow::Result<Option<((u32, u32), Vec<(u32, u32)>)>> {
        let input = &self.data_services.messages;
        let bindings = self.device_key_bindings();
        if !enter_mapping(bindings, &self.input_task.key_enabled)
            || !bindings[0].enabled
            || !self.input_task.key_enabled[0]
            || bindings[0].codes[ButtonSource::Keyboard.index()][0] != 13
            || (0..80).any(|key| input.device_keys.forced_queries(key).ok() != Some(0))
        {
            return Ok(None);
        }
        let flow = self.task_parents.get(&body.script).copied();
        self.with_host(|host| {
            let mut candidates = Vec::new();
            let mut bottom = None;
            let mut seen = std::collections::BTreeSet::new();
            for &region in &host.input_task.regions {
                if region.words[8] == 0
                    || region.words[2] & ButtonSource::Keyboard.mask() == 0
                    || !host.host_input_region_ready(region.id())
                {
                    continue;
                }
                let id = region.id();
                let Some(node) = host.scene_nodes.get(&id) else {
                    return Ok(None);
                };
                if !seen.insert(id)
                    || !matches!(node.kind, 2 | 6)
                    || !host
                        .data_services
                        .tasks
                        .message_task_state(id)
                        .is_some_and(|(kind, deleted, _)| kind == node.kind && deleted == 0)
                {
                    return Ok(None);
                }
                let runtime = host.data_services.messages.input_buttons.get(&id);
                let receiver = host.effective_input_region_receiver(id);
                let Some(receiver) = receiver else {
                    continue;
                };
                if !host.task_is_reachable(receiver)
                    || !(receiver == 0
                        || receiver == body.script
                        || receiver == body.owner
                        || Some(receiver) == flow)
                {
                    return Ok(None);
                }
                candidates.push((id, receiver));
                if node.kind == 6 && node.name == "bottom button" && receiver == body.script {
                    let Some(button) = runtime else {
                        return Ok(None);
                    };
                    if node.parent != body.script
                        || host.task_parents.get(&id) != Some(&body.script)
                        || !bottom_registration(
                            &region.words,
                            &button.state,
                            id,
                            receiver,
                            button.execution_checkpoint().3,
                        )
                        || bottom.replace((id, receiver)).is_some()
                    {
                        return Ok(None);
                    }
                }
            }
            Ok(bottom.map(|bottom| (bottom, candidates)))
        })
    }
}
