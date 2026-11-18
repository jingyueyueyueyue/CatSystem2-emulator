//! Kind13/228: owned two-string configuration for a real speech provider.
//! The original OpenJTalk DLL is neither loaded nor called.
use cs2_formats::ConfigXml;
use cs2_runtime::{ResourceIndex, RuntimeError, RuntimeResult};
use cs2_script::{Result, Vm};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{HeadlessPlayer, LoggingHost, audio_script_error, read_u32, trim_nul};

pub(super) const MAX_RECORD: usize = 4 + 260 * 2;

/// Text is decoded strictly using the player's declared script encoding.
/// Names preserve message228 order. The original DLL's +8 call reverses
/// that order; the identities of its two configuration assets are not
/// inferred from their pointer values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechConfiguration {
    pub native_first: String,
    pub native_second: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeechPcm {
    pub sample_rate: u32,
    pub channels: u8,
    pub samples: Arc<[i16]>,
}

/// Actual TTS/speed and TTS/tone XML floats. PE's UI uses 0.1 speed steps
/// and semitone offsets; it converts these stored f32 values to double at
/// the synthesis call. These are independent of physical channel gain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeechSynthesisParameters {
    pub speed: f32,
    pub tone: f32,
}

impl Default for SpeechSynthesisParameters {
    fn default() -> Self { Self { speed: 1., tone: 0. } }
}

impl SpeechSynthesisParameters {
    pub(super) fn validate(self) -> RuntimeResult<Self> {
        if !self.speed.is_finite() || self.speed <= 0. || !self.tone.is_finite() {
            return Err(RuntimeError::Host("speech speed must be positive and speed/tone finite".into()));
        }
        Ok(self)
    }
}

/// Implemented by the application-owned Windows/Android speech service.
/// Return success only after applying both strings to a working provider.
/// Keeping state in this service is not provider success.
pub trait SpeechConfigurationHost: Send {
    fn configure(&mut self, configuration: &SpeechConfiguration) -> RuntimeResult<()>;
    fn synthesize(&mut self, _text: &str) -> RuntimeResult<SpeechPcm> {
        Err(RuntimeError::Host("this speech provider cannot produce actual PCM".into()))
    }
    fn synthesize_with_parameters(
        &mut self, text: &str, parameters: SpeechSynthesisParameters,
    ) -> RuntimeResult<SpeechPcm> {
        parameters.validate()?;
        if parameters != SpeechSynthesisParameters::default() {
            return Err(RuntimeError::Host("this speech provider cannot apply TTS speed/tone".into()));
        }
        self.synthesize(text)
    }
}

#[derive(Default)]
pub(super) struct SpeechConfigurationService {
    host: Option<Box<dyn SpeechConfigurationHost>>,
    applied: Option<SpeechConfiguration>,
    data_root: Option<PathBuf>,
    parameters: SpeechSynthesisParameters,
    parameters_explicit: bool,
    voice_table: Option<String>,
    voice_id: String,
    resolved_voice_id: Option<String>,
    selection_dirty: bool,
    explicit_configuration: bool,
}

impl SpeechConfigurationService {
    pub(crate) fn check_audio_checkpoint_boundary(&self) -> RuntimeResult<()> {
        if self.host.is_some() || self.applied.is_some() || self.data_root.is_some()
            || self.parameters != SpeechSynthesisParameters::default() || self.parameters_explicit
            || self.voice_table.is_some() || !self.voice_id.is_empty() || self.resolved_voice_id.is_some()
            || self.selection_dirty || self.explicit_configuration {
            return Err(RuntimeError::Host("speech provider/configuration checkpoints need their actual provider adapter".into()));
        }
        Ok(())
    }

    fn builtin_data_root(&self, save_root: &Path) -> PathBuf {
        if let Some(root) = &self.data_root { return root.clone(); }
        if let Some(root) = std::env::var_os("CS2_SPEECH_DATA") { return root.into(); }
        let private = save_root.join("speech-data");
        if private.is_dir() { return private; }
        // An independently distributed Windows host may carry its fixed
        // data beside cs2_speech.dll. Never search a game EXE/DLL or PATH.
        #[cfg(windows)]
        if let Ok(executable) = std::env::current_exe() {
            let packaged = executable.with_file_name("speech-data");
            if packaged.is_dir() { return packaged; }
        }
        private // Preserve a concrete missing-asset diagnostic from configure.
    }

