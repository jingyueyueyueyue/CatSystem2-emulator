//! Ordinary1055..1061 share task-local configuration and explicit VM outputs.
use std::collections::BTreeMap;
use cs2_formats::ConfigXml;
use cs2_runtime::{ResourceLocation, RuntimeError, emocon::EmoconConfiguration};
use cs2_script::{NativeReturn, Result, ScriptError, Vm};
use super::{HeadlessPlayer, LoggingHost, expand_kcs_format, native_state_error, read_u32, trim_nul, write_kcs_terminated};

#[derive(Default)]
pub(super) struct EmoconStore {
    tasks: BTreeMap<u32, EmoconConfiguration>,
}

impl EmoconStore {
    pub(super) fn release(&mut self, id: u32) { self.tasks.remove(&id); }
    /// Only the full graph transaction calls this after validating every
    /// configuration and its owner in the replacement graph.
    pub(crate) fn install_prepared_graph(&mut self, tasks: BTreeMap<u32, EmoconConfiguration>) {
        self.tasks = tasks;
    }
}

impl HeadlessPlayer {
    pub(crate) fn emocon_state_snapshot(&self) -> BTreeMap<u32, EmoconConfiguration> {
        self.data_services.emocon.tasks.clone()
    }

    pub(crate) fn restore_emocon_state(
        &mut self, states: BTreeMap<u32, EmoconConfiguration>,
    ) -> anyhow::Result<()> {
        if states.len() > 65_536 { anyhow::bail!("emocon snapshot has too many task owners"); }
        for (&id, state) in &states {
            if id != self.current_task_id && self.data_services.tasks.message_task_state(id).is_none() {
                anyhow::bail!("emocon snapshot owner has no rebuilt task record");
            }
            state.validate()?;
        }
        self.data_services.emocon.tasks = states;
        Ok(())
    }
}

