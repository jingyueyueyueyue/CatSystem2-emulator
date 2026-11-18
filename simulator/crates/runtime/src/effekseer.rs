//! Concrete type61 manager and deterministic SKFE node simulation.
//!
//! PE contract: YukikoiMelt SHA256 below; ctor18b9f0, receiver18c1f0,
//! update18bed0, draw18be10. Resource parsing follows pinned MIT format
//! documentation/source behavior, independently implemented. No engine FFI.

use std::{collections::BTreeMap, sync::Arc};
#[path = "effekseer_curve.rs"]
mod curve;
#[path = "effekseer_force.rs"]
mod force;
#[path = "effekseer_format.rs"]
mod format;
#[path = "effekseer_geometry.rs"]
mod geometry;
#[path = "effekseer_ribbon.rs"]
mod ribbon;
#[path = "effekseer_transform.rs"]
mod transform;
#[path = "effekseer_tree.rs"]
mod tree;
#[path = "effekseer_trig.rs"]
mod trig;

pub const PE_SHA256: &str = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976";
pub const NATIVE_COMMANDS: [(u32, u32, usize); 3] =
    [(1014, 578, 8), (1015, 579, 4), (1016, 580, 8)];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Effekseer SKFE byte{at:#x}: {reason}")]
    Format { at: usize, reason: &'static str },
    #[error("Effekseer unsupported at byte{at:#x}: {branch}")]
    Unsupported { at: usize, branch: &'static str },
    #[error("Effekseer resource/instance capacity exceeded")]
    Capacity,
    #[error("Effekseer task {0} is not registered")]
    Task(u32),
    #[error("Effekseer host parameter: {0}")]
    Host(&'static str),
}

#[derive(Debug, Clone)]
pub(super) enum Curve3 {
    Keys([curve::FCurve; 3]),
    Fixed([f32; 3]),
    Pva {
        value: [f32; 3],
        velocity: [f32; 3],
        acceleration: [f32; 3],
    },
    Ease {
        start: [f32; 3],
        end: [f32; 3],
        abc: [f32; 3],
    },
}
impl Curve3 {
    fn value(&self, t: f32, life: f32) -> Result<[f32; 3], Error> {
        Ok(match self {
            Self::Keys(c) => [c[0].value(t)?, c[1].value(t)?, c[2].value(t)?],
            Self::Fixed(v) => *v,
            Self::Pva {
                value,
                velocity,
                acceleration,
            } => {
                std::array::from_fn(|i| value[i] + velocity[i] * t + acceleration[i] * t * t * 0.5)
            }
            Self::Ease { start, end, abc } => {
                let d = polynomial(*abc, t / life);
                std::array::from_fn(|i| start[i] + (end[i] - start[i]) * d)
            }
        })
    }
    pub(super) fn multiply(&mut self, k: f32) {
        self.multiply_axes([k; 3]);
    }
    pub(super) fn multiply_axes(&mut self, k: [f32; 3]) {
        match self {
            Self::Keys(c) => {
                for (i, value) in c.iter_mut().enumerate() {
                    value.multiply(k[i]);
                }
            }
            Self::Fixed(a) => {
                for (i, v) in a.iter_mut().enumerate() {
                    *v *= k[i];
                }
            }
            Self::Pva {
                value,
                velocity,
                acceleration,
            } => {
                for a in [value, velocity, acceleration] {
                    for (i, v) in a.iter_mut().enumerate() {
                        *v *= k[i];
                    }
                }
            }
            Self::Ease { start, end, .. } => {
                for a in [start, end] {
                    for (i, v) in a.iter_mut().enumerate() {
                        *v *= k[i];
                    }
                }
            }
        }
    }
}
fn polynomial(abc: [f32; 3], t: f32) -> f32 {
    abc[0] * t * t * t + abc[1] * t * t + abc[2] * t
}
#[derive(Debug, Clone)]
pub(super) struct Fade {
    frames: f32,
    abc: [f32; 3],
}
#[derive(Debug, Clone)]
pub(super) enum Uv {
    Fixed([f32; 4]),
    Keys([curve::FCurve; 4]),
    Scroll {
        rect: [f32; 4],
        speed: [f32; 2],
    },
    Animation {
        rect: [f32; 4],
        frames: i32,
        x: i32,
        y: i32,
        mode: i32,
    },
}
impl Uv {
    fn value(&self, t: f32) -> Result<[f32; 4], Error> {
        let value = match self {
            Self::Fixed(v) => *v,
            Self::Keys(c) => [
                c[0].value(t)?,
                c[1].value(t)?,
                c[2].value(t)?,
                c[3].value(t)?,
            ],
            Self::Scroll { rect, speed } => {
                // e2891..e28e0: signed CVTTSS2SI before the f32 multiply.
                if !t.is_finite() || !(-2147483648.0..2147483648.0).contains(&t) {
                    return Err(Error::Host("UV scroll outside native frame domain"));
                }
                let frame = (t as i32) as f32;
                [
                    rect[0] + speed[0] * frame,
                    rect[1] + speed[1] * frame,
                    rect[2],
                    rect[3],
                ]
            }
            Self::Animation {
                rect,
                frames,
                x,
                y,
                mode,
            } => {
                let frame = t / (*frames as f32);
                if !frame.is_finite() || !(-2147483648.0..2147483648.0).contains(&frame) {
                    return Err(Error::Host("UV animation outside native frame domain"));
                }
                let mut n = frame as i32;
                let count = x * y;
                if *mode == 0 {
                    n = n.min(count - 1);
                } else {
                    let rev = *mode == 2 && (n / count) % 2 == 1;
                    n %= count;
                    if rev {
                        n = count - 1 - n;
                    }
                }
                [
                    rect[0] + rect[2] * (n % x) as f32,
                    rect[1] + rect[3] * (n / x) as f32,
                    rect[2],
                    rect[3],
                ]
            }
        };
        if value.into_iter().all(f32::is_finite) {
            Ok(value)
        } else {
            Err(Error::Host("nonfinite SKFE UV output"))
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Node {
    generations: usize,
    bind: [i32; 3],
    life: f32,
    remove_life: bool,
    remove_parent: bool,
    remove_children: bool,
    interval: f32,
    offset: f32,
    translation: Curve3,
    rotation: Rotation,
    scaling: Curve3,
    absolute: force::Absolute,
    spawn: Spawn,
    effects_rotation: bool,
    children: Vec<usize>,
    renderer: Option<Renderer>,
}
impl Node {
    fn root(children: Vec<usize>) -> Self {
        Self {
            generations: 1,
            bind: [0; 3],
            life: f32::MAX,
            remove_life: false,
            remove_parent: false,
            remove_children: false,
            interval: 0.0,
            offset: 0.0,
            translation: Curve3::Fixed([0.0; 3]),
            rotation: Rotation::Euler(Curve3::Fixed([0.0; 3])),
            scaling: Curve3::Fixed([1.0; 3]),
            absolute: force::Absolute::None,
            spawn: Spawn::Point([0.0; 3]),
            effects_rotation: false,
            children,
            renderer: None,
        }
    }
}
#[derive(Debug, Clone)]
pub(super) enum Rotation {
    Euler(Curve3),
    Axis { axis: [f32; 3], angle: Curve3 },
}
impl Rotation {
    fn matrix(&self, age: f32, life: f32) -> Result<transform::Matrix, Error> {
        Ok(match self {
            Self::Euler(c) => transform::Matrix::euler(c.value(age, life)?),
            Self::Axis { axis, angle } => {
                transform::Matrix::axis(*axis, angle.value(age, life)?[0])
            }
        })
    }
}
#[derive(Debug, Clone)]
pub(super) enum Spawn {
    Point([f32; 3]),
    Sphere {
        radius: f32,
        x: f32,
        y: f32,
    },
    Circle {
        division: usize,
        radius: f32,
        start: f32,
        end: f32,
        reverse: bool,
    },
}
impl Spawn {
    fn matrix(&self, number: usize) -> transform::Matrix {
        use transform::Matrix;
        match self {
            Self::Point(p) => Matrix::translation(*p),
            Self::Sphere { radius, x, y } => Matrix::translation([0.0, *radius, 0.0])
                .then(Matrix::euler([*x, 0.0, 0.0]))
                .then(Matrix::euler([0.0, *y, 0.0])),
            Self::Circle {
                division,
                radius,
                start,
                end,
                reverse,
            } => {
                let mut index = number % division;
                if *reverse {
                    index = division - 1 - index;
                }
                let angle = (end - start) * (index as f32 / *division as f32) + start;
                Matrix::translation([0.0, *radius, 0.0]).then(Matrix::euler([0.0, 0.0, angle]))
            }
        }
    }
    fn magnify(&mut self, k: f32) {
        match self {
            Self::Point(p) => {
                for v in p {
                    *v *= k;
                }
            }
            Self::Sphere { radius, .. } | Self::Circle { radius, .. } => *radius *= k,
        }
    }
}
#[derive(Debug, Clone)]
pub(super) struct Renderer {
    texture: i32,
    blend: i32,
    filter: i32,
    wrap: i32,
    ztest: bool,
    zwrite: bool,
    fade_in: Option<Fade>,
    fade_out: Option<Fade>,
    uv: Uv,
    order: i32,
    billboard: i32,
    geometry: geometry::Geometry,
}
impl Renderer {
    fn fade(&self, age: f32, life: f32) -> Result<f32, Error> {
        // PE dd9d0..dda9b, sampled by rendered-instance update before the
        // instance clock increments. Neither easing result is clamped.
        let horner = |abc: [f32; 3], t: f32| ((abc[0] * t + abc[1]) * t + abc[2]) * t;
        let mut value = 1.0;
        if let Some(f) = &self.fade_in {
            if age < f.frames {
                value = horner(f.abc, age / f.frames);
            }
        }
        if let Some(f) = &self.fade_out {
            if age + f.frames > life {
                value = (1.0 - horner(f.abc, (age + f.frames - life) / f.frames)) * value;
            }
        }
        if value.is_finite() {
            Ok(value)
        } else {
            Err(Error::Host("nonfinite SKFE fade output"))
        }
    }
}
#[derive(Debug, Clone)]
pub struct Effect {
    pub version: i32,
    pub textures: Vec<String>,
    nodes: Vec<Node>,
}
impl Effect {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        format::parse(bytes)
    }
}

#[derive(Debug, Clone)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
impl Texture {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, Error> {
        let n = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(Error::Capacity)?;
        if width == 0 || height == 0 || n > 64 * 1024 * 1024 || rgba.len() != n {
            return Err(Error::Host("texture dimensions/byte count"));
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}
#[derive(Debug, Clone)]
pub struct Resource {
    pub effect: Effect,
    pub textures: Vec<Texture>,
}
impl Resource {
    pub fn new(effect: Effect, textures: Vec<Texture>) -> Result<Self, Error> {
        if textures.len() != effect.textures.len() {
            return Err(Error::Host("texture bank count"));
        }
        Ok(Self { effect, textures })
    }
}
#[derive(Debug, Clone, Copy)]
pub struct Vertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [u8; 4],
}
#[derive(Debug, Clone)]
pub struct Quad {
    pub resource: Arc<Resource>,
    pub texture: i32,
    pub blend: i32,
    pub filter: i32,
    pub wrap: i32,
    pub ztest: bool,
    pub zwrite: bool,
    pub vertices: [Vertex; 4],
}

#[derive(Debug, Clone)]
struct Playback {
    resource: Arc<Resource>,
    root: transform::Matrix,
    color: [u8; 4],
    tree: tree::Tree,
}
#[derive(Debug, Clone)]
struct Task {
    effect: Option<Arc<Resource>>,
    handle: Option<u32>,
    active: bool,
    repeat: u32,
    position: [f32; 3],
    scale: [f32; 3],
    color: [u8; 4],
    device_lost: bool,
}
impl Default for Task {
    fn default() -> Self {
        Self {
            effect: None,
            handle: None,
            active: false,
            repeat: 0,
            position: [0.0; 3],
            scale: [1.0; 3],
            color: [255; 4],
            device_lost: false,
        }
    }
}

#[derive(Debug)]
pub struct Manager {
    tasks: BTreeMap<u32, Task>,
    plays: BTreeMap<u32, Playback>,
    next: u32,
    projection_scale: [f32; 3],
    pub submitted_pixels: u64,
    pub draw_updates: u64,
}
impl Default for Manager {
    fn default() -> Self {
        Self {
            tasks: BTreeMap::new(),
            plays: BTreeMap::new(),
            next: 0,
            projection_scale: [1.0; 3],
            submitted_pixels: 0,
            draw_updates: 0,
        }
    }
}
impl Manager {
    pub fn register(&mut self, id: u32) -> Result<(), Error> {
        if self.tasks.contains_key(&id) {
            return Err(Error::Host("duplicate type61 registration"));
        }
        self.tasks.insert(id, Task::default());
        // All type61 tasks share the renderer. Each constructor18c720
        // overwrites its projection, rather than owning a per-task camera.
        self.projection_scale = [1.0; 3];
        Ok(())
    }
    pub fn contains(&self, id: u32) -> bool {
        self.tasks.contains_key(&id)
    }
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }
    pub fn playback_count(&self) -> usize {
        self.plays.len()
    }
    /// Receiver578 destroys old state before any resource IO/parse is attempted.
    pub fn begin_load(&mut self, id: u32) -> Result<(), Error> {
        let task = self.tasks.get_mut(&id).ok_or(Error::Task(id))?;
        if task.effect.is_some() {
            if let Some(h) = task.handle.take() {
                self.plays.remove(&h);
            }
        }
        task.active = false;
        task.effect = None;
        Ok(())
    }
    pub fn finish_load(&mut self, id: u32, resource: Arc<Resource>) -> Result<(), Error> {
        self.tasks.get_mut(&id).ok_or(Error::Task(id))?.effect = Some(resource);
        Ok(())
    }
    /// Receiver579 leaves handle and active untouched. Plays own independent Arcs.
    pub fn unload(&mut self, id: u32) -> Result<(), Error> {
        self.tasks.get_mut(&id).ok_or(Error::Task(id))?.effect = None;
        Ok(())
    }
    fn allocate_play(&mut self, id: u32) -> Result<(), Error> {
        let task = self.tasks.get_mut(&id).ok_or(Error::Task(id))?;
        let Some(resource) = task.effect.clone() else {
            return Ok(());
        };
        if self.plays.len() >= 2000 {
            return Err(Error::Capacity);
        }
        self.next = self.next.checked_add(1).ok_or(Error::Capacity)?;
        let tree = tree::Tree::new(&resource.effect, task.position);
        self.plays.insert(
            self.next,
            Playback {
                resource,
                root: transform::Matrix::translation(task.position),
                color: task.color,
                tree,
            },
        );
        // No Stop of the previous handle: it is still a shared-manager instance.
        task.handle = Some(self.next);
        task.active = true;
        Ok(())
    }
    pub fn play(&mut self, id: u32, repeat: u32) -> Result<(), Error> {
        let task = self.tasks.get_mut(&id).ok_or(Error::Task(id))?;
        if task.effect.is_none() {
            return Ok(());
        }
        task.repeat = repeat;
        self.allocate_play(id)
    }
    pub fn stop(&mut self, id: u32) -> Result<(), Error> {
        let task = self.tasks.get_mut(&id).ok_or(Error::Task(id))?;
        if task.effect.is_some() {
            if let Some(h) = task.handle.take() {
                self.plays.remove(&h);
            }
        }
        task.active = false;
        Ok(())
    }
    pub fn exists(&self, id: u32) -> Result<bool, Error> {
        let t = self.tasks.get(&id).ok_or(Error::Task(id))?;
        Ok(t.effect.is_some() && t.handle.is_some_and(|h| self.plays.contains_key(&h)))
    }
    /// PE18c34d tests the stored handle, including a handle that has expired
    /// but has not yet been cleared by the type61 update. No handle skips IO.
    pub fn rotation_reads_data(&self, id: u32) -> Result<bool, Error> {
        Ok(self.tasks.get(&id).ok_or(Error::Task(id))?.handle.is_some())
    }
    /// PE manager virtual+c8/d8600 replaces the CURRENT playback's root
    /// rotation, preserving row scales and translation. New/repeated Play
    /// creates an identity rotation and does not inherit this live command.
    pub fn set_live_rotation(&mut self, id: u32, radians: [f32; 3]) -> Result<(), Error> {
        let task = self.tasks.get(&id).ok_or(Error::Task(id))?;
        if let Some(play) = task.handle.and_then(|handle| self.plays.get_mut(&handle)) {
            play.root = play.root.replace_rotation(radians)?;
        }
        Ok(())
    }
    /// PE18bed0 checks Exists only. It does not advance time using elapsed ms.
    pub fn update_task(&mut self, id: u32) -> Result<(), Error> {
        let t = self.tasks.get_mut(&id).ok_or(Error::Task(id))?;
        if !(t.active && t.effect.is_some() && t.handle.is_some()) {
            return Ok(());
        }
        if t.handle.is_some_and(|h| self.plays.contains_key(&h)) {
            return Ok(());
        }
        t.handle = None;
        t.active = false;
        if t.repeat != 0 {
            self.allocate_play(id)?;
        }
        Ok(())
    }
    pub fn set_transform(
        &mut self,
        id: u32,
        position: [f32; 3],
        scale: [f32; 3],
        color: u32,
    ) -> Result<(), Error> {
        if !position.into_iter().chain(scale).all(f32::is_finite)
            || scale[0] == 0.0
            || scale[1] == 0.0
        {
            return Err(Error::Host("invalid task transform"));
        }
        let task = self.tasks.get_mut(&id).ok_or(Error::Task(id))?;
        task.position = [position[0] * 0.0625, position[1] * -0.0625, position[2]];
        if task.scale != scale {
            self.projection_scale = scale;
            task.scale = scale;
        }
        task.color = color.to_le_bytes();
        if let Some(p) = task.handle.and_then(|h| self.plays.get_mut(&h)) {
            p.root.0[3] = task.position;
            p.color = task.color;
        }
        Ok(())
    }
    pub fn release(&mut self, id: u32) {
        if let Some(t) = self.tasks.remove(&id) {
            if t.active {
                if let Some(h) = t.handle {
                    self.plays.remove(&h);
                }
            }
        }
        if self.tasks.is_empty() {
            self.plays.clear();
        }
    }
    /// PE18be10 manager.Update(1.0), followed by callback18c670 manager.Draw.
    /// Caller supplies logical graphics viewport, not elapsed time.
    pub fn draw_task(&mut self, id: u32, width: u32, height: u32) -> Result<Vec<Quad>, Error> {
        let t = self.tasks.get(&id).ok_or(Error::Task(id))?;
        if t.device_lost || t.effect.is_none() || t.handle.is_none() {
            return Ok(Vec::new());
        }
        if width == 0 || height == 0 {
            return Err(Error::Host("empty viewport"));
        }
        let scale = self.projection_scale;
        self.advance_frame()?;
        self.draw_updates = self.draw_updates.saturating_add(1);
        let mut quads = Vec::new();
        for play in self.plays.values() {
            for (node_index, particles) in play.tree.visible_groups(&play.resource.effect) {
                let node = &play.resource.effect.nodes[node_index];
                let s = node.renderer.as_ref().unwrap();
                if let geometry::Geometry::Ribbon(ribbon) = &s.geometry {
                    let group_quads = ribbon.quads(&particles, node, s, play.color)?;
                    if quads.len().saturating_add(group_quads.len()) > 262144 {
                        return Err(Error::Capacity);
                    }
                    for mut vertices in group_quads {
                        for vertex in &mut vertices {
                            if vertex
                                .position
                                .into_iter()
                                .chain(vertex.uv)
                                .any(|v| !v.is_finite())
                            {
                                return Err(Error::Host("nonfinite ribbon geometry/UV"));
                            }
                            vertex.position = project(vertex.position, scale, width, height);
                        }
                        quads.push(Quad {
                            resource: play.resource.clone(),
                            texture: s.texture,
                            blend: s.blend,
                            filter: s.filter,
                            wrap: s.wrap,
                            ztest: s.ztest,
                            zwrite: s.zwrite,
                            vertices,
                        });
                    }
                    continue;
                }
                for particle in particles {
                    let uv = s.uv.value(particle.age)?;
                    let fade = s.fade(particle.render_age, node.life)?;
                    let local_quads = s.geometry.quads(uv, particle.render_age, node.life, fade)?;
                    if quads.len().saturating_add(local_quads.len()) > 262144 {
                        return Err(Error::Capacity);
                    }
                    for mut vertices in local_quads {
                        for vertex in &mut vertices {
                            let world = particle
                                .world
                                .geometry_point(vertex.position, s.billboard)?;
                            if world.into_iter().chain(vertex.uv).any(|v| !v.is_finite()) {
                                return Err(Error::Host("nonfinite Effekseer geometry/UV"));
                            }
                            vertex.position = project(world, scale, width, height);
                            for k in 0..4 {
                                vertex.color[k] =
                                    geometry::modulate(vertex.color[k], play.color[k]);
                            }
                        }
                        quads.push(Quad {
                            resource: play.resource.clone(),
                            texture: s.texture,
                            blend: s.blend,
                            filter: s.filter,
                            wrap: s.wrap,
                            ztest: s.ztest,
                            zwrite: s.zwrite,
                            vertices,
                        });
                    }
                }
            }
        }
        Ok(quads)
    }
    fn advance_frame(&mut self) -> Result<(), Error> {
        // Flip removes only the root-only play whose child schedules ended.
        // Removing particles retain their pool slots for two group updates.
        self.plays
            .retain(|_, p| !p.tree.finished(&p.resource.effect));
        let mut pooled = self.plays.values().map(|p| p.tree.pooled()).sum::<usize>();
        for p in self.plays.values_mut() {
            p.tree.advance(&p.resource.effect, p.root, &mut pooled)?;
        }
        Ok(())
    }
}

fn inverse_sqrt(v: f32) -> f32 {
    // PE d3000: magic seed and two Newton steps.
    let mut r = f32::from_bits(0x5f3759df_u32.wrapping_sub(v.to_bits() >> 1));
    for _ in 0..2 {
        r *= 1.5 - 0.5 * v * r * r;
    }
    r
}
fn project(p: [f32; 3], scale: [f32; 3], w: u32, h: u32) -> [f32; 3] {
    // d3000 is target-eye (+Z), up cross forward (+X): the actual
    // LH matrix, despite an earlier report calling the helper LookAtRH.
    let forward = 100.0 * inverse_sqrt(10000.0);
    let right = forward * inverse_sqrt(forward * forward);
    let up = forward * right * inverse_sqrt(forward * right * forward * right);
    [
        w as f32 * 0.5 + p[0] * right * scale[0] * 16.0,
        h as f32 * 0.5 - p[1] * up * scale[1] * 16.0,
        ((p[2] + 100.0) * forward - 1.0) / 299.0,
    ]
}
