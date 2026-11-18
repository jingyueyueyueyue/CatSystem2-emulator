//! Application save schema for task-owned numeric areas. Aliased tasks share
//! one area and one 609/610 backup; neither is reconstructed per task.
use std::collections::{BTreeMap, BTreeSet};
use serde::{Deserialize, Serialize};
use crate::{Result, ScriptError};

const UNBOUND_AREA: u64 = 1 << 63;
const MAX_VALUES: usize = 1_000_000;
const MAX_BYTES: usize = 256 * 1024 * 1024;

fn invalid(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedNumericStateSnapshot {
    pub version: u32,
    pub next_identity: u64,
    pub active_identity: u64,
    pub initialized: BTreeSet<u64>,
    pub active_values: Vec<u32>,
    pub inactive_values: BTreeMap<u64, Vec<u32>>,
    pub backups: BTreeMap<u64, Vec<u8>>,
    pub task_bindings: BTreeMap<u32, Option<u64>>,
    #[serde(default)]
    pub local_regions: BTreeMap<u64, BTreeMap<u32, Vec<u32>>>,
}

impl SharedNumericStateSnapshot {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 || self.next_identity >= UNBOUND_AREA
            || self.initialized.len() > MAX_VALUES || self.task_bindings.len() > MAX_VALUES
            || self.inactive_values.len() > MAX_VALUES
            || self.backups.keys().copied().collect::<BTreeSet<_>>() != self.initialized
            || self.inactive_values.contains_key(&self.active_identity)
        {
            return Err(invalid("shared numeric snapshot version or identity set is invalid"));
        }
        let valid_storage_identity = |identity: u64| {
            if identity < UNBOUND_AREA {
                identity == 0 || self.initialized.contains(&identity)
            } else {
                let task = identity & !UNBOUND_AREA;
                task <= u64::from(u32::MAX)
                    && self.task_bindings.get(&(task as u32)) == Some(&None)
            }
        };
        if !valid_storage_identity(self.active_identity)
            || self.inactive_values.keys().any(|&identity| !valid_storage_identity(identity))
            || self.task_bindings.values().flatten().any(|identity| !self.initialized.contains(identity))
            || self.initialized.iter().any(|&identity| identity >= UNBOUND_AREA || identity > self.next_identity)
        {
            return Err(invalid("shared numeric snapshot has an invalid task binding or area identity"));
        }
        let mut total = 0usize;
        for values in std::iter::once(&self.active_values).chain(self.inactive_values.values()) {
            if values.len() > MAX_VALUES { return Err(invalid("numeric area exceeds one million slots")); }
            total = total.checked_add(values.len().checked_mul(4)
                .ok_or_else(|| invalid("numeric area byte length overflows"))?)
                .filter(|&total| total <= MAX_BYTES)
                .ok_or_else(|| invalid("numeric area snapshots exceed 256 MiB"))?;
        }
        for &identity in &self.initialized {
            let values = if identity == self.active_identity { &self.active_values }
                else { self.inactive_values.get(&identity)
                    .ok_or_else(|| invalid("numeric snapshot omits a live area storage"))? };
            let backup = &self.backups[&identity];
            if backup.len() != values.len() {
                return Err(invalid("numeric snapshot backup byte length differs from its slot count"));
            }
            total = total.checked_add(backup.len()).filter(|&total| total <= MAX_BYTES)
                .ok_or_else(|| invalid("numeric area snapshots exceed 256 MiB"))?;
        }
        let mut local_count = 0usize;
        for (&area, regions) in &self.local_regions {
            if !self.initialized.contains(&area) {
                return Err(invalid("local numeric snapshot references an unknown shared area"));
            }
            for (&task, values) in regions {
                local_count += 1;
                if local_count > MAX_VALUES || values.len() != 512
                    || self.task_bindings.get(&task) != Some(&Some(area))
                {
                    return Err(invalid("local numeric snapshot has an invalid task binding or slot count"));
                }
                total = total.checked_add(2048).filter(|&total| total <= MAX_BYTES)
                    .ok_or_else(|| invalid("numeric area snapshots exceed 256 MiB"))?;
            }
        }
        Ok(())
    }
}
