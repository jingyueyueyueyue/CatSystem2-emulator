//! Type61 concrete resource/manager/CPU raster receiver. No optional adapter.
//! PE63de0e14: wrappers1ac7e0/1ac860/1ac8a0 -> receiver18c1f0.

use super::native_tasks::OwnedTaskBuffers;
use super::{LoggingHost, SceneNode, expand_kcs_format, read_u32};
use cs2_renderer::Canvas;
use cs2_runtime::effekseer::{
    Effect, Error, Manager, NATIVE_COMMANDS, PE_SHA256, Quad, Resource, Texture, Vertex,
};
use cs2_runtime::{ResourceIndex, TaskCommand, TaskCommandOutcome};
use cs2_script::{NativeReturn, Result, ScriptError, Vm};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct EffekseerStore {
    pub(super) manager: Manager,
}
fn error(e: impl std::fmt::Display) -> ScriptError {
    super::native_state_error_owned(format!("type61 Effekseer: {e}"))
}
pub(super) fn identified(profile: Option<&cs2_runtime::native_profile::FeatureProfile>) -> bool {
    profile.is_some_and(|p| p.pe_sha256.eq_ignore_ascii_case(PE_SHA256))
}
pub(super) fn supports(index: u32) -> bool {
    NATIVE_COMMANDS.iter().any(|d| d.0 == index)
}

impl EffekseerStore {
    fn sync(&mut self, node: &SceneNode) -> std::result::Result<(), Error> {
        self.manager
            .set_transform(node.id, node.position, node.scale, node.color)
    }
    pub(super) fn draw_scene_task(
        &mut self,
        node: &SceneNode,
        canvas: &mut Canvas,
    ) -> std::result::Result<bool, Error> {
        if node.kind != 61 {
            return Ok(false);
        }
        if !self.manager.contains(node.id) {
            return Err(Error::Task(node.id));
        }
        self.sync(node)?;
        let quads = self
            .manager
            .draw_task(node.id, canvas.width(), canvas.height())?;
        let count = raster(canvas, &quads)?;
        self.manager.submitted_pixels = self.manager.submitted_pixels.saturating_add(count);
        Ok(true)
    }
}

