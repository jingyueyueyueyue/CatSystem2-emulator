use std::{
    cell::RefCell,
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock, Weak},
};

use super::{
    Canvas, Color, FramePixels, ImageDrawOptions, Point, RgbaFrame, ScaleFilter, blend_into,
    modulate_alpha, sample_frame,
};

type Bounds = (u32, u32, u32, u32);

pub(super) fn draw_axis_aligned(
    canvas: &mut Canvas,
    frame: &RgbaFrame,
    options: &ImageDrawOptions,
    bounds: Bounds,
) {
    let (left, top, right, bottom) = bounds;
    let source_x = |x| (f64::from(x) - f64::from(options.position.x)) / f64::from(options.scale_x);
    let source_y = |y| (f64::from(y) - f64::from(options.position.y)) / f64::from(options.scale_y);
    if options.filter == ScaleFilter::Linear {
        draw_sampled(canvas, frame, options, bounds, false, 1, |x, y| {
            Some(sample_frame(
                frame,
                source_x(x),
                source_y(y),
                ScaleFilter::Linear,
            ))
        });
        return;
    }

    let columns: Vec<_> = (left..right)
        .map(|x| (source_x(x) as u32).min(frame.width - 1) as usize)
        .collect();
    let source_stride = frame.width as usize * 4;
    let stride = canvas.width as usize * 4;
    for (y, target) in (top..bottom).zip(
        canvas.pixels[top as usize * stride..bottom as usize * stride].chunks_exact_mut(stride),
    ) {
        let offset = (source_y(y) as u32).min(frame.height - 1) as usize * source_stride;
        let source = frame.pixels[offset..offset + source_stride]
            .as_chunks::<4>()
            .0;
        let target = target[left as usize * 4..right as usize * 4]
            .as_chunks_mut::<4>()
            .0;
        let mask = row_mask(options, bounds, stride, y, false);
        for (column, (target, &source_x)) in target.iter_mut().zip(&columns).enumerate() {
            let [red, green, blue, alpha] = source[source_x];
            composite(
                target,
                Color::rgba(red, green, blue, alpha),
                options,
                mask,
                column,
                false,
            );
        }
    }
}

pub(super) fn draw_sampled(
    canvas: &mut Canvas,
    frame: &RgbaFrame,
    options: &ImageDrawOptions,
    bounds: Bounds,
    is_transformed: bool,
    workers: usize,
    sample: impl Fn(u32, u32) -> Option<Color> + Sync,
) {
    let (left, top, right, bottom) = bounds;
    let columns = (right - left) as usize;
    let rows = (bottom - top) as usize;
    let stride = canvas.width as usize * 4;
    let target = &mut canvas.pixels[top as usize * stride..bottom as usize * stride];
    let list_alpha = options.blend_mode.is_list_alpha_mask();
    let cached = if list_alpha {
        None
    } else {
        sampled_pixels(frame, options, bounds, |pixels| {
            for (row, y) in pixels.chunks_exact_mut(columns).zip(top..bottom) {
                for (pixel, x) in row.iter_mut().zip(left..right) {
                    *pixel = sample(x, y).unwrap_or(Color::TRANSPARENT);
                }
            }
        })
    };
    let draw_row = |row: &mut [u8], y: u32| {
        let target = row[left as usize * 4..right as usize * 4]
            .as_chunks_mut::<4>()
            .0;
        let mask = row_mask(options, bounds, stride, y, is_transformed);
        if let Some(pixels) = &cached {
            let offset = (y - top) as usize * columns;
            for (column, (target, source)) in target
                .iter_mut()
                .zip(&pixels[offset..offset + columns])
                .enumerate()
            {
                composite(target, *source, options, mask, column, is_transformed);
            }
        } else {
            for (column, (target, x)) in target.iter_mut().zip(left..right).enumerate() {
                if let Some(source) = sample(x, y) {
                    composite(target, source, options, mask, column, is_transformed);
                }
            }
        }
    };
    let workers = if cached.is_none()
        && options.filter == ScaleFilter::Linear
        && columns * rows >= 512 * 1024
        && rows >= 64
    {
        workers.clamp(1, 4)
    } else {
        1
    };
    draw_pixel_rows(target, stride, top, workers, draw_row);
}

