//! Ordinary KX resource receivers and CPU mesh submission.
//! File geometry and the live plane properties have separate ownership.

use std::collections::BTreeMap;

use cs2_renderer::{BlendMode, Canvas, Color, Rect, RgbaFrame};
use cs2_runtime::message_object::kx::KxMesh;
use cs2_script::{NativeReturn, Result, ScriptError, Vm};

use crate::{HeadlessPlayer, LoggingHost, SceneNode, expand_kcs_format};

const IDENTITY: [f32; 16] = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];

fn invalid(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct KxGeometry {
    pub(crate) name: Vec<u8>,
    pub(crate) mesh: KxMesh,
    pub(crate) canvas_wrapped: bool,
}

/// Snapshot supplied by the plane service, after parent/state processing.
#[derive(Clone, Debug)]
pub(crate) struct KxPlaneView {
    pub(crate) words: [u32; 36],
    pub(crate) source_words: [u32; 36],
    pub(crate) blend: u32,
    pub(crate) clip: Option<Rect>,
    pub(crate) camera: u32,
    pub(crate) perspective_origin: u32,
    pub(crate) color: Option<u32>,
    pub(crate) wrap_transform: [f32; 16],
    pub(crate) source_transform: [f32; 16],
    pub(crate) raster_wrap: Option<crate::plane_native::RasterVertexTransform>,
    pub(crate) raster_source: Option<crate::plane_native::RasterVertexTransform>,
}

impl KxPlaneView {
    pub(crate) fn new(words: [u32; 36]) -> Self {
        let mut source_words = [0; 36];
        for (i, value) in IDENTITY.into_iter().enumerate() {
            source_words[9 + i] = value.to_bits();
        }
        source_words[25..28].fill(1.0f32.to_bits());
        source_words[35] = 1;
        let wrap_transform = matrix(&words);
        Self {
            words,
            source_words,
            blend: 0,
            clip: None,
            camera: 0,
            perspective_origin: 0,
            color: None,
            wrap_transform,
            source_transform: IDENTITY,
            raster_wrap: None,
            raster_source: None,
        }
    }
}

pub(crate) type KxPlaneViewHook =
    fn(&SceneNode, &crate::plane_native::PlaneServices) -> Result<KxPlaneView>;

pub(crate) type KxPlaneInstallHook =
    fn(&mut crate::plane_native::PlaneServices, &mut SceneNode, bool, [f32; 4]) -> Result<()>;

#[derive(Clone, Debug, Default)]
pub(crate) struct KxServices {
    pub(crate) objects: BTreeMap<u32, KxGeometry>,
    pub(crate) names: BTreeMap<u32, Vec<u8>>,
    metadata: BTreeMap<u32, ([u32; 4], u32)>,
    pub(crate) view_hook: Option<KxPlaneViewHook>,
    pub(crate) install_hook: Option<KxPlaneInstallHook>,
    previous_receiver: Option<crate::native_tasks::TaskBufferReceiver>,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct KxGraphSnapshot {
    pub(crate) objects: BTreeMap<u32, KxGeometry>,
    pub(crate) names: BTreeMap<u32, Vec<u8>>,
    pub(crate) metadata: BTreeMap<u32, ([u32; 4], u32)>,
}

impl KxServices {
    pub(crate) fn graph_snapshot(&self) -> KxGraphSnapshot {
        KxGraphSnapshot { objects: self.objects.clone(), names: self.names.clone(),
            metadata: self.metadata.clone() }
    }

    /// Keep the session's real receiver and typed installation hooks. The
    /// serialized graph owns data, never code pointers or resource reloads.
    pub(crate) fn install_graph(&mut self, saved: KxGraphSnapshot) {
        self.objects = saved.objects;
        self.names = saved.names;
        self.metadata = saved.metadata;
    }

    pub(crate) fn release_metadata(&mut self, id: u32) {
        self.metadata.remove(&id);
    }
}

fn kx_receiver(
    host: &mut LoggingHost<'_>,
    command: cs2_runtime::TaskCommand,
    buffers: &mut crate::native_tasks::OwnedTaskBuffers,
    output: &mut u32,
    budget: usize,
) -> Result<Option<cs2_runtime::TaskCommandOutcome>> {
    if let Some(outcome) = host.receive_kx_command(command, buffers, output)? {
        return Ok(Some(outcome));
    }
    if let Some(previous) = host.data_services.messages.kx.previous_receiver {
        previous(host, command, buffers, output, budget)
    } else {
        host.receive_input_button_command(command, buffers, output, budget)
    }
}

impl LoggingHost<'_> {
    pub(crate) fn install_kx_receiver(&mut self) {
        if let Some(previous) = self
            .data_services
            .tasks
            .install_buffer_receiver(2, kx_receiver)
        {
            if !std::ptr::fn_addr_eq(
                previous,
                kx_receiver as crate::native_tasks::TaskBufferReceiver,
            ) {
                self.data_services.messages.kx.previous_receiver = Some(previous);
            }
        }
    }

    pub(crate) fn kx_raw_native(
        &mut self,
        args: &[u8],
        vm: &mut Vm,
        queued: bool,
    ) -> Result<Option<NativeReturn>> {
        let id = crate::read_u32(args, 0)?;
        let code = crate::read_u32(args, 4)?;
        if !matches!(code, 69..=71) || !self.scene_nodes.get(&id).is_some_and(|node| node.kind == 2)
        {
            return Ok(None);
        }
        if self
            .data_services
            .tasks
            .message_task_state(id)
            .is_some_and(|(_, deletion, _)| deletion != 0)
        {
            return Ok(Some(NativeReturn::Value(0)));
        }
        let command = cs2_runtime::TaskCommand {
            target: id as i32,
            sender: self.current_task_id as i32,
            code,
            data: [crate::read_u32(args, 8)?, 1, 0],
        };
        let mut name = if code == 71 {
            Vec::new()
        } else {
            vm.read_string(crate::read_u32(args, 12)?)?
        };
        if code != 71 && name.last() != Some(&0) {
            name.push(0);
        }
        let mut buffers = crate::native_tasks::OwnedTaskBuffers::new(None, Some(name))?;
        let mut output = 0;
        self.install_kx_receiver();
        if queued {
            // The installed concrete receiver delegates these codes back to
            // receive_kx_command when its owned task queue is advanced.
            self.queue_task_command_buffers(command, &buffers, &mut output)?;
            return Ok(Some(NativeReturn::Value(output)));
        }
        self.receive_kx_command(command, &mut buffers, &mut output)?;
        Ok(Some(NativeReturn::Value(output)))
    }

    pub(crate) fn kx_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let target = crate::read_u32(args, 0)?;
        if target == 0 {
            return Ok(NativeReturn::Continue);
        }
        let bytes = if index == 346 { 12 } else { 8 };
        if args.len() < bytes || args.len() % 4 != 0 {
            return Err(invalid("KX native is missing its fixed DWORD prefix"));
        }
        let raw = vm.read_string(crate::read_u32(args, 4)?)?;
        let expanded = expand_kcs_format(vm, &raw, 0)?;
        let expanded = &expanded[..expanded
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(expanded.len())];
        let (decoded, _, malformed) = encoding_rs::SHIFT_JIS.decode(expanded);
        if malformed {
            return Err(invalid("KX resource name is not valid CP932"));
        }
        let category: &[u8] = if index == 346 { b"kx3" } else { b"kx2" };
        let names = self
            .resources
            .category_names(category, &decoded)
            .map_err(|_| invalid("KX resource category completion failed"))?;
        let name = names
            .iter()
            .find(|name| self.resources.find(name).is_some())
            .or_else(|| names.first())
            .ok_or_else(|| invalid("KX resource category has no completion"))?;
        let (name, _, malformed) = encoding_rs::SHIFT_JIS.encode(name);
        if malformed || name.len() >= 260 {
            return Err(invalid(
                "KX completed name exceeds its CP932 filename buffer",
            ));
        }
        let mut terminated = name.into_owned();
        terminated.push(0);
        let command = cs2_runtime::TaskCommand {
            target: target as i32,
            sender: self.current_task_id as i32,
            code: match index {
                81 => 69,
                82 => 70,
                346 => 71,
                _ => return Err(ScriptError::UnimplementedNativeCall { index }),
            },
            data: [
                if index == 346 {
                    crate::read_u32(args, 8)?
                } else {
                    0
                },
                1,
                0,
            ],
        };
        let mut buffers = crate::native_tasks::OwnedTaskBuffers::new(None, Some(terminated))?;
        let mut output = 0;
        self.install_kx_receiver();
        let outcome = self.receive_kx_command(command, &mut buffers, &mut output)?;
        if outcome.is_none() && self.task_is_reachable(target) {
            return Err(invalid("KX target has no recovered geometry receiver"));
        }
        Ok(NativeReturn::Continue)
    }

    /// Called by the concrete kind-two owned receiver before its plane base.
    pub(crate) fn receive_kx_command(
        &mut self,
        command: cs2_runtime::TaskCommand,
        buffers: &mut crate::native_tasks::OwnedTaskBuffers,
        _output: &mut u32,
    ) -> Result<Option<cs2_runtime::TaskCommandOutcome>> {
        if !matches!(command.code, 69..=71) {
            return Ok(None);
        }
        let id = command.target as u32;
        if !self.task_is_reachable(id) {
            return Ok(None);
        }
        self.ensure_task_record(id)?;
        let Some((kind, deletion, _)) = self.data_services.tasks.message_task_state(id) else {
            return Ok(None);
        };
        if kind != 2 || self.scene_nodes.get(&id).is_none_or(|node| node.kind != 2) {
            return Ok(None);
        }
        let done = cs2_runtime::TaskCommandOutcome {
            done: true,
            propagate: false,
        };
        if deletion != 0 || command.code == 71 {
            // The recovered SimplePlane handler explicitly consumes 71.
            return Ok(Some(done));
        }
        let name = buffers
            .data1()
            .ok_or_else(|| invalid("KX receiver has no owned name buffer"))?;
        let end = name
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| invalid("KX receiver name is not terminated"))?;
        if end >= 260 {
            return Err(invalid("KX receiver name exceeds 259 bytes"));
        }
        let name = name[..end].to_vec();
        let create = self
            .data_services
            .planes
            .state(id)
            .map_or(1, |state| state.create);
        self.data_services.messages.kx.objects.remove(&id);
        if let Some(node) = self.scene_nodes.get_mut(&id) {
            node.initialized = false;
        }
        if create != 1 {
            return Ok(Some(done));
        }
        self.ensure_canvas_state().map_err(|_| {
            invalid("KX geometry cannot initialize the current drawing configuration")
        })?;
        if self.data_services.canvas_native.mode != 0
            || !self.data_services.canvas_native.enabled
            || self.canvas.is_none()
        {
            return Ok(Some(done));
        }
        let (decoded, _, malformed) = encoding_rs::SHIFT_JIS.decode(&name);
        if malformed {
            return Err(invalid("KX receiver name is not valid CP932"));
        }
        let bytes = match self.resources.read(decoded.as_ref()) {
            Ok(bytes) => bytes,
            Err(cs2_runtime::RuntimeError::ResourceNotFound(_)) => return Ok(Some(done)),
            Err(_) => return Err(invalid("KX resource read failed")),
        };
        if bytes.get(..4) != Some(b"KX2\0") {
            return Ok(Some(done));
        }
        let mesh = KxMesh::decode(&bytes)?;
        let total = self
            .data_services
            .messages
            .kx
            .objects
            .values()
            .try_fold(mesh.triangles.len() * 48, |sum, object| {
                sum.checked_add(object.mesh.triangles.len() * 48)
            })
            .filter(|&total| total <= 256 * 1024 * 1024);
        if total.is_none() {
            return Err(invalid("live KX mesh storage exceeds 256 MiB"));
        }
        let extent = mesh.extent();
        let install = self.data_services.messages.kx.install_hook;
        if command.code == 69 && install.is_none() {
            return Err(invalid(
                "KX canvas geometry needs its typed plane installation adapter",
            ));
        }
        let node = self
            .scene_nodes
            .get_mut(&id)
            .ok_or_else(|| invalid("KX task has no scene geometry node"))?;
        node.position = [0.; 3];
        node.scale = [1.; 3];
        node.color = u32::MAX;
        node.overlay_color = 0;
        node.visible = false;
        node.initialized = true;
        node.size = extent;
        if let Some(install) = install {
            install(
                &mut self.data_services.planes,
                node,
                command.code == 69,
                mesh.rectangle(),
            )?;
        } else {
            let initialize: Vec<_> = [id, extent[0].to_bits(), extent[1].to_bits(), u32::MAX]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect();
            self.data_services
                .planes
                .observe_existing(79, &initialize, self.scene_nodes)?;
        }
        self.data_services
            .messages
            .kx
            .names
            .insert(id, name.clone());
        self.data_services
            .messages
            .kx
            .metadata
            .insert(id, (mesh.bounds, if command.code == 69 { 6 } else { 7 }));
        self.data_services.messages.kx.objects.insert(
            id,
            KxGeometry {
                name,
                mesh,
                canvas_wrapped: command.code == 69,
            },
        );
        Ok(Some(done))
    }

    /// Last successful file metadata survives a failed reload. Plane 75/76
    /// can use this without accessing the source's owned vertex allocation.
    pub(crate) fn kx_geometry_metadata(&self, id: u32) -> Option<([f32; 4], u32)> {
        self.data_services
            .messages
            .kx
            .metadata
            .get(&id)
            .map(|(bounds, kind)| (bounds.map(f32::from_bits), *kind))
    }

    pub(crate) fn kx_geometry_name(&self, id: u32) -> Option<&[u8]> {
        self.data_services
            .messages
            .kx
            .names
            .get(&id)
            .map(Vec::as_slice)
    }

    /// Offscreen captures use the same geometry and shader path as the main
    /// scene. The caller supplies its current work surface and source snapshot.
    pub(crate) fn draw_kx_geometry_into(
        &self,
        output: &mut Canvas,
        node: &SceneNode,
        source: &RgbaFrame,
        cameras: &BTreeMap<u32, crate::movie_plane::PlaneCamera>,
        projection: &crate::movie_plane_render::MoviePlaneProjection,
    ) -> anyhow::Result<bool> {
        draw_geometry_into(
            output,
            node,
            source,
            &self.data_services.messages.kx,
            &self.data_services.planes,
            &self.data_services.tasks,
            &self.data_services.canvas_native,
            cameras,
            projection,
            false,
        )
    }
}

