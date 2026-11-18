//! Ordinary201/214/215/903/904/905, with independently typed media contracts.
use std::collections::BTreeMap;
use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_script::{NativeReturn, Result, Vm};

use super::{LoggingHost, expand_kcs_format, native_state_error, read_u32, trim_nul};

const V401: &str = "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f";
const YUKIKOI: &str = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976";

#[derive(Default)]
pub(super) struct MediaEffectStore {
    raster: BTreeMap<u32, RasterParameters>,
}

#[derive(Debug, Clone, Copy, PartialEq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct RasterParameters {
    pub(super) mode: u32,
    pub(super) period_bits: u32,
    pub(super) amplitude_bits: u32,
    pub(super) relative: bool,
    pub(super) constructor_word: Option<u32>,
    pub(super) configured: bool,
}

impl MediaEffectStore {
    /// Graphics-only configuration. Physical SOUND ownership is independent.
    pub(crate) fn raster_graph_snapshot(&self) -> BTreeMap<u32, RasterParameters> {
        self.raster.clone()
    }

    pub(crate) fn install_raster_graph(&mut self, raster: BTreeMap<u32, RasterParameters>) {
        self.raster = raster;
    }
}

impl LoggingHost<'_> {
    pub(super) fn media_family_native(
        &mut self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        match index {
            201 => self.media_effect_parameters_native(args, vm),
            214 => {
                if args.len() < 4 {
                    return Err(native_state_error("native214 requires oneRECT output address"));
                }
                vm.write(read_u32(args, 0)?, &[0; 16])?;
                Ok(NativeReturn::Continue)
            }
            215 => {
                if args.len() < 8 {
                    return Err(native_state_error("native215 requires twoRECT addresses"));
                }
                let target = read_u32(args, 0)?;
                // Both source RECTs are snapshotted before writing, including
                // partial overlap. Win32 considers inverted rectangles empty.
                let left = rect(&vm.read(target, 16)?)?;
                let right = rect(&vm.read(read_u32(args, 4)?, 16)?)?;
                let empty = |r: [i32; 4]| r[2] <= r[0] || r[3] <= r[1];
                let output = match (empty(left), empty(right)) {
                    (true, true) => [0; 4],
                    (false, true) => left,
                    (true, false) => right,
                    (false, false) => [
                        left[0].min(right[0]), left[1].min(right[1]),
                        left[2].max(right[2]), left[3].max(right[3]),
                    ],
                };
                vm.write(target, &output.into_iter().flat_map(i32::to_le_bytes).collect::<Vec<_>>())?;
                Ok(NativeReturn::Continue)
            }
            903 => self.sound_tag_wrapper_native(args, vm),
            904 => {
                if args.len() < 8 {
                    return Err(native_state_error("native904 requires output and format addresses"));
                }
                let target = read_u32(args, 0)?;
                let original = vm.read_string(target)?;
                let expanded = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
                let original = trim_nul(&original);
                let expanded = trim_nul(&expanded);
                if original.len() > 2046 || expanded.len() > 2046 {
                    return Err(native_state_error("native904 comma table exceeds2048 bytes"));
                }
                let tokens = |source: &[u8]| -> Vec<Vec<u8>> {
                    super::split_kcs_string_table(source, b",")
                        .split(|&byte| byte == 0).filter(|part| !part.is_empty())
                        .map(ToOwned::to_owned).collect()
                };
                let prior = tokens(original);
                let requested = tokens(expanded);
                let mut output = Vec::new();
                for (index, token) in requested.iter().enumerate() {
                    if index != 0 { output.push(b','); }
                    output.extend_from_slice(if token == b"@" {
                        prior.get(index).map_or(b"0".as_slice(), Vec::as_slice)
                    } else { token });
                }
                output.push(0);
                vm.write(target, &output)?;
                Ok(NativeReturn::Continue)
            }
            905 => {
                if args.len() < 20 {
                    return Err(native_state_error("native905 requires fiveDWORDs"));
                }
                let target = read_u32(args, 0)?;
                let mask = read_u32(args, 16)?;
                if target == 0 || mask == 0 { return Ok(NativeReturn::Continue); }
                // Original unwraps mask's resource pointer. The semantic
                // render API deliberately accepts the owned wrapper instead.
                let mut record = args[8..16].to_vec();
                record.extend_from_slice(&args[4..8]);
                record.extend_from_slice(&mask.to_le_bytes());
                record.extend_from_slice(&1u32.to_le_bytes());
                self.render_task_command_bytes(
                    target, super::native_tasks::RenderTaskOperation::ConfigureWipe,
                    0, Some(&record), &mut 0,
                ).map_err(|cause| self.task_error(target,
                    cs2_runtime::RuntimeError::Host(cause.to_string())))?;
                Ok(NativeReturn::Continue)
            }
            _ => Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
        }
    }

    fn sound_tag_wrapper_native(&mut self, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        if args.len() < 8 {
            return Err(native_state_error("native903 requires channel and tag format"));
        }
        // Preserve original order: root sound-service query precedes format.
        let task = self.default_sound_task()?;
        let expanded = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
        let name = trim_nul(&expanded);
        if name.len() >= 1024 {
            return Err(native_state_error("native903 tag format exceeds1023 bytes"));
        }
        let Some(task) = task else { return Ok(NativeReturn::Value(0)); };
        let mut name = name.to_vec();
        name.push(0);
        let mut output = 0;
        if !self.dispatch_sound_task_command(
            task,
            TaskCommand { target: task as i32, sender: self.current_task_id as i32,
                code: 215, data: [read_u32(args, 0)?, 1, 0] },
            &mut TaskCommandBuffers { data0: None, data1: Some(&mut name) },
            &mut output,
        )? {
            return Err(native_state_error("native903 sound tag receiver did not consume its request"));
        }
        Ok(NativeReturn::Value(output))
    }

    fn media_effect_parameters_native(&mut self, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        if args.len() < 12 {
            return Err(native_state_error("native201 requires target, mode and record address"));
        }
        let target = read_u32(args, 0)?;
        let mut kind = 0;
        let query = self.receive_task_command(
            TaskCommand { target: target as i32, sender: self.current_task_id as i32,
                code: 13, data: [0; 3] },
            &mut TaskCommandBuffers::default(), &mut kind, 4096,
        )?;
        if query.is_some_and(|outcome| !outcome.done) { return Ok(NativeReturn::Retry); }
        if !matches!(kind, 18 | 25) || read_u32(args, 4)? != 0 {
            return Ok(NativeReturn::Continue);
        }
        let command = match (profile(self)?, kind) {
            (false, 18) => 259,
            (false, 25) => 268,
            (true, 18) => 270,
            (true, 25) => 279,
            _ => unreachable!(),
        };
        let mut record = if kind == 18 {
            Some(vm.read(read_u32(args, 8)?, 20)?)
        } else {
            // The identified Hydro base receiver never reads this record.
            None
        };
        let outcome = self.receive_media_effect_command(
            TaskCommand { target: target as i32, sender: self.current_task_id as i32,
                code: command, data: [0, 1, 0] },
            &mut TaskCommandBuffers { data0: None, data1: record.as_deref_mut() },
            &mut 0,
        )?.ok_or_else(|| native_state_error("native201 has no concrete media effect recipient"))?;
        Ok(if outcome.done { NativeReturn::Continue } else { NativeReturn::Retry })
    }

    /// Shared recipient for C's ordinary69/70 path. The type18 Raster update
    /// changes its actual configuration fields; it does not start the effect.
    /// Type25 Hydro's identified receiver delegates every command unchanged.
    pub(super) fn receive_media_effect_command(
        &mut self, command: TaskCommand, buffers: &mut TaskCommandBuffers<'_>,
        _output: &mut u32,
    ) -> Result<Option<TaskCommandOutcome>> {
        let target = command.target as u32;
        let Some((kind, deletion, _)) = self.data_services.tasks.message_task_state(target)
        else { return Ok(None); };
        if !matches!(kind, 18 | 25) { return Ok(None); }
        let yukikoi = profile(self)?;
        if kind == 25 {
            if command.code != if yukikoi { 279 } else { 268 } { return Ok(None); }
            return Ok(Some(TaskCommandOutcome { done: true, propagate: deletion == 0 }));
        }
        if command.code != if yukikoi { 270 } else { 259 } { return Ok(None); }
        if deletion != 0 {
            return Ok(Some(TaskCommandOutcome { done: true, propagate: false }));
        }
        let bytes = buffers.data1.as_deref()
            .filter(|bytes| bytes.len() >= 20)
            .ok_or_else(|| native_state_error("Raster parameter update requires20 explicit bytes"))?;
        let mut state = self.raster_media_parameters(target).unwrap_or(RasterParameters {
            mode: 0, period_bits: 0, amplitude_bits: 1f32.to_bits(), relative: false,
            constructor_word: None, configured: false,
        });
        state.mode = read_u32(bytes, 8)?;
        let period = f32::from_bits(read_u32(bytes, 12)?);
        let amplitude = f32::from_bits(read_u32(bytes, 16)?);
        if state.relative {
            let basis = state.constructor_word.ok_or_else(|| native_state_error(
                "relative Raster update requires the actual constructor word",
            ))?;
            // PE converts task+48 as an unsigned DWORD to f32. Its name is
            // intentionally not guessed as FPS, screen width or a handle.
            state.period_bits = (1f32 / period).to_bits();
            // Constant VA0x8e8ed8 is the IEEE754 value60.0.
            state.amplitude_bits = ((basis as f32) * (amplitude / 60f32)).to_bits();
        } else {
            state.period_bits = period.to_bits();
            state.amplitude_bits = amplitude.to_bits();
        }
        state.configured = true;
        let live = &self.data_services.tasks;
        self.data_services.sound_media.effects.raster.retain(|&id, _| {
            live.message_task_state(id).is_some_and(|(_, deletion, _)| deletion == 0)
        });
        if self.data_services.sound_media.effects.raster.len() >= 65_536
            && !self.data_services.sound_media.effects.raster.contains_key(&target)
        {
            return Err(native_state_error("Raster parameter owner limit exceeded65536"));
        }
        self.data_services.sound_media.effects.raster.insert(target, state);
        Ok(Some(TaskCommandOutcome { done: true, propagate: false }))
    }

    pub(super) fn raster_media_parameters(&self, target: u32) -> Option<RasterParameters> {
        self.data_services.tasks.message_task_state(target)
            .filter(|&(kind, deletion, _)| kind == 18 && deletion == 0)?;
        self.data_services.sound_media.effects.raster.get(&target).copied()
    }

    /// Factory/primary Raster configuration bridge. relative is the original
    /// task+94 mode bit, constructor_word is the original task+48 DWORD.
    /// Binding this does not create resources, start or draw the effect.
    pub(super) fn bind_raster_media_context(
        &mut self, target: u32, relative: bool, constructor_word: u32,
    ) -> Result<()> {
        if !self.data_services.tasks.message_task_state(target)
            .is_some_and(|(kind, deletion, _)| kind == 18 && deletion == 0)
        {
            return Err(native_state_error("Raster context owner is not a live kind18 task"));
        }
        let live = &self.data_services.tasks;
        self.data_services.sound_media.effects.raster.retain(|&id, _| {
            live.message_task_state(id).is_some_and(|(_, deletion, _)| deletion == 0)
        });
        if self.data_services.sound_media.effects.raster.len() >= 65_536
            && !self.data_services.sound_media.effects.raster.contains_key(&target)
        {
            return Err(native_state_error("Raster parameter owner limit exceeded65536"));
        }
        let mut state = self.raster_media_parameters(target).unwrap_or(RasterParameters {
            mode: 0, period_bits: 0, amplitude_bits: 1f32.to_bits(), relative,
            constructor_word: Some(constructor_word), configured: false,
        });
        state.relative = relative;
        state.constructor_word = Some(constructor_word);
        self.data_services.sound_media.effects.raster.insert(target, state);
        Ok(())
    }

    /// Call from the real owner release path, before its task id can be reused.
    pub(super) fn release_media_effect_parameters(&mut self, target: u32) {
        self.data_services.sound_media.effects.raster.remove(&target);
    }
}

fn profile(host: &LoggingHost<'_>) -> Result<bool> {
    match host.data_services.native_profile.map(|profile| profile.pe_sha256) {
        Some(YUKIKOI) => Ok(true),
        Some(V401) => Ok(false),
        _ => Err(native_state_error("media command layout needs an identified PE profile")),
    }
}

fn rect(bytes: &[u8]) -> Result<[i32; 4]> {
    Ok([read_u32(bytes, 0)? as i32, read_u32(bytes, 4)? as i32,
        read_u32(bytes, 8)? as i32, read_u32(bytes, 12)? as i32])
}
