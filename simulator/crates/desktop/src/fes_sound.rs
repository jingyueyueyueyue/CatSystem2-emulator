//! FES SOUND banks over the desktop decoder and physical sound channels.
//! Original encoded names, signed properties and command continuations are
//! retained. No VM pointer or original-engine task is fabricated.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use cs2_runtime::{TaskCommand, TaskCommandBuffers};
use cs2_script::{ExpressionState, FesEvent, FesObjectInitialization, NativeReturn};

use super::fes_driver::{
    FesDriverCommand, FesDriverEffect, FesDriverTarget,
    FesObjectEventPhase, FesScriptDriver,
};
use super::fes_tasks::FesTaskView;
use super::{FesRuntime, LoggingHost};
use super::sound_media::{
    SoundMediaSubmission, SoundMediaRequest, SoundMediaProgress,
    SoundMediaNotice, SoundMediaNoticeStep, SoundMediaNotification,
};

#[path = "fes_playlist.rs"]
mod playlist;

#[path = "fes_sound_state.rs"]
mod checkpoint;

type Key = (u32, Vec<u8>, i32);
const MAX_BANKS: usize = 65_536;

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct SoundBank {
    deleted: bool,
    playlist: Option<playlist::Playlist>,
    alias: Vec<u8>,
    notice: Option<NoticeWork>,
    file: Vec<u8>,
    group: Vec<u8>,
    channel: i32,
    stype: i32,
    category_type: i32,
    volume: i32,
    pan: i32,
    load_values: [i32; 2],
    play_values: [i32; 3],
    load_stage: Option<u8>,
    dirty: bool,
    action: bool,
    requested_play: bool,
    enabled: bool,
    submission: Option<SoundMediaSubmission>,
    pending_stop_notification: bool,
    play_changed: bool,
    action_stage: u8,
    allowed: bool,
    callback_done: bool,
    started_tick: u32,
    channel_result: u32,
    media_result: Option<MediaResult>,
    result_pending: bool,
}

/// The original retained media record's +4c/+54. Publish only after the
/// SOUND media call (including its synchronous notification) really returns.
#[derive(Clone, Copy, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct MediaResult {
    channel: i32,
    release: i32,
}

impl Default for SoundBank {
    fn default() -> Self {
        Self {
            deleted: false,
            playlist: None,
            alias: Vec::new(), notice: None,
            file: Vec::new(), group: Vec::new(), channel: -1, stype: 0,
            category_type: 0, volume: 100, pan: 0, load_values: [0; 2],
            play_values: [0; 3], load_stage: None, dirty: false, action: false,
            requested_play: false, enabled: true, submission: None,
            pending_stop_notification: false,
            play_changed: false, action_stage: 0, allowed: false,
            callback_done: false, started_tick: u32::MAX, channel_result: 0,
            media_result: None, result_pending: false,
        }
    }
}

#[derive(Clone, Copy, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
enum NoticeWaiting { Alias, Object }

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct NoticeWork {
    state: SoundMediaNotice,
    notification: SoundMediaNotification,
    waiting: Option<NoticeWaiting>,
}

fn start_notice(record: &mut SoundBank, key: &Key, notification: SoundMediaNotification) -> Result<()> {
    if record.notice.is_some() { return Ok(()); }
    record.notice = Some(NoticeWork {
        state: SoundMediaNotice::new(&key.1, key.2, &record.alias, notification)?,
        notification, waiting: None,
    });
    record.callback_done = false;
    Ok(())
}

/// Only schedule effects here: the owner has temporarily moved the sound
/// store out of the host. The outer task pump applies them after it is back,
/// so an alias can issue commands against the same live SOUND bank.
fn advance_notice(record: &mut SoundBank, driver: &mut FesScriptDriver) -> Result<bool> {
    let Some(mut notice) = record.notice.take() else { return Ok(true); };
    let result = (|| {
        for _ in 0..4 {
            if let Some(waiting) = notice.waiting {
                match waiting {
                    NoticeWaiting::Alias => {
                        let Some(result) = driver.take_media_statement_result() else { return Ok(false); };
                        // The original outer loop resets wait_output to zero
                        // for each attempt rather than delaying on this value.
                        let _wait_output = result.wait_output;
                        notice.state.alias_returned(result.complete, result.statement)?;
                    }
                    NoticeWaiting::Object => {
                        if !record.callback_done { return Ok(false); }
                        notice.state.object_returned()?;
                        record.callback_done = false;
                    }
                }
                notice.waiting = None;
            }
            match notice.state.poll() {
                SoundMediaNoticeStep::Alias { header, statement, object, bank } => {
                    driver.begin_media_statement(&header, statement, FesDriverTarget {
                        name: object, bank, default_group: None, value: -1,
                    })?;
                    notice.state.alias_started()?;
                    notice.waiting = Some(NoticeWaiting::Alias);
                    return Ok(false);
                }
                SoundMediaNoticeStep::Object { object, bank, notification } => {
                    let immediate = driver.begin_media_event(FesDriverTarget {
                        name: object, bank, default_group: None, value: -1,
                    }, notification.event())?;
                    notice.state.object_started()?;
                    if matches!(immediate, Some(FesDriverEffect::ObjectEvent {
                        phase: FesObjectEventPhase::Complete { .. }, ..
                    })) {
                        notice.state.object_returned()?;
                    } else {
                        notice.waiting = Some(NoticeWaiting::Object);
                        return Ok(false);
                    }
                }
                SoundMediaNoticeStep::Complete => return Ok(true),
                SoundMediaNoticeStep::Waiting => bail!("SOUND media notice lost its running dispatcher"),
            }
        }
        Ok(false)
    })();
    if !matches!(result, Ok(true)) { record.notice = Some(notice); }
    result
}

