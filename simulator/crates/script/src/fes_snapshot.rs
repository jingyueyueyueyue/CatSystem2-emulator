//! Validation for full FES execution checkpoints. No instruction is stepped.
use super::*;

impl FesInterpreter {
    pub fn validate_execution_checkpoint(&self) -> Result<()> {
        let invalid = || scene_error(0, "invalid FES execution checkpoint");
        if self.local.len() != 512 || self.calls.len() > MAX_FES_CALL_DEPTH
            || self.program.blocks.len() > MAX_FES_BLOCKS
            || !self.program.diagnostics.is_empty()
            || !self.initialization.subcommands.diagnostics.is_empty()
            || self.initialization_cursor.keyblock > self.initialization.keyblocks.len()
            || self.initialization_cursor.definition > self.initialization.definitions.len()
            || self.initialization_cursor.initializer > self.initialization.initializers.len()
            || self.initialization_cursor.key_binding > self.initialization.key_bindings.len()
            || self.named.len() > MAX_SCRIPT_ITEMS
        { return Err(invalid()); }
        let mut lines = 0usize;
        for block in &self.program.blocks {
            lines = lines.checked_add(block.lines.len()).filter(|&n| n <= MAX_FES_LINES)
                .ok_or_else(invalid)?;
            if block.false_jump.len() != block.lines.len() || block.end_jump.len() != block.lines.len()
                || block.false_jump.iter().chain(&block.end_jump).flatten()
                    .any(|&target| target > block.lines.len())
            { return Err(invalid()); }
        }
        for (block, pc) in std::iter::once((self.block, self.pc))
            .chain(self.calls.iter().map(|frame| (frame.block, frame.pc)))
        {
            if self.program.blocks.get(block).is_none_or(|block| pc > block.lines.len()) {
                return Err(invalid());
            }
        }
        if self.named.values().any(|values| values.len() > MAX_SCRIPT_ITEMS) {
            return Err(invalid());
        }
        if let Some(event) = &self.pending { event.validate_execution_checkpoint()?; }
        Ok(())
    }
}

impl FesControlPosition {
    pub fn validate_execution_checkpoint(&self, interpreter: &FesInterpreter) -> Result<()> {
        if interpreter.program.blocks.get(self.block)
            .is_none_or(|block| self.position > block.lines.len())
        { return Err(scene_error(self.position, "invalid parked FES checkpoint position")); }
        if let Some(event) = &self.pending { event.validate_execution_checkpoint()?; }
        Ok(())
    }
}

impl FesEvent {
    pub fn validate_execution_checkpoint(&self) -> Result<()> {
        // Diagnostics deliberately have no wire representation. Never let
        // serialization turn a rejected declaration into a clean event.
        if matches!(self, Self::Subcommands(table) if !table.diagnostics.is_empty()) {
            return Err(scene_error(0, "cannot checkpoint an invalid FES subcommand event"));
        }
        Ok(())
    }
}
