//! Concrete CPU stencil passes from the driver's D3D9 mode0 state writes.
//!
//! 577ef initializes an8-bit plane, fail=KEEP and alpha GREATER64.
//! 57a96 selects EQUAL/reference + wrapping INCR on pass and depth-fail;
//! 57bde selects EQUAL/reference + wrapping DECR;57ccd selects EQUAL/KEEP.
//! Local type1/2 own-sprite and type5/6 composite scheduling follows4ff0.
//! Local/caller owners share one linked-order plane across motion roots.
//! Captured motion ranges feed the same plane; mode1 targets remain diagnostic.

use super::{EmoteError, EmoteRasterFrame, MAX_FRAME_BYTES, PsbValue, Result};

#[derive(Debug, Clone)]
pub struct CpuStencilBinding {
    pub owner_index: usize,
    pub owner_label: String,
    pub stencil_type: u32,
    /// Own textured sprite first, then explicit composite list order for5/6.
    /// A noncomposite reference group has no own textured primitive (540b).
    pub source_indices: Vec<usize>,
    /// Original3a530 skips absent labels and non-sprite/non-motion layer kinds.
    pub ignored_labels: Vec<String>,
}

/// Resolves local mask geometry. Composite lists use layerIndexMap (3a530);
///noncomposite type1/2 uses the owner's own textured primitive (540b).
///This does not infer linked/nested motion ownership or native completion.
pub fn resolve_emote_stencil_bindings(motion: &PsbValue) -> Result<Vec<CpuStencilBinding>> {
    fn invalid(why: &'static str) -> EmoteError {
        EmoteError::InvalidResource(why)
    }
    fn flatten<'a>(
        layer: &'a PsbValue,
        layers: &mut Vec<&'a PsbValue>,
        depth: usize,
    ) -> Result<()> {
        if depth > 128 || layers.len() >= 16384 {
            return Err(EmoteError::Capacity);
        }
        layers.push(layer);
        for child in layer
            .get("children")
            .and_then(PsbValue::list)
            .ok_or_else(|| invalid("stencil layer children"))?
        {
            flatten(child, layers, depth + 1)?;
        }
        Ok(())
    }
    let mut layers = Vec::new();
    for layer in motion
        .get("layer")
        .and_then(PsbValue::list)
        .ok_or_else(|| invalid("stencil motion layer list"))?
    {
        flatten(layer, &mut layers, 0)?;
    }
    let mut bindings = Vec::new();
    for (owner_index, layer) in layers.iter().enumerate() {
        let kind = layer
            .get("stencilType")
            .and_then(PsbValue::number)
            .unwrap_or(0.0);
        if !kind.is_finite() || kind.fract() != 0.0 || !(0.0..=u32::MAX as f64).contains(&kind) {
            return Err(invalid("stencil type integer"));
        }
        let kind = kind as u32;
        if kind == 0 {
            continue;
        }
        if matches!(kind, 1 | 2) {
            //540b skips nonzero primitive kinds. A kind3 owner retains its
            //linked reference/clear but contributes no own mask geometry;
            //its referenced motion must not be flattened into this pass.
            let layer_kind = layer.get("type").and_then(PsbValue::number);
            if !matches!(layer_kind, Some(0.0 | 3.0)) {
                return Err(EmoteError::Unsupported {
                    native: 906,
                    reason: "noncomposite stencil owner is not a recovered textured sprite",
                });
            }
            bindings.push(CpuStencilBinding {
                owner_index,
                owner_label: layer
                    .get("label")
                    .and_then(PsbValue::text)
                    .unwrap_or("")
                    .to_owned(),
                stencil_type: kind,
                source_indices: if layer_kind == Some(0.0) {
                    vec![owner_index]
                } else {
                    Vec::new()
                },
                ignored_labels: Vec::new(),
            });
            continue;
        }
        if kind & 4 == 0 {
            return Err(EmoteError::Unsupported {
                native: 906,
                reason: "stencil type has an unrecovered local geometry mode",
            });
        }
        let names = layer
            .get("stencilCompositeMaskLayerList")
            .and_then(PsbValue::list)
            .ok_or_else(|| invalid("composite mask source list"))?;
        let indices = motion
            .get("layerIndexMap")
            .and_then(PsbValue::object)
            .ok_or_else(|| invalid("composite mask layerIndexMap"))?;
        let mut binding = CpuStencilBinding {
            owner_index,
            owner_label: layer
                .get("label")
                .and_then(PsbValue::text)
                .unwrap_or("")
                .to_owned(),
            stencil_type: kind,
            //44f56..44fcc includes the owner's own primitive before the
            //explicit sources. Group/blank own primitives are non-textured
            //and skipped by50d6. A textured sprite owner does contribute.
            source_indices: if layer.get("type").and_then(PsbValue::number) == Some(0.0) {
                vec![owner_index]
            } else {
                Vec::new()
            },
            ignored_labels: Vec::new(),
        };
        for name in names {
            let name = name
                .text()
                .ok_or_else(|| invalid("composite source label"))?;
            let Some(index) = indices.get(name).and_then(PsbValue::number) else {
                binding.ignored_labels.push(name.to_owned());
                continue;
            };
            if !index.is_finite()
                || index.fract() != 0.0
                || index < 0.0
                || index >= layers.len() as f64
            {
                return Err(invalid("composite source layer index"));
            }
            let index = index as usize;
            let layer_kind = layers[index].get("type").and_then(PsbValue::number);
            if !matches!(layer_kind, Some(0.0 | 3.0)) {
                binding.ignored_labels.push(name.to_owned());
                continue;
            }
            binding.source_indices.push(index);
        }
        bindings.push(binding);
    }
    Ok(bindings)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuStencilWrite {
    IncrementWrapping,
    DecrementWrapping,
}

