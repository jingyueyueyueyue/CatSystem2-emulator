//! The retained native list uses local tiles, independent of current output
//! expansion/metadata. No ANM instructions or random draws run in this module.
use super::*;
use cs2_runtime::object_graph::{ImageResourceRegistrationSnapshot as Registration,
    ImageResourceSurfaceSnapshot as Surface};

type Area = [i32; 4];

fn intersect(a: Area, b: Area) -> Option<Area> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

fn union(a: Option<Area>, b: Option<Area>) -> Option<Area> {
    match (a, b) {
        (Some(a), Some(b)) => Some([a[0].min(b[0]), a[1].min(b[1]),
            a[2].max(b[2]), a[3].max(b[3])]),
        (a, b) => a.or(b),
    }
}

fn tiles(s: &Surface) -> Result<Vec<Area>> {
    if s.tile.contains(&0) || s.size.iter().any(|&v| v > i32::MAX as u32) {
        return Err(native_state_error("resource list has invalid size/tile geometry"));
    }
    if s.size.contains(&0) { return Ok(Vec::new()); }
    let columns = s.size[0].div_ceil(s.tile[0]);
    let rows = s.size[1].div_ceil(s.tile[1]);
    if u64::from(columns) * u64::from(rows) > MAX_ENTRIES as u64 {
        return Err(native_state_error("resource list tile count exceeds limit"));
    }
    if columns == 1 && rows == 1 {
        return Ok(vec![[0, 0, s.size[0] as i32, s.size[1] as i32]]);
    }
    let mut result = Vec::new();
    for row in 0..rows {
        for col in 0..columns {
            let x = col * s.tile[0];
            let y = row * s.tile[1];
            // 69c4c5/69c4f0: the native also uses tile WIDTH for row height.
            result.push([x as i32, y as i32,
                (x + s.tile[0].min(s.size[0] - x)) as i32,
                (y + s.tile[0].min(s.size[1] - y)) as i32]);
        }
    }
    Ok(result)
}

fn placement(layer: &ResourceLayer, r: &Registration) -> [i32; 2] {
    let m = layer.frames[&layer.selected].metadata();
    let d = layer.animation.display();
    let p = if layer.animation.timeline().is_some() { [d.x, d.y] } else { [0; 2] };
    // position is the effective anchor: position + live offset represents
    // the native fixed position + cached registration offset (6a1080).
    // Static source blur compensates this anchor below; ANM selection instead
    // refreshes the native offset along with size/pivot (6a03e0).
    [m.offset[0].wrapping_add(r.position[0]).wrapping_add(p[0]).wrapping_sub(r.pivot[0]),
        m.offset[1].wrapping_add(r.position[1]).wrapping_add(p[1]).wrapping_sub(r.pivot[1])]
}

