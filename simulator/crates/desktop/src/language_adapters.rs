//! Default adapters for the current desktop media and ordinary-task models.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use cs2_runtime::TaskCommand;
use cs2_runtime::language::{LanguageNotificationCache, LanguageState, LanguageTarget};

use super::HeadlessPlayer;

pub(super) type OrdinaryConfiguration =
    fn(&mut HeadlessPlayer, u32, u32, &LanguageState) -> Result<()>;

#[derive(Debug, Clone, Default)]
pub(super) struct LanguageAdapterState {
    pub(super) resolved_title: Option<String>,
    pub(super) button_geometry: BTreeMap<String, [i32; 4]>,
    ordinary: BTreeMap<u32, LanguageNotificationCache>,
    ordinary_configuration: Option<OrdinaryConfiguration>,
    configuration: Option<ConfigurationFrame>,
}

#[derive(Debug, Clone)]
struct ConfigurationFrame {
    languages: LanguageState,
    tasks: Vec<u32>,
    next_task: usize,
    fes: Option<FesControl>,
    fes_started: bool,
    media: Vec<String>,
    next_media: usize,
}

#[derive(Debug, Clone)]
struct FesControl {
    section: usize,
    line: usize,
    wait_frames: u32,
    exit_code: Option<i32>,
    event_return: Option<(usize, usize, u32)>,
    event_target: Option<String>,
    event_bank: Option<i32>,
}

impl LanguageAdapterState {
    pub(super) fn has_configuration_continuation(&self) -> bool {
        self.configuration.is_some()
    }
}

impl HeadlessPlayer {
    /// Keep custom adapters installed by the window/task host. The default
    /// interface adapter updates headless title state; a real window consumes
    /// language_window_title() when presenting that frame.
    pub(super) fn install_default_language_adapters(&mut self) {
        let cache = LanguageNotificationCache::new(&self.data_services.language.languages);
        let platform = &mut self.data_services.language.platform;
        platform
            .interface_refresh
            .get_or_insert(default_interface_refresh);
        platform.text_refresh.get_or_insert(default_text_refresh);
        platform
            .button_refresh
            .get_or_insert(default_button_refresh);
        platform
            .configuration_refresh
            .get_or_insert(default_configuration_refresh);
        platform
            .adapters
            .ordinary
            .entry(self.current_task_id)
            .or_insert(cache);
    }

    /// C's task-store bridge must update its private per-object language cache
    /// even for unloaded tasks, and enqueue messages without entering a VM.
    pub(super) fn set_language_ordinary_configuration(&mut self, callback: OrdinaryConfiguration) {
        self.data_services
            .language
            .platform
            .adapters
            .ordinary_configuration = Some(callback);
    }

    pub(super) fn resolved_language_title(&self) -> Option<&str> {
        self.data_services
            .language
            .platform
            .adapters
            .resolved_title
            .as_deref()
    }
}

fn default_interface_refresh(player: &mut HeadlessPlayer) -> Result<()> {
    let title = player.language_window_title()?;
    // Tables were reloaded before this adapter. Store the resolved title used
    // by headless hosts; window hosts apply it through their window handle.
    player
        .data_services
        .language
        .platform
        .adapters
        .resolved_title = title;
    player.refresh_language_media()?;
    player.request_language_menu_refresh()?;
    Ok(())
}

fn default_text_refresh(player: &mut HeadlessPlayer, target: LanguageTarget) -> Result<()> {
    // Ordinary CST readers feed the existing shared strings and FES handlers.
    // They are refreshed by the command-19 task notification below, not by
    // the optional direct-CST tool view (including its subtitle limitation).
    let Some(cst) = player.cst.as_ref() else {
        return Ok(());
    };
    if target == LanguageTarget::Subtitle {
        let index = player.data_services.language.languages.subtitle_index;
        if !player
            .data_services
            .language
            .languages
            .contains_index(index)
        {
            return Ok(());
        }
        bail!("subtitle request retained: this desktop scene has no subtitle renderer");
    }
    let name = player
        .current_cstl_language_name()
        .context(
            "main-text request retained: no CSTL name exactly matches the selected prefix bytes",
        )?
        .to_owned();
    let table = cst
        .localization
        .as_ref()
        .context("main-text request retained: the active scene has no CSTL table")?;
    // The active language selects a separate UTF-8 CSTL view. Preserve the
    // original scene bytes and its captured codec: neither switching language
    // nor a missing bound fragment may transcode/overwrite those bytes.
    let mut choices = cst.choices.clone();
    if let Some(choices) = &mut choices {
        for (index, choice) in choices.iter_mut().enumerate() {
            let localized = table
                .option(&name, cst.document.inputs.len(), index)
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| choice.source_text.clone());
            choice.text = localized;
        }
    }
    let choices_changed = cst.choices != choices;
    let cst = player
        .cst
        .as_mut()
        .context("active CST disappeared during language refresh")?;
    cst.language = name;
    cst.choices = choices;
    if choices_changed {
        // The existing event is the direct view's choice consumer. Publish
        // the changed labels without collecting conditions again or touching
        // ids, targets, order, pending input, or the suspended scene cursor.
        if let Some(choices) = &cst.choices {
            player.events.push(super::LogicEvent {
                frame: player.state.clock.frame,
                kind: "cst-choices".into(),
                detail: choices.iter()
                    .map(|choice| format!("{}:{}:{}", choice.id, choice.target, choice.text))
                    .collect::<Vec<_>>().join("|"),
            });
        }
    }
    Ok(())
}

