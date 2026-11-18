//! Body-flow and ruby placement, independently implemented from the documented
//! message contracts. Glyph ink bearings do not participate in advance widths.

use std::collections::BTreeMap;

use super::{
    GlyphMask, Point, Result, StyledTextGlyph, StyledTextRun, TextAlignment, TextEncoding,
    TextMarkup, TextToken,
};

pub(super) fn layout(
    markup: &TextMarkup,
    origin: Point,
    line_height: i32,
    letter_spacing: i32,
    available_width: Option<i32>,
    no_start: Option<&[char]>,
    no_end: Option<&[char]>,
    ruby_offset: i32,
    mut rasterize: impl FnMut(char, u32, bool, bool) -> Result<GlyphMask>,
) -> Result<StyledTextRun> {
    // Measure once. Looking ahead for a group must not rasterize the same
    // glyph again, nor depend on a stateful font callback returning it twice.
    let mut masks = Vec::with_capacity(markup.tokens.len());
    for token in &markup.tokens {
        masks.push(match token {
            TextToken::Glyph {
                character,
                style,
                ruby,
                ..
            } => Some(rasterize(
                *character,
                if *ruby { style.ruby_size } else { style.size },
                style.bold,
                style.italic,
            )?),
            _ => None,
        });
    }
    let group_widths = measure_groups(markup, &masks, letter_spacing);
    let mut run = StyledTextRun::default();
    let mut pen_x = origin.x;
    let mut baseline_y = origin.y;
    let mut line_start = 0;
    let mut alignment = TextAlignment::Left;
    let mut previous_character = None;
    let mut previous_group = None;
    let mut line = 0usize;
    let mut glyph_lines = Vec::new();

    for (token_index, token) in markup.tokens.iter().enumerate() {
        if is_newline(token, markup.encoding) {
            finish_line(
                &mut run,
                line_start,
                pen_x,
                origin,
                alignment,
                available_width,
            );
            pen_x = origin.x;
            baseline_y = baseline_y.saturating_add(line_height);
            line_start = run.glyphs.len();
            previous_character = None;
            previous_group = None;
            line += 1;
            continue;
        }
        match token {
            TextToken::Glyph {
                character,
                style,
                ruby,
                group,
            } => {
                let glyph = masks[token_index].take().expect("measured glyph token");
                let step = glyph.advance().saturating_add(letter_spacing);
                if !ruby {
                    if let Some(width) = available_width {
                        let mut required = i64::from(step);
                        if group.is_some() && *group != previous_group {
                            if let Some(group_width) = group_widths[token_index] {
                                // Oversized groups fall back to character flow.
                                // This always consumes a glyph at the line start.
                                if group_width <= i64::from(width) {
                                    required = required.max(group_width);
                                }
                            }
                        }
                        if no_end.is_some_and(|set| set.contains(character)) {
                            if let Some(next) = next_body(markup, &masks, token_index + 1) {
                                let next_width = group_widths[next]
                                    .filter(|&value| value <= i64::from(width))
                                    .unwrap_or_else(|| {
                                        i64::from(masks[next].as_ref().unwrap().advance())
                                            + i64::from(letter_spacing)
                                    });
                                required = required.max(i64::from(step) + next_width);
                            }
                        }
                        let forbidden_start = no_start.is_some_and(|set| set.contains(character));
                        let follows_open = previous_character.is_some_and(|previous| {
                            no_end.is_some_and(|set| set.contains(&previous))
                        });
                        if pen_x > origin.x
                            && i64::from(pen_x) - i64::from(origin.x) + required > i64::from(width)
                            && !forbidden_start
                            && !follows_open
                        {
                            finish_line(
                                &mut run,
                                line_start,
                                pen_x,
                                origin,
                                alignment,
                                available_width,
                            );
                            pen_x = origin.x;
                            baseline_y = baseline_y.saturating_add(line_height);
                            line_start = run.glyphs.len();
                            line += 1;
                        }
                    }
                    run.advance.y = run.advance.y.max(
                        baseline_y
                            .saturating_sub(origin.y)
                            .saturating_add(line_height),
                    );
                }
                // Ruby never advances the body cursor or starts a new line.
                // Its actual coordinates are assigned after line alignment.
                run.glyphs.push(StyledTextGlyph {
                    glyph,
                    baseline: Point::new(pen_x, baseline_y),
                    style: *style,
                    ruby: *ruby,
                    group: *group,
                });
                glyph_lines.push(line);
                if !ruby {
                    pen_x = pen_x.saturating_add(step);
                    previous_character = Some(*character);
                    previous_group = *group;
                }
            }
            TextToken::PositionX(value) => {
                pen_x = origin.x.saturating_add(*value as i32);
                previous_group = None;
                line += 1;
            }
            TextToken::PositionY(value) => {
                baseline_y = origin.y.saturating_add(*value as i32);
                previous_group = None;
                // Coordinate changes create a distinct ruby binding segment
                // even when no automatic newline was required.
                line += 1;
            }
            TextToken::Align(next) => {
                alignment = *next;
                previous_group = None;
            }
            TextToken::Control(6) => previous_group = None,
            _ => {}
        }
    }
    finish_line(
        &mut run,
        line_start,
        pen_x,
        origin,
        alignment,
        available_width,
    );
    run.advance.y = run.advance.y.max(line_height);
    position_ruby(
        &mut run,
        &glyph_lines,
        origin.x,
        available_width,
        ruby_offset,
    );
    Ok(run)
}

