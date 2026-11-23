//! FES sound media policy, independently recovered from the 2.6.1.90 PE.
//! See the D report for SHA256, RVAs and the distinction between a media
//! submission, a synchronous script notification, and device completion.
use cs2_runtime::{TaskCommand, TaskCommandBuffers};
use cs2_script::{KcsProgram, NativeReturn, Result};

use super::{
    LoggingHost, MotionControllerState, MotionTaskState, audio_script_error, create_motion_runner,
    trim_nul,
};

const MAX_MOTION_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Default)]
pub(super) struct SoundMediaStore {
    // ResourceIndex is immutable for a player. Keep the parsed program, not a
    // running VM: every object/bank gets its own VM and parameter storage.
    blend_program: Option<(String, KcsProgram)>,
    pub(super) effects: super::media_wrapper_native::MediaEffectStore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) enum SoundMediaNotification {
    Play,
    Stop,
    Next,
    Reverse,
}

impl SoundMediaNotification {
    /// PE RVA 0x2b50b8 / 0x2b5159: both dispatch tables use this exact order.
    pub(super) fn from_raw(value: u32) -> Result<Self> {
        match value {
            0 => Ok(Self::Play),
            1 => Ok(Self::Stop),
            2 => Ok(Self::Next),
            3 => Ok(Self::Reverse),
            _ => Err(audio_script_error(
                "sound media notification is outside 0..3",
            )),
        }
    }