#[derive(Clone, Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct FesSoundStore {
    banks: BTreeMap<Key, SoundBank>,
    declarations: BTreeSet<(u32, usize)>,
    initializers: BTreeMap<(u32, usize), usize>,
    commands: BTreeMap<(u32, usize), CommandProgress>,
    callbacks: BTreeMap<u32, Key>,
    updates: BTreeMap<u32, UpdateProgress>,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct CommandProgress {
    keys: Vec<Key>,
    cursor: usize,
    stage: u8,
    value: Option<i32>,
    values: Vec<i32>,
    raw: Option<Vec<u8>>,
    release: Option<ReleaseProgress>,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct ReleaseProgress {
    items: Vec<super::fes_task_platform::FesBankRelease>,
    cursor: usize,
    accepted: bool,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct UpdateProgress {
    now: u32,
    keys: Vec<Key>,
    cursor: usize,
    stop_scan: bool,
}

#[derive(cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct SoundCheckpoint {
    version: u32,
    store: FesSoundStore,
}

impl FesSoundStore {
    /// Existing CSS graph component codec; this is the live bank/receipt
    /// store, not a second playback map or a declaration replay on restore.
    pub(crate) fn capture_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<Vec<u8>> {
        self.validate_checkpoint(kinds)?;
        Ok(cs2_runtime::object_graph::encode_component(&SoundCheckpoint {
            version: 1, store: self.clone(),
        })?)
    }

    /// Prepare only. The caller commits this store in its existing graph
    /// transaction alongside the saved driver receipts and sound-task state.
    pub(crate) fn prepare_checkpoint(bytes: &[u8], kinds: &BTreeMap<u32, u32>) -> Result<Self> {
        let saved: SoundCheckpoint = cs2_runtime::object_graph::decode_component(bytes)?;
        if saved.version != 1 { bail!("SOUND checkpoint version is unsupported"); }
        saved.store.validate_checkpoint(kinds)?;
        Ok(saved.store)
    }

    fn validate_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<()> {
        let owner = |id: &u32| kinds.get(id) == Some(&34);
        if self.banks.len() > MAX_BANKS || self.commands.len() > MAX_BANKS
            || self.updates.len() > MAX_BANKS || self.callbacks.len() > MAX_BANKS
        { bail!("SOUND checkpoint exceeds live storage bounds"); }
        for ((id, name, bank), record) in &self.banks {
            if !owner(id) || name.len() > 31 || name.contains(&0) || *bank < 0
                || record.file.len() > 259 || record.group.len() > 31 || record.alias.len() > 31
                || record.action_stage > 3 || record.load_stage.is_some_and(|stage| stage > 2)
            { bail!("SOUND checkpoint has an invalid bank or continuation"); }
            if let Some(table) = &record.playlist { table.validate_checkpoint()?; }
        }
        for (id, key) in &self.callbacks {
            if *id != key.0 || !self.banks.get(key).is_some_and(|record| record.notice.is_some()) {
                bail!("SOUND checkpoint callback lost its live receipt owner");
            }
        }
        for ((id, _), progress) in &self.commands {
            if !owner(id) || progress.keys.len() > MAX_BANKS || progress.cursor > progress.keys.len()
                || progress.stage > 3 || progress.keys.iter().any(|key| key.0 != *id)
            { bail!("SOUND checkpoint command cursor is invalid"); }
            if let Some(release) = &progress.release {
                if release.items.len() > MAX_BANKS || release.cursor > release.items.len()
                    || release.items.iter().any(|item| item.bank < 0 || item.name.len() > 31)
                { bail!("SOUND checkpoint subtree release cursor is invalid"); }
            }
        }
        for (id, progress) in &self.updates {
            if !owner(id) || progress.keys.len() > MAX_BANKS || progress.cursor > progress.keys.len()
                || progress.keys.iter().any(|key| key.0 != *id)
            { bail!("SOUND checkpoint media cursor is invalid"); }
        }
        if self.declarations.iter().any(|(id, _)| !owner(id))
            || self.initializers.keys().any(|(id, _)| !owner(id))
        { bail!("SOUND checkpoint declaration owner is absent"); }
        Ok(())
    }

    fn keys(&self, owner: u32, name: &[u8], bank: Option<i32>) -> Vec<Key> {
        self.banks.iter().filter(|((id, object, index), record)|
            !record.deleted && *id == owner && object == name && bank.is_none_or(|bank| bank == *index))
            .map(|(key, _)| key.clone()).collect()
    }

    fn initialize(
        &mut self, owner: u32, init: &FesObjectInitialization,
        driver: &mut FesScriptDriver, shared: &mut ExpressionState,
    ) -> Result<bool> {
        let (name, index) = split_target(&init.target)?;
        if self.keys(owner, name, None).is_empty() { return Ok(false); }
        // The interpreter has already evaluated initialization bank syntax.
        let index = index.map(|index| -> Result<i32> {
            Ok(std::str::from_utf8(index)?.parse()?)
        }).transpose()?;
        let keys = self.keys(owner, name, index);
        let mut cursor = *self.initializers.get(&(owner, init.offset)).unwrap_or(&0);
        while cursor < init.columns.len().min(init.values.len()) {
            let column = init.columns[cursor].as_slice();
            let raw = init.values[cursor].as_slice();
            match column {
                b"FILE" | b"GROUP" => {
                    let bytes = bounded_name(raw, if column == b"FILE" { 260 } else { 32 })?;
                    for key in &keys {
                        let record = self.banks.get_mut(key).unwrap();
                        if column == b"FILE" {
                            record.file = bytes.clone();
                            record.load_stage = Some(0);
                        } else { record.group = bytes.clone(); }
                    }
                }
                b"SIZE" if keys.iter().any(|key| self.banks[key].playlist.is_some()) => {
                    let value = driver.evaluate_parameter(raw, 0, shared)?;
                    for key in &keys {
                        if let Some(table) = &mut self.banks.get_mut(key).unwrap().playlist {
                            table.resize(value)?;
                        }
                    }
                }
                b"CHANNEL" | b"STYPE" | b"TYPE" => {
                    let value = driver.evaluate_parameter(raw, 0, shared)?;
                    for key in &keys {
                        let record = self.banks.get_mut(key).unwrap();
                        match column {
                            b"CHANNEL" => {
                                record.dirty |= record.channel != value;
                                record.channel = value;
                            }
                            b"STYPE" => {
                                record.dirty |= record.stype != value;
                                record.stype = value;
                            }
                            _ => record.category_type = value,
                        }
                    }
                }
                // SOUND's original initializer ignores all other columns,
                // including BLEND/POS/ENABLE/DEFAULT, without numeric evaluation.
                _ => {}
            }
            cursor += 1;
            self.initializers.insert((owner, init.offset), cursor);
        }
        Ok(true)
    }

    fn command(
        &mut self, host: &mut LoggingHost<'_>, view: &FesTaskView,
        runtime: &mut FesRuntime,
        command: &FesDriverCommand, driver: &mut FesScriptDriver,
        shared: &mut ExpressionState,
    ) -> Result<bool> {
        let Some(target) = command.parameters.first() else { return Ok(false); };
        let (name, bank_source) = split_target(target)?;
        let all = self.keys(view.id, name, None);
        if command.parameters.get(1).is_some_and(|verb| verb == b"delete")
            && driver.adapter.object(name).is_some_and(|object|
                matches!(object.definition.kind_name.as_slice(), b"SOUND" | b"SNDTABLE")
                    || super::fes_text::is_text_kind(&object.definition.kind_name))
        {
            let stamp = (view.id, command.offset);
            if !self.commands.contains_key(&stamp) {
                let bank = bank_source.map(|source|
                    driver.evaluate_parameter(source, command.context.current, shared)).transpose()?;
                let count = driver.adapter.object(name).unwrap().definition.bank_count;
                let targets: Vec<_> = match bank {
                    Some(bank) if bank >= 0 => vec![bank],
                    None | Some(-1) => (0..count as i32).collect(),
                    _ => Vec::new(),
                };
                self.commands.insert(stamp, CommandProgress {
                    keys: targets.into_iter().map(|bank| (view.id, name.to_vec(), bank)).collect(),
                    cursor: 0, stage: 0, value: None, values: Vec::new(), raw: None, release: None,
                });
            }
            let mut progress = self.commands.remove(&stamp).unwrap();
            let result = (|| -> Result<()> {
                while let Some(key) = progress.keys.get(progress.cursor) {
                    // PE 2ca9cc: a hole ends the traversal rather than promoting
                    // another declaration or evaluating an expression fallback.
                    if progress.release.is_none() {
                        if driver.adapter.object_bank(&key.1, key.2 as usize).is_none() { break; }
                        let items = super::fes_task_platform::plan_bank_release(
                            host, runtime, driver, view.id, &key.1, key.2,
                        )?;
                        if items.is_empty() { bail!("FES delete has no shared motion identity"); }
                        // F validates its graphics/control owners before any
                        // deletion. Validate this detached store in that same
                        // plan, so a missing SOUND child cannot cause half-delete.
                        for item in items.iter().filter(|item| item.is_sound()) {
                            if self.banks.get(&(view.id, item.name.clone(), item.bank))
                                .is_none_or(|record| record.deleted)
                            { bail!("SOUND delete subtree has no live media bank owner"); }
                        }
                        progress.release = Some(ReleaseProgress { items, cursor: 0, accepted: false });
                    }
                    let release = progress.release.as_mut().unwrap();
                    while let Some(item) = release.items.get(release.cursor) {
                        if super::fes_task_platform::validate_bank_release(host, item)? {
                            if item.is_sound() {
                                if !release.accepted {
                                    release_bank(self, view.id, &item.name, item.bank)?;
                                    release.accepted = true;
                                }
                                super::fes_task_platform::finish_bank_release(host, driver, item)?;
                            } else {
                                super::fes_task_platform::release_graphics_bank(host, runtime, driver, item)?;
                            }
                        }
                        release.cursor += 1;
                        release.accepted = false;
                    }
                    progress.release = None;
                    progress.cursor += 1;
                }
                Ok(())
            })();
            if result.is_err() { self.commands.insert(stamp, progress); }
            return result.map(|_| true);
        }
        if all.is_empty() { return Ok(false); }
        let verb = command.parameters.get(1).context("SOUND needs a subcommand")?.as_slice();
        if !matches!(verb, b"load" | b"play" | b"channel" | b"stype" | b"blend" | b"enable" | b"loop" | b"crossfade" | b"time")
            && !playlist::is_command(verb) {
            bail!("SOUND subcommand {:?} has no connected host contract",
                String::from_utf8_lossy(verb));
        }
        // Ordinary enable does not change a SOUND's separate media switch.
        if verb == b"enable" { return Ok(true); }
        let stamp = (view.id, command.offset);
        if !self.commands.contains_key(&stamp) {
            let single = matches!(verb, b"play" | b"loop") || playlist::single_bank(verb);
            if single && bank_source.is_none() && all.len() != 1 {
                return Ok(true);
            }
            let index = bank_source.map(|source|
                driver.evaluate_parameter(source, command.context.current, shared)).transpose()?;
            if single && index == Some(-1) { return Ok(true); }
            let keys = self.keys(view.id, name, index.filter(|index| *index != -1));
            if keys.is_empty() && verb != b"blend" {
                driver.fallback_command(command, shared)?;
                return Ok(true);
            }
            self.commands.insert(stamp, CommandProgress {
                keys, cursor: 0, stage: 0, value: None, values: Vec::new(), raw: None, release: None,
            });
        }
        let mut progress = self.commands.remove(&stamp).unwrap();
        let args = &command.parameters[2..];
        let result = (|| -> Result<()> {
            if verb == b"loop" && args.is_empty() { return Ok(()); }
            if playlist::is_command(verb) {
                playlist::prepare(&mut progress, verb, args, driver, shared)?;
            }
            if matches!(verb, b"loop" | b"crossfade" | b"time") {
                let count = match verb { b"loop" => 1, b"time" => 3, _ => 2 };
                while progress.values.len() < count {
                    let slot = progress.values.len();
                    progress.values.push(number(args, slot, if verb == b"time" { -1 } else { 0 }, driver, shared)?);
                }
            }
            if matches!(verb, b"channel" | b"stype" | b"blend") && progress.value.is_none() {
                progress.value = Some(number(args, 0, if verb == b"blend" { 255 } else { 0 }, driver, shared)?);
            }
            while let Some(key) = progress.keys.get(progress.cursor) {
                let record = self.banks.get_mut(key).context("SOUND bank disappeared during command")?;
                if playlist::is_command(verb) {
                    playlist::command(host, record, &progress, verb, shared)?;
                    progress.cursor += 1;
                    continue;
                }
                match verb {
                    b"time" => {
                        // PE 2d5b50 reads +1ac, populated by the real load's
                        // command201 result. This is source duration in ms,
                        // split as minutes/seconds/remainder, not elapsed ticks.
                        let duration = record.channel_result;
                        let values = [duration / 60_000, duration % 60_000 / 1_000, duration % 1_000];
                        for (slot, value) in progress.values.iter().zip(values) {
                            if *slot != -1 {
                                if let Some(output) = shared.shared.get_mut(*slot as u32 as usize) { *output = value as i32; }
                            }
                        }
                    }
                    b"loop" => {
                        let value = progress.values[0];
                        record.play_changed |= record.play_values[0] != value;
                        record.dirty |= record.play_values[0] != value;
                        record.play_values[0] = value;
                    }
                    b"crossfade" => {
                        // PE 2d8b80 -> 2a71e0 writes the load mode/span only.
                        // The next real selection/load consumes these through
                        // the shared sound task's alternate-channel fade path.
                        record.load_values.copy_from_slice(&progress.values);
                    }
                    b"load" => {
                        if progress.stage == 0 {
                            record.file = bounded_name(args.first().map(Vec::as_slice).unwrap_or_default(), 260)?;
                            record.load_stage = Some(0);
                            progress.stage = 1;
                        }
                        if progress.stage == 1 {
                            // The original generic and SOUND-specific branches
                            // each evaluate mode, in this order, for every bank.
                            number(args, 1, 0, driver, shared)?;
                            progress.stage = 2;
                        }
                        if progress.stage == 2 {
                            record.load_values[0] = number(args, 1, 0, driver, shared)?;
                            progress.stage = 3;
                        }
                        record.load_values[1] = number(args, 2, 60, driver, shared)?;
                    }
                    b"play" => {
                        if progress.stage == 0 {
                            // A play inside PLAY changes the pending parameters;
                            // it does not restart the action already committed
                            // before entering this synchronous callback.
                            if !record.action { record.action_stage = 0; }
                            record.action = true;
                            record.requested_play = true;
                            record.dirty = true;
                            progress.stage = 1;
                        }
                        if progress.stage == 1 {
                            let value = number(args, 0, record.play_values[0], driver, shared)?;
                            record.play_changed |= record.play_values[0] != value;
                            record.play_values[0] = value;
                            progress.stage = 2;
                        }
                        if progress.stage == 2 {
                            record.play_values[1] = number(args, 1, 1, driver, shared)?;
                            // PE 2ca70c -> 2a6ee0 writes +188, exported in the
                            // media result record by 18015a/18017d. Playback's
                            // enable is the distinct base +14 (2a28a0).
                            progress.stage = 3;
                        }
                        record.play_values[2] = number(args, 2, 0, driver, shared)?;
                    }
                    b"channel" => {
                        let value = progress.value.unwrap();
                        record.dirty |= record.channel != value;
                        record.channel = value;
                    }
                    b"stype" => {
                        let value = progress.value.unwrap();
                        record.dirty |= record.stype != value;
                        record.stype = value;
                    }
                    b"blend" => {
                        let value = progress.value.unwrap();
                        if progress.stage == 0 {
                            record.dirty |= record.volume != value;
                            record.volume = value;
                            progress.stage = 1;
                        }
                        super::fes_motion::queue_sound_blend(
                            host, view, command.offset, &key.1, key.2, value,
                            sound_properties(record.volume, record.pan),
                        )?;
                    }
                    _ => unreachable!(),
                }
                progress.cursor += 1;
                progress.stage = 0;
            }
            Ok(())
        })();
        if result.is_err() { self.commands.insert(stamp, progress); }
        result.map(|_| true)
    }

    fn delete_bank(&mut self, key: &Key) {
        let Some(record) = self.banks.get_mut(key) else { return; };
        record.deleted = true;
        record.playlist = None;
        record.file.clear(); record.group.clear(); record.alias.clear();
        record.load_stage = None;
        record.requested_play = false;
        record.action = false; record.submission = None;
        if let Some(notice) = &mut record.notice { notice.state.owner_released(); }
        // A currently executing object handler still owns its real return.
        // Keep that receipt and the last committed media result until the
        // outer media pass consumes them; never load/play/scan STOP again.
        let waiting = record.notice.as_ref().and_then(|notice| notice.waiting);
        if waiting.is_none() && record.media_result.is_none() && !record.result_pending {
            self.banks.remove(key);
            if self.callbacks.get(&key.0) == Some(key) { self.callbacks.remove(&key.0); }
        }
        for progress in self.commands.values_mut() {
            if progress.keys.get(progress.cursor) == Some(key) { progress.stage = 0; }
            let consumed = progress.keys[..progress.cursor].iter().filter(|candidate| *candidate != key).count();
            progress.keys.retain(|candidate| candidate != key);
            progress.cursor = consumed;
        }
    }
}

/// The common mixed-subtree releaser passes the actual detached SOUND store.
/// Keep its retained media result until the normal media/update consumer.
pub(super) fn release_bank(
    store: &mut FesSoundStore, owner: u32, name: &[u8], bank: i32,
) -> Result<()> {
    let key = (owner, name.to_vec(), bank);
    if !store.banks.contains_key(&key) { bail!("SOUND release has no media bank owner"); }
    store.delete_bank(&key);
    Ok(())
}

/// Shared motion-tree capture/writeback. These access the same SOUND bank
/// consumed by the media pass; no duplicate volume/pan object is created.
pub(super) fn motion_properties(
    store: &FesSoundStore, owner: u32, name: &[u8], bank: i32,
) -> Option<[u32; 36]> {
    store.banks.get(&(owner, name.to_vec(), bank))
        .map(|record| sound_properties(record.volume, record.pan))
}

pub(super) fn apply_motion_properties(
    store: &mut FesSoundStore, owner: u32, name: &[u8], bank: i32, words: [u32; 36],
) -> Result<()> {
    let record = store.banks.get_mut(&(owner, name.to_vec(), bank))
        .context("SOUND motion writeback has no live bank")?;
    let (volume, pan) = super::sound_media::sound_motion_values(&words);
    record.dirty |= record.volume != volume || record.pan != pan;
    record.volume = volume;
    record.pan = pan;
    Ok(())
}

fn stop_record(record: &mut SoundBank) {
    record.requested_play = false;
    record.play_changed = false;
    // PE 1b8817 -> 1b88a6 finishes the active PLAY dispatcher before clearing
    // its action flag. A stop from that callback must retain its submission.
    if record.action && record.action_stage == 3 { return; }
    record.action = true;
    record.action_stage = 0;
    record.submission = None;
}

/// The common stop command first finishes the live motion subtree, then
/// requests SOUND's existing physical-channel stop submission (PE 2c9d78).
pub(super) fn request_stop(
    store: &mut FesSoundStore, owner: u32, name: &[u8], bank: i32,
) -> Result<()> {
    let record = store.banks.get_mut(&(owner, name.to_vec(), bank))
        .context("SOUND stop has no live bank")?;
    stop_record(record);
    Ok(())
}

pub(super) fn apply_effect(
    host: &mut LoggingHost<'_>, view: &FesTaskView, runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver, effect: &FesDriverEffect, shared: &mut ExpressionState,
) -> Result<Option<bool>> {
    let mut store = std::mem::take(&mut host.data_services.fes_sound);
    let result = (|| -> Result<Option<bool>> {
        match effect {
            FesDriverEffect::Initialize { event: FesEvent::CreateObject(definition), .. }
                if matches!(definition.kind.as_slice(), b"SOUND" | b"SNDTABLE") =>
            {
                if !store.declarations.contains(&(view.id, definition.offset)) {
                    let count = driver.adapter.object(&definition.name)
                        .context("SOUND declaration has no adapter definition")?.definition.bank_count;
                    if store.banks.len().checked_add(count).is_none_or(|n| n > MAX_BANKS) {
                        bail!("SOUND object bank limit exceeded");
                    }
                    for bank in 0..count {
                        let mut record = SoundBank::default();
                        if definition.kind == b"SNDTABLE" {
                            record.playlist = Some(playlist::Playlist::default());
                            // PE 2bf5d0 sets inherited +188 to1 even before
                            // an explicit play writes its optional argument.
                            record.play_values[1] = 1;
                        }
                        store.banks.insert((view.id, definition.name.clone(), bank as i32), record);
                    }
                    store.declarations.insert((view.id, definition.offset));
                }
                Ok(Some(true))
            }
            FesDriverEffect::Initialize { event: FesEvent::InitializeObject(init), .. } =>
                Ok(store.initialize(view.id, init, driver, shared)?.then_some(true)),
            FesDriverEffect::Command(command) =>
                Ok(store.command(host, view, runtime, command, driver, shared)?.then_some(true)),
            FesDriverEffect::ObjectEvent {
                target, event, phase: FesObjectEventPhase::Complete { .. },
            } if matches!(event.as_slice(), b"PLAY" | b"STOP" | b"NEXT" | b"REV") => {
                let key = (view.id, target.name.clone(), target.bank);
                if store.callbacks.get(&view.id) != Some(&key) { return Ok(None); }
                if let Some(record) = store.banks.get_mut(&key) {
                    if record.notice.as_ref().is_none_or(|notice| notice.notification.event() != event.as_slice()) {
                        return Ok(None);
                    }
                    record.callback_done = true;
                }
                Ok(Some(true))
            }
            _ => Ok(None),
        }
    })();
    host.data_services.fes_sound = store;
    result
}

/// Returns false while a motion budget or a synchronous PLAY block is pending.
/// The owner boundary retains its exact bank cursor until the block completes.
pub(super) fn update_owner(
    host: &mut LoggingHost<'_>, view: &FesTaskView, _runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
) -> Result<bool> {
    let mut store = std::mem::take(&mut host.data_services.fes_sound);
    let mut update = store.updates.remove(&view.id).unwrap_or_else(|| UpdateProgress {
        now: view.now, keys: store.banks.keys().filter(|(owner, _, _)| *owner == view.id).cloned().collect(),
        cursor: 0,
        stop_scan: false,
    });
    let result = (|| -> Result<bool> {
        if update.now != view.now { bail!("SOUND media continuation changed its logical time"); }
        loop {
        while let Some(key) = update.keys.get(update.cursor) {
            if store.banks.get(key).is_some_and(|record| !record.deleted)
                && driver.adapter.object_bank(&key.1, key.2 as usize).is_none()
            {
                // A parent/another real owner may have removed the binding.
                // Its media result has the same release consumer as delete.
                store.delete_bank(key);
            }
            let Some(record) = store.banks.get_mut(key) else { update.cursor += 1; continue; };
            if record.deleted {
                let ready = match record.notice.as_ref().and_then(|notice| notice.waiting) {
                    Some(NoticeWaiting::Object) => record.callback_done,
                    Some(NoticeWaiting::Alias) => driver.take_media_statement_result().is_some(),
                    None => true,
                };
                if !ready { return Ok(false); }
                // Commit the actual receipt before calling the fallible
                // physical consumer, so retry never takes the return twice.
                store.callbacks.remove(&view.id);
                record.notice = None;
                record.callback_done = false;
                if record.result_pending { commit_media_result(record); }
                release_media_result(host, record)?;
                store.banks.remove(key);
                update.cursor += 1;
                continue;
            }
            if update.stop_scan {
                if store.callbacks.get(&view.id) == Some(key) {
                    if !advance_notice(record, driver)? { return Ok(false); }
                    store.callbacks.remove(&view.id);
                    record.pending_stop_notification = false;
                } else if record.pending_stop_notification || host.sound_media_natural_stop(
                    &record.file, record.stype, record.play_values[0], record.channel as u32,
                    record.requested_play, host.data_services.language.system_settings_dirty != 0,
                )? {
                    record.requested_play = false;
                    record.action = true;
                    record.action_stage = 0;
                    record.submission = None;
                    record.pending_stop_notification = true;
                    start_notice(record, key, SoundMediaNotification::Stop)?;
                    if !advance_notice(record, driver)? {
                        store.callbacks.insert(view.id, key.clone());
                        return Ok(false);
                    }
                    record.pending_stop_notification = false;
                }
                update.cursor += 1;
                continue;
            }
            let resuming_callback = record.action && record.action_stage == 3;
            if !resuming_callback { load_record(host, record)?; }
            if record.action && record.action_stage == 0 {
                record.play_changed = false;
                if record.requested_play && record.play_values[0] == 0
                    && (host.sound_media_file_play(&record.file, false)? | record.play_values[2]) == 0
                    && playing(host, record.channel)?
                { control(host, 159, record.channel, 0)?; }
                let request = if record.requested_play {
                    record.allowed = (host.sound_media_file_play(&record.file, record.stype != 0)?
                        | record.play_values[2]) != 0;
                    SoundMediaRequest::Play {
                        count: record.play_values[0] as u32, allowed: record.allowed, enabled: record.enabled,
                    }
                } else { SoundMediaRequest::Stop };
                record.submission = Some(SoundMediaSubmission::new(record.channel as u32, request));
                record.action_stage = 1;
            }
            // PE 1b8571..1b8593 ORs the shared settings marker with this
            // object's dirty bit. Consume the same live config here even if
            // its command19 refresh has not scheduled the next media pass.
            if (record.dirty || host.data_services.language.system_settings_dirty != 0)
                && !resuming_callback {
                if !record.file.is_empty() {
                    let volume = host.sound_media_volume(
                        &record.file, &record.group, record.category_type, record.volume,
                    )?;
                    control(host, 161, record.channel, volume as u32)?;
                    control(host, 164, record.channel, record.pan as u32)?;
                }
                record.dirty = false;
            }
            if record.action {
                if record.action_stage == 3 {
                    if !advance_notice(record, driver)? { return Ok(false); }
                    store.callbacks.remove(&view.id);
                    record.submission.as_mut().context("SOUND callback lost its submission")?
                        .complete_callback()?;
                    record.action_stage = 1;
                }
                let now_millis = host.host_millis();
                let progress = record.submission.as_mut().context("SOUND action has no submission")?
                    .advance(host, now_millis, &mut record.started_tick)?;
                match progress {
                    SoundMediaProgress::Notify(notification) => {
                        start_notice(record, key, notification)?;
                        if advance_notice(record, driver)? {
                            record.submission.as_mut().unwrap().complete_callback()?;
                        } else {
                            record.submission.as_mut().unwrap().callback_started()?;
                            store.callbacks.insert(view.id, key.clone());
                            record.callback_done = false;
                            record.action_stage = 3;
                            record.result_pending = true;
                            return Ok(false);
                        }
                    }
                    SoundMediaProgress::Waiting => return Ok(false),
                    SoundMediaProgress::Complete => {}
                }
                store.callbacks.remove(&view.id);
                record.action = false;
                record.action_stage = 0;
                record.submission = None;
            }
            if record.requested_play && record.channel == -1 {
                record.requested_play = false;
                record.action = true;
                record.action_stage = 0;
            }
            if record.requested_play && record.play_changed {
                // Count changes made inside PLAY affect the current handle;
                // they must never restart a sample from its beginning.
                record.play_changed = false;
                host.sound_media_control(219, record.channel as u32, record.play_values[0] as u32)?;
            }
            commit_media_result(record);
            update.cursor += 1;
        }
        if !update.stop_scan {
            update.stop_scan = true;
            update.cursor = 0;
            continue;
        }
        break;
        }
        Ok(true)
    })();
    if !matches!(result, Ok(true)) { store.updates.insert(view.id, update); }
    host.data_services.fes_sound = store;
    result
}

fn commit_media_result(record: &mut SoundBank) {
    record.media_result = Some(MediaResult {
        channel: record.channel, release: record.play_values[1],
    });
    record.result_pending = false;
}

fn release_media_result(host: &mut LoggingHost<'_>, record: &mut SoundBank) -> Result<()> {
    if let Some(result) = record.media_result {
        // PE 180536..18055b: zero flag keeps the channel; -1 skips dispatch.
        // This is resource unload182, not playback stop185 or enable183/218.
        if result.release != 0 && result.channel != -1 {
            host.sound_media_control(182, result.channel as u32, 0)?;
        }
        record.media_result = None;
    }
    Ok(())
}

/// Call before the extension owners are destroyed. Successful channel releases
/// clear their own receipt; a failed release leaves the remainder for retry.
pub(super) fn prepare_release_owner(host: &mut LoggingHost<'_>, owner: u32) -> Result<()> {
    let mut store = std::mem::take(&mut host.data_services.fes_sound);
    let result = (|| {
        for (key, record) in &mut store.banks {
            if key.0 != owner { continue; }
            release_media_result(host, record)?;
        }
        Ok(())
    })();
    host.data_services.fes_sound = store;
    result
}

fn load_record(host: &mut LoggingHost<'_>, record: &mut SoundBank) -> Result<()> {
    let Some(mut stage) = record.load_stage else { return Ok(()); };
    if stage == 0 {
        if record.file.is_empty() {
            control(host, 156, record.channel, 0)?;
            record.load_stage = None;
            record.dirty = true;
            return Ok(());
        }
        if record.channel != -1 && record.load_values[0] == 0 {
            control(host, 156, record.channel, 0)?;
        }
        record.load_stage = Some(1);
        stage = 1;
    }
    if stage == 1 {
        if record.group.is_empty() {
            if valid_channel(record.channel) {
                host.load_sound_name_mode(
                    record.channel as u32, &record.file, 1,
                    record.load_values[0], record.load_values[1] as u32,
                )?;
            }
        } else {
            // PE 1b8340..1b837b sends 210 first, then copies its real
            // output into command206's group-key field. The receiver owns
            // configured/legacy name matching and missing-group behavior.
            let group = query_group_key(host, &record.group)?;
            let result = host.load_sound_group_name_mode(
                group, true, &record.file, 1,
                record.load_values[0], record.load_values[1] as u32,
            )?;
            let NativeReturn::Value(channel) = result else { bail!("SOUND grouped load did not return a channel"); };
            record.channel = channel as i32;
        }
        record.load_stage = Some(2);
    }
    // PE 1b8399..1b83b2 queries command201 after the load. Keep stage2
    // until the actual task returns, so failure does not repeat the load.
    record.channel_result = query_load_field(host, 201, record.channel as u32, None)?;
    record.load_stage = None;
    record.dirty = true;
    Ok(())
}

fn query_group_key(host: &mut LoggingHost<'_>, name: &[u8]) -> Result<u32> {
    let mut name = name.to_vec();
    name.push(0);
    query_load_field(host, 210, 0, Some(&mut name))
}

fn query_load_field(
    host: &mut LoggingHost<'_>, code: u32, channel: u32, data1: Option<&mut [u8]>,
) -> Result<u32> {
    let task = host.default_sound_task()?
        .context("FES SOUND load query requires a kind 13 sound task")?;
    let mut output = 0;
    if !host.dispatch_sound_task_command(
        task,
        TaskCommand {
            target: task as i32,
            sender: host.current_task_id as i32,
            code,
            data: [channel, 0, 0],
        },
        &mut TaskCommandBuffers { data0: None, data1 },
        &mut output,
    )? {
        bail!("sound task did not consume FES load query {code}");
    }
    Ok(output)
}

fn valid_channel(channel: i32) -> bool { (0..64).contains(&channel) }

fn control(host: &mut LoggingHost<'_>, index: u32, channel: i32, value: u32) -> Result<()> {
    host.sound_media_control(index + 26, channel as u32, value)?;
    Ok(())
}

fn playing(host: &mut LoggingHost<'_>, channel: i32) -> Result<bool> {
    Ok(host.sound_media_control(186, channel as u32, 0)? != 0)
}

pub(super) fn release_owner(host: &mut LoggingHost<'_>, owner: u32) {
    // The fallible prepare_release_owner consumes retained media results
    // first. A zero +54 keeps the shared physical channel after owner exit.
    let store = &mut host.data_services.fes_sound;
    store.banks.retain(|(id, _, _), _| *id != owner);
    store.declarations.retain(|(id, _)| *id != owner);
    store.initializers.retain(|(id, _), _| *id != owner);
    store.commands.retain(|(id, _), _| *id != owner);
    store.callbacks.remove(&owner);
    store.updates.remove(&owner);
}

/// Command 19 invokes this after its scene/media update, never as a fabricated
/// SOUND system block. Requests are consumed by the following media update.
pub(super) fn refresh_configuration(host: &mut LoggingHost<'_>, owner: u32) -> Result<()> {
    let mut store = std::mem::take(&mut host.data_services.fes_sound);
    let result = (|| -> Result<()> {
        for ((id, _, _), record) in &mut store.banks {
            if *id != owner || record.deleted { continue; }
            record.dirty = true;
            record.play_changed = false;
            if record.play_values[0] == 0 {
                let allowed = host.sound_media_file_play(&record.file, false)? | record.play_values[2];
                if !playing(host, record.channel)? || allowed == 0 {
                    record.action = true;
                    record.action_stage = 0;
                    record.submission = None;
                }
            }
        }
        Ok(())
    })();
    host.data_services.fes_sound = store;
    result
}

fn sound_properties(volume: i32, pan: i32) -> [u32; 36] {
    super::sound_media::sound_motion_properties(volume, pan)
}

fn bounded_name(value: &[u8], capacity: usize) -> Result<Vec<u8>> {
    let value = super::trim_nul(value);
    if value.len() >= capacity { bail!("SOUND name exceeds {} bytes", capacity - 1); }
    Ok(value.to_vec())
}

fn split_target(target: &[u8]) -> Result<(&[u8], Option<&[u8]>)> {
    let Some(open) = target.iter().position(|&byte| byte == b'[') else { return Ok((target, None)); };
    if target.last() != Some(&b']') { bail!("SOUND bank target is not closed"); }
    Ok((&target[..open], Some(&target[open + 1..target.len() - 1])))
}

fn number(
    args: &[Vec<u8>], index: usize, default: i32,
    driver: &mut FesScriptDriver, shared: &mut ExpressionState,
) -> Result<i32> {
    args.get(index).map(|source| driver.evaluate_parameter(source, default, shared))
        .unwrap_or(Ok(default))
}