fn is_newline(token: &TextToken, encoding: TextEncoding) -> bool {
    matches!(token, TextToken::Newline)
        || (encoding == TextEncoding::Unicode && matches!(token, TextToken::Control(8)))
}

fn ends_group_segment(token: &TextToken, encoding: TextEncoding) -> bool {
    is_newline(token, encoding)
        || matches!(
            token,
            TextToken::PositionX(_)
                | TextToken::PositionY(_)
                | TextToken::Align(_)
                | TextToken::Control(6)
        )
}

fn measure_groups(
    markup: &TextMarkup,
    masks: &[Option<GlyphMask>],
    spacing: i32,
) -> Vec<Option<i64>> {
    let mut widths = vec![None; markup.tokens.len()];
    let mut next_group = None;
    let mut remaining = 0i64;
    for (index, token) in markup.tokens.iter().enumerate().rev() {
        if ends_group_segment(token, markup.encoding) {
            next_group = None;
        }
        if let TextToken::Glyph {
            group, ruby: false, ..
        } = token
        {
            let step = i64::from(masks[index].as_ref().unwrap().advance()) + i64::from(spacing);
            remaining = if group.is_some() && *group == next_group {
                // Retain the furthest occupied advance for negative spacing.
                step.max(step.saturating_add(remaining))
            } else {
                step
            };
            if group.is_some() {
                widths[index] = Some(remaining);
            }
            next_group = *group;
        }
    }
    widths
}

fn next_body(markup: &TextMarkup, masks: &[Option<GlyphMask>], start: usize) -> Option<usize> {
    for (index, token) in markup.tokens.iter().enumerate().skip(start) {
        if ends_group_segment(token, markup.encoding) {
            return None;
        }
        if matches!(token, TextToken::Glyph { ruby: false, .. }) && masks[index].is_some() {
            return Some(index);
        }
    }
    None
}

fn finish_line(
    run: &mut StyledTextRun,
    start: usize,
    end_x: i32,
    origin: Point,
    alignment: TextAlignment,
    available_width: Option<i32>,
) {
    let width = end_x.saturating_sub(origin.x);
    run.advance.x = run.advance.x.max(width);
    let remaining = available_width.map_or(0, |available| available.saturating_sub(width));
    let shift = match alignment {
        TextAlignment::Left => 0,
        TextAlignment::Center => remaining / 2,
        TextAlignment::Right => remaining,
    };
    for glyph in &mut run.glyphs[start..] {
        glyph.baseline.x = glyph.baseline.x.saturating_add(shift);
    }
}

#[derive(Default)]
struct RubyGroup {
    base: Vec<(usize, Vec<usize>)>,
    ruby: Vec<usize>,
}