/// Owned resource bytes are consumed synchronously and never retained as a VM pointer.
pub(super) fn receive(
    host: &mut LoggingHost<'_>,
    command: TaskCommand,
    buffers: &mut OwnedTaskBuffers,
    output: &mut u32,
    _budget: usize,
) -> Result<Option<TaskCommandOutcome>> {
    if !matches!(command.code, 51 | 55 | 59 | 66 | 578..=582) {
        return Ok(None);
    }
    let id = command.target as u32;
    if !identified(host.data_services.native_profile) {
        return Err(error("unidentified Effekseer PE profile"));
    }
    if !host.data_services.effekseer.manager.contains(id) {
        return Err(error(Error::Task(id)));
    }
    if let Some(node) = host.scene_nodes.get(&id) {
        host.data_services.effekseer.sync(node).map_err(error)?;
    }
    match command.code {
        51 | 66 => {
            let bytes = buffers
                .data1()
                .filter(|b| b.len() >= 12)
                .ok_or_else(|| error("position/scale command needs owned three-float data1"))?;
            let value = std::array::from_fn::<_, 3, _>(|i| {
                f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
            });
            if !value.into_iter().all(f32::is_finite) {
                return Err(error("nonfinite task position/scale"));
            }
            let node = host
                .scene_nodes
                .get_mut(&id)
                .ok_or_else(|| error(Error::Task(id)))?;
            if command.code == 51 {
                node.position = value;
            } else {
                node.scale = value;
            }
            host.data_services.effekseer.sync(node).map_err(error)?;
        }
        55 => {
            // 18c383 always broadcasts56 through1376b0. Preserve an
            // explicit boundary for children until their actual receivers
            // and resumable delegation are installed; never silently drop it.
            if host.task_parents.iter().any(|(&child, &parent)| {
                parent == id && child != id && host.data_services.tasks.task_is_registered(child)
            }) {
                return Err(error(
                    "Effekseer rotation child command56 receiver chain is unrecovered",
                ));
            }
            if host
                .data_services
                .effekseer
                .manager
                .rotation_reads_data(id)
                .map_err(error)?
            {
                let bytes = buffers
                    .data1()
                    .filter(|bytes| bytes.len() >= 12)
                    .ok_or_else(|| error("live rotation requires owned three-float radians"))?;
                let radians = std::array::from_fn(|i| {
                    f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
                });
                host.data_services
                    .effekseer
                    .manager
                    .set_live_rotation(id, radians)
                    .map_err(error)?;
            }
        }
        59 => {
            let node = host
                .scene_nodes
                .get_mut(&id)
                .ok_or_else(|| error(Error::Task(id)))?;
            node.color = command.data[1];
            host.data_services.effekseer.sync(node).map_err(error)?;
        }
        578 => {
            // Failure leaves the old state released, matching18c444.
            host.data_services
                .effekseer
                .manager
                .begin_load(id)
                .map_err(error)?;
            let raw = buffers
                .data1()
                .ok_or_else(|| error("load requires owned CP932 path"))?;
            let name = &raw[..raw.iter().position(|b| *b == 0).unwrap_or(raw.len())];
            if !name.is_empty() {
                let resource = read_resource(host.resources, name)?;
                host.data_services
                    .effekseer
                    .manager
                    .finish_load(id, Arc::new(resource))
                    .map_err(error)?;
            }
            if let Some(node) = host.scene_nodes.get_mut(&id) {
                node.initialized = true;
            }
        }
        579 => host
            .data_services
            .effekseer
            .manager
            .unload(id)
            .map_err(error)?,
        580 => host
            .data_services
            .effekseer
            .manager
            .play(id, command.data[1])
            .map_err(error)?,
        581 => host
            .data_services
            .effekseer
            .manager
            .stop(id)
            .map_err(error)?,
        582 => {
            *output = host
                .data_services
                .effekseer
                .manager
                .exists(id)
                .map_err(error)? as u32;
            return Ok(Some(TaskCommandOutcome {
                done: true,
                propagate: false,
            }));
        }
        _ => unreachable!(),
    }
    *output = 0; // PE command receiver success value, distinct from native Continue.
    Ok(Some(TaskCommandOutcome {
        done: true,
        propagate: false,
    }))
}

fn cp932(bytes: &[u8]) -> Result<String> {
    let (text, _, errors) = encoding_rs::SHIFT_JIS.decode(bytes);
    if errors {
        return Err(error("invalid CP932 resource path"));
    }
    Ok(text.into_owned())
}
fn read_resource(resources: &ResourceIndex, name: &[u8]) -> Result<Resource> {
    if name.len() > 1023 {
        return Err(error("effect path exceeds original wide buffer bound"));
    }
    let name = cp932(name)?;
    let bytes = resources.read_category(b"efk", &name).map_err(error)?;
    let effect = Effect::parse(&bytes).map_err(error)?;
    let mut textures = Vec::with_capacity(effect.textures.len());
    let mut total = 0usize;
    for path in &effect.textures {
        let (encoded, _, errors) = encoding_rs::SHIFT_JIS.encode(path);
        if errors || encoded.len() > 1023 {
            return Err(error("texture path CP932 conversion/buffer bound"));
        }
        // PE RVA2005c0 scans raw bytes for '/' or '\\', and strips after the
        // final separator when its index is nonzero. Preserve that behavior.
        let start = encoded
            .iter()
            .rposition(|b| matches!(*b, b'/' | b'\\'))
            .filter(|p| *p != 0)
            .map_or(0, |p| p + 1);
        let basename = cp932(&encoded[start..])?;
        let data = resources
            .read_category(b"efktex", &basename)
            .map_err(error)?;
        let image = image::ImageReader::new(std::io::Cursor::new(&data))
            .with_guessed_format()
            .map_err(error)?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(64 * 1024 * 1024);
        let mut image = image;
        image.limits(limits);
        let rgba = image.decode().map_err(error)?.to_rgba8();
        total = total
            .checked_add(rgba.len())
            .ok_or_else(|| error(Error::Capacity))?;
        if total > 256 * 1024 * 1024 {
            return Err(error(Error::Capacity));
        }
        textures.push(Texture::new(rgba.width(), rgba.height(), rgba.into_raw()).map_err(error)?);
    }
    Resource::new(effect, textures).map_err(error)
}