    pub(super) fn event(self) -> &'static [u8] {
        match self {
            Self::Play => b"PLAY",
            Self::Stop => b"STOP",
            Self::Next => b"NEXT",
            Self::Reverse => b"REV",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
enum NoticePhase {
    Alias,
    AliasRunning,
    Object,
    ObjectRunning,
    Complete,
}

/// One externally established PLAY/STOP/NEXT/REV invocation. The alias is
/// captured at entry, separately from object DEFAULT/button bindings.
/// The caller retains this state across script yields and device failures.
#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct SoundMediaNotice {
    object: Vec<u8>,
    bank: i32,
    notification: SoundMediaNotification,
    alias_header: Vec<u8>,
    statement: u32,
    phase: NoticePhase,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SoundMediaNoticeStep {
    /// Invoke the actual single-statement dispatcher with this persistent
    /// statement cursor and a fresh zero wait-output on *every* iteration.
    /// This is not an ordinary complete alias block or button DEFAULT event.
    Alias {
        header: Vec<u8>,
        statement: u32,
        object: Vec<u8>,
        bank: i32,
    },
    Object {
        object: Vec<u8>,
        bank: i32,
        notification: SoundMediaNotification,
    },
    Waiting,
    Complete,
}

impl SoundMediaNotice {
    pub(crate) fn checkpoint_alias(&self) -> (&[u8], u32) {
        (&self.alias_header, self.statement)
    }

    pub(crate) fn validate_checkpoint(&self, object: &[u8], bank: i32,
        notification: SoundMediaNotification, waiting_alias: bool, waiting_object: bool,
        deleted: bool,
    ) -> anyhow::Result<()> {
        if self.object != object || self.bank != bank || self.notification != notification
            || self.alias_header.len() > 36 || self.alias_header.contains(&0)
            || waiting_alias && waiting_object
            || !deleted && ((self.phase == NoticePhase::AliasRunning) != waiting_alias
                || (self.phase == NoticePhase::ObjectRunning) != waiting_object)
            || deleted && self.phase != NoticePhase::Complete {
            anyhow::bail!("SOUND notification checkpoint lost its actual bank/dispatcher phase");
        }
        Ok(())
    }

    pub(super) fn new(
        object: &[u8],
        bank: i32,
        alias: &[u8],
        notification: SoundMediaNotification,
    ) -> Result<Self> {
        let object = trim_nul(object);
        let alias = trim_nul(alias);
        if object.len() >= 32 || alias.len() >= 32 {
            return Err(audio_script_error("media object/alias exceeds31 encoded bytes"));
        }
        let mut alias_header = alias.to_vec();
        if !alias.is_empty() {
            alias_header.push(b'.');
            alias_header.extend_from_slice(notification.event());
        }
        Ok(Self {
            object: object.to_vec(),
            bank,
            notification,
            alias_header,
            statement: 1,
            phase: if alias.is_empty() { NoticePhase::Object } else { NoticePhase::Alias },
        })
    }

    pub(super) fn poll(&self) -> SoundMediaNoticeStep {
        match self.phase {
            NoticePhase::Alias => SoundMediaNoticeStep::Alias {
                header: self.alias_header.clone(),
                statement: self.statement,
                object: self.object.clone(),
                bank: self.bank,
            },
            NoticePhase::Object => SoundMediaNoticeStep::Object {
                object: self.object.clone(),
                bank: self.bank,
                notification: self.notification,
            },
            NoticePhase::AliasRunning | NoticePhase::ObjectRunning => SoundMediaNoticeStep::Waiting,
            NoticePhase::Complete => SoundMediaNoticeStep::Complete,
        }
    }

    /// Commit only after the script dispatcher accepted the invocation.
    pub(super) fn alias_started(&mut self) -> Result<()> {
        if self.phase != NoticePhase::Alias {
            return Err(audio_script_error("media alias has no pending dispatch"));
        }
        self.phase = NoticePhase::AliasRunning;
        Ok(())
    }

    /// PE 0x2b5130 resets the wait output, but preserves the statement cursor
    /// supplied at0x2b511f. A false dispatcher return repeats; an empty/missing
    /// alias is not silently promoted to successful completion.
    pub(super) fn alias_returned(&mut self, complete: bool, statement: u32) -> Result<()> {
        if self.phase != NoticePhase::AliasRunning {
            return Err(audio_script_error("media alias completion has no active dispatch"));
        }
        self.statement = statement;
        self.phase = if complete { NoticePhase::Object } else { NoticePhase::Alias };
        Ok(())
    }

    pub(super) fn object_started(&mut self) -> Result<()> {
        if self.phase != NoticePhase::Object {
            return Err(audio_script_error("media object has no pending notification"));
        }
        self.phase = NoticePhase::ObjectRunning;
        Ok(())
    }

    pub(super) fn object_returned(&mut self) -> Result<()> {
        if self.phase != NoticePhase::ObjectRunning {
            return Err(audio_script_error("media object completion has no active notification"));
        }
        self.phase = NoticePhase::Complete;
        Ok(())
    }

    /// Actual owner/bank destruction cancels pending callbacks. Playback EOF,
    /// crossfade completion and a statement budget boundary do not cancel it.
    pub(super) fn owner_released(&mut self) {
        self.phase = NoticePhase::Complete;
    }
}

#[derive(Clone, Copy, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) enum SoundMediaRequest {
    Play {
        count: u32,
        allowed: bool,
        enabled: bool,
    },
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SoundMediaProgress {
    Notify(SoundMediaNotification),
    Waiting,
    Complete,
}

/// Retain this value across host failures and across the PLAY handler. Each
/// successfully submitted physical command is committed before the next one.
/// Only the FES caller owns/clears its action flag after Complete; callback
/// commands must remain in the live object rather than a copied record.
#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct SoundMediaSubmission {
    request: SoundMediaRequest,
    channel: u32,
    stage: u8,
}

impl SoundMediaSubmission {
    pub(crate) fn validate_checkpoint(&self, callback_running: bool) -> anyhow::Result<()> {
        let valid = match self.request {
            SoundMediaRequest::Stop => matches!(self.stage, 0 | 1 | 3),
            SoundMediaRequest::Play { allowed, .. } => self.stage <= 4
                && (allowed || !matches!(self.stage, 2 | 4)),
        };
        if !valid || (self.stage == 4) != callback_running {
            anyhow::bail!("SOUND submission checkpoint has an invalid accepted-command/callback phase");
        }
        Ok(())
    }

    pub(crate) fn checkpoint_channel(&self) -> u32 { self.channel }

    pub(super) fn new(channel: u32, request: SoundMediaRequest) -> Self {
        Self {
            request,
            channel,
            stage: 0,
        }
    }