impl HeadlessPlayer {
    pub(crate) fn set_kx_plane_view_hook(&mut self, hook: Option<KxPlaneViewHook>) {
        self.data_services.messages.kx.view_hook = hook;
    }

    pub(crate) fn set_kx_plane_install_hook(&mut self, hook: Option<KxPlaneInstallHook>) {
        self.data_services.messages.kx.install_hook = hook;
    }

    pub(crate) fn clear_kx_geometry(&mut self, id: u32) {
        self.data_services.messages.kx.objects.remove(&id);
        self.data_services.messages.kx.metadata.remove(&id);
    }

    pub(crate) fn kx_geometry(&self, id: u32) -> Option<&KxGeometry> {
        self.data_services.messages.kx.objects.get(&id)
    }

    /// Call in the normal sorted scene pass, before the ordinary rectangle.
    pub(crate) fn draw_kx_geometry(
        &mut self,
        id: u32,
        canvas_source: &RgbaFrame,
    ) -> anyhow::Result<bool> {
        let Some(node) = self.scene_nodes.get(&id) else {
            return Ok(false);
        };
        draw_geometry_into(
            &mut self.canvas,
            node,
            canvas_source,
            &self.data_services.messages.kx,
            &self.data_services.planes,
            &self.data_services.tasks,
            &self.data_services.canvas_native,
            &self.movie_host.planes.cameras,
            &self.movie_host.planes.projection,
            true,
        )
    }
}

