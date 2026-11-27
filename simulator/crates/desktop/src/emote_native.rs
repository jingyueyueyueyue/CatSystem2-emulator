//! NEKOPARA ordinary-table E-mote ABI planning and CPU pixel submission.
//!
//! DataServices owns this service. Task creation/update/release use additive
//! hooks, while exact-profile startup and strict scene drawing live in lib.rs.
//! Timed M2 chains, audio-device origin and target0 routing remain partial.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, OnceLock},
};

use cs2_renderer::{BlendMode, Canvas, Rect};
use cs2_runtime::ResourceIndex;
use cs2_runtime::emote::{
    CpuEmoteBackend, CpuEmoteResources, CpuM2ActionConfig, CpuM2Config, CpuM2ControllerConfig,
    CpuM2DifferenceTransform, CpuM2LipBinding, CpuM2LipSidecarKind, CpuM2Load, CpuM2LoadSpec,
    CpuM2ModelSource, EmoteArchiveVersion,
    CpuM2ProfileSelection, CpuM2ResetBankSeeds, CpuM2SplineConfig, CpuViewport, EmoteBatchValue, EmoteBuffer,
    EmoteCommand, EmoteDispatchShape, EmoteError, EmoteNativeDescriptor, EmotePacking,
    EmoteRasterFrame, EmoteReply, EmoteResource, EmoteServices, EmoteTick,
    emote_native_descriptor,
};
use cs2_runtime::resource_router::ResourceMode;
#[path = "emote_progress.rs"]
mod progress;
use cs2_script::{NativeReturn, Result, ScriptError, Vm};

use super::{LoggingHost, expand_kcs_format};
use super::perf_detail::SlowTask;

const MAX_FORMAT_BYTES: usize = 4095;
const MAX_BATCH_ENTRIES: usize = 1_000_000;
const EMOTE_BLEND_PARALLEL_PIXELS: usize = 128 * 1024;

#[path = "emote_geometry.rs"]
mod geometry;
#[path = "emote_settings.rs"]
pub(super) mod settings;
#[path = "emote_load_async.rs"]
mod load_async;

/// C's owned-buffer task receiver. Command IDs are kept separate from ordinary
///slot IDs; no temporary VM/process address is retained by the M2 owner.
pub(super) fn receive(
    host: &mut LoggingHost<'_>,
    addressed: cs2_runtime::TaskCommand,
    buffers: &mut super::native_tasks::OwnedTaskBuffers,
    output: &mut u32,
    _budget: usize,
) -> Result<Option<cs2_runtime::TaskCommandOutcome>> {
    if !matches!(addressed.code, 49 | 51 | 55 | 61 | 66 | 81 | 524..=553) {
        return Ok(None);
    }
    let target = addressed.target as u32;
    if host
        .scene_nodes
        .get(&target)
        .is_none_or(|node| node.kind != 54)
    {
        return Ok(None);
    }
    if matches!(addressed.code, 49 | 61) {
        return host.receive_emote_base(addressed, buffers).map(Some);
    }
    if addressed.code == 81 {
        let placement = host.data_services.emote.placements.get_mut(&target)
            .ok_or_else(|| super::native_state_error("M2 blend command has no owned placement state"))?;
        placement.blend = BlendMode::from_raw(addressed.data[1]);
        return Ok(Some(cs2_runtime::TaskCommandOutcome { done: true, propagate: false }));
    }
    if let Some(outcome) = host.receive_emote_geometry(addressed, buffers.data1())? {
        return Ok(Some(outcome));
    }
    if addressed.code == 553 {
        *output = u32::from(host.data_services.emote.state.m2_action_busy(target).map_err(error)?);
        return Ok(Some(cs2_runtime::TaskCommandOutcome { done: true, propagate: false }));
    }
    if addressed.code == 524 {
        let spec = buffers
            .data1()
            .ok_or_else(|| unsupported(906, "M2 command524 requires owned load-spec bytes"))?;
        let result = host.data_services
            .emote
            .load_m2(host.resources, &mut host.data_services.ordinary, host.current_task_id,
                target, spec, addressed.data[0], host.data_services.fast_forward)?;
        if result == NativeReturn::Retry {
            return Ok(Some(cs2_runtime::TaskCommandOutcome { done: false, propagate: false }));
        }
        if let Some(node) = host.scene_nodes.get_mut(&target) {
            node.initialized = true;
        }
    } else if addressed.code == 540 {
        host.emote_link_legacy_sound_lip(target, addressed.data[1], addressed.data[0])?;
    } else {
        if addressed.code == 527 { host.data_services.emote.loads.cancel_target(target); }
        let native = match addressed.code {
            526 => 907,
            527 => 908,
            528 => 909,
            529 => 910,
            530 => if buffers.data0().is_some() { 912 } else { 911 },
            531 => 966,
            532 => if buffers.data0().is_some() { 915 } else { 914 },
            533 => 917,
            534 => 926,
            535 => 918,
            536 => 919,
            538 => 920,
            539 => 921,
            541 => 927,
            542 => 928,
            543 => 929,
            544 => 930,
            545 => 931,
            547 => 965,
            548 | 549 => 1025,
            550 => 1048,
            551 => 1027,
            552 => 1047,
            _ => {
                return Err(unsupported(
                    0,
                    "M2 owned command receiver has no recovered semantic operation for this code",
                ));
            }
        };
        let mut packet = command(native, addressed.code);
        packet.data0 = addressed.data[0];
        packet.data1 = addressed.data[1];
        if addressed.code == 538 {
            packet.buffer0 = EmoteBuffer::Bytes(buffers.data0()
                .ok_or_else(|| unsupported(native, "M2 material hold requires an owned name"))?.to_vec());
        }
        if addressed.code == 531 {
            //The wrapper emits a name but the original receiver ignores it.
            packet.buffer0 = EmoteBuffer::Bytes(buffers.data0().unwrap_or_default().to_vec());
        }
        if matches!(addressed.code, 530 | 532) {
            let bytes = buffers.data1().filter(|bytes| bytes.len() >= 4)
                .ok_or_else(|| unsupported(native, "M2 scalar requires four owned bytes"))?;
            packet.buffer1 = EmoteBuffer::FloatBits(u32::from_le_bytes(bytes[..4].try_into().unwrap()));
            if let Some(name) = buffers.data0() {
                packet.buffer0 = EmoteBuffer::Bytes(name.to_vec());
            }
        }
        if matches!(addressed.code, 533 | 543 | 551) {
            packet.buffer1 = EmoteBuffer::Bytes(buffers.data1()
                .ok_or_else(|| unsupported(native, "M2 selection/label requires owned bytes"))?.to_vec());
        }
        if addressed.code == 547 {
            let bytes = buffers.data1().filter(|bytes| bytes.len() >= 80)
                .ok_or_else(|| unsupported(native, "M2 program requires eighty owned bytes"))?;
            packet.buffer1 = EmoteBuffer::Bytes(bytes[..80].to_vec());
        }
        if matches!(addressed.code, 529 | 536) {
            let bytes = buffers
                .data1()
                .filter(|b| b.len() >= 12)
                .ok_or_else(|| unsupported(native, "M2 three-word command needs twelve owned bytes"))?;
            packet.buffer1 = EmoteBuffer::ThreeWords([
                u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
                u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
                u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            ]);
        }
        if addressed.code == 534 {
            buffers.write_data1(&[0])?;
        }
        let reply = host.data_services.emote.state.dispatch(target, &packet).map_err(error)?;
        if addressed.code == 534 {
            let EmoteReply::ByteString(mut bytes) = reply else {
                return Err(error(EmoteError::InvalidReply { native }));
            };
            if bytes.contains(&0) {
                return Err(error(EmoteError::InvalidReply { native }));
            }
            bytes.push(0);
            buffers.write_data1(&bytes)?;
            *output = 0;
            return Ok(Some(cs2_runtime::TaskCommandOutcome { done: true, propagate: false }));
        }
        if addressed.code == 529 {
            let EmoteReply::ThreeWords(values) = reply else {
                return Err(error(EmoteError::InvalidReply { native }));
            };
            let mut bytes = [0; 12];
            for (word, value) in bytes.chunks_exact_mut(4).zip(values) {
                word.copy_from_slice(&value.to_le_bytes());
            }
            buffers.write_data1(&bytes)?;
            *output = 0;
            return Ok(Some(cs2_runtime::TaskCommandOutcome { done: true, propagate: false }));
        }
        if matches!(addressed.code, 528 | 551) {
            let EmoteReply::Value(value) = reply else {
                return Err(error(EmoteError::InvalidReply { native }));
            };
            *output = value;
            return Ok(Some(cs2_runtime::TaskCommandOutcome { done: true, propagate: false }));
        }
        expect_continue(native, reply)?;
    }
    //Original M2Plane command cases return0 after consuming the operation.
    *output = 0;
    Ok(Some(cs2_runtime::TaskCommandOutcome {
        done: true,
        propagate: false,
    }))
}

/// Actual profile/resource selection, ready for the parent-owned M2 manager.
/// Returning this plan does not execute or count the native906 load operation:
/// selected timeline,pattern and controller actions still have to be applied.
pub(super) struct EmoteM2ResourcePlan {
    pub(super) load_spec: CpuM2LoadSpec,
    pub(super) selection: CpuM2ProfileSelection,
    pub(super) difference_transforms: Vec<(Vec<u8>, CpuM2DifferenceTransform)>,
    pub(super) config_files: BTreeMap<Vec<u8>, CpuM2Config>,
    pub(super) resource_name: Vec<u8>,
    resource_category: Vec<u8>,
    pub(super) controller_config: CpuM2ControllerConfig,
    pub(super) secondary_config: CpuM2ControllerConfig,
    pub(super) reset_bank_seeds: CpuM2ResetBankSeeds,
}