impl LoggingHost<'_> {
    pub(super) fn receive_effekseer_transform(
        &mut self,
        command: TaskCommand,
        data1: Option<&[u8]>,
        output: &mut u32,
        budget: usize,
    ) -> Result<Option<TaskCommandOutcome>> {
        if !matches!(command.code, 51 | 55 | 59 | 66) {
            return Ok(None);
        }
        let mut buffers = OwnedTaskBuffers::new(None, data1.map(<[u8]>::to_vec))?;
        receive(self, command, &mut buffers, output, budget)
    }
    /// Fixed-profile ordinary93 -> command55, including its real three-f32
    /// stack buffer. Other task kinds retain their own wrapper consumers.
    pub(super) fn effekseer_rotation_native(
        &mut self,
        index: u32,
        args: &[u8],
    ) -> Result<Option<NativeReturn>> {
        if index != 93 {
            return Ok(None);
        }
        let id = read_u32(args, 0)?;
        if !self
            .scene_nodes
            .get(&id)
            .is_some_and(|node| node.kind == 61)
        {
            return Ok(None);
        }
        if !identified(self.data_services.native_profile) {
            return Err(error(
                "type61 rotation wrapper requires the recovered PE fingerprint",
            ));
        }
        let vector = args
            .get(4..16)
            .ok_or_else(|| error("native93 needs three f32 radians"))?;
        let command = TaskCommand {
            target: id as i32,
            sender: self.current_task_id as i32,
            code: 55,
            data: [read_u32(args, 16)?, 1, 0],
        };
        let mut buffers = OwnedTaskBuffers::new(None, Some(vector.to_vec()))?;
        if self
            .route_task_command_buffers(command, &mut buffers, &mut 0, 4096)?
            .is_some_and(|outcome| !outcome.done)
        {
            return Err(error("leaf Effekseer rotation unexpectedly suspended"));
        }
        Ok(Some(NativeReturn::Continue))
    }
    pub(super) fn effekseer_register_task_hook(&mut self, id: u32) -> Result<()> {
        if !identified(self.data_services.native_profile) {
            return Err(error(
                "type61 constructor requires recovered PE fingerprint",
            ));
        }
        self.data_services
            .effekseer
            .manager
            .register(id)
            .map_err(error)
    }
    pub(super) fn effekseer_update_task_hook(&mut self, id: u32) -> Result<()> {
        if let Some(node) = self.scene_nodes.get(&id) {
            self.data_services.effekseer.sync(node).map_err(error)?;
        }
        self.data_services
            .effekseer
            .manager
            .update_task(id)
            .map_err(error)
    }
    pub(super) fn effekseer_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let &(_, code, size) = NATIVE_COMMANDS
            .iter()
            .find(|d| d.0 == index)
            .ok_or_else(|| error("unknown descriptor"))?;
        if args.len() < size {
            return Err(error("missing native arguments"));
        }
        if !identified(self.data_services.native_profile) {
            return Err(error("ordinary Effekseer ABI has no matching fingerprint"));
        }
        let target = read_u32(args, 0)?;
        // Unlike M2, these three wrappers skip the zero target directly.
        if target == 0 {
            return Ok(NativeReturn::Continue);
        }
        let mut data1 = 0;
        let owned = if index == 1014 {
            let text = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
            let mut raw = if self.text_encoding == super::ScriptTextEncoding::Utf8 {
                let utf8 = std::str::from_utf8(&text).map_err(error)?;
                let (raw, _, errors) = encoding_rs::SHIFT_JIS.encode(utf8);
                if errors {
                    return Err(error("formatted path is not CP932 representable"));
                }
                raw.into_owned()
            } else {
                text
            };
            raw.push(0);
            data1 = 1;
            Some(raw)
        } else {
            if index == 1016 {
                data1 = read_u32(args, 4)?;
            }
            None
        };
        let command = TaskCommand {
            target: target as i32,
            sender: self.current_task_id as i32,
            code,
            data: [0, data1, 0],
        };
        let mut buffers = OwnedTaskBuffers::new(None, owned)?;
        let result = self.route_task_command_buffers(command, &mut buffers, &mut 0, 4096)?;
        if result.is_some_and(|o| !o.done) {
            return Err(error("Effekseer command unexpectedly suspended"));
        }
        Ok(NativeReturn::Continue)
    }
    pub(super) fn dispatch_effekseer_message(
        &mut self,
        id: u32,
        packet: [u32; 4],
        queued: bool,
        output: &mut u32,
        vm: Option<&mut Vm>,
    ) -> Result<Option<TaskCommandOutcome>> {
        if !matches!(packet[1], 51 | 55 | 59 | 66 | 578..=582) {
            return Ok(None);
        }
        let command = TaskCommand {
            target: id as i32,
            sender: packet[0] as i32,
            code: packet[1],
            data: [packet[2], packet[3], 0],
        };
        let mut buffers = if !queued {
            self.pending_owned_task_buffers(command)
        } else {
            None
        };
        if buffers.is_none() {
            let path = if command.code == 578 {
                let vm = vm.ok_or_else(|| error("queued load has no owned resource path"))?;
                let mut bytes = vm.read_string(packet[3])?;
                bytes.push(0);
                Some(bytes)
            } else if matches!(command.code, 51 | 66)
                || (command.code == 55
                    && ((queued && packet[3] != 0)
                        || self
                            .data_services
                            .effekseer
                            .manager
                            .rotation_reads_data(id)
                            .map_err(error)?))
            {
                let vm =
                    vm.ok_or_else(|| error("transform message has no owned three-float data1"))?;
                Some(vm.read(packet[3], 12)?.to_vec())
            } else {
                None
            };
            buffers = Some(OwnedTaskBuffers::new(None, path)?);
        }
        if queued {
            // Raw70 owns a copy now; the task body consumes it through
            // update_task_command_buffers. Do not execute resource IO or
            // mutate the manager while merely accepting a queued packet.
            let found =
                self.queue_task_command_buffers(command, buffers.as_ref().unwrap(), output)?;
            Ok(found.then_some(TaskCommandOutcome {
                done: true,
                propagate: false,
            }))
        } else {
            self.route_task_command_buffers(command, buffers.as_mut().unwrap(), output, 4096)
        }
    }
}

