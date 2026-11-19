//! NEKOPARA ordinary-table E-mote ABI planning and CPU pixel submission.
//!
//! DataServices owns this service. Task creation/update/release use additive
//! hooks, while exact-profile startup and strict scene drawing live in lib.rs.
//! Timed M2 chains, audio-device origin and target0 routing remain partial.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};

use cs2_renderer::{BlendMode, Canvas, Color};
use cs2_runtime::ResourceIndex;
use cs2_runtime::emote::{
    CpuEmoteBackend, CpuEmoteResources, CpuM2ActionConfig, CpuM2Config, CpuM2ControllerConfig,
    CpuM2DifferenceTransform, CpuM2LipBinding, CpuM2LipSidecarKind, CpuM2Load, CpuM2LoadSpec,
    CpuM2ProfileSelection, CpuM2ResetBankSeeds, CpuM2SplineConfig, CpuViewport, EmoteBatchValue, EmoteBuffer,
    EmoteCommand, EmoteDispatchShape, EmoteError, EmoteNativeDescriptor, EmotePacking,
    EmoteRasterFrame, EmoteReply, EmoteResource, EmoteServices, EmoteTick, NEKOPARA_PE_SHA256,
    emote_native_descriptor,
};
use cs2_script::{NativeReturn, Result, ScriptError, Vm};

use super::{LoggingHost, expand_kcs_format};

const MAX_FORMAT_BYTES: usize = 4095;
const MAX_BATCH_ENTRIES: usize = 1_000_000;

#[path = "emote_geometry.rs"]
mod geometry;
#[path = "emote_settings.rs"]
pub(super) mod settings;

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
        host.data_services
            .emote
            .load_m2(host.resources, target, spec, addressed.data[0])?;
        if let Some(node) = host.scene_nodes.get_mut(&target) {
            node.initialized = true;
        }
    } else if addressed.code == 540 {
        host.emote_link_legacy_sound_lip(target, addressed.data[1], addressed.data[0])?;
    } else {
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
    pub(super) resource: Option<EmoteResource>,
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
    )
}