pub(super) fn load_m2_resource_plan(
    resources: &ResourceIndex,
    startup_config_category: &[u8],
    startup_config_name: &[u8],
    formatted_spec: &[u8],
) -> std::result::Result<EmoteM2ResourcePlan, EmoteError> {
    load_m2_resource_plan_impl(
        resources,
        startup_config_category,
        startup_config_name,
        formatted_spec,
        None,
        None,
    )
}

fn load_m2_resource_plan_impl(
    resources: &ResourceIndex,
    startup_config_category: &[u8],
    startup_config_name: &[u8],
    formatted_spec: &[u8],
    task: Option<u32>,
    mut identities: Option<&mut Vec<(cs2_runtime::ResourceCandidate, EmoteArchiveVersion)>>,
) -> std::result::Result<EmoteM2ResourcePlan, EmoteError> {
    fn read(
        resources: &ResourceIndex,
        category: &[u8],
        name: &[u8],
        identities: Option<&mut Vec<(cs2_runtime::ResourceCandidate, EmoteArchiveVersion)>>,
        remaining: &mut usize,
    ) -> std::result::Result<Vec<u8>, EmoteError> {
        if name.is_empty() || name.len() > MAX_FORMAT_BYTES || name.contains(&0) {
            return Err(EmoteError::InvalidResource("M2 resource name"));
        }
        let (decoded, _, errors) = encoding_rs::SHIFT_JIS.decode(name);
        if errors {
            return Err(EmoteError::InvalidResource(
                "invalid CP932 M2 resource name",
            ));
        }
        for name in resources.category_names(category, &decoded).map_err(|e| EmoteError::Backend(e.to_string()))? {
            let Some(candidate) = resources.resolve_mode_candidate(&name, ResourceMode::LooseFirst)
                .map_err(|e| EmoteError::Backend(e.to_string()))? else { continue; };
            let version = EmoteArchiveVersion::observe(candidate).ok_or(EmoteError::InvalidResource("M2 configuration version unavailable"))?;
            let bytes = resources.read_candidate_bounded(candidate, (*remaining).min(4 * 1024 * 1024))
                .map_err(|e| EmoteError::Backend(e.to_string()))?.ok_or(EmoteError::Capacity)?;
            *remaining = remaining.saturating_sub(bytes.len());
            if EmoteArchiveVersion::observe(candidate).as_ref() != Some(&version) {
                return Err(EmoteError::InvalidResource("M2 configuration changed during preparation"));
            }
            if let Some(identities) = identities { identities.push((candidate.clone(), version)); }
            return Ok(bytes);
        }
        Err(EmoteError::InvalidResource("M2 configuration not found"))
    }
    fn single(config: &CpuM2Config, section: &[u8]) -> std::result::Result<Vec<u8>, EmoteError> {
        let rows = config.records(section);
        if rows.len() != 1 || rows[0].len() != 1 {
            return Err(EmoteError::InvalidResource("M2 required singleton section"));
        }
        Ok(rows[0][0].clone())
    }
    let config_timer = task.map(|id| SlowTask::new(id, || 54, "emote_m2_config"));
    let mut remaining = 16 * 1024 * 1024;
    let view = CpuM2Config::parse_cp932(&read(
        resources,
        startup_config_category,
        startup_config_name,
        identities.as_deref_mut(),
        &mut remaining,
    )?)?;
    let profile_category = single(&view, b"profilepath")?;
    let data_category = single(&view, b"datapath")?;
    let archive_category = if data_category.eq_ignore_ascii_case(b"psz")
        && resources.optional_category_names(b"m2psb", "probe")
            .map_err(|cause| EmoteError::Backend(cause.to_string()))?.is_some()
    {
        b"m2psb".as_slice()
    } else {
        &data_category
    };
    let extension = single(&view, b"dataext")?;
    let load_spec = CpuM2LoadSpec::parse_unquoted(formatted_spec)?;
    let filename = view.profile_file(&load_spec.profile_alias_cp932)?.to_vec();
    let profile = CpuM2Config::parse_cp932(&read(resources, &profile_category, &filename, identities.as_deref_mut(), &mut remaining)?)?;
    let selection = profile.select_profile(load_spec.chara_id, load_spec.motion_id)?;
    let mut configs = BTreeMap::new();
    configs.insert(filename, profile);
    let mut queue = VecDeque::from(selection.difference_files_cp932.clone());
    let mut differences = Vec::new();
    while let Some(name) = queue.pop_front() {
        if configs.contains_key(&name) {
            continue;
        }
        if configs.len() >= 128 {
            return Err(EmoteError::Capacity);
        }
        let config = CpuM2Config::parse_cp932(&read(resources, &profile_category, &name, identities.as_deref_mut(), &mut remaining)?)?;
        let transform = config.difference_transform()?;
        for include in &transform.include_files_cp932 {
            if !configs.contains_key(include) {
                queue.push_back(include.clone());
            }
        }
        differences.push((name.clone(), transform));
        configs.insert(name, config);
    }
    fn expand_primary<'a>(
        name: &[u8],
        files: &'a BTreeMap<Vec<u8>, CpuM2Config>,
        stack: &mut Vec<Vec<u8>>,
        output: &mut Vec<&'a CpuM2Config>,
    ) -> std::result::Result<(), EmoteError> {
        if stack.len() >= 64 || stack.iter().any(|n| n == name) {
            return Err(EmoteError::InvalidResource("cyclic M2 difference include"));
        }
        if output.len() >= 128 {
            return Err(EmoteError::Capacity);
        }
        let config = files.get(name).ok_or(EmoteError::InvalidResource(
            "M2 include configuration is absent",
        ))?;
        stack.push(name.to_vec());
        for row in config.records(b"include") {
            if row.len() != 1 {
                return Err(EmoteError::InvalidResource("M2 include singleton"));
            }
            expand_primary(&row[0], files, stack, output)?;
        }
        stack.pop();
        output.push(config);
        Ok(())
    }
    let mut expanded = Vec::new();
    if let Some(primary) = selection.difference_files_cp932.first() {
        expand_primary(primary, &configs, &mut Vec::new(), &mut expanded)?;
    }
    let controller_config = CpuM2ControllerConfig::from_expanded_registered_defaults(expanded)?;
    let mut secondary = Vec::new();
    if let Some(name) = selection.difference_files_cp932.get(1) {
        expand_primary(name, &configs, &mut Vec::new(), &mut secondary)?;
    }
    let secondary_config = CpuM2ControllerConfig::from_expanded_registered_defaults(secondary)?;
    let reset_bank_seeds = controller_config.reset_bank_seeds()?;
    let mut resource_name = selection.resource_stem_cp932.clone();
    resource_name.push(b'.');
    resource_name.extend_from_slice(&extension);
    drop(config_timer);
    Ok(EmoteM2ResourcePlan {
        load_spec,
        selection,
        difference_transforms: differences,
        config_files: configs,
        resource_name,
        resource_category: archive_category.to_vec(),
        controller_config,
        secondary_config,
        reset_bank_seeds,
    })
}

/// Actual archive/loose-resource loading through the user's indexed files.
/// Category comes from resolved startup configuration, never from a guessed
/// native number. Complex colon/comma specs require the original manager parser.
pub(super) struct EmoteArchiveResources {
    pub(super) resources: Arc<ResourceIndex>,
    pub(super) category: Vec<u8>,
}

impl CpuEmoteResources for EmoteArchiveResources {
    fn read(
        &mut self,
        load_spec: &[u8],
        mode: u32,
    ) -> std::result::Result<(Vec<u8>, EmoteResource), EmoteError> {
        if mode != 0 {
            return Err(EmoteError::Unsupported {
                native: 906,
                reason: "nonzero native load mode has not been mapped",
            });
        }
        let bytes = load_spec.split(|&byte| byte == 0).next().unwrap_or(&[]);
        if bytes.is_empty() || bytes.len() > MAX_FORMAT_BYTES {
            return Err(EmoteError::InvalidResource("empty/oversized load name"));
        }
        if bytes.iter().any(|&b| matches!(b, b':' | b',')) {
            return Err(EmoteError::Unsupported {
                native: 906,
                reason: "compound load spec requires model/variant parser",
            });
        }
        let (name, _, had_errors) = encoding_rs::SHIFT_JIS.decode(bytes);
        if had_errors {
            return Err(EmoteError::InvalidResource("invalid CP932 load name"));
        }
        let packed = self
            .resources
            .read_category(&self.category, &name)
            .map_err(|error| EmoteError::Backend(error.to_string()))?;
        Ok((bytes.to_vec(), EmoteResource::decode(&packed)?))
    }
}

#[derive(Default)]
pub(super) struct EmoteNativeServices {
    pub(super) state: EmoteServices,
    model_cache_budget: usize,
    target_size: Option<[u32; 2]>,
    sound_links: BTreeMap<u32, EmoteSoundLink>,
    placements: BTreeMap<u32, geometry::Placement>,
    surface_fade_default: Option<u32>,
    material_settings: settings::M2Settings,
    spline_config: Arc<CpuM2SplineConfig>,
    action_config: Arc<CpuM2ActionConfig>,
    prepared_frames: BTreeMap<u32, EmoteRasterFrame>,
    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    pose_jobs: Option<crate::gpu_raster::PoseScheduler>,
    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    sampled_scene_tick: Option<u64>,
    loads: load_async::Loads,
    progress: progress::Progress,
}

#[derive(Debug, Clone, Copy)]
struct EmoteSoundLink {
    channel: u32,
    waveform: bool,
    monitor_enabled: bool,
}

