use std::{hint::black_box, sync::Arc, time::Instant};

use cs2_renderer::{
    AlphaMask, BlendMode, Canvas, Color, FontRasterizer, ImageDrawOptions, Point, Rect, RgbaFrame,
    ScaleFilter, TextLayoutSettings, TextMarkup, TextStyle,
};

fn frame(width: u32, height: u32, opaque: bool) -> RgbaFrame {
    let pixels = (0..width * height)
        .flat_map(|index| {
            let value = index.wrapping_mul(0x9e3779b9).to_le_bytes();
            [
                value[0],
                value[2],
                value[3],
                if opaque { 255 } else { value[1] },
            ]
        })
        .collect();
    RgbaFrame::from_rgba8(width, height, pixels).unwrap()
}

const MODES: [BlendMode; 10] = [
    BlendMode::SourceOver,
    BlendMode::AddSource,
    BlendMode::AddDestination,
    BlendMode::Multiply,
    BlendMode::InvertDestination,
    BlendMode::AddMultiply,
    BlendMode::ListAlphaFromBlue,
    BlendMode::ListAlphaFromGreen,
    BlendMode::ListAlphaFromRed,
    BlendMode::ListAlphaFromAlpha,
];

#[test]
fn draw_matches_baseline_frames() {
    let opaque = frame(111, 107, true);
    let mixed = frame(111, 107, false);
    let mut hashes = [0; 10];
    for (index, mode) in MODES.into_iter().enumerate() {
        let mut canvas = Canvas::new(143, 133).unwrap();
        canvas.clear(Color::rgba(9, 171, 86, 103));
        for step in 0..22 {
            let mut options = ImageDrawOptions::new(Point::new(-19, 6));
            options.blend_mode = mode;
            options.clip = Some(Rect::new(3, -2, 131, 128));
            options.filter = if step % 2 == 0 {
                ScaleFilter::Nearest
            } else {
                ScaleFilter::Linear
            };
            if step % 3 != 0 {
                options.mask = Some(
                    AlphaMask::new(
                        143,
                        133,
                        (0..143 * 133).map(|offset| (offset * 23) as u8).collect(),
                    )
                    .unwrap(),
                );
                options.opacity = (step * 53) as u8;
            }
            match step / 4 {
                1 => {
                    options.scale_x = 1.37;
                    options.scale_y = 0.43;
                }
                2 => {
                    options.scale_x = 0.1;
                    options.scale_y = 2.75;
                }
                3 | 4 => {
                    options.position = Point::new(44, 13);
                    options.scale_x = -1.08;
                    options.scale_y = 1.28;
                    options.rotation_radians = 0.29;
                    options.pivot = [41.25, -2.5];
                    options.flip_x = step / 4 == 3;
                }
                _ => {}
            }
            let source = if step % 4 == 0 { &opaque } else { &mixed };
            for _ in 0..3 {
                canvas.draw_frame(source, &options).unwrap();
                hashes[index] = u64::rotate_left(hashes[index], 7) ^ canvas.hash();
            }
        }
    }
    assert_eq!(
        hashes,
        [
            0xf7aa0ac616c6a7e2,
            0x654d9af07aecc22e,
            0x615b49033d43ec76,
            0xd6783046668a29db,
            0xa9b002890bdf3a9b,
            0x29e98462cdebe5e9,
            0xf536e980b2912956,
            0x37e232f8017a72b4,
            0xc5ff6e0be3f5aea4,
            0x76830667d91d2c34,
        ]
    );
}

#[test]
fn cold_worker_bands_match_serial_pixels() {
    let source = frame(1031, 701, false);
    let mut options = ImageDrawOptions::new(Point::new(-18, 9));
    options.clip = Some(Rect::new(7, 13, 1010, 665));
    options.mask = Some(
        AlphaMask::new(
            1057,
            723,
            (0..1057 * 723).map(|offset| (offset * 23) as u8).collect(),
        )
        .unwrap(),
    );
    options.filter = ScaleFilter::Linear;
    options.rotation_radians = 0.071;
    options.opacity = 181;
    let draw = |options: &ImageDrawOptions, workers| {
        let source = &source;
        std::thread::scope(|scope| {
            scope
                .spawn(move || {
                    let mut canvas = Canvas::new(1057, 723).unwrap();
                    canvas.clear(Color::rgba(9, 17, 26, 118));
                    canvas
                        .draw_frame_with_workers(source, options, workers)
                        .unwrap();
                    canvas
                })
                .join()
                .unwrap()
        })
    };
    for mode in [
        BlendMode::SourceOver,
        BlendMode::AddDestination,
        BlendMode::ListAlphaFromAlpha,
    ] {
        options.blend_mode = mode;
        let serial = draw(&options, 1);
        for workers in [2, 4] {
            assert!(
                serial.pixels() == draw(&options, workers).pixels(),
                "{mode:?}, {workers} workers"
            );
        }
    }
}