fn draw_geometry_into(
    output: &mut Canvas,
    node: &SceneNode,
    source: &RgbaFrame,
    kx: &KxServices,
    planes: &crate::plane_native::PlaneServices,
    tasks: &crate::native_tasks::NativeTaskStore,
    canvas: &crate::canvas_native::CanvasNativeState,
    cameras: &BTreeMap<u32, crate::movie_plane::PlaneCamera>,
    projection: &crate::movie_plane_render::MoviePlaneProjection,
    require_active: bool,
) -> anyhow::Result<bool> {
    let Some(object) = kx.objects.get(&node.id) else {
        return Ok(false);
    };
    if !node.initialized
        || !node.visible
        || tasks
            .message_task_state(planes.history_owner(node.id).unwrap_or(node.id))
            .is_none_or(|(kind, deletion, _)| kind != 2 || deletion != 0)
        || canvas.mode != 0
        || !canvas.enabled
        || (require_active && !canvas.active)
    {
        return Ok(true);
    }
    let view = kx
        .view_hook
        .ok_or_else(|| invalid("KX geometry needs the live plane property snapshot"))?(
        node, planes,
    )?;
    draw_mesh(
        output,
        &object.mesh,
        object.canvas_wrapped.then_some(source),
        &view,
        cameras,
        projection,
    )?;
    Ok(true)
}

