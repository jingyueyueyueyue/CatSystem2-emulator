//! Image-list history and ordered dirty tiles. Tile rectangles deliberately
//! use the horizontal extent for their height, retaining overlapping rows.
use super::{Layer, LoggingHost};
use anyhow::{Context, Result, bail};
use cs2_renderer::{Canvas, FrameMetadata, ImageDrawOptions, Point, Rect, RgbaFrame};
use std::collections::{BTreeMap, BTreeSet};

type Key = (u32, String);

use cs2_runtime::graph_serde::{Deserialize, Serialize};
use super::snapshots::{Options, Rectangle, options, rect, unoptions, unrect};

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct SurfaceSnapshot {
    pub(super) size: [u32; 2],
    tiles: Vec<(Rectangle, Option<Rectangle>)>,
    history: BTreeMap<Key, HistorySnapshot>,
    intermediate: BTreeMap<Key, Option<Rectangle>>,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct HistorySnapshot {
    bounds: Option<Rectangle>, frame: cs2_runtime::object_graph::ImageFrameSnapshot,
    options: Options, priority: i32,
}

struct Tile {
    bounds: Rect,
    dirty: Option<Rect>,
}
struct History {
    bounds: Option<Rect>,
    frame: RgbaFrame,
    options: ImageDrawOptions,
    priority: i32,
    affine: Option<super::AffinePlacement>,
}

pub(super) struct ListSurface {
    size: [u32; 2],
    tiles: Vec<Tile>,
    history: BTreeMap<Key, History>,
    intermediate: BTreeMap<Key, Option<Rect>>,
}

impl ListSurface {
    pub(super) fn snapshot(&self) -> SurfaceSnapshot {
        SurfaceSnapshot {
            size: self.size,
            tiles: self.tiles.iter().map(|t| (rect(t.bounds), t.dirty.map(rect))).collect(),
            history: self.history.iter().map(|(key, h)| (key.clone(), HistorySnapshot {
                bounds: h.bounds.map(rect), frame: crate::image_manager::capture_frame(&h.frame),
                options: options(&h.options, h.affine), priority: h.priority,
            })).collect(),
            intermediate: self.intermediate.iter().map(|(key, area)| (key.clone(), area.map(rect))).collect(),
        }
    }

    pub(super) fn from_snapshot(s: &SurfaceSnapshot) -> Result<Self> {
        if s.tiles.len() > 1_000_000 || s.history.len() > 1_000_000
            || s.intermediate.len() > 1_000_000 {
            bail!("saved image-list tile/history count exceeds limit");
        }
        let tiles = s.tiles.iter().map(|(bounds, dirty)| {
            let bounds = unrect(bounds);
            if bounds.x < 0 || bounds.y < 0
                || bounds.x as u64 + u64::from(bounds.width) > u64::from(s.size[0])
                || bounds.y as u64 + u64::from(bounds.height) > u64::from(s.size[1]) {
                bail!("saved image-list tile lies outside its surface");
            }
            let dirty = dirty.as_ref().map(unrect);
            if dirty.is_some_and(|d| intersection(bounds, d) != Some(d)) {
                bail!("saved image-list dirty area lies outside its tile");
            }
            Ok(Tile { bounds, dirty })
        }).collect::<Result<_>>()?;
        let history = s.history.iter().map(|(key, h)| Ok((key.clone(), History {
            bounds: h.bounds.as_ref().map(unrect),
            frame: crate::image_manager::restore_frame(&h.frame)?,
            options: unoptions(&h.options)?, priority: h.priority, affine: h.options.affine,
        }))).collect::<Result<_>>()?;
        Ok(Self { size: s.size, tiles, history, intermediate: s.intermediate.iter()
            .map(|(key, area)| (key.clone(), area.as_ref().map(unrect))).collect() })
    }

    fn new(size: [u32; 2], tile: [u32; 2]) -> Result<Self> {
        if tile.contains(&0) || size.iter().any(|&axis| axis > i32::MAX as u32) {
            bail!("list tile dimensions must be positive");
        }
        if size.contains(&0) {
            return Ok(Self { size, tiles: Vec::new(), history: BTreeMap::new(),
                intermediate: BTreeMap::new() });
        }
        let columns = size[0].div_ceil(tile[0]);
        let rows = size[1].div_ceil(tile[1]);
        if u64::from(columns) * u64::from(rows) > 1_000_000 {
            bail!("list tile count exceeds one million");
        }
        let mut tiles = Vec::new();
        for row in 0..rows {
            for column in 0..columns {
                let x = column * tile[0];
                let y = row * tile[1];
                let bounds = Rect::new(
                    x as i32,
                    y as i32,
                    tile[0].min(size[0] - x),
                    tile[0].min(size[1] - y),
                );
                tiles.push(Tile {
                    bounds,
                    dirty: Some(bounds),
                });
            }
        }
        Ok(Self {
            size,
            tiles,
            history: BTreeMap::new(),
            intermediate: BTreeMap::new(),
        })
    }

    pub(super) fn invalidate_all(&mut self) {
        for tile in &mut self.tiles {
            tile.dirty = Some(tile.bounds);
        }
    }

    pub(super) fn resize(&mut self, size: [u32; 2], tile: [u32; 2]) -> Result<()> {
        let replacement = Self::new(size, tile)?;
        self.size = replacement.size;
        self.tiles = replacement.tiles;
        Ok(())
    }

    pub(super) fn clear_clip(&mut self, key: u32) {
        if let Some(history) = self
            .history
            .get_mut(&(u32::MAX, format!("\0list-native:{key:08x}")))
        {
            history.options.clip = None;
        }
    }

    pub(super) fn registration(&mut self, key: u32, bounds: Option<Rect>) {
        let key = (u32::MAX, format!("\0list-native:{key:08x}"));
        let previous = self.intermediate.remove(&key).flatten();
        self.intermediate.insert(key, union(previous, bounds));
    }

    fn dirty(&mut self, rectangle: Option<Rect>) {
        let Some(rectangle) = rectangle else {
            return;
        };
        for tile in &mut self.tiles {
            if let Some(area) = intersection(tile.bounds, rectangle) {
                tile.dirty = Some(union(tile.dirty, Some(area)).unwrap());
            }
        }
    }
}

fn intersection(a: Rect, b: Rect) -> Option<Rect> {
    let left = i64::from(a.x).max(i64::from(b.x));
    let top = i64::from(a.y).max(i64::from(b.y));
    let right = (i64::from(a.x) + i64::from(a.width)).min(i64::from(b.x) + i64::from(b.width));
    let bottom = (i64::from(a.y) + i64::from(a.height)).min(i64::from(b.y) + i64::from(b.height));
    (right > left && bottom > top).then(|| {
        Rect::new(
            left as i32,
            top as i32,
            (right - left) as u32,
            (bottom - top) as u32,
        )
    })
}

fn union(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => {
            let x = a.x.min(b.x);
            let y = a.y.min(b.y);
            let right =
                (i64::from(a.x) + i64::from(a.width)).max(i64::from(b.x) + i64::from(b.width));
            let bottom =
                (i64::from(a.y) + i64::from(a.height)).max(i64::from(b.y) + i64::from(b.height));
            Some(Rect::new(
                x,
                y,
                (right - i64::from(x)) as u32,
                (bottom - i64::from(y)) as u32,
            ))
        }
    }
}

