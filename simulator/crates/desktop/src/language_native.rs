//! Ordinary language natives and deferred configuration notifications.
//!
//! Store one instance inside DataServices, shared by every ordinary task.

use std::collections::VecDeque;

use cs2_formats::{ConfigError, ConfigXml};
use cs2_runtime::TaskCommand;
use cs2_runtime::language::{
    LanguageChangeRequest, LanguageState, LanguageTarget, MAX_LANGUAGE_CHANGE_REQUESTS,
    MAX_LANGUAGE_QUERY_PREFIX_BYTES,
};
use cs2_script::{NativeReturn, Result, ScriptError, Vm};

#[derive(Debug, Clone, Default)]
pub(super) struct LanguageNativeServices {
    pub(super) languages: LanguageState,
    pub(super) system_settings_dirty: u32,
    pub(super) platform: super::language_platform::LanguagePlatformState,
    query_story_prefixes: bool,
    language_change_requests: VecDeque<LanguageChangeRequest>,
    configuration_broadcast: Option<ConfigurationBroadcast>,
}

#[derive(Debug, Clone)]
struct ConfigurationBroadcast {
    recipients: Vec<i32>,
    next: usize,
}

impl LanguageNativeServices {
    pub(crate) fn graph_has_continuation(&self) -> bool {
        !self.language_change_requests.is_empty() || self.configuration_broadcast.is_some()
            || self.platform.graph_has_continuation()
    }
    pub(super) fn new(languages: LanguageState) -> Self {
        Self {
            languages,
            ..Self::default()
        }
    }

    pub(super) fn from_config_xml(document: &ConfigXml) -> Self {
        Self::new(LanguageState::from_config_xml(document))
    }

    pub(super) fn from_xml(bytes: &[u8]) -> std::result::Result<Self, ConfigError> {
        Ok(Self::new(LanguageState::from_xml(bytes)?))
    }

    pub(super) fn apply_user_xml(&mut self, bytes: &[u8]) -> std::result::Result<(), ConfigError> {
        self.languages.apply_user_xml(bytes)
    }

    pub(super) fn apply_user_config_xml(&mut self, document: &ConfigXml) {
        self.languages.apply_user_config_xml(document);
    }

    pub(super) fn write_xml(&self, document: &mut ConfigXml) -> Result<()> {
        self.languages.write_xml(document)
    }

    pub(super) fn supports(index: u32) -> bool {
        matches!(index, 533 | 1036 | 1053 | 1062)
    }

    pub(super) fn configure_prefix_query(&mut self, story_prefixes: bool) {
        self.query_story_prefixes = story_prefixes;
    }

    /// task_id is the sender's raw DWORD, including any signed-id bit pattern.
    /// Every successful supported call returns status 2 without a return value.
    pub(super) fn call(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
        task_id: u32,
    ) -> Result<NativeReturn> {
        match index {
            533 => self.system_settings_dirty = 1,
            1036 => {
                let first_output = argument(arguments, 0)?;
                let second_output = argument(arguments, 4)?;
                // Both starts must be valid before either output is modified.
                vm.read(first_output, 1)?;
                vm.read(second_output, 1)?;
                let [first, second] = if self.query_story_prefixes {
                    [LanguageTarget::MainText, LanguageTarget::Subtitle]
                } else {
                    [LanguageTarget::Interface, LanguageTarget::MainText]
                };
                self.write_prefix(vm, first_output, first)?;
                self.write_prefix(vm, second_output, second)?;
            }
            1053 => {
                let mode = argument(arguments, 0)?;
                let selection = argument(arguments, 4)?;
                if let Some(target) = self.languages.select_ordinary(mode, selection) {
                    self.system_settings_dirty = 1;
                    // State and dirty marker intentionally precede posting.
                    self.enqueue_language_change(LanguageChangeRequest { task_id, target })?;
                }
            }
            1062 => {
                let mode = argument(arguments, 0)?;
                if mode <= 1 {
                    let output = argument(arguments, 4)?;
                    let input = vm.read_string(output)?;
                    let mut expanded = self.languages.expand(&input, mode)?;
                    expanded.push(0);
                    // Vm::write checks the full region before copying, so a
                    // capacity error leaves the original string untouched.
                    vm.write(output, &expanded)?;
                }
            }
            _ => return Err(ScriptError::UnimplementedNativeCall { index }),
        }
        Ok(NativeReturn::Continue)
    }