fn transform(point: [f32; 4], matrix: [f32; 16]) -> [f32; 4] {
    std::array::from_fn(|column| {
        let first = point[0] * matrix[column];
        let second = point[1] * matrix[4 + column];
        let third = point[2] * matrix[8 + column];
        let fourth = point[3] * matrix[12 + column];
        ((first + second) + third) + fourth
    })
}

fn matrix(words: &[u32; 36]) -> [f32; 16] {
    let explicit: [f32; 16] = std::array::from_fn(|i| f32::from_bits(words[9 + i]));
    let scales: [f32; 3] = std::array::from_fn(|i| f32::from_bits(words[25 + i]));
    std::array::from_fn(|i| explicit[i] * if i / 4 < 3 { scales[i / 4] } else { 1. })
}

fn project(
    mut point: [f32; 4],
    view: &KxPlaneView,
    camera: &crate::movie_plane::PlaneCamera,
    projection: &crate::movie_plane_render::MoviePlaneProjection,
) -> [f64; 4] {
    let position: [f32; 3] = std::array::from_fn(|i| f32::from_bits(view.words[6 + i]));
    let base_z = f32::from_bits(view.words[5]);
    let perspective = projection.perspective && point[2] != 0.;
    if perspective {
        let distance = (65000. - base_z) - projection.camera_depth;
        let denominator = if point[2] == distance {
            1.
        } else {
            point[2] - distance
        };
        let ratio = (projection.camera_depth / denominator).max(0.);
        let origin = match view.perspective_origin {
            0 => [position[0], position[1]],
            1 => projection.center,
            _ => [0.; 2],
        };
        point = [
            (point[0] - origin[0]) * ratio + origin[0],
            (point[1] - origin[1]) * ratio + origin[1],
            point[2] * (1. / 65000.),
            1. / denominator,
        ];
    } else {
        point[2] = (65000. - (point[2] + base_z)) * (1. / 65000.);
    }
    let depth = point[2];
    point[0] = (point[0] - projection.center[0]) * camera.scale[0];
    point[1] = (point[1] - projection.center[1]) * camera.scale[1];
    point = transform(point, camera.matrix);
    point[0] += projection.center[0] - camera.offset[0];
    point[1] += projection.center[1] - camera.offset[1];
    if !perspective {
        point[2] = depth;
    }
    point = transform(point, projection.matrix);
    point[0] += projection.translation[0];
    point[1] += projection.translation[1];
    point.map(f64::from)
}