    fn selected_configuration(
        &self, resources: &ResourceIndex,
    ) -> RuntimeResult<(SpeechConfiguration, Option<String>)> {
        if self.explicit_configuration {
            Ok((self.applied.clone().expect("explicit speech configuration was accepted"), None))
        } else if let Some(filename) = &self.voice_table {
            let selection = super::speech_table::load(resources, filename, &self.voice_id)?;
            Ok((selection.configuration, Some(selection.id)))
        } else {
            Ok((SpeechConfiguration { native_first: String::new(), native_second: String::new() }, None))
        }
    }

    fn accept_selection(&mut self, resolved_id: Option<String>) {
        if let Some(id) = &resolved_id {
            // PE0x26e903 writes a valid default row back into the shared
            // TTS/id field. Subsequent lookups use the accepted row identity.
            self.voice_id.clone_from(id);
        }
        self.resolved_voice_id = resolved_id;
        self.selection_dirty = false;
    }

    fn configure(
        &mut self,
        configuration: SpeechConfiguration,
        game_root: &Path,
        save_root: &Path,
    ) -> RuntimeResult<()> {
        if let Some(host) = self.host.as_mut() {
            host.configure(&configuration)?;
        } else {
            let data_root = self.builtin_data_root(save_root);
            let mut provider = super::speech_openjtalk::OpenJtalkProvider::new(
                game_root.to_owned(), data_root,
            );
            // A failed load must not publish an uninitialized engine. Both
            // the accepted configuration and a new host commit on success.
            provider.configure(&configuration)?;
            self.host = Some(Box::new(provider));
        }
        self.applied = Some(configuration);
        Ok(())
    }

    fn ensure_ready(&mut self, resources: &ResourceIndex, save_root: &Path) -> RuntimeResult<()> {
        if self.host.is_some() && self.applied.is_some()
            && (self.explicit_configuration || !self.selection_dirty)
        {
            return Ok(());
        }
        // Host policy: a script need not issue228 to use independently
        // installed default assets when no table is requested. Explicit228
        // is always reapplied verbatim; a configured table must actually
        // resolve its selected dictionary/model and never falls back silently.
        let (configuration, resolved_id) = self.selected_configuration(resources)?;
        self.configure(configuration, resources.root(), save_root)?;
        self.accept_selection(resolved_id);
        Ok(())
    }

    fn synthesize(
        &mut self, resources: &ResourceIndex, save_root: &Path, text: &str,
    ) -> RuntimeResult<SpeechPcm> {
        self.ensure_ready(resources, save_root)?;
        self.host.as_mut().expect("speech initialization installed a provider")
            .synthesize_with_parameters(text, self.parameters)
    }
}

impl HeadlessPlayer {
    /// Install a real provider before enabling OpenJTalk-backed commands.
    /// Replacing it first loads the selected configuration; failure retains
    /// the previous provider and accepted configuration.
    pub fn set_speech_configuration_host(
        &mut self,
        mut provider: Option<Box<dyn SpeechConfigurationHost>>,
    ) -> RuntimeResult<()> {
        let service = &mut self.data_services.sounds.speech;
        if let Some(provider) = provider.as_mut() {
            let (configuration, resolved_id) = service.selected_configuration(&self.scan.resources)?;
            provider.configure(&configuration)?;
            service.applied = Some(configuration);
            service.accept_selection(resolved_id);
        }
        service.host = provider;
        self.synchronize_speech_configuration();
        Ok(())
    }

    pub fn speech_configuration(&self) -> Option<&SpeechConfiguration> {
        self.data_services.sounds.speech.applied.as_ref()
    }

    /// Actual successfully loaded table id, including the default-row
    /// fallback. Platform settings can reflect this back into TTS/id. An
    /// independent default or explicit228 has no table row identity.
    pub fn resolved_speech_voice_id(&self) -> Option<&str> {
        self.data_services.sounds.speech.resolved_voice_id.as_deref()
    }

    pub fn speech_synthesis_parameters(&self) -> SpeechSynthesisParameters {
        self.data_services.sounds.speech.parameters
    }

    /// Applies to subsequent generated sources. Already loaded or playing PCM
    /// retains its sample identity and cursor, as in the original loader.
    pub fn set_speech_synthesis_parameters(
        &mut self, parameters: SpeechSynthesisParameters,
    ) -> RuntimeResult<()> {
        self.data_services.sounds.speech.parameters = parameters.validate()?;
        self.data_services.sounds.speech.parameters_explicit = true;
        self.synchronize_speech_configuration();
        Ok(())
    }