impl LoggingHost<'_> {
    /// Typed sound-owner adapter. NEKO220/221 return owned LPB/LV data pairs;
    ///Yukikoi uses different command numbers, so never forward raw messages.
    ///The caller supplies the actual preset origin. Legacy LV uses the
    ///verified constructor0; neither host_millis nor started_tick is used.
    pub(super) fn emote_bind_sound_lip_sidecar_hook(
        &mut self,
        task: u32,
        logical_sound_channel: u32,
        actual_origin_tick: u32,
        bindings: &[CpuM2LipBinding],
    ) -> Result<usize> {
        let (bytes, kind) = if let Some(bytes) = self.sound_lip_bytes(logical_sound_channel, false)
        {
            (bytes, CpuM2LipSidecarKind::Lpd)
        } else if let Some(bytes) = self.sound_lip_bytes(logical_sound_channel, true) {
            (bytes, CpuM2LipSidecarKind::LegacyLv)
        } else {
            return Err(unsupported(
                924,
                "actual sound source has no owned lip sidecar",
            ));
        };
        self.data_services
            .emote
            .state
            .bind_m2_lip_sidecar(task, &bytes, kind, actual_origin_tick, bindings)
            .map_err(error)
    }

    ///540 sidecar branch238ecb..239026. The actual NEKO .lv path uses
    ///175460/1a1df0; its unused third argument is not an audio clock origin.
    fn emote_link_legacy_sound_lip(
        &mut self,
        task: u32,
        enabled: u32,
        logical_sound_channel: u32,
    ) -> Result<()> {
        if self.sound_lip_bytes(logical_sound_channel, false).is_none()
            && self.sound_lip_bytes(logical_sound_channel, true).is_none()
        {
            //174610 updates threshold/phase flags without releasing b4;
            //184150 continues to use that retained sidecar virtual sampler.
            //The actual new SOUND clock and signal window are still bound.
            let selected = self.sound_selected_channel(logical_sound_channel);
            if let Some(state) = self.sound_channels.get_mut(&selected) {
                state.task.signal_enabled = enabled != 0;
                if let Some(signal) = &state.task.signal {
                    signal.enable(enabled != 0);
                } else if state.handle.is_some() && state.task.sidecars.signal.is_none() {
                    return Err(unsupported(
                        924,
                        "active audio host has no actual PCM signal window",
                    ));
                }
            }
            if enabled == 0 {
                self.data_services
                    .emote
                    .state
                    .control_m2_waveform(task, false)
                    .map_err(error)?;
            }
            self.data_services.emote.sound_links.insert(
                task,
                EmoteSoundLink {
                    channel: logical_sound_channel,
                    waveform: true,
                    monitor_enabled: enabled != 0,
                },
            );
            return Ok(());
        }
        if enabled != 0 {
            let (bytes, kind) =
                if let Some(bytes) = self.sound_lip_bytes(logical_sound_channel, false) {
                    (bytes, CpuM2LipSidecarKind::Lpd)
                } else if let Some(bytes) = self.sound_lip_bytes(logical_sound_channel, true) {
                    (bytes, CpuM2LipSidecarKind::LegacyLv)
                } else {
                    return Err(unsupported(
                        924,
                        "owned sound sidecar changed during binding",
                    ));
                };
            //175270 chooses the profile's real clipper record per
            //dimension/pattern.175460 LV retains constructor parameters.
            self.data_services
                .emote
                .state
                .bind_m2_profile_lip_sidecar(task, &bytes, kind)
                .map_err(error)?;
        }
        //Original238fee applies literal1 after import; disable239002 uses0.
        let mut packet = command(928, 542);
        packet.data1 = u32::from(enabled != 0);
        expect_continue(
            928,
            self.data_services
                .emote
                .state
                .dispatch(task, &packet)
                .map_err(error)?,
        )?;
        if enabled != 0 {
            self.data_services.emote.sound_links.insert(
                task,
                EmoteSoundLink {
                    channel: logical_sound_channel,
                    waveform: false,
                    monitor_enabled: false,
                },
            );
        } else {
            self.data_services.emote.sound_links.remove(&task);
        }
        Ok(())
    }

    /// Task owner calls before publishing type54. Ordinary creation remains in
    /// native_tasks.rs; this hook has no effect on any other task kind.
    pub(super) fn emote_register_task_hook(&mut self, id: u32, kind: u32) -> Result<()> {
        if kind == 54 {
            let duration = self.data_services.emote.surface_fade_default
                .ok_or_else(|| unsupported(931, "M2 startup surface owner is not initialized"))?;
            self.data_services
                .emote
                .state
                .register_task(id, kind)
                .map_err(error)?;
            self.data_services.emote.placements.insert(id, geometry::Placement::default());
            self.data_services.emote.state.set_surface_fade_default(id, duration).map_err(error)?;
        }
        Ok(())
    }

    /// Call at the original task body update after TaskClock::update. An
    /// unloaded M2 task has no driver player to tick in the original path.
    pub(super) fn emote_update_task_hook(&mut self, id: u32, raw_task_elapsed: u32) -> Result<()> {
        let Some(state) = self.data_services.emote.state.state(id) else {
            return Ok(());
        };
        if state.successful_resource_imports == 0 {
            return Ok(());
        }
        if self.data_services.fast_forward && self.data_services.frame_capture.enabled == 0
            && !self.data_services.tasks.render.has_composition()
            && self.data_services.emote.state.hold_idle_scene_tick(id, raw_task_elapsed)
        {
            return Ok(());
        }
        if self.data_services.emote.state.surface_holds_tick(id, raw_task_elapsed).map_err(error)? {
            let random = &mut *self.script_random;
            return self.data_services.emote.state.advance_with_random(id,
                EmoteTick { task_elapsed: raw_task_elapsed, lip_elapsed: None, increment: 0 },
                &mut || random.next_integer()).map_err(error);
        }
        let link = self.data_services.emote.sound_links.get(&id).copied();
        let lip_elapsed = link.map(|link| self.sound_elapsed_ticks(link.channel));
        if let Some(link) = link.filter(|link| link.waveform && link.monitor_enabled) {
            let physical = self.sound_selected_channel(link.channel);
            let playing = matches!(self.control_sound_channel_values(160, physical, 0, 0)?,
                NativeReturn::Value(value) if value != 0);
            if playing {
                let extent = self.sound_signal_window(link.channel, None)?;
                let capacity = self.sound_signal_capacity(link.channel, extent)?;
                let mut samples = vec![0; capacity];
                let actual_extent = self.sound_signal_window(link.channel, Some(&mut samples))?;
                self.data_services
                    .emote
                    .state
                    .update_m2_waveform(id, &samples, actual_extent)
                    .map_err(error)?;
            } else {
                self.data_services
                    .emote
                    .state
                    .control_m2_waveform(id, true)
                    .map_err(error)?;
            }
        }
        let random = &mut *self.script_random;
        self.data_services
            .emote
            .state
            .advance_with_random(
                id,
                EmoteTick {
                    task_elapsed: raw_task_elapsed,
                    lip_elapsed,
                    increment: 0,
                },
                &mut || random.next_integer(),
            )
            .map_err(error)
    }

    /// Valid after a profile mismatch as well.
    pub(super) fn emote_release_task_hook(&mut self, id: u32) -> Result<()> {
        self.data_services.emote.progress.remove(id);
        #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
        self.data_services.emote.cancel_pose_job(id);
        self.data_services.emote.cancel_pending_loads_for(id);
        self.data_services.emote.sound_links.remove(&id);
        self.data_services.emote.placements.remove(&id);
        if self.data_services.emote.state.state(id).is_some() {
            self.data_services
                .emote
                .state
                .release_task(id)
                .map_err(error)?;
        }
        Ok(())
    }

    /// Route the shared descriptor receiver before RequiresImplementation's
    /// default rejection. Profile readiness still requires exact PE identity.
    pub(super) fn emote_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let descriptor = checked_descriptor(index, args)?;
        if index == 933 {
            if !self.data_services.emote.state.status().identified_profile {
                return Err(error(EmoteError::UnknownProfile));
            }
            //24cbb0 returns status2 without touching the clipboard when the
            //manager is absent or no selector0 bank contributes any text.
            if self.data_services.emote.state.backend_ready() {
                let bytes = self.data_services.emote.state.debug_m2_text_cp932().map_err(error)?;
                if !bytes.is_empty() { self.publish_cp932_clipboard(933, &bytes)?; }
            }
            return Ok(NativeReturn::Continue);
        }
        if matches!(descriptor.shape, EmoteDispatchShape::Manager) {
            return self.data_services.emote.receive(index, args, vm, None, None);
        }
        if !self.data_services.emote.state.backend_ready() {
            return Err(error(EmoteError::UnknownProfile));
        }
        if matches!(descriptor.shape, EmoteDispatchShape::Unrecovered(_)) {
            return self
                .data_services
                .emote
                .receive(index, args, vm, None, None);
        }
        if matches!(
            descriptor.shape,
            EmoteDispatchShape::OrderedNameTable { .. }
        ) && word(args, 1)? == 0
        {
            return Ok(NativeReturn::Continue);
        }
        let target = if index == 906 { self.data_services.emote.loads.saved(self.current_task_id).map(|saved| saved.0).unwrap_or(word(args, 0)?) } else { word(args, 0)? };
        if index == 906 { self.data_services.emote.loads.check_cancelled(self.current_task_id).map_err(error)?; }
        if !self
            .data_services
            .tasks
            .message_task_state(target)
            .is_some_and(|(kind, deletion, _)| kind == 54 && deletion == 0)
        {
            // The original finish wrapper returns status 2 even when its
            // command has no live M2 receiver.
            if index == 908 {
                return Ok(NativeReturn::Continue);
            }
            return Err(unsupported(
                index,
                "addressed live type54 task is unavailable",
            ));
        }
        if index == 908 { self.data_services.emote.loads.cancel_target(target); }
        if index == 906 {
            let saved = self.data_services.emote.loads.saved(self.current_task_id);
            let (spec, mode) = if let Some((_, spec, mode)) = saved { (spec, mode) } else {
                (formatted(vm, word(args, 1)?)?, word(args, 2)?)
            };
            // LOAD_LoadObj replays the saved plane's fixed string even when
            // it is empty. For that input the original receiver returns 0
            // (RVA 0x2389ac) and the newly created plane stays unloaded.
            if spec == [0]
                && vm.instruction_pointer == 790_654
                && vm.frames().last().map(|frame| frame.return_pc) == Some(721_011)
                && vm.program_code_sha256()
                    == Some("bf9933a33675e5e44d34f7486d54aa6b53bb3f32a74c2fe0393b5a43efc6f4ad")
                && self
                    .data_services
                    .emote
                    .state
                    .state(target)
                    .is_some_and(|state| state.successful_resource_imports == 0)
            {
                if self.trace_neko_cstl() {
                    self.events.push(super::LogicEvent {
                        frame: self.frame,
                        kind: "neko-m2-restore-empty".into(),
                        detail: format!("native=906 target={target} mode={mode}"),
                    });
                }
                return Ok(NativeReturn::Continue);
            }
            let result = self
                .data_services
                .emote
                .load_m2(self.resources, &mut self.data_services.ordinary, self.current_task_id,
                    target, &spec, mode, self.data_services.fast_forward)?;
            if result != NativeReturn::Retry {
                if let Some(node) = self.scene_nodes.get_mut(&target) { node.initialized = true; }
            }
            return Ok(result);
        }
        if index == 924 {
            self.emote_link_legacy_sound_lip(target, word(args, 1)?, word(args, 2)?)?;
            return Ok(NativeReturn::Continue);
        }
        let pairs = if matches!(
            descriptor.shape,
            EmoteDispatchShape::OrderedNameTable { .. }
        ) {
            Some(self.integer_maps.get(&word(args, 1)?).ok_or_else(|| {
                unsupported(index, "nonzero name/value map is not owned by this host")
            })?)
        } else {
            None
        };
        self.data_services
            .emote
            .receive(index, args, vm, Some(target), pairs)
    }
}

