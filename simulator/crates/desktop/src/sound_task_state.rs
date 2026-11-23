//! Physical kind13 task/channel state paired with the actual desktop PCM
//! host. FES banks, driver receipts and motion stay in their existing stores.
use super::*;
use crate::{HeadlessPlayer, SoundChannelState};
use crate::audio::checkpoint::{CapturedAudioState, PreparedAudioState};
use cs2_runtime::graph_serde::{Serialize, Deserialize};
use cs2_runtime::object_graph as wire;
use anyhow::{Result as CheckpointResult, Context, bail};
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Serialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Capture<'a> {
    version: u32, tasks: &'a BTreeMap<u32, SoundTask>, ever_created: bool,
    root_task: Option<u32>,
    channels: &'a BTreeMap<u32, SoundChannelState>, groups: &'a [SoundGroup],
    audio: Vec<u8>, runtime_audio: &'a cs2_runtime::AudioState,
    legacy: &'a crate::sound_legacy::LegacySoundSettings,
}
#[derive(Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Snapshot {
    version: u32, tasks: BTreeMap<u32, SoundTask>, ever_created: bool,
    root_task: Option<u32>,
    channels: BTreeMap<u32, SoundChannelState>, groups: Vec<SoundGroup>,
    audio: Vec<u8>, runtime_audio: cs2_runtime::AudioState,
    legacy: crate::sound_legacy::LegacySoundSettings,
}

#[derive(Clone, Debug)]
pub(crate) struct PhysicalSoundIdentity {
    pub(crate) owner: Option<u32>, pub(crate) resource_id: u64,
    pub(crate) handle: Option<u64>, pub(crate) resource: Option<String>,
}
pub(crate) struct PreparedPhysicalSoundState {
    saved: Snapshot,
    audio: PreparedAudioState,
    next_resource: u64,
}
impl PreparedPhysicalSoundState {
    pub(crate) fn task_ids(&self) -> impl Iterator<Item = &u32> { self.saved.tasks.keys() }
    pub(crate) fn channel_identity(&self, channel: u32) -> Option<PhysicalSoundIdentity> {
        self.saved.channels.get(&channel).map(|s| PhysicalSoundIdentity { owner: s.task.owner,
            resource_id: s.task.resource_id, handle: s.handle, resource: s.resource.clone() })
    }
    pub(crate) fn runtime_audio(&self) -> &cs2_runtime::AudioState { &self.saved.runtime_audio }
}

