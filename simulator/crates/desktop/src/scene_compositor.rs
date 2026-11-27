use anyhow::Result;
use cs2_renderer::{Canvas, Color, ImageDrawOptions, Point, Rect, RgbaFrame};

const TILE_WIDTH: u32 = 128;
const TILE_HEIGHT: u32 = 64;
const CLEAR: Color = Color::rgba(0, 0, 0, 255);

#[derive(Clone)]
pub(super) struct SceneImage {
    pub(super) frame: RgbaFrame,
    pub(super) position: Point,
    pub(super) opacity: u8,
}

impl SceneImage {
    fn same_layer(&self, other: &Self) -> bool {
        self.position == other.position
            && self.opacity == other.opacity
            && self.bounds() == other.bounds()
            && std::ptr::eq(self.frame.pixels(), other.frame.pixels())
    }

    fn bounds(&self) -> Rect {
        Rect::new(self.position.x, self.position.y, self.frame.width(), self.frame.height())
    }

    fn draw(&self, target: &mut Canvas, clip: Option<Rect>) -> Result<()> {
        let mut options = ImageDrawOptions::new(self.position);
        options.opacity = self.opacity;
        options.clip = clip;
        target.draw_frame(&self.frame, &options)?;
        Ok(())
    }

    fn opaque_full_surface(&self, size: [u32; 2]) -> bool {
        self.opacity == 255
            && self.position == Point::new(0, 0)
            && [self.frame.width(), self.frame.height()] == size
            && self.frame.pixels().chunks_exact(4).all(|pixel| pixel[3] == 255)
    }
}

#[derive(Default)]
pub(super) struct SceneCompositor {
    prefix: Option<ImagePrefix>,
    pub(super) graphics_frame: Option<RgbaFrame>,
    pub(super) revision: Option<u64>,
    #[cfg(all(windows, feature = "gpu-raster"))]
    pub(super) back_buffer: Option<Canvas>,
    revision_counter: u64,
}

struct ImagePrefix {
    pixels: Canvas,
    layers: Vec<SceneImage>,
}

impl SceneCompositor {
    pub(super) fn clear_prefix(&mut self) {
        self.prefix = None;
    }

    pub(super) fn record_static(&mut self, previous: Option<u64>, unchanged: bool) {
        if unchanged && previous.is_some() {
            self.revision = previous;
        } else {
            self.revision_counter = self.revision_counter.wrapping_add(1);
            self.revision = Some(self.revision_counter);
        }
    }

    pub(super) fn compose_prefix(
        &mut self,
        target: &mut Canvas,
        layers: Vec<SceneImage>,
        allow_reuse: bool,
    ) -> Result<bool> {
        let size = [target.width(), target.height()];
        if self.prefix.as_ref().is_some_and(|cached| {
            [cached.pixels.width(), cached.pixels.height()] != size
        }) {
            self.clear_prefix();
        }
        let mut cached = match self.prefix.take() {
            Some(cached) => cached,
            None => {
                let mut pixels = Canvas::new(size[0], size[1])?;
                draw_full(&mut pixels, &layers, size)?;
                target.pixels_mut().copy_from_slice(pixels.pixels());
                self.prefix = Some(ImagePrefix { pixels, layers });
                return Ok(false);
            }
        };

        let columns = size[0].div_ceil(TILE_WIDTH);
        let rows = size[1].div_ceil(TILE_HEIGHT);
        let mut dirty = vec![false; (columns * rows) as usize];
        for index in 0..cached.layers.len().max(layers.len()) {
            let before = cached.layers.get(index);
            let after = layers.get(index);
            if before.zip(after).is_some_and(|(before, after)| before.same_layer(after)) {
                continue;
            }
            for layer in before.into_iter().chain(after) {
                mark_tiles(&mut dirty, columns, size, layer.bounds());
            }
        }

        let unchanged = !dirty.contains(&true);
        if !unchanged && dirty.iter().all(|tile| *tile) {
            draw_full(&mut cached.pixels, &layers, size)?;
        } else if !unchanged {
            for (index, _) in dirty.into_iter().enumerate().filter(|(_, dirty)| *dirty) {
                let left = index as u32 % columns * TILE_WIDTH;
                let top = index as u32 / columns * TILE_HEIGHT;
                let tile = Rect::new(left as i32, top as i32,
                    TILE_WIDTH.min(size[0] - left), TILE_HEIGHT.min(size[1] - top));
                cached.pixels.fill_rect(tile, CLEAR)?;
                for layer in &layers {
                    layer.draw(&mut cached.pixels, Some(tile))?;
                }
            }
        }

        let reused = unchanged && allow_reuse && target.pixels() == cached.pixels.pixels();
        if !reused {
            target.pixels_mut().copy_from_slice(cached.pixels.pixels());
        }
        cached.layers = layers;
        self.prefix = Some(cached);
        Ok(reused)
    }
}

