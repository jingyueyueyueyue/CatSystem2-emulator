//! Native resource state which must travel with the corresponding object
//! identities and pixel data during a complete host graph restore.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{RuntimeError, RuntimeResult};

/// The monotonic pointer-identity counter travels with the complete object
/// graph. Task numbers and VM memory-region addresses are separate domains.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectAllocatorSnapshot {
    pub version: u32,
    pub next_handle: u32,
}

/// Validated counter; preparing it does not allocate or publish an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedObjectAllocator {
    next_handle: u32,
}

impl PreparedObjectAllocator {
    pub fn next_handle(self) -> u32 { self.next_handle }
}

impl ObjectAllocatorSnapshot {
    /// Pass each allocated pointer identity exactly once, including every VM
    /// native_context_identity. Shared image pixels appear once per underlying
    /// allocation, rather than once per alias. RAW reserves [base,end), with
    /// the original aligned tail included. Its base is not also an object ID.
    /// The caller must contribute all graph components before any new object
    /// is allocated; this helper cannot infer omitted domains from task IDs.
    pub fn prepare_restore(
        self,
        object_ids: impl IntoIterator<Item = u32>,
        raw_ranges: impl IntoIterator<Item = (u32, u32)>,
    ) -> RuntimeResult<PreparedObjectAllocator> {
        if self.version != 1 || self.next_handle == 0 {
            return Err(invalid("object allocator snapshot version or counter is invalid"));
        }
        let mut identities = BTreeSet::new();
        let mut next = self.next_handle;
        for identity in object_ids {
            if identity == 0 || identity == u32::MAX || identities.len() >= 1_000_000
                || !identities.insert(identity)
            {
                return Err(invalid("object graph has an invalid or repeated pointer identity"));
            }
            next = next.max(identity + 1);
        }
        let mut ranges = BTreeMap::new();
        for (base, end) in raw_ranges {
            if base == 0 || base & 3 != 0 || end & 3 != 0 || end <= base
                || identities.len() + ranges.len() >= 1_000_000
                || ranges.insert(base, end).is_some()
            {
                return Err(invalid("object graph has an invalid or repeated RAW reservation"));
            }
        }
        let mut previous_end = 0;
        for (&base, &end) in &ranges {
            if base < previous_end || identities.range(base..end).next().is_some() {
                return Err(invalid("object pointer identity or RAW reservation overlaps another allocation"));
            }
            previous_end = end;
            next = next.max(end);
        }
        if next == u32::MAX {
            return Err(invalid("object identity counter has no allocatable non-sentinel identity"));
        }
        Ok(PreparedObjectAllocator { next_handle: next })
    }
}

/// This flag belongs to the underlying pixel allocation. Restoring wrapper
/// IDs alone or rerunning the export conversion would lose visible changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageExportStateSnapshot {
    pub version: u32,
    pub rgb_pixels: BTreeSet<u32>,
}

impl ImageExportStateSnapshot {
    /// The supplied maps describe the rebuilt image group, not the old live
    /// host. Pixel bytes and wrapper aliases are restored in that same group.
    pub fn validate(
        &self,
        wrapper_pixels: &BTreeMap<u32, u32>,
        live_wrappers: &BTreeSet<u32>,
    ) -> RuntimeResult<()> {
        if self.version != 1 || self.rgb_pixels.len() > 1_000_000
            || wrapper_pixels.len() > 1_000_000 || live_wrappers.len() > 1_000_000
        {
            return Err(invalid("image export snapshot version or item count is invalid"));
        }
        if self.rgb_pixels.contains(&0) || live_wrappers.contains(&0) {
            return Err(invalid("image export snapshot has a zero image identity"));
        }
        if !wrapper_pixels.keys().eq(live_wrappers.iter()) {
            return Err(invalid("image aliases do not cover exactly the live image wrappers"));
        }
        let mut live_pixels = BTreeSet::new();
        for (&wrapper, &pixel) in wrapper_pixels {
            if wrapper == 0 || pixel == 0 || !live_wrappers.contains(&wrapper) {
                return Err(invalid("image alias refers to an absent wrapper or zero identity"));
            }
            if live_wrappers.contains(&pixel) {
                return Err(invalid("underlying pixel identity conflicts with an image wrapper identity"));
            }
            live_pixels.insert(pixel);
        }
        if !self.rgb_pixels.is_subset(&live_pixels) {
            return Err(invalid("image export flag refers to pixels without a live wrapper alias"));
        }
        Ok(())
    }
}

fn invalid(message: &str) -> RuntimeError { RuntimeError::Host(message.into()) }