fn load_m2_resource_plan_impl(
    resources: &ResourceIndex,
    startup_config_category: &[u8],
    startup_config_name: &[u8],
    formatted_spec: &[u8],
    reusable_resource_name: Option<&[u8]>,
) -> std::result::Result<EmoteM2ResourcePlan, EmoteError> {
    fn read(
        resources: &ResourceIndex,
        category: &[u8],
        name: &[u8],
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
        resources
            .read_category(category, &decoded)
            .map_err(|e| EmoteError::Backend(e.to_string()))
    }
    fn single(config: &CpuM2Config, section: &[u8]) -> std::result::Result<Vec<u8>, EmoteError> {
        let rows = config.records(section);
        if rows.len() != 1 || rows[0].len() != 1 {
            return Err(EmoteError::InvalidResource("M2 required singleton section"));
        }
        Ok(rows[0][0].clone())
    }
    let view = CpuM2Config::parse_cp932(&read(
        resources,
        startup_config_category,
        startup_config_name,
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
    let profile = CpuM2Config::parse_cp932(&read(resources, &profile_category, &filename)?)?;
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
        let config = CpuM2Config::parse_cp932(&read(resources, &profile_category, &name)?)?;
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
    //17a990 preserves an already loaded resource with the same name. Avoid
    //reading/inflating a68MiB PSB for every script pattern/timeline change.
    let resource = if reusable_resource_name == Some(resource_name.as_slice()) {
        None
    } else {
        Some(EmoteResource::decode(&read(
            resources,
            archive_category,
            &resource_name,
        )?)?)
    };
    Ok(EmoteM2ResourcePlan {
        load_spec,
        selection,
        difference_transforms: differences,
        config_files: configs,
        resource_name,
        resource,
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
    target_size: Option<[u32; 2]>,
    sound_links: BTreeMap<u32, EmoteSoundLink>,
    placements: BTreeMap<u32, geometry::Placement>,
    surface_fade_default: Option<u32>,
    material_settings: settings::M2Settings,
    spline_config: Arc<CpuM2SplineConfig>,
    action_config: Arc<CpuM2ActionConfig>,
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
        let target = word(args, 0)?;
        if !self
            .data_services
            .tasks
            .message_task_state(target)
            .is_some_and(|(kind, deletion, _)| kind == 54 && deletion == 0)
        {
            return Err(unsupported(
                index,
                "addressed live type54 task is unavailable",
            ));
        }
        if index == 906 {
            let spec = formatted(vm, word(args, 1)?)?;
            let mode = word(args, 2)?;
            let result = self
                .data_services
                .emote
                .load_m2(self.resources, target, &spec, mode)?;
            if let Some(node) = self.scene_nodes.get_mut(&target) {
                node.initialized = true;
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
    pub(super) fn initialize(&mut self, resources: &ResourceIndex, sha256: &str) -> Result<()> {
        if !NEKOPARA_PE_SHA256.eq_ignore_ascii_case(sha256) {
            return Ok(());
        }
        self.state.bind_pe_sha256(sha256);
        // Fixed-profile startup names; dimensions come from the actual file.
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
        let backend = CpuEmoteBackend::new_original_task_clock(viewport).map_err(error)?;
        self.state.set_backend(Box::new(backend)).map_err(error)?;
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
        task: u32,
        spec: &[u8],
        mode: u32,
    ) -> Result<NativeReturn> {
        let current_name = self.state.playback_info(task).ok().map(|i| i.resource_name);
        let plan = load_m2_resource_plan_impl(
            resources,
            b"m2config",
            b"m2view.cnf",
            spec,
            current_name.as_deref(),
        )
        .map_err(error)?;
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
        self.state
            .import_m2_resource(task, &plan.resource_name, plan.resource.as_ref(), load)
            .map_err(error)?;
        let placement = self.placements.entry(task).or_default();
        placement.profile_position = position.map(|value| value as f32);
        placement.profile_scale = scale[0];
        placement.scale_offset = scale_offset.map(|value| value as f32);
        self.apply_placement(task).map_err(error)?;
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
        if self.target_size != Some([canvas.width(), canvas.height()]) {
            return Err(EmoteError::Backend(
                "type54 final display quad requires a resolved host offset/scale mapping".into(),
            ));
        }
        self.state.set_base_argb(node.id, node.color)?;
        self.draw(node.id, canvas)?;
        Ok(true)
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

    /// Render an actual evaluated frame, then submit its RGBA into the same CPU
    /// canvas used by other objects. Never substitute a static or empty frame.
    pub(super) fn draw(
        &mut self,
        resolved_target: u32,
        canvas: &mut Canvas,
    ) -> std::result::Result<usize, EmoteError> {
        let frame = self.state.render_rgba(resolved_target)?;
        let blend = self.placements.get(&resolved_target).ok_or(EmoteError::UnknownTask(resolved_target))?.blend;
        let submitted = submit_rgba(canvas, &frame, blend)?;
        if submitted != 0 {
            self.state.record_pixel_submission(resolved_target)?;
        }
        Ok(submitted)
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

pub(super) fn submit_rgba(
    canvas: &mut Canvas,
    frame: &EmoteRasterFrame,
    blend: BlendMode,
) -> std::result::Result<usize, EmoteError> {
    let [origin_x, origin_y] = frame.origin();
    let left = i64::from(origin_x).max(0);
    let top = i64::from(origin_y).max(0);
    let right = (i64::from(origin_x) + i64::from(frame.width())).min(i64::from(canvas.width()));
    let bottom = (i64::from(origin_y) + i64::from(frame.height())).min(i64::from(canvas.height()));
    if right <= left || bottom <= top {
        return Ok(0);
    }
    let mut written = 0;
    for y in top..bottom {
        for x in left..right {
            let source_x = (x - i64::from(origin_x)) as usize;
            let source_y = (y - i64::from(origin_y)) as usize;
            let offset = (source_y * frame.width() as usize + source_x) * 4;
            let p = &frame.rgba()[offset..offset + 4];
            let x = i32::try_from(x).map_err(|_| EmoteError::InvalidFrame)?;
            let y = i32::try_from(y).map_err(|_| EmoteError::InvalidFrame)?;
            canvas
                .blend_pixel_with_mode(x, y, Color::rgba(p[0], p[1], p[2], p[3]), blend)
                .map_err(|error| EmoteError::Backend(error.to_string()))?;
            written += 1;
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(emote.state.bind_pe_sha256(NEKOPARA_PE_SHA256));
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
            assert!(emote.state.bind_pe_sha256(NEKOPARA_PE_SHA256));
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
