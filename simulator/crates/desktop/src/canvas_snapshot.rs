//! Native canvas ownership: texture slots, dirty bits, view and offscreen pixels.
use super::*;
use cs2_runtime::graph_serde::{Deserialize, Serialize};
use cs2_runtime::object_graph::{self as wire, ImageFrameSnapshot};

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct ViewSnapshot {
    center: [u32; 2], depth: u32, translation: [u32; 2], matrix: [u32; 16],
    parent: [u32; 16], perspective: bool,
    cameras: BTreeMap<u32, ([u32; 2], [u32; 2], [u32; 16])>,
}
impl ViewSnapshot {
    pub(crate) fn capture(p: &MoviePlaneProjection, cameras: &BTreeMap<u32, PlaneCamera>) -> Self {
        Self { center: p.center.map(f32::to_bits), depth: p.camera_depth.to_bits(),
            translation: p.translation.map(f32::to_bits), matrix: p.matrix.map(f32::to_bits),
            parent: p.parent.map(f32::to_bits), perspective: p.perspective,
            cameras: cameras.iter().map(|(&id, c)| (id, (c.offset.map(f32::to_bits),
                c.scale.map(f32::to_bits), c.matrix.map(f32::to_bits)))).collect() }
    }
    pub(crate) fn restore(&self) -> Result<(MoviePlaneProjection, BTreeMap<u32, PlaneCamera>)> {
        if self.cameras.len() > 65_536 { bail!("saved canvas camera table exceeds its limit"); }
        Ok((MoviePlaneProjection {
            center: self.center.map(f32::from_bits), camera_depth: f32::from_bits(self.depth),
            translation: self.translation.map(f32::from_bits), matrix: self.matrix.map(f32::from_bits),
            parent: self.parent.map(f32::from_bits), perspective: self.perspective,
        }, self.cameras.iter().map(|(&id, &(offset, scale, matrix))| (id, PlaneCamera {
            offset: offset.map(f32::from_bits), scale: scale.map(f32::from_bits), matrix: matrix.map(f32::from_bits),
        })).collect()))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Snapshot {
    version: u32, initialized: bool, mode: u32, enabled: bool, active: bool,
    size: [u32; 2], tile: [u32; 2], grid: [u32; 2], areas: [[u32; 4]; 8],
    split_row: u32, tiles: Vec<Option<Tile>>, view: Option<ViewSnapshot>,
    offscreen: Option<ImageFrameSnapshot>,
}

const OVERFLOW_TAG: &[u8] = b"CS2VRAM2\0";

fn capture_offscreen(canvas: &Canvas) -> Result<ImageFrameSnapshot> {
    let size = [canvas.width(), canvas.height()];
    let rgba = if let Some(device) = canvas.device_pixels() {
        let mut rgba = vec![0; size[0] as usize * size[1] as usize * 4];
        if !device.read_rgba(&mut rgba) {
            bail!("read offscreen canvas pixels for the saved graph");
        }
        rgba
    } else {
        canvas.pixels().to_vec()
    };
    Ok(ImageFrameSnapshot {
        size,
        rgba,
        container: 0,
        frame_id: None,
        bit_depth: 32,
        offset: [0; 2],
        canvas_size: size,
        base: [0; 2],
        alpha_flag: 0,
        attributes: BTreeMap::new(),
    })
}

impl CanvasNativeState {
    pub(crate) fn graph_snapshot(&self, overflow: Option<&VramOverflow>) -> Result<Vec<u8>> {
        let state = Snapshot {
            version: 1, initialized: self.initialized, mode: self.mode, enabled: self.enabled,
            active: self.active, size: self.size, tile: self.tile, grid: self.grid, areas: self.areas,
            split_row: self.split_row, tiles: self.tiles.clone(),
            view: self.view.as_ref().map(|(p, c)| ViewSnapshot::capture(p, c)),
            offscreen: self.offscreen.as_ref().map(capture_offscreen).transpose()?,
        };
        if let Some(overflow) = overflow {
            let mut payload = OVERFLOW_TAG.to_vec();
            payload.extend(wire::encode_component(&(state, overflow))?);
            Ok(payload)
        } else {
            Ok(wire::encode_component(&state)?)
        }
    }

    pub(crate) fn prepare_graph_restore(bytes: &[u8], logical: [u32; 2], memory: ImageListMemory) -> Result<(Self, Option<VramOverflow>)> {
        let (s, overflow): (Snapshot, Option<VramOverflow>) = if let Some(bytes) = bytes.strip_prefix(OVERFLOW_TAG) {
            let (state, overflow) = wire::decode_component(bytes)?;
            (state, Some(overflow))
        } else {
            (wire::decode_component(bytes)?, None)
        };
        if s.version != 1 || s.tiles.len() > 1_000_000
            || s.tiles.iter().flatten().any(|t| !matches!(t.depth, 16 | 32)) {
            bail!("saved native canvas has invalid texture slots");
        }
        if s.initialized {
            texture_extent(s.tile, s.grid)?;
            if s.size != logical
                || u64::from(s.grid[0]) * u64::from(s.grid[1]) != s.tiles.len() as u64
            {
                bail!("saved canvas geometry/storage differs from the render context");
            }
        } else if !s.tiles.is_empty() {
            bail!("saved uninitialized canvas owns texture slots");
        }
        if let Some(overflow) = &overflow {
            if !s.initialized || overflow.tile != s.tile || overflow.grid != s.grid
                || overflow.pixels.len() > overflow.max_resident_tiles(memory)
                || overflow.pixels.iter().any(|(&index, bytes)| {
                    s.tiles.get(index as usize).is_none_or(Option::is_none)
                        || bytes.len() as u64 != overflow.tile_bytes()
                })
            {
                bail!("saved offscreen VRAM differs from the allocated texture slots");
            }
        }
        let view = s.view.as_ref().map(ViewSnapshot::restore).transpose()?;
        let offscreen = s.offscreen.as_ref().map(|saved| -> Result<Canvas> {
            let frame = crate::image_manager::restore_frame(saved)?;
            frame.try_into_canvas().map_err(|_| anyhow::anyhow!("saved offscreen canvas is empty"))
        }).transpose()?;
        Ok((Self { initialized: s.initialized, mode: s.mode, enabled: s.enabled, active: s.active,
            size: s.size, tile: s.tile, grid: s.grid, memory, areas: s.areas, split_row: s.split_row,
            tiles: s.tiles, view, offscreen }, overflow))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

    #[derive(Debug)]
    struct OffscreenReadback {
        reads: Arc<AtomicUsize>,
        rgba: [u8; 8],
    }

    impl cs2_renderer::CanvasReadback for OffscreenReadback {
        fn read_rgba(&self, output: &mut [u8]) -> bool {
            self.reads.fetch_add(1, Ordering::Relaxed);
            output.copy_from_slice(&self.rgba);
            true
        }

        fn as_any(&self) -> &dyn std::any::Any { self }
    }

    #[test]
    fn offscreen_capture_defers_readback_and_preserves_snapshot_bytes() -> Result<()> {
        let reads = Arc::new(AtomicUsize::new(0));
        let rgba = [1, 2, 3, 4, 15, 16, 17, 18];
        let mut offscreen = Canvas::new(2, 1)?;
        offscreen.set_deferred_device_pixels(Arc::new(OffscreenReadback {
            reads: reads.clone(),
            rgba,
        }));
        let state = CanvasNativeState { offscreen: Some(offscreen), ..Default::default() };
        assert!(state.graph_has_tile_state());
        assert_eq!(reads.load(Ordering::Relaxed), 0);

        let bytes = state.graph_snapshot(None)?;
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        let saved: Snapshot = wire::decode_component(&bytes)?;
        let legacy = crate::image_manager::capture_frame(&RgbaFrame::from_rgba8(2, 1, rgba.to_vec())?);
        assert_eq!(wire::encode_component(&saved.offscreen)?, wire::encode_component(&Some(legacy))?);

        let (restored, overflow) = CanvasNativeState::prepare_graph_restore(&bytes, [2, 1], state.memory)?;
        assert!(overflow.is_none());
        assert_eq!(restored.offscreen.as_ref().unwrap().pixels(), rgba);
        assert_eq!(restored.graph_snapshot(None)?, bytes);
        Ok(())
    }
}
