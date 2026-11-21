use super::{Bank, Canvas, FrameMetadata, ImageDrawOptions, Kind, Point, Result, RgbaFrame};
use super::{checked_size, copy_scaled_bottom, decode_frame};
use cs2_formats::hg::HgFrame;
use cs2_runtime::graph_serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error, SeqAccess, Visitor},
    ser::SerializeSeq,
};

#[derive(Default)]
pub(super) struct Buttons {
    pub(super) primary: [Option<u32>; 2],
    pub(super) arrows: [Option<u32>; 2],
    pub(super) input_priority: Option<i32>,
    focused: Option<u32>,
    pressed: Option<u32>,
}

impl Buttons {
    pub(super) fn react_region(&self) -> Option<u32> {
        self.pressed.or(self.focused).or(self.primary[1])
    }

    pub(super) fn has_pressed_region(&self) -> bool {
        self.pressed.is_some()
    }

    pub(super) fn valid_for(&self, kind: Kind) -> bool {
        if kind != Kind::Scroll {
            return !self.extended();
        }
        let is_button = |id| self.primary[1] == Some(id) || self.arrows.contains(&Some(id));
        self.focused.is_none_or(is_button)
            && self
                .pressed
                .is_none_or(|id| is_button(id) || self.primary[0] == Some(id))
    }

    fn extended(&self) -> bool {
        self.arrows != [None; 2]
            || self.input_priority.is_some()
            || self.focused.is_some()
            || self.pressed.is_some()
    }

    pub(super) fn focus(&mut self, region: u32, focused: bool) -> bool {
        if self.primary[1] != Some(region) && !self.arrows.contains(&Some(region)) {
            return false;
        }
        let previously_focused = self.focused;
        if focused {
            self.focused = Some(region);
        } else if self.focused == Some(region) {
            self.focused = None;
        }
        self.focused != previously_focused
    }

    fn frame_variant(&self, region: Option<u32>, enabled: bool) -> u32 {
        if !enabled {
            1
        } else if region.is_some() && self.pressed == region {
            3
        } else if region.is_some() && self.focused == region {
            2
        } else {
            0
        }
    }
}

// A preexisting control bank stores a two-element tuple in this field.
// The host graph gives tuples and sequences the same count-prefixed wire format.
impl Serialize for Buttons {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let extended = self.extended();
        let mut sequence = serializer.serialize_seq(Some(if extended { 7 } else { 2 }))?;
        sequence.serialize_element(&self.primary[0])?;
        sequence.serialize_element(&self.primary[1])?;
        if extended {
            sequence.serialize_element(&self.arrows[0])?;
            sequence.serialize_element(&self.arrows[1])?;
            sequence.serialize_element(&self.input_priority)?;
            sequence.serialize_element(&self.focused)?;
            sequence.serialize_element(&self.pressed)?;
        }
        sequence.end()
    }
}

impl<'de> Deserialize<'de> for Buttons {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct ButtonVisitor;
        impl<'de> Visitor<'de> for ButtonVisitor {
            type Value = Buttons;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("two or seven saved control button fields")
            }

            fn visit_seq<S: SeqAccess<'de>>(
                self,
                mut sequence: S,
            ) -> std::result::Result<Buttons, S::Error> {
                fn required<'de, T: Deserialize<'de>, S: SeqAccess<'de>>(
                    sequence: &mut S,
                ) -> std::result::Result<T, S::Error> {
                    sequence
                        .next_element()?
                        .ok_or_else(|| Error::custom("incomplete saved control buttons"))
                }
                let mut buttons = Buttons {
                    primary: [required(&mut sequence)?, required(&mut sequence)?],
                    ..Buttons::default()
                };
                if let Some(first_arrow) = sequence.next_element()? {
                    buttons.arrows = [first_arrow, required(&mut sequence)?];
                    buttons.input_priority = required(&mut sequence)?;
                    buttons.focused = required(&mut sequence)?;
                    buttons.pressed = required(&mut sequence)?;
                    if sequence.next_element::<Option<u32>>()?.is_some() {
                        return Err(Error::custom("too many saved control button fields"));
                    }
                }
                Ok(buttons)
            }
        }
        deserializer.deserialize_seq(ButtonVisitor)
    }
}

fn frame(object: &Bank, id: u32) -> Option<&HgFrame> {
    object
        .source
        .as_ref()?
        .frames
        .iter()
        .find(|frame| frame.id == id)
}

fn dimensions(frame: &HgFrame) -> [i32; 2] {
    [frame.width as i32, frame.height as i32]
}

fn extent(object: &Bank, id: u32, axis: usize) -> i32 {
    frame(object, id).map_or(0, |frame| dimensions(frame)[axis])
}