    /// App-private extracted fixed dictionary/voice assets. Explicit228
    /// game paths still take precedence and must exist; no silent fallback
    /// replaces a requested model. Returns only after the new provider has
    /// actually loaded its assets; failure retains the prior provider/root.
    /// This does not synthesize or play speech.
    pub fn set_builtin_speech_data_root(&mut self, path: impl AsRef<Path>) -> RuntimeResult<()> {
        let root = path.as_ref().to_owned();
        let mut provider = super::speech_openjtalk::OpenJtalkProvider::new(
            self.scan.resources.root().to_owned(), root.clone(),
        );
        let service = &mut self.data_services.sounds.speech;
        let (configuration, resolved_id) = service.selected_configuration(&self.scan.resources)?;
        provider.configure(&configuration)?;
        service.host = Some(Box::new(provider));
        service.data_root = Some(root);
        service.applied = Some(configuration);
        service.accept_selection(resolved_id);
        self.initialize_speech()
    }

    /// Load the independent speech engine and its installed dictionary and
    /// voice, without synthesizing or playing. This may be called after the
    /// platform has extracted its assets. Synthesis also calls it lazily.
    /// Missing assets/library are errors and may be retried after installing
    /// them; a successful explicit228 configuration takes precedence.
    pub fn initialize_speech(&mut self) -> RuntimeResult<()> {
        self.data_services.sounds.speech.ensure_ready(
            &self.scan.resources, &self.save_root,
        )?;
        self.synchronize_speech_configuration();
        Ok(())
    }

    /// Startup/configuration/lifecycle callers share the live use_jtalk
    /// switch. Disabled speech does not claim that its assets are ready.
    pub(super) fn initialize_enabled_speech(&mut self) -> RuntimeResult<()> {
        if self.data_services.legacy_sound.jtalk {
            self.initialize_speech()?;
        } else {
            self.synchronize_speech_configuration();
        }
        Ok(())
    }

    fn synchronize_speech_configuration(&mut self) {
        let speech = &self.data_services.sounds.speech;
        self.data_services.configuration.synchronize_speech(
            &speech.voice_id, speech.parameters,
        );
    }

    /// Useful to platform callers which need owned PCM; playback remains in
    /// the existing WinMM/AAudio host, so lifecycle pause/volume are shared.
    pub fn synthesize_speech(&mut self, text: &str) -> RuntimeResult<SpeechPcm> {
        self.initialize_speech()?;
        self.data_services.sounds.speech.synthesize(
            &self.scan.resources, &self.save_root, text,
        )
    }
}