fn default_button_refresh(player: &mut HeadlessPlayer, name: &str) -> Result<()> {
    let fes = player.fes.as_ref().context("FES button scene is absent")?;
    let layer = fes.media.get(name).context("FES button layer is absent")?;
    let image = layer.image.as_ref().context("FES button image is absent")?;
    let position = layer.draw_position(&fes.planes);
    let right = i64::from(position.x) + i64::from(image.width());
    let bottom = i64::from(position.y) + i64::from(image.height());
    let geometry = [
        position.x,
        position.y,
        i32::try_from(right).context("FES button right edge exceeds i32")?,
        i32::try_from(bottom).context("FES button bottom edge exceeds i32")?,
    ];
    // FesRuntime::button_contains reads this same layer/image on every hit
    // test. Replacing its pixels updates the live input rectangle immediately.
    // Ordinary input regions have independent ids and are not fabricated here.
    player
        .data_services
        .language
        .platform
        .adapters
        .button_geometry
        .insert(name.to_owned(), geometry);
    Ok(())
}

fn default_configuration_refresh(player: &mut HeadlessPlayer) -> Result<bool> {
    let has_root = player.vm.is_some() || player.fes.is_some() || player.cst.is_some();
    if !has_root {
        return Ok(true);
    }
    let mut frame = match player
        .data_services
        .language
        .platform
        .adapters
        .configuration
        .take()
    {
        Some(frame) => frame,
        None => ConfigurationFrame {
            languages: player.data_services.language.languages.clone(),
            tasks: ordinary_traversal(player)?,
            next_task: 0,
            fes: None,
            fes_started: false,
            media: player
                .fes
                .as_ref()
                .map(|fes| fes.media.keys().cloned().collect())
                .unwrap_or_default(),
            next_media: 0,
        },
    };
    let result = refresh_configuration_frame(player, &mut frame);
    match result {
        Ok(true) => {
            player.data_services.language.system_settings_dirty = 0;
            Ok(true)
        }
        Ok(false) => {
            player.data_services.language.platform.adapters.configuration = Some(frame);
            Ok(false)
        }
        Err(error) => {
            player.data_services.language.system_settings_dirty = 1;
            player
                .data_services
                .language
                .platform
                .adapters
                .configuration = Some(frame);
            Err(error)
        }
    }
}

fn ordinary_traversal(player: &HeadlessPlayer) -> Result<Vec<u32>> {
    let mut result = Vec::new();
    let mut visited = std::collections::BTreeSet::new();
    let mut pending = vec![player.current_task_id];
    // Include detached objects after the root traversal, retaining signed-id
    // sibling ordering for the ordinary task model.
    let mut roots: Vec<_> = player
        .task_parents
        .iter()
        .filter(|(_, parent)| {
            **parent != player.current_task_id && !player.task_parents.contains_key(*parent)
        })
        .map(|(&id, _)| id)
        .collect();
    roots.sort_by_key(|&id| id as i32);
    roots.reverse();
    let mut detached = roots;
    while let Some(id) = pending.pop().or_else(|| detached.pop()) {
        if !visited.insert(id) {
            bail!("configuration traversal contains a task-parent cycle");
        }
        result.push(id);
        let mut children: Vec<_> = player
            .task_parents
            .iter()
            .filter(|(child, parent)| **parent == id && **child != id)
            .map(|(&child, _)| child)
            .collect();
        children.sort_by_key(|&child| child as i32);
        children.reverse();
        pending.extend(children);
    }
    Ok(result)
}