fn track_start(object: &Bank, axis: usize) -> i32 {
    frame(object, 1000)
        .or_else(|| frame(object, 1001))
        .or_else(|| frame(object, 0))
        .map_or(0, |frame| frame.offset[axis])
}

fn range(object: &Bank) -> (i32, i32) {
    let [minimum, maximum] = object.scroll.range;
    (
        minimum,
        maximum
            .saturating_sub(object.scroll.page.max(0))
            .max(minimum),
    )
}

pub(super) fn cancel_drag(object: &mut Bank) {
    if object.drag_origin.take().is_some() || object.buttons.pressed.is_some() {
        object.buttons.pressed = None;
        set_position(object, object.scroll.position);
        object.visual_dirty = true;
    }
}

pub(super) fn set_position(object: &mut Bank, position: i32) {
    let (minimum, maximum) = range(object);
    object.scroll.position = position.clamp(minimum, maximum);
    if object.drag_origin.is_none() {
        let span = i64::from(maximum) - i64::from(minimum);
        object.scroll.pixel = if span == 0 {
            0
        } else {
            let relative = i64::from(object.scroll.position) - i64::from(minimum);
            (relative * i64::from(object.scroll.travel[1]) / span) as i32
        };
    }
}

pub(super) fn pointer_input(object: &mut Bank, region: u32, code: u32, at: Option<i32>) {
    let knob = object.buttons.primary[1] == Some(region);
    match (code, at) {
        (2, Some(at)) => {
            object.buttons.pressed = Some(region);
            if knob {
                if object.scroll.travel[1] > 0 {
                    object.drag_origin = Some([at, object.scroll.pixel]);
                }
            } else {
                let delta = if object.buttons.arrows[0] == Some(region) {
                    -1
                } else if object.buttons.arrows[1] == Some(region) {
                    1
                } else {
                    let axis = object.direction as usize;
                    let start = object.rectangle[axis]
                        .saturating_add(track_start(object, axis))
                        .saturating_add(object.scroll.pixel);
                    let page = object.scroll.page.max(1);
                    if at < start { -page } else { page }
                };
                set_position(object, object.scroll.position.saturating_add(delta));
            }
        }
        (4, _) => {
            cancel_drag(object);
        }
        (6, Some(at)) => {
            let Some([pressed, pixel]) = object.drag_origin else {
                return;
            };
            let travel = i64::from(object.scroll.travel[1]);
            let pixel = (i64::from(pixel) + i64::from(at) - i64::from(pressed)).clamp(0, travel);
            let (minimum, maximum) = range(object);
            let span = i64::from(maximum) - i64::from(minimum);
            if travel > 0 {
                object.scroll.position =
                    (i64::from(minimum) + (pixel * span + travel / 2) / travel) as i32;
            }
            object.scroll.pixel = pixel as i32;
        }
        _ => return,
    }
    object.visual_dirty = true;
}

pub(super) fn prepare(object: &mut Bank) -> Result<()> {
    let [left, top, right, bottom] = object.rectangle;
    let size = [
        right.saturating_sub(left).max(0) as u32,
        bottom.saturating_sub(top).max(0) as u32,
    ];
    if size.contains(&0) {
        object.control_image = None;
        return Ok(());
    }
    checked_size(size)?;
    let axis = usize::from(size[1] > size[0]);
    object.direction = axis as u32;
    let caps = [extent(object, 1000, axis), extent(object, 1002, axis)];
    let start = track_start(object, axis).clamp(0, size[axis] as i32);
    let available = size[axis] as i32 - start;
    let track = (caps[0] + extent(object, 1001, axis) + caps[1]).min(available);
    let track = if track == 0 { available } else { track };
    let handle = extent(object, 200, axis);
    let minimum = (caps[0] + caps[1]).max(handle).max(1).min(track);
    let span = (i64::from(object.scroll.range[1]) - i64::from(object.scroll.range[0])).max(1);
    let page = i64::from(object.scroll.page).clamp(0, span);
    let length = ((i64::from(track) * page / span) as i32).clamp(minimum, track);
    object.scroll.track_extent = track;
    object.scroll.minimum_extent = minimum;
    object.scroll.knob_extent = length;
    object.scroll.travel = [0, track - length];
    set_position(object, object.scroll.position);

    let mut canvas = Canvas::new(size[0], size[1])?;
    let enabled = object.applied_enabled.unwrap_or(object.enabled) != 0;
    let normal = |id, variant| frame(object, id + variant).or_else(|| frame(object, id));
    let draw = |canvas: &mut Canvas, frame: &HgFrame| -> Result<()> {
        let image = decode_frame(frame, &object.source_bytes)?;
        canvas.draw_frame(
            &image,
            &ImageDrawOptions::new(Point::new(frame.offset[0], frame.offset[1])),
        )?;
        Ok(())
    };
    if let Some(rail) = frame(object, 0) {
        draw(&mut canvas, rail)?;
    }
    let knob_variant = object
        .buttons
        .frame_variant(object.buttons.primary[1], enabled);
    // The 1000-series crops define the rail geometry. The 100-series
    // variants contain the corresponding artwork for the moving thumb.
    if let Some(thumb) = normal(100, knob_variant) {
        let image = decode_frame(thumb, &object.source_bytes)?;
        let mut part_size = [thumb.width, thumb.height];
        part_size[axis] = length as u32;
        let mut offset = thumb.offset;
        offset[axis] = start + object.scroll.pixel;
        offset[1] = size[1] as i32 - offset[1] - part_size[1] as i32;
        copy_scaled_bottom(&mut canvas, &image, offset, part_size)?;
    }
    if let Some(handle) = normal(200, knob_variant) {
        let image = decode_frame(handle, &object.source_bytes)?;
        let mut offset = handle.offset;
        offset[axis] = start + object.scroll.pixel + (length - dimensions(handle)[axis]) / 2;
        canvas.draw_frame(
            &image,
            &ImageDrawOptions::new(Point::new(offset[0], offset[1])),
        )?;
    }
    for (id, region) in [10, 20].into_iter().zip(object.buttons.arrows) {
        if let Some(arrow) = normal(id, object.buttons.frame_variant(region, enabled)) {
            draw(&mut canvas, arrow)?;
        }
    }
    object.control_image = Some(
        RgbaFrame::from_rgba8(size[0], size[1], canvas.pixels().to_vec())?
            .with_raw_metadata(FrameMetadata::raster([0; 2])),
    );
    Ok(())
}