fn row_mask(
    options: &ImageDrawOptions,
    (left, _, right, _): Bounds,
    stride: usize,
    y: u32,
    transformed: bool,
) -> Option<&[u8]> {
    if !transformed && options.blend_mode.is_list_alpha_mask() {
        return None;
    }
    options.mask.as_ref().map(|mask| {
        let offset = y as usize * (stride / 4);
        &mask.alpha[offset + left as usize..offset + right as usize]
    })
}

#[inline]
fn composite(
    target: &mut [u8; 4],
    mut source: Color,
    options: &ImageDrawOptions,
    mask: Option<&[u8]>,
    column: usize,
    transformed: bool,
) {
    let list_alpha = options.blend_mode.is_list_alpha_mask();
    if source.a == 0 && !list_alpha {
        return;
    }
    if !list_alpha || transformed {
        source.a = modulate_alpha(source.a, options.opacity);
        if let Some(mask) = mask {
            source.a = modulate_alpha(source.a, mask[column]);
        }
    }
    blend_into(target, source, options.blend_mode);
}

/// Draw disjoint RGBA rows. The callback receives the absolute row index.
pub fn draw_pixel_rows(
    target: &mut [u8],
    stride: usize,
    top: u32,
    workers: usize,
    draw_row: impl Fn(&mut [u8], u32) + Sync,
) {
    static USE_ROW_POOL: OnceLock<bool> = OnceLock::new();
    let use_row_pool = *USE_ROW_POOL.get_or_init(|| {
        match std::env::var("CS2_RENDERER_ROW_POOL") {
            Ok(value) => value == "1",
            Err(_) => cfg!(target_os = "android"),
        }
    });
    if use_row_pool {
        super::row_pool::draw_pixel_rows(target, stride, top, workers, draw_row);
    } else {
        draw_pixel_rows_scoped(target, stride, top, workers, draw_row);
    }
}

fn draw_pixel_rows_scoped(
    target: &mut [u8],
    stride: usize,
    top: u32,
    workers: usize,
    draw_row: impl Fn(&mut [u8], u32) + Sync,
) {
    assert!(stride > 0 && target.len().is_multiple_of(stride));
    if target.is_empty() {
        return;
    }
    let workers = workers.clamp(1, target.len() / stride);
    let process = |first: u32, band: &mut [u8]| {
        for (row, pixels) in band.chunks_exact_mut(stride).enumerate() {
            draw_row(pixels, first + row as u32);
        }
    };
    if workers == 1 {
        process(top, target);
        return;
    }
    let band_rows = (target.len() / stride).div_ceil(workers);
    let bands = Mutex::new(target.chunks_mut(band_rows * stride).enumerate());
    let work = || loop {
        let next = bands.lock().unwrap().next();
        let Some((index, band)) = next else { break };
        process(top + (index * band_rows) as u32, band);
    };
    std::thread::scope(|scope| {
        for _ in 1..workers {
            let _ = std::thread::Builder::new().spawn_scoped(scope, &work);
        }
        work();
    });
}

const SAMPLE_CACHE_BYTES: usize = 16 * 1024 * 1024;
const SAMPLE_CACHE_ENTRIES: usize = 16;

#[derive(PartialEq, Eq)]
struct SampleKey {
    size: [u32; 2],
    bounds: Bounds,
    position: Point,
    transform: [u32; 5],
    flip: [bool; 2],
    filter: ScaleFilter,
}

struct SampleEntry {
    source: Weak<FramePixels>,
    key: SampleKey,
    pixels: Option<Arc<Vec<Color>>>,
}

impl SampleEntry {
    fn bytes(&self) -> usize {
        self.pixels
            .as_ref()
            .map_or(0, |pixels| pixels.len() * size_of::<Color>())
    }
}

thread_local! {
    static SAMPLES: RefCell<VecDeque<SampleEntry>> = const { RefCell::new(VecDeque::new()) };
}