fn position_ruby(
    run: &mut StyledTextRun,
    lines: &[usize],
    left_limit: i32,
    available_width: Option<i32>,
    ruby_offset: i32,
) {
    let mut groups: BTreeMap<u32, RubyGroup> = BTreeMap::new();
    for (index, glyph) in run.glyphs.iter().enumerate() {
        let Some(id) = glyph.group else { continue };
        let group = groups.entry(id).or_default();
        if glyph.ruby {
            group.ruby.push(index);
        } else {
            if group
                .base
                .last()
                .is_none_or(|(line, _)| *line != lines[index])
            {
                group.base.push((lines[index], Vec::new()));
            }
            group.base.last_mut().unwrap().1.push(index);
        }
    }
    for group in groups.values() {
        if group.base.is_empty() || group.ruby.is_empty() {
            continue;
        }
        // A fitting group has one segment. If an oversized group had to split,
        // keep each reading on a real body line, rather than deriving a single
        // bounding rectangle from unrelated lines. Proportional distribution
        // is a portable fallback; exact legacy oversized binding needs traces.
        let total_base: usize = group.base.iter().map(|(_, indices)| indices.len()).sum();
        let mut consumed_base = 0usize;
        let mut consumed_ruby = 0usize;
        for (_, base) in &group.base {
            consumed_base += base.len();
            let end =
                ((consumed_base as u128 * group.ruby.len() as u128) / total_base as u128) as usize;
            let ruby = &group.ruby[consumed_ruby..end];
            consumed_ruby = end;
            if ruby.is_empty() {
                continue;
            }
            position_segment(run, base, ruby, left_limit, available_width, ruby_offset);
        }
    }
}

fn position_segment(
    run: &mut StyledTextRun,
    base: &[usize],
    ruby: &[usize],
    left_limit: i32,
    available_width: Option<i32>,
    ruby_offset: i32,
) {
    // Alignment uses advances, not ink bounds: side bearings must not shift a
    // pronunciation when the host switches to a font with different outlines.
    let left = base
        .iter()
        .map(|&index| run.glyphs[index].baseline.x)
        .min()
        .unwrap();
    // The message contract measures the group's body without character
    // spacing, just as it measures the reading without character spacing.
    let base_width = base
        .iter()
        .map(|&index| i64::from(run.glyphs[index].glyph.advance()))
        .sum::<i64>()
        .max(0);
    let ruby_width: i64 = ruby
        .iter()
        .map(|&index| i64::from(run.glyphs[index].glyph.advance()))
        .sum();
    let spare = base_width - ruby_width;
    let mut x = i64::from(left);
    if spare < 0 {
        x += spare / 2;
        if let Some(width) = available_width {
            // Apply the left bound first. A reading wider than the window is
            // then right-limited, which deliberately permits a negative X.
            x = x
                .max(i64::from(left_limit))
                .min(i64::from(left_limit) + i64::from(width) - ruby_width);
        }
    }
    let base_top = base
        .iter()
        .map(|&index| {
            let glyph = &run.glyphs[index];
            glyph
                .baseline
                .y
                .saturating_sub(glyph.glyph.metrics.bearing_y)
        })
        .min()
        .unwrap();
    let ruby_height = ruby
        .iter()
        .map(|&index| run.glyphs[index].glyph.height)
        .max()
        .unwrap_or(0)
        .min(i32::MAX as u32) as i32;
    let top = base_top
        .saturating_sub(ruby_height)
        .saturating_add(ruby_offset);
    let mut remaining_spare = spare.max(0);
    for (position, &index) in ruby.iter().enumerate() {
        let glyph = &mut run.glyphs[index];
        glyph.baseline.x = x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        // All readings share an ink top even with mixed ruby sizes.
        glyph.baseline.y = top.saturating_add(glyph.glyph.metrics.bearing_y);
        x += i64::from(glyph.glyph.advance());
        // Repeated integer division retains the remainder for later gaps.
        // The final interval is trailing space, rather than an extra leading
        // half interval.
        let gaps = ruby.len() - position;
        if spare >= 0 {
            let gap = remaining_spare / gaps as i64;
            x += gap;
            remaining_spare -= gap;
        }
    }
}