struct Plan {
    command: EmoteCommand,
    outputs: Option<[u32; 3]>,
    string_output: Option<u32>,
    touch_outputs: Option<[u32; 2]>,
}

impl EmoteNativeServices {
    pub(super) fn set_model_cache_budget(&mut self, bytes: usize) { self.model_cache_budget = bytes; }
    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    pub(super) fn enable_pose_jobs(&mut self) {
        self.pose_jobs.get_or_insert_with(Default::default);
    }

    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    pub(super) fn cancel_pose_job(&mut self, id: u32) {
        if let Some(jobs) = &mut self.pose_jobs {
            jobs.cancel(id);
        }
    }

    #[cfg(all(test, feature = "gpu-raster", any(windows, target_os = "android")))]
    pub(super) fn set_pose_test_gate(&mut self, task: u32, gate: Arc<crate::gpu_raster::PoseTestGate>) {
        self.pose_jobs.as_mut().expect("enabled pose scheduler").set_gate(task, gate);
    }

    #[cfg(all(test, feature = "gpu-raster", any(windows, target_os = "android")))]
    pub(super) fn displayed_pose_tick(&self, task: u32) -> Option<u64> {
        self.pose_jobs.as_ref()?.displayed_tick(task)
    }

    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    pub(super) fn preflight_scene_pose_jobs(
        &mut self, nodes: &BTreeMap<u32, super::SceneNode>, tick: u64, exact: bool,
    ) -> std::result::Result<bool, EmoteError> {
        self.sampled_scene_tick = None;
        let Some(jobs) = &mut self.pose_jobs else { return Ok(true); };
        if !crate::gpu_raster::emote_batch_requested() || !crate::gpu_raster::pose_jobs_available() {
            jobs.retain(&[]);
            return Ok(true);
        }
        let mut tasks = Vec::new();
        for node in nodes.values().filter(|node| node.kind == 54 && node.visible && node.initialized) {
            if self.state.scene_raster_player(node.id).is_some() {
                tasks.push(node.id);
            }
        }
        if tasks.len() > 16 {
            jobs.retain(&[]);
            return Ok(true);
        }
        jobs.retain(&tasks);
        for &task in &tasks {
            let node = &nodes[&task];
            self.state.set_base_argb(task, node.color)?;
            let Some((player, viewport)) = self.state.scene_raster_player(task) else {
                jobs.cancel(task);
                continue;
            };
            let task_state = self.state.state(task).ok_or(EmoteError::UnknownTask(task))?;
            let placement = self.placements.get(&task).ok_or(EmoteError::UnknownTask(task))?;
            let context = crate::gpu_raster::ScenePoseContext {
                imports: task_state.successful_resource_imports,
                base_color: node.color,
                color_percentages: task_state.color_percentages_raw,
                blend: placement.blend,
            };
            let revision = self.progress.request(task, Some((player, viewport)));
            jobs.request(task, tick, revision, context, player, viewport, exact);
        }
        jobs.dispatch(&tasks);
        if tasks.iter().all(|&task| jobs.scene_ready(task)) {
            self.sampled_scene_tick = Some(tick);
            return Ok(true);
        }
        Ok(false)
    }

    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    pub(super) fn finish_scene_pose_jobs(&mut self) {
        self.sampled_scene_tick = None;
    }
    pub(super) fn set_material_resources(&mut self, resources: Arc<ResourceIndex>) { self.loads.set_resources(resources); }
    pub(super) fn pending_load_owners(&self) -> impl Iterator<Item = u32> + '_ { self.loads.owners() }
    pub(super) fn has_pending_loads(&self) -> bool { self.loads.has_pending() }
    pub(super) fn cancel_pending_loads_for(&mut self, id: u32) { self.loads.cancel(id); }

    pub(super) fn initialize(&mut self, resources: &ResourceIndex) -> Result<()> {
        // The M2 view configuration defines the startup resources and dimensions.
        let bytes = resources
            .read_category(b"m2config", "m2view.cnf")
            .map_err(|e| error(EmoteError::Backend(e.to_string())))?;
        let config = CpuM2Config::parse_cp932(&bytes).map_err(error)?;
        let mut spline_config = CpuM2SplineConfig::default();
        for row in config.records(b"spline") {
            if row.len() != 1 { return Err(unsupported(929, "M2 spline resource row")); }
            let (name, _, invalid) = encoding_rs::SHIFT_JIS.decode(&row[0]);
            if invalid { return Err(unsupported(929, "M2 spline resource CP932")); }
            let bytes = resources.read_category(b"m2config", &name)
                .map_err(|e| error(EmoteError::Backend(e.to_string())))?;
            spline_config.append_cp932(&bytes).map_err(error)?;
        }
        self.spline_config = Arc::new(spline_config);
        let mut action_config = CpuM2ActionConfig::default();
        for row in config.records(b"action") {
            if row.len() != 1 { return Err(unsupported(1027, "M2 action resource row")); }
            let (name, _, invalid) = encoding_rs::SHIFT_JIS.decode(&row[0]);
            if invalid { return Err(unsupported(1027, "M2 action resource CP932")); }
            let bytes = resources.read_category(b"m2config", &name)
                .map_err(|e| error(EmoteError::Backend(e.to_string())))?;
            action_config.append_cp932(&bytes).map_err(error)?;
        }
        self.action_config = Arc::new(action_config);
        self.material_settings.register(&config)
            .map_err(|e| error(EmoteError::Backend(e.to_string())))?;
        let rows = config.records(b"wndsize");
        if rows.len() != 1 || rows[0].len() != 2 {
            return Err(unsupported(906, "M2 startup wndsize pair is absent"));
        }
        let dimension = |bytes: &[u8]| -> Result<u32> {
            std::str::from_utf8(bytes)
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .filter(|n| *n > 0 && *n <= 16384)
                .ok_or_else(|| unsupported(906, "M2 startup wndsize integer"))
        };
        let size = [dimension(&rows[0][0])?, dimension(&rows[0][1])?];
        let viewport = CpuViewport::original_neko_target(size[0], size[1]).map_err(error)?;
        let mut backend = CpuEmoteBackend::new_original_task_clock(viewport).map_err(error)?;
        backend.set_m2_retention_budget(self.model_cache_budget);
        #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
        if crate::gpu_raster::emote_requested() {
            backend.set_rasterizer(crate::gpu_raster::draw_emote);
            backend.set_surface_tinter(crate::gpu_raster::tint_emote_surface);
            backend.set_surface_gpu(
                crate::gpu_raster::mix_emote_surfaces,
                crate::gpu_raster::prepare_emote_surfaces,
                crate::gpu_raster::expand_emote_surface,
                crate::gpu_raster::draw_emote_surface,
            );
        }
        self.state.set_backend(Box::new(backend)).map_err(error)?;
        self.state.bind_m2_tasks().map_err(error)?;
        #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
        if crate::gpu_raster::emote_requested() {
            crate::gpu_raster::prewarm_emote();
        }
        self.target_size = Some(size);
        let mut fade = 30;
        for row in config.records(b"fade") {
            if let Some(value) = row.first() {
                fade = cs2_formats::config::parse_integer_prefix(value) as u32;
            }
        }
        self.surface_fade_default = Some(fade);
        Ok(())
    }

