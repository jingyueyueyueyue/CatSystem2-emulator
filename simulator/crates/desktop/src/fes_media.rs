//! Structured desktop FES media dispatch.
//!
//! `apply_driver_media` uses the CP932 baseline; hosts with an explicit script
//! encoding use `apply_driver_media_encoded`. Numeric parameters, including
//! bank expressions, go through the driver's live expression context.
//!
//! After completing a logical frame, call `update_driver_media` once. Budget
//! continuations at that same frame must not call it again. Rendering should
//! obtain image options through `driver_media_draw_options` to apply clipping.
//! No command source string is reparsed into parameters.
//! IMAGE/BUTTON ANM updates belong to `prepare_driver_animations`, after the
//! source stage; a false result retains the current boundary and task time.

use anyhow::{Context, Result, bail};
use cs2_renderer::{Color, ImageDrawOptions, Point, Rect};
use cs2_runtime::ResourceIndex;
use cs2_script::ExpressionState;

use super::fes_driver::{FesDriverCommand, FesScriptDriver};
use super::{
    FesMediaLayer, FesRuntime, LogicEvent, ScriptTextEncoding, LoggingHost,
    load_fes_media_frame, load_fes_media_image, solid_frame,
};

#[path = "fes_image_source.rs"]
mod image_source;
pub(crate) use image_source::ImageSourceState;
pub(super) use image_source::{apply_difimage_attribute, apply_difimage_reference,
    apply_image_source_command, request_image_file, update_image_sources};
pub(crate) use image_source::{initialize_image_idn, source_image, release_image_source,
    release_image_sources, validate_image_source_bindings, source_draw_options, prepare_raw_image_source};
pub(super) fn install_ordinary_frame(layer: &mut FesMediaLayer, image: cs2_renderer::RgbaFrame) {
    image_source::install_ordinary_frame(layer, image);
}

const BACKGROUND: &str = "\0driver.baseimg";
const BACKGROUND_PENDING: &str = "\0driver.baseimg.pending";
const PLANE_FADE: &str = "\0driver.plane.fade.";
const PLANE_CLIP: &str = "\0driver.plane.clip.";

#[derive(Clone, Debug, Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct ImageAnimationRequest {
    name: Vec<u8>,
    playing: i32,
    pending: bool,
    // A suspended round keeps its original task time; completed objects are
    // skipped when a later object yields and the same finish stage retries.
    update_at: Option<u32>,
    completed_at: Option<u32>,
}

impl ImageAnimationRequest {
    pub(crate) fn validate_checkpoint(&self) -> Result<()> {
        if self.name.len() > 255 || self.name.contains(&0) || !matches!(self.playing, 0 | 1) {
            bail!("FES IMAGE animation request has an invalid name or playback switch");
        }
        Ok(())
    }