fn sampled_pixels(
    frame: &RgbaFrame,
    options: &ImageDrawOptions,
    bounds: Bounds,
    fill: impl FnOnce(&mut [Color]),
) -> Option<Arc<Vec<Color>>> {
    let count = (bounds.2 - bounds.0) as usize * (bounds.3 - bounds.1) as usize;
    let bytes = count * size_of::<Color>();
    if count < 4096 || bytes > SAMPLE_CACHE_BYTES {
        return None;
    }
    let key = SampleKey {
        size: [frame.width, frame.height],
        bounds,
        position: options.position,
        transform: [
            options.scale_x,
            options.scale_y,
            options.rotation_radians,
            options.pivot[0],
            options.pivot[1],
        ]
        .map(f32::to_bits),
        flip: [options.flip_x, options.flip_y],
        filter: options.filter,
    };
    SAMPLES.with_borrow_mut(|cache| {
        cache.retain(|entry| entry.source.strong_count() != 0);
        let source = Arc::downgrade(&frame.pixels);
        let entry = if let Some(index) = cache
            .iter()
            .position(|entry| entry.source.ptr_eq(&source) && entry.key == key)
        {
            let mut entry = cache.remove(index).unwrap();
            if entry.pixels.is_none() {
                let mut pixels = Vec::new();
                if pixels.try_reserve_exact(count).is_ok() {
                    let mut used = cache.iter().map(SampleEntry::bytes).sum::<usize>();
                    while used + bytes > SAMPLE_CACHE_BYTES {
                        used -= cache.pop_front().unwrap().bytes();
                    }
                    pixels.resize(count, Color::TRANSPARENT);
                    fill(&mut pixels);
                    entry.pixels = Some(Arc::new(pixels));
                }
            }
            entry
        } else {
            if cache.len() == SAMPLE_CACHE_ENTRIES {
                cache.pop_front();
            }
            SampleEntry {
                source,
                key,
                pixels: None,
            }
        };
        let pixels = entry.pixels.clone();
        cache.push_back(entry);
        pixels
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlendMode, Rect, blend_special, sample_linear};

    #[test]
    fn rgba_row_workers_match_serial_source_over() {
        const WIDTH: usize = 258;
        const HEIGHT: usize = 512;
        let source: Vec<u8> = (0..WIDTH * HEIGHT)
            .flat_map(|index| {
                let value = index as u8;
                let alpha = match index % 7 {
                    0 => 0,
                    1 => 255,
                    _ => value,
                };
                [value, value.wrapping_add(3), value.wrapping_add(9), alpha]
            })
            .collect();
        let background: Vec<u8> = (0..260 * 520)
            .flat_map(|index| {
                let value = index as u8;
                [value, value.wrapping_add(11), value.wrapping_add(23), 200]
            })
            .collect();
        let rect = Rect::new(1, 2, WIDTH as u32, HEIGHT as u32);
        let mut expected = Canvas::from_rgba8(260, 520, background.clone()).unwrap();
        expected
            .blend_rgba_rows(rect, &source, WIDTH * 4, BlendMode::SourceOver)
            .unwrap();
        for workers in [2, 4] {
            let mut actual = Canvas::from_rgba8(260, 520, background.clone()).unwrap();
            actual
                .blend_rgba_rows_with_workers(
                    rect,
                    &source,
                    WIDTH * 4,
                    BlendMode::SourceOver,
                    workers,
                )
                .unwrap();
            assert_eq!(actual.pixels(), expected.pixels(), "workers={workers}");
        }
    }

    #[test]
    fn special_blends_match_float_reference_for_every_channel_input() {
        for mode in [
            BlendMode::AddSource,
            BlendMode::AddDestination,
            BlendMode::Multiply,
            BlendMode::InvertDestination,
            BlendMode::AddMultiply,
        ] {
            for alpha in 0..=255u8 {
                if alpha > 1
                    && alpha < 255
                    && !matches!(mode, BlendMode::AddSource | BlendMode::AddDestination)
                {
                    continue;
                }
                for dst in 0..=255u8 {
                    for src in (0..=255u8).step_by(3) {
                        let source = [src, src.saturating_add(1), src.saturating_add(2), alpha];
                        let mut actual = [dst; 4];
                        blend_special(&mut actual, &source, mode);
                        let expected = source.map(|src| {
                            if alpha == 0 {
                                return dst;
                            }
                            let a = f64::from(alpha) / 255.0;
                            let src = f64::from(src);
                            let dst = f64::from(dst);
                            let channel = match mode {
                                BlendMode::AddSource => src * a + dst,
                                BlendMode::AddDestination => src + dst * (1.0 - a),
                                BlendMode::Multiply => src * dst / 255.0,
                                BlendMode::InvertDestination => src * (255.0 - dst) / 255.0,
                                BlendMode::AddMultiply => src * dst / 255.0 + dst,
                                _ => unreachable!(),
                            };
                            channel.clamp(0.0, 255.0).round() as u8
                        });
                        assert_eq!(actual, expected, "{mode:?}, src={source:?}, dst={dst}");
                    }
                }
            }
        }
    }

    #[test]
    fn linear_sampling_matches_reference_at_edges_and_half_steps() {
        let frame = RgbaFrame::from_rgba8(
            2,
            2,
            vec![0, 3, 221, 0, 1, 2, 12, 255, 2, 211, 3, 163, 0, 2, 5, 1],
        )
        .unwrap();
        let coordinates = [
            -1.5,
            -0.5,
            0.0,
            0.5f64.next_down(),
            0.5,
            0.5f64.next_up(),
            1.0f64.next_down(),
            1.0,
            1.0f64.next_up(),
            1.5,
        ];
        for x in coordinates {
            for y in coordinates {
                let left = x.floor().clamp(0.0, 1.0) as usize;
                let top = y.floor().clamp(0.0, 1.0) as usize;
                let tx = (x - left as f64).clamp(0.0, 1.0);
                let ty = (y - top as f64).clamp(0.0, 1.0);
                let at =
                    |row, col, channel| f64::from(frame.pixels()[(row * 2 + col) * 4 + channel]);
                let channel = |index| {
                    let upper = at(top, left, index) * (1.0 - tx) + at(top, 1, index) * tx;
                    let lower = at(1, left, index) * (1.0 - tx) + at(1, 1, index) * tx;
                    (upper * (1.0 - ty) + lower * ty).round().clamp(0.0, 255.0) as u8
                };
                assert_eq!(
                    sample_linear(&frame, x, y),
                    Color::rgba(channel(0), channel(1), channel(2), channel(3))
                );
            }
        }
    }

    #[test]
    fn transform_cache_keys_exact_samples_and_admits_only_repeats() {
        let frame = RgbaFrame::from_rgba8(64, 64, vec![255; 64 * 64 * 4]).unwrap();
        let clone = frame.clone();
        let other = RgbaFrame::from_rgba8(64, 64, vec![0; 64 * 64 * 4]).unwrap();
        let mut options = ImageDrawOptions::new(Point::new(0, 0));
        options.filter = ScaleFilter::Linear;
        let bounds = (0, 0, 64, 64);
        assert!(sampled_pixels(&frame, &options, bounds, |_| panic!()).is_none());
        let pixels =
            sampled_pixels(&clone, &options, bounds, |pixels| pixels.fill(Color::WHITE)).unwrap();
        options.opacity = 91;
        options.blend_mode = BlendMode::Multiply;
        let hit = sampled_pixels(&frame, &options, bounds, |_| panic!()).unwrap();
        assert!(Arc::ptr_eq(&hit, &pixels));
        assert!(sampled_pixels(&other, &options, bounds, |_| panic!()).is_none());
        options.rotation_radians = f32::from_bits(1);
        assert!(sampled_pixels(&frame, &options, bounds, |_| panic!()).is_none());
        options.rotation_radians = 0.0;
        options.scale_y = 1.2;
        assert!(sampled_pixels(&frame, &options, bounds, |_| panic!()).is_none());
        options.scale_y = 1.0;
        assert!(sampled_pixels(&frame, &options, (1, 0, 65, 64), |_| panic!()).is_none());
    }
}