    /// A PLAY notification is returned even for an empty source or invalid
    /// channel. No script is evaluated here. The caller synchronously enters
    /// the media handler, waits for its completion, then acknowledges it.
    pub(super) fn advance(
        &mut self,
        host: &mut LoggingHost<'_>,
        now_millis: u32,
        started_tick: &mut u32,
    ) -> Result<SoundMediaProgress> {
        match self.request {
            SoundMediaRequest::Stop => {
                if self.stage == 0 {
                    host.sound_media_control(185, self.channel, 0)?;
                    self.stage = 1;
                }
                if self.stage == 1 {
                    if matches!(
                        host.poll_task_audio_start(host.current_task_id)?,
                        Some(NativeReturn::Retry)
                    ) {
                        return Ok(SoundMediaProgress::Waiting);
                    }
                    self.stage = 3;
                }
                Ok(SoundMediaProgress::Complete)
            }
            SoundMediaRequest::Play {
                count,
                allowed,
                enabled,
            } => {
                if self.stage == 0 {
                    host.sound_media_control(
                        if allowed && enabled {
                            183
                        } else if allowed {
                            218
                        } else {
                            185
                        },
                        self.channel,
                        if allowed && enabled { count } else { 0 },
                    )?;
                    self.stage = 1;
                }
                if self.stage == 1 {
                    // Device startup may span logical frames. The accepted
                    // command owns stage1 until its completion, so a retry
                    // neither submits playback twice nor enters PLAY early.
                    if matches!(
                        host.poll_task_audio_start(host.current_task_id)?,
                        Some(NativeReturn::Retry)
                    ) {
                        return Ok(SoundMediaProgress::Waiting);
                    }
                    // Forbidden playback submits BOTH stop and first-time
                    // registration, in that order, and skips PLAY.
                    if !allowed {
                        host.sound_media_control(218, self.channel, 0)?;
                    }
                    if *started_tick == u32::MAX {
                        *started_tick = now_millis;
                    }
                    self.stage = if allowed { 2 } else { 3 };
                }
                Ok(if self.stage == 2 {
                    SoundMediaProgress::Notify(SoundMediaNotification::Play)
                } else if self.stage == 4 {
                    SoundMediaProgress::Waiting
                } else {
                    SoundMediaProgress::Complete
                })
            }
        }
    }

    /// Commit after the driver accepted entry. On driver entry failure keep
    /// stage2 so it can retry; when it yields, stage4 prevents a second entry.
    pub(super) fn callback_started(&mut self) -> Result<()> {
        if self.stage != 2 {
            return Err(audio_script_error(
                "sound media has no pending PLAY callback",
            ));
        }
        self.stage = 4;
        Ok(())
    }

    pub(super) fn complete_callback(&mut self) -> Result<()> {
        if !matches!(self.stage, 2 | 4) {
            return Err(audio_script_error(
                "sound media has no pending PLAY callback",
            ));
        }
        self.stage = 3;
        Ok(())
    }
}

impl LoggingHost<'_> {
    /// Action precheck and playback permission. Legacy mode ignores STYPE;
    /// both branches call the original 606 query. Preserve the integer, since
    /// the caller must OR it with play parameter three before testing zero.
    pub(super) fn sound_media_file_play(&mut self, file: &[u8], system_voice: bool) -> Result<i32> {
        if self.sound_config_enabled == 0 {
            return self
                .legacy_sound_file_value(606, file)
                .map(|value| value as i32);
        }
        let file = checked_file(file)?;
        Ok(self.sound_config.file_part(file).map_or(1, |part| {
            if system_voice {
                part.system_voice
            } else {
                part.play_enabled
            }
        }))
    }

    /// Media property formula. TYPE selects this branch; STYPE does not.
    /// Every multiplication wraps at signed 32 bits before truncating /100.
    pub(super) fn sound_media_volume(
        &mut self,
        file: &[u8],
        group: &[u8],
        category_type: i32,
        object_volume: i32,
    ) -> Result<i32> {
        let group = trim_nul(group);
        if self.sound_config_enabled == 0 {
            if category_type != 0 {
                return Ok(if self.legacy_sound_group_muted(group)? {
                    0
                } else {
                    object_volume
                });
            }
            let category_volume = if group.is_empty() {
                self.legacy_sound_file_value(605, file)?
            } else {
                self.legacy_sound_group_volume(group)?
            };
            return Ok((category_volume as i32).wrapping_mul(object_volume) / 100);
        }
        // Master mute is checked before any filename/group query. Do not
        // require an otherwise unused master volume in this early branch.
        if self.sound_config.master_mute != 0 && self.sound_config.master_mute != -1 {
            return Ok(0);
        }
        let mut volume = self
            .sound_config
            .master_volume
            .ok_or_else(|| audio_script_error("SOUND master volume is unavailable"))?;
        if category_type == 0 {
            if !group.is_empty() {
                volume = volume.wrapping_mul(self.sound_config.group_volume(group)) / 100;
            }
            let file = checked_file(file)?;
            let part = self.sound_config.file_part(file);
            let file_volume = self
                .sound_config
                .override_value(file, 0)
                .unwrap_or_else(|| part.map_or(100, |part| part.volume));
            volume = volume.wrapping_mul(file_volume) / 100;
            let muted = self
                .sound_config
                .override_value(file, 2)
                .unwrap_or_else(|| {
                    part.map_or(100, |part| if part.mute == -1 { 0 } else { part.mute })
                });
            if muted != 0 {
                volume = 0;
            }
        }
        Ok(volume.wrapping_mul(object_volume) / 100)
    }