fn measure(name: &str, count: u32, mut operation: impl FnMut(u32)) {
    for index in 0..5 {
        operation(index);
    }
    let mut samples = [0.0; 7];
    for sample in &mut samples {
        let start = Instant::now();
        for index in 0..count {
            operation(index);
        }
        *sample = start.elapsed().as_secs_f64() * 1e3 / f64::from(count);
    }
    samples.sort_by(f64::total_cmp);
    println!("{name:22} {:8.4} ms/iteration", samples[3]);
}

#[test]
#[ignore = "synthetic release benchmark; run with --ignored --nocapture --test-threads=1"]
fn cpu_timings() {
    let opaque = frame(1280, 720, true);
    let mixed = frame(1280, 720, false);
    let mut canvas = Canvas::new(1280, 720).unwrap();
    canvas.clear(Color::BLACK);
    let mut options = ImageDrawOptions::new(Point::new(0, 0));
    for (name, source) in [("opaque 1:1", &opaque), ("mixed 1:1", &mixed)] {
        measure(name, 20, |_| {
            canvas
                .draw_frame(black_box(source), black_box(&options))
                .unwrap()
        });
        println!("{name}: {:016x}", canvas.hash());
    }
    options.scale_x = 0.97;
    options.scale_y = 1.09;
    options.mask = Some(AlphaMask::solid(1280, 720, 173).unwrap());
    options.blend_mode = BlendMode::AddDestination;
    measure("nearest masked add", 20, |_| {
        canvas
            .draw_frame(black_box(&mixed), black_box(&options))
            .unwrap()
    });
    println!("nearest masked add: {:016x}", canvas.hash());
    options.mask = None;
    options.blend_mode = BlendMode::SourceOver;
    options.filter = ScaleFilter::Linear;
    options.rotation_radians = 0.22;
    measure("linear rotated static", 20, |_| {
        canvas
            .draw_frame(black_box(&mixed), black_box(&options))
            .unwrap()
    });
    println!("linear rotated static: {:016x}", canvas.hash());
    measure("linear rotated moving", 20, |index| {
        options.rotation_radians = 0.22 + index as f32 / 900.0;
        canvas
            .draw_frame(black_box(&mixed), black_box(&options))
            .unwrap();
    });
    println!("linear rotated moving: {:016x}", canvas.hash());

    for workers in [1, 2, 4] {
        canvas.clear(Color::BLACK);
        measure(&format!("moving {workers} workers"), 20, |index| {
            options.rotation_radians = 0.22 + index as f32 / 900.0;
            canvas
                .draw_frame_with_workers(black_box(&mixed), black_box(&options), workers)
                .unwrap();
        });
        println!("moving {workers} workers: {:016x}", canvas.hash());
    }

    let Ok(font_path) = std::env::var("CS2_RENDERER_TEST_FONT") else {
        return;
    };
    let font = FontRasterizer::from_bytes(&std::fs::read(font_path).unwrap(), 0).unwrap();
    let markup = TextMarkup::parse(
        &"ABCD [EFG/hij] KLMNO PQRS. ".repeat(24),
        TextStyle::default(),
    )
    .unwrap();
    let settings = TextLayoutSettings {
        width: 550,
        ..TextLayoutSettings::default()
    };
    measure("repeat layout", 500, |_| {
        let run = markup
            .layout_with_settings(&settings, |ch, size, bold, italic| {
                font.rasterize_styled(ch, size, bold, italic)
            })
            .unwrap();
        black_box(run);
    });
    let uncached = markup
        .layout_with_settings(&settings, |ch, size, bold, italic| {
            font.rasterize_styled(ch, size, bold, italic)
        })
        .unwrap();
    let cached = font.layout_cached(&markup, &settings, usize::MAX).unwrap();
    assert_eq!(*cached, uncached);
    assert!(Arc::ptr_eq(
        &cached,
        &font.layout_cached(&markup, &settings, usize::MAX).unwrap()
    ));
    let prefix = font.layout_cached(&markup, &settings, 4).unwrap();
    assert_eq!(prefix.glyphs().len(), 4);
    assert!(!Arc::ptr_eq(&prefix, &cached));
    let mut narrower = settings.clone();
    narrower.width = 100;
    assert_ne!(
        cached.advance(),
        font.layout_cached(&markup, &narrower, usize::MAX)
            .unwrap()
            .advance()
    );
    measure("repeat layout cached", 500, |_| {
        black_box(
            font.layout_cached(black_box(&markup), black_box(&settings), usize::MAX)
                .unwrap(),
        );
    });
}