impl LoggingHost<'_> {
    pub(super) fn synchronize_sound_speech_configuration(&mut self) {
        let speech = &self.data_services.sounds.speech;
        self.data_services.configuration.synchronize_speech(&speech.voice_id, speech.parameters);
    }

    pub(super) fn initialize_sound_speech(&mut self) -> Result<()> {
        let result = self.data_services.sounds.speech.ensure_ready(self.resources, self.save_root);
        result.map_err(|cause| self.sound_host_error(cause))?;
        self.synchronize_sound_speech_configuration();
        Ok(())
    }

    pub(super) fn write_sound_speech_configuration(
        &self, document: &mut cs2_runtime::configuration::ConfigurationDocument,
    ) -> RuntimeResult<()> {
        let speech = &self.data_services.sounds.speech;
        document.write(&[b"TTS", b"id"], speech.voice_id.as_bytes())?;
        document.float(&[b"TTS", b"speed"], speech.parameters.speed)?;
        document.float(&[b"TTS", b"tone"], speech.parameters.tone)
    }

    pub(super) fn configure_sound_speech(&mut self, first: &[u8], second: &[u8]) -> Result<()> {
        let first = bounded(first)?;
        let second = bounded(second)?;
        let configuration = SpeechConfiguration {
            native_first: super::sound_native::decode_name(self.text_encoding, first)?,
            native_second: super::sound_native::decode_name(self.text_encoding, second)?,
        };
        let result = self.data_services.sounds.speech.configure(
            configuration, self.resources.root(), self.save_root,
        );
        if result.is_ok() {
            self.data_services.sounds.speech.explicit_configuration = true;
            self.data_services.sounds.speech.selection_dirty = false;
            self.data_services.sounds.speech.resolved_voice_id = None;
        }
        result.map_err(|cause| self.sound_host_error(cause))
    }

    pub(super) fn synthesize_sound_speech(&mut self, text: &[u8]) -> Result<SpeechPcm> {
        let text = super::sound_native::decode_name(self.text_encoding, trim_nul(text))?;
        self.initialize_sound_speech()?;
        let result = self.data_services.sounds.speech.synthesize(
            self.resources, self.save_root, &text,
        );
        result.map_err(|cause| self.sound_host_error(cause))
    }

    /// Shared initial-config/user-document hook. Missing keys keep the prior
    /// value; both keys validate before committing either one.
    pub(super) fn apply_sound_speech_parameters_xml(&mut self, xml: &ConfigXml) -> Result<()> {
        let table = super::speech_table::xml_string(xml, "document/APP/ttstable", 260)
            .map_err(|cause| self.sound_host_error(cause))?;
        let selected_id = super::speech_table::xml_string(xml, "document/TTS/id", 64)
            .map_err(|cause| self.sound_host_error(cause))?;
        let mut parameters = self.data_services.sounds.speech.parameters;
        for (key, destination) in [
            ("document/TTS/speed", &mut parameters.speed),
            ("document/TTS/tone", &mut parameters.tone),
        ] {
            if !self.data_services.sounds.speech.parameters_explicit
                && let Some(bytes) = super::xml_text(xml, key)
            {
                *destination = std::str::from_utf8(trim_nul(bytes))
                    .ok().and_then(|text| text.trim().parse::<f32>().ok())
                    .ok_or_else(|| audio_script_error("TTS speed/tone XML value is not a float"))?;
            }
        }
        self.data_services.sounds.speech.parameters = parameters.validate()
            .map_err(|cause| self.sound_host_error(cause))?;
        let service = &mut self.data_services.sounds.speech;
        if let Some(table) = table {
            let table = (!table.is_empty()).then_some(table);
            if service.voice_table != table {
                service.voice_table = table;
                service.selection_dirty = true;
            }
        }
        if let Some(id) = selected_id {
            if service.voice_id != id {
                service.voice_id = id;
                service.selection_dirty = true;
            }
        }
        Ok(())
    }

    /// Explicit temporary-buffer ABI: length(first includingNUL), first,
    /// second includingNUL. command.data[0/1] are presence markers here;
    /// ordinary69/70 snapshot the actual caller-VM strings into this record.
    pub(super) fn receive_sound_speech_configuration(&mut self, record: &[u8]) -> Result<()> {
        if record.len() > MAX_RECORD {
            return Err(audio_script_error("sound228 configuration record exceeds524 bytes"));
        }
        let first_len = read_u32(record, 0)? as usize;
        if !(1..=260).contains(&first_len) {
            return Err(audio_script_error("sound228 first string length is invalid"));
        }
        let first = record.get(4..4 + first_len)
            .ok_or_else(|| audio_script_error("sound228 first string is truncated"))?;
        let second = record.get(4 + first_len..)
            .filter(|bytes| !bytes.is_empty() && bytes.len() <= 260)
            .ok_or_else(|| audio_script_error("sound228 second string is truncated"))?;
        if first.last() != Some(&0) || second.last() != Some(&0)
            || trim_nul(first).len() + 1 != first.len()
            || trim_nul(second).len() + 1 != second.len()
        {
            return Err(audio_script_error("sound228 strings require exactNUL termination"));
        }
        self.configure_sound_speech(&first[..first.len() - 1], &second[..second.len() - 1])
    }
}

pub(super) fn snapshot_configuration(vm: &Vm, first: u32, second: u32) -> Result<Vec<u8>> {
    let first = read_c_string(vm, first)?;
    let second = read_c_string(vm, second)?;
    let mut record = Vec::with_capacity(4 + first.len() + second.len());
    record.extend_from_slice(&(first.len() as u32).to_le_bytes());
    record.extend_from_slice(&first);
    record.extend_from_slice(&second);
    Ok(record)
}

fn read_c_string(vm: &Vm, address: u32) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for offset in 0..260u32 {
        let address = address.checked_add(offset)
            .ok_or_else(|| audio_script_error("sound228 VM string address overflow"))?;
        let byte = vm.read(address, 1)?[0];
        bytes.push(byte);
        if byte == 0 {
            return Ok(bytes);
        }
    }
    Err(audio_script_error("sound228 string exceeds259 encoded bytes"))
}

fn bounded(bytes: &[u8]) -> Result<&[u8]> {
    let bytes = trim_nul(bytes);
    if bytes.len() >= 260 {
        return Err(audio_script_error("sound228 string exceeds259 encoded bytes"));
    }
    Ok(bytes)
}
