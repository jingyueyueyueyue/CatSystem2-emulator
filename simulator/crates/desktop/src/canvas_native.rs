//! Native canvas tile lifetime and explicit depth-range frame capture.
use super::movie_plane::PlaneCamera;
use super::{LoggingHost, MoviePlaneProjection, native_state_error, read_u32};
use anyhow::{Context, Result, bail};
use cs2_formats::ConfigXml;
use cs2_renderer::{Canvas, Color, Rect, RgbaFrame};
use cs2_script::{NativeReturn, Vm};
use std::collections::BTreeMap;

const MAX_BYTES: u64 = 128 * 1024 * 1024;
const HATSUMIRA: &str = "c48d383c335c5b283bb0c17bd9f14b2b8bfef4a2e3d635365157b1c69498df78";

#[path = "canvas_snapshot.rs"]
mod graph;
pub(crate) use graph::ViewSnapshot;

#[derive(Clone, Copy, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Tile {
    depth: u32,
    dirty: bool,
}

fn texture_geometry(xml: Option<&ConfigXml>, logical: [u32; 2]) -> Result<([u32; 2], [u32; 2])> {
    let value = |section: &[u8], axis: &[u8], default| -> Result<u32> {
        let number = xml.map_or(default, |xml| {
            xml.integer(&[b"document", b"TEXTURE", section, axis], default)
        });
        u32::try_from(number).context("canvas texture dimensions must not be negative")
    };
    let tile = [value(b"size", b"x", 256)?, value(b"size", b"y", 256)?];
    if tile.contains(&0) {
        bail!("canvas texture size must be positive");
    }
    let mut grid = [value(b"vt", b"x", 0)?, value(b"vt", b"y", 0)?];
    for (axis, configured) in grid.iter_mut().enumerate() {
        if *configured == 0 {
            *configured = logical[axis].div_ceil(tile[axis]);
        }
    }
    texture_extent(tile, grid)?;
    Ok((tile, grid))
}

