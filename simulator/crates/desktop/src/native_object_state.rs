//! Image export flags are committed with restored image bytes and alias maps.
//! Preparing this state never converts, swaps or reloads pixel resources.
use std::collections::{BTreeMap, BTreeSet};

use cs2_renderer::RgbaFrame;
use cs2_runtime::object_state::{
    ImageExportStateSnapshot, ObjectAllocatorSnapshot, PreparedObjectAllocator,
};
use cs2_script::{Result, VmSnapshotGroup};

use crate::{HeadlessPlayer, native_state_error};
use crate::data_services::DataServices;

pub(crate) struct PreparedImageExportState(BTreeSet<u32>);

impl HeadlessPlayer {
    pub fn object_allocator_snapshot(&self) -> ObjectAllocatorSnapshot {
        ObjectAllocatorSnapshot { version: 1, next_handle: self.next_xml_handle }
    }

    /// Other pointer identities must include every non-VM object allocation
    /// in the prepared graph. Task numbers are not pointer identities. The
    /// caller validates VM execution and commits the entire graph at one
    /// frozen boundary. Shared underlying pixel identities are supplied once.
    pub(crate) fn prepare_object_allocator_restore(
        saved: ObjectAllocatorSnapshot,
        programs: &VmSnapshotGroup,
        other_pointer_identities: impl IntoIterator<Item = u32>,
        raw_blocks: &[cs2_script::data_snapshot::RawBlockSnapshot],
    ) -> Result<PreparedObjectAllocator> {
        let mut raw_bytes = 0usize;
        if raw_blocks.len() > 1_000_000 {
            return Err(native_state_error("saved object allocator has too many RAW reservations"));
        }
        for block in raw_blocks {
            let span = u32::try_from(block.bytes.len()).ok()
                .and_then(|length| length.checked_add(4)).map(|length| length & !3);
            if block.bytes.len() > 64 * 1024 * 1024
                || span.and_then(|span| block.base.checked_add(span)) != Some(block.reserved_end)
            {
                return Err(native_state_error("saved RAW bytes differ from the reserved identity range"));
            }
            raw_bytes = raw_bytes.checked_add(block.bytes.len())
                .filter(|&length| length <= 256 * 1024 * 1024)
                .ok_or_else(|| native_state_error("saved RAW reservations exceed 256 MiB"))?;
        }
        let vm_identities = programs.programs.values()
            .filter_map(cs2_script::VmExecutionSnapshot::native_context_identity);
        saved.prepare_restore(
            vm_identities.chain(other_pointer_identities),
            raw_blocks.iter().map(|block| (block.base, block.reserved_end)),
        ).map_err(|_| native_state_error("saved object allocator has conflicting or invalid graph identities"))
    }

    /// Commit before any graph component can allocate a new pointer identity.
    pub(crate) fn commit_object_allocator_restore(&mut self, prepared: PreparedObjectAllocator) {
        self.next_xml_handle = prepared.next_handle();
    }

    pub fn image_export_state_snapshot(&self) -> Result<ImageExportStateSnapshot> {
        let saved = ImageExportStateSnapshot {
            version: 1,
            rgb_pixels: self.data_services.image_export.rgb_pixels.clone(),
        };
        validate_image_group(
            &saved,
            &self.data_services.image_wrapper_pixels,
            &self.image_handles,
        )?;
        Ok(saved)
    }

    /// Call before live mutation, using the same rebuilt alias map and image
    /// frames which the complete graph transaction will actually install.
    /// Equal pixel identities must have equal bytes and complete metadata.
    pub(crate) fn prepare_image_export_state_restore(
        saved: &ImageExportStateSnapshot,
        rebuilt_wrapper_pixels: &BTreeMap<u32, u32>,
        rebuilt_image_wrappers: &BTreeMap<u32, RgbaFrame>,
    ) -> Result<PreparedImageExportState> {
        validate_image_group(saved, rebuilt_wrapper_pixels, rebuilt_image_wrappers)?;
        Ok(PreparedImageExportState(saved.rgb_pixels.clone()))
    }

    /// The complete graph loader commits the prepared image group and these
    /// flags together, after all graph components have passed validation.
    pub(crate) fn commit_image_export_state_restore(&mut self, saved: PreparedImageExportState) {
        self.data_services.image_export.rgb_pixels = saved.0;
    }
}

fn validate_image_group(
    saved: &ImageExportStateSnapshot,
    wrapper_pixels: &BTreeMap<u32, u32>,
    frames: &BTreeMap<u32, RgbaFrame>,
) -> Result<()> {
    let wrappers = frames.keys().copied().collect();
    saved.validate(wrapper_pixels, &wrappers)
        .map_err(|_| native_state_error("image export flags or aliases do not match the complete image group"))?;
    let mut shared_frames: BTreeMap<u32, &RgbaFrame> = BTreeMap::new();
    for (&wrapper, &pixel) in wrapper_pixels {
        let frame = frames.get(&wrapper)
            .ok_or_else(|| native_state_error("image alias has no actual frame"))?;
        if let Some(previous) = shared_frames.get(&pixel) {
            if *previous != frame {
                return Err(native_state_error("shared image pixel identity has different frame bytes or metadata"));
            }
        } else {
            shared_frames.insert(pixel, frame);
        }
    }
    Ok(())
}

impl DataServices {
    /// Detach a wrapper's alias when its actual image is released. Export
    /// state survives while any other wrapper still owns the same pixels.
    pub(crate) fn release_image_pixel_identity(&mut self, wrapper: u32) {
        if let Some(pixel) = self.image_wrapper_pixels.remove(&wrapper) {
            if !self.image_wrapper_pixels.values().any(|&other| other == pixel) {
                self.image_export.rgb_pixels.remove(&pixel);
            }
        }
    }
}