    /// The separate post-media natural-STOP scan at PE RVA 0x17aee0.
    /// If true, first request play=false on the live FES object, THEN invoke
    /// STOP synchronously. Its pending physical stop is consumed in the next
    /// media pass. Never clear a new play request created inside STOP.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn sound_media_natural_stop(
        &mut self,
        file: &[u8],
        stype: i32,
        count: i32,
        channel: u32,
        requested_play: bool,
        settings_dirty: bool,
    ) -> Result<bool> {
        if settings_dirty || !requested_play {
            return Ok(false);
        }
        // Natural completion ignores play parameter three. In the old path
        // nonzero counts bypass per-file play only AFTER a matching definition.
        let allowed = if self.sound_config_enabled != 0 {
            self.sound_media_file_play(file, stype != 0)? != 0
        } else if self.legacy_sound_file_value(844, file)? == u32::MAX {
            false
        } else {
            count != 0 || self.legacy_sound_file_value(606, file)? != 0
        };
        if allowed {
            return Ok(self.sound_media_control(186, channel, 0)? == 0);
        }
        Ok(count != 0)
    }

    /// Exact kind13 command route, including 218 and invalid-channel no-ops.
    /// FES must not guard this with a resource-is-loaded check: 183 stamps the
    /// physical channel even when its resource is absent.
    pub(super) fn sound_media_control(
        &mut self,
        code: u32,
        channel: u32,
        value: u32,
    ) -> Result<u32> {
        if !matches!(code, 182..=188 | 190..=191 | 218..=219) {
            return Err(audio_script_error("unsupported sound media command"));
        }
        if let Some(task) = self.default_sound_task()? {
            let mut output = 0;
            let consumed = self.dispatch_sound_task_command(
                task,
                TaskCommand {
                    target: task as i32,
                    sender: self.current_task_id as i32,
                    code,
                    data: [channel, value, 0],
                },
                &mut TaskCommandBuffers {
                    data0: None,
                    data1: None,
                },
                &mut output,
            )?;
            if !consumed {
                return Err(audio_script_error(
                    "sound task did not consume a media command",
                ));
            }
            return Ok(output);
        }
        if code == 218 {
            if channel < 64 {
                self.stamp_sound_start(channel);
                if let Some(linked) = self
                    .sound_channels
                    .get(&channel)
                    .and_then(|state| state.task.linked)
                    .filter(|&linked| linked < 64)
                {
                    self.stamp_sound_start(linked);
                }
            }
            return Ok(0);
        }
        if code == 219 {
            self.control_sound_play_count(channel, value)?;
            return Ok(0);
        }
        let result = self.control_sound_channel_values(code - 26, channel, value, 0)?;
        Ok(match result {
            NativeReturn::Value(value) => value,
            _ => 0,
        })
    }

    /// Blend's resource name is the complete UTF-16 string pColor, not "p".
    /// Capture the baseline in the common object-tree motion owner before
    /// calling this, only when target AND descendants have no active motion.
    /// Return false solely when the resolved resource does not exist.
    pub(super) fn start_sound_blend_motion(
        &mut self,
        controller: &mut MotionControllerState,
        value: i32,
        delay: u32,
    ) -> anyhow::Result<bool> {
        let names = self.resources.category_names(b"mot", "pColor")?;
        let mut selected = None;
        for name in names {
            if let Some(resource) = self.resources.find(&name) {
                if resource.size > MAX_MOTION_BYTES {
                    anyhow::bail!("SOUND pColor motion exceeds 64 MiB");
                }
                let cached = &self.data_services.sound_media.blend_program;
                let program = if let Some((_, program)) =
                    cached.as_ref().filter(|(key, _)| key == &name)
                {
                    program.clone()
                } else {
                    let program = KcsProgram::parse(&self.resources.read_candidate(resource)?)?;
                    self.data_services.sound_media.blend_program = Some((name, program.clone()));
                    program
                };
                selected = Some(program);
                break;
            }
        }
        let Some(program) = selected else {
            return Ok(false);
        };
        let mut parameters = [0; 256];
        for (slot, value) in parameters.iter_mut().zip([value as f32, 255., 255., 255.]) {
            *slot = value.to_bits();
        }
        let id = controller.next_task_id;
        let next = id
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("SOUND motion id overflow"))?;
        let mut runner = create_motion_runner(
            program,
            b"pColor",
            &parameters,
            &[],
            0,
            controller.base_properties,
            0,
        )?;
        runner.set_base_time(delay)?;
        controller.tasks.insert(
            id,
            MotionTaskState {
                name: b"pColor".to_vec(),
                group: 0,
                parameters,
                saved_data: Vec::new(),
                snapshot_flags: 0,
                external_value: 0,
                origin: 0,
                elapsed: 0,
                running: 1,
                finishing: 0,
                runner,
            },
        );
        controller.next_task_id = next;
        Ok(true)
    }
}