fn texture_extent(tile: [u32; 2], grid: [u32; 2]) -> Result<[u32; 2]> {
    if tile.contains(&0) || grid.contains(&0) {
        bail!("canvas texture layout must have positive tile and grid dimensions");
    }
    if u64::from(grid[0]) * u64::from(grid[1]) > 1_000_000 {
        bail!("canvas grid exceeds one million slots");
    }
    let width = tile[0].checked_mul(grid[0]);
    let height = tile[1].checked_mul(grid[1]);
    let (Some(width), Some(height)) = (width, height) else {
        bail!("canvas texture extent overflows");
    };
    if u64::from(width) * u64::from(height) > MAX_BYTES / 4 {
        bail!("adopted canvas texture storage exceeds 128 MiB");
    }
    Ok([width, height])
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct VramOverflow {
    tile: [u32; 2],
    grid: [u32; 2],
    pixels: BTreeMap<u32, Vec<u8>>,
    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    #[serde(skip)]
    packed: std::cell::RefCell<PackedRegions>,
    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    #[serde(skip)]
    texture_updates: std::cell::RefCell<AtlasUpdates>,
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
#[derive(Debug, Default)]
struct AtlasUpdates {
    id: u64,
    version: u64,
    dirty: std::collections::BTreeSet<u32>,
    versions: BTreeMap<u32, u64>,
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
impl Clone for AtlasUpdates {
    fn clone(&self) -> Self { Self::default() }
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
#[derive(Default, Debug)]
struct PackedRegions {
    entries: std::collections::VecDeque<PackedRegion>,
    all_dirty: bool,
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
impl Clone for PackedRegions {
    fn clone(&self) -> Self { Self::default() }
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
#[derive(Debug)]
struct PackedRegion {
    origin: [u32; 2],
    size: [u32; 2],
    viewport: Option<cs2_renderer::FramePixelIdentity>,
    frame: RgbaFrame,
    valid: bool,
}

impl VramOverflow {
    pub(super) fn dimensions(&self) -> [u32; 2] {
        [self.tile[0] * self.grid[0], self.tile[1] * self.grid[1]]
    }

    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    pub(super) fn gpu_version(&self) -> (u64, u64) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_ATLAS: AtomicU64 = AtomicU64::new(1);
        let mut updates = self.texture_updates.borrow_mut();
        if updates.id == 0 { updates.id = NEXT_ATLAS.fetch_add(1, Ordering::Relaxed); }
        if !updates.dirty.is_empty() {
            updates.version += 1;
            let version = updates.version;
            for tile in std::mem::take(&mut updates.dirty) { updates.versions.insert(tile, version); }
        }
        (updates.id, updates.version)
    }

    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    pub(super) fn visit_gpu_tiles(&self, after: Option<u64>, mut visit: impl FnMut([u32; 2], [u32; 2], &[u8])) {
        let updates = self.texture_updates.borrow();
        for (&index, pixels) in &self.pixels {
            if after.is_some_and(|version| updates.versions.get(&index).copied().unwrap_or(0) <= version) {
                continue;
            }
            let origin = [(index % self.grid[0]) * self.tile[0], (index / self.grid[0]) * self.tile[1]];
            visit(origin, self.tile, pixels);
        }
    }

    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    fn mark_gpu_tile(&mut self, index: u32) {
        let updates = self.texture_updates.get_mut();
        if updates.id != 0 { updates.dirty.insert(index); }
    }

    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    pub(super) fn pack_region(&self, viewport: &RgbaFrame, origin: [u32; 2], size: [u32; 2]) -> Result<RgbaFrame> {
        let [width, height] = size;
        let depends_on_viewport = origin[0] < viewport.width() && origin[1] < viewport.height();
        let mut cache = self.packed.borrow_mut();
        if std::mem::take(&mut cache.all_dirty) {
            for entry in &mut cache.entries { entry.valid = false; }
        }
        let previous = cache.entries.iter().position(|entry| entry.origin == origin && entry.size == size)
            .and_then(|index| cache.entries.remove(index));
        if let Some(entry) = previous.as_ref().filter(|entry| entry.valid
            && if depends_on_viewport { entry.viewport.as_ref().is_some_and(|pixels| pixels.matches(viewport)) }
                else { entry.viewport.is_none() })
        {
            let frame = entry.frame.clone();
            cache.entries.push_back(previous.unwrap());
            return Ok(frame);
        }
        let mut canvas = Canvas::new(width, height)?;
        let source_width = viewport.width();
        for y in 0..height {
            let sample_y = origin[1] + y;
            let row = y as usize * width as usize * 4;
            let mut x = 0;
            while x < width {
                let sample_x = origin[0] + x;
                let (count, input) = if sample_y < viewport.height() && sample_x < source_width {
                    let count = (width - x).min(source_width - sample_x);
                    let start = (sample_y as usize * source_width as usize + sample_x as usize) * 4;
                    (count, Some(&viewport.pixels()[start..start + count as usize * 4]))
                } else {
                    self.row_span(sample_x, sample_y, width - x)
                };
                if let Some(input) = input {
                    let start = row + x as usize * 4;
                    canvas.pixels_mut()[start..start + input.len()].copy_from_slice(input);
                }
                x += count;
            }
        }
        // Dirty regions can be written more than once with the same pixels.
        // Preserve the immutable identity if repacking proves output equality.
        let frame = match previous {
            Some(entry) if entry.frame.pixels() == canvas.pixels() => entry.frame,
            _ => canvas.into_frame(),
        };
        let bytes = frame.pixels().len();
        const MAX_BYTES: usize = 16 * 1024 * 1024;
        if bytes <= MAX_BYTES {
            let mut resident: usize = cache.entries.iter().map(|entry| entry.frame.pixels().len()).sum();
            while resident + bytes > MAX_BYTES || cache.entries.len() >= 16 {
                resident -= cache.entries.pop_front().unwrap().frame.pixels().len();
            }
            cache.entries.push_back(PackedRegion { origin, size, frame: frame.clone(), valid: true,
                viewport: depends_on_viewport.then(|| viewport.pixel_identity()) });
        }
        Ok(frame)
    }

    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    fn invalidate_packed(&mut self, origin: [u32; 2], end: [u32; 2]) {
        for entry in &mut self.packed.get_mut().entries {
            if (0..2).all(|axis| origin[axis] < entry.origin[axis] + entry.size[axis] && entry.origin[axis] < end[axis]) {
                entry.valid = false;
            }
        }
    }

    fn byte_offset(&self, x: u32, y: u32) -> usize {
        ((y % self.tile[1]) as usize * self.tile[0] as usize + (x % self.tile[0]) as usize) * 4
    }

    fn tile_bytes(&self) -> usize {
        self.tile[0] as usize * self.tile[1] as usize * 4
    }

    fn set(&mut self, index: u32, x: u32, y: u32, color: Color) {
        let offset = self.byte_offset(x, y);
        let length = self.tile_bytes();
        let rgba = [color.r, color.g, color.b, color.a];
        match self.pixels.entry(index) {
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if entry.get()[offset..offset + 4] == rgba { return; }
                entry.get_mut()[offset..offset + 4].copy_from_slice(&rgba);
            }
            std::collections::btree_map::Entry::Vacant(entry) if color != Color::TRANSPARENT => {
                entry.insert(vec![0; length])[offset..offset + 4]
                    .copy_from_slice(&rgba);
            }
            _ => return,
        }
        #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
        {
            self.packed.get_mut().all_dirty = true;
            self.mark_gpu_tile(index);
        }
    }

    pub(super) fn pixel(&self, x: u32, y: u32) -> Option<Color> {
        self.pixel_sampler()(x, y)
    }

    pub(super) fn pixel_sampler(&self) -> impl Fn(u32, u32) -> Option<Color> + Sync + '_ {
        let Self { tile, grid, pixels, .. } = self;
        move |x, y| {
            let column = x / tile[0];
            let row = y / tile[1];
            if column >= grid[0] || row >= grid[1] {
                return None;
            }
            let offset = ((y % tile[1]) as usize * tile[0] as usize + (x % tile[0]) as usize) * 4;
            Some(pixels.get(&(row * grid[0] + column)).map_or(Color::TRANSPARENT, |bytes| {
                Color::rgba(bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3])
            }))
        }
    }

    pub(super) fn row_span(&self, x: u32, y: u32, width: u32) -> (u32, Option<&[u8]>) {
        let column = x / self.tile[0];
        let row = y / self.tile[1];
        if column >= self.grid[0] || row >= self.grid[1] {
            return (width, None);
        }
        let count = width.min(self.tile[0] - x % self.tile[0]);
        let offset = self.byte_offset(x, y);
        let bytes = self.pixels.get(&(row * self.grid[0] + column))
            .map(|pixels| &pixels[offset..offset + count as usize * 4]);
        (count, bytes)
    }
}

#[derive(Default)]
pub(super) struct CanvasNativeState {
    initialized: bool,
    pub(super) mode: u32,
    pub(super) enabled: bool,
    pub(super) active: bool,
    size: [u32; 2],
    tile: [u32; 2],
    grid: [u32; 2],
    areas: [[u32; 4]; 8],
    split_row: u32,
    tiles: Vec<Option<Tile>>,
    view: Option<(MoviePlaneProjection, BTreeMap<u32, PlaneCamera>)>,
    offscreen: Option<Canvas>,
}

impl CanvasNativeState {
    pub(crate) fn graph_has_tile_state(&self) -> bool { self.initialized || self.offscreen.is_some() }
    pub(super) fn supports(index: u32) -> bool {
        matches!(index, 541 | 542 | 568 | 858 | 859)
    }
    pub(super) fn tile_size(&self) -> Option<[u32; 2]> {
        self.initialized.then_some(self.tile)
    }
    pub(super) fn tile_bit_depth(&self, index: usize) -> Option<u32> {
        self.tiles
            .get(index)
            .copied()
            .flatten()
            .map(|tile| tile.depth)
    }
    pub(super) fn dirty_tiles(&self) -> Vec<usize> {
        self.tiles
            .iter()
            .enumerate()
            .filter_map(|(index, tile)| tile.is_some_and(|tile| tile.dirty).then_some(index))
            .collect()
    }

    fn stores_pixels(&self) -> bool {
        !self.initialized || (self.mode == 0 && self.enabled && self.active)
    }

    fn write_pixel(
        &mut self, target: &mut Canvas, overflow: &mut Option<VramOverflow>,
        x: i32, y: i32, mut color: Color,
    ) -> Result<()> {
        if x < 0 || y < 0 {
            return Ok(());
        }
        let in_viewport = (x as u32) < target.width() && (y as u32) < target.height();
        if self.initialized {
            let column = x as u32 / self.tile[0];
            let row = y as u32 / self.tile[1];
            if column >= self.grid[0] || row >= self.grid[1] {
                return Ok(());
            }
            let index = row * self.grid[0] + column;
            let Some(tile) = self.tiles.get_mut(index as usize).and_then(Option::as_mut) else {
                return Ok(());
            };
            if tile.depth == 16 {
                color = Color::rgba(
                    (color.r >> 4) * 17,
                    (color.g >> 4) * 17,
                    (color.b >> 4) * 17,
                    (color.a >> 4) * 17,
                );
            }
            tile.dirty = true;
            if !in_viewport && (overflow.is_some() || color != Color::TRANSPARENT) {
                overflow.get_or_insert_with(|| VramOverflow {
                    tile: self.tile, grid: self.grid, pixels: BTreeMap::new(),
                    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
                    packed: Default::default(),
                    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
                    texture_updates: Default::default(),
                }).set(index, x as u32, y as u32, color);
            }
        }
        if in_viewport {
            target.set_pixel(x, y, color)?;
        }
        Ok(())
    }

    fn storage_bounds(&self, target: &Canvas) -> [i32; 4] {
        let size = if self.initialized {
            [self.tile[0] * self.grid[0], self.tile[1] * self.grid[1]]
        } else {
            [target.width(), target.height()]
        };
        [0, 0, size[0] as i32, size[1] as i32]
    }

    fn clear_rectangle(&mut self, target: &mut Canvas, overflow: &mut Option<VramOverflow>, rectangle: [i32; 4]) -> Result<()> {
        if !self.stores_pixels() {
            return Ok(());
        }
        let Some([left, top, right, bottom]) = clipped_rectangle(
            rectangle,
            self.storage_bounds(target),
        ) else {
            return Ok(());
        };
        let [left, top, right, bottom] = [left, top, right, bottom].map(|value| value as u32);
        if !self.initialized {
            zero_tile(target, left, top, [right - left, bottom - top]);
            return Ok(());
        }
        #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
        if let Some(atlas) = overflow.as_mut() { atlas.invalidate_packed([left, top], [right, bottom]); }

        let [tile_width, tile_height] = self.tile;
        let viewport = [target.width(), target.height()];
        for row in top / tile_height..=(bottom - 1) / tile_height {
            for column in left / tile_width..=(right - 1) / tile_width {
                let index = row * self.grid[0] + column;
                let Some(tile) = self.tiles.get_mut(index as usize).and_then(Option::as_mut) else { continue };
                tile.dirty = true;
                let [tile_x, tile_y] = [column * tile_width, row * tile_height];
                let [x0, y0] = [left.max(tile_x), top.max(tile_y)];
                let [x1, y1] = [right.min(tile_x + tile_width), bottom.min(tile_y + tile_height)];
                zero_tile(target, x0, y0, [x1 - x0, y1 - y0]);
                if let Some(atlas) = overflow.as_mut() && let Some(bytes) = atlas.pixels.get_mut(&index) {
                    // Only the part outside the viewport is owned by the
                    // sparse atlas. Clearing unused space never allocates it.
                    let stride = tile_width as usize * 4;
                    let end = (x1 - tile_x) as usize * 4;
                    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
                    let mut changed = false;
                    for y in y0..y1 {
                        let left = if y < viewport[1] { x0.max(viewport[0]) } else { x0 };
                        if left < x1 {
                            let start = (left - tile_x) as usize * 4;
                            let row = (y - tile_y) as usize * stride;
                            let pixels = &mut bytes[row + start..row + end];
                            #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
                            { changed |= list_span_occupies_tile(pixels, 32); }
                            pixels.fill(0);
                        }
                    }
                    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
                    if changed { atlas.mark_gpu_tile(index); }
                }
            }
        }
        Ok(())
    }

    fn copy_image(
        &mut self,
        target: &mut Canvas,
        overflow: &mut Option<VramOverflow>,
        source: &RgbaFrame,
        at: [i32; 2],
        rectangle: [i32; 4],
    ) -> Result<()> {
        if !self.stores_pixels() {
            return Ok(());
        }
        let [width, height] = [
            i32::try_from(source.width())?,
            i32::try_from(source.height())?,
        ];
        let offset = source.metadata().offset;
        let Some(from) = clipped_rectangle(rectangle, [offset[0], offset[1], width, height]) else {
            return Ok(());
        };
        let from_local = [
            from[0].wrapping_sub(offset[0]),
            from[1].wrapping_sub(offset[1]),
        ];
        let destination = [at[0].wrapping_add(offset[0]), at[1].wrapping_add(offset[1])];
        let Some(to) = clipped_rectangle(
            [
                destination[0],
                destination[1],
                from[2] - from[0],
                from[3] - from[1],
            ],
            self.storage_bounds(target),
        ) else {
            return Ok(());
        };
        let source_at = [
            from_local[0] + to[0].wrapping_sub(destination[0]),
            from_local[1] + to[1].wrapping_sub(destination[1]),
        ];
        // Native 115/367's canvas path walks the decoded bitmap with a
        // positive pitch. RgbaFrame and the destination atlas are top-down.
        for y in to[1]..to[3] {
            for x in to[0]..to[2] {
                let sx = source_at[0] + x - to[0];
                let sy = source_at[1] + y - to[1];
                let color = source.pixel(sx, sy)?;
                self.write_pixel(target, overflow, x, y, color)?;
            }
        }
        Ok(())
    }

    pub(super) fn copy_list_rectangle(
        &mut self,
        source: &RgbaFrame,
        target: &mut Canvas,
        overflow: &mut Option<VramOverflow>,
        rectangle: Rect,
        offset: [i32; 2],
    ) -> Result<()> {
        if !self.stores_pixels() {
            return Ok(());
        }
        let [_, _, width, height] = self.storage_bounds(target);
        let [offset_x, offset_y] = offset.map(i64::from);
        let [left, top] = [i64::from(rectangle.x), i64::from(rectangle.y)];
        let source_left = left.max(0).max(-offset_x);
        let source_top = top.max(0).max(-offset_y);
        let source_right = (left + i64::from(rectangle.width))
            .min(i64::from(source.width()))
            .min(i64::from(width) - offset_x);
        let source_bottom = (top + i64::from(rectangle.height))
            .min(i64::from(source.height()))
            .min(i64::from(height) - offset_y);
        if source_left >= source_right || source_top >= source_bottom {
            return Ok(());
        }

        let destination_left = (source_left + offset_x) as u32;
        let destination_right = (source_right + offset_x) as u32;
        #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
        if let Some(atlas) = overflow.as_mut() {
            atlas.invalidate_packed([destination_left, (source_top + offset_y) as u32],
                [destination_right, (source_bottom + offset_y) as u32]);
        }
        let (target_width, target_height) = target.dimensions();
        let source_stride = source.width() as usize * 4;
        let target_stride = target_width as usize * 4;
        for source_y in source_top..source_bottom {
            let destination_y = (source_y + offset_y) as u32;
            let mut destination_x = destination_left;
            while destination_x < destination_right {
                let in_viewport = destination_y < target_height && destination_x < target_width;
                let mut end = if in_viewport {
                    destination_right.min(target_width)
                } else {
                    destination_right
                };
                let mut depth = 32;
                let mut tile_index = 0;
                if self.initialized {
                    let column = destination_x / self.tile[0];
                    let row = destination_y / self.tile[1];
                    tile_index = row * self.grid[0] + column;
                    end = end.min((column + 1) * self.tile[0]);
                    let Some(tile) = self.tiles.get_mut(tile_index as usize).and_then(Option::as_mut) else {
                        destination_x = end;
                        continue;
                    };
                    depth = tile.depth;
                    tile.dirty = true;
                }
                let source_x = source_left as usize + (destination_x - destination_left) as usize;
                let source_start = source_y as usize * source_stride + source_x * 4;
                let byte_count = (end - destination_x) as usize * 4;
                let input = &source.pixels()[source_start..source_start + byte_count];
                if in_viewport {
                    let start = destination_y as usize * target_stride + destination_x as usize * 4;
                    copy_list_span(&mut target.pixels_mut()[start..start + byte_count], input, depth);
                } else if overflow.as_ref().is_some_and(|atlas| atlas.pixels.contains_key(&tile_index))
                    || list_span_occupies_tile(input, depth)
                {
                    let atlas = overflow.get_or_insert_with(|| VramOverflow {
                        tile: self.tile, grid: self.grid, pixels: BTreeMap::new(),
                        #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
                        packed: Default::default(),
                        #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
                        texture_updates: Default::default(),
                    });
                    let start = atlas.byte_offset(destination_x, destination_y);
                    let tile_bytes = atlas.tile_bytes();
                    let output = atlas.pixels.entry(tile_index).or_insert_with(|| vec![0; tile_bytes]);
                    let pixels = &mut output[start..start + byte_count];
                    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
                    let changed = if depth == 16 { pixels.iter().zip(input).any(|(&old, &new)| old != (new >> 4) * 17) }
                        else { pixels != input };
                    copy_list_span(pixels, input, depth);
                    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
                    if changed { atlas.mark_gpu_tile(tile_index); }
                }
                destination_x = end;
            }
        }
        Ok(())
    }
}

fn list_span_occupies_tile(input: &[u8], depth: u32) -> bool {
    // The overflow atlas stores RGB even at zero alpha. A 16-bit tile
    // discards the low nibble of every channel, including alpha.
    let mask = if depth == 16 { 0xf0u8 } else { u8::MAX };
    let word_mask = u64::from_ne_bytes([mask; 8]);
    let (words, tail) = input.as_chunks::<8>();
    words.iter().any(|&word| u64::from_ne_bytes(word) & word_mask != 0)
        || tail.iter().any(|&byte| byte & mask != 0)
}

fn copy_list_span(target: &mut [u8], source: &[u8], depth: u32) {
    if depth == 16 {
        for (target, source) in target.iter_mut().zip(source) {
            *target = (source >> 4) * 17;
        }
    } else {
        target.copy_from_slice(source);
    }
}

impl LoggingHost<'_> {
    pub(super) fn clear_graphics_canvas_rect(&mut self, rectangle: [i32; 4]) -> Result<()> {
        self.ensure_canvas_state()?;
        if let Some(canvas) = self.canvas.as_deref_mut() {
            self.data_services
                .canvas_native
                .clear_rectangle(canvas, &mut self.data_services.planes.vram_overflow, rectangle)?;
        }
        Ok(())
    }

    pub(super) fn copy_image_to_graphics_canvas(
        &mut self,
        source: &RgbaFrame,
        at: [i32; 2],
        rectangle: [i32; 4],
    ) -> Result<()> {
        self.ensure_canvas_state()?;
        if let Some(canvas) = self.canvas.as_deref_mut() {
            self.data_services
                .canvas_native
                .copy_image(canvas, &mut self.data_services.planes.vram_overflow, source, at, rectangle)?;
        }
        Ok(())
    }

    pub(super) fn ensure_canvas_state(&mut self) -> Result<()> {
        if self.data_services.canvas_native.initialized {
            if let Some(movie) = self.movie_host.as_deref() {
                self.data_services.canvas_native.view = Some((
                    movie.planes.projection.clone(),
                    movie.planes.cameras.clone(),
                ));
            }
            return Ok(());
        }
        let Some(canvas) = self.canvas.as_ref() else {
            return Ok(());
        };
        let size = [canvas.width(), canvas.height()];
        let xml = self.resources.startup_configuration()?;
        let (tile, grid) = texture_geometry(xml.as_ref(), size)?;
        let value = |path: &[&[u8]], default: i32| {
            xml.as_ref()
                .map_or(default, |xml| xml.integer(path, default)) as u32
        };
        let count = u64::from(grid[0]) * u64::from(grid[1]);
        let mut areas = [[u32::MAX; 4]; 8];
        for (index, area) in areas.iter_mut().enumerate() {
            let name = format!("area{index:02}");
            for (field, key) in [b"sx".as_slice(), b"sy", b"dx", b"dy"]
                .into_iter()
                .enumerate()
            {
                area[field] = value(&[b"document", b"TEXTURE", name.as_bytes(), key], -1);
            }
        }
        let view = self.movie_host.as_deref().map(|movie| {
            (
                movie.planes.projection.clone(),
                movie.planes.cameras.clone(),
            )
        });
        self.data_services.canvas_native = CanvasNativeState {
            initialized: true,
            mode: 0,
            enabled: true,
            active: true,
            size,
            tile,
            grid,
            areas,
            split_row: value(&[b"document", b"TEXTURE", b"bpp16y"], 0),
            tiles: vec![
                Some(Tile {
                    depth: 32,
                    dirty: false
                });
                count as usize
            ],
            view,
            offscreen: None,
        };
        Ok(())
    }

    pub(super) fn canvas_native(
        &mut self,
        index: u32,
        args: &[u8],
        _vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        // The depth wrapper consumes both float DWORDs before context gates.
        let depth = if matches!(index, 568 | 859) {
            let offset = if index == 859 { 4 } else { 0 };
            Some([
                f32::from_bits(read_u32(args, offset)?),
                f32::from_bits(read_u32(args, offset + 4)?),
            ])
        } else {
            None
        };
        let auxiliary = if matches!(index, 858 | 859) {
            Some(read_u32(args, 0)?)
        } else {
            None
        };
        let result = (|| -> Result<()> {
            self.ensure_canvas_state()?;
            let (initialized, mode, enabled, surface_size) = {
                let state = &self.data_services.canvas_native;
                (state.initialized, state.mode, state.enabled, state.size)
            };
            if !initialized || mode != 0 || !enabled || self.canvas.is_none() {
                return Ok(());
            }
            match index {
                858 => {
                    self.ensure_configuration_state()?;
                    if self
                        .data_services
                        .configuration
                        .platform_value("D3D/backbuffer")
                        != Some(0)
                    {
                        let size = self
                            .back_buffer
                            .as_ref()
                            .map_or(self.data_services.canvas_native.size, |buffer| {
                                [buffer.width(), buffer.height()]
                            });
                        let limit = self.data_services.tasks.render.auxiliary_dimension_limit();
                        self.data_services.tasks.render.prepare_auxiliary_targets(
                            auxiliary.unwrap(),
                            size,
                            limit,
                        )?;
                    }
                }
                859 => {
                    let index = auxiliary.unwrap();
                    let Some(target) = self.data_services.tasks.render.auxiliary_target(index)
                    else {
                        return Ok(());
                    };
                    let mut output = Canvas::new(target.width(), target.height())?;
                    output.clear(Color::TRANSPARENT);
                    let [lower, upper] = depth.unwrap();
                    self.draw_scene_depth_range(&mut output, lower, upper)?;
                    let output = RgbaFrame::from_rgba8(
                        output.width(),
                        output.height(),
                        output.pixels().to_vec(),
                    )?;
                    self.data_services
                        .tasks
                        .render
                        .replace_auxiliary_target(index, output)?;
                }
                541 => {
                    let [x, y, w, h] =
                        [0, 4, 8, 12].map(|at| read_u32(args, at).map(|value| value as i32));
                    let (x, y, w, h) = (x?, y?, w?, h?);
                    let right = x.wrapping_add(w);
                    let bottom = y.wrapping_add(h);
                    if right <= x || bottom <= y {
                        return Ok(());
                    }
                    let state = &mut self.data_services.canvas_native;
                    let canvas = self.canvas.as_mut().unwrap();
                    for at in 0..state.tiles.len() {
                        let column = at as u32 % state.grid[0];
                        let row = at as u32 / state.grid[0];
                        let left = i64::from(column) * i64::from(state.tile[0]);
                        let top = i64::from(row) * i64::from(state.tile[1]);
                        if left >= i64::from(right)
                            || top >= i64::from(bottom)
                            || left + i64::from(state.tile[0]) <= i64::from(x)
                            || top + i64::from(state.tile[1]) <= i64::from(y)
                        {
                            continue;
                        }
                        if state.tiles[at].take().is_some() {
                            zero_tile(canvas, left as u32, top as u32, state.tile);
                            if let Some(overflow) = self.data_services.planes.vram_overflow.as_mut() {
                                overflow.pixels.remove(&(at as u32));
                            }
                        }
                    }
                }
                542 => {
                    let state = &mut self.data_services.canvas_native;
                    let mut visits = 0usize;
                    let mut allocated = state.tiles.iter().filter(|tile| tile.is_some()).count()
                        as u64
                        * u64::from(state.tile[0])
                        * u64::from(state.tile[1])
                        * 4;
                    for low in [true, false] {
                        for area in state.areas {
                            if area[0] == u32::MAX {
                                continue;
                            }
                            let end_x = area[0].wrapping_add(area[2]) as i32;
                            let end_y = area[1].wrapping_add(area[3]) as i32;
                            let start_y = if low {
                                (area[1] as i32).max(state.split_row as i32)
                            } else {
                                area[1] as i32
                            };
                            let end_y = if low {
                                end_y
                            } else {
                                end_y.min(state.split_row as i32)
                            };
                            for row in start_y..end_y {
                                for column in area[0] as i32..end_x {
                                    visits += 1;
                                    if visits > 1_000_000 {
                                        bail!("canvas preparation exceeds one million positions");
                                    }
                                    let at = (row as u32)
                                        .wrapping_mul(state.grid[0])
                                        .wrapping_add(column as u32)
                                        as usize;
                                    let tile = state
                                        .tiles
                                        .get_mut(at)
                                        .context("canvas preparation index is outside its grid")?;
                                    if tile.is_none() {
                                        allocated = allocated
                                            .checked_add(
                                                u64::from(state.tile[0])
                                                    * u64::from(state.tile[1])
                                                    * 4,
                                            )
                                            .filter(|&size| size <= MAX_BYTES)
                                            .context("canvas tile pixels exceed 128 MiB")?;
                                        *tile = Some(Tile {
                                            depth: if low { 16 } else { 32 },
                                            dirty: false,
                                        });
                                        zero_tile(
                                            self.canvas.as_mut().unwrap(),
                                            (column as u32).wrapping_mul(state.tile[0]),
                                            (row as u32).wrapping_mul(state.tile[1]),
                                            state.tile,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                568 => {
                    let size = self
                        .back_buffer
                        .as_ref()
                        .map_or(surface_size, |buffer| [buffer.width(), buffer.height()]);
                    let shared_capture = self.css_task_sha() == Some(HATSUMIRA);
                    if shared_capture {
                        // Hatsumira 0x524a50 allocates one auxiliary target and
                        // calls 0x52dc30 with slot 0 before the script changes the scene.
                        let policy = self.frame_effect_capture_policy()?;
                        if !self.data_services.tasks.render.prepare_effect_auxiliary(1, size, policy)? {
                            return Ok(());
                        }
                    }
                    let mut output = Canvas::new(size[0], size[1])?;
                    let [lower, upper] = depth.unwrap();
                    self.draw_scene_depth_range(&mut output, lower, upper)?;
                    if shared_capture {
                        self.data_services.tasks.render.replace_auxiliary_target(0, output.into_frame())?;
                    } else {
                        self.data_services.canvas_native.offscreen = Some(output);
                    }
                }
                _ => bail!("canvas native {index} has no contract"),
            }
            Ok(())
        })();
        result.map(|()| NativeReturn::Continue).map_err(|error| {
            self.events.push(super::LogicEvent {
                frame: self.frame,
                kind: "canvas-error".into(),
                detail: error.to_string(),
            });
            native_state_error("canvas operation failed; see canvas-error")
        })
    }

    /// Repaints ordinary drawables for 568 and render-task work passes. The
    /// range comparison intentionally preserves unordered NaN comparisons.
    pub(super) fn draw_scene_depth_range(
        &mut self,
        output: &mut Canvas,
        lower: f32,
        upper: f32,
    ) -> Result<()> {
        self.ensure_canvas_state()?;
        let (projection, cameras) = self
            .data_services
            .canvas_native
            .view
            .clone()
            .context("offscreen draw has no saved frame view")?;
        if upper <= 0. {
            return Ok(());
        }
        let source = self
            .canvas
            .as_ref()
            .context("offscreen draw has no source canvas")?;
        let source =
            RgbaFrame::from_rgba8(source.width(), source.height(), source.pixels().to_vec())?;
        let mut depths = BTreeMap::new();
        if let Some(movie) = self.movie_host.as_deref() {
            for node in self.scene_nodes.values() {
                if let Some(mut depth) = movie.movie_plane_depth(node.id) {
                    if let Some(properties) = movie.planes.properties.get(&node.id)
                        && let Some(offset) = properties.parent_offset
                        && let Some(parent) = self.scene_nodes.get(&node.parent)
                    {
                        let base = movie
                            .planes
                            .properties
                            .get(&parent.id)
                            .map_or(5000., |properties| properties.base[2]);
                        depth = base + offset[2];
                    }
                    depths.insert(node.id, depth);
                }
            }
        }
        let order = self
            .data_services
            .planes
            .draw_order_with_depths(self.scene_nodes, &depths);
        for id in order {
            let Some(node) = self.scene_nodes.get(&id) else {
                continue;
            };
            let depth = depths.get(&id).copied().unwrap_or_else(|| {
                super::plane_native::plane_depth(node, &self.data_services.planes)
            });
            if depth < lower {
                continue;
            }
            if depth >= upper {
                break;
            }
            if !node.visible || !node.initialized {
                continue;
            }
            if super::particle_native::draw_ordinary_task(
                id, output, self.particles, &projection, &cameras, lower, upper,
            )? { continue; }
            if self.data_services.css_models.draw(id, output, self.resources, self.text_encoding)? { continue; }
            if self.data_services.particle_external.draw(id, output, self.particles)? { continue; }
            if super::native_tasks::RenderTaskStore::supports_kind(node.kind) {
                // Retained effects submit through the single window callback.
                // Repainting a source range must not recurse into that callback.
                continue;
            }
            if self.data_services.emote.draw_scene_task(node, output)? {
                continue;
            }
            if self
                .data_services
                .tasks
                .javascript
                .render_plane(output, node)?
            {
                continue;
            }
            if let Some(movie) = self.movie_host.as_deref_mut()
                && movie.draw_movie_plane(output, node, self.scene_nodes)?
            {
                continue;
            }
            if self.draw_kx_geometry_into(output, node, &source, &cameras, &projection)? {
                continue;
            }
            if super::plane_native::draw_plane_with_view(
                output,
                node,
                &self.data_services.planes,
                Some(&source),
                &cameras,
                &projection,
            )? {
                continue;
            }
            if matches!(node.kind,0|1|6|8|11..=13|15|16|32..=49|55|63) {
                continue;
            }
            bail!(
                "task {id} type {} has no concrete offscreen drawable",
                node.kind
            );
        }
        draw_text_range(self.data_services, self.scene_nodes, output, lower, upper)?;
        Ok(())
    }
}

/// Message owners submit their already prepared surfaces; offscreen capture
/// never advances reveal timing, handlers or the message interpreter.
pub(super) fn draw_text_range(
    services: &mut super::data_services::DataServices,
    nodes: &BTreeMap<u32, super::SceneNode>,
    output: &mut Canvas,
    lower: f32,
    upper: f32,
) -> Result<()> {
    let mut ids: Vec<_> = services.messages.objects().keys().copied().collect();
    ids.sort_by_key(|id| {
        nodes
            .get(id)
            .map(|node| (node.position[2] as i32, *id as i32))
    });
    for id in ids {
        let Some(node) = nodes.get(&id) else {
            continue;
        };
        if node.position[2] < lower
            || node.position[2] >= upper
            || !node.visible
            || !matches!(node.kind, 48 | 63)
        {
            continue;
        }
        if services
            .tasks
            .message_task_state(id)
            .is_none_or(|(kind, deletion, _)| kind != node.kind || deletion != 0)
        {
            continue;
        }
        let Some(object) = services.messages.object(id) else {
            continue;
        };
        services.message_device.sync_object(object)?;
        let Some(view) = services.message_device.surface(id as i32) else {
            continue;
        };
        let Some(configuration) = object.configuration() else {
            continue;
        };
        let Some((size, offset)) = object.registered_geometry() else {
            continue;
        };
        if node
            .position
            .iter()
            .chain(node.scale.iter())
            .any(|value| !value.is_finite())
        {
            bail!("offscreen message plane has nonfinite coordinates");
        }
        let shift = configuration.canvas_offset();
        let dx = i64::from(shift[0]) - i64::from(offset[0]);
        let dy = i64::from(shift[1]) - i64::from(offset[1]);
        let width = view.submitted.width();
        let height = view.submitted.height();
        let mut pixels = vec![0; view.submitted.pixels().len()];
        for y in 0..height {
            for x in 0..width {
                let sx = i64::from(x) - dx;
                let sy = i64::from(y) - dy;
                if sx < 0 || sy < 0 || sx >= i64::from(width) || sy >= i64::from(height) {
                    continue;
                }
                let from = (sy as usize * width as usize + sx as usize) * 4;
                let to = (y as usize * width as usize + x as usize) * 4;
                pixels[to..to + 4].copy_from_slice(&view.submitted.pixels()[from..from + 4]);
            }
        }
        super::message_backend::draw_message_plane(
            output,
            RgbaFrame::from_rgba8(width, height, pixels)?,
            node,
            offset,
            size,
        )?;
    }
    let mut ids: Vec<_> = services.messages.legacy.objects().keys().copied().collect();
    ids.sort_by_key(|id| {
        nodes
            .get(id)
            .map(|node| (node.position[2] as i32, *id as i32))
    });
    for id in ids {
        let Some(node) = nodes.get(&id) else {
            continue;
        };
        if node.position[2] < lower || node.position[2] >= upper {
            continue;
        }
        if services
            .tasks
            .message_task_state(id)
            .is_none_or(|(kind, deletion, _)| {
                kind != super::legacy_text_native::LEGACY_STRING_KIND || deletion != 0
            })
        {
            continue;
        }
        if let Some(object) = services.messages.legacy.object(id) {
            object.draw(output, node)?;
        }
    }
    Ok(())
}

/// Both inputs are XYWH; outputs are clipped, exclusive endpoints.
fn clipped_rectangle(rectangle: [i32; 4], bounds: [i32; 4]) -> Option<[i32; 4]> {
    let right = rectangle[0].wrapping_add(rectangle[2]);
    let bottom = rectangle[1].wrapping_add(rectangle[3]);
    let bounds_right = bounds[0].wrapping_add(bounds[2]);
    let bounds_bottom = bounds[1].wrapping_add(bounds[3]);
    let left = rectangle[0].max(bounds[0]);
    let top = rectangle[1].max(bounds[1]);
    let right = right.min(bounds_right);
    let bottom = bottom.min(bounds_bottom);
    (left < right && top < bottom).then_some([left, top, right, bottom])
}

fn zero_tile(canvas: &mut Canvas, x: u32, y: u32, size: [u32; 2]) {
    for row in y..y.saturating_add(size[1]).min(canvas.height()) {
        let left = x.min(canvas.width());
        let right = x.saturating_add(size[0]).min(canvas.width());
        let start = (row as usize * canvas.width() as usize + left as usize) * 4;
        let end = start + (right - left) as usize * 4;
        canvas.pixels_mut()[start..end].fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SceneNode;
    use crate::plane_native::{PlaneServices, draw_plane};

    #[test]
    fn ordinary_image_copy_preserves_top_down_rows_in_the_canvas_atlas() -> Result<()> {
        let source = RgbaFrame::from_rgba8(
            1,
            3,
            vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255],
        )?;
        let mut canvas = Canvas::new(1, 2)?;
        let mut overflow = None;
        let mut state = CanvasNativeState {
            initialized: true,
            enabled: true,
            active: true,
            size: [1, 2],
            tile: [1, 2],
            grid: [1, 2],
            tiles: vec![Some(Tile { depth: 32, dirty: false }); 2],
            ..CanvasNativeState::default()
        };
        // Crop off the first source row and copy across the viewport edge.
        state.copy_image(&mut canvas, &mut overflow, &source, [0, 1], [0, 1, 1, 2])?;
        assert_eq!(canvas.pixel(0, 1)?, Color::rgb(0, 255, 0));
        assert_eq!(overflow.unwrap().pixel(0, 2), Some(Color::rgb(0, 0, 255)));
        Ok(())
    }

    #[test]
    fn list_row_copies_match_scalar_writes() -> Result<()> {
        let make_state = || CanvasNativeState {
            initialized: true,
            enabled: true,
            active: true,
            size: [5, 4],
            tile: [4, 3],
            grid: [3, 3],
            tiles: (0..9)
                .map(|index| {
                    (index != 4).then_some(Tile {
                        depth: if index % 2 == 0 { 16 } else { 32 },
                        dirty: false,
                    })
                })
                .collect(),
            ..CanvasNativeState::default()
        };
        let transparent = RgbaFrame::from_rgba8(13, 10, vec![0; 13 * 10 * 4])?;
        let below_quantization = RgbaFrame::from_rgba8(13, 10, vec![15; 13 * 10 * 4])?;
        let pattern = RgbaFrame::from_rgba8(
            13,
            10,
            (0..13 * 10 * 4).map(|index| (index * 37) as u8).collect(),
        )?;
        let test_cases = [
            (&below_quantization, Rect::new(8, 0, 4, 3), [0, 0]),
            (&pattern, Rect::new(-2, -3, 17, 15), [-2, -1]),
            (&pattern, Rect::new(0, 0, 11, 8), [3, 2]),
            (&transparent, Rect::new(1, 2, 11, 10), [1, 1]),
            (&pattern, Rect::new(0, 0, 13, 10), [i32::MAX, i32::MIN]),
        ];

        for (mut expected_state, mut actual_state) in [
            (make_state(), make_state()),
            (CanvasNativeState::default(), CanvasNativeState::default()),
        ] {
            let mut expected = Canvas::new(5, 4)?;
            expected.clear(Color::rgba(12, 34, 56, 78));
            let mut actual = expected.clone();
            let mut expected_overflow = None;
            let mut actual_overflow = None;
            let [_, _, width, height] = expected_state.storage_bounds(&expected);
            for (source, rect, [offset_x, offset_y]) in test_cases {
                for tile in expected_state.tiles.iter_mut().flatten() {
                    tile.dirty = false;
                }
                for tile in actual_state.tiles.iter_mut().flatten() {
                    tile.dirty = false;
                }
                let right = (rect.x + rect.width as i32).min(source.width() as i32);
                let bottom = (rect.y + rect.height as i32).min(source.height() as i32);
                for source_y in rect.y.max(0)..bottom {
                    for source_x in rect.x.max(0)..right {
                        let dest_x = source_x.wrapping_add(offset_x);
                        let dest_y = source_y.wrapping_add(offset_y);
                        if (0..width).contains(&dest_x) && (0..height).contains(&dest_y) {
                            expected_state.write_pixel(
                                &mut expected,
                                &mut expected_overflow,
                                dest_x,
                                dest_y,
                                source.pixel(source_x, source_y)?,
                            )?;
                        }
                    }
                }
                actual_state.copy_list_rectangle(
                    source,
                    &mut actual,
                    &mut actual_overflow,
                    rect,
                    [offset_x, offset_y],
                )?;

                assert_eq!(actual.pixels(), expected.pixels(), "{rect:?}");
                assert_eq!(actual_state.dirty_tiles(), expected_state.dirty_tiles(), "{rect:?}");
                assert_eq!(
                    actual_overflow.as_ref().map(|atlas| &atlas.pixels),
                    expected_overflow.as_ref().map(|atlas| &atlas.pixels),
                    "{rect:?}",
                );
            }
        }
        Ok(())
    }

    #[test]
    fn configured_atlas_extent_is_independent_of_the_screen() {
        let xml = ConfigXml::parse_runtime(
            br#"<?xml version="1.0" encoding="UTF-8"?><document><TEXTURE><size><x>256</x><y>256</y></size><vt><x>10</x><y>14</y></vt></TEXTURE></document>"#,
        )
        .unwrap();
        let (tile, grid) = texture_geometry(Some(&xml), [1024, 576]).unwrap();
        assert_eq!(texture_extent(tile, grid).unwrap(), [2560, 3584]);
        let (tile, grid) = texture_geometry(None, [1024, 576]).unwrap();
        assert_eq!(texture_extent(tile, grid).unwrap(), [1024, 768]);
        assert!(texture_extent([256, 256], [100, 100]).is_err());
        assert!(texture_extent([u32::MAX, 256], [2, 2]).is_err());
    }

    #[test]
    fn fes_planes_sample_list_pixels_to_the_right_and_below_the_viewport() -> Result<()> {
        let logical = [8, 4];
        let tile = [4, 4];
        let grid = [4, 2];
        let [width, height] = texture_extent(tile, grid)?;
        let mut viewport_storage = Canvas::new(logical[0], logical[1])?;
        let mut overflow = None;
        let mut state = CanvasNativeState {
            initialized: true,
            enabled: true,
            active: true,
            size: logical,
            tile,
            grid,
            tiles: vec![Some(Tile { depth: 32, dirty: false }); 8],
            ..CanvasNativeState::default()
        };
        let red = Color::rgb(255, 0, 0);
        let green = Color::rgb(0, 255, 0);
        let mut list = Canvas::new(width, height)?;
        list.fill_rect(Rect::new(8, 0, 4, 4), red)?;
        list.fill_rect(Rect::new(0, 4, 4, 4), green)?;
        let list = RgbaFrame::from_rgba8(width, height, list.pixels().to_vec())?;
        let region = Rect::new(0, 0, width, height);
        state.copy_list_rectangle(&list, &mut viewport_storage, &mut overflow, region, [0; 2])?;
        let frame = |canvas: &Canvas| {
            RgbaFrame::from_rgba8(canvas.width(), canvas.height(), canvas.pixels().to_vec())
        };
        let legacy_frame = frame(&viewport_storage)?;
        let legacy = state.graph_snapshot(None)?;
        assert!(CanvasNativeState::prepare_graph_restore(&legacy, logical)?.1.is_none());
        let saved = state.graph_snapshot(overflow.as_ref())?;
        let (_, restored) = CanvasNativeState::prepare_graph_restore(&saved, logical)?;
        let mut node = SceneNode {
            id: 1,
            kind: 2,
            parent: 0,
            name: String::new(),
            size: [0.; 2],
            position: [0.; 3],
            scale: [1.; 3],
            color: u32::MAX,
            overlay_color: 0,
            visible: true,
            initialized: true,
            fes_block_name: Vec::new(),
            fes_object_prefix: Vec::new(),
            fes_bank: 0,
            fes_parameters: [0; 512],
        };
        let mut planes = PlaneServices::default();
        let mut words = [0; 36];
        words[9..25].copy_from_slice(&crate::movie_plane::IDENTITY.map(f32::to_bits));
        words[25..28].fill(1f32.to_bits());
        words[28] = u32::MAX;
        words[35] = 1;
        let mut output = Canvas::new(logical[0], logical[1])?;
        let mut restored = restored;
        for (source, expected) in [([8, 0], red), ([0, 4], green)] {
            planes.sync_fes_plane(&mut node, true, [4, 4], source, &words, 5000, true, None);
            output.clear(Color::TRANSPARENT);
            draw_plane(&mut output, &node, &planes, Some(&legacy_frame))?;
            assert_eq!(output.pixel(1, 1)?, Color::TRANSPARENT);
            planes.vram_overflow = restored.take();
            draw_plane(&mut output, &node, &planes, Some(&legacy_frame))?;
            restored = planes.vram_overflow.take();
            assert_eq!(output.pixel(1, 1)?, expected);
        }
        let atlas = restored.as_mut().unwrap();
        atlas.set(2, 9, 2, Color::rgba(80, 120, 160, 123));
        let mixed_source = [6, 2];
        let clip = Rect::new(1, 0, 6, 4);
        let seed = Color::rgba(20, 40, 60, 145);
        let mut expected = Canvas::new(logical[0], logical[1])?;
        expected.clear(seed);
        for y in 0..logical[1] {
            for x in 1..7 {
                let [sample_x, sample_y] = [mixed_source[0] as u32 + x, mixed_source[1] as u32 + y];
                let pixel = if sample_x < logical[0] && sample_y < logical[1] {
                    Some(viewport_storage.pixel(sample_x as i32, sample_y as i32)?)
                } else {
                    atlas.pixel(sample_x, sample_y)
                };
                if let Some(pixel) = pixel {
                    expected.blend_pixel_with_mode(x as i32, y as i32, pixel, cs2_renderer::BlendMode::SourceOver)?;
                }
            }
        }
        planes.sync_fes_plane(&mut node, true, logical, mixed_source, &words, 5000, true, Some(clip));
        planes.vram_overflow = restored.take();
        output.clear(seed);
        draw_plane(&mut output, &node, &planes, Some(&legacy_frame))?;
        assert_eq!(output.pixels(), expected.pixels());
        restored = planes.vram_overflow.take();
        assert_eq!(output.dimensions(), (8, 4));
        state.clear_rectangle(&mut viewport_storage, &mut restored, [8, 0, 4, 4])?;
        assert_eq!(restored.as_ref().unwrap().pixel(8, 0), Some(Color::TRANSPARENT));
        assert_eq!(restored.as_ref().unwrap().pixel(0, 4), Some(green));
        Ok(())
    }
}