    fn load_m2(
        &mut self,
        resources: &ResourceIndex,
        ordinary: &mut super::ordinary_native::OrdinaryNativeServices,
        owner: u32,
        task: u32,
        spec: &[u8],
        mode: u32,
        fast_forward: bool,
    ) -> Result<NativeReturn> {
        self.loads.check_cancelled(owner).map_err(error)?;
        let filter = self.state.m2_texture_filter(task)
            .ok_or_else(|| unsupported(906, "M2 target has no CPU texture configuration"))?;
        let imports = self.state.state(task).map_or(0, |state| state.successful_resource_imports);
        let Some(prepared) = self.loads.poll(resources, owner, task, spec, mode, filter, imports, fast_forward, |plan, version| {
            self.state.cached_m2_model(task, &plan.resource_name, version).map(load_async::CachedModel::Current).or_else(|| {
                self.state.uses_unfiltered_m2_texture(task)
                    .then(|| ordinary.subscribe_preloaded_model(version))
                    .flatten().map(load_async::CachedModel::Preload)
            })
        }).map_err(error)? else {
            return Ok(NativeReturn::Retry);
        };
        let commit_started = std::time::Instant::now();
        let phases_us = prepared.phases_us;
        let preload_wait_us = prepared.preload_wait_us;
        let atlas_wait_us = prepared.atlas_wait_us;
        let load_async::Prepared { plan, model, version, .. } = prepared;
        let source = CpuM2ModelSource::Shared(model, version);
        let mut secondary_variables = plan.secondary_config.default_variables().map_err(error)?;
        let secondary_force_defaults = secondary_variables.clone();
        for (dimension, label) in plan
            .secondary_config
            .pattern_selection(&[])
            .map_err(error)?
            .iter()
            .enumerate()
        {
            secondary_variables.extend(
                plan.secondary_config
                    .pattern_variables(dimension as u32, label)
                    .map_err(error)?,
            );
        }
        let primary_name = plan.selection.difference_files_cp932.first();
        let transform = primary_name.and_then(|name| {
            plan.difference_transforms
                .iter()
                .find(|(candidate, _)| candidate == name)
                .map(|(_, t)| t)
        });
        let position = transform.and_then(|t| t.position).unwrap_or([0; 2]);
        let scale = transform.and_then(|t| t.scale).unwrap_or([1.0; 2]);
        let scale_offset = transform.and_then(|t| t.scale_offset).unwrap_or([0; 2]);
        let (timeline, _, errors) =
            encoding_rs::SHIFT_JIS.decode(&plan.selection.timeline_label_cp932);
        if errors {
            return Err(unsupported(906, "invalid CP932 M2 timeline label"));
        }
        let load = CpuM2Load {
            profile_alias_cp932: plan.load_spec.profile_alias_cp932.clone(),
            player_name_cp932: plan.selection.player_name_cp932.clone(),
            controller_config: plan.controller_config,
            pattern_selectors_cp932: plan.load_spec.pattern_selectors_cp932,
            secondary_variables,
            secondary_force_defaults,
            secondary_config: plan.secondary_config,
            spline_config: self.spline_config.clone(),
            action_config: self.action_config.clone(),
            material_settings: self.material_settings.current(),
            timeline_label: timeline.into_owned(),
            // Driver placement modifies each motion root before its local
            // transformOrder, not a parent matrix outside the PSB hierarchy.
            root_affine: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            transition_raw: mode,
        };
        {
            let _timing = SlowTask::new(task, || 54, "emote_m2_import").named(&plan.resource_name, None);
            self.state.import_m2_resource(task, &plan.resource_name, source, load)
                .map_err(error)?;
        }
        let placement = self.placements.entry(task).or_default();
        placement.profile_position = position.map(|value| value as f32);
        placement.profile_scale = scale[0];
        placement.scale_offset = scale_offset.map(|value| value as f32);
        self.apply_placement(task).map_err(error)?;
        super::perf_detail::resource_warmup_note(&format!("m2_async native=906 target={task} name={} config_us={} read_us={} decode_us={} model_us={} preload_hit={} preload_wait_us={} atlas_wait_us={atlas_wait_us} commit_us={}",
            String::from_utf8_lossy(&plan.resource_name), phases_us[0], phases_us[1], phases_us[2], phases_us[3],
            preload_wait_us.is_some(), preload_wait_us.unwrap_or_default(), commit_started.elapsed().as_micros()));
        Ok(NativeReturn::Continue)
    }