impl LoggingHost<'_> {
    pub(super) fn emocon_native(&mut self, index: u32, args: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        if index == 1055 {
            let mut state = self.data_services.emocon.tasks.remove(&self.current_task_id).unwrap_or_default();
            state.clear_tables();
            let result = (|| {
                let name: Vec<_> = "emodef.xml".encode_utf16().collect();
                let Some(document) = self.read_emocon_document(&name)? else { return Ok(()); };
                state.load_document(&document, |name| self.read_emocon_document(name))
            })();
            self.data_services.emocon.tasks.insert(self.current_task_id, state);
            result.map_err(|cause| self.task_error(self.current_task_id, cause))?;
            return Ok(NativeReturn::Continue);
        }
        if index == 1057 {
            return Ok(NativeReturn::Value(self.data_services.emocon.tasks.get(&self.current_task_id)
                .map_or(0, EmoconConfiguration::mark_count)));
        }
        let fallback = EmoconConfiguration::default();
        let state = self.data_services.emocon.tasks.get(&self.current_task_id).unwrap_or(&fallback);
        match index {
            1058 | 1059 | 1060 => {
                let (units, output) = if index == 1060 {
                    (state.stop_label(), read_u32(args, 0)?)
                } else {
                    let row = read_u32(args, 0)?;
                    (if index == 1058 { state.mark_name(row) } else { state.mark_label(row) },
                        read_u32(args, 4)?)
                };
                match vm.read(output, 1) {
                    Err(ScriptError::MemoryOutOfBounds { .. }) => return Ok(NativeReturn::Continue),
                    result => { result?; }
                }
                write_kcs_terminated(vm, output, &encode_cp932(units)?, 1)?;
            }
            1056 => {
                // Format and resolve all four addresses before any mutation.
                let raw = expand_kcs_format(vm, &vm.read_string(read_u32(args, 0)?)?, 0)?;
                let raw = trim_nul(&raw);
                if raw.len() >= 4096 { return Err(native_state_error("emocon name exceeds the original format buffer")); }
                let (name, _, invalid) = encoding_rs::SHIFT_JIS.decode(raw);
                if invalid { return Err(native_state_error("emocon input name is invalid CP932")); }
                let name: Vec<_> = name.encode_utf16().collect();
                let mut outputs = [0; 4];
                for (number, address) in outputs.iter_mut().enumerate() {
                    *address = read_u32(args, 16 + number * 4)?;
                }
                vm.read(outputs[0], 1)?;
                vm.read(outputs[1], 1)?;
                let placement = state.placement(&name, read_u32(args, 4)?,
                    [read_u32(args, 8)? as i32, read_u32(args, 12)? as i32]);
                vm.write(outputs[0], &0u32.to_le_bytes())?;
                vm.write(outputs[1], &0u32.to_le_bytes())?;
                if let Some(placement) = placement {
                    vm.write(outputs[0], &placement.position[0].to_le_bytes())?;
                    vm.write(outputs[1], &placement.position[1].to_le_bytes())?;
                    vm.write(outputs[2], &placement.scale.to_le_bytes())?;
                    write_kcs_terminated(vm, outputs[3], &encode_cp932(&placement.texture)?, 1)?;
                } else {
                    write_kcs_terminated(vm, outputs[3], b"dummy", 1)?;
                }
            }
            1061 => {
                let x = read_u32(args, 12)?;
                vm.read(x, 1)?;
                let y = read_u32(args, 16)?;
                vm.read(y, 1)?;
                let offset = state.pose_offset(read_u32(args, 0)?, read_u32(args, 4)?, read_u32(args, 8)?);
                vm.write(x, &0u32.to_le_bytes())?;
                vm.write(y, &0u32.to_le_bytes())?;
                if let Some(offset) = offset {
                    vm.write(x, &offset[0].to_le_bytes())?;
                    vm.write(y, &offset[1].to_le_bytes())?;
                }
            }
            _ => return Err(ScriptError::UnimplementedNativeCall { index }),
        }
        Ok(NativeReturn::Continue)
    }

    fn read_emocon_document(&self, name: &[u16]) -> std::result::Result<Option<ConfigXml>, RuntimeError> {
        let name = String::from_utf16(name)
            .map_err(|_| RuntimeError::Host("emocon table path contains an invalid wide unit".into()))?;
        let literal = format!("config.int/{name}");
        let names = self.resources.category_names(b"config", &name).unwrap_or_default();
        let mut candidates = Vec::new();
        // Registered loose configuration folders precede archive sources.
        for name in names.iter().chain(std::iter::once(&literal)) {
            for candidate in self.resources.candidates(name) {
                if matches!(&candidate.location, ResourceLocation::File { .. }) {
                    candidates.push(candidate);
                }
            }
        }
        if let Some(preferred) = self.resources.find(&literal) { candidates.push(preferred); }
        candidates.extend(self.resources.candidates(&literal));
        for candidate in candidates {
            if candidate.size > 64 * 1024 * 1024 {
                return Err(RuntimeError::Host("emocon XML resource exceeds 64 MiB".into()));
            }
            let bytes = match self.resources.read_candidate(candidate) {
                Err(RuntimeError::ResourceNotFound(_)) => continue,
                Err(RuntimeError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => continue,
                result => result?,
            };
            // Runtime XML rejection means a failed load, not a fabricated
            // empty successful parse; the owner already cleared its tables.
            return Ok(ConfigXml::parse_runtime(&bytes).ok());
        }
        Ok(None)
    }
}

fn encode_cp932(units: &[u16]) -> Result<Vec<u8>> {
    let text = String::from_utf16(units)
        .map_err(|_| native_state_error("emocon output contains an invalid wide character"))?;
    let (bytes, _, invalid) = encoding_rs::SHIFT_JIS.encode(&text);
    if invalid { return Err(native_state_error("emocon output is not representable in CP932")); }
    Ok(bytes.into_owned())
}