/// Screen x/y/z, reciprocal W, RGBA, UV multiplied by reciprocal W.
type Vertex = [f64; 10];

fn draw_mesh(
    canvas: &mut Canvas,
    mesh: &KxMesh,
    texture: Option<&RgbaFrame>,
    view: &KxPlaneView,
    cameras: &BTreeMap<u32, crate::movie_plane::PlaneCamera>,
    projection: &crate::movie_plane_render::MoviePlaneProjection,
) -> anyhow::Result<()> {
    if view.words[35] == 0 {
        return Ok(());
    }
    let default_camera = crate::movie_plane::PlaneCamera::default();
    let camera = cameras
        .get(&view.camera)
        .filter(|_| view.camera != 0)
        .unwrap_or(&default_camera);
    let wrap = view.wrap_transform;
    let source = view.source_transform;
    let limit = if texture.is_some() {
        mesh.triangles.len()
    } else {
        32
    };
    for triangle in mesh.triangles.iter().take(limit) {
        let mut polygon = Vec::with_capacity(3);
        for vertex in triangle {
            let mut xyz = vertex.xyz();
            let mut uv = [0.; 2];
            if let Some(texture) = texture {
                let mut local = [
                        xyz[0] - f32::from_bits(view.source_words[3]),
                        xyz[1] - f32::from_bits(view.source_words[4]),
                        xyz[2],
                        1.,
                    ];
                if let Some(callback) = view.raster_source { callback.apply(&mut local)?; }
                let mut point = transform(local, source);
                for axis in 0..3 {
                    point[axis] += f32::from_bits(view.source_words[6 + axis]);
                }
                uv = [
                    point[0] - texture.metadata().offset[0] as f32,
                    point[1] - texture.metadata().offset[1] as f32,
                ];
                xyz = [
                    point[0] - f32::from_bits(view.source_words[6]),
                    point[1] - f32::from_bits(view.source_words[7]),
                    point[2],
                ];
            }
            let mut local = [
                    xyz[0] - f32::from_bits(view.words[3]),
                    xyz[1] - f32::from_bits(view.words[4]),
                    xyz[2],
                    1.,
                ];
            if let Some(callback) = view.raster_wrap { callback.apply(&mut local)?; }
            let mut point = transform(local, wrap);
            for axis in 0..3 {
                point[axis] += f32::from_bits(view.words[6 + axis]);
            }
            point[3] += 1.;
            let screen = project(
                transform(point, projection.parent),
                view,
                camera,
                projection,
            );
            let color = view.color.unwrap_or(vertex.argb);
            polygon.push([
                screen[0],
                screen[1],
                screen[2],
                screen[3],
                f64::from((color >> 16) as u8),
                f64::from((color >> 8) as u8),
                f64::from(color as u8),
                f64::from((color >> 24) as u8),
                f64::from(uv[0]) * screen[3],
                f64::from(uv[1]) * screen[3],
            ]);
        }
        if polygon.iter().flatten().any(|n| !n.is_finite()) {
            anyhow::bail!("KX projected triangle contains non-finite values");
        }
        for (edge, lower) in [(0., true), (1., false)] {
            polygon = clip_depth(&polygon, edge, lower);
        }
        for index in 1..polygon.len().saturating_sub(1) {
            raster(
                canvas,
                [polygon[0], polygon[index], polygon[index + 1]],
                texture,
                view,
            )?;
        }
    }
    Ok(())
}

