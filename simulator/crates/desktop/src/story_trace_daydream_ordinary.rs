use super::{HeadlessPlayer, StoryTraceProfile};

impl HeadlessPlayer {

    pub(super) fn host_daydream_save_panel_diagnostic(&self) -> Option<String> {
        if self.host_story_trace_profile() != Some(StoryTraceProfile::Daydream) {
            return None;
        }
        let (owner, locals) = self.host_neko_menu_driver(b"saveload", b"WAIT")?;
        let snapshot = self.shared_numeric_state_snapshot().ok()?;
        let root = if snapshot.active_identity == 0 {
            &snapshot.active_values
        } else {
            snapshot.inactive_values.get(&0)?
        };
        let flags = [800, 801, 870, 981].map(|slot| root.get(slot).copied());
        let runtime = self.data_services.tasks.fes.runtime(owner)?;
        let button = runtime.media.get("btn_data[0]").map(|layer| (layer.enabled, layer.visible));
        let card = runtime.media.get("data[0]").map(|layer| (layer.visible, layer.opacity, layer.image.is_some()));
        Some(format!("owner={owner} page={:?} mode={:?} flags800_801_870_981={flags:?} button={button:?} card={card:?}",
            locals.get(100), locals.get(110)))
    }

    pub fn host_daydream_ordinary_empty_slot(
        &mut self, anchor: (u32, u64, u32, u64, u64),
    ) -> anyhow::Result<Option<([i32; 2], u32, u32)>> {
        let Some((owner, region)) = self.host_daydream_ordinary_save_panel(anchor)? else { return Ok(None); };
        Ok(self.host_daydream_save_control(owner, "btn_data", 0)?
            .filter(|(_, hit)| *hit == region).map(|(point, _)| (point, owner, region)))
    }

    pub fn host_daydream_ordinary_saved_panel(
        &self, anchor: (u32, u64, u32, u64, u64), panel: u32,
    ) -> anyhow::Result<bool> {
        let (context, load, owner, epoch, published) = anchor;
        let frame = self.state.clock.frame;
        if self.host_story_trace_profile() != Some(StoryTraceProfile::Daydream)
            || self.frame_in_progress() || !self.state.input.focused || self.data_services.host_pause.paused
            || self.data_services.host_quit_requested || !self.state.input.pressed.is_empty()
            || !self.pending_messages.is_empty() || !self.pending_fes_events.is_empty()
            || !self.data_services.pending_host_input.is_empty() || !self.movie_playbacks.is_empty()
            || self.host_neko_slot_zero_absent()
        {
            return Ok(false);
        }

        let retained = self.cst_contexts.get(&context).is_some_and(|context| {
            context.story.is_scene("c01_01_01")
                && context.story.settled_body_record(frame).is_some_and(|(_, group, record)| (group, record) == (13, 121))
        });
        let source = self.data_services.fes_text.story_body_sources(owner, epoch, &self.data_services.cst_text_origins);
        if !retained || !self.data_services.tasks.task_is_registered(owner)
            || !self.data_services.fes_text.story_body_generation(owner)
                .is_some_and(|generation| (generation.source_epoch, generation.source_frame) == (epoch, published))
            || !source.is_some_and(|sources| matches!(sources, [source]
                if (source.context_handle, source.load_id, source.group, source.absolute_record)
                    == (context, load, 13, 120)))
        {
            return Ok(false);
        }
        let Some((current, locals)) = self.host_neko_menu_driver(b"saveload", b"WAIT") else { return Ok(false); };
        let snapshot = self.shared_numeric_state_snapshot()?;
        let root = if snapshot.active_identity == 0 { Some(&snapshot.active_values) } else { snapshot.inactive_values.get(&0) };
        Ok(current == panel && locals.get(100) == Some(&0) && locals.get(110) == Some(&0)
            && root.is_some_and(|root| root.get(800) == Some(&0) && root.get(801) == Some(&1)
                && root.get(870).is_some_and(|occupied| *occupied != 0)))
    }

    pub fn host_daydream_ordinary_load_control(
        &mut self, control: &str, expected_panel: Option<u32>,
    ) -> anyhow::Result<Option<([i32; 2], u32, u32, bool)>> {
        if self.host_story_trace_profile() != Some(StoryTraceProfile::Daydream)
            || !self.data_services.story_trace || self.frame_in_progress()
            || !self.state.input.focused || self.data_services.host_pause.paused
            || self.data_services.host_quit_requested || !self.state.input.pressed.is_empty()
            || !self.pending_messages.is_empty() || !self.pending_fes_events.is_empty()
            || !self.data_services.pending_host_input.is_empty() || !self.movie_playbacks.is_empty()
            || self.host_pointer_move_state().remaining > 0
            || !self.cst_contexts.is_empty() || self.message_wait_address != Some(0xd6d)
            || !self.vm.as_ref().is_some_and(|vm| vm.waiting_for_message())
        {
            return Ok(None);
        }

        let Some((flow, _)) = self.host_neko_menu_driver(b"flow", b"TITLE_WAIT") else { return Ok(None); };
        let ttl_block: &[u8] = if control == "title" { b"FES_WAIT" } else { b"LOAD_WAIT" };
        let Some((ttl, _)) = self.host_neko_menu_driver(b"ttlflow", ttl_block) else { return Ok(None); };
        if self.task_parents.get(&ttl) != Some(&flow) {
            return Ok(None);
        }
        let snapshot = self.shared_numeric_state_snapshot()?;
        let root = if snapshot.active_identity == 0 { Some(&snapshot.active_values) } else { snapshot.inactive_values.get(&0) };
        let Some(root) = root else { return Ok(None); };
        let (owner, name, bank, confirm) = if control == "title" {
            if root.get(541) != Some(&0) || root.get(542) != Some(&1)
                || self.data_services.tasks.fes.story_runtimes().count() != 3
            {
                return Ok(None);
            }
            let Some((title, _)) = self.host_neko_menu_driver(b"title", b"WAIT") else { return Ok(None); };
            if self.task_parents.get(&title) != Some(&ttl) {
                return Ok(None);
            }
            (title, "btn1", 1, false)
        } else {
            let block: &[u8] = if control == "confirm" { b"CONFIRM_WAIT" } else { b"WAIT" };
            let Some((panel, locals)) = self.host_neko_menu_driver(b"saveload", block) else { return Ok(None); };
            if self.task_parents.get(&panel) != Some(&ttl)
                || locals.get(100) != Some(&0) || locals.get(110) != Some(&1)
                || root.get(800) != Some(&1) || root.get(801) != Some(&0)
                || !root.get(870).is_some_and(|occupied| *occupied != 0)
            {
                return Ok(None);
            }
            match control {
                "slot" if self.data_services.tasks.fes.story_runtimes().count() == 3
                    && matches!(root.get(981), Some(0 | 1)) => (panel, "btn_data", 0, root[981] == 1),
                "confirm" if expected_panel == Some(panel)
                    && self.data_services.tasks.fes.story_runtimes().count() == 4
                    && locals.first() == Some(&1) && locals.get(101) == Some(&0) && locals.get(102) == Some(&0)
                    && root.get(981) == Some(&1) && root.get(1020) == Some(&1) => {
                    let Some((child, _)) = self.host_neko_menu_driver(b"conf", b"WAIT") else { return Ok(None); };
                    if self.task_parents.get(&child) != Some(&panel) {
                        return Ok(None);
                    }
                    (child, "btn", 0, false)
                }
                _ => return Ok(None),
            }
        };
        Ok(self.host_daydream_save_control(owner, name, bank)?.map(|(point, region)| (point, owner, region, confirm)))
    }
}