fn validate(
    tasks: &BTreeMap<u32, SoundTask>, ever_created: bool, root_task: Option<u32>,
    channels: &BTreeMap<u32, SoundChannelState>,
    groups: &[SoundGroup], kinds: &BTreeMap<u32,u32>, parents: &BTreeMap<u32,u32>,
) -> CheckpointResult<u64> {
    let real: BTreeSet<_> = kinds.iter().filter_map(|(&id,&kind)| (kind == 13).then_some(id)).collect();
    if real != tasks.keys().copied().collect() || tasks.len() > 65_536 || channels.len() > 64
        || !tasks.is_empty() && !ever_created || groups.len() > 65_536 {
        bail!("physical SOUND checkpoint task/owner registry differs from saved graph");
    }
    if root_task.is_some_and(|id| id == 0 || id > i32::MAX as u32 || !ever_created
        || tasks.contains_key(&id) && (kinds.get(&id) != Some(&13) || parents.get(&id) != Some(&0))) {
        bail!("physical SOUND checkpoint root service binding differs from its actual task");
    }
    for (&id, task) in tasks {
        if parents.get(&id).is_some_and(|parent| !kinds.contains_key(parent) || *parent == id)
            || task.pending.len() > MAX_PENDING || task.loops.len() > MAX_LOOPS || task.groups.len() > 65_536
            || task.group_reserved.values().any(|&physical| physical >= 64)
            || task.loops.keys().any(|name| name.len() >= 260 || name.contains(&0)) {
            bail!("physical SOUND checkpoint has invalid task ownership/queue/loop table");
        }
        for pending in &task.pending {
            if !sound_specific(pending.packet[1]) || matches!(pending.packet[1], 198 | 214)
                || pending.input.as_ref().is_some_and(|v| v.len() > crate::sound_speech_load::MAX_RECORD.max(crate::sound_speech::MAX_RECORD).max(292)) {
                bail!("physical SOUND checkpoint has an invalid owned command packet");
            }
        }
    }
    let mut next = 1u64;
    let mut handles = BTreeSet::new();
    for (&channel, state) in channels {
        if channel >= 64 || state.task.owner.is_some_and(|id| !tasks.contains_key(&id))
            || state.task.linked.is_some_and(|id| id >= 64)
            || state.task.alternate_origin.is_some_and(|id| id >= 64)
            || state.resource.as_ref().is_some_and(|r| r.is_empty() || r.len() > 4096 || r.contains('\0'))
            || state.task.is_speech || state.handle.is_some_and(|h| h == 0 || h == u64::MAX || !handles.insert(h))
            || state.handle.is_some() && (state.resource.is_none() || state.prepared_audio.is_none())
            || state.prepared_audio.is_some() && state.resource.is_none() {
            bail!("physical SOUND checkpoint has an invalid channel/source/handle or unsupported speech source");
        }
        state.task.sidecars.validate_checkpoint()?;
        next = next.max(state.task.resource_id.checked_add(1).context("saved sound resource identity exhausted")?);
        if let Some(t) = &state.task.transition {
            if t.logical >= 64 || (t.old >= 64 && (t.old != u32::MAX || t.old_handle.is_some() || t.old_resource_id != 0))
                || t.old_handle.is_some_and(|h| h == 0 || h == u64::MAX) {
                bail!("physical SOUND checkpoint has an invalid crossfade identity");
            }
            // The old channel can already have been reloaded. Preserve its
            // captured comparison token rather than substituting its new id.
            next = next.max(t.old_resource_id.checked_add(1).context("saved crossfade resource identity exhausted")?);
        }
    }
    for group in groups.iter().chain(tasks.values().flat_map(|task| &task.groups)) {
        if group.label.len() > 4096 || group.label.contains(&0) { bail!("SOUND checkpoint group name is invalid"); }
    }
    Ok(next)
}

fn validate_captured_audio(
    host: &crate::audio::DesktopAudioHost, captured: &CapturedAudioState,
    channels: &BTreeMap<u32,SoundChannelState>, runtime: &cs2_runtime::AudioState,
) -> CheckpointResult<()> {
    let active: BTreeSet<_> = captured.handles().copied().collect();
    let mut matched = BTreeSet::new();
    for state in channels.values() {
        let Some(handle) = state.handle.filter(|id| active.contains(id)) else { continue; };
        let resource = state.resource.as_deref().context("live SOUND handle has no original source")?;
        let source = state.prepared_audio.as_ref().context("live SOUND handle has no actual decoded PCM")?;
        let signal = state.task.signal.as_ref().context("live SOUND handle has no actual signal/completion owner")?;
        if !host.checkpoint_owns_signal(handle, signal)
            || !captured.matches(handle, resource, source, signal, state.paused) {
            bail!("physical SOUND channel/worker resource, pause, cursor or signal identity differs");
        }
        matched.insert(handle);
    }
    for (playback, category) in runtime.bgm.iter().map(|p| (p, cs2_runtime::AudioChannel::Bgm))
        .chain(runtime.voice.iter().map(|p| (p, cs2_runtime::AudioChannel::Voice)))
        .chain(runtime.sound_effects.iter().map(|p| (p, cs2_runtime::AudioChannel::SoundEffect))) {
        if active.contains(&playback.handle) {
            if !captured.matches_reference(playback.handle, &playback.resource, playback.paused, category) {
                bail!("runtime AudioState playback differs from its real AudioHost owner");
            }
            matched.insert(playback.handle);
        }
    }
    if active != matched { bail!("physical SOUND checkpoint has an unowned live AudioHost playback"); }
    Ok(())
}