fn bounds(layer: &ResourceLayer, r: &Registration, size: [u32; 2]) -> Option<Area> {
    if layer.frames[&layer.selected].is_empty() { return None; }
    if layer.animation.timeline().is_some() && layer.animation.display().visible == 0 { return None; }
    let p = placement(layer, r);
    let right = (i64::from(p[0]) + i64::from(r.size[0])).min(i64::from(size[0]));
    let bottom = (i64::from(p[1]) + i64::from(r.size[1])).min(i64::from(size[1]));
    let r = [p[0].max(0), p[1].max(0), right.max(0) as i32, bottom.max(0) as i32];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

pub(super) fn create(layers: &[ResourceLayer], geometry: &PlanGeometry,
    tile: Option<[u32; 2]>) -> Result<Option<Surface>> {
    if layers.len() == 1 && layers[0].animation.timeline().is_none() { return Ok(None); }
    if layers.is_empty() { return Ok(None); }
    let b = geometry.bounds.unwrap_or([0; 4]);
    let size = [(i64::from(b[2]) - i64::from(b[0])) as u32,
        (i64::from(b[3]) - i64::from(b[1])) as u32];
    let mut s = Surface { size, tile: tile.unwrap_or(size.map(|axis| axis.max(1))),
        registrations: Vec::new(), dirty: Vec::new() };
    for layer in layers {
        let frame = &layer.frames[&layer.selected];
        let pivot = frame.metadata().base;
        let mut r = Registration {
            position: [pivot[0].wrapping_sub(b[0]), pivot[1].wrapping_sub(b[1])],
            pivot, size: [frame.width(), frame.height()], history: None,
        };
        r.history = bounds(layer, &r, size);
        s.registrations.push(r);
    }
    s.dirty.resize(tiles(&s)?.len(), None);
    Ok(Some(s))
}

/// 69a900 invalidates static registrations without refreshing any geometry.
/// Their cached offset must survive the live source wrapper's second border
/// expansion. Encode that difference in the existing effective anchor, so
/// placement/history/drawing and snapshots all retain the same coordinates.
/// No frame, ANM cursor, random state or wire layout is replaced here.
pub(super) fn invalidate_blurred_source(s: &mut Surface, layer: &ResourceLayer,
    index: usize, previous_offset: [i32; 2]) -> Result<()> {
    let animated = layer.animation.timeline().is_some();
    if !animated {
        let current_offset = layer.frames[&layer.selected].metadata().offset;
        let r = &mut s.registrations[index];
        for axis in 0..2 {
            r.position[axis] = r.position[axis].wrapping_add(previous_offset[axis])
                .wrapping_sub(current_offset[axis]);
        }
    }
    invalidate(s, layer, index, animated)
}

/// Refreshing an ANM selected wrapper updates size/pivot and consumes its live
/// offset. Static registrations retain their cached geometry after a blur.
pub(super) fn invalidate(s: &mut Surface, layer: &ResourceLayer, index: usize,
    selected: bool) -> Result<()> {
    let r = &mut s.registrations[index];
    if selected {
        let frame = &layer.frames[&layer.selected];
        r.size = [frame.width(), frame.height()];
        r.pivot = frame.metadata().base;
    }
    r.history = union(r.history, bounds(layer, r, s.size));
    let history = r.history;
    let t = tiles(s)?;
    for (dirty, tile) in s.dirty.iter_mut().zip(t) {
        *dirty = union(*dirty, history.and_then(|r| intersect(r, tile)));
    }
    Ok(())
}

pub(super) fn validate(s: &Surface, layers: &[ResourceLayer], geometry: &PlanGeometry) -> Result<()> {
    let b = geometry.bounds.unwrap_or([0; 4]);
    if s.size != [(i64::from(b[2]) - i64::from(b[0])) as u32,
        (i64::from(b[3]) - i64::from(b[1])) as u32]
        || s.registrations.len() != layers.len() {
        return Err(native_state_error("saved resource list registration/directory mismatch"));
    }
    let t = tiles(s)?;
    if t.len() != s.dirty.len() || s.dirty.iter().zip(t).any(|(r, t)|
        r.is_some_and(|r| intersect(r, t) != Some(r)))
        || s.registrations.iter().any(|r| r.size.iter().any(|&v| v > i32::MAX as u32)
            || r.history.is_some_and(|r| intersect(r, [0, 0, s.size[0] as i32, s.size[1] as i32]) != Some(r))) {
        return Err(native_state_error("saved resource list history/tile bounds are invalid"));
    }
    Ok(())
}

pub(super) fn redraw(layers: &[ResourceLayer], s: &Surface, current: &RgbaFrame) -> Result<RgbaFrame> {
    let _timing = crate::measure_host_phase(crate::HostPhase::Compose);
    let mut target = LayerComposition {
        width: current.width(), height: current.height(), pixels: current.pixels().to_vec(),
        metadata: current.metadata(), attributes: current.hg_attributes().as_map().clone(),
    };
    // Clear/draw in ORIGINAL list-local coordinates. Current offset and its
    // larger dimensions are not the list origin; expansion must survive.
    for r in s.dirty.iter().flatten() {
        let clipped = intersect(*r, [0, 0, target.width as i32, target.height as i32]);
        if let Some(r) = clipped {
            for y in r[1]..r[3] {
                let start = (y as usize * target.width as usize
                    + r[0] as usize) * 4;
                target.pixels[start..start + (r[2] - r[0]) as usize * 4].fill(0);
            }
        }
    }
    for (layer, registration) in layers.iter().zip(&s.registrations) {
        let animated = layer.animation.timeline().is_some();
        let d = layer.animation.display();
        if animated && d.visible == 0 { continue; }
        for r in s.dirty.iter().flatten() {
            target.draw_animation_at(&layer.frames[&layer.selected], placement(layer, registration),
                if animated { d.compose((0., 0.), 255, 1).blend as u32 } else { 255 }, Some(*r))?;
        }
    }
    target.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: [u8; 4] = [20, 40, 60, 255];
    const UPPER: [u8; 4] = [200, 0, 0, 255];
    const LOWER: [u8; 4] = [0, 200, 0, 255];

    fn layer(offset: [i32; 2], width: u32, rows: &[[u8; 4]]) -> ResourceLayer {
        let pixels = rows.iter().flat_map(|pixel| pixel.repeat(width as usize)).collect();
        let frame = RgbaFrame::from_rgba8(width, rows.len() as u32, pixels).unwrap();
        let metadata = FrameMetadata { offset, base: [4, 9], ..frame.metadata() };
        ResourceLayer::new(BTreeMap::from([(0, frame.with_raw_metadata(metadata))]),
            Default::default()).unwrap()
    }

    fn pixel(frame: &RgbaFrame, x: usize, y: usize) -> [u8; 4] {
        let start = (y * frame.width() as usize + x) * 4;
        frame.pixels()[start..start + 4].try_into().unwrap()
    }

    #[test]
    fn resource_top_origin_static_selector_preserves_crop_and_row_order() {
        let layers = [layer([2, 5], 4, &[BODY; 6]), layer([3, 6], 2, &[UPPER, LOWER])];
        let geometry = ResourceLayers::directory_geometry(&layers).unwrap();
        let mut target = LayerComposition::new(geometry).unwrap().unwrap();
        for source in &layers {
            target.draw(&source.frames[&0]).unwrap();
        }
        let frame = target.finish().unwrap();
        assert_eq!(frame.metadata().offset, [2, 5]);
        assert_eq!(pixel(&frame, 1, 1), UPPER);
        assert_eq!(pixel(&frame, 1, 2), LOWER);
        assert_eq!(pixel(&frame, 1, 4), BODY);
    }

    #[test]
    fn resource_top_origin_redraw_clears_old_rows_and_clips_across_tiles() {
        let layers = ResourceLayers::new(vec![layer([2, 5], 4, &[[0; 4]; 6]),
            layer([3, 6], 2, &[UPPER, LOWER])]).unwrap();
        let initial = layers.compose().unwrap().unwrap();
        assert_eq!(pixel(&initial, 1, 1), UPPER);
        assert_eq!(pixel(&initial, 1, 2), LOWER);

        let mut surface = create(&layers.layers, &layers.geometry, Some([2, 2])).unwrap().unwrap();
        surface.registrations[1].position[1] += 3;
        invalidate(&mut surface, &layers.layers[1], 1, false).unwrap();
        assert!(surface.dirty.iter().all(Option::is_some));

        let mut pixels = initial.pixels().to_vec();
        pixels[..4].copy_from_slice(&BODY);
        let current = RgbaFrame::from_rgba8(initial.width(), initial.height(), pixels)
            .unwrap().with_raw_metadata(initial.metadata());
        let redrawn = redraw(&layers.layers, &surface, &current).unwrap();
        assert_eq!(pixel(&redrawn, 0, 0), BODY);
        for x in [1, 2] {
            assert_eq!(pixel(&redrawn, x, 1)[3], 0);
            assert_eq!(pixel(&redrawn, x, 2)[3], 0);
            assert_eq!(pixel(&redrawn, x, 4), UPPER);
            assert_eq!(pixel(&redrawn, x, 5), LOWER);
        }
    }
}