    /// Natural-size logical-canvas path. Other view mappings remain explicit;
    /// a type54 task is always consumed here and never becomes a solid rect.
    pub(super) fn draw_scene_task(
        &mut self,
        node: &super::SceneNode,
        canvas: &mut Canvas,
    ) -> std::result::Result<bool, EmoteError> {
        if node.kind != 54 {
            return Ok(false);
        }
        let Some(state) = self.state.state(node.id) else {
            return Err(EmoteError::UnknownTask(node.id));
        };
        if state.successful_resource_imports == 0 {
            return Ok(true);
        }
        let _timing = super::measure_host_phase(super::HostPhase::ComposeEmote);
        if self.target_size != Some([canvas.width(), canvas.height()]) {
            return Err(EmoteError::Backend(
                "type54 final display quad requires a resolved host offset/scale mapping".into(),
            ));
        }
        self.state.set_base_argb(node.id, node.color)?;
        if let Some(frame) = self.prepared_frames.remove(&node.id) {
            self.submit_frame(node.id, canvas, frame)?;
        } else {
            self.draw(node.id, canvas)?;
        }
        Ok(true)
    }

    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    pub(super) fn prepare_scene_tasks<'a>(
        &mut self, nodes: impl IntoIterator<Item = &'a super::SceneNode>, size: [u32; 2],
    ) -> std::result::Result<(), EmoteError> {
        self.prepared_frames.clear();
        let mut tasks = Vec::new();
        for node in nodes {
            let Some(state) = self.state.state(node.id) else { continue };
            if state.successful_resource_imports == 0 { continue }
            self.state.set_base_argb(node.id, node.color)?;
            if self.state.can_defer_scene_raster(node.id) {
                tasks.push(node.id);
            }
        }
        let mut sampled = BTreeMap::new();
        let sampled_tick = self.sampled_scene_tick;
        if let Some(tick) = sampled_tick
            && let Some(jobs) = &mut self.pose_jobs
        {
            for &task in &tasks {
                sampled.insert(task, jobs.take(task, tick));
            }
        }
        {
            super::gpu_raster::protect_emote_models(sampled.iter().filter_map(|(&task, sample)| {
                matches!(sample, crate::gpu_raster::ScenePose::Ready(_) | crate::gpu_raster::ScenePose::Reuse(_))
                    .then(|| self.state.scene_raster_player(task).map(|(player, _)| player.shared_motion_model()))
                    .flatten()
            }));
            let players: Vec<_> = tasks.iter().filter_map(|&task| {
                matches!(sampled.get(&task), None | Some(crate::gpu_raster::ScenePose::Exact))
                    .then(|| self.state.scene_raster_player(task)).flatten()
            }).collect();
            super::gpu_raster::prepare_emote_poses(&players);
        }
        for task in tasks {
            use crate::gpu_raster::ScenePose;
            let frame = match sampled.remove(&task) {
                None | Some(ScenePose::Exact) => self.render_frame(task, size)?,
                Some(ScenePose::Reuse(frame)) => frame,
                Some(ScenePose::Ready(pose)) => {
                    let (player, variables) = pose.sample();
                    let frame = {
                        let _installed = pose.install();
                        self.state.render_sampled_scene_rgba(task, player, variables)
                    };
                    let frame = self.prepare_blend_frame(task, size, frame?)?;
                    self.progress.rendered_revision(task, &frame, Some(pose.revision()));
                    self.pose_jobs.as_mut().expect("pose scheduler during composition")
                        .record_render(task, sampled_tick.expect("sampled scene tick"), pose, &frame);
                    frame
                }
                Some(ScenePose::Pending) => {
                    return Err(EmoteError::Backend("sampled scene posed without completed preflight".into()));
                }
            };
            self.prepared_frames.insert(task, frame);
        }
        Ok(())
    }

    /// Covers the whole36-slot complex family, including explicit diagnostics.
    /// Native925 remains in the exact-PE constant profile.
    pub(super) fn recognizes(index: u32) -> bool {
        emote_native_descriptor(index).is_some()
    }

    /// These wrappers have verified parameter/command layouts. This predicate
    /// does not assert that a PSB backend is ready: call returns that diagnostic.
    pub(super) fn supports(index: u32) -> bool {
        emote_native_descriptor(index)
            .is_some_and(|d| matches!(d.shape, EmoteDispatchShape::Single { .. } | EmoteDispatchShape::Manager))
    }

    pub(super) fn supports_batch(index: u32) -> bool {
        emote_native_descriptor(index)
            .is_some_and(|d| matches!(d.shape, EmoteDispatchShape::OrderedNameTable { .. }))
    }

    /// One receiver for the whole descriptor set. The parent supplies an
    /// actual resolved task and a borrowed table from its typed table owner.
    ///
    /// A zero script table handle skips target resolution, as in the original.
    /// A nonzero handle with no real table is an error, never an empty batch.
    /// Check descriptor.target_word() when preparing the resolved target.
    pub(super) fn receive(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
        resolved_target: Option<u32>,
        pairs: Option<&BTreeMap<Vec<u8>, u32>>,
    ) -> Result<NativeReturn> {
        let descriptor = checked_descriptor(index, args)?;
        match descriptor.shape {
            EmoteDispatchShape::Manager => {
                if index != 932 {
                    return Err(unsupported(index, "M2 bank text publication requires the host clipboard receiver"));
                }
                //24cb50 consumes an absent manager without a task lookup.
                if self.state.backend_ready() {
                    self.state.recompute_m2_controls(&self.material_settings.current()).map_err(error)?;
                }
                Ok(NativeReturn::Continue)
            }
            EmoteDispatchShape::Single { .. } => {
                let target = resolved_target
                    .ok_or_else(|| unsupported(index, "real type54 task resolution is required"))?;
                let plan = plan(descriptor, args, vm)?;
                self.submit_plan(target, plan, vm)
            }
            EmoteDispatchShape::OrderedNameTable { .. } => {
                if word(args, 1)? == 0 {
                    return Ok(NativeReturn::Continue);
                }
                let pairs = pairs.ok_or_else(|| {
                    unsupported(
                        index,
                        "nonzero table handle has no actual typed name/value table",
                    )
                })?;
                let target = resolved_target
                    .ok_or_else(|| unsupported(index, "real type54 task resolution is required"))?;
                self.submit_batch(descriptor, target, pairs)
            }
            EmoteDispatchShape::Unrecovered(reason) => Err(unsupported(index, reason)),
        }
    }

    /// Compatibility entry; new parent integration should call receive().
    pub(super) fn call(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
        resolved_target: u32,
    ) -> Result<NativeReturn> {
        self.receive(index, args, vm, Some(resolved_target), None)
    }

    fn submit_plan(
        &mut self,
        resolved_target: u32,
        plan: Plan,
        vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let index = plan.command.native;
        if let Some(address) = plan.string_output {
            //17dd00 initializes output[0] before looking up selector0.
            //Neither the old string nor its format escapes are input here.
            vm.write(address, &[0])?;
        }
        if let Some(addresses) = plan.outputs {
            // Preserve alias semantics: output writes occur X, Y, Z after the
            // result exists, with the last overlapping DWORD winning.
            for address in addresses {
                vm.read(address, 4)?;
            }
        }
        let reply = self
            .state
            .dispatch(resolved_target, &plan.command)
            .map_err(error)?;
        match (plan.outputs, plan.string_output, plan.touch_outputs, reply) {
            (Some(addresses), None, None, EmoteReply::ThreeWords(values)) => {
                for (address, value) in addresses.into_iter().zip(values) {
                    vm.write(address, &value.to_le_bytes())?;
                }
                Ok(NativeReturn::Continue)
            }
            (None, Some(address), None, EmoteReply::ByteString(mut bytes)) => {
                if bytes.contains(&0) {
                    return Err(error(EmoteError::InvalidReply { native: index }));
                }
                bytes.push(0);
                vm.write(address, &bytes)?;
                Ok(NativeReturn::Continue)
            }
            (None, None, Some(addresses), EmoteReply::DescriptorColumns(fields)) => {
                let success = fields[0].is_some();
                //17ff90 queries and copies each column independently, in
                //order. Absent fields leave their original destinations.
                for (address, field) in addresses.into_iter().zip(fields) {
                    if let Some(mut bytes) = field {
                        if bytes.contains(&0) {
                            return Err(error(EmoteError::InvalidReply { native: index }));
                        }
                        bytes.push(0);
                        vm.write(address, &bytes)?;
                    }
                }
                Ok(NativeReturn::Value(u32::from(success)))
            }
            (None, None, None, EmoteReply::Continue) => Ok(NativeReturn::Continue),
            (None, None, None, EmoteReply::Value(value)) => Ok(NativeReturn::Value(value)),
            _ => Err(error(EmoteError::InvalidReply { native: index })),
        }
    }

    /// `pairs` must come from the actual typed name/value table owner.
    /// It must never be fabricated by dereferencing the script's numeric handle.
    /// BTreeMap preserves original byte-name key order and raw DWORD values.
    pub(super) fn call_batch(
        &mut self,
        index: u32,
        resolved_target: u32,
        pairs: Option<&BTreeMap<Vec<u8>, u32>>,
    ) -> Result<NativeReturn> {
        let descriptor = emote_native_descriptor(index)
            .ok_or_else(|| unsupported(index, "native absent from E-mote descriptor table"))?;
        if !matches!(
            descriptor.shape,
            EmoteDispatchShape::OrderedNameTable { .. }
        ) {
            return Err(unsupported(index, "not a recovered batch wrapper"));
        }
        // Original zero table handle skips all target/parameter accesses.
        let Some(pairs) = pairs else {
            return Ok(NativeReturn::Continue);
        };
        self.submit_batch(descriptor, resolved_target, pairs)
    }

    fn submit_batch(
        &mut self,
        descriptor: &EmoteNativeDescriptor,
        resolved_target: u32,
        pairs: &BTreeMap<Vec<u8>, u32>,
    ) -> Result<NativeReturn> {
        let index = descriptor.index;
        let EmoteDispatchShape::OrderedNameTable {
            command: code,
            prefix_command,
            value: value_kind,
        } = descriptor.shape
        else {
            return Err(unsupported(
                index,
                "descriptor does not accept a name table",
            ));
        };
        if pairs.len() > MAX_BATCH_ENTRIES {
            return Err(error(EmoteError::Capacity));
        }
        // Validate the entire borrowed table before any reset or dispatch.
        for name in pairs.keys() {
            if name.len() > MAX_FORMAT_BYTES || name.contains(&0) {
                return Err(unsupported(index, "batch name exceeds byte-name bounds"));
            }
        }
        if let Some(prefix) = prefix_command {
            let reset = command(index, prefix);
            expect_continue(
                index,
                self.state
                    .dispatch(resolved_target, &reset)
                    .map_err(error)?,
            )?;
        }
        for (name, value) in pairs {
            let mut request = command(index, code);
            let mut terminated = name.clone();
            terminated.push(0);
            request.buffer0 = EmoteBuffer::Bytes(terminated);
            match value_kind {
                EmoteBatchValue::FloatBits => request.buffer1 = EmoteBuffer::FloatBits(*value),
                EmoteBatchValue::RawData1 => request.data1 = *value,
                EmoteBatchValue::NamesOnly => {}
            }
            expect_continue(
                index,
                self.state
                    .dispatch(resolved_target, &request)
                    .map_err(error)?,
            )?;
        }
        Ok(NativeReturn::Continue)
    }

    fn render_frame(
        &mut self, resolved_target: u32, size: [u32; 2],
    ) -> std::result::Result<EmoteRasterFrame, EmoteError> {
        if super::perf_detail::enabled() {
            self.progress.request(resolved_target, self.state.diagnostic_player(resolved_target));
        }
        let frame = {
            let _timing = SlowTask::new(resolved_target, || 54, "emote_render_rgba");
            self.state.render_rgba(resolved_target)?
        };
        let frame = self.prepare_blend_frame(resolved_target, size, frame)?;
        self.progress.rendered(resolved_target, &frame);
        Ok(frame)
    }

    fn prepare_blend_frame(
        &self, task: u32, size: [u32; 2], frame: EmoteRasterFrame,
    ) -> std::result::Result<EmoteRasterFrame, EmoteError> {
        let blend = self.placements.get(&task).ok_or(EmoteError::UnknownTask(task))?.blend;
        if blend == BlendMode::SourceOver || frame.device().is_some() {
            return Ok(frame);
        }
        // Other blend modes can depend on transparent pixels outside the pose bounds.
        frame.into_viewport(size)
    }

    fn submit_frame(
        &mut self, resolved_target: u32, canvas: &mut Canvas, frame: EmoteRasterFrame,
    ) -> std::result::Result<usize, EmoteError> {
        let blend = self.placements.get(&resolved_target).ok_or(EmoteError::UnknownTask(resolved_target))?.blend;
        let submitted = {
            let _timing = SlowTask::new(resolved_target, || 54, "emote_blend");
            match submit_rgba(canvas, &frame, blend) {
                Err(EmoteError::DeviceReadback) => {
                    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
                    self.cancel_pose_job(resolved_target);
                    return Ok(0);
                }
                result => result?,
            }
        };
        if submitted != 0 {
            self.state.record_pixel_submission(resolved_target)?;
            self.progress.scene_applied(resolved_target, &frame);
        }
        Ok(submitted)
    }

    /// Render an actual evaluated frame, then blend its device or CPU pixels
    /// into the scene canvas. Never substitute a static or empty frame.
    pub(super) fn draw(
        &mut self,
        resolved_target: u32,
        canvas: &mut Canvas,
    ) -> std::result::Result<usize, EmoteError> {
        let frame = self.render_frame(resolved_target, [canvas.width(), canvas.height()])?;
        self.submit_frame(resolved_target, canvas, frame)
    }

    pub(super) fn trace_visual_progress(&mut self) -> (usize, usize) {
        self.progress.trace(&self.state)
    }
}

fn command(native: u32, code: u32) -> EmoteCommand {
    EmoteCommand {
        native,
        command: code,
        data0: 0,
        data1: 0,
        buffer0: EmoteBuffer::None,
        buffer1: EmoteBuffer::None,
    }
}

fn checked_descriptor(index: u32, args: &[u8]) -> Result<&'static EmoteNativeDescriptor> {
    let descriptor = emote_native_descriptor(index)
        .ok_or_else(|| unsupported(index, "unrecovered ordinary E-mote wrapper"))?;
    let expected = usize::from(descriptor.argument_bytes);
    if args.len() != expected {
        return Err(super::native_state_error_owned(format!(
            "E-mote native {index}: expected {expected} argument bytes, got {}",
            args.len()
        )));
    }
    Ok(descriptor)
}