fn refresh_configuration_frame(
    player: &mut HeadlessPlayer,
    frame: &mut ConfigurationFrame,
) -> Result<bool> {
    while let Some(&id) = frame.tasks.get(frame.next_task) {
        if id == player.current_task_id
            && player.data_services.tasks.fes.is_direct_entry(id)
        {
            let done = player.with_host(|host| {
                let mut output = 0;
                Ok(host.receive_task_command(
                    TaskCommand { target: id as i32, sender: 0, code: 19, data: [0; 3] },
                    &mut cs2_runtime::TaskCommandBuffers::default(), &mut output, 4096,
                )?.is_none_or(|outcome| outcome.done))
            })?;
            if !done { return Ok(false); }
        } else if id == player.current_task_id {
            let cache = player
                .data_services
                .language
                .platform
                .adapters
                .ordinary
                .entry(id)
                .or_insert_with(|| LanguageNotificationCache::new(&frame.languages));
            let messages = cache.configuration_messages(0, &frame.languages, player.vm.is_some());
            player.pending_messages.extend(messages);
        } else if player.task_parents.contains_key(&id) || player.scene_nodes.contains_key(&id) {
            if let Some(notify) = player
                .data_services
                .language
                .platform
                .adapters
                .ordinary_configuration
            {
                notify(player, id, 0, &frame.languages)?;
            } else if let Some((kind, deletion, _)) =
                player.data_services.tasks.message_task_state(id)
            {
                if deletion == 0 && matches!(kind, 0 | 12 | 13 | 34) {
                    let done = player.with_host(|host| {
                        let mut output = 0;
                        Ok(host.receive_task_command(
                            TaskCommand { target: id as i32, sender: 0, code: 19, data: [0; 3] },
                            &mut cs2_runtime::TaskCommandBuffers::default(), &mut output, 4096,
                        )?.is_none_or(|outcome| outcome.done))
                    })?;
                    if !done { return Ok(false); }
                }
            } else if player.task_vms.contains_key(&id) {
                bail!(
                    "configuration request retained: loaded ordinary task has no lifetime metadata"
                );
            }
        }
        frame.next_task += 1;
    }
    if !refresh_fes_configuration(player, frame)? {
        return Ok(false);
    }
    // The desktop renderer submits the full canvas every frame; rebuilding the
    // current media sources is the concrete image invalidation for this host.
    player.refresh_language_media()?;
    while let Some(name) = frame.media.get(frame.next_media).cloned() {
        if let Some(layer) = player.fes.as_ref().and_then(|fes| fes.media.get(&name)) {
            if matches!(layer.kind.as_str(), "IMAGE" | "BUTTON")
                && layer.file.as_ref().is_some_and(|name| !name.is_empty())
            {
                let button = layer.kind == "BUTTON";
                let mut replacement = layer.clone();
                super::load_fes_media_image(&mut replacement, &player.scan.resources)
                    .with_context(|| format!("invalidate FES configuration image {name:?}"))?;
                if let Some(layer) = player.fes.as_mut().and_then(|fes| fes.media.get_mut(&name)) {
                    *layer = replacement;
                }
                if button {
                    default_button_refresh(player, &name)?;
                }
            }
        }
        frame.next_media += 1;
    }
    Ok(true)
}

fn refresh_fes_configuration(
    player: &mut HeadlessPlayer,
    frame: &mut ConfigurationFrame,
) -> Result<bool> {
    let Some(fes) = player.fes.as_mut() else {
        return Ok(true);
    };
    if !frame.fes_started {
        frame.fes_started = true;
        let Some(section) = fes
            .document
            .sections
            .iter()
            .position(|section| section.header.eq_ignore_ascii_case(b"MES_CHANGECONFIG"))
        else {
            return Ok(true);
        };
        frame.fes = Some(FesControl {
            section: fes.section,
            line: fes.line,
            wait_frames: fes.wait_frames,
            exit_code: fes.exit_code,
            event_return: fes.event_return,
            event_target: fes.event_target.clone(),
            event_bank: fes.event_bank,
        });
        fes.section = section;
        fes.line = 0;
        fes.wait_frames = 0;
        fes.exit_code = None;
        fes.event_return = Some((usize::MAX, 0, 0));
        fes.event_target = None;
        fes.event_bank = None;
    }
    if frame.fes.is_none() {
        return Ok(true);
    }
    // Use the actual scene interpreter. Its successful media/numeric changes
    // remain in the scene; restore only the suspended body/event control.
    fes.advance(
        player.state.clock.frame,
        &player.scan.resources,
        &mut player.events,
        &mut player.scene_nodes,
    );
    if fes.section != usize::MAX {
        return Ok(false);
    }
    let saved = frame
        .fes
        .take()
        .context("FES configuration control snapshot is absent")?;
    fes.section = saved.section;
    fes.line = saved.line;
    fes.wait_frames = saved.wait_frames;
    fes.exit_code = saved.exit_code;
    fes.event_return = saved.event_return;
    fes.event_target = saved.event_target;
    fes.event_bank = saved.event_bank;
    Ok(true)
}
