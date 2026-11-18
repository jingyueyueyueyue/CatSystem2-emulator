//! Checked SKFE reader, independently written from the pinned 1.01 format.
//! Unknown layouts are rejected at their byte offset, never rendered as empty.

use super::curve::FCurve;
use super::force::Absolute;
use super::geometry::{ColorCurve, Geometry, Ring};
use super::ribbon::Ribbon;
use super::{Curve3, Effect, Error, Fade, Node, Renderer, Rotation, Spawn, Uv};

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(n).ok_or(Error::Capacity)?;
        let v = self.bytes.get(self.at..end).ok_or(Error::Format {
            at: self.at,
            reason: "truncated SKFE",
        })?;
        self.at = end;
        Ok(v)
    }
    fn int(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn float(&mut self) -> Result<f32, Error> {
        let v = f32::from_le_bytes(self.take(4)?.try_into().unwrap());
        if !v.is_finite() {
            return self.bad("nonfinite parameter");
        }
        Ok(v)
    }
    fn bad<T>(&self, reason: &'static str) -> Result<T, Error> {
        Err(Error::Format {
            at: self.at,
            reason,
        })
    }
    fn unsupported<T>(&self, branch: &'static str) -> Result<T, Error> {
        Err(Error::Unsupported {
            at: self.at,
            branch,
        })
    }
    fn count(&mut self, limit: usize) -> Result<usize, Error> {
        let v = self.int()?;
        if v < 0 || v as usize > limit {
            return self.bad("count outside resource bound");
        }
        Ok(v as usize)
    }
    fn vec3(&mut self) -> Result<[f32; 3], Error> {
        Ok([self.float()?, self.float()?, self.float()?])
    }
    fn range(&mut self) -> Result<f32, Error> {
        let max = self.float()?;
        let min = self.float()?;
        if max != min {
            return self.unsupported("nondegenerate random float (native RNG binding pending)");
        }
        Ok(min)
    }
    fn range_vec(&mut self) -> Result<[f32; 3], Error> {
        let max = self.vec3()?;
        let min = self.vec3()?;
        if max != min {
            return self.unsupported("nondegenerate random vector (native RNG binding pending)");
        }
        Ok(min)
    }
    fn paths(&mut self) -> Result<Vec<String>, Error> {
        let count = self.count(4096)?;
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let n = self.count(32768)?;
            let raw = self.take(n.checked_mul(2).ok_or(Error::Capacity)?)?;
            let mut units = raw
                .chunks_exact(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect::<Vec<_>>();
            if units.pop() != Some(0) || units.contains(&0) {
                return self.bad("texture path must end with one UTF16 NUL");
            }
            out.push(String::from_utf16(&units).map_err(|_| Error::Format {
                at: self.at,
                reason: "invalid UTF16 path",
            })?);
        }
        Ok(out)
    }
    fn curve(&mut self, default: [f32; 3], scaling: bool) -> Result<Curve3, Error> {
        let kind = self.int()?;
        if kind == 0x7ffffffe {
            return Ok(Curve3::Fixed(default));
        }
        let n = self.count(1 << 20)?;
        let end = self.at.checked_add(n).ok_or(Error::Capacity)?;
        let result = match kind {
            v if v == if scaling { 5 } else { 3 } => Curve3::Keys([
                self.fcurve(default[0])?,
                self.fcurve(default[1])?,
                self.fcurve(default[2])?,
            ]),
            0 => Curve3::Fixed(self.vec3()?),
            1 => Curve3::Pva {
                value: self.range_vec()?,
                velocity: self.range_vec()?,
                acceleration: self.range_vec()?,
            },
            2 => Curve3::Ease {
                start: self.range_vec()?,
                end: self.range_vec()?,
                abc: self.vec3()?,
            },
            3 if scaling => {
                let v = self.range()?;
                let d = self.range()?;
                let a = self.range()?;
                Curve3::Pva {
                    value: [v; 3],
                    velocity: [d; 3],
                    acceleration: [a; 3],
                }
            }
            4 if scaling => {
                let a = self.range()?;
                let b = self.range()?;
                Curve3::Ease {
                    start: [a; 3],
                    end: [b; 3],
                    abc: self.vec3()?,
                }
            }
            _ => return self.unsupported("unknown transform tag"),
        };
        if self.at != end {
            return self.bad("transform payload size mismatch");
        }
        Ok(result)
    }
    fn fcurve(&mut self, default: f32) -> Result<FCurve, Error> {
        let edges = [self.int()?, self.int()?];
        if edges.into_iter().any(|e| !(0..=2).contains(&e)) {
            return self.bad("FCurve edge tag");
        }
        let random_offset = self.range()?;
        let frame_offset = self.int()?;
        let length = self.int()?;
        let frequency = self.int()?;
        let count = self.count(262144)?;
        let mut keys = Vec::with_capacity(count);
        for _ in 0..count {
            keys.push(self.float()?);
        }
        if length < 0
            || (count != 0 && frequency <= 0)
            || (count != 0 && length == 0 && edges != [0; 2])
        {
            return self.bad("FCurve invalid length/frequency/zero-length loop");
        }
        // Each interpolated interval reads index+1, including the final
        // partial frequency interval. Refuse out-of-range native addresses.
        if count != 0
            && length > 0
            && count < ((length as i64 + frequency as i64 - 1) / frequency as i64 + 1) as usize
        {
            return self.unsupported("FCurve key storage lacks native next-key access");
        }
        Ok(FCurve {
            edges,
            random_offset,
            frame_offset,
            length,
            frequency,
            keys,
            default,
        })
    }
    fn fade(&mut self) -> Result<Option<Fade>, Error> {
        match self.int()? {
            0 => Ok(None),
            1 => {
                let frames = self.float()?;
                let abc = self.vec3()?;
                if frames < 0.0 {
                    return self.bad("negative fade length");
                }
                Ok(Some(Fade { frames, abc }))
            }
            _ => self.bad("unknown fade tag"),
        }
    }
    fn rotation(&mut self) -> Result<Rotation, Error> {
        let kind = self.int()?;
        if !matches!(kind, 3 | 4) {
            self.at -= 4;
            // Rotation5 and scaling5 share the sampled-vector layout;
            // rotation3/4 were handled above as axis forms.
            return Ok(Rotation::Euler(self.curve([0.0; 3], kind == 5)?));
        }
        let n = self.count(1024)?;
        let end = self.at.checked_add(n).ok_or(Error::Capacity)?;
        let axis = self.range_vec()?;
        let angle = if kind == 3 {
            Curve3::Pva {
                value: [self.range()?; 3],
                velocity: [self.range()?; 3],
                acceleration: [self.range()?; 3],
            }
        } else {
            Curve3::Ease {
                start: [self.range()?; 3],
                end: [self.range()?; 3],
                abc: self.vec3()?,
            }
        };
        if self.at != end {
            return self.bad("axis rotation payload size");
        }
        Ok(Rotation::Axis { axis, angle })
    }
    fn rect(&mut self) -> Result<[f32; 4], Error> {
        Ok([self.float()?, self.float()?, self.float()?, self.float()?])
    }
    fn color(&mut self) -> Result<[u8; 4], Error> {
        Ok(self.take(4)?.try_into().unwrap())
    }
    fn range_color(&mut self, version: i32) -> Result<([u8; 4], bool), Error> {
        let hsva = if version >= 4 {
            let flags = self.take(2)?;
            if flags[0] > 1 {
                return self.unsupported("unknown random color space");
            }
            flags[0] == 1
        } else {
            false
        };
        let max = self.color()?;
        let min = self.color()?;
        if max != min {
            return self.unsupported("nondegenerate random color/native RNG");
        }
        Ok((min, hsva))
    }
    fn color_curve(&mut self, version: i32, sampled: bool) -> Result<ColorCurve, Error> {
        match self.int()? {
            0 => Ok(ColorCurve::Fixed(self.color()?)),
            1 => {
                let (raw, hsva) = self.range_color(version)?;
                Ok(if hsva {
                    ColorCurve::FixedHsva(raw)
                } else {
                    ColorCurve::Fixed(raw)
                })
            }
            2 => {
                let (start, hsva) = self.range_color(version)?;
                let (end, _) = self.range_color(version)?;
                // Target ed946/f0d9e checks only start.mode, after raw
                // endpoint interpolation. end.mode does not select output.
                Ok(ColorCurve::Ease {
                    start,
                    end,
                    abc: self.vec3()?,
                    hsva,
                })
            }
            3 if sampled => Ok(ColorCurve::Keys([
                self.fcurve(255.0)?,
                self.fcurve(255.0)?,
                self.fcurve(255.0)?,
                self.fcurve(255.0)?,
            ])),
            _ => self.unsupported("color FCurve/unknown tag"),
        }
    }
    fn ring_scalar(&mut self) -> Result<Curve3, Error> {
        match self.int()? {
            0 => Ok(Curve3::Fixed([self.float()?; 3])),
            1 => Ok(Curve3::Fixed([self.range()?; 3])),
            2 => Ok(Curve3::Ease {
                start: [self.range()?; 3],
                end: [self.range()?; 3],
                abc: self.vec3()?,
            }),
            _ => self.bad("ring scalar tag"),
        }
    }
    fn vec2(&mut self) -> Result<[f32; 3], Error> {
        Ok([self.float()?, self.float()?, 0.0])
    }
    fn range_vec2(&mut self) -> Result<[f32; 3], Error> {
        let max = self.vec2()?;
        let min = self.vec2()?;
        if max != min {
            return self.unsupported("nondegenerate random2D vector/native RNG");
        }
        Ok(min)
    }
    fn ring_location(&mut self) -> Result<Curve3, Error> {
        match self.int()? {
            0 => Ok(Curve3::Fixed(self.vec2()?)),
            1 => Ok(Curve3::Pva {
                value: self.range_vec2()?,
                velocity: self.range_vec2()?,
                acceleration: self.range_vec2()?,
            }),
            2 => Ok(Curve3::Ease {
                start: self.range_vec2()?,
                end: self.range_vec2()?,
                abc: self.vec3()?,
            }),
            _ => self.bad("ring location tag"),
        }
    }
    fn node(
        &mut self,
        version: i32,
        magnification: f32,
        nodes: &mut Vec<Node>,
        depth: usize,
    ) -> Result<usize, Error> {
        if depth > 64 || nodes.len() >= 4096 {
            return Err(Error::Capacity);
        }
        let kind = self.int()?;
        if !matches!(kind, 0 | 2 | 3 | 4) {
            return self.unsupported("model/track renderer");
        }
        let index = nodes.len();
        nodes.push(Node::root(Vec::new()));
        let n = self.int()?;
        if n != if version >= 9 { 52 } else { 44 } {
            return self.bad("common payload size");
        }
        // MaxGeneration is a lifetime total, not the simultaneous pool size.
        // Long emitters may create >2000 particles while recycling old ones.
        let generations = self.count(i32::MAX as usize)?;
        let translation_bind = self.int()?;
        let rotation_bind = self.int()?;
        let scale_bind = self.int()?;
        if !(0..=3).contains(&translation_bind)
            || !(0..=3).contains(&rotation_bind)
            || !(0..=3).contains(&scale_bind)
        {
            return self.bad("unknown parent binding");
        }
        let remove_life = self.int()? != 0;
        let remove_parent = self.int()? != 0;
        let remove_children = self.int()? != 0;
        let max = self.int()?;
        let min = self.int()?;
        if max != min {
            return self.unsupported("random lifetime");
        }
        if min <= 0 {
            return self.bad("nonpositive sprite lifetime");
        }
        let life = min as f32;
        let interval = if version >= 9 {
            self.range()?
        } else {
            self.float()?
        };
        let offset = if version >= 9 {
            self.range()?
        } else {
            self.float()?
        };
        if offset < 0.0 {
            return self.unsupported("negative generation offset/prewarm");
        }
        let mut translation = self.curve([0.0; 3], false)?;
        let abs_kind = self.int()?;
        let abs_size = self.count(1024)?;
        let mut absolute = match (abs_kind, abs_size) {
            (0, 0) => Absolute::None,
            (1, 12) => Absolute::Gravity(self.vec3()?),
            (2, 16) => Absolute::Attraction {
                force: self.float()?,
                control: self.float()?,
                min_range: self.float()?,
                max_range: self.float()?,
            },
            _ => return self.unsupported("absolute controller tag/size"),
        };
        let mut rotation = self.rotation()?;
        let scaling = self.curve([1.0; 3], true)?;
        let effects_rotation = self.int()? != 0;
        let mut spawn = match self.int()? {
            0 => Spawn::Point(self.range_vec()?),
            1 => Spawn::Sphere {
                radius: self.range()?,
                x: self.range()?,
                y: self.range()?,
            },
            3 => {
                let division = self.int()?.max(1) as usize;
                let radius = self.range()?;
                let start = self.range()?;
                let end = self.range()?;
                let order = self.int()?;
                if order == 0 {
                    return self.unsupported("random circle index/native RNG");
                }
                if !matches!(order, 1 | 2) {
                    return self.bad("circle order tag");
                }
                Spawn::Circle {
                    division,
                    radius,
                    start,
                    end,
                    reverse: order == 2,
                }
            }
            _ => return self.unsupported("model/line generation location"),
        };
        if version >= 8 {
            translation.multiply(magnification);
            absolute.magnify(magnification);
            spawn.magnify(magnification);
        } else if magnification != 1.0 {
            return self.unsupported("legacy external magnification");
        }
        // PE18bc99 passes0 to manager virtual+38 (d7eb0), setting LH.
        // Common reader dd24d..dd59a applies these conversions. Generation
        // points and absolute gravity are intentionally not negated there.
        // PE LH conversion has no translation-FCurve case; do not apply
        // fixed/PVA/easing's Z negation to sampled translation keys.
        if !matches!(&translation, Curve3::Keys(_)) {
            translation.multiply_axes([1.0, 1.0, -1.0]);
        }
        match &mut rotation {
            Rotation::Euler(c) => c.multiply_axes([-1.0, -1.0, 1.0]),
            Rotation::Axis { axis, .. } => axis[2] = -axis[2],
        }
        if let Spawn::Sphere { x, y, .. } = &mut spawn {
            *x = -*x;
            *y = -*y;
        }
        let mut texture = -1;
        let mut blend = 1;
        let mut filter = 0;
        let mut wrap = 0;
        let mut ztest = false;
        let mut zwrite = false;
        let mut fade_in = None;
        let mut fade_out = None;
        let mut uv = Uv::Fixed([0.0, 0.0, 1.0, 1.0]);
        if version >= 3 {
            texture = self.int()?;
            blend = self.int()?;
            filter = self.int()?;
            wrap = self.int()?;
            if version >= 5 {
                ztest = self.int()? != 0;
                zwrite = self.int()? != 0;
            } else {
                ztest = true;
            }
            fade_in = self.fade()?;
            fade_out = self.fade()?;
            uv = match self.int()? {
                0 => uv,
                1 => Uv::Fixed(self.rect()?),
                2 => {
                    let rect = self.rect()?;
                    let frames = self.int()?;
                    let x = self.int()?;
                    let y = self.int()?;
                    let mode = self.int()?;
                    if frames <= 0
                        || x <= 0
                        || y <= 0
                        || x.checked_mul(y).is_none()
                        || !(0..=2).contains(&mode)
                    {
                        return self.bad("UV animation bounds");
                    }
                    Uv::Animation {
                        rect,
                        frames,
                        x,
                        y,
                        mode,
                    }
                }
                3 => Uv::Scroll {
                    rect: self.rect()?,
                    speed: [self.float()?, self.float()?],
                },
                // PE dbdaf..dbdf6 reads two Vector2 curves, position then
                // size. Their scalar empty defaults are all0 (ee050).
                4 => Uv::Keys([
                    self.fcurve(0.0)?,
                    self.fcurve(0.0)?,
                    self.fcurve(0.0)?,
                    self.fcurve(0.0)?,
                ]),
                _ => return self.unsupported("unknown UV layout"),
            };
            if version >= 9 {
                let distortion = self.int()?;
                let _intensity = self.float()?;
                if distortion != 0 {
                    return self.unsupported("distortion/background sampling");
                }
            }
        }
        let renderer = if kind == 0 {
            if self.int()? != 0 {
                return self.bad("none renderer tag");
            }
            None
        } else {
            if self.int()? != kind {
                return self.bad("node/renderer tag mismatch");
            }
            // Ribbon has no order field: its first parameter is legacy
            // AlphaBlend or (>=3) ViewpointDependent, per PE ee2aa..ee2d9.
            let order = if kind == 3 { 0 } else { self.int()? };
            if !(0..=1).contains(&order) {
                return self.bad("sprite order tag");
            }
            if version < 3 {
                blend = self.int()?;
            }
            let billboard = if kind == 3 { 2 } else { self.int()? };
            if !(0..=3).contains(&billboard) {
                return self.bad("billboard tag");
            }
            let geometry = if kind == 3 {
                let viewpoint = self.int()? != 0;
                let color = self.color_curve(version, false)?;
                let colors = match self.int()? {
                    0 => None,
                    1 => Some([self.color()?, self.color()?]),
                    _ => return self.bad("ribbon edge color tag"),
                };
                let position_type = self.int()?;
                let mut positions = [-0.5, 0.5];
                if position_type == 1 || (position_type == 0 && version >= 8) {
                    positions = [self.float()?, self.float()?];
                } else if position_type != 0 {
                    return self.bad("ribbon position tag");
                }
                if version >= 8 {
                    positions.iter_mut().for_each(|v| *v *= magnification);
                }
                Geometry::Ribbon(Ribbon {
                    color,
                    colors,
                    positions,
                    viewpoint,
                })
            } else if kind == 4 {
                let segments = self.count(1024)?;
                if segments == 0 {
                    return self.bad("zero ring segment count");
                }
                let angle = self.ring_scalar()?;
                let mut outer = self.ring_location()?;
                let mut inner = self.ring_location()?;
                let ratio = self.ring_scalar()?;
                let colors = [
                    self.color_curve(version, false)?,
                    self.color_curve(version, false)?,
                    self.color_curve(version, false)?,
                ];
                // Type61 explicitly selects LH. Ring radial height is the
                // second location component; magnification applies at>=8.
                let k = if version >= 8 { magnification } else { 1.0 };
                outer.multiply_axes([k, -k, k]);
                inner.multiply_axes([k, -k, k]);
                Geometry::Ring(Ring {
                    segments,
                    angle,
                    outer,
                    inner,
                    ratio,
                    colors,
                })
            } else {
                let color = self.color_curve(version, true)?;
                let corners = match self.int()? {
                    0 => None,
                    1 => Some([self.color()?, self.color()?, self.color()?, self.color()?]),
                    _ => return self.bad("corner color tag"),
                };
                let position_type = self.int()?;
                let mut positions = [[-0.5, -0.5], [0.5, -0.5], [-0.5, 0.5], [0.5, 0.5]];
                if position_type == 1 || (position_type == 0 && version >= 8) {
                    for p in &mut positions {
                        *p = [self.float()?, self.float()?];
                    }
                } else if position_type != 0 {
                    return self.bad("sprite position tag");
                }
                if version >= 8 {
                    for p in &mut positions {
                        p[0] *= magnification;
                        p[1] *= magnification;
                    }
                }
                Geometry::Sprite {
                    color,
                    corners,
                    positions,
                }
            };
            if version < 3 {
                texture = self.int()?;
            }
            if !(0..=4).contains(&blend) || !(0..=1).contains(&filter) || !(0..=1).contains(&wrap) {
                return self.bad("texture state tag");
            }
            Some(Renderer {
                texture,
                blend,
                filter,
                wrap,
                ztest,
                zwrite,
                fade_in,
                fade_out,
                uv,
                order,
                billboard,
                geometry,
            })
        };
        if version >= 1 && self.int()? != 0 {
            return self.unsupported("embedded sound node");
        }
        let count = self.count(4096)?;
        // Target instance storage has exactly16 child emitters. Reject larger
        // topology explicitly instead of silently discarding child nodes.
        if count > 16 {
            return self.unsupported("more than16 child emitters per node");
        }
        let mut children = Vec::with_capacity(count);
        for _ in 0..count {
            children.push(self.node(version, magnification, nodes, depth + 1)?);
        }
        nodes[index] = Node {
            generations,
            bind: [translation_bind, rotation_bind, scale_bind],
            life,
            remove_life,
            remove_parent,
            remove_children,
            interval,
            offset,
            translation,
            rotation,
            scaling,
            absolute,
            spawn,
            effects_rotation,
            children,
            renderer,
        };
        Ok(index)
    }
}