    pub(super) fn pending_language_requests(&self) -> impl Iterator<Item = &LanguageChangeRequest> {
        self.language_change_requests.iter()
    }

    /// Acknowledge only after all platform work for the current head succeeds.
    pub(super) fn acknowledge_language_request(
        &mut self,
        request: LanguageChangeRequest,
    ) -> Result<()> {
        if self.language_change_requests.front() != Some(&request) {
            return Err(error(
                "language request acknowledgment does not match queue head",
            ));
        }
        if self.languages.finish_platform_refresh(request.target) {
            self.system_settings_dirty = 1;
        }
        self.language_change_requests.pop_front();
        Ok(())
    }

    /// May also receive FES requests after that scene's notification phase.
    /// This operation does not change selections or the configuration marker.
    pub(super) fn enqueue_language_change(&mut self, request: LanguageChangeRequest) -> Result<()> {
        if self.language_change_requests.len() >= MAX_LANGUAGE_CHANGE_REQUESTS {
            return Err(error("language request queue exceeds 1000000 entries"));
        }
        self.language_change_requests
            .try_reserve(1)
            .map_err(|_| error("language request queue allocation failed"))?;
        self.language_change_requests.push_back(request);
        Ok(())
    }

    /// Process pending requests at the platform boundary. The callback sees
    /// current global choices, not a snapshot from the native call.
    ///
    /// true acknowledges completion; false or Err retains the current request
    /// and its successors. Successful earlier requests are not replayed.
    /// Invalid interface requests are acknowledged without platform work.
    /// The callback owns resource/table/menu refresh, including main-text table
    /// reload even for an invalid selection and no table reads for subtitles.
    pub(super) fn consume_language_requests(
        &mut self,
        mut refresh: impl FnMut(LanguageChangeRequest, &LanguageState) -> Result<bool>,
    ) -> Result<usize> {
        let count = self.language_change_requests.len();
        let mut completed = 0;
        for _ in 0..count {
            let Some(request) = self.language_change_requests.front().copied() else {
                break;
            };
            if request.target == LanguageTarget::Interface
                && !self.languages.contains_index(self.languages.primary_index)
            {
                self.language_change_requests.pop_front();
                completed += 1;
                continue;
            }
            if !refresh(request, &self.languages)? {
                break;
            }
            if self.languages.finish_platform_refresh(request.target) {
                self.system_settings_dirty = 1;
            }
            self.language_change_requests.pop_front();
            completed += 1;
        }
        Ok(completed)
    }

    pub(super) fn has_pending_configuration_change(&self) -> bool {
        self.system_settings_dirty != 0 || self.configuration_broadcast.is_some()
    }

    /// A failed frame must resume this broadcast before advancing tasks again.
    pub(super) fn has_pending_configuration_broadcast(&self) -> bool {
        self.configuration_broadcast.is_some()
            || self.platform.adapters.has_configuration_continuation()
    }

