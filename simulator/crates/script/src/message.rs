use crate::{
    Result,
    scene::{CstTextEncoding, CstTextToken},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageLayoutSettings {
    pub encoding: CstTextEncoding,
    pub width: i32,
    pub line_height: i32,
    pub glyph_width: i32,
    pub character_spacing: i32,
}

impl Default for MessageLayoutSettings {
    fn default() -> Self {
        Self {
            encoding: CstTextEncoding::Cp932,
            width: 800,
            line_height: 24,
            glyph_width: 24,
            character_spacing: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageGlyph {
    pub token_index: usize,
    pub unit_index: usize,
    pub line: u32,
    pub position: [i32; 2],
    pub size: [i32; 2],
    pub unit: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageLayout {
    pub entries: Vec<MessageGlyph>,
    pub line_count: u32,
    pub cursor: [i32; 2],
    pub encoding: CstTextEncoding,
}

impl MessageLayout {
    pub fn from_tokens(tokens: &[CstTextToken], settings: &MessageLayoutSettings) -> Result<Self> {
        if settings.width <= 0 || settings.line_height <= 0 || settings.glyph_width < 0 {
            return Err(crate::ScriptError::InvalidStructuredText {
                offset: 0,
                message: "message layout dimensions are invalid",
            });
        }
        let mut entries = Vec::new();
        let mut x = 0i32;
        let mut y = 0i32;
        let mut line = 0u32;
        let mut line_start = 0i32;
        let mut unit_index = 0usize;
        for (token_index, token) in tokens.iter().enumerate() {
            match token {
                CstTextToken::Text(bytes) => {
                    for unit in text_units(bytes, settings.encoding) {
                        if x > 0 && x.saturating_add(settings.glyph_width) > settings.width {
                            line += 1;
                            y = y.saturating_add(settings.line_height);
                            x = line_start;
                        }
                        entries.push(MessageGlyph {
                            token_index,
                            unit_index,
                            line,
                            position: [x, y],
                            size: [settings.glyph_width, settings.line_height],
                            unit,
                        });
                        unit_index = unit_index.saturating_add(1);
                        x = x
                            .saturating_add(settings.glyph_width)
                            .saturating_add(settings.character_spacing);
                    }
                }
                CstTextToken::Ruby { base, .. } | CstTextToken::NoBreak(base) => {
                    for unit in text_units(base, settings.encoding) {
                        entries.push(MessageGlyph {
                            token_index,
                            unit_index,
                            line,
                            position: [x, y],
                            size: [settings.glyph_width, settings.line_height],
                            unit,
                        });
                        unit_index = unit_index.saturating_add(1);
                        x = x
                            .saturating_add(settings.glyph_width)
                            .saturating_add(settings.character_spacing);
                    }
                }
                CstTextToken::Character(value) | CstTextToken::Unicode(value) => {
                    let unit = value.to_le_bytes().to_vec();
                    entries.push(MessageGlyph {
                        token_index,
                        unit_index,
                        line,
                        position: [x, y],
                        size: [settings.glyph_width, settings.line_height],
                        unit,
                    });
                    unit_index = unit_index.saturating_add(1);
                    x = x
                        .saturating_add(settings.glyph_width)
                        .saturating_add(settings.character_spacing);
                }
                CstTextToken::Newline => {
                    line += 1;
                    y = y.saturating_add(settings.line_height);
                    x = line_start;
                }
                CstTextToken::Position { axis, value } => {
                    let value = (*value).min(i32::MAX as u32) as i32;
                    if *axis == b'x' || *axis == b'X' {
                        x = value;
                    } else if *axis == b'y' || *axis == b'Y' {
                        y = value;
                    }
                }
                CstTextToken::Align(value) => match value.as_slice() {
                    b"l" => {
                        line_start = 0;
                        x = line_start;
                    }
                    b"c" => {
                        line_start = settings.width / 2;
                        x = line_start;
                    }
                    b"r" => {
                        line_start = settings.width;
                        x = line_start;
                    }
                    _ => {}
                },
                CstTextToken::Continue
                | CstTextToken::WaitCharacters(_)
                | CstTextToken::WaitFrames(_)
                | CstTextToken::WaitReset
                | CstTextToken::Style { .. }
                | CstTextToken::Identifier { .. }
                | CstTextToken::Control { .. } => {}
            }
        }
        Ok(Self {
            entries,
            line_count: line.saturating_add(1),
            cursor: [x, y],
            encoding: settings.encoding,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageTimingSettings {
    pub time_scale: u32,
    pub character_delay: u32,
    pub period_delay: u32,
    pub skip_spaces: bool,
}

impl Default for MessageTimingSettings {
    fn default() -> Self {
        Self {
            time_scale: 1,
            character_delay: 1,
            period_delay: 1,
            skip_spaces: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageSchedule {
    pub deadlines: Vec<u32>,
    pub accumulated_units: u32,
}

impl MessageSchedule {
    pub fn from_tokens(
        tokens: &[CstTextToken],
        layout: &MessageLayout,
        settings: &MessageTimingSettings,
        initial_units: u32,
    ) -> Result<Self> {
        let mut deadlines = vec![0; layout.entries.len()];
        let mut elapsed = initial_units;
        let mut entry = 0usize;
        for (token_index, token) in tokens.iter().enumerate() {
            match token {
                CstTextToken::WaitFrames(value) => {
                    elapsed = elapsed.wrapping_add(*value);
                }
                CstTextToken::WaitCharacters(value) => {
                    elapsed = if settings.time_scale == 0 || settings.time_scale as i32 <= 0 {
                        elapsed
                    } else {
                        settings
                            .character_delay
                            .wrapping_mul(*value)
                            .wrapping_mul(10)
                            .wrapping_div(settings.time_scale)
                    };
                }
                CstTextToken::WaitReset => {
                    elapsed = 0;
                }
                _ => {
                    while entry < layout.entries.len()
                        && layout.entries[entry].token_index == token_index
                    {
                        deadlines[entry] =
                            elapsed.wrapping_mul(settings.time_scale).wrapping_div(10);
                        if !settings.skip_spaces
                            || !is_space(&layout.entries[entry].unit, layout.encoding)
                        {
                            elapsed = elapsed.wrapping_add(
                                if is_period(&layout.entries[entry].unit, layout.encoding) {
                                    settings.period_delay
                                } else {
                                    settings.character_delay
                                },
                            );
                        }
                        entry += 1;
                    }
                }
            }
        }
        Ok(Self {
            deadlines,
            accumulated_units: elapsed,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageRevealUpdate {
    pub newly_visible: Vec<usize>,
    pub complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageReveal {
    deadlines: Vec<u32>,
    visible: Vec<bool>,
}

impl MessageReveal {
    pub fn new(schedule: &MessageSchedule) -> Self {
        Self {
            deadlines: schedule.deadlines.clone(),
            visible: vec![false; schedule.deadlines.len()],
        }
    }

    pub fn advance(&mut self, now: u32) -> MessageRevealUpdate {
        let mut newly_visible = Vec::new();
        for (index, deadline) in self.deadlines.iter().copied().enumerate() {
            if !self.visible[index] && time_reached(now, deadline) {
                self.visible[index] = true;
                newly_visible.push(index);
            }
        }
        MessageRevealUpdate {
            complete: self.visible.iter().all(|visible| *visible),
            newly_visible,
        }
    }

    pub fn reveal_all(&mut self) -> MessageRevealUpdate {
        let mut newly_visible = Vec::new();
        for (index, visible) in self.visible.iter_mut().enumerate() {
            if !*visible {
                *visible = true;
                newly_visible.push(index);
            }
        }
        MessageRevealUpdate {
            complete: true,
            newly_visible,
        }
    }

    pub fn remaining(&self) -> usize {
        self.visible.iter().filter(|visible| !**visible).count()
    }

    pub fn is_visible(&self, index: usize) -> Option<bool> {
        self.visible.get(index).copied()
    }
}

fn text_units(bytes: &[u8], encoding: CstTextEncoding) -> Vec<Vec<u8>> {
    match encoding {
        CstTextEncoding::Cp932 => {
            let mut units = Vec::new();
            let mut cursor = 0;
            while cursor < bytes.len() {
                let length = if cursor + 1 < bytes.len()
                    && matches!(bytes[cursor], 0x81..=0x9f | 0xe0..=0xfc)
                    && matches!(bytes[cursor + 1], 0x40..=0x7e | 0x80..=0xfc)
                {
                    2
                } else {
                    1
                };
                units.push(bytes[cursor..cursor + length].to_vec());
                cursor += length;
            }
            units
        }
        CstTextEncoding::Unicode => {
            let mut units = Vec::new();
            let mut cursor = 0;
            while cursor < bytes.len() {
                match std::str::from_utf8(&bytes[cursor..]) {
                    Ok(text) => {
                        for character in text.chars() {
                            let mut buffer = [0; 4];
                            units.push(character.encode_utf8(&mut buffer).as_bytes().to_vec());
                        }
                        break;
                    }
                    Err(error) => {
                        let valid = error.valid_up_to();
                        if valid > 0 {
                            let text = &bytes[cursor..cursor + valid];
                            for character in String::from_utf8_lossy(text).chars() {
                                let mut buffer = [0; 4];
                                units.push(character.encode_utf8(&mut buffer).as_bytes().to_vec());
                            }
                            cursor += valid;
                        } else {
                            units.push(vec![bytes[cursor]]);
                            cursor += 1;
                        }
                    }
                }
            }
            units
        }
    }
}

fn is_space(unit: &[u8], encoding: CstTextEncoding) -> bool {
    unit == b" "
        || match encoding {
            CstTextEncoding::Cp932 => unit == [0x81, 0x40],
            CstTextEncoding::Unicode => unit == "\u{3000}".as_bytes(),
        }
}

fn is_period(unit: &[u8], encoding: CstTextEncoding) -> bool {
    match encoding {
        CstTextEncoding::Cp932 => unit == [0x81, 0x42],
        CstTextEncoding::Unicode => unit == b"." || unit == "。".as_bytes(),
    }
}

fn time_reached(now: u32, deadline: u32) -> bool {
    now.wrapping_sub(deadline) as i32 >= 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_keeps_token_indices_and_wraps_visible_glyphs() {
        let tokens = vec![
            CstTextToken::Text(b"AB".to_vec()),
            CstTextToken::Newline,
            CstTextToken::Text(b"C".to_vec()),
        ];
        let layout = MessageLayout::from_tokens(
            &tokens,
            &MessageLayoutSettings {
                width: 24,
                glyph_width: 16,
                line_height: 20,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(layout.entries.len(), 3);
        assert_eq!(layout.entries[0].token_index, 0);
        assert_eq!(layout.entries[1].line, 0);
        assert_eq!(layout.entries[2].line, 1);
    }

    #[test]
    fn schedule_applies_waits_and_reveal_is_repeatable() {
        let tokens = vec![
            CstTextToken::Text(b"A".to_vec()),
            CstTextToken::WaitFrames(3),
            CstTextToken::Text(b"B".to_vec()),
            CstTextToken::WaitReset,
            CstTextToken::Text(b"C".to_vec()),
        ];
        let layout = MessageLayout::from_tokens(&tokens, &Default::default()).unwrap();
        let schedule = MessageSchedule::from_tokens(
            &tokens,
            &layout,
            &MessageTimingSettings {
                time_scale: 10,
                character_delay: 2,
                period_delay: 5,
                skip_spaces: false,
            },
            0,
        )
        .unwrap();
        assert_eq!(schedule.deadlines, [0, 5, 0]);
        let mut reveal = MessageReveal::new(&schedule);
        assert_eq!(reveal.advance(0).newly_visible, vec![0, 2]);
        assert_eq!(reveal.advance(0).newly_visible, Vec::<usize>::new());
        assert_eq!(reveal.advance(5).newly_visible, vec![1]);
        assert_eq!(reveal.remaining(), 0);
    }
}