impl HeadlessPlayer {
    pub(crate) fn physical_sound_task_ids(&self) -> impl Iterator<Item = &u32> {
        self.data_services.sounds.tasks.keys()
    }
    pub(crate) fn physical_sound_channel_identity(&self, channel: u32) -> Option<PhysicalSoundIdentity> {
        self.sound_channels.get(&channel).map(|state| PhysicalSoundIdentity {
            owner: state.task.owner, resource_id: state.task.resource_id,
            handle: state.handle, resource: state.resource.clone(),
        })
    }

    pub(crate) fn capture_physical_sound_state(
        &self, kinds: &BTreeMap<u32,u32>, parents: &BTreeMap<u32,u32>,
    ) -> CheckpointResult<Vec<u8>> {
        if self.audio_input_deferred || self.audio_configuration_deferred
            || self.data_services.tasks.fes.has_synchronous_calls()
            || self.sound_channels.values().any(|state| state.pending_start.is_some())
            || !self.data_services.sound_async.cancelled.is_empty()
            || !self.data_services.sound_async.loads.is_empty()
            || !self.data_services.sound_async.task_loads.is_empty()
        {
            bail!("physical SOUND checkpoint is waiting for an asynchronous device completion");
        }
        self.data_services.sounds.speech.check_audio_checkpoint_boundary()?;
        self.state.audio.validate()?;
        validate(&self.data_services.sounds.tasks, self.data_services.sounds.ever_created,
            self.data_services.sounds.root_task,
            &self.sound_channels, &self.sound_groups, kinds, parents)?;
        let captured = self.audio_host.freeze_checkpoint()?;
        validate_captured_audio(&self.audio_host, &captured, &self.sound_channels, &self.state.audio)?;
        let bytes = wire::encode_component(&Capture { version: 2, tasks: &self.data_services.sounds.tasks,
            ever_created: self.data_services.sounds.ever_created, root_task: self.data_services.sounds.root_task,
            channels: &self.sound_channels, groups: &self.sound_groups,
            audio: captured.encode()?, runtime_audio: &self.state.audio, legacy: &self.data_services.legacy_sound })?;
        if bytes.len() > wire::MAX_GRAPH_BYTES { bail!("physical SOUND checkpoint exceeds the portable graph byte bound"); }
        Ok(bytes)
    }