/// The original memset(144) stores pan at byte24 and alpha at byte115.
/// DWORD0 belongs to a different vector in the common motion property layout.
pub(super) fn sound_motion_properties(volume: i32, pan: i32) -> [u32; 36] {
    let mut words = [0; 36];
    words[6] = (pan as f32).to_bits();
    words[28] = (volume as u32 & 255) << 24;
    words
}

pub(super) fn sound_motion_values(words: &[u32; 36]) -> (i32, i32) {
    (
        (words[28] >> 24) as i32,
        super::sound_task::x86_i32(f32::from_bits(words[6])),
    )
}

fn checked_file(file: &[u8]) -> Result<&[u8]> {
    let file = trim_nul(file);
    if file.len() >= 260 {
        return Err(audio_script_error("SOUND media filename exceeds 259 bytes"));
    }
    Ok(file)
}

#[cfg(test)]
mod async_media_tests {
    use super::*;
    use crate::{HeadlessPlayer, SoundChannelState, sound_async};
    use cs2_runtime::{AudioChannel, AudioRequest, AudioStart};

    #[test]
    fn async_fes_play_enters_callback_only_after_device_completion() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("cs2-fes-audio-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut player = HeadlessPlayer::open_with_save_root(&directory, directory.join("profile")).unwrap();
        let (completion, gate) = std::sync::mpsc::channel();
        let request = AudioRequest { channel: AudioChannel::SoundEffect, resource: "gated.ogg".into(),
            volume: 1.0, pan: 0.0, looped: false };
        let AudioStart::Pending(ticket) = player.audio_host.begin_test_pending_start(request, gate).unwrap()
            else { panic!("test startup must remain pending"); };
        player.sound_channels.insert(6, SoundChannelState { resource: Some("gated.ogg".into()),
            pending_start: Some(sound_async::PendingSoundStart { started: std::time::Instant::now(), owner: player.current_task_id,
                index: 157, value: 1, configured: Some(AudioChannel::SoundEffect), transition: None,
                operation: sound_async::Operation::Start(ticket) }), ..Default::default() });
        // The physical command has been accepted; the media dispatcher owns
        // stage1 and must retain the callback until the device responds.
        let mut submission = SoundMediaSubmission::new(6,
            SoundMediaRequest::Play { count: 1, allowed: true, enabled: true });
        submission.stage = 1;
        let mut started = u32::MAX;
        for _ in 0..3 {
            assert_eq!(player.with_host(|host| Ok(submission.advance(host, 7, &mut started)?)).unwrap(),
                SoundMediaProgress::Waiting);
            assert_eq!(started, u32::MAX);
            assert!(player.sound_channels[&6].handle.is_none());
        }
        completion.send(Ok(())).unwrap();
        let mut progress = SoundMediaProgress::Waiting;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while std::time::Instant::now() < deadline {
            progress = player.with_host(|host| Ok(submission.advance(host, 7, &mut started)?)).unwrap();
            if progress != SoundMediaProgress::Waiting { break; }
            std::thread::yield_now();
        }
        assert_eq!(progress, SoundMediaProgress::Notify(SoundMediaNotification::Play));
        assert_eq!(started, 7);
        assert!(player.sound_channels[&6].handle.is_some());
        submission.callback_started().unwrap();
        assert_eq!(player.with_host(|host| Ok(submission.advance(host, 8, &mut started)?)).unwrap(),
            SoundMediaProgress::Waiting);
        submission.complete_callback().unwrap();
        assert_eq!(player.with_host(|host| Ok(submission.advance(host, 8, &mut started)?)).unwrap(),
            SoundMediaProgress::Complete);
        drop(player);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