pub(super) struct CpuLinkedStencilPlan {
    pub clear: u8,
    pub color_reference: u8,
    pub passes: Vec<(usize, u8, CpuStencilWrite)>,
}

/// Driver4ff0..5602 and table7cbc8=[1,2]. Owners are supplied in their
/// actual linked order (nearest first), across local and caller scopes.
/// Positive owners advance reference once each; negative owners all subtract
/// from the same final reference. Alpha-disjoint primitives form each union.
/// Pass owner indices address this flattened chain, not a per-motion preorder.
pub(super) fn linked_stencil_plan(kinds: &[u32]) -> Result<CpuLinkedStencilPlan> {
    for kind in kinds {
        if !matches!(*kind, 1 | 2 | 5 | 6) {
            return Err(EmoteError::Unsupported {
                native: 906,
                reason: "stencil owner requires a nonlocal or unrecovered type",
            });
        }
    }
    let clear = u8::from(kinds.iter().any(|kind| kind & 3 == 2));
    let mut reference = clear;
    let mut passes = Vec::with_capacity(kinds.len());
    for (index, kind) in kinds.iter().enumerate() {
        if kind & 3 == 1 {
            passes.push((index, reference, CpuStencilWrite::IncrementWrapping));
            reference = reference.wrapping_add(1);
        }
    }
    for (index, kind) in kinds.iter().enumerate() {
        if kind & 3 == 2 {
            passes.push((index, reference, CpuStencilWrite::DecrementWrapping));
        }
    }
    Ok(CpuLinkedStencilPlan {
        clear,
        color_reference: reference,
        passes,
    })
}

#[derive(Debug, Clone)]
pub struct CpuStencilBuffer {
    width: u32,
    height: u32,
    values: Vec<u8>,
    sparse: Option<(u8, Vec<Vec<(usize, u8)>>)>,
}