pub(super) fn regions(object: &Bank) -> [[i32; 4]; 4] {
    let origin = [object.rectangle[0], object.rectangle[1]];
    let bounds = |offset: [i32; 2], size: [i32; 2]| {
        let x = origin[0].saturating_add(offset[0]);
        let y = origin[1].saturating_add(offset[1]);
        [x, y, x.saturating_add(size[0]), y.saturating_add(size[1])]
    };
    let axis = object.direction as usize;
    let start = track_start(object, axis);
    let mut size = [
        object.rectangle[2].saturating_sub(origin[0]),
        object.rectangle[3].saturating_sub(origin[1]),
    ];
    size[axis] = object.scroll.track_extent;
    let mut offset = [0; 2];
    offset[axis] = start;
    let track = bounds(offset, size);
    offset[axis] += object.scroll.pixel;
    size[axis] = object.scroll.knob_extent;
    let knob = bounds(offset, size);
    let arrow =
        |id| frame(object, id).map_or([0; 4], |frame| bounds(frame.offset, dimensions(frame)));
    [track, knob, arrow(10), arrow(20)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_runtime::object_graph::{decode_component, encode_component};

    #[test]
    fn saved_buttons_read_preexisting_two_element_records() -> Result<()> {
        let primary = [Some(17u32), None];
        let old = encode_component(&primary)?;
        let buttons: Buttons = decode_component(&old)?;
        assert_eq!(buttons.primary, primary);
        assert!(buttons.valid_for(Kind::Scroll2));
        assert_eq!(encode_component(&buttons)?, old);
        Ok(())
    }

    #[test]
    fn saved_legacy_buttons_keep_arrows_and_pointer_state() -> Result<()> {
        let buttons = Buttons {
            primary: [Some(17), Some(18)],
            arrows: [Some(19), Some(20)],
            input_priority: Some(10000),
            focused: Some(19),
            pressed: Some(18),
        };
        let encoded = encode_component(&buttons)?;
        let restored: Buttons = decode_component(&encoded)?;
        assert!(restored.valid_for(Kind::Scroll));
        assert_eq!(restored.primary, buttons.primary);
        assert_eq!(restored.arrows, buttons.arrows);
        assert_eq!(restored.input_priority, buttons.input_priority);
        assert_eq!(restored.focused, buttons.focused);
        assert_eq!(restored.pressed, buttons.pressed);
        Ok(())
    }

    #[test]
    fn legacy_thumb_release_clears_drag_without_moving_logical_position() {
        let mut object = Bank::new(Kind::Scroll);
        object.buttons.primary[1] = Some(18);
        object.scroll.set_range([0, 18], 4);
        object.scroll.travel = [0, 70];
        pointer_input(&mut object, 18, 2, Some(20));
        pointer_input(&mut object, 18, 6, Some(56));
        assert_eq!(object.scroll.position, 7);
        cancel_drag(&mut object);
        assert_eq!(object.drag_origin, None);
        assert_eq!(object.buttons.pressed, None);
        assert_eq!(object.scroll.position, 7);
        assert_eq!(object.scroll.pixel, 35);
    }
}
