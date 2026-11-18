//! The actually empty NEKO wrapper+100 program tables, not action+180.
//!
//! 174398 binds secondary profile+88. 198730/1a4980 can populate wave nodes
//! only from %time/%set data; 1983c2 iterates the live node map, not labels.
//! Keep populated programs explicit until that producer/evaluator is recovered.

use super::super::{CpuM2Config, EmoteError, Result};

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::emote) enum CpuM2ProgramInput {
    ///A hand-constructed controller config is not evidence of empty input.
    #[default]
    Unbound,
    NoWaveDirectives,
    WaveDirectives,
}

impl CpuM2ProgramInput {
    pub(in crate::emote) fn from_expanded<'a>(configs: impl IntoIterator<Item = &'a CpuM2Config>) -> Self {
        for config in configs {
            for row in config.sections.values().flatten() {
                if row.first().is_some_and(|word| matches!(word.as_slice(), b"%time" | b"%set")) {
                    return Self::WaveDirectives;
                }
            }
        }
        Self::NoWaveDirectives
    }
}

///1981f0 constructs empty current/previous/compiled maps and enable+20=0.
///Every successful selection below keeps those maps empty. Therefore1983a0
///has no variable writes to submit, regardless of the requested enable bit.
///This owner must never be used to acknowledge a populated program.
#[derive(Debug, Clone, Default)]
pub(in crate::emote) struct CpuM2ProgramOwner {
    enabled: bool,
}

impl CpuM2ProgramOwner {
    pub(super) fn enabled(&self) -> bool { self.enabled }

    pub(super) fn select_record80(&mut self, record: &[u8], input: CpuM2ProgramInput) -> Result<()> {
        if record.len() != 80 {
            return Err(EmoteError::InvalidResource("M2 program requires an owned 80-byte record"));
        }
        let end = record[..72].iter().position(|&byte| byte == 0)
            .ok_or(EmoteError::InvalidResource("M2 program record label lacks prefix terminator"))?;
        //239197/175250: empty string only writes enable=0; it does not
        //select another program, clear tables, reset a clock or restore vars.
        if end == 0 {
            self.enabled = false;
            return Ok(());
        }
        match input {
            CpuM2ProgramInput::NoWaveDirectives => {
                //Even a matching #label cannot create wave nodes without
                //the %time/%set producer. 2391dd still writes enable=1.
                //+72/+76 are never forwarded by2391ac to175230/198730.
                self.enabled = true;
                Ok(())
            }
            CpuM2ProgramInput::Unbound => Err(EmoteError::Unsupported {
                native: 965, reason: "M2 program selection requires the actual secondary configuration",
            }),
            CpuM2ProgramInput::WaveDirectives => Err(EmoteError::Unsupported {
                native: 965, reason: "M2 %time/%set program nodes and waveform evaluator are not recovered",
            }),
        }
    }
}

impl super::CpuM2LiveOwner {
    ///965/547 owns the string record but ignores its two trailing DWORDs.
    ///The current secondary config is used even after retained-profile reuse.
    pub(in crate::emote) fn select_program_record80(&mut self, record: &[u8]) -> Result<()> {
        self.program.select_record80(record, self.secondary_config.program_input)
    }

    ///The wrapper+100 lifetime is shared with action+180 across PSB reloads.
    ///Call while preparing a replacement owner, before the CPU publishes it.
    pub(in crate::emote) fn inherit_program_owner(&mut self, previous: &Self) {
        self.program = previous.program.clone();
    }
}