fn draw_full(target: &mut Canvas, layers: &[SceneImage], size: [u32; 2]) -> Result<()> {
    let layers = if let Some(first) = layers.first().filter(|layer| layer.opaque_full_surface(size)) {
        target.pixels_mut().copy_from_slice(first.frame.pixels());
        &layers[1..]
    } else {
        target.clear(CLEAR);
        layers
    };
    for layer in layers {
        layer.draw(target, None)?;
    }
    Ok(())
}

fn mark_tiles(dirty: &mut [bool], columns: u32, size: [u32; 2], rect: Rect) {
    let left = i64::from(rect.x).max(0);
    let top = i64::from(rect.y).max(0);
    let right = (i64::from(rect.x) + i64::from(rect.width)).min(i64::from(size[0]));
    let bottom = (i64::from(rect.y) + i64::from(rect.height)).min(i64::from(size[1]));
    if left >= right || top >= bottom { return; }
    let first_column = left as u32 / TILE_WIDTH;
    let last_column = (right as u32 - 1) / TILE_WIDTH;
    for row in top as u32 / TILE_HEIGHT..=(bottom as u32 - 1) / TILE_HEIGHT {
        let row_start = row * columns;
        dirty[(row_start + first_column) as usize..=(row_start + last_column) as usize].fill(true);
    }
}

pub(super) fn graphics_source<'a>(
    graphics: &Canvas,
    snapshot: &'a mut Option<RgbaFrame>,
) -> Result<&'a RgbaFrame> {
    if snapshot.is_none() {
        *snapshot = Some(RgbaFrame::from_rgba8(
            graphics.width(), graphics.height(), graphics.pixels().to_vec(),
        )?);
    }
    Ok(snapshot.as_ref().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(size: [u32; 2], pixel: [u8; 4], position: Point, opacity: u8) -> SceneImage {
        SceneImage {
            frame: RgbaFrame::from_rgba8(size[0], size[1], pixel.repeat((size[0] * size[1]) as usize)).unwrap(),
            position,
            opacity,
        }
    }

    #[test]
    fn report_perf_prefix_pixels_match_full_redraw_across_tile_boundaries() {
        let size = [401, 201];
        let mut target = Canvas::new(size[0], size[1]).unwrap();
        let mut baseline = Canvas::new(size[0], size[1]).unwrap();
        let mut compositor = SceneCompositor::default();
        let background = image(size, [32, 48, 19, 255], Point::new(0, 0), 255);
        let overlap = image([7, 7], [96, 230, 129, 110], Point::new(125, 61), 200);
        let moving = image([8, 8], [198, 33, 113, 127], Point::new(121, 58), 255);
        let mut moved = moving.clone();
        moved.position = Point::new(128, 61);
        moved.opacity = 151;
        let cropped = image([8, 8], [199, 55, 80, 224], Point::new(-5, 199), 255);
        let steps = [
            vec![background.clone(), moving.clone(), overlap.clone()],
            vec![background.clone(), moved, overlap.clone()],
            vec![background.clone(), overlap, moving],
            vec![background, cropped.clone()],
            vec![cropped],
        ];
        for layers in steps {
            baseline.clear(CLEAR);
            for layer in &layers {
                layer.draw(&mut baseline, None).unwrap();
            }
            assert!(!compositor.compose_prefix(&mut target, layers.clone(), true).unwrap());
            assert_eq!(target.pixels(), baseline.pixels());
            assert!(compositor.compose_prefix(&mut target, layers.clone(), true).unwrap());
            target.clear(Color::rgb(23, 22, 21));
            assert!(!compositor.compose_prefix(&mut target, layers, true).unwrap());
            assert_eq!(target.pixels(), baseline.pixels());
        }
    }

    #[test]
    fn report_perf_graphics_snapshot_is_frozen_for_one_composition() {
        let mut graphics = Canvas::new(2, 1).unwrap();
        graphics.clear(Color::rgb(17, 27, 37));
        let mut source = None;
        let first = graphics_source(&graphics, &mut source).unwrap().clone();
        graphics.clear(Color::rgb(14, 24, 34));
        let second = graphics_source(&graphics, &mut source).unwrap();
        assert!(std::ptr::eq(first.pixels(), second.pixels()));
        let next_frame = graphics_source(&graphics, &mut None).unwrap().clone();
        assert_ne!(next_frame.pixels(), first.pixels());
    }
}
