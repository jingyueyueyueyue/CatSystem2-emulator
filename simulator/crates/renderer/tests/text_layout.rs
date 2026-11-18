use cs2_renderer::{
    GlyphMask, GlyphMetrics, Point, TextEncoding, TextLayoutSettings, TextMarkup, TextStyle,
};

fn measured_glyph(advance: i32, bearing_x: i32, bearing_y: i32) -> cs2_renderer::Result<GlyphMask> {
    GlyphMask::from_alpha_with_metrics(
        1,
        1,
        vec![255],
        GlyphMetrics {
            bearing_x,
            bearing_y,
            advance,
        },
    )
}

#[test]
fn fitting_group_moves_together_and_ruby_does_not_wrap_or_advance_body() {
    let style = TextStyle {
        size: 10,
        ruby_size: 5,
        ..TextStyle::default()
    };
    let markup = TextMarkup::parse("AB[CD/abcdef]E", style).unwrap();
    let mut calls = 0;
    let run = markup
        .layout_in_width(Point::new(0, 20), 30, 20, 0, |_, size, _, _| {
            calls += 1;
            measured_glyph(size as i32, 0, 1)
        })
        .unwrap();
    let body: Vec<_> = run
        .glyphs()
        .iter()
        .filter(|glyph| !glyph.ruby)
        .map(|glyph| glyph.baseline)
        .collect();
    assert_eq!(
        body,
        [
            Point::new(0, 20),
            Point::new(10, 20),
            Point::new(0, 40),
            Point::new(10, 40),
            Point::new(20, 40),
        ]
    );
    assert_eq!(calls, 11);
    assert_eq!(run.advance(), Point::new(30, 40));
    let reading: Vec<_> = run.glyphs().iter().filter(|glyph| glyph.ruby).collect();
    assert_eq!(reading[0].baseline.x, 0);
    assert_eq!(reading[5].baseline.x, 25);
    assert!(reading.iter().all(|glyph| glyph.baseline.y < body[2].y));
}

#[test]
fn short_ruby_uses_advance_gaps_and_all_layout_entry_points_position_it() {
    let markup = TextMarkup::parse("[AB/xyz]", TextStyle::default()).unwrap();
    let rasterize = |_, size, _, _| {
        if size == 10 {
            measured_glyph(4, 2, 1)
        } else {
            measured_glyph(10, 7, 4)
        }
    };
    let settings = TextLayoutSettings {
        origin: Point::new(10, 20),
        width: 100,
        letter_spacing: 3,
        ..TextLayoutSettings::default()
    };
    let bounded = markup.layout_with_settings(&settings, rasterize).unwrap();
    let unbounded = markup.layout(settings.origin, 20, 3, rasterize).unwrap();
    assert_eq!(bounded, unbounded);
    let reading: Vec<_> = bounded
        .glyphs()
        .iter()
        .filter(|glyph| glyph.ruby)
        .map(|glyph| glyph.baseline)
        .collect();
    assert_eq!(
        reading,
        [Point::new(10, 16), Point::new(16, 16), Point::new(23, 16),]
    );
}

#[test]
fn oversized_groups_make_progress_and_opening_brackets_move_with_following_body() {
    let oversized = TextMarkup::parse("[ABCDE]", TextStyle::default()).unwrap();
    let run = oversized
        .layout_in_width(Point::new(0, 20), 20, 20, 0, |_, _, _, _| {
            measured_glyph(10, 0, 0)
        })
        .unwrap();
    let rows: Vec<_> = run.glyphs().iter().map(|glyph| glyph.baseline.y).collect();
    assert_eq!(rows, [20, 20, 40, 40, 60]);

    let brackets = TextMarkup::parse("A\u{300c}B", TextStyle::default()).unwrap();
    let run = brackets
        .layout_with_settings(
            &TextLayoutSettings {
                origin: Point::new(0, 20),
                width: 20,
                ..TextLayoutSettings::default()
            },
            |_, _, _, _| measured_glyph(10, 0, 0),
        )
        .unwrap();
    assert_eq!(run.glyphs()[0].baseline, Point::new(0, 20));
    assert_eq!(run.glyphs()[1].baseline, Point::new(0, 40));
    assert_eq!(run.glyphs()[2].baseline, Point::new(10, 40));
}

#[test]
fn unicode_record_eight_breaks_lines_and_overwide_ruby_is_right_limited() {
    let cp932 = TextMarkup::parse(r"A\rB", TextStyle::default()).unwrap();
    let unicode =
        TextMarkup::parse_with_encoding(r"A\rB", TextStyle::default(), TextEncoding::Unicode)
            .unwrap();
    assert_eq!(unicode.encoding(), TextEncoding::Unicode);
    let rasterize = |_, _, _, _| measured_glyph(10, 0, 0);
    let legacy = cp932.layout(Point::new(0, 20), 20, 0, rasterize).unwrap();
    let wide = unicode.layout(Point::new(0, 20), 20, 0, rasterize).unwrap();
    assert_eq!(legacy.glyphs()[1].baseline.y, 20);
    assert_eq!(wide.glyphs()[1].baseline.y, 40);

    let markup = TextMarkup::parse("[A/xyz]", TextStyle::default()).unwrap();
    let run = markup
        .layout_in_width(Point::new(0, 20), 20, 20, 0, rasterize)
        .unwrap();
    let reading: Vec<_> = run
        .glyphs()
        .iter()
        .filter(|glyph| glyph.ruby)
        .map(|glyph| glyph.baseline.x)
        .collect();
    assert_eq!(reading, [-10, 0, 10]);
}