fn edge(a: &Vertex, b: &Vertex, p: [f32; 2]) -> f32 {
    (b.position[0] - a.position[0]) * (p[1] - a.position[1])
        - (b.position[1] - a.position[1]) * (p[0] - a.position[0])
}
fn top_left(a: &Vertex, b: &Vertex) -> bool {
    let dy = b.position[1] - a.position[1];
    dy < 0.0 || (dy == 0.0 && b.position[0] > a.position[0])
}
fn texel(texture: &Texture, x: i32, y: i32, wrap: i32) -> [f32; 4] {
    let address = |v: i32, n: u32| {
        if wrap == 0 {
            v.rem_euclid(n as i32) as usize
        } else {
            v.clamp(0, n as i32 - 1) as usize
        }
    };
    let at = (address(y, texture.height) * texture.width as usize + address(x, texture.width)) * 4;
    std::array::from_fn(|c| texture.rgba[at + c] as f32)
}
fn sample(texture: Option<&Texture>, uv: [f32; 2], filter: i32, wrap: i32) -> [f32; 4] {
    let Some(t) = texture else {
        return [255.0; 4];
    };
    if filter == 0 {
        return texel(
            t,
            (uv[0] * t.width as f32).floor() as i32,
            (uv[1] * t.height as f32).floor() as i32,
            wrap,
        );
    }
    let x = uv[0] * t.width as f32 - 0.5;
    let y = uv[1] * t.height as f32 - 0.5;
    let fx = x - x.floor();
    let fy = y - y.floor();
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let a = texel(t, ix, iy, wrap);
    let b = texel(t, ix + 1, iy, wrap);
    let c = texel(t, ix, iy + 1, wrap);
    let d = texel(t, ix + 1, iy + 1, wrap);
    std::array::from_fn(|i| {
        (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy
    })
}

fn raster(canvas: &mut Canvas, quads: &[Quad]) -> std::result::Result<u64, Error> {
    let w = canvas.width() as usize;
    let h = canvas.height() as usize;
    let count = w
        .checked_mul(h)
        .filter(|n| *n <= 16 * 1024 * 1024)
        .ok_or(Error::Capacity)?;
    let mut depth = vec![1.0f32; count];
    let mut written = 0u64;
    for q in quads {
        if q.vertices
            .iter()
            .any(|v| !v.position.iter().chain(&v.uv).all(|f| f.is_finite()))
        {
            return Err(Error::Host("nonfinite projected sprite"));
        }
        let texture = if q.texture < 0 {
            None
        } else {
            Some(
                q.resource
                    .textures
                    .get(q.texture as usize)
                    .ok_or(Error::Host("missing sprite texture"))?,
            )
        };
        for indices in [[0, 1, 2], [2, 1, 3]] {
            let mut v = indices.map(|i| q.vertices[i]);
            let mut area = edge(&v[0], &v[1], [v[2].position[0], v[2].position[1]]);
            if area == 0.0 {
                continue;
            }
            if area < 0.0 {
                v.swap(1, 2);
                area = -area;
            }
            let minx = v
                .iter()
                .map(|v| v.position[0])
                .fold(f32::INFINITY, f32::min)
                .ceil()
                .max(0.0)
                .min(w as f32) as usize;
            let miny = v
                .iter()
                .map(|v| v.position[1])
                .fold(f32::INFINITY, f32::min)
                .ceil()
                .max(0.0)
                .min(h as f32) as usize;
            let maxx = v
                .iter()
                .map(|v| v.position[0])
                .fold(f32::NEG_INFINITY, f32::max)
                .floor()
                .max(0.0)
                .min(w.saturating_sub(1) as f32) as usize;
            let maxy = v
                .iter()
                .map(|v| v.position[1])
                .fold(f32::NEG_INFINITY, f32::max)
                .floor()
                .max(0.0)
                .min(h.saturating_sub(1) as f32) as usize;
            if minx > maxx || miny > maxy {
                continue;
            }
            for y in miny..=maxy {
                for x in minx..=maxx {
                    // D3D9 viewport pixel centers are integer coordinates.
                    let p = [x as f32, y as f32];
                    let e = [
                        edge(&v[1], &v[2], p),
                        edge(&v[2], &v[0], p),
                        edge(&v[0], &v[1], p),
                    ];
                    let tl = [
                        top_left(&v[1], &v[2]),
                        top_left(&v[2], &v[0]),
                        top_left(&v[0], &v[1]),
                    ];
                    if (0..3).any(|i| e[i] < 0.0 || (e[i] == 0.0 && !tl[i])) {
                        continue;
                    }
                    let b = e.map(|x| x / area);
                    let z = (0..3).map(|i| v[i].position[2] * b[i]).sum::<f32>();
                    let pixel = y * w + x;
                    if !(0.0..=1.0).contains(&z) || (q.ztest && z > depth[pixel]) {
                        continue;
                    }
                    let uv = std::array::from_fn(|c| (0..3).map(|i| v[i].uv[c] * b[i]).sum());
                    let tex = sample(texture, uv, q.filter, q.wrap);
                    let source = std::array::from_fn::<_, 4, _>(|c| {
                        tex[c] * (0..3).map(|i| v[i].color[c] as f32 * b[i]).sum::<f32>() / 255.0
                    });
                    let at = pixel * 4;
                    let dest = &mut canvas.pixels_mut()[at..at + 4];
                    blend(dest, source, q.blend)?;
                    if q.zwrite {
                        depth[pixel] = z;
                    }
                    written += 1;
                }
            }
        }
    }
    Ok(written)
}
fn blend(dest: &mut [u8], s: [f32; 4], mode: i32) -> std::result::Result<(), Error> {
    let a = s[3] / 255.0;
    for c in 0..4 {
        let d = dest[c] as f32;
        let value = match mode {
            0 => s[c],
            1 => s[c] * a + d * (1.0 - a),
            2 => s[c] * a + d,
            // Pinned DX9 RenderState: REVSUBTRACT, SRCALPHA/ONE.
            3 => d - s[c] * a,
            // Pinned DX9 RenderState: ADD, ZERO/SRCCOLOR.
            4 => d * s[c] / 255.0,
            _ => return Err(Error::Host("unknown Effekseer blend state")),
        };
        dest[c] = value.clamp(0.0, 255.0).round() as u8;
    }
    Ok(())
}