impl CpuStencilBuffer {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let size = (width as usize)
            .checked_mul(height as usize)
            .filter(|&n| n <= MAX_FRAME_BYTES / 4 && n != 0)
            .ok_or(EmoteError::Capacity)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(size)
            .map_err(|_| EmoteError::Capacity)?;
        values.resize(size, 0);
        Ok(Self { width, height, values, sparse: None })
    }

    pub(super) fn new_sparse(width: u32, height: u32, clear: u8) -> Result<Self> {
        let size = (width as usize).checked_mul(height as usize)
            .filter(|&n| n <= MAX_FRAME_BYTES / 4 && n != 0)
            .ok_or(EmoteError::Capacity)?;
        let _ = size;
        Ok(Self {
            width, height, values: Vec::new(),
            sparse: Some((clear, vec![Vec::new(); height as usize])),
        })
    }

    fn get(&self, x: u32, y: u32) -> Option<u8> {
        if x >= self.width || y >= self.height { return None }
        if let Some((clear, rows)) = &self.sparse {
            let row = &rows[y as usize];
            return Some(row.binary_search_by_key(&(x as usize), |&(x, _)| x)
                .map_or(*clear, |index| row[index].1));
        }
        Some(self.values[y as usize * self.width as usize + x as usize])
    }

    fn set(&mut self, x: u32, y: u32, value: u8) -> bool {
        if x >= self.width || y >= self.height { return false }
        if let Some((_, rows)) = &mut self.sparse {
            let row = &mut rows[y as usize];
            match row.binary_search_by_key(&(x as usize), |&(x, _)| x) {
                Ok(index) => row[index].1 = value,
                Err(index) => row.insert(index, (x as usize, value)),
            }
            return true;
        }
        let index = y as usize * self.width as usize + x as usize;
        self.values[index] = value;
        true
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn values(&self) -> &[u8] {
        &self.values
    }

    /// Reference plane clear from the actual draw schedule, not from layer type.
    pub fn clear(&mut self, value: u8) {
        if let Some((clear, rows)) = &mut self.sparse {
            *clear = value;
            for row in rows { row.clear(); }
        } else {
            self.values.fill(value);
        }
    }

    /// Real texture-alpha mask update. Strict alpha GREATER64 and EQUAL ref,
    /// then wrapping increment/decrement; no color is written in a mask pass.
    /// Supply one primitive's sampled-alpha surface at a time. Precompositing
    /// several subthreshold masks would change the original alpha-test result.
    pub fn write_rgba_mask(
        &mut self,
        mask: &EmoteRasterFrame,
        reference: u8,
        operation: CpuStencilWrite,
    ) -> Result<usize> {
        let mut written = 0usize;
        let [ox, oy] = mask.origin();
        let pixels = mask.rgba()?;
        for sy in 0..mask.height() {
            let y = i64::from(oy) + i64::from(sy);
            if y < 0 || y >= i64::from(self.height) {
                continue;
            }
            for sx in 0..mask.width() {
                let x = i64::from(ox) + i64::from(sx);
                if x < 0 || x >= i64::from(self.width) {
                    continue;
                }
                let source = (sy as usize * mask.width() as usize + sx as usize) * 4;
                if pixels[source + 3] <= 64 {
                    continue;
                }
                if self.get(x as u32, y as u32) != Some(reference) {
                    continue;
                }
                self.set(x as u32, y as u32, match operation {
                    CpuStencilWrite::IncrementWrapping => reference.wrapping_add(1),
                    CpuStencilWrite::DecrementWrapping => reference.wrapping_sub(1),
                });
                written += 1;
            }
        }
        Ok(written)
    }

    /// Per-fragment form for a mesh rasterizer. Coordinates outside the plane
    /// fail; alpha has already been quantized by the sampled-pixel pipeline.
    pub fn write_fragment(
        &mut self,
        x: u32,
        y: u32,
        alpha: u8,
        reference: u8,
        operation: CpuStencilWrite,
    ) -> bool {
        if alpha <= 64 || self.get(x, y) != Some(reference) {
            return false;
        }
        self.set(x, y, match operation {
            CpuStencilWrite::IncrementWrapping => reference.wrapping_add(1),
            CpuStencilWrite::DecrementWrapping => reference.wrapping_sub(1),
        })
    }

    pub fn test_equal(&self, x: u32, y: u32, reference: u8) -> bool {
        self.get(x, y) == Some(reference)
    }

    /// EQUAL/KEEP-gated real straight-RGBA source-over pixels. The caller owns
    /// the recovered draw order and alpha-test policy; this method supplies no
    /// guessed PSB layer scheduling and is not promoted to native success.
    pub fn blend_rgba_equal(
        &self,
        destination: &mut EmoteRasterFrame,
        source: &EmoteRasterFrame,
        reference: u8,
    ) -> Result<usize> {
        if destination.origin() != [0, 0]
            || destination.width() != self.width
            || destination.height() != self.height
        {
            return Err(EmoteError::InvalidFrame);
        }
        let mut submitted = 0usize;
        let [ox, oy] = source.origin();
        let source_pixels = source.rgba()?;
        let destination_pixels = destination.rgba_mut()?;
        for sy in 0..source.height() {
            let y = i64::from(oy) + i64::from(sy);
            if y < 0 || y >= i64::from(self.height) {
                continue;
            }
            for sx in 0..source.width() {
                let x = i64::from(ox) + i64::from(sx);
                if x < 0 || x >= i64::from(self.width) {
                    continue;
                }
                if self.get(x as u32, y as u32) != Some(reference) {
                    continue;
                }
                let index = y as usize * self.width as usize + x as usize;
                let src = (sy as usize * source.width() as usize + sx as usize) * 4;
                let dst = index * 4;
                let alpha = f64::from(source_pixels[src + 3]) / 255.0;
                let old_alpha = f64::from(destination_pixels[dst + 3]) / 255.0;
                let combined = alpha + old_alpha * (1.0 - alpha);
                if combined == 0.0 {
                    continue;
                }
                for c in 0..3 {
                    destination_pixels[dst + c] = ((f64::from(source_pixels[src + c]) * alpha
                        + f64::from(destination_pixels[dst + c]) * old_alpha * (1.0 - alpha))
                        / combined)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
                destination_pixels[dst + 3] = (combined * 255.0).round().clamp(0.0, 255.0) as u8;
                submitted += 1;
            }
        }
        Ok(submitted)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_stencil_matches_dense_for_selected_fragments() {
        for &(clear, operation) in &[
            (0, CpuStencilWrite::IncrementWrapping),
            (255, CpuStencilWrite::IncrementWrapping),
            (0, CpuStencilWrite::DecrementWrapping),
        ] {
            let mut dense = CpuStencilBuffer::new(8, 4).unwrap();
            dense.clear(clear);
            let mut sparse = CpuStencilBuffer::new_sparse(8, 4, clear).unwrap();
            for &(x, y, alpha) in &[
                (0, 0, 65), (3, 1, 64), (7, 3, 255), (3, 1, 66), (8, 0, 255),
            ] {
                assert_eq!(
                    sparse.write_fragment(x, y, alpha, clear, operation),
                    dense.write_fragment(x, y, alpha, clear, operation),
                );
                for test_y in 0..4 {
                    for test_x in 0..8 {
                        for reference in [0, 1, 255] {
                            assert_eq!(
                                sparse.test_equal(test_x, test_y, reference),
                                dense.test_equal(test_x, test_y, reference),
                            );
                        }
                    }
                }
            }
            sparse.clear(clear.wrapping_add(7));
            dense.clear(clear.wrapping_add(7));
            assert!(sparse.values().is_empty());
            for y in 0..4 {
                for x in 0..8 {
                    assert_eq!(
                        sparse.test_equal(x, y, clear.wrapping_add(7)),
                        dense.test_equal(x, y, clear.wrapping_add(7)),
                    );
                }
            }
        }
    }
}