fn options_equal(a: &ImageDrawOptions, b: &ImageDrawOptions) -> bool {
    a.position == b.position
        && a.scale_x.to_bits() == b.scale_x.to_bits()
        && a.scale_y.to_bits() == b.scale_y.to_bits()
        && a.opacity == b.opacity
        && a.visible == b.visible
        && a.clip == b.clip
        && a.mask == b.mask
        && a.pivot.map(f32::to_bits) == b.pivot.map(f32::to_bits)
        && a.rotation_radians.to_bits() == b.rotation_radians.to_bits()
        && a.flip_x == b.flip_x
        && a.flip_y == b.flip_y
        && a.blend_mode == b.blend_mode
        && a.filter == b.filter
}

fn same_render_frame(left: &RgbaFrame, right: &RgbaFrame) -> bool {
    left.width() == right.width()
        && left.height() == right.height()
        && std::ptr::eq(left.pixels(), right.pixels())
        && left.metadata() == right.metadata()
        && left.hg_attributes() == right.hg_attributes()
}

fn bounds(
    host: &LoggingHost<'_>,
    id: u32,
    key: &Key,
    layer: &Layer,
    size: [u32; 2],
) -> Result<Option<Rect>> {
    if layer.frame.is_empty() { return Ok(None); }
    if let Some(placement) = layer.affine {
        return super::affine_bounds([layer.frame.width(), layer.frame.height()], placement,
            size, layer.options.clip);
    }
    if key.0 == u32::MAX
        && let Some(number) = key
            .1
            .strip_prefix("\0list-native:")
            .and_then(|text| u32::from_str_radix(text, 16).ok())
        && let Some(entry) = host.data_services.list_native.entries.get(&(id, number))
    {
        return super::entry_bounds(super::source_geometry(host, &entry, &layer.frame)?,
            &entry, entry.base, size, layer.options.clip);
    }
    let area = if layer.options.rotation_radians != 0.
        || layer.options.scale_x != 1.
        || layer.options.scale_y != 1.
    {
        Rect::new(0, 0, size[0], size[1])
    } else {
        Rect::new(
            layer.options.position.x,
            layer.options.position.y,
            layer.frame.width(),
            layer.frame.height(),
        )
    };
    let Some(area) = intersection(area, Rect::new(0, 0, size[0], size[1])) else {
        return Ok(None);
    };
    Ok(match layer.options.clip {
        Some(clip) => intersection(area, clip),
        None => Some(area),
    })
}