pub(super) fn parse(bytes: &[u8]) -> Result<Effect, Error> {
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(Error::Capacity);
    }
    let mut r = Reader { bytes, at: 0 };
    if r.take(4)? != b"SKFE" {
        return r.bad("SKFE magic");
    }
    let version = r.int()?;
    // The target PE additionally has >=11/13 layouts. Their fields must not
    // be consumed using the older public 1.01 node layout.
    if !(0..=9).contains(&version) {
        return r.unsupported("SKFE version outside independently recovered 0..9 layout");
    }
    let textures = r.paths()?;
    if version >= 9 {
        if !r.paths()?.is_empty() || !r.paths()?.is_empty() {
            return r.unsupported("normal/distortion texture banks");
        }
    }
    if version >= 1 && !r.paths()?.is_empty() {
        return r.unsupported("wave resource bank");
    }
    if version >= 6 && !r.paths()?.is_empty() {
        return r.unsupported("model resource bank");
    }
    let magnification = if version >= 2 { r.float()? } else { 1.0 };
    if magnification <= 0.0 {
        return r.bad("nonpositive magnification");
    }
    if version >= 9 {
        match r.int()? {
            0 => {}
            1 => {
                let _radius = r.float()?;
                let _center = r.vec3()?;
            }
            _ => return r.bad("culling shape"),
        }
    }
    if r.int()? != -1 {
        return r.bad("missing root node");
    }
    let count = r.count(4096)?;
    if count > 16 {
        return r.unsupported("more than16 root child emitters");
    }
    let mut nodes = vec![Node::root(Vec::new())];
    let mut children = Vec::with_capacity(count);
    for _ in 0..count {
        children.push(r.node(version, magnification, &mut nodes, 1)?);
    }
    nodes[0].children = children;
    for node in &nodes {
        if let Some(s) = &node.renderer {
            if s.texture < -1 || s.texture >= textures.len() as i32 {
                return r.bad("color texture index");
            }
        }
    }
    if r.at != bytes.len() {
        return r.unsupported("trailing SKFE extension/options");
    }
    Ok(Effect {
        version,
        textures,
        nodes,
    })
}