fn plan(descriptor: &EmoteNativeDescriptor, args: &[u8], vm: &Vm) -> Result<Plan> {
    let index = descriptor.index;
    let EmoteDispatchShape::Single {
        command: selector,
        packing,
    } = descriptor.shape
    else {
        return Err(unsupported(index, "descriptor requires the group receiver"));
    };
    // The parent resolves args0 through the real task tree. Retain the original
    // target in the script bytes only; no host pointer is manufactured here.
    let _raw_target = word(args, 0)?;
    let arg1 = if args.len() >= 8 { word(args, 1)? } else { 0 };
    let code = selector.select(arg1);
    let mut request = command(index, code);
    let mut outputs = None;
    let mut string_output = None;
    let mut touch_outputs = None;
    match packing {
        EmotePacking::LoadSpec => {
            request.data0 = word(args, 2)?;
            request.buffer1 = EmoteBuffer::Bytes(formatted(vm, word(args, 1)?)?);
        }
        EmotePacking::RawData1 => request.data1 = word(args, 1)?,
        EmotePacking::VectorQuery => {
            request.data0 = word(args, 1)?;
            request.buffer1 = EmoteBuffer::ThreeWords([0; 3]);
            outputs = Some([word(args, 2)?, word(args, 3)?, word(args, 4)?]);
        }
        EmotePacking::StringOutput => {
            let address = word(args, 1)?;
            vm.read(address, 1)?;
            string_output = Some(address);
        }
        EmotePacking::TouchCount => {
            request.data0 = word(args, 1)?;
            request.data1 = word(args, 2)?;
        }
        EmotePacking::TouchItem => {
            request.buffer1 = EmoteBuffer::ThreeWords([word(args, 1)?, word(args, 2)?, word(args, 3)?]);
            touch_outputs = Some([word(args, 4)?, word(args, 5)?]);
        }
        EmotePacking::FloatAll => request.buffer1 = EmoteBuffer::FloatBits(word(args, 1)?),
        EmotePacking::FloatNamed => {
            request.buffer0 = EmoteBuffer::Bytes(formatted(vm, word(args, 1)?)?);
            request.buffer1 = EmoteBuffer::FloatBits(word(args, 2)?);
        }
        EmotePacking::ThreeRawWords => {
            request.buffer1 =
                EmoteBuffer::ThreeWords([word(args, 1)?, word(args, 2)?, word(args, 3)?])
        }
        EmotePacking::NamedRaw => {
            request.buffer0 = EmoteBuffer::Bytes(formatted(vm, word(args, 1)?)?);
            request.data1 = word(args, 2)?;
        }
        EmotePacking::TwoRawWords => {
            request.data1 = word(args, 1)?;
            request.data0 = word(args, 2)?;
        }
        EmotePacking::Label => {
            request.buffer1 = EmoteBuffer::Bytes(formatted(vm, word(args, 1)?)?);
        }
        EmotePacking::DiscardedTailString => {
            let storage = vm.native_argument_storage(descriptor.argument_storage_bytes())?;
            let address = word(storage, 1)?;
            request.buffer1 = EmoteBuffer::Bytes(formatted(vm, address)?);
        }
        EmotePacking::LabelRecord80 => {
            let name = formatted(vm, word(args, 1)?)?;
            if name.len() > 72 {
                return Err(unsupported(index, "label exceeds72-byte record prefix"));
            }
            let mut record = vec![0; 80];
            record[..name.len()].copy_from_slice(&name);
            record[72..76].copy_from_slice(&word(args, 2)?.to_le_bytes());
            record[76..80].copy_from_slice(&word(args, 3)?.to_le_bytes());
            request.buffer1 = EmoteBuffer::Bytes(record);
        }
        EmotePacking::Empty => {}
    }
    Ok(Plan {
        command: request,
        outputs,
        string_output,
        touch_outputs,
    })
}

fn formatted(vm: &Vm, address: u32) -> Result<Vec<u8>> {
    let mut bytes = expand_kcs_format(vm, &vm.read_string(address)?, 0)?;
    bytes.truncate(bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len()));
    if bytes.len() > MAX_FORMAT_BYTES {
        return Err(error(EmoteError::Capacity));
    }
    bytes.push(0);
    Ok(bytes)
}