pub(crate) fn compose(
    host: &mut LoggingHost<'_>,
    id: u32,
    force: bool,
    clip: Option<Rect>,
) -> Result<()> {
    let _timing = crate::measure_script_detail(crate::HostPhase::ScriptList);
    super::prepare_layers(host, id)?;
    let list = host
        .data_services
        .tasks
        .fes
        .platform
        .lists
        .get(&id)
        .context("image-list target is not initialized")?;
    let size = list.size;
    let mut surface = match host.data_services.list_native.surfaces.remove(&id) {
        Some(surface) if surface.size == size => surface,
        _ => {
            host.ensure_canvas_state()?;
            let tile = host
                .data_services
                .canvas_native
                .tile_size()
                .unwrap_or(size.map(|value| value.max(1)));
            ListSurface::new(size, tile)?
        }
    };
    let result = compose_inner(host, id, force, clip, &mut surface);
    host.data_services.list_native.surfaces.insert(id, surface);
    result
}

fn compose_inner(
    host: &mut LoggingHost<'_>,
    id: u32,
    force: bool,
    clip: Option<Rect>,
    surface: &mut ListSurface,
) -> Result<()> {
    let list = &host.data_services.tasks.fes.platform.lists[&id];
    let mut present = BTreeSet::new();
    for (key, layer) in &list.layers {
        present.insert(key.clone());
        let native = key.0 == u32::MAX;
        let number = key
            .1
            .strip_prefix("\0list-native:")
            .and_then(|text| u32::from_str_radix(text, 16).ok());
        let explicit = number.is_some_and(|key| {
            host.data_services
                .list_native
                .invalidated
                .contains(&(id, key))
        });
        let current = bounds(host, id, key, layer, surface.size)?;
        let old = surface.history.remove(key);
        let changed = explicit
            || old.as_ref().is_none_or(|old| {
                !native
                    && (!same_render_frame(&old.frame, &layer.frame)
                        || !options_equal(&old.options, &layer.options)
                        || old.affine != layer.affine
                        || old.priority != layer.priority)
            });
        let history = if changed {
            let previous = old.as_ref().and_then(|old| old.bounds);
            if old.as_ref().is_some_and(|old| old.options.visible) {
                surface.dirty(previous);
            }
            let intermediate = surface.intermediate.remove(key).flatten();
            let accumulated = union(union(previous, intermediate), current);
            let accumulated = match layer.options.clip {
                Some(clip) => accumulated.and_then(|area| intersection(area, clip)),
                None => accumulated,
            };
            if layer.options.visible {
                surface.dirty(accumulated);
            }
            accumulated
        } else {
            old.as_ref().and_then(|old| old.bounds)
        };
        surface.history.insert(
            key.clone(),
            History {
                bounds: history,
                frame: layer.frame.clone(),
                options: layer.options.clone(),
                priority: layer.priority,
                affine: layer.affine,
            },
        );
    }
    let removed: Vec<_> = surface
        .history
        .keys()
        .filter(|key| !present.contains(*key))
        .cloned()
        .collect();
    for key in removed {
        let intermediate = surface.intermediate.remove(&key).flatten();
        if let Some(old) = surface.history.remove(&key)
            && old.options.visible
        {
            surface.dirty(union(old.bounds, intermediate));
        }
    }
    if force {
        surface.invalidate_all();
    }
    let dirty: Vec<_> = surface.tiles.iter().filter_map(|tile| tile.dirty).collect();
    let areas: Vec<_> = match clip {
        Some(clip) => dirty
            .iter()
            .filter_map(|area| intersection(*area, clip))
            .collect(),
        None => dirty.clone(),
    };
    let old = host
        .image_handles
        .get(&list.output)
        .context("list output wrapper disappeared")?;
    if surface.size.contains(&0) {
        // No Canvas allocation or sampling is needed, but keep the bound
        // output identity, entry histories, owners and animation cursors.
        if [old.width(), old.height()] != surface.size || !old.is_empty() {
            bail!("empty list output dimensions differ from its surface");
        }
        let submit = list.submit || force;
        if clip.is_none() {
            host.data_services.list_native.invalidated.retain(|(list, _)| *list != id);
        }
        if submit {
            host.data_services.tasks.fes.platform.lists.get_mut(&id).unwrap().force = false;
        }
        return Ok(());
    }
    let _raster_timing = crate::measure_script_detail(crate::HostPhase::ScriptListRaster);
    let (handle, offset, submit, forced) = (
        list.output,
        list.offset,
        list.submit || force,
        list.force || force,
    );
    let same_size = [old.width(), old.height()] == surface.size;
    if dirty.is_empty()
        && !(submit && forced)
        && (!submit || host.canvas.is_some())
        && same_size
        && old.metadata() == FrameMetadata::raster(surface.size)
        && old.hg_attributes() == &Default::default()
        && host.data_services.image_wrapper_pixels.get(&handle).is_none_or(|pixel| {
            !host.data_services.image_wrapper_pixels.iter().any(|(&wrapper, candidate)| {
                wrapper != handle && candidate == pixel && host.image_handles.contains_key(&wrapper)
            })
        })
    {
        if clip.is_none() {
            host.data_services.list_native.invalidated.retain(|(list, _)| *list != id);
        }
        return Ok(());
    }
    let mut canvas = if same_size {
        Canvas::from_rgba8(surface.size[0], surface.size[1], old.pixels().to_vec())?
    } else {
        let mut canvas = Canvas::new(surface.size[0], surface.size[1])?;
        canvas.copy_frame(old, &ImageDrawOptions::new(Point::new(0, 0)))?;
        canvas
    };
    for area in &areas {
        for y in area.y..area.y + area.height as i32 {
            let start = (y as usize * canvas.width() as usize + area.x as usize) * 4;
            canvas.pixels_mut()[start..start + area.width as usize * 4].fill(0);
        }
    }
    let mut layers: Vec<_> = list.layers.iter().collect();
    layers.sort_by(|(ak, a), (bk, b)| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| super::compare_keys(ak, bk))
    });
    for (key, layer) in layers {
        let mut options = layer.options.clone();
        let mut opaque = None;
        for area in &areas {
            let effective = match layer.options.clip {
                Some(clip) => intersection(*area, clip),
                None => Some(*area),
            };
            let Some(clip) = effective else {
                continue;
            };
            options.clip = Some(clip);
            super::draw_layer(host, id, &mut canvas, key, &layer.frame, &options, layer.affine, &mut opaque)?;
        }
    }
    let output = RgbaFrame::from_rgba8(canvas.width(), canvas.height(), canvas.pixels().to_vec())?;
    host.replace_image_wrapper_pixels(handle, output)?;
    if submit {
        let output = &host.image_handles[&handle];
        let canvas = host
            .canvas
            .as_mut()
            .context("list submission needs an actual canvas")?;
        let overflow = &mut host.data_services.planes.vram_overflow;
        if forced {
            host.data_services.canvas_native.copy_list_rectangle(
                output,
                canvas,
                overflow,
                Rect::new(0, 0, output.width(), output.height()),
                offset,
            )?;
        } else {
            for area in &dirty {
                host.data_services
                    .canvas_native
                    .copy_list_rectangle(output, canvas, overflow, *area, offset)?;
            }
        }
    }
    if clip.is_none() {
        for tile in &mut surface.tiles {
            tile.dirty = None;
        }
        host.data_services
            .list_native
            .invalidated
            .retain(|(list, _)| *list != id);
    }
    if submit && (!surface.tiles.is_empty() || forced) {
        host.data_services
            .tasks
            .fes
            .platform
            .lists
            .get_mut(&id)
            .unwrap()
            .force = false;
    }
    Ok(())
}