    pub(crate) fn validate_animation(&self, animation: Option<&cs2_script::ImageAnimation>) -> Result<()> {
        self.validate_checkpoint()?;
        if self.update_at.is_some() && animation.and_then(|animation| animation.timeline()).is_none() {
            bail!("FES animation continuation has no retained timeline");
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
enum Target {
    Image(String),
    Plane(String),
}

#[derive(Clone, Copy, Debug)]
enum Change {
    Position(i32, i32),
    Size(i32, i32),
    Display(bool),
    Color(Color),
    Alpha(u8),
    Clip(Option<Rect>),
    SetId(i32),
    Enable(bool),
    Fade { duration: i32, from: u8, to: u8 },
}

impl FesRuntime {
    pub(super) fn release_driver_media_bank(&mut self, name: &str) {
        self.media.remove(name);
        self.planes.remove(name);
        self.media.remove(&format!("{PLANE_CLIP}{name}"));
        self.media.remove(&format!("{PLANE_FADE}{name}"));
    }

    pub(super) fn apply_driver_media(
        &mut self,
        command: &FesDriverCommand,
        driver: &mut FesScriptDriver,
        expression: &mut ExpressionState,
        resources: &ResourceIndex,
        frame: u64,
        events: &mut Vec<LogicEvent>,
    ) -> Result<bool> {
        self.apply_driver_media_encoded(
            command,
            driver,
            expression,
            resources,
            frame,
            events,
            ScriptTextEncoding::Cp932,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_driver_media_encoded(
        &mut self,
        command: &FesDriverCommand,
        driver: &mut FesScriptDriver,
        expression: &mut ExpressionState,
        resources: &ResourceIndex,
        frame: u64,
        events: &mut Vec<LogicEvent>,
        encoding: ScriptTextEncoding,
    ) -> Result<bool> {
        let Some(first) = command.parameters.first() else {
            return Ok(false);
        };
        if first.eq_ignore_ascii_case(b"baseimg") {
            return self.driver_base_image(command, driver, expression, frame, events, encoding);
        }
        let source = if first == b"@" {
            command
                .object_event
                .as_ref()
                .context("FES media @ has no current event object")?
                .parameter()
        } else {
            first.clone()
        };
        let (name_bytes, bank) = split_target(&source, encoding)?;
        let name = decode(name_bytes, encoding)?;
        // Do not evaluate a non-media object's bank. Its own service must
        // evaluate it exactly once when it handles this command.
        if !self.has_driver_media(&name, driver, name_bytes) {
            return Ok(false);
        }
        let Some(subcommand) = command.parameters.get(1) else {
            return Ok(true);
        };
        let bank = bank
            .map(|source| driver.evaluate_parameter(source, command.context.current, expression))
            .transpose()?;
        let subcommand = ascii_command(subcommand)?;
        if matches!(subcommand.as_str(), "getsize" | "getbase" | "getoffset")
            && driver.adapter.object(name_bytes).is_some_and(|object|
            object.definition.kind_name == b"IMAGE"
        ) {
            let bank = match bank {
                Some(bank) if bank >= 0 => bank,
                None if driver.adapter.object(name_bytes).is_some_and(|object|
                    object.definition.bank_count == 1
                ) => 0,
                _ => return Ok(true),
            };
            let targets = self.driver_media_targets(&name, name_bytes, Some(bank), driver)?;
            let [Target::Image(key)] = targets.as_slice() else { return Ok(true); };
            let layer = &self.media[key];
            let [x, y] = match subcommand.as_str() {
                "getbase" => layer.image_base,
                "getoffset" => layer.image_offset,
                _ => layer.navigation.size,
            };
            let values = [x, y, 0];
            let count = if subcommand == "getsize" { 3 } else { 2 };
            let mut destinations = [-1; 3];
            for (destination, source) in destinations[..count].iter_mut().zip(&command.parameters[2..]) {
                *destination = driver.evaluate_parameter(source, command.context.current, expression)?;
            }
            for (&destination, value) in destinations[..count].iter().zip(values) {
                if let Some(slot) = expression.shared.get_mut(destination as usize) {
                    *slot = value;
                }
            }
            events.push(LogicEvent {
                frame,
                kind: "fes-media".into(),
                detail: format!("object={name:?} bank={bank} command={subcommand} value={x},{y}"),
            });
            return Ok(true);
        }
        let targets = self.driver_media_targets(&name, name_bytes, bank, driver)?;
        if targets.is_empty() {
            driver.fallback_command(command, expression)?;
            return Ok(true);
        }
        if subcommand == "draw"
            && targets.iter().all(|target| {
                matches!(target, Target::Image(key) if self.media[key].kind == "BUTTON")
            })
        {
            events.push(LogicEvent {
                frame,
                kind: "fes-media".into(),
                detail: format!("object={name:?} command=draw action=ignored-by-button targets={}", targets.len()),
            });
            return Ok(true);
        }
        if !matches!(
            subcommand.as_str(),
            "pos"
                | "size"
                | "disp"
                | "color"
                | "alpha"
                | "clip"
                | "setid"
                | "load"
                | "setanm"
                | "anm"
                | "fade"
                | "enable"
                | "coordmode"
        ) {
            bail!("unsupported FES media subcommand {subcommand:?} for {name:?}");
        }
        let args = &command.parameters[2..];

        if subcommand == "coordmode" {
            if !targets.iter().all(|target| matches!(target, Target::Image(key) if self.media[key].kind == "IMAGE")) {
                bail!("unsupported FES media subcommand {subcommand:?} for {name:?}");
            }
            let mode = number(args, 0, Some(0), command.context.current, driver, expression)?;
            for target in &targets {
                if let Target::Image(key) = target {
                    self.media.get_mut(key).context("FES coordmode target disappeared")?.source.coordmode = mode;
                }
            }
        } else if subcommand == "load" {
            for target in &targets {
                // Unlike ordinary property arguments, IMAGE load evaluates its
                // frame number separately for every valid target bank.
                if let Target::Image(key) = target {
                    if self.media[key].kind != "CIMAGE" {
                        let filename = args.first().map(Vec::as_slice).unwrap_or_default();
                        self.driver_request_image(key, filename, encoding)?;
                    }
                }
                let current = self.driver_current_id(target);
                let id = number(args, 1, Some(0), current, driver, expression)?;
                if let Target::Image(key) = target {
                    if self.media[key].kind == "CIMAGE" {
                        continue;
                    }
                    let layer = self
                        .media
                        .get_mut(key)
                        .context("FES load target disappeared")?;
                    select_id(layer, id, resources)?;
                } else {
                    bail!("FES plane load requires a source service absent from this media model");
                }
            }
        } else if matches!(subcommand.as_str(), "setanm" | "anm") {
            self.driver_animation(&targets, &subcommand, args)?;
        } else if let Some(first_target) = targets.first() {
            let change =
                self.driver_media_change(first_target, &subcommand, args, driver, expression)?;
            for target in &targets {
                self.commit_driver_media(target, &change, resources)?;
            }
        }
        events.push(LogicEvent {
            frame,
            kind: "fes-media".into(),
            detail: format!(
                "object={name:?} command={subcommand} targets={}",
                targets.len()
            ),
        });
        Ok(true)
    }

    /// Reuses the actual fade/ANM updater. Resource/ANM errors are surfaced to
    /// the task host rather than being represented as successful log events.
    pub(super) fn update_driver_media(
        &mut self,
        frame: u64,
        budget: usize,
        resources: &ResourceIndex,
        events: &mut Vec<LogicEvent>,
    ) -> Result<()> {
        if budget == 0 {
            bail!("FES media animation budget must be positive");
        }
        let start = events.len();
        // IMAGE/BUTTON load/play and all later timeline slices belong to
        // the object stage after source preparation. The legacy updater
        // neither propagates a budget yield nor uses the retained raw source.
        // Taking only these animations leaves once-per-frame fades intact.
        let parked: Vec<_> = self.media.iter_mut().filter_map(|(key, layer)| {
            matches!(layer.kind.as_str(), "IMAGE" | "BUTTON")
                .then(|| layer.animation.take().map(|animation| (key.clone(), animation))).flatten()
        }).collect();
        self.update_animations(frame, budget, resources, events);
        for (key, animation) in parked {
            if let Some(layer) = self.media.get_mut(&key) { layer.animation = Some(animation); }
        }
        let fades: Vec<_> = self
            .media
            .iter()
            .filter(|(_, layer)| layer.kind == "DRIVER_PLANE_FADE")
            .filter_map(|(key, layer)| {
                layer.plane.as_ref().map(|plane| {
                    (
                        key.clone(),
                        plane.clone(),
                        layer.opacity,
                        layer.fade.is_none(),
                    )
                })
            })
            .collect();
        for (key, plane, opacity, finished) in fades {
            if let Some(plane) = self.planes.get_mut(&plane) {
                plane.opacity = opacity;
            }
            if finished {
                self.media.remove(&key);
            }
        }
        if let Some(error) = events[start..]
            .iter()
            .find(|event| event.kind == "fes-media-error")
        {
            bail!("FES media update failed: {}", error.detail);
        }
        Ok(())
    }

    /// Render the current layer using the same position/opacity/plane rules as
    /// the desktop renderer, additionally intersecting its explicit clip.
    pub(super) fn driver_media_draw_options(&self, name: &str) -> Result<Option<ImageDrawOptions>> {
        let layer = self
            .media
            .get(name)
            .context("FES media drawing target is absent")?;
        if !layer.visible
            || layer
                .animation_display
                .is_some_and(|display| display.visible == 0)
        {
            return Ok(None);
        }
        let mut options = ImageDrawOptions::new(layer.draw_position(&self.planes));
        if let Some(words) = &layer.motion {
            let base = [f32::from_bits(words[3]), f32::from_bits(words[4])];
            options.position.x = options
                .position
                .x
                .wrapping_add(cs2_runtime::fes_motion::x86_integer(base[0]));
            options.position.y = options
                .position
                .y
                .wrapping_add(cs2_runtime::fes_motion::x86_integer(base[1]));
            options.pivot = base;
            options.scale_x = f32::from_bits(words[25]);
            options.scale_y = f32::from_bits(words[26]);
            options.rotation_radians = f32::from_bits(words[31]);
        }
        options.opacity = layer
            .animation_display
            .map(|display| {
                display
                    .compose((0.0, 0.0), i32::from(layer.opacity), 1)
                    .blend
                    .clamp(0, 255) as u8
            })
            .unwrap_or(layer.opacity);
        options.clip = layer.clip;
        if let Some(name) = &layer.plane {
            let Some(name) = self.driver_plane_key(name) else {
                return Ok(None);
            };
            let plane = &self.planes[name];
            let canvas_plane = !self.media.contains_key(name);
            let animation = layer
                .animation_display
                .map_or([0; 2], |display| [display.x, display.y]);
            let mut point = [
                layer
                    .position
                    .x
                    .wrapping_add(animation[0])
                    .wrapping_add(layer.image_offset[0])
                    .wrapping_sub(layer.image_base[0]),
                layer
                    .position
                    .y
                    .wrapping_add(animation[1])
                    .wrapping_add(layer.image_offset[1])
                    .wrapping_sub(layer.image_base[1]),
            ];
            if let Some(words) = &layer.motion {
                for axis in 0..2 {
                    point[axis] = point[axis].wrapping_add(cs2_runtime::fes_motion::x86_integer(
                        f32::from_bits(words[3 + axis]),
                    ));
                }
            }
            if canvas_plane {
                point[0] = point[0].wrapping_add(plane.vram[0]);
                point[1] = point[1].wrapping_add(plane.vram[1]);
                let plane_clip =
                    Rect::new(plane.vram[0], plane.vram[1], plane.size[0], plane.size[1]);
                options.clip = Some(match options.clip {
                    Some(clip) => intersect(clip, plane_clip)?,
                    None => plane_clip,
                });
            }
            options.position = Point::new(point[0], point[1]);
        }
        Ok(Some(options))
    }

    pub(super) fn driver_plane_key(&self, name: &str) -> Option<&str> {
        // A single bank is allocated under its base name even if the FES
        // refers to it explicitly as name[0].
        self.planes.get_key_value(name)
            .or_else(|| self.planes.get_key_value(name.strip_suffix("[0]")?))
            .map(|(key, _)| key.as_str())
    }

    pub(super) fn driver_plane_clip(&self, name: &str) -> Option<Rect> {
        self.media
            .get(&format!("{PLANE_CLIP}{name}"))
            .and_then(|style| style.clip)
    }

    fn has_driver_media(&self, name: &str, driver: &FesScriptDriver, bytes: &[u8]) -> bool {
        self.media.get(name).is_some_and(media_kind)
            || self.planes.contains_key(name)
            || driver.adapter.object(bytes).is_some_and(|object| {
                matches!(
                    object.definition.kind_name.as_slice(),
                    b"IMAGE" | b"CIMAGE" | b"DIFIMAGE" | b"BUTTON" | b"PLANE" | b"CPLANE" | b"BGTABLE2" | b"CHTABLE2" | b"CHTABLE3"
                )
            })
            || self
                .media
                .iter()
                .any(|(key, layer)| media_kind(layer) && key.starts_with(&format!("{name}[")))
            || self
                .planes
                .keys()
                .any(|key| key.starts_with(&format!("{name}[")))
    }

    fn driver_media_targets(
        &self,
        name: &str,
        raw_name: &[u8],
        bank: Option<i32>,
        driver: &FesScriptDriver,
    ) -> Result<Vec<Target>> {
        let banks: Vec<i32> = match bank {
            Some(bank) if bank >= 0 => vec![bank],
            Some(bank) if bank != -1 => return Ok(Vec::new()),
            _ => {
                let count = match driver.adapter.object(raw_name) {
                    Some(object) => object.definition.bank_count,
                    None if self.media.contains_key(name) || self.planes.contains_key(name) => 1,
                    None => bail!("FES media array {name:?} lacks declaration bank metadata"),
                };
                (0..i32::try_from(count).context("FES media bank count exceeds i32")?).collect()
            }
        };
        let mut targets = Vec::new();
        for bank in banks {
            if driver.adapter.object(raw_name).is_some()
                && driver
                    .adapter
                    .object_bank(raw_name, bank as usize)
                    .is_none()
            {
                continue;
            }
            let key = format!("{name}[{bank}]");
            let key =
                if bank == 0 && (self.media.contains_key(name) || self.planes.contains_key(name)) {
                    name.to_owned()
                } else {
                    key
                };
            if self.media.get(&key).is_some_and(media_kind) {
                targets.push(Target::Image(key));
            } else if self.planes.contains_key(&key) {
                targets.push(Target::Plane(key));
            } else if driver.adapter.object(raw_name).is_some() {
                bail!("FES media bank {key:?} is not initialized");
            }
        }
        Ok(targets)
    }

    fn driver_current_id(&self, target: &Target) -> i32 {
        match target {
            Target::Image(name) => self.media[name].selected_id as u32 as i32,
            Target::Plane(_) => 0,
        }
    }

    fn driver_media_change(
        &self,
        target: &Target,
        subcommand: &str,
        args: &[Vec<u8>],
        driver: &mut FesScriptDriver,
        expression: &mut ExpressionState,
    ) -> Result<Change> {
        let (position, size, opacity, visible, color, enabled) = match target {
            Target::Image(name) => {
                let layer = &self.media[name];
                let size = layer
                    .clip
                    .map(|clip| [clip.width, clip.height])
                    .or_else(|| {
                        layer
                            .image
                            .as_ref()
                            .map(|image| [image.width(), image.height()])
                    })
                    .unwrap_or([0, 0]);
                (
                    layer.position,
                    size,
                    layer.opacity,
                    layer.visible,
                    layer.color,
                    layer.enabled,
                )
            }
            Target::Plane(name) => {
                let plane = &self.planes[name];
                (
                    plane.position,
                    plane.size,
                    plane.opacity,
                    plane.visible,
                    Color::WHITE,
                    true,
                )
            }
        };
        let mut eval =
            |index, default, current| number(args, index, default, current, driver, expression);
        Ok(match subcommand {
            "pos" => {
                let x = eval(0, Some(position.x), position.x)?;
                let y = eval(1, Some(position.y), position.y)?;
                let z = eval(2, Some(0), 0)?;
                if matches!(target, Target::Plane(_)) && z != 0 {
                    bail!(
                        "nonzero plane Z requires a 3D position field absent from this media model"
                    );
                }
                Change::Position(x, y)
            }
            "size" => Change::Size(
                eval(0, Some(0), size[0] as i32)?,
                eval(1, Some(0), size[1] as i32)?,
            ),
            "disp" => Change::Display(eval(0, Some(1), i32::from(visible))? != 0),
            "enable" => Change::Enable(eval(0, Some(0), i32::from(enabled))? != 0),
            "setid" => Change::SetId(eval(0, Some(0), self.driver_current_id(target))?),
            "color" => {
                let argb = (u32::from(color.a) << 24)
                    | (u32::from(color.r) << 16)
                    | (u32::from(color.g) << 8)
                    | u32::from(color.b);
                let value = eval(0, None, argb as i32)? as u32;
                Change::Color(Color::rgba(
                    (value >> 16) as u8,
                    (value >> 8) as u8,
                    value as u8,
                    (value >> 24) as u8,
                ))
            }
            "alpha" => Change::Alpha(alpha(eval(0, None, i32::from(opacity))?)?),
            "clip" if args.len() == 1 && args[0].eq_ignore_ascii_case(b"off") => Change::Clip(None),
            "clip" => {
                let current = match target {
                    Target::Image(name) => self.media[name].clip,
                    Target::Plane(name) => self
                        .media
                        .get(&format!("{PLANE_CLIP}{name}"))
                        .and_then(|style| style.clip),
                }
                .unwrap_or_default();
                let left = eval(0, None, current.x)?;
                let top = eval(1, None, current.y)?;
                let right = eval(2, None, current.x.wrapping_add(current.width as i32))?;
                let bottom = eval(3, None, current.y.wrapping_add(current.height as i32))?;
                let width = u32::try_from(i64::from(right) - i64::from(left))
                    .context("FES media clip has an inverted or unrepresentable width")?;
                let height = u32::try_from(i64::from(bottom) - i64::from(top))
                    .context("FES media clip has an inverted or unrepresentable height")?;
                Change::Clip(Some(Rect::new(left, top, width, height)))
            }
            "fade" => {
                let duration = eval(0, None, 0)?;
                let from = eval(1, None, i32::from(opacity))?;
                let to = eval(2, None, i32::from(opacity))?;
                if duration < 0 {
                    bail!("FES media fade duration must not be negative");
                }
                Change::Fade {
                    duration,
                    from: alpha(from)?,
                    to: alpha(to)?,
                }
            }
            _ => bail!("FES media property dispatcher has no contract for {subcommand:?}"),
        })
    }

    fn commit_driver_media(
        &mut self,
        target: &Target,
        change: &Change,
        resources: &ResourceIndex,
    ) -> Result<()> {
        match target {
            Target::Image(name) => {
                if matches!(change, Change::Size(_, _)) && let Some(plane) = self.planes.get_mut(name) {
                    plane.metadata.size_dirty = true;
                }
                let layer = self
                    .media
                    .get_mut(name)
                    .context("FES media target disappeared")?;
                match *change {
                    Change::Position(x, y) => {
                        layer.position = Point::new(x, y);
                        if let Some(words) = &mut layer.motion {
                            words[6] = (x as f32).to_bits();
                            words[7] = (y as f32).to_bits();
                        }
                    }
                    Change::Size(width, height) => {
                        layer.metadata.size_dirty = true;
                        if width == -1 || height == -1 {
                            let image = layer
                                .image
                                .as_ref()
                                .context("FES media physical size requires a loaded image")?;
                            layer.clip = Some(Rect::new(0, 0, image.width(), image.height()));
                        } else {
                            let width =
                                u32::try_from(width).context("FES media width is negative")?;
                            let height =
                                u32::try_from(height).context("FES media height is negative")?;
                            layer.clip = Some(Rect::new(0, 0, width, height));
                        }
                        let size = layer.clip.context("SIZE lost its dimensions")?;
                        layer.navigation.set_size([size.width as i32, size.height as i32]);
                        if layer.kind == "CIMAGE" {
                            rebuild_solid(layer)?;
                        } else if !layer.source.has_request()
                            && layer.file.as_ref().is_some_and(|file| !file.is_empty()) {
                            load_fes_media_image(layer, resources)?;
                        }
                    }
                    Change::Display(value) => {
                        layer.visible = value;
                        if value && layer.image.is_none() {
                            if layer.kind == "CIMAGE" {
                                rebuild_solid(layer)?;
                            } else if !layer.source.has_request()
                                && layer.file.as_ref().is_some_and(|file| !file.is_empty()) {
                                load_fes_media_image(layer, resources)?;
                            }
                        }
                        if let Some(plane) = self.planes.get_mut(name) {
                            plane.visible = value;
                        }
                    }
                    Change::Color(value) => {
                        if layer.kind != "CIMAGE" {
                            bail!("FES image color modulation requires a renderer color adapter");
                        }
                        layer.color = value;
                        rebuild_solid(layer)?;
                    }
                    Change::Alpha(value) => {
                        layer.opacity = value;
                        layer.fade = None;
                        if let Some(plane) = self.planes.get_mut(name) {
                            plane.opacity = value;
                        }
                    }
                    Change::Clip(value) => layer.clip = value,
                    Change::SetId(id) if layer.kind != "CIMAGE" => select_id(layer, id, resources)?,
                    Change::SetId(_) => {}
                    Change::Enable(value) if layer.kind == "BUTTON" => layer.enabled = value,
                    Change::Enable(_) => {}
                    Change::Fade { duration, from, to } => {
                        layer.visible = true;
                        if duration == 0 {
                            layer.opacity = to;
                            layer.fade = None;
                        } else {
                            layer.opacity = from;
                            layer.fade = Some((duration, i32::from(from), i32::from(to), 0));
                        }
                    }
                }
            }
            Target::Plane(name) => {
                if let Change::Clip(clip) = *change {
                    if !self.planes.contains_key(name) {
                        bail!("FES plane target disappeared");
                    }
                    let key = format!("{PLANE_CLIP}{name}");
                    match clip {
                        None => {
                            self.media.remove(&key);
                        }
                        Some(clip) => {
                            let mut style = internal_layer("DRIVER_PLANE_CLIP", &key);
                            style.clip = Some(clip);
                            self.media.insert(key, style);
                        }
                    }
                    return Ok(());
                }
                if let Change::Fade { duration, from, to } = *change {
                    let plane = self
                        .planes
                        .get_mut(name)
                        .context("FES plane target disappeared")?;
                    plane.visible = true;
                    if duration == 0 {
                        plane.opacity = to;
                        self.media.remove(&format!("{PLANE_FADE}{name}"));
                    } else {
                        plane.opacity = from;
                        let mut tween =
                            internal_layer("DRIVER_PLANE_FADE", &format!("{PLANE_FADE}{name}"));
                        tween.plane = Some(name.clone());
                        tween.opacity = from;
                        tween.fade = Some((duration, i32::from(from), i32::from(to), 0));
                        self.media.insert(tween.name.clone(), tween);
                    }
                    return Ok(());
                }
                if matches!(change, Change::Alpha(_)) {
                    self.media.remove(&format!("{PLANE_FADE}{name}"));
                }
                let plane = self
                    .planes
                    .get_mut(name)
                    .context("FES plane target disappeared")?;
                match *change {
                    Change::Position(x, y) => plane.position = Point::new(x, y),
                    Change::Size(width, height) => {
                        plane.metadata.size_dirty = true;
                        plane.size = [
                            u32::try_from(width).context("FES plane width is negative")?,
                            u32::try_from(height).context("FES plane height is negative")?,
                        ]
                    }
                    Change::Display(value) => plane.visible = value,
                    Change::Alpha(value) => plane.opacity = value,
                    Change::SetId(_) | Change::Enable(_) => {}
                    Change::Color(value) if value.r == 255 && value.g == 255 && value.b == 255 => {
                        plane.opacity = value.a
                    }
                    Change::Color(_) => bail!(
                        "FES plane RGB color requires a color field absent from this media model"
                    ),
                    Change::Clip(_) => {
                        unreachable!("plane clipping is handled before borrowing the plane")
                    }
                    Change::Fade { .. } => {
                        unreachable!("plane fades are handled before borrowing the plane")
                    }
                }
            }
        }
        Ok(())
    }

    fn driver_request_image(
        &mut self,
        key: &str,
        filename: &[u8],
        encoding: ScriptTextEncoding,
    ) -> Result<()> {
        let layer = self
            .media
            .get_mut(key)
            .context("FES load target disappeared")?;
        if !filename.is_empty() {
            let filename = decode(filename, encoding)?;
            if filename.contains("%LANG%") || filename.contains("%LANG_M%") {
                bail!("FES IMAGE load needs the host language expansion service before loading");
            }
            layer.file = Some(filename);
        }
        Ok(())
    }

    fn driver_animation(
        &mut self,
        targets: &[Target],
        subcommand: &str,
        args: &[Vec<u8>],
    ) -> Result<()> {
        // These are expanded string arguments. The bank was evaluated once
        // by the addressed dispatcher; STOP is the sole stopped mode.
        let argument = args.first().map(Vec::as_slice).unwrap_or_default();
        let argument = &argument[..argument.iter().position(|&byte| byte == 0)
            .unwrap_or(argument.len())];
        for target in targets {
            let Target::Image(name) = target else {
                continue;
            };
            let layer = self
                .media
                .get_mut(name)
                .context("FES animation target disappeared")?;
            if !matches!(layer.kind.as_str(), "IMAGE" | "BUTTON") {
                continue;
            }
            if subcommand == "setanm" {
                if argument.len() > 255 {
                    bail!("FES animation resource name exceeds 255 original bytes");
                }
                layer.animation_request.name = argument.to_vec();
            } else {
                layer.animation_request.playing = i32::from(!argument.eq_ignore_ascii_case(b"STOP"));
            }
            layer.animation_request.pending = true;
        }
        Ok(())
    }

    /// Call immediately after update_image_sources in finish_task_media,
    /// before extension finish and projection. False retains that boundary
    /// and view.now; retry here without rerunning update_driver_media/fades.
    /// Requests consume once; every later ANM slice uses the same owned source.
    pub(super) fn prepare_driver_animations(
        &mut self, host: &mut LoggingHost<'_>, view: &super::fes_tasks::FesTaskView,
    ) -> Result<bool> {
        // This is a bounded object-update slice, independent of the FES
        // instruction dispatcher. A false result retains the same boundary.
        const UPDATE_BUDGET: usize = 4096;
        let buttons = super::fes_task_platform::navigation_bindings(host, view.id);
        let random_state = &mut self.animation_random_state;
        for (key, layer) in &mut self.media {
            if !matches!(layer.kind.as_str(), "IMAGE" | "BUTTON") { continue; }
            layer.animation_request.validate_checkpoint()?;
            if layer.animation_request.update_at.is_some_and(|now| now != view.now) {
                bail!("FES IMAGE animation continuation changed task time before its boundary completed");
            }
            if layer.animation_request.pending {
                layer.animation_request.pending = false;
                layer.animation_request.update_at = None;
                layer.animation_request.completed_at = None;
                if layer.animation_request.playing == 0 {
                    if let Some(animation) = layer.animation.as_mut() { animation.playing = 0; }
                    layer.animation_request.completed_at = Some(view.now);
                    continue;
                }
                let mut name = decode(&layer.animation_request.name, host.text_encoding)?;
                // The original name setter turns the omitted reset name
                // into .anm before completing its category directory.
                if name.is_empty() { name.push_str(".anm"); }
                // The original completes the image-category path, then replaces
                // its extension. Reuse the real image manager's alias/router.
                let mut resolved = None;
                for candidate in host.resources.category_names(b"image", &name)? {
                    if let Some(path) = host.resolve_image_animation_resource(&candidate)? {
                        resolved = Some(path);
                        break;
                    }
                }
                let bytes = match resolved {
                    Some(path) => host.read_managed_raw_image_bytes(&path)?,
                    None => None,
                };
                // The list load receiver preserves an existing timeline for
                // an absent/empty file, then still receives the play command.
                // Routing/IO and malformed-file errors remain real errors.
                if let Some(bytes) = bytes.filter(|bytes| !bytes.is_empty()) {
                    // Raw sources already own their exact encoded HG directory. Do
                    // not reopen FILE or infer max from the ANM's display operands.
                    let source_bytes = if let Some(bytes) = layer.source.raw_image_bytes() {
                        std::borrow::Cow::Borrowed(bytes)
                    } else {
                        let image = layer.file.as_deref().context("FES animation playback requires its image source")?;
                        let mut bytes = None;
                        for candidate in host.resources.category_names(b"image", image)? {
                            if let Some(source) = host.read_managed_raw_image_bytes(&candidate)? {
                                bytes = Some(source);
                                break;
                            }
                        }
                        std::borrow::Cow::Owned(bytes.with_context(||
                            format!("FES IMAGE animation source {image:?} was not found"))?)
                    };
                    let maximum = cs2_formats::HgFile::parse(&source_bytes)
                        .context("FES animation source HG directory is malformed")?
                        .frames.iter().map(|frame| frame.id).max().unwrap_or(0);
                    let animation = layer.animation.get_or_insert_with(Default::default);
                    // Loading clears the old timeline even on a parse error, while
                    // preserving its playback switch and last completed display.
                    animation.load(&bytes, maximum).context("FES IMAGE animation is malformed")?;
                }
                let animation = layer.animation.get_or_insert_with(Default::default);
                animation.playing = 1;
                animation.reset_time_on_update = true;
                layer.animation_request.update_at = Some(view.now);
            }
            let previous_display = layer.animation_display;
            let Some(animation) = layer.animation.as_mut() else { continue; };
            let suspended = animation.timeline().is_some_and(|timeline| timeline.has_pending_update());
            let mut advanced = false;
            if layer.animation_request.update_at.is_some() || suspended
                || layer.animation_request.completed_at != Some(view.now) {
                let now = layer.animation_request.update_at.unwrap_or(view.now);
                layer.animation_request.update_at = Some(now);
                let mut random = || {
                    *random_state = random_state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    *random_state
                };
                let complete = animation.update_with_random(now, UPDATE_BUDGET, false, &mut random)
                    .context("FES IMAGE animation update failed")?;
                if !complete {
                    layer.animation_request.update_at = Some(now);
                    return Ok(false);
                }
                layer.animation_request.update_at = None;
                layer.animation_request.completed_at = Some(now);
                layer.animation_display = Some(animation.display());
                advanced = true;
            }
            let frame_changed = animation.frame_change().is_some()
                || previous_display.map(|display| display.frame_id)
                    != layer.animation_display.map(|display| display.frame_id);
            if frame_changed && !layer.source.has_raw_source() {
                let frame = animation.display().frame_id as u32;
                if let Some(name) = layer.file.as_deref() {
                    // Use the same category/router as the source producer;
                    // no extension/path guesses or ANM-operand max fallback.
                    for candidate in host.resources.category_names(b"image", name)? {
                        if let Some(image) = host.load_managed_image_projection(&candidate, frame)? {
                            image_source::install_ordinary_frame(layer, image);
                            break;
                        }
                    }
                }
            }
            if layer.kind == "BUTTON" && advanced {
                let (target, _, _) = buttons.iter().find(|(_, binding, _)| binding == key)
                    .context("BUTTON animation has no live source binding")?;
                super::fes_task_platform::mark_button_source_changed(host, view.id, &target.name, target.bank)?;
            }
        }
        Ok(true)
    }

    fn driver_base_image(
        &mut self,
        command: &FesDriverCommand,
        driver: &mut FesScriptDriver,
        expression: &mut ExpressionState,
        frame: u64,
        events: &mut Vec<LogicEvent>,
        encoding: ScriptTextEncoding,
    ) -> Result<bool> {
        let Some(source) = command
            .parameters
            .get(1)
            .filter(|source| !source.is_empty())
        else {
            self.media.insert(
                BACKGROUND_PENDING.to_owned(),
                internal_layer("DRIVER_CLEAR_BACKGROUND", BACKGROUND_PENDING),
            );
            return Ok(true);
        };
        let source = if source == b"@" {
            command
                .object_event
                .as_ref()
                .context("baseimg @ has no event object")?
                .parameter()
        } else {
            source.clone()
        };
        let (raw_name, bank) = split_target(&source, encoding)?;
        let name = decode(raw_name, encoding)?;
        let bank = match bank {
            Some(source) => {
                driver.evaluate_parameter(source, command.context.current, expression)?
            }
            None => 0,
        };
        if !self.has_driver_media(&name, driver, raw_name) {
            return Ok(true);
        }
        let bank_key = format!("{name}[{bank}]");
        if !self.media.contains_key(&bank_key)
            && !self.planes.contains_key(&bank_key)
            && !(bank == 0 && (self.media.contains_key(&name) || self.planes.contains_key(&name)))
        {
            return Ok(true);
        }
        let targets = self.driver_media_targets(&name, raw_name, Some(bank), driver)?;
        let Some(target) = targets.first() else {
            return Ok(true);
        };
        let Target::Image(key) = target else {
            let Target::Plane(name) = target else {
                unreachable!()
            };
            if driver
                .adapter
                .object_bank_definition(raw_name, bank as usize)
                .is_some_and(|definition| definition.kind_name.eq_ignore_ascii_case(b"CPLANE"))
            {
                bail!("CPLANE baseimg has no documented source-snapshot contract");
            }
            let mut pending = internal_layer("DRIVER_PLANE_BACKGROUND", BACKGROUND_PENDING);
            pending.plane = Some(name.clone());
            self.media.insert(BACKGROUND_PENDING.to_owned(), pending);
            return Ok(true);
        };
        let source = &self.media[key];
        let image = source
            .image
            .clone()
            .context("baseimg requires an already prepared image source")?;
        let mut background = source.clone();
        // The background owns the completed pixel snapshot, not the source's
        // retained frame directory. Cloning that handle would release one
        // canonical entry twice when the original bank and background die.
        background.source = ImageSourceState::default();
        background.animation_request = ImageAnimationRequest::default();
        background.name = BACKGROUND_PENDING.to_owned();
        background.kind = "DRIVER_IMAGE_BACKGROUND".to_owned();
        background.file = None;
        background.image = Some(image);
        background.color = Color::WHITE;
        background.position = Point::new(0, 0);
        background.plane = None;
        background.priority = i32::MIN;
        background.visible = true;
        background.opacity = 255;
        background.animation = None;
        background.animation_display = None;
        background.fade = None;
        background.clip = None;
        background.image_offset = [0; 2];
        background.image_base = [0; 2];
        background.visible = false;
        self.media.insert(BACKGROUND_PENDING.to_owned(), background);
        events.push(LogicEvent {
            frame,
            kind: "fes-media".into(),
            detail: "baseimg snapshot queued".into(),
        });
        Ok(true)
    }

    /// Consume immediately after a normal-body baseimg command, before pumping
    /// its next effect. System/object handlers leave it queued until the next
    /// ordinary body update. The canvas argument is needed only for PLANE and
    /// must contain that plane's completed shared-list pixels.
    pub(super) fn commit_driver_background(
        &mut self,
        canvas: Option<&cs2_renderer::RgbaFrame>,
    ) -> Result<bool> {
        let Some(pending) = self.media.get(BACKGROUND_PENDING) else {
            return Ok(false);
        };
        if pending.kind == "DRIVER_CLEAR_BACKGROUND" {
            self.media.remove(BACKGROUND_PENDING);
            self.media.remove(BACKGROUND);
            return Ok(true);
        }
        let mut background = pending.clone();
        if pending.kind == "DRIVER_PLANE_BACKGROUND" {
            let name = pending
                .plane
                .as_ref()
                .context("background plane identity is absent")?;
            let plane = self
                .planes
                .get(name)
                .context("background plane no longer exists")?;
            let canvas = canvas
                .context("PLANE baseimg needs an explicit completed shared-canvas snapshot")?;
            background.image = Some(crop_background(canvas, plane.vram, plane.size)?);
            background.plane = None;
            background.image_base = [0; 2];
            background.image_offset = [0; 2];
        }
        background.name = BACKGROUND.to_owned();
        background.kind = "DRIVER_BACKGROUND".to_owned();
        background.visible = true;
        self.media.insert(BACKGROUND.to_owned(), background);
        self.media.remove(BACKGROUND_PENDING);
        Ok(true)
    }
}

fn number(
    args: &[Vec<u8>],
    index: usize,
    default: Option<i32>,
    current: i32,
    driver: &mut FesScriptDriver,
    expression: &mut ExpressionState,
) -> Result<i32> {
    match args.get(index) {
        Some(source) => driver.evaluate_parameter(source, current, expression),
        None => {
            default.with_context(|| format!("FES media numeric argument {} is missing", index + 1))
        }
    }
}

fn media_kind(layer: &FesMediaLayer) -> bool {
    matches!(layer.kind.as_str(), "IMAGE" | "CIMAGE" | "DIFIMAGE" | "BUTTON" | "BGTABLE2" | "CHTABLE2" | "CHTABLE3")
}

fn internal_layer(kind: &str, name: &str) -> FesMediaLayer {
    FesMediaLayer {
        navigation: Default::default(),
        metadata: Default::default(),
        motion: None,
        name: name.to_owned(),
        kind: kind.to_owned(),
        file: None,
        source: Default::default(),
        frame_ids: Vec::new(),
        selected_id: 0,
        plane: None,
        position: Point::new(0, 0),
        priority: i32::MIN,
        clip: None,
        color: Color::TRANSPARENT,
        visible: false,
        enabled: false,
        opacity: 255,
        image: None,
        image_offset: [0; 2],
        image_base: [0; 2],
        animation: None,
        animation_request: Default::default(),
        animation_display: None,
        fade: None,
    }
}

fn crop_background(
    source: &cs2_renderer::RgbaFrame,
    origin: [i32; 2],
    size: [u32; 2],
) -> Result<cs2_renderer::RgbaFrame> {
    let [width, height] = size;
    let length = u64::from(width) * u64::from(height) * 4;
    if width == 0 || height == 0 || length > 64 * 1024 * 1024 {
        bail!("PLANE background dimensions are empty or exceed 64 MiB");
    }
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(length as usize)
        .context("PLANE background allocation failed")?;
    pixels.resize(length as usize, 0);
    let source_pixels = source.pixels();
    for y in 0..height {
        let sy = i64::from(origin[1]) + i64::from(y);
        if sy < 0 || sy >= i64::from(source.height()) {
            continue;
        }
        for x in 0..width {
            let sx = i64::from(origin[0]) + i64::from(x);
            if sx < 0 || sx >= i64::from(source.width()) {
                continue;
            }
            let input = ((sy as usize * source.width() as usize) + sx as usize) * 4;
            let output = ((y as usize * width as usize) + x as usize) * 4;
            pixels[output..output + 4].copy_from_slice(&source_pixels[input..input + 4]);
        }
    }
    Ok(cs2_renderer::RgbaFrame::from_rgba8(width, height, pixels)?)
}

fn alpha(value: i32) -> Result<u8> {
    u8::try_from(value).context("FES media alpha exceeds this desktop model's 0..255 range")
}

fn select_id(layer: &mut FesMediaLayer, id: i32, resources: &ResourceIndex) -> Result<()> {
    let id = if layer.frame_ids.is_empty() {
        id as u32
    } else {
        let Ok(index) = usize::try_from(id) else {
            return Ok(());
        };
        if index >= layer.frame_ids.len() {
            return Ok(());
        }
        id as u32
    };
    let selected = usize::try_from(id).context("FES frame ID exceeds host usize")?;
    layer.selected_id = selected;
    if layer.file.as_ref().is_some_and(|file| !file.is_empty()) {
        let frame = layer.frame_ids.get(selected).copied().unwrap_or(id);
        load_fes_media_frame(layer, resources, frame)?;
    }
    Ok(())
}

pub(super) fn rebuild_solid(layer: &mut FesMediaLayer) -> Result<()> {
    let clip = layer
        .clip
        .context("CIMAGE solid source needs an explicit size")?;
    if clip.width == 0 || clip.height == 0 {
        layer.image = None;
        return Ok(());
    }
    // CIMAGE recreates the underlying surface only when both axes differ.
    // A single changed axis retains the existing physical surface dimensions.
    let [width, height] = layer
        .image
        .as_ref()
        .filter(|image| image.width() == clip.width || image.height() == clip.height)
        .map(|image| [image.width(), image.height()])
        .unwrap_or([clip.width, clip.height]);
    if u64::from(width) * u64::from(height) * 4 > 64 * 1024 * 1024 {
        bail!("CIMAGE solid source exceeds 64 MiB");
    }
    layer.image =
        Some(solid_frame(width, height, layer.color).context("CIMAGE pixel allocation failed")?);
    Ok(())
}

fn ascii_command(bytes: &[u8]) -> Result<String> {
    if !bytes.is_ascii() || bytes.contains(&0) {
        bail!("FES media subcommand must be a non-NUL ASCII name");
    }
    Ok(std::str::from_utf8(bytes)?.to_ascii_lowercase())
}

fn decode(bytes: &[u8], encoding: ScriptTextEncoding) -> Result<String> {
    let bytes = &bytes[..bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len())];
    let codec = match encoding {
        ScriptTextEncoding::Cp932 => encoding_rs::SHIFT_JIS,
        ScriptTextEncoding::Gbk => encoding_rs::GBK,
        ScriptTextEncoding::Utf8 => encoding_rs::UTF_8,
    };
    let (text, failed) = codec.decode_without_bom_handling(bytes);
    if failed {
        bail!("FES media name is invalid for its explicitly selected script encoding");
    }
    Ok(text.into_owned())
}

pub(super) fn split_target(bytes: &[u8], encoding: ScriptTextEncoding) -> Result<(&[u8], Option<&[u8]>)> {
    let mut offset = 0;
    while offset < bytes.len() {
        let byte = bytes[offset];
        if byte == b'[' {
            if bytes.last() != Some(&b']') || offset == 0 {
                bail!("FES media bank reference is malformed");
            }
            return Ok((&bytes[..offset], Some(&bytes[offset + 1..bytes.len() - 1])));
        }
        let lead = match encoding {
            ScriptTextEncoding::Cp932 => matches!(byte, 0x81..=0x9f | 0xe0..=0xfc),
            ScriptTextEncoding::Gbk => matches!(byte, 0x81..=0xfe),
            ScriptTextEncoding::Utf8 => false,
        };
        offset += if lead { 2 } else { 1 };
    }
    Ok((bytes, None))
}

fn intersect(left: Rect, right: Rect) -> Result<Rect> {
    let x = i64::from(left.x).max(i64::from(right.x));
    let y = i64::from(left.y).max(i64::from(right.y));
    let end_x = (i64::from(left.x) + i64::from(left.width))
        .min(i64::from(right.x) + i64::from(right.width));
    let end_y = (i64::from(left.y) + i64::from(left.height))
        .min(i64::from(right.y) + i64::from(right.height));
    Ok(Rect::new(
        i32::try_from(x).context("FES clip X exceeds i32")?,
        i32::try_from(y).context("FES clip Y exceeds i32")?,
        u32::try_from((end_x - x).max(0)).context("FES clip width exceeds u32")?,
        u32::try_from((end_y - y).max(0)).context("FES clip height exceeds u32")?,
    ))
}