fn word(args: &[u8], index: usize) -> Result<u32> {
    let start = index
        .checked_mul(4)
        .ok_or_else(|| error(EmoteError::Capacity))?;
    let end = start
        .checked_add(4)
        .ok_or_else(|| error(EmoteError::Capacity))?;
    let bytes = args
        .get(start..end)
        .ok_or(ScriptError::Truncated { offset: start, needed: 4 })?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn expect_continue(index: u32, reply: EmoteReply) -> Result<()> {
    match reply {
        EmoteReply::Continue => Ok(()),
        _ => Err(error(EmoteError::InvalidReply { native: index })),
    }
}

fn unsupported(index: u32, reason: &'static str) -> ScriptError {
    error(EmoteError::Unsupported {
        native: index,
        reason,
    })
}

fn error(error: EmoteError) -> ScriptError {
    super::native_state_error_owned(error.to_string())
}

fn emote_blend_workers() -> usize {
    static WORKERS: OnceLock<usize> = OnceLock::new();
    *WORKERS.get_or_init(|| {
        std::thread::available_parallelism().map_or(1, |count| count.get()).min(4)
    })
}

pub(super) fn submit_rgba(
    canvas: &mut Canvas,
    frame: &EmoteRasterFrame,
    blend: BlendMode,
) -> std::result::Result<usize, EmoteError> {
    submit_rgba_with_workers(canvas, frame, blend, emote_blend_workers())
}

fn submit_rgba_with_workers(
    canvas: &mut Canvas,
    frame: &EmoteRasterFrame,
    blend: BlendMode,
    workers: usize,
) -> std::result::Result<usize, EmoteError> {
    let [origin_x, origin_y] = frame.origin();
    let left = i64::from(origin_x).max(0);
    let top = i64::from(origin_y).max(0);
    let right = (i64::from(origin_x) + i64::from(frame.width())).min(i64::from(canvas.width()));
    let bottom = (i64::from(origin_y) + i64::from(frame.height())).min(i64::from(canvas.height()));
    if right <= left || bottom <= top {
        return Ok(0);
    }
    let submitted = ((right - left) * (bottom - top)) as usize;
    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    if crate::gpu_raster::blend_emote(canvas, frame, blend)
        .map_err(|error| {
            if matches!(error.downcast_ref::<cs2_renderer::RendererError>(),
                Some(cs2_renderer::RendererError::DeviceReadbackFailed))
            {
                EmoteError::CanvasReadback
            } else {
                EmoteError::Backend(error.to_string())
            }
        })?
    {
        return Ok(submitted);
    }
    let source_over = blend == BlendMode::SourceOver;
    let (left, top, right, bottom) = if source_over {
        let Some([first_x, first_y, end_x, end_y]) = frame.alpha_bounds()? else {
            return Ok(submitted);
        };
        (left.max(i64::from(origin_x) + i64::from(first_x)),
         top.max(i64::from(origin_y) + i64::from(first_y)),
         right.min(i64::from(origin_x) + i64::from(end_x)),
         bottom.min(i64::from(origin_y) + i64::from(end_y)))
    } else {
        (left, top, right, bottom)
    };
    if right <= left || bottom <= top {
        return Ok(submitted);
    }
    let target = Rect::new(
        i32::try_from(left).map_err(|_| EmoteError::InvalidFrame)?,
        i32::try_from(top).map_err(|_| EmoteError::InvalidFrame)?,
        (right - left) as u32,
        (bottom - top) as u32,
    );
    let stride = frame.width() as usize * 4;
    let source_x = (left - i64::from(origin_x)) as usize;
    let source_y = (top - i64::from(origin_y)) as usize;
    let offset = source_y * stride + source_x * 4;
    let area = target.width as usize * target.height as usize;
    let workers = if source_over
        && area >= EMOTE_BLEND_PARALLEL_PIXELS
        && target.height >= 16
    {
        workers
    } else {
        1
    };
    canvas.try_pixels().map_err(|_| EmoteError::CanvasReadback)?;
    canvas.blend_rgba_rows_with_workers(target, &frame.rgba()?[offset..], stride, blend, workers)
        .map_err(|error| EmoteError::Backend(error.to_string()))?;
    Ok(submitted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_renderer::Color;

    #[test]
    fn native906_cold_preparation_yields_before_reading_resources() -> anyhow::Result<()> {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-emote-cold-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root)?;
        let mut player = crate::HeadlessPlayer::open_with_save_root(&root, root.join("profile"))?;
        let result = player.with_host(|host| {
            let emote = &mut host.data_services.emote;
            emote.state.bind_m2_tasks().map_err(error)?;
            let viewport = CpuViewport::original_neko_target(1280, 720).map_err(error)?;
            emote.state.set_backend(Box::new(CpuEmoteBackend::new_original_task_clock(viewport).map_err(error)?)).map_err(error)?;
            emote.surface_fade_default = Some(0);
            let target = host.create_task_object(54, host.current_task_id, b"m2plane", b"")?;
            let mut vm = Vm::from_program(cs2_script::KcsProgram::from_code(&[0x3b], vec![0; 256], 64)?)?;
            vm.write(0, b"actor,0\0")?;
            let args = [target, 0, 0].into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
            assert_eq!(host.emote_native(906, &args, &mut vm)?, NativeReturn::Retry,
                "cold preparation must return to the frame before opening configuration or PSZ files");
            assert_eq!(host.data_services.emote.state.state(target).unwrap().successful_resource_imports, 0);
            host.emote_release_task_hook(host.current_task_id)?;
            assert!(!host.data_services.emote.has_pending_loads());
            Ok(())
        });
        std::fs::remove_dir_all(&root)?;
        result
    }

    #[test]
    fn native908_finishes_with_or_without_a_live_emote_task() -> anyhow::Result<()> {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-emote-finish-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root)?;
        let player = crate::HeadlessPlayer::open_with_save_root(&root, root.join("profile"));
        std::fs::remove_dir(&root)?;
        let mut player = player?;
        player.with_host(|host| {
            let emote = &mut host.data_services.emote;
            emote.state.bind_m2_tasks().map_err(error)?;
            let viewport = CpuViewport::original_neko_target(1280, 720).map_err(error)?;
            let backend = CpuEmoteBackend::new_original_task_clock(viewport).map_err(error)?;
            emote.state.set_backend(Box::new(backend)).map_err(error)?;
            emote.surface_fade_default = Some(0);

            let live = host.create_task_object(54, host.current_task_id, b"m2plane", b"")?;
            let ordinary = host.create_task_object(2, host.current_task_id, b"plane", b"")?;
            let deleting = host.create_task_object(54, host.current_task_id, b"m2plane", b"")?;
            host.request_ordinary_task_delete(deleting)?;
            let absent = 22_u32;
            assert!(host.data_services.tasks.message_task_state(absent).is_none());

            let mut vm = Vm::new();
            for target in [live, absent, deleting, ordinary] {
                assert!(matches!(
                    host.emote_native(908, &target.to_le_bytes(), &mut vm)?,
                    NativeReturn::Continue
                ));
            }
            assert!(host.emote_native(909, &absent.to_le_bytes(), &mut vm).is_err());
            assert!(host.emote_native(908, &[], &mut vm).is_err());
            Ok(())
        })
    }

    #[test]
    fn sidecar_emote_submission_preserves_clipping_and_mask_modes() {
        let transparent = [200, 100, 50, 0];
        let opaque = [40, 50, 60, 255];
        let half = [90, 60, 10, 128];
        let pixels = [transparent, transparent, transparent, transparent,
            transparent, opaque, transparent, half, transparent, transparent, transparent, transparent];
        let frame = EmoteRasterFrame::new(4, 3, [-1, -1], pixels.concat()).unwrap();
        assert_eq!(frame.alpha_bounds().unwrap(), Some([1, 1, 4, 2]));
        for blend in [
            BlendMode::SourceOver, BlendMode::AddSource, BlendMode::AddDestination,
            BlendMode::Multiply, BlendMode::InvertDestination, BlendMode::AddMultiply,
            BlendMode::ListAlphaFromBlue, BlendMode::ListAlphaFromGreen,
            BlendMode::ListAlphaFromRed, BlendMode::ListAlphaFromAlpha,
        ] {
            let mut fast = Canvas::new(3, 2).unwrap();
            fast.clear(Color::rgba(20, 30, 40, 200));
            let mut reference = fast.clone();
            for row in 0..2 {
                for column in 0..3 {
                    let [red, green, blue, alpha] = pixels[(row + 1) * 4 + column + 1];
                    reference.blend_pixel_with_mode(column as i32, row as i32, Color::rgba(red, green, blue, alpha), blend).unwrap();
                }
            }
            assert_eq!(submit_rgba(&mut fast, &frame, blend).unwrap(), 6);
            assert_eq!(fast.pixels(), reference.pixels());
        }
    }

    #[test]
    fn large_source_over_submission_matches_serial_with_clipped_transparent_edges() {
        const FRAME_WIDTH: usize = 514;
        const FRAME_HEIGHT: usize = 258;
        const CANVAS_WIDTH: usize = 512;
        const CANVAS_HEIGHT: usize = 256;
        let mut rgba = Vec::with_capacity(FRAME_WIDTH * FRAME_HEIGHT * 4);
        for y in 0..FRAME_HEIGHT {
            for x in 0..FRAME_WIDTH {
                let value = (x * 17 + y * 31) as u8;
                let alpha = match (x + y) % 7 {
                    0 => 0,
                    1 => 255,
                    _ => value,
                };
                rgba.extend_from_slice(&[value, value.wrapping_add(3), value.wrapping_add(9), alpha]);
            }
        }
        let frame = EmoteRasterFrame::new(
            FRAME_WIDTH as u32,
            FRAME_HEIGHT as u32,
            [-1, -1],
            rgba,
        )
        .unwrap();
        let background: Vec<u8> = (0..CANVAS_WIDTH * CANVAS_HEIGHT)
            .flat_map(|index| {
                let value = index as u8;
                [value, value.wrapping_add(11), value.wrapping_add(23), 200]
            })
            .collect();
        let mut reference = Canvas::from_rgba8(CANVAS_WIDTH as u32, CANVAS_HEIGHT as u32, background.clone())
            .unwrap();
        for y in 0..CANVAS_HEIGHT {
            for x in 0..CANVAS_WIDTH {
                let source = ((y + 1) * FRAME_WIDTH + x + 1) * 4;
                let [red, green, blue, alpha] = frame.rgba().unwrap()[source..source + 4] else {
                    unreachable!()
                };
                reference
                    .blend_pixel_with_mode(
                        x as i32,
                        y as i32,
                        Color::rgba(red, green, blue, alpha),
                        BlendMode::SourceOver,
                    )
                    .unwrap();
            }
        }
        for workers in [1, 2, 4] {
            let mut actual = Canvas::from_rgba8(
                CANVAS_WIDTH as u32,
                CANVAS_HEIGHT as u32,
                background.clone(),
            )
            .unwrap();
            assert_eq!(
                submit_rgba_with_workers(
                    &mut actual,
                    &frame,
                    BlendMode::SourceOver,
                    workers,
                )
                .unwrap(),
                CANVAS_WIDTH * CANVAS_HEIGHT
            );
            assert_eq!(actual.pixels(), reference.pixels(), "workers={workers}");
        }
    }

    #[test]
    fn native85_keeps_m2_base_depth_and_consumes_the_xy_setter() -> anyhow::Result<()> {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-emote-base-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root)?;
        let player = crate::HeadlessPlayer::open_with_save_root(&root, root.join("profile"));
        std::fs::remove_dir(&root)?;
        let mut player = player?;
        player.with_host(|host| {
            let emote = &mut host.data_services.emote;
            emote.state.bind_m2_tasks().map_err(error)?;
            let viewport = CpuViewport::original_neko_target(1280, 720).map_err(error)?;
            let backend = CpuEmoteBackend::new_original_task_clock(viewport).map_err(error)?;
            emote.state.set_backend(Box::new(backend)).map_err(error)?;
            emote.surface_fade_default = Some(0);
            let target = host.create_task_object(54, host.current_task_id, b"m2plane", b"")?;
            let ordinary = host.create_task_object(2, host.current_task_id, b"plane", b"")?;
            let command = cs2_runtime::TaskCommand { target: target as i32,
                sender: host.current_task_id as i32, code: 49, data: [0, 0, 1] };
            let base = [100.0_f32, -200.0, 273.5];
            let mut buffers = crate::native_tasks::OwnedTaskBuffers::new(None,
                Some(base.into_iter().flat_map(f32::to_le_bytes).collect()))?;
            let mut output = 0x1234_5678;
            assert!(host.dispatch_registered_task_buffers(command, &mut buffers, &mut output, 4096)?.done);
            assert_eq!(host.scene_nodes[&target].position, [0.0, 0.0, 273.5]);
            let arguments: Vec<u8> = [target, 5.0_f32.to_bits(), 6.0_f32.to_bits(), 0]
                .into_iter().flat_map(u32::to_le_bytes).collect();
            assert!(matches!(host.emote_geometry_native(85, &arguments)?, Some(NativeReturn::Continue)));
            assert!(host.emote_geometry_native(85, &arguments[..12]).is_err());
            assert!(host.emote_geometry_native(85, &ordinary.to_le_bytes())?.is_none());
            assert_eq!(host.scene_nodes[&target].position, [0.0, 0.0, 273.5]);
            let mut query = crate::native_tasks::OwnedTaskBuffers::new(None, Some(vec![0xff; 12]))?;
            assert!(host.dispatch_registered_task_buffers(cs2_runtime::TaskCommand { code: 61, ..command },
                &mut query, &mut output, 4096)?.done);
            let expected: Vec<u8> = [0.0_f32, 0.0, 273.5].into_iter().flat_map(f32::to_le_bytes).collect();
            assert_eq!(query.data1(), Some(expected.as_slice()));
            assert_eq!(output, 0x1234_5678);
            Ok(())
        })
    }

    #[test]
    fn real_rgba_submission_clips_source_pixels_without_a_placeholder() {
        let mut canvas = Canvas::new(2, 1).unwrap();
        canvas.clear(Color::rgba(0, 0, 0, 255));
        let frame =
            EmoteRasterFrame::new(2, 1, [-1, 0], vec![255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
        assert_eq!(submit_rgba(&mut canvas, &frame, BlendMode::SourceOver).unwrap(), 1);
        assert_eq!(canvas.pixel(0, 0).unwrap(), Color::rgba(0, 255, 0, 255));
        assert_eq!(canvas.pixel(1, 0).unwrap(), Color::rgba(0, 0, 0, 255));
        let outside = EmoteRasterFrame::new(1, 1, [2, 0], vec![255; 4]).unwrap();
        assert_eq!(submit_rgba(&mut canvas, &outside, BlendMode::SourceOver).unwrap(), 0);
        assert_eq!(canvas.pixel(1, 0).unwrap(), Color::rgba(0, 0, 0, 255));
    }

    #[test]
    fn native402_sets_the_m2_final_surface_blend() -> anyhow::Result<()> {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-emote-blend-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root)?;
        let player = crate::HeadlessPlayer::open_with_save_root(&root, root.join("profile"));
        std::fs::remove_dir(&root)?;
        let mut player = player?;
        player.with_host(|host| {
            let emote = &mut host.data_services.emote;
            emote.state.bind_m2_tasks().map_err(error)?;
            let viewport = CpuViewport::original_neko_target(1280, 720).map_err(error)?;
            let backend = CpuEmoteBackend::new_original_task_clock(viewport).map_err(error)?;
            emote.state.set_backend(Box::new(backend)).map_err(error)?;
            emote.surface_fade_default = Some(0);
            let target = host.create_task_object(54, host.current_task_id, b"m2plane", b"")?;
            let ordinary = host.create_task_object(2, host.current_task_id, b"plane", b"")?;
            let arguments = |task: u32, blend: u32| [task.to_le_bytes(), blend.to_le_bytes()].concat();
            assert!(host.emote_geometry_native(402, &arguments(ordinary, 1))?.is_none());
            assert!(host.emote_geometry_native(402, &target.to_le_bytes()).is_err());
            let mut canvas = Canvas::new(1, 1).unwrap();
            let background = Color::rgba(5, 10, 15, 255);
            let frame = EmoteRasterFrame::new(1, 1, [0, 0], vec![40, 50, 60, 255]).unwrap();
            for (raw, mode, pixel) in [
                (1, BlendMode::AddSource, Color::rgba(45, 60, 75, 255)),
                (0, BlendMode::SourceOver, Color::rgba(40, 50, 60, 255)),
                (u32::MAX, BlendMode::SourceOver, Color::rgba(40, 50, 60, 255)),
            ] {
                assert!(matches!(host.emote_geometry_native(402, &arguments(target, raw))?, Some(NativeReturn::Continue)));
                let blend = host.data_services.emote.placements[&target].blend;
                assert_eq!(blend, mode);
                canvas.clear(background);
                assert_eq!(submit_rgba(&mut canvas, &frame, blend).unwrap(), 1);
                assert_eq!(canvas.pixel(0, 0).unwrap(), pixel);
            }
            Ok(())
        })
    }
}