    pub(crate) fn prepare_physical_sound_state(
        &self, bytes: &[u8], kinds: &BTreeMap<u32,u32>, parents: &BTreeMap<u32,u32>,
    ) -> CheckpointResult<PreparedPhysicalSoundState> {
        self.data_services.sounds.speech.check_audio_checkpoint_boundary()?;
        if bytes.len() > wire::MAX_GRAPH_BYTES { bail!("physical SOUND checkpoint exceeds graph byte bound"); }
        let mut saved: Snapshot = wire::decode_component(bytes)?;
        if saved.version != 2 { bail!("unsupported physical SOUND component version (root service binding required)"); }
        saved.runtime_audio.validate()?;
        let next_resource = validate(&saved.tasks, saved.ever_created, saved.root_task,
            &saved.channels, &saved.groups, kinds, parents)?;
        for state in saved.channels.values_mut() {
            if let Some(source) = &state.prepared_audio {
                let name = state.resource.as_deref().context("saved SOUND PCM has no resource")?;
                let candidate = self.scan.resources.resolve_mode_utf16(&name.encode_utf16().collect::<Vec<_>>(),
                    cs2_runtime::resource_router::ResourceMode::LooseFirst)?
                    .context("saved SOUND original resource disappeared")?;
                let raw = self.scan.resources.read_candidate(candidate.value)?;
                let actual = crate::audio_decode::prepare_audio(raw)?;
                if !source.matches_pcm(&actual)? { bail!("saved SOUND channel PCM differs from the actual resource"); }
                state.prepared_audio = Some(actual);
            }
        }
        // Completed handles and old crossfade comparison tokens also reserve
        // their original numbers. A later allocation must not turn a stale
        // reference into an unrelated new playback after restore.
        let retained_handles = saved.channels.values().flat_map(|state|
            state.handle.into_iter().chain(state.task.transition.as_ref().and_then(|t| t.old_handle)))
            .chain(saved.runtime_audio.handles()).collect();
        let audio = self.audio_host.prepare_checkpoint(&saved.audio, &retained_handles, |handle, resource|
            saved.channels.values().find(|state| state.handle == Some(handle)
                && state.resource.as_deref() == Some(resource)).and_then(|state| state.prepared_audio.clone()))?;
        let active: BTreeSet<_> = audio.handles().copied().collect();
        let mut matched = BTreeSet::new();
        for state in saved.channels.values_mut() {
            if let Some(handle) = state.handle.filter(|id| active.contains(id)) {
                let signal = state.task.signal.as_ref().context("saved active SOUND lost its signal owner")?;
                if !audio.matches(handle, state.resource.as_deref().context("saved active SOUND lost its resource")?,
                    state.prepared_audio.as_ref().context("saved active SOUND lost its source PCM")?, signal, state.paused)? {
                    bail!("saved physical SOUND channel/worker source or signal differs");
                }
                state.task.signal = audio.signal(handle);
                matched.insert(handle);
            }
        }
        for (playback, category) in saved.runtime_audio.bgm.iter().map(|p| (p, cs2_runtime::AudioChannel::Bgm))
            .chain(saved.runtime_audio.voice.iter().map(|p| (p, cs2_runtime::AudioChannel::Voice)))
            .chain(saved.runtime_audio.sound_effects.iter().map(|p| (p, cs2_runtime::AudioChannel::SoundEffect))) {
            if active.contains(&playback.handle) {
                if !audio.matches_reference(playback.handle, &playback.resource, playback.paused, category) { bail!("saved runtime AudioState has a different real source/pause/category"); }
                matched.insert(playback.handle);
            }
        }
        if active != matched { bail!("saved physical SOUND has an unowned active worker"); }
        Ok(PreparedPhysicalSoundState { saved, audio, next_resource })
    }

    /// Called only after C has prepared the SAME task graph, FES stores,
    /// receipts and motion. Original numeric handles and resource ids survive.
    pub(crate) fn commit_physical_sound_state(&mut self, prepared: PreparedPhysicalSoundState) {
        let PreparedPhysicalSoundState { saved, audio, next_resource } = prepared;
        self.data_services.sounds.tasks = saved.tasks;
        self.data_services.sounds.ever_created = saved.ever_created;
        self.data_services.sounds.root_task = saved.root_task;
        self.sound_channels = saved.channels;
        self.sound_groups = saved.groups;
        self.data_services.legacy_sound = saved.legacy;
        self.state.audio = saved.runtime_audio;
        crate::sound_native::reserve_checkpoint_resource_identity(next_resource);
        self.audio_host.commit_checkpoint(audio);
    }
}

pub(crate) mod arc_bytes {
    use super::*;
    pub(crate) fn serialize<S: cs2_runtime::graph_serde::Serializer>(v: &Option<Arc<[u8]>>, s: S) -> std::result::Result<S::Ok,S::Error> {
        v.as_ref().map(|v| v.as_ref()).serialize(s)
    }
    pub(crate) fn deserialize<'de,D: cs2_runtime::graph_serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<Arc<[u8]>>,D::Error> {
        Ok(Option::<Vec<u8>>::deserialize(d)?.map(Arc::from))
    }
}
pub(crate) mod arc_u16 {
    use super::*;
    pub(crate) fn serialize<S: cs2_runtime::graph_serde::Serializer>(v: &Arc<[u16]>, s: S) -> std::result::Result<S::Ok,S::Error> { v.as_ref().serialize(s) }
    pub(crate) fn deserialize<'de,D: cs2_runtime::graph_serde::Deserializer<'de>>(d: D) -> std::result::Result<Arc<[u16]>,D::Error> { Ok(Vec::<u16>::deserialize(d)?.into()) }
}