fn clip_depth(input: &[Vertex], edge: f64, lower: bool) -> Vec<Vertex> {
    let mut output = Vec::new();
    let Some(mut previous) = input.last().copied() else {
        return output;
    };
    let inside = |vertex: Vertex| {
        if lower {
            vertex[2] >= edge
        } else {
            vertex[2] <= edge
        }
    };
    for &current in input {
        if inside(previous) != inside(current) {
            let amount = (edge - previous[2]) / (current[2] - previous[2]);
            output.push(std::array::from_fn(|axis| {
                previous[axis] + (current[axis] - previous[axis]) * amount
            }));
        }
        if inside(current) {
            output.push(current);
        }
        previous = current;
    }
    output
}

fn edge(a: Vertex, b: Vertex, x: f64, y: f64) -> f64 {
    (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])
}

fn raster(
    canvas: &mut Canvas,
    mut vertices: [Vertex; 3],
    texture: Option<&RgbaFrame>,
    view: &KxPlaneView,
) -> anyhow::Result<()> {
    for vertex in &mut vertices {
        vertex[0] = (vertex[0] * 256. + 0.5).floor() / 256.;
        vertex[1] = (vertex[1] * 256. + 0.5).floor() / 256.;
        if vertex[3] <= 0. {
            anyhow::bail!("KX visible vertex has non-positive reciprocal W");
        }
    }
    let mut area = edge(vertices[0], vertices[1], vertices[2][0], vertices[2][1]);
    if area == 0. {
        return Ok(());
    }
    if area < 0. {
        vertices.swap(1, 2);
        area = -area;
    }
    let bounds = |axis: usize, limit: u32| {
        let lower = vertices
            .iter()
            .map(|v| v[axis])
            .fold(f64::INFINITY, f64::min)
            .ceil()
            .max(0.)
            .min(f64::from(limit)) as i32;
        let upper = vertices
            .iter()
            .map(|v| v[axis])
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .max(0.)
            .min(f64::from(limit)) as i32;
        (lower, upper)
    };
    let (left, right) = bounds(0, canvas.width());
    let (top, bottom) = bounds(1, canvas.height());
    let inclusive = std::array::from_fn::<_, 3, _>(|i| {
        let a = vertices[(i + 1) % 3];
        let b = vertices[(i + 2) % 3];
        b[1] < a[1] || (b[1] == a[1] && b[0] > a[0])
    });
    for y in top..bottom {
        for x in left..right {
            if view.clip.is_some_and(|clip| {
                x < clip.x
                    || y < clip.y
                    || i64::from(x) >= i64::from(clip.x) + i64::from(clip.width)
                    || i64::from(y) >= i64::from(clip.y) + i64::from(clip.height)
            }) {
                continue;
            }
            let weights = [
                edge(vertices[1], vertices[2], x as f64, y as f64),
                edge(vertices[2], vertices[0], x as f64, y as f64),
                edge(vertices[0], vertices[1], x as f64, y as f64),
            ];
            if (0..3).any(|i| weights[i] < 0. || (weights[i] == 0. && !inclusive[i])) {
                continue;
            }
            let weights = weights.map(|n| n / area);
            let reciprocal: f64 = (0..3).map(|i| weights[i] * vertices[i][3]).sum();
            if reciprocal <= 0. {
                continue;
            }
            let mut rgba: [f64; 4] = std::array::from_fn(|axis| {
                (0..3)
                    .map(|i| weights[i] * vertices[i][3] * vertices[i][4 + axis])
                    .sum::<f64>()
                    / reciprocal
            });
            if let Some(texture) = texture {
                let uv: [f64; 2] = std::array::from_fn(|axis| {
                    (0..3)
                        .map(|i| weights[i] * vertices[i][8 + axis])
                        .sum::<f64>()
                        / reciprocal
                });
                let sample = sample(texture, uv);
                for axis in 0..4 {
                    rgba[axis] *= sample[axis] / 255.;
                }
            }
            let rgba = rgba.map(|n| n.round().clamp(0., 255.) as u8);
            if rgba[3] != 0 {
                canvas.blend_pixel_with_mode(
                    x,
                    y,
                    Color::rgba(rgba[0], rgba[1], rgba[2], rgba[3]),
                    BlendMode::from_raw(view.blend),
                )?;
            }
        }
    }
    Ok(())
}

fn sample(frame: &RgbaFrame, uv: [f64; 2]) -> [f64; 4] {
    if frame.width() == 0 || frame.height() == 0 {
        return [0.; 4];
    }
    let x = (uv[0] - 0.5).clamp(0., f64::from(frame.width() - 1));
    let y = (uv[1] - 0.5).clamp(0., f64::from(frame.height() - 1));
    let x0 = x.floor() as u32;
    let x1 = (x0 + 1).min(frame.width() - 1);
    let y0 = y.floor() as u32;
    let y1 = (y0 + 1).min(frame.height() - 1);
    let dx = x - x.floor();
    let dy = y - y.floor();
    let mut output = [0.; 4];
    for (x, y, weight) in [
        (x0, y0, (1. - dx) * (1. - dy)),
        (x1, y0, dx * (1. - dy)),
        (x0, y1, (1. - dx) * dy),
        (x1, y1, dx * dy),
    ] {
        let at = (y as usize * frame.width() as usize + x as usize) * 4;
        for axis in 0..4 {
            output[axis] += f64::from(frame.pixels()[at + axis]) * weight;
        }
    }
    output
}