    /// True means a successful, fully consumed broadcast. Supply recipients in
    /// tree traversal order, including the root and detached live subtrees.
    /// A suspended broadcast keeps its recipient list and cursor across calls.
    ///
    /// notify must skip a recipient that has since been deleted. It receives
    /// command 19 with sender/data zero and must not execute ordinary scripts.
    /// invalidate runs after all recipients; only then is the marker cleared,
    /// including a marker set again by either callback.
    pub(super) fn consume_configuration_change(
        &mut self,
        root_present: bool,
        recipients: &[i32],
        mut notify: impl FnMut(TaskCommand, &mut LanguageState, &mut u32) -> Result<()>,
        mut invalidate: impl FnMut(&mut u32) -> Result<()>,
    ) -> Result<bool> {
        if !root_present {
            return Ok(false);
        }
        if self.configuration_broadcast.is_none() {
            if self.system_settings_dirty == 0 || recipients.is_empty() {
                return Ok(false);
            }
            let mut pending = Vec::new();
            pending
                .try_reserve_exact(recipients.len())
                .map_err(|_| error("configuration broadcast allocation failed"))?;
            pending.extend_from_slice(recipients);
            self.configuration_broadcast = Some(ConfigurationBroadcast {
                recipients: pending,
                next: 0,
            });
        }

        while let Some(target) = self
            .configuration_broadcast
            .as_ref()
            .and_then(|pending| pending.recipients.get(pending.next).copied())
        {
            let command = TaskCommand {
                sender: 0,
                target,
                code: 19,
                data: [0; 3],
            };
            if let Err(failure) = notify(
                command,
                &mut self.languages,
                &mut self.system_settings_dirty,
            ) {
                self.system_settings_dirty = 1;
                return Err(failure);
            }
            if let Some(pending) = &mut self.configuration_broadcast {
                pending.next += 1;
            }
        }
        if let Err(failure) = invalidate(&mut self.system_settings_dirty) {
            self.system_settings_dirty = 1;
            return Err(failure);
        }
        self.system_settings_dirty = 0;
        self.configuration_broadcast = None;
        Ok(true)
    }

    fn write_prefix(&self, vm: &mut Vm, address: u32, target: LanguageTarget) -> Result<()> {
        // The query clears each destination before copying that prefix. This
        // also defines overlap order and the observable prefix-capacity error.
        vm.write(address, &[0])?;
        let languages = &self.languages;
        let prefix = match target {
            LanguageTarget::Interface => languages.query_prefix(0)?,
            LanguageTarget::MainText => languages.query_prefix(1)?,
            LanguageTarget::Subtitle => {
                let prefix = languages.prefixes.get(languages.subtitle_index as usize)
                    .or_else(|| languages.prefixes.get(languages.default_index as usize))
                    .map_or(&[][..], |prefix| super::trim_nul(prefix));
                if prefix.len() > MAX_LANGUAGE_QUERY_PREFIX_BYTES {
                    return Err(error("language query prefix exceeds 4095 bytes"));
                }
                prefix
            }
        };
        let mut output = prefix.to_vec();
        output.push(0);
        vm.write(address, &output)
    }
}

fn argument(arguments: &[u8], offset: usize) -> Result<u32> {
    let bytes = arguments
        .get(offset..offset + 4)
        .ok_or(ScriptError::Truncated { offset, needed: 4 })?;
    Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| {
        ScriptError::Truncated { offset, needed: 4 }
    })?))
}

fn error(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_script::KcsProgram;

    #[test]
    fn query_1036_uses_the_configured_two_prefix_outputs() -> Result<()> {
        let program = KcsProgram::from_code(&[0x3b], vec![0; 64], 0)?;
        let mut vm = Vm::from_program(program)?;
        let mut service = LanguageNativeServices::new(LanguageState {
            prefixes: vec![b"en".to_vec(), b"jp".to_vec(), b"cn".to_vec()],
            default_index: 0,
            primary_index: 0,
            message_index: 2,
            subtitle_index: 1,
        });
        let mut arguments = [0_u8; 8];
        arguments[..4].copy_from_slice(&4_u32.to_le_bytes());
        arguments[4..].copy_from_slice(&16_u32.to_le_bytes());

        service.configure_prefix_query(true);
        service.call(1036, &arguments, &mut vm, 0)?;
        assert_eq!(vm.read(4, 3)?, b"cn\0");
        assert_eq!(vm.read(16, 3)?, b"jp\0");

        service.languages.message_index = 1;
        service.languages.subtitle_index = u32::MAX;
        service.call(1036, &arguments, &mut vm, 0)?;
        assert_eq!(vm.read(4, 3)?, b"jp\0");
        assert_eq!(vm.read(16, 3)?, b"en\0");

        service.configure_prefix_query(false);
        service.call(1036, &arguments, &mut vm, 0)?;
        assert_eq!(vm.read(4, 3)?, b"en\0");
        assert_eq!(vm.read(16, 3)?, b"jp\0");
        assert_eq!(service.languages.selections(), [0, 1, u32::MAX]);
        Ok(())
    }
}
