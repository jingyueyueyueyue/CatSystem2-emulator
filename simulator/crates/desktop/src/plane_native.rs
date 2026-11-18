//! Ordinary two-dimensional plane wrappers and their CPU presentation.
//!
//! Install `PlaneServices` as `DataServices.planes`. The optional native guard
//! follows movie_plane_native. Geometry wrappers share PLANE_GEOMETRY_ABI and
//! the same explicit-buffer command receiver. Remaining geometry constructors
//! notify observe_existing. The mixed scene pass consumes the saved properties.

use std::collections::BTreeMap;

use anyhow::{Context, Result as HostResult, bail};
use cs2_renderer::{BlendMode, Canvas, Color, Rect, RgbaFrame};
use cs2_runtime::TaskCommand;
use cs2_script::{NativeReturn, Result, ScriptError, Vm};

use super::canvas_native::VramOverflow;
use super::movie_plane::PlaneCamera;
use super::movie_plane_render::MoviePlaneProjection;
use super::{DesktopInputTask, HeadlessPlayer, InputRegionState, LoggingHost, SceneNode};

#[path = "plane_history.rs"]
mod history;

#[path = "plane_snapshot.rs"]
mod snapshot;
pub(crate) use snapshot::PreparedPlaneVisuals;

#[path = "raster_plane.rs"]
mod raster;
pub(crate) use raster::RasterVertexTransform;

const IDENTITY: [f32; 16] = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];

const V401_PLANE_COMMANDS: &[(u32, u32)] = &[
    (44, 46),
    (45, 47),
    (46, 48),
    (47, 49),
    (48, 50),
    (49, 51),
    (50, 52),
    (51, 53),
    (52, 54),
    (53, 55),
    (54, 56),
    (55, 57),
    (56, 58),
    (57, 59),
    (59, 61),
    (60, 62),
    (61, 63),
    (64, 66),
    (65, 67),
    (73, 75),
    (74, 76),
    (77, 79),
    (78, 80),
    (79, 81),
    (80, 82),
    (82, 84),
    (83, 85),
    (84, 86),
];

/// Ordinary-table wrapper ABI; object command ids are a separate namespace.
#[derive(Clone, Copy, Debug)]
pub(super) struct PlaneGeometryAbi {
    pub slot: u32,
    pub command: u32,
    pub argument_bytes: usize,
    pub zero_target_skips: bool,
    pub returns_value: bool,
}

pub(super) const PLANE_GEOMETRY_ABI: &[PlaneGeometryAbi] = &[
    abi(84, 51, 20, true, false),
    abi(85, 49, 16, true, false),
    abi(86, 61, 16, true, false),
    abi(87, 62, 20, true, false),
    abi(88, 49, 8, true, false),
    abi(89, 61, 4, true, true),
    abi(90, 53, 16, true, false),
    abi(91, 54, 4, true, false),
    abi(92, 47, 8, true, false),
    abi(93, 55, 20, true, false),
    abi(94, 63, 8, true, false),
    abi(100, 59, 12, true, false),
    abi(101, 66, 20, true, false),
    abi(343, 75, 12, true, false),
    abi(344, 57, 12, true, false),
    abi(345, 76, 20, true, false),
    abi(402, 81, 8, false, false),
    abi(466, 82, 8, true, false),
    abi(653, 84, 8, false, false),
    abi(1066, 591, 8, false, false),
    abi(1067, 592, 4, false, false),
    abi(1068, 591, 8, false, false),
    abi(1069, 592, 4, false, false),
];

const fn abi(
    slot: u32,
    command: u32,
    argument_bytes: usize,
    zero_target_skips: bool,
    returns_value: bool,
) -> PlaneGeometryAbi {
    PlaneGeometryAbi {
        slot,
        command,
        argument_bytes,
        zero_target_skips,
        returns_value,
    }
}

#[derive(Default)]
pub(super) struct PlaneCommandBuffers<'a> {
    pub data0: Option<&'a mut [u8]>,
    pub data1: Option<&'a mut [u8]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) enum PlaneShape {
    Rectangle,
    Canvas,
    Empty,
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Geometry {
    base: [f32; 3],
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
    matrix: Option<[f32; 16]>,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            base: [0., 0., 5000.],
            position: [0.; 3],
            rotation: [0.; 3],
            scale: [1.; 3],
            matrix: None,
        }
    }
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct PlaneState {
    wrapper: Geometry,
    source: Geometry,
    pub(super) shape: PlaneShape,
    extent: [f32; 2],
    pub(super) show: u32,
    display: u32,
    pub(super) color: u32,
    pub(super) color_enabled: u32,
    pub(super) create: u32,
    input_configured: bool,
    receiver: Option<u32>,
    backup_configuration: u32,
    input_geometry_dirty: bool,
    #[serde(with = "snapshot::blend")]
    blend: BlendMode,
    camera: u32,
    perspective_origin: u32,
    #[serde(with = "snapshot::rectangle")]
    clip: Option<Rect>,
    parent_offset: Option<[f32; 3]>,
    declared_bounds: Option<[f32; 4]>,
}

impl PlaneState {
    fn from_node(node: &SceneNode) -> Self {
        let mut wrapper = Geometry::default();
        wrapper.position = node.position;
        wrapper.scale = node.scale;
        Self {
            wrapper,
            source: Geometry::default(),
            shape: PlaneShape::Rectangle,
            extent: node.size,
            show: 1,
            display: u32::from(node.visible),
            color: u32::MAX,
            color_enabled: 0,
            create: 1,
            input_configured: false,
            receiver: None,
            backup_configuration: 0,
            input_geometry_dirty: false,
            blend: BlendMode::SourceOver,
            camera: 0,
            perspective_origin: 0,
            clip: None,
            parent_offset: None,
            declared_bounds: None,
        }
    }

    fn geometry(&self, source: u32) -> &Geometry {
        if source != 0 && self.shape == PlaneShape::Canvas {
            &self.source
        } else {
            &self.wrapper
        }
    }

    fn geometry_mut(&mut self, source: u32) -> &mut Geometry {
        if source != 0 && self.shape == PlaneShape::Canvas {
            &mut self.source
        } else {
            &mut self.wrapper
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PlaneServices {
    pub(super) vram_overflow: Option<VramOverflow>,
    planes: BTreeMap<u32, PlaneState>,
    order: Vec<u32>,
    order_depths: BTreeMap<u32, u32>,
    order_dirty: bool,
    grid_records: BTreeMap<u32, [u32; 10]>,
    histories: BTreeMap<u32, history::PlaneHistory>,
    history_owners: BTreeMap<u32, u32>,
    inert_media: std::collections::BTreeSet<u32>,
    raster: raster::RasterStore,
}

impl PlaneServices {
    pub(super) fn observe_movie_creation_flag(&mut self, id: u32, create: u32) {
        if let Some(plane) = self.planes.get_mut(&id) { plane.create = create; }
    }

    pub(super) fn register_non_draw_media(
        &mut self,
        id: u32,
        kind: u32,
        profile: Option<&cs2_runtime::native_profile::FeatureProfile>,
    ) {
        self.inert_media.remove(&id);
        // These two identified Hydro constructors only call their task base.
        // Their virtual update/message entries install no drawable callback.
        if kind == 25
            && profile.is_some_and(|profile| {
                matches!(
                    profile.pe_sha256,
                    "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f"
                        | "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"
                )
            })
        {
            self.inert_media.insert(id);
        }
    }

    pub(super) fn sync_fes_plane(
        &mut self,
        node: &mut SceneNode,
        canvas: bool,
        size: [u32; 2],
        vram: [i32; 2],
        words: &[u32; 36],
        priority: i32,
        visible: bool,
        clip: Option<Rect>,
    ) {
        let state = self
            .planes
            .entry(node.id)
            .or_insert_with(|| PlaneState::from_node(node));
        state.shape = if canvas {
            PlaneShape::Canvas
        } else {
            PlaneShape::Rectangle
        };
        state.extent = size.map(|value| value as f32);
        state.wrapper.base = std::array::from_fn(|axis| f32::from_bits(words[3 + axis]));
        // FES submits base X/Y and its independent PRI to the lower plane;
        // motion DWORD5 remains the abstract object's saved base Z.
        state.wrapper.base[2] = priority as f32;
        state.wrapper.position = std::array::from_fn(|axis| f32::from_bits(words[6 + axis]));
        state.wrapper.scale = std::array::from_fn(|axis| f32::from_bits(words[25 + axis]));
        state.wrapper.rotation = std::array::from_fn(|axis| f32::from_bits(words[29 + axis]));
        state.wrapper.matrix = Some(std::array::from_fn(|axis| f32::from_bits(words[9 + axis])));
        state.source = Geometry::default();
        state.source.position = [vram[0] as f32, vram[1] as f32, 0.];
        state.color = words[28];
        state.color_enabled = 1;
        state.show = words[35];
        state.display = if visible { state.show } else { 0 };
        state.clip = clip;
        node.position = state.wrapper.position;
        node.scale = state.wrapper.scale;
        node.size = state.extent;
        node.visible = state.display != 0;
        node.initialized = true;
        node.color = state.color;
        self.register_drawable(node.id);
    }

    pub(super) fn initialize_list_plane(&mut self, node: &mut SceneNode, size: [u32; 2]) {
        let mut state = PlaneState::from_node(node);
        state.wrapper = Geometry::default();
        state.wrapper.base[2] = 1000.;
        state.source = Geometry::default();
        state.shape = PlaneShape::Canvas;
        state.extent = size.map(|value| value as f32);
        state.display = 0;
        node.position = [0.; 3];
        node.scale = [1.; 3];
        node.size = state.extent;
        node.color = u32::MAX;
        node.visible = false;
        node.initialized = true;
        self.planes.insert(node.id, state);
        self.register_drawable(node.id);
    }

    pub(super) fn set_list_plane_visible(&mut self, node: &mut SceneNode, visible: bool) {
        if let Some(state) = self.planes.get_mut(&node.id) {
            state.show = u32::from(visible);
            state.display = state.show;
            node.visible = visible;
        }
    }

    /// 77 queries a root service and belongs to the system adapter. Ordinary
    /// slots and object command numbers remain separate namespaces.
    pub(super) fn supports(index: u32) -> bool {
        matches!(
            index,
            69 | 70 | 78 | 79 | 80 | 83 | 95 | 106 | 107 | 116 | 398 | 399 | 433 | 654 | 655
        ) || PLANE_GEOMETRY_ABI.iter().any(|abi| abi.slot == index)
    }

    pub(super) fn state(&self, id: u32) -> Option<&PlaneState> {
        self.planes.get(&id)
    }

    pub(crate) fn history_owner(&self, id: u32) -> Option<u32> {
        self.history_owners.get(&id).copied()
    }

    pub(super) fn release(&mut self, id: u32) {
        self.raster.release(id);
        self.inert_media.remove(&id);
        self.planes.remove(&id);
        self.order.retain(|&registered| registered != id);
        self.order_depths.remove(&id);
        self.grid_records.remove(&id);
        self.histories.remove(&id);
        self.history_owners.remove(&id);
    }

    fn register_drawable(&mut self, id: u32) {
        self.order.retain(|&registered| registered != id);
        self.order.push(id);
        self.order_dirty = true;
    }

    /// Use instead of a total_cmp/id sort. Keeps the prior order on equal
    /// depths and follows ordinary floating comparisons for NaN in each merge.
    pub(super) fn draw_order(&mut self, nodes: &BTreeMap<u32, SceneNode>) -> Vec<u32> {
        self.draw_order_with_depths(nodes, &BTreeMap::new())
    }

    /// Foreign drawable owners supply their actual depth without copying any
    /// of their transform/color state into the ordinary-plane property map.
    pub(super) fn draw_order_with_depths(
        &mut self,
        nodes: &BTreeMap<u32, SceneNode>,
        depths: &BTreeMap<u32, f32>,
    ) -> Vec<u32> {
        self.order
            .retain(|id| nodes.get(id).is_some_and(|node| node.initialized));
        self.order_depths.retain(|id, _| nodes.contains_key(id));
        let mut registered = self
            .order
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        for (&id, node) in nodes {
            if !node.initialized {
                continue;
            }
            if registered.insert(id) {
                self.order.push(id);
                self.order_dirty = true;
            }
            let bits = depths
                .get(&id)
                .copied()
                .unwrap_or_else(|| plane_depth(node, self))
                .to_bits();
            if self.order_depths.insert(id, bits) != Some(bits) {
                self.order_dirty = true;
            }
        }
        if self.order_dirty {
            let mut source = self.order.clone();
            let mut destination = source.clone();
            let mut width = 1;
            while width < source.len() {
                for start in (0..source.len()).step_by(width.saturating_mul(2)) {
                    let middle = start.saturating_add(width).min(source.len());
                    let end = middle.saturating_add(width).min(source.len());
                    let (mut left, mut right) = (start, middle);
                    for output in &mut destination[start..end] {
                        let take_right = if left == middle {
                            true
                        } else if right == end {
                            false
                        } else {
                            let a = f32::from_bits(self.order_depths[&source[left]]);
                            let b = f32::from_bits(self.order_depths[&source[right]]);
                            a > b
                        };
                        if take_right {
                            *output = source[right];
                            right += 1;
                        } else {
                            *output = source[left];
                            left += 1;
                        }
                    }
                }
                std::mem::swap(&mut source, &mut destination);
                width = width.saturating_mul(2);
            }
            self.order = source;
            self.order_dirty = false;
        }
        self.order.clone()
    }

    /// Call after a successful existing native. This records raw DWORD state
    /// and replacement resets; it neither dispatches nor claims those slots.
    pub(super) fn observe_existing(
        &mut self,
        index: u32,
        args: &[u8],
        nodes: &mut BTreeMap<u32, SceneNode>,
    ) -> Result<()> {
        let id = word(args, 0)?;
        if id == 0 {
            return Ok(());
        }
        let Some(node) = nodes.get_mut(&id) else {
            return Ok(());
        };
        if node.kind != 2 {
            return Ok(());
        }
        if matches!(index, 79 | 80 | 83) {
            self.invalidate_history_geometry(id);
            if index != 83 {
                self.grid_records.remove(&id);
            }
            let previous = self.planes.remove(&id);
            if index == 83 {
                // Command 68 removes geometry, not the plane's task state.
                // In particular its create gate and input binding survive.
                let mut retained = previous.unwrap_or_else(|| PlaneState::from_node(node));
                retained.extent = [0.; 2];
                if let Some(bounds) = &mut retained.declared_bounds {
                    bounds[2] = bounds[0];
                    bounds[3] = bounds[1];
                }
                node.size = [0.; 2];
                self.planes.insert(id, retained);
                self.order.retain(|&registered| registered != id);
                return Ok(());
            }
            let create = previous.as_ref().map_or(1, |state| state.create);
            if create != 1 {
                node.initialized = false;
                if let Some(previous) = previous {
                    node.size = previous.extent.map(f32::abs);
                    node.position = previous.wrapper.position;
                    node.scale = previous.wrapper.scale;
                    self.planes.insert(id, previous);
                }
                return Ok(());
            }
            let mut fresh = PlaneState::from_node(node);
            fresh.wrapper = Geometry::default();
            fresh.source = Geometry::default();
            fresh.create = create;
            if let Some(previous) = &previous {
                fresh.input_configured = previous.input_configured;
                fresh.receiver = previous.receiver;
                fresh.backup_configuration = previous.backup_configuration;
            }
            fresh.show = previous.as_ref().map_or(1, |state| state.show);
            fresh.display = 0;
            fresh.parent_offset = previous.as_ref().and_then(|state| state.parent_offset);
            fresh.shape = if index == 80 {
                PlaneShape::Empty
            } else {
                PlaneShape::Rectangle
            };
            fresh.extent = if index == 80 {
                [0.; 2]
            } else {
                [float(args, 4)?, float(args, 8)?]
            };
            node.position = [0.; 3];
            node.scale = [1.; 3];
            node.visible = false;
            node.size = fresh.extent.map(f32::abs);
            node.overlay_color = 0;
            self.planes.insert(id, fresh);
            self.register_drawable(id);
            return Ok(());
        }
        let state = self
            .planes
            .entry(id)
            .or_insert_with(|| PlaneState::from_node(node));
        match index {
            84 => {
                let position = [float(args, 4)?, float(args, 8)?, float(args, 12)?];
                let selector = word(args, 16)?;
                let geometry = state.geometry_mut(selector);
                if geometry.position != position {
                    geometry.position = position;
                }
                node.position = state.wrapper.position;
                state.input_geometry_dirty = true;
            }
            88 => {
                // The existing legacy handler used position.z. Restore the
                // separately retained translation and preserve exact base bits.
                state.wrapper.base[2] = float(args, 4)?;
                node.position = state.wrapper.position;
                state.input_geometry_dirty = true;
                self.order_dirty = true;
            }
            92 => {
                state.show = word(args, 4)?;
                state.display = state.show;
                node.visible = state.show != 0;
            }
            100 => {
                state.color = word(args, 4)?;
                state.color_enabled = word(args, 8)?;
            }
            101 => {
                let scale = [float(args, 4)?, float(args, 8)?, float(args, 12)?];
                if state.wrapper.scale != scale {
                    state.wrapper.scale = scale;
                }
                node.scale = state.wrapper.scale;
                state.input_geometry_dirty = true;
            }
            653 => state.create = word(args, 4)?,
            _ => {
                return Err(error(
                    "plane observer received a slot owned by another service",
                ));
            }
        }
        Ok(())
    }

    /// Supplies an explicit matrix for an existing geometry-command adapter.
    /// Comparison uses all DWORDs, including NaN payloads.
    pub(super) fn set_matrix(&mut self, node: &SceneNode, source: u32, matrix: [f32; 16]) {
        let state = self
            .planes
            .entry(node.id)
            .or_insert_with(|| PlaneState::from_node(node));
        let geometry = state.geometry_mut(source);
        if geometry.matrix.as_ref().is_none_or(|old| {
            old.iter()
                .zip(matrix)
                .any(|(left, right)| left.to_bits() != right.to_bits())
        }) {
            geometry.matrix = Some(matrix);
        }
        state.input_geometry_dirty = true;
    }

    pub(super) fn input_receiver(&self, id: u32) -> Option<u32> {
        self.planes.get(&id).and_then(|state| state.receiver)
    }

    pub(super) fn source_position(&self, id: u32, selector: u32) -> Option<[f32; 3]> {
        self.planes
            .get(&id)
            .map(|state| state.geometry(selector).position)
    }
}

fn geometry_words(geometry: &Geometry, display: u32, color: u32) -> [u32; 36] {
    let mut words = [0u32; 36];
    for axis in 0..3 {
        words[3 + axis] = geometry.base[axis].to_bits();
        words[6 + axis] = geometry.position[axis].to_bits();
        words[25 + axis] = geometry.scale[axis].to_bits();
        words[29 + axis] = geometry.rotation[axis].to_bits();
    }
    let mut unscaled = geometry.clone();
    unscaled.scale = [1.; 3];
    words[9..25].copy_from_slice(&geometry_matrix(&unscaled).map(f32::to_bits));
    words[28] = color;
    words[35] = display;
    words
}

pub(crate) fn kx_plane_view(
    node: &SceneNode,
    services: &PlaneServices,
) -> Result<super::message_platform::kx_native::KxPlaneView> {
    let state = services
        .planes
        .get(&node.id)
        .ok_or_else(|| error("KX geometry has no live ordinary plane properties"))?;
    let mut wrapper = state.wrapper.clone();
    wrapper.position = node.position;
    wrapper.scale = node.scale;
    let display = if node.initialized && node.visible {
        state.display
    } else {
        0
    };
    let source = state.geometry(1);
    Ok(super::message_platform::kx_native::KxPlaneView {
        words: geometry_words(&wrapper, display, state.color),
        source_words: geometry_words(source, 1, u32::MAX),
        wrap_transform: geometry_matrix(&wrapper),
        source_transform: geometry_matrix(source),
        blend: state.blend.raw(),
        clip: state.clip,
        camera: state.camera,
        perspective_origin: state.perspective_origin,
        color: (state.color_enabled != 0).then_some(state.color),
        raster_wrap: services.raster.callback(node.id, false),
        raster_source: services.raster.callback(node.id, true),
    })
}

pub(crate) fn install_kx_plane(
    services: &mut PlaneServices,
    node: &mut SceneNode,
    canvas_wrapped: bool,
    bounds: [f32; 4],
) -> Result<()> {
    if node.kind != 2 {
        return Err(error("KX installation requires an ordinary plane owner"));
    }
    let previous = services.planes.remove(&node.id);
    let mut state = PlaneState::from_node(node);
    if let Some(previous) = previous {
        state.show = previous.show;
        state.create = previous.create;
        state.parent_offset = previous.parent_offset;
        state.input_configured = previous.input_configured;
        state.receiver = previous.receiver;
        state.backup_configuration = previous.backup_configuration;
    }
    state.wrapper = Geometry::default();
    state.source = Geometry::default();
    state.shape = if canvas_wrapped {
        PlaneShape::Canvas
    } else {
        PlaneShape::Rectangle
    };
    state.extent = [(bounds[2] - bounds[0]).abs(), (bounds[3] - bounds[1]).abs()];
    state.declared_bounds = Some(bounds);
    state.display = 0;
    node.size = state.extent;
    node.position = [0.; 3];
    node.scale = [1.; 3];
    node.color = u32::MAX;
    node.overlay_color = 0;
    node.visible = false;
    node.initialized = true;
    services.grid_records.remove(&node.id);
    services.invalidate_history_geometry(node.id);
    services.planes.insert(node.id, state);
    services.register_drawable(node.id);
    Ok(())
}

impl LoggingHost<'_> {
    pub(super) fn clear_plane_mesh(&mut self, id: u32) {
        self.data_services.messages.kx.objects.remove(&id);
        self.data_services.messages.kx.release_metadata(id);
    }

    /// Call only after the movie/camera adapter. None leaves another service in
    /// charge; recognized ordinary planes propagate errors and real results.
    pub(super) fn plane_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
    ) -> Result<Option<NativeReturn>> {
        if let Some(result) = self.raster_native(index, args, vm)? {
            return Ok(Some(result));
        }
        if !PlaneServices::supports(index) {
            return Ok(None);
        }
        if matches!(index, 69 | 70) {
            return self.plane_raw_native(index, args, vm);
        }
        if flush_input_geometry(
            &mut self.data_services.planes,
            self.scene_nodes,
            self.input_task,
        ) {
            self.mark_input_geometry_dirty();
        }
        if index == 116 {
            return self.plane_wrapper_copy(args).map(Some);
        }
        if matches!(index, 398 | 399) {
            return self.plane_history_native(index, args, vm);
        }
        if matches!(index, 654 | 655) {
            let id = word(args, 0)?;
            if (id != 0 && self.scene_nodes.get(&id).is_some_and(|node| node.kind != 2))
                || self
                    .movie_host
                    .as_ref()
                    .is_some_and(|movie| movie.plane_target_handle(id).is_some())
            {
                return Ok(None);
            }
            let formatted = |at| -> Result<Vec<u8>> {
                let source = vm.read_string(word(args, at)?)?;
                let mut value = super::expand_kcs_format(vm, &source, 0)?;
                value.truncate(
                    value
                        .iter()
                        .position(|&byte| byte == 0)
                        .unwrap_or(value.len()),
                );
                if value.len() >= 256 {
                    return Err(error("plane formatted name exceeds 255 bytes"));
                }
                value.push(0);
                Ok(value)
            };
            let first = formatted(4)?;
            let second = if index == 654 {
                Some(formatted(8)?)
            } else {
                None
            };
            if index == 655 && first.len() == 1 {
                return Ok(Some(NativeReturn::Continue));
            }
            let (mut first, mut second) = if index == 654 {
                (Some(first), second)
            } else {
                (None, Some(first))
            };
            self.dispatch_plane_command(
                TaskCommand {
                    target: id as i32,
                    sender: self.current_task_id as i32,
                    code: if index == 654 { 85 } else { 86 },
                    data: [u32::from(index == 654), 1, 0],
                },
                &mut PlaneCommandBuffers {
                    data0: first.as_deref_mut(),
                    data1: second.as_deref_mut(),
                },
                &mut 0,
            )?;
            return Ok(Some(NativeReturn::Continue));
        }
        if matches!(index, 79 | 80 | 83) {
            let id = word(args, 0)?;
            if id == 0 {
                return Ok(Some(NativeReturn::Continue));
            }
            if self.scene_nodes.get(&id).is_some_and(|node| node.kind != 2)
                || self
                    .movie_host
                    .as_ref()
                    .is_some_and(|movie| movie.plane_target_handle(id).is_some())
            {
                return Ok(None);
            }
            let rectangle = if index == 79 {
                Some([word(args, 4)?, word(args, 8)?, word(args, 12)?])
            } else {
                None
            };
            if self
                .data_services
                .tasks
                .message_task_state(id)
                .is_some_and(|(_, deleted, _)| deleted != 0)
            {
                return Ok(Some(NativeReturn::Continue));
            }
            self.clear_plane_mesh(id);
            let Some(node) = self.scene_nodes.get_mut(&id) else {
                return Ok(Some(NativeReturn::Continue));
            };
            node.initialized = index != 83;
            if let Some([width, height, color]) = rectangle {
                node.size = [f32::from_bits(width), f32::from_bits(height)];
                node.color = color;
            }
            if index == 83 {
                node.visible = false;
                self.particles.remove(&id);
            }
            self.data_services
                .planes
                .observe_existing(index, args, self.scene_nodes)?;
            return Ok(Some(NativeReturn::Continue));
        }
        if index == 433 {
            let id = word(args, 0)?;
            if id == 0 {
                return Ok(Some(NativeReturn::Continue));
            }
            if self.scene_nodes.get(&id).is_some_and(|node| node.kind != 2)
                || self
                    .movie_host
                    .as_ref()
                    .is_some_and(|movie| movie.plane_target_handle(id).is_some())
            {
                return Ok(None);
            }
            let words = [
                0,
                0,
                0,
                word(args, 4)?,
                word(args, 8)?,
                0,
                0,
                0,
                word(args, 12)?,
                word(args, 16)?,
            ];
            let mut bytes: Vec<_> = words.into_iter().flat_map(u32::to_le_bytes).collect();
            self.dispatch_plane_command(
                TaskCommand {
                    target: id as i32,
                    sender: self.current_task_id as i32,
                    code: 46,
                    data: [0; 3],
                },
                &mut PlaneCommandBuffers {
                    data0: None,
                    data1: Some(&mut bytes),
                },
                &mut 0,
            )?;
            return Ok(Some(NativeReturn::Continue));
        }
        if let Some(abi) = PLANE_GEOMETRY_ABI.iter().find(|abi| abi.slot == index) {
            return self.plane_geometry_native(*abi, args, vm);
        }
        let id = word(args, 0)?;
        if index == 107 {
            // Zero is a valid region id; no native zero-target shortcut.
            let selected = if self.input_task.alive {
                self.input_task
                    .regions
                    .iter()
                    .find(|region| region.id() == id)
                    .map_or(0, |region| region.words[7])
            } else {
                0
            };
            return Ok(Some(NativeReturn::Value(selected)));
        }
        if id == 0 && index != 106 {
            return Ok(Some(NativeReturn::Continue));
        }
        if self.scene_nodes.get(&id).is_some_and(|node| node.kind != 2) {
            return Ok(None);
        }
        if self
            .movie_host
            .as_ref()
            .is_some_and(|host| host.plane_target_handle(id).is_some())
        {
            return Ok(None);
        }

        match index {
            78 => {
                let width = float(args, 4)?;
                let height = float(args, 8)?;
                if self
                    .data_services
                    .tasks
                    .message_task_state(id)
                    .is_some_and(|(_, deletion, _)| deletion != 0)
                {
                    return Ok(Some(NativeReturn::Continue));
                }
                self.clear_plane_mesh(id);
                self.data_services.planes.invalidate_history_geometry(id);
                let Some(node) = self.scene_nodes.get_mut(&id) else {
                    return Ok(Some(NativeReturn::Continue));
                };
                self.data_services.planes.grid_records.remove(&id);
                let old = self.data_services.planes.planes.remove(&id);
                let create = old.as_ref().map_or(1, |state| state.create);
                if create != 1 {
                    node.initialized = false;
                    if let Some(old) = old {
                        self.data_services.planes.planes.insert(id, old);
                    }
                    return Ok(Some(NativeReturn::Continue));
                }
                // A real canvas texture requires a live canvas layout/source.
                // Failure occurs after removing the previous drawable.
                node.initialized = false;
                self.canvas
                    .as_ref()
                    .ok_or_else(|| error("native 78 requires a live drawing canvas"))?;
                let mut state = PlaneState::from_node(node);
                state.wrapper = Geometry::default();
                state.source = Geometry::default();
                state.shape = PlaneShape::Canvas;
                state.extent = [width, height];
                state.show = old.as_ref().map_or(1, |state| state.show);
                state.display = 0;
                state.parent_offset = old.as_ref().and_then(|state| state.parent_offset);
                state.create = create;
                if let Some(old) = &old {
                    state.input_configured = old.input_configured;
                    state.receiver = old.receiver;
                    state.backup_configuration = old.backup_configuration;
                }
                node.position = [0.; 3];
                node.scale = [1.; 3];
                node.size = [width.abs(), height.abs()];
                node.color = u32::MAX;
                node.overlay_color = 0;
                node.visible = false;
                node.initialized = true;
                self.data_services.planes.planes.insert(id, state);
                self.data_services.planes.register_drawable(id);
            }
            95 => {
                let backup = word(args, 4)?;
                self.register_plane_input(id, backup)?;
            }
            106 => {
                let selected = word(args, 4)?;
                if self
                    .data_services
                    .tasks
                    .message_task_state(id)
                    .is_some_and(|(_, deleted, _)| deleted != 0)
                {
                    return Ok(Some(NativeReturn::Continue));
                }
                let bound = self
                    .data_services
                    .planes
                    .planes
                    .get(&id)
                    .is_some_and(|state| state.receiver.is_some());
                if bound && self.input_task.alive {
                    for region in &mut self.input_task.regions {
                        if region.id() == id {
                            region.words[7] = selected;
                        } else if selected != 0 {
                            region.words[7] = 0;
                        }
                    }
                    if selected == 0 && self.input_task.selected_region == Some(id) {
                        self.input_task.selected_region = None;
                    }
                }
            }
            _ => {
                return Err(error(
                    "ordinary plane slot has no independent implementation",
                ));
            }
        }
        Ok(Some(NativeReturn::Continue))
    }

    fn plane_geometry_native(
        &mut self,
        abi: PlaneGeometryAbi,
        args: &[u8],
        vm: &mut Vm,
    ) -> Result<Option<NativeReturn>> {
        let id = word(args, 0)?;
        if id == 0 && abi.zero_target_skips {
            return Ok(Some(if abi.returns_value {
                NativeReturn::Value(0)
            } else {
                NativeReturn::Continue
            }));
        }
        if (id != 0 && self.scene_nodes.get(&id).is_some_and(|node| node.kind != 2))
            || self
                .movie_host
                .as_ref()
                .is_some_and(|host| host.plane_target_handle(id).is_some())
        {
            return Ok(None);
        }
        if args.len() < abi.argument_bytes {
            return Err(ScriptError::Truncated {
                offset: args.len(),
                needed: abi.argument_bytes - args.len(),
            });
        }
        let mut packet = TaskCommand {
            target: id as i32,
            sender: self.current_task_id as i32,
            code: abi.command,
            data: [0; 3],
        };
        let mut output = 0;
        let dispatch = |host: &mut Self,
                        packet,
                        data0: Option<&mut [u8]>,
                        data1: Option<&mut [u8]>,
                        output: &mut u32| {
            host.dispatch_plane_command(packet, &mut PlaneCommandBuffers { data0, data1 }, output)
        };
        match abi.slot {
            90 => {
                let mut vector = args[4..16].to_vec();
                dispatch(self, packet, None, Some(&mut vector), &mut output)?;
            }
            84 | 93 | 101 => {
                packet.data = [word(args, 16)?, 0, u32::from(abi.slot == 93)];
                let mut values = args[4..16].to_vec();
                dispatch(self, packet, None, Some(&mut values), &mut output)?;
            }
            85 | 88 => {
                let mut values = [0u8; 12];
                let selector = if abi.slot == 85 { word(args, 12)? } else { 0 };
                let query = TaskCommand {
                    code: 61,
                    data: [selector, 0, 1],
                    ..packet
                };
                dispatch(self, query, None, Some(&mut values), &mut output)?;
                if abi.slot == 85 {
                    values[..8].copy_from_slice(&args[4..12]);
                } else {
                    values[8..12].copy_from_slice(&args[4..8]);
                }
                packet.data = [selector, 0, 1];
                dispatch(self, packet, None, Some(&mut values), &mut output)?;
            }
            86 | 87 => {
                packet.data = [word(args, if abi.slot == 86 { 12 } else { 16 })?, 0, 1];
                let mut values = [0u8; 12];
                dispatch(self, packet, None, Some(&mut values), &mut output)?;
                let count = if abi.slot == 86 { 2 } else { 3 };
                let addresses = output_addresses(vm, args, count)?;
                for (axis, address) in addresses.into_iter().enumerate() {
                    vm.write(address, &values[axis * 4..axis * 4 + 4])?;
                }
            }
            89 => {
                packet.data = [0, 0, 1];
                let mut values = [0u8; 12];
                dispatch(self, packet, None, Some(&mut values), &mut output)?;
                output = u32::from_le_bytes(
                    values[8..12]
                        .try_into()
                        .map_err(|_| error("plane base query size changed"))?,
                );
            }
            343 => {
                let addresses = output_addresses(vm, args, 2)?;
                let mut width = [0u8; 4];
                let mut height = [0u8; 4];
                dispatch(
                    self,
                    packet,
                    Some(&mut width),
                    Some(&mut height),
                    &mut output,
                )?;
                vm.write(addresses[0], &width)?;
                vm.write(addresses[1], &height)?;
            }
            345 => {
                let addresses = output_addresses(vm, args, 4)?;
                let mut low = [0u8; 12];
                let mut high = [0u8; 12];
                dispatch(self, packet, Some(&mut low), Some(&mut high), &mut output)?;
                for (index, address) in addresses.into_iter().enumerate() {
                    let (values, axis) = if index < 2 {
                        (&low, index)
                    } else {
                        (&high, index - 2)
                    };
                    vm.write(address, &values[axis * 4..axis * 4 + 4])?;
                }
            }
            344 => {
                let address = word(args, 4)?;
                packet.data = [word(args, 8)?, address, 1];
                vm.read(address, 1)?;
                if self.live_plane_node(id).is_some() {
                    let mut matrix = vm.read(address, 64)?;
                    let result = dispatch(self, packet, None, Some(&mut matrix), &mut output);
                    vm.write(address, &matrix)?;
                    result?;
                }
            }
            91 | 1067 | 1069 => {
                dispatch(self, packet, None, None, &mut output)?;
            }
            92 | 402 | 466 | 653 | 1066 | 1068 => {
                packet.data = [0, word(args, 4)?, u32::from(abi.slot == 92)];
                dispatch(self, packet, None, None, &mut output)?;
            }
            94 => {
                packet.data[1] = (word(args, 4)? as i32).clamp(0, 255) as u32;
                dispatch(self, packet, None, None, &mut output)?;
            }
            100 => {
                packet.data = [word(args, 8)?, word(args, 4)?, 0];
                dispatch(self, packet, None, None, &mut output)?;
            }
            _ => return Err(error("plane ABI record has no marshalling implementation")),
        }
        Ok(Some(if abi.returns_value {
            NativeReturn::Value(output)
        } else {
            NativeReturn::Continue
        }))
    }

    fn plane_raw_native(
        &mut self,
        slot: u32,
        args: &[u8],
        vm: &mut Vm,
    ) -> Result<Option<NativeReturn>> {
        let id = word(args, 0)?;
        if !self.scene_nodes.get(&id).is_some_and(|node| node.kind == 2)
            || self
                .movie_host
                .as_ref()
                .is_some_and(|host| host.plane_target_handle(id).is_some())
        {
            return Ok(None);
        }
        let raw_code = word(args, 4)?;
        let code = self.canonical_plane_command(raw_code);
        if !plane_command_supported(code) {
            return Ok(None);
        }
        let data = [word(args, 8)?, word(args, 12)?, 0];
        if self
            .data_services
            .tasks
            .message_task_state(id)
            .is_some_and(|(_, deleted, _)| deleted != 0)
        {
            return Ok(Some(NativeReturn::Value(0)));
        }
        if slot == 70 {
            let mut output = 0;
            self.queue_plane_raw_message(id,
                [self.current_task_id, raw_code, data[0], data[1]],
                &mut output, Some(vm))?;
            return Ok(Some(NativeReturn::Value(output)));
        }
        let mut first = match code {
            75 => Some(vm.read(data[0], 4)?),
            76 => Some(vm.read(data[0], 12)?),
            _ => None,
        };
        let mut second = match code {
            79 if data[0] != 0 && self.scene_nodes[&id].initialized => Some(vm.read(data[1], 52)?),
            80 if self.plane_history_accepts_snapshot(id) => Some(vm.read(data[1], 128)?),
            46 if self
                .data_services
                .planes
                .planes
                .get(&id)
                .is_none_or(|state| state.create == 1) =>
            {
                Some(vm.read(data[1], 40)?)
            }
            49 | 51 | 53 | 55 | 56 | 61 | 62 | 66 | 67 | 76 => Some(vm.read(data[1], 12)?),
            57 | 58 => Some(vm.read(data[1], 64)?),
            75 => Some(vm.read(data[1], 4)?),
            89 if data[1] != 0 => Some(vm.read(data[1], 16)?),
            _ => None,
        };
        let mut output = 0;
        let packet = TaskCommand {
            target: id as i32,
            sender: self.current_task_id as i32,
            code,
            data,
        };
        let result = self.dispatch_plane_command(
            packet,
            &mut PlaneCommandBuffers {
                data0: first.as_deref_mut(),
                data1: second.as_deref_mut(),
            },
            &mut output,
        );
        if matches!(code, 75 | 76) {
            if let Some(first) = first {
                vm.write(data[0], &first)?;
            }
        }
        if matches!(code, 57 | 58 | 61 | 62 | 75 | 76) {
            if let Some(second) = second {
                vm.write(data[1], &second)?;
            }
        }
        result?;
        Ok(Some(NativeReturn::Value(output)))
    }

    fn canonical_plane_command(&self, raw: u32) -> u32 {
        if self.data_services.native_profile.is_some_and(|profile|
            profile.pe_sha256 == "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f") {
            V401_PLANE_COMMANDS.iter()
                .find_map(|&(from, to)| (from == raw).then_some(to)).unwrap_or(raw)
        } else { raw }
    }

    /// Raw70 and task-message child traversal use the same owned input queue.
    /// The command is canonicalized once; the Body receiver does not remap it.
    pub(crate) fn queue_plane_raw_message(
        &mut self, id: u32, packet: [u32; 4], output: &mut u32,
        vm: Option<&Vm>,
    ) -> Result<Option<cs2_runtime::TaskCommandOutcome>> {
        let Some(node) = self.scene_nodes.get(&id).filter(|node| node.kind == 2) else {
            return Ok(None);
        };
        if self.movie_host.as_ref().is_some_and(|host| host.plane_target_handle(id).is_some()) {
            return Ok(None);
        }
        let code = self.canonical_plane_command(packet[1]);
        if !plane_command_supported(code) { return Ok(None); }
        if self.data_services.tasks.message_task_state(id)
            .is_some_and(|(_, deleted, _)| deleted != 0)
        {
            *output = 0;
            return Ok(Some(cs2_runtime::TaskCommandOutcome { done: true, propagate: false }));
        }
        if matches!(code, 61 | 62 | 75 | 76) {
            return Err(error("queued plane query requires a retained VM output destination"));
        }
        let data = [packet[2], packet[3], 0];
        let size = match code {
            46 if self.data_services.planes.planes.get(&id)
                .is_none_or(|state| state.create == 1) => 40,
            79 if data[0] != 0 && node.initialized => 52,
            80 if self.plane_history_accepts_snapshot(id) => 128,
            49 | 51 | 53 | 55 | 56 | 66 | 67 => 12,
            57 | 58 => 64,
            89 if data[1] != 0 => 16,
            _ => 0,
        };
        let bytes = if size == 0 { None } else {
            Some(vm.ok_or_else(|| error("queued plane input requires its originating VM"))?
                .read(data[1], size)?)
        };
        let found = self.queue_task_command_buffers(TaskCommand {
            target: id as i32, sender: packet[0] as i32, code, data,
        }, &super::OwnedTaskBuffers::new(None, bytes)?, output)?;
        Ok(found.then_some(cs2_runtime::TaskCommandOutcome { done: true, propagate: false }))
    }

    pub(super) fn dispatch_plane_command(
        &mut self,
        command: TaskCommand,
        buffers: &mut PlaneCommandBuffers<'_>,
        output: &mut u32,
    ) -> Result<bool> {
        self.plane_command_inner(command, buffers, output, 0)
    }

    fn plane_command_inner(
        &mut self,
        command: TaskCommand,
        buffers: &mut PlaneCommandBuffers<'_>,
        output: &mut u32,
        depth: usize,
    ) -> Result<bool> {
        if !plane_command_supported(command.code) {
            return Ok(false);
        }
        if depth > 256 {
            return Err(error("plane command propagation exceeds 256 levels"));
        }
        let id = command.target as u32;
        let Some(node) = self
            .scene_nodes
            .get(&id)
            .filter(|node| node.kind == 2)
            .cloned()
        else {
            return Ok(false);
        };
        if self
            .data_services
            .tasks
            .message_task_state(id)
            .is_some_and(|(_, deleted, _)| deleted != 0)
        {
            return Ok(true);
        }
        if self
            .movie_host
            .as_ref()
            .is_some_and(|host| host.plane_target_handle(id).is_some())
        {
            return Err(error(
                "movie-plane object commands require the movie receiver",
            ));
        }
        let mut propagate = None;
        match command.code {
            84 => {
                self.data_services
                    .planes
                    .planes
                    .entry(id)
                    .or_insert_with(|| PlaneState::from_node(&node))
                    .create = command.data[1];
            }
            85 | 86 => {}
            79 => {
                let record = if command.data[0] != 0 && node.initialized {
                    Some(require_buffer(&buffers.data1, 52)?.to_vec())
                } else {
                    None
                };
                self.configure_plane_history(id, record.as_deref())?;
            }
            80 => {
                self.push_plane_history_snapshot(id, buffers.data1.as_deref())?;
            }
            46 => {
                self.clear_plane_mesh(id);
                if self
                    .data_services
                    .planes
                    .planes
                    .get(&id)
                    .is_some_and(|state| state.create != 1)
                {
                    self.scene_nodes.get_mut(&id).unwrap().initialized = false;
                } else {
                    let record = require_buffer(&buffers.data1, 40)?;
                    let words: [u32; 10] = std::array::from_fn(|index| {
                        u32::from_le_bytes(record[index * 4..index * 4 + 4].try_into().unwrap())
                    });
                    self.create_plane_grid(id, words)?;
                }
            }
            61 | 62 => {
                if node.initialized {
                    let state = self
                        .data_services
                        .planes
                        .planes
                        .entry(id)
                        .or_insert_with(|| PlaneState::from_node(&node));
                    let geometry = state.geometry(command.data[0]);
                    let values = if command.code == 61 {
                        geometry.base
                    } else if command.data[0] == 0 || state.shape != PlaneShape::Canvas {
                        node.position
                    } else {
                        geometry.position
                    };
                    write_vector_buffer(&mut buffers.data1, values)?;
                }
            }
            75 | 76 => {
                if command.code == 75 {
                    require_buffer(&buffers.data0, 4)?;
                    require_buffer(&buffers.data1, 4)?;
                    buffers
                        .data0
                        .as_deref_mut()
                        .ok_or_else(|| error("plane width output is absent"))?[..4]
                        .copy_from_slice(&node.size[0].to_bits().to_le_bytes());
                    buffers
                        .data1
                        .as_deref_mut()
                        .ok_or_else(|| error("plane height output is absent"))?[..4]
                        .copy_from_slice(&node.size[1].to_bits().to_le_bytes());
                } else {
                    require_buffer(&buffers.data0, 12)?;
                    require_buffer(&buffers.data1, 12)?;
                    let bounds = self
                        .data_services
                        .planes
                        .planes
                        .get(&id)
                        .and_then(|state| state.declared_bounds)
                        .unwrap_or([0., 0., node.size[0], node.size[1]]);
                    write_vector_buffer(&mut buffers.data0, [bounds[0], bounds[1], 0.])?;
                    write_vector_buffer(&mut buffers.data1, [bounds[2], bounds[3], 0.])?;
                }
            }
            49 | 51 | 55 | 56 | 66 | 67 => {
                let inherited = matches!(command.code, 56 | 67);
                let bound = self
                    .data_services
                    .planes
                    .planes
                    .get(&id)
                    .is_some_and(|state| state.parent_offset.is_some());
                if node.initialized && (!inherited || bound) {
                    if matches!(command.code, 49 | 51) && command.data[0] == 0 && bound {
                        self.rebind_plane_parent(id)?;
                    } else {
                        let values = read_vector_buffer(&buffers.data1)?;
                        let state = self
                            .data_services
                            .planes
                            .planes
                            .entry(id)
                            .or_insert_with(|| PlaneState::from_node(&node));
                        let selector = if matches!(command.code, 66 | 67) {
                            0
                        } else {
                            command.data[0]
                        };
                        let geometry = state.geometry_mut(selector);
                        match command.code {
                            49 => {
                                if geometry.base != values {
                                    geometry.base = values;
                                }
                            }
                            51 => {
                                if geometry.position != values {
                                    geometry.position = values;
                                }
                            }
                            55 | 56 => {
                                if geometry.rotation != values {
                                    geometry.rotation = values;
                                }
                                geometry.matrix = None;
                            }
                            66 | 67 => {
                                if geometry.scale != values {
                                    geometry.scale = values;
                                }
                            }
                            _ => {}
                        }
                        let position = state.wrapper.position;
                        let scale = state.wrapper.scale;
                        state.input_geometry_dirty = true;
                        if let Some(node) = self.scene_nodes.get_mut(&id) {
                            node.position = position;
                            node.scale = scale;
                        }
                        if command.code == 49 {
                            self.data_services.planes.order_dirty = true;
                        }
                    }
                }
                self.refresh_plane_input(id)?;
                propagate = Some(match command.code {
                    49 => 50,
                    51 => 52,
                    55 | 56 => 56,
                    _ => 67,
                });
            }
            50 | 52 => {
                if command.data[0] == 0 {
                    self.rebind_plane_parent(id)?;
                    self.refresh_plane_input(id)?;
                }
                propagate = Some(command.code);
            }
            53 => {
                let offset = read_vector_buffer(&buffers.data1)?;
                self.data_services
                    .planes
                    .planes
                    .entry(id)
                    .or_insert_with(|| PlaneState::from_node(&node))
                    .parent_offset = Some(offset);
                self.rebind_plane_parent(id)?;
            }
            54 => {
                self.data_services
                    .planes
                    .planes
                    .entry(id)
                    .or_insert_with(|| PlaneState::from_node(&node))
                    .parent_offset = None;
            }
            57 | 58 => {
                let bound = self
                    .data_services
                    .planes
                    .planes
                    .get(&id)
                    .is_some_and(|state| state.parent_offset.is_some());
                if node.initialized && (command.code == 57 || bound) {
                    let matrix = read_matrix_buffer(&buffers.data1)?;
                    self.data_services
                        .planes
                        .set_matrix(&node, command.data[0], matrix);
                }
                self.refresh_plane_input(id)?;
                propagate = Some(58);
            }
            47 | 48 => {
                let parent_display = self
                    .scene_nodes
                    .get(&node.parent)
                    .filter(|parent| parent.initialized)
                    .map(|parent| {
                        self.data_services
                            .planes
                            .planes
                            .get(&parent.id)
                            .map_or(u32::from(parent.visible), |state| state.display)
                    });
                let state = self
                    .data_services
                    .planes
                    .planes
                    .entry(id)
                    .or_insert_with(|| PlaneState::from_node(&node));
                if command.code == 47 {
                    state.show = command.data[1];
                }
                let incoming = if command.code == 47 && state.parent_offset.is_some() {
                    parent_display
                } else {
                    Some(command.data[1])
                };
                if let Some(incoming) = incoming {
                    state.display = if state.show == 0 { 0 } else { incoming };
                    if let Some(node) = self.scene_nodes.get_mut(&id) {
                        node.visible = state.display != 0;
                    }
                    if state.input_configured
                        && (command.code == 48 || state.parent_offset.is_some())
                    {
                        for region in self
                            .input_task
                            .regions
                            .iter_mut()
                            .filter(|region| region.id() == id)
                        {
                            region.words[8] = state.display;
                            if state.display == 0 {
                                region.words[7] = 0;
                            }
                        }
                        self.mark_input_geometry_dirty();
                    }
                }
                propagate = Some(48);
            }
            59 | 63 | 81 | 82 | 89 => {
                if node.initialized {
                    let state = self
                        .data_services
                        .planes
                        .planes
                        .entry(id)
                        .or_insert_with(|| PlaneState::from_node(&node));
                    match command.code {
                        59 => {
                            state.color = command.data[1];
                            state.color_enabled = command.data[0];
                        }
                        63 => {
                            state.color =
                                (state.color & 0x00ff_ffff) | ((command.data[1] & 255) << 24);
                            state.color_enabled = 1;
                        }
                        81 => state.blend = BlendMode::from_raw(command.data[1]),
                        82 => state.perspective_origin = command.data[1],
                        89 => {
                            state.clip = if command.data[1] == 0 && buffers.data1.is_none() {
                                None
                            } else {
                                let bytes = require_buffer(&buffers.data1, 16)?;
                                let values = [
                                    word(bytes, 0)? as i32,
                                    word(bytes, 4)? as i32,
                                    word(bytes, 8)? as i32,
                                    word(bytes, 12)? as i32,
                                ];
                                Some(Rect::new(
                                    values[0],
                                    values[1],
                                    values[2].wrapping_sub(values[0]).max(0) as u32,
                                    values[3].wrapping_sub(values[1]).max(0) as u32,
                                ))
                            };
                        }
                        _ => {}
                    }
                    if let Some(node) = self.scene_nodes.get_mut(&id) {
                        node.overlay_color = if state.color_enabled == 0 {
                            0
                        } else {
                            state.color
                        };
                    }
                }
            }
            591 => {
                let camera_target = command.data[1];
                let valid = self
                    .scene_nodes
                    .get(&camera_target)
                    .is_some_and(|camera| camera.kind == 65)
                    && !self
                        .data_services
                        .tasks
                        .message_task_state(camera_target)
                        .is_some_and(|(_, deletion, _)| deletion != 0);
                let camera = if valid {
                    self.movie_host
                        .as_deref_mut()
                        .ok_or_else(|| error("camera query needs the registered camera service"))?
                        .planes
                        .cameras
                        .entry(camera_target)
                        .or_default();
                    camera_target
                } else {
                    0
                };
                if node.initialized {
                    self.data_services
                        .planes
                        .planes
                        .entry(id)
                        .or_insert_with(|| PlaneState::from_node(&node))
                        .camera = camera;
                }
            }
            592 => {
                if let Some(state) = self.data_services.planes.planes.get_mut(&id) {
                    state.camera = 0;
                }
            }
            _ => return Ok(false),
        }
        if matches!(command.code, 47 | 48 | 81) {
            self.synchronize_plane_history_control(id, command.code)?;
        }
        if let Some(code) = propagate {
            let mut cursor = None;
            loop {
                let child = self
                    .task_parents
                    .iter()
                    .filter(|(child, parent)| {
                        **parent == id && cursor.is_none_or(|cursor| (**child as i32) > cursor)
                    })
                    .map(|(&child, _)| child)
                    .min_by_key(|&child| child as i32);
                let Some(child) = child else { break };
                cursor = Some(child as i32);
                let forwarded = TaskCommand {
                    target: child as i32,
                    sender: id as i32,
                    code,
                    ..command
                };
                if !self.plane_command_inner(forwarded, buffers, output, depth + 1)? {
                    return Err(error(
                        "plane propagation needs a receiver for its non-plane child",
                    ));
                }
            }
        }
        Ok(true)
    }

    fn create_plane_grid(&mut self, id: u32, record: [u32; 10]) -> Result<()> {
        self.data_services.planes.invalidate_history_geometry(id);
        let old = self.data_services.planes.planes.remove(&id);
        let mut state = old
            .clone()
            .unwrap_or_else(|| PlaneState::from_node(&self.scene_nodes[&id]));
        let create = state.create;
        self.scene_nodes.get_mut(&id).unwrap().initialized = false;
        if create != 1 {
            self.data_services.planes.planes.insert(id, state);
            return Ok(());
        }
        state.wrapper = Geometry::default();
        state.source = Geometry::default();
        state.shape = PlaneShape::Empty;
        state.extent = [0.; 2];
        state.display = 0;
        state.color = u32::MAX;
        state.color_enabled = 0;
        state.camera = 0;
        state.blend = BlendMode::SourceOver;
        state.perspective_origin = 0;
        state.clip = None;
        self.data_services.planes.planes.insert(id, state.clone());
        let node = self.scene_nodes.get_mut(&id).unwrap();
        node.initialized = true;
        node.visible = false;
        node.size = [0.; 2];
        node.position = [0.; 3];
        node.scale = [1.; 3];
        node.color = u32::MAX;
        self.data_services.planes.register_drawable(id);
        let columns = record[8] as i32;
        let rows = record[9] as i32;
        let count = u64::from(columns.max(0) as u32) * u64::from(rows.max(0) as u32);
        if count > 65_536 {
            return Err(error("plane grid exceeds 65536 child tasks"));
        }
        let [width, height] = [f32::from_bits(record[3]), f32::from_bits(record[4])];
        let origin: [f32; 3] = std::array::from_fn(|axis| f32::from_bits(record[axis]));
        for row in 0..rows {
            for column in 0..columns {
                let child = self
                    .create_task_object(2, id, b"child plane", b"")
                    .map_err(|_| error("plane grid child creation failed"))?;
                let node = self
                    .scene_nodes
                    .get_mut(&child)
                    .ok_or_else(|| error("plane grid child disappeared"))?;
                node.initialized = false;
                self.canvas
                    .as_ref()
                    .ok_or_else(|| error("plane grid needs the actual canvas layout"))?;
                let mut cell = PlaneState::from_node(node);
                cell.wrapper = Geometry::default();
                cell.source = Geometry::default();
                cell.shape = PlaneShape::Canvas;
                cell.extent = [width, height];
                cell.wrapper.position = [
                    column as f32 * width + origin[0],
                    row as f32 * height + origin[1],
                    origin[2],
                ];
                cell.display = 0;
                node.position = cell.wrapper.position;
                node.scale = [1.; 3];
                node.size = [width.abs(), height.abs()];
                node.color = u32::MAX;
                node.visible = false;
                node.initialized = true;
                self.data_services.planes.planes.insert(child, cell);
                self.data_services.planes.register_drawable(child);
            }
        }
        self.data_services.planes.grid_records.insert(id, record);
        Ok(())
    }

    fn rebind_plane_parent(&mut self, id: u32) -> Result<()> {
        let Some(node) = self.scene_nodes.get(&id).cloned() else {
            return Ok(());
        };
        let Some(offset) = self
            .data_services
            .planes
            .planes
            .get(&id)
            .and_then(|state| state.parent_offset)
        else {
            return Ok(());
        };
        let parent_id = self.task_parents.get(&id).copied().unwrap_or(node.parent);
        let parent = self.live_plane_node(parent_id);
        let (position, base) = match parent {
            None => ([0.; 3], [0.; 3]),
            Some(parent) => {
                let base = self
                    .data_services
                    .planes
                    .planes
                    .get(&parent_id)
                    .map(|state| state.wrapper.base)
                    .or_else(|| {
                        self.movie_host.as_ref().and_then(|host| {
                            host.planes
                                .properties
                                .get(&parent_id)
                                .map(|properties| properties.base)
                        })
                    })
                    .unwrap_or([0., 0., 5000.]);
                (parent.position, base)
            }
        };
        if node.initialized {
            if let Some(state) = self.data_services.planes.planes.get_mut(&id) {
                state.wrapper.base = [
                    base[0] - offset[0],
                    base[1] - offset[1],
                    offset[2] + base[2],
                ];
                state.wrapper.position = position;
            }
            if let Some(node) = self.scene_nodes.get_mut(&id) {
                node.position = position;
            }
            self.data_services.planes.order_dirty = true;
        }
        Ok(())
    }

    fn live_plane_node(&self, id: u32) -> Option<SceneNode> {
        if self
            .data_services
            .tasks
            .message_task_state(id)
            .is_some_and(|(_, deletion, _)| deletion != 0)
        {
            return None;
        }
        self.scene_nodes
            .get(&id)
            .filter(|node| node.kind == 2 && node.initialized)
            .cloned()
    }

    fn register_plane_input(&mut self, id: u32, backup: u32) -> Result<()> {
        if self
            .data_services
            .tasks
            .message_task_state(id)
            .is_some_and(|(_, deleted, _)| deleted != 0)
        {
            return Ok(());
        }
        let Some(node) = self.live_plane_node(id) else {
            if self.scene_nodes.contains_key(&id) {
                return Err(error("native 95 requires an existing plane drawable"));
            }
            return Ok(());
        };
        let state = self
            .data_services
            .planes
            .planes
            .entry(id)
            .or_insert_with(|| PlaneState::from_node(&node));
        if state.receiver.is_some() {
            return Ok(());
        }
        if !self.input_task.alive {
            return Err(error("native 95 requires the root input service"));
        }
        if self.input_task.regions.len() >= 1_000_000 {
            return Err(error("native 95 input region count exceeds 1000000"));
        }
        let mut bounds = input_bounds(&node, state);
        if let Some(canvas) = &self.canvas {
            if bounds[0] <= 0
                && bounds[1] <= 0
                && bounds[2] >= canvas.width() as i32
                && bounds[3] >= canvas.height() as i32
            {
                bounds = [-100000, -100000, 100000, 100000];
            }
        }
        let mut words = [0; 15];
        words[0] = id;
        words[1] = float_to_i32(state.wrapper.base[2]) as u32;
        words[2] = 7;
        words[3..7].copy_from_slice(&bounds.map(|value| value as u32));
        words[8] = 1;
        words[9] = 1;
        words[14] = 1;
        self.input_task.regions.push(InputRegionState { words });
        self.input_task
            .regions
            .sort_by_key(|region| std::cmp::Reverse(region.words[1] as i32));
        state.input_configured = true;
        state.receiver = Some(self.current_task_id);
        state.backup_configuration = backup;
        state.input_geometry_dirty = false;
        Ok(())
    }

    /// Existing 84/88/101/matrix adapters should call this after changing a
    /// plane. Geometry-based input rectangles intentionally ignore rotation.
    pub(super) fn refresh_plane_input(&mut self, id: u32) -> Result<()> {
        if self.data_services.planes.planes.get(&id)
            .is_some_and(|state| state.input_configured)
        {
            // Lower input130 marks even for equal bounds or a missing region.
            self.mark_input_geometry_dirty();
        }
        let Some(node) = self.live_plane_node(id) else {
            return Ok(());
        };
        let Some(state) = self.data_services.planes.planes.get_mut(&id) else {
            return Ok(());
        };
        if !state.input_configured {
            return Ok(());
        }
        let bounds = input_bounds(&node, state);
        if let Some(region) = self
            .input_task
            .regions
            .iter_mut()
            .find(|region| region.id() == id)
        {
            region.words[3..7].copy_from_slice(&bounds.map(|value| value as u32));
        }
        state.input_geometry_dirty = false;
        Ok(())
    }

    fn plane_wrapper_copy(&mut self, args: &[u8]) -> Result<NativeReturn> {
        let target_id = word(args, 0)?;
        if target_id == 0 {
            return Ok(NativeReturn::Continue);
        }
        let source_id = word(args, 20)?;
        if source_id == 0 {
            return Ok(NativeReturn::Continue);
        }
        let target = self
            .image_handles
            .get(&target_id)
            .ok_or_else(|| error("native 116 target is not an image wrapper"))?;
        let source = self
            .image_handles
            .get(&source_id)
            .ok_or_else(|| error("native 116 source is not an image wrapper"))?;
        let to = request_rect(args, 4)?;
        let from = request_rect(args, 24)?;
        let shared_pixels = target_id == source_id
            || self
                .data_services
                .image_wrapper_pixels
                .get(&target_id)
                .is_some_and(|pixel| {
                    self.data_services.image_wrapper_pixels.get(&source_id) == Some(pixel)
                });
        let updated = copy_wrapper(target, source, to, from, shared_pixels)?;
        if let Some(updated) = updated {
            self.replace_image_wrapper_pixels(target_id, updated)?;
        }
        Ok(NativeReturn::Continue)
    }
}

impl HeadlessPlayer {
    /// Call before logical hit testing, and after an observer when same-call
    /// input visibility is required. Only geometry actually dirtied is touched.
    pub(super) fn synchronize_plane_input_geometry(&mut self) {
        if flush_input_geometry(
            &mut self.data_services.planes,
            &self.scene_nodes,
            &mut self.input_task,
        ) {
            self.mark_host_input_geometry_dirty();
        }
    }

    /// Invoke after successfully issuing the old/new focus notifications but
    /// before assigning input_task.selected_region. Existing raw nonzero
    /// selection values on the newly selected region are retained.
    pub(super) fn synchronize_plane_pointer_selection(&mut self, selected: Option<u32>) {
        if selected == self.input_task.selected_region {
            return;
        }
        for region in &mut self.input_task.regions {
            if Some(region.id()) == selected {
                if region.words[7] == 0 {
                    region.words[7] = 1;
                }
            } else {
                region.words[7] = 0;
            }
        }
    }

    /// Replace the raw PlaneServices::release call in the actual task-release
    /// path; this also clears stale pointer identity after region removal.
    pub(super) fn release_plane_state(&mut self, id: u32) {
        self.release_plane_history(id);
        self.clear_kx_geometry(id);
        self.data_services.planes.release(id);
        raster::synchronize_movie_callbacks(&self.data_services.planes, &mut self.movie_host);
        self.input_task.regions.retain(|region| region.id() != id);
        if self.input_task.selected_region == Some(id) {
            self.input_task.selected_region = None;
        }
    }
}

fn flush_input_geometry(
    services: &mut PlaneServices,
    nodes: &BTreeMap<u32, SceneNode>,
    input: &mut DesktopInputTask,
) -> bool {
    let mut published = false;
    for (&id, state) in &mut services.planes {
        if !state.input_geometry_dirty {
            continue;
        }
        let Some(node) = nodes.get(&id).filter(|node| node.initialized) else {
            continue;
        };
        if state.input_configured {
            published = true;
            let bounds = input_bounds(node, state);
            if let Some(region) = input.regions.iter_mut().find(|region| region.id() == id) {
                region.words[3..7].copy_from_slice(&bounds.map(|value| value as u32));
            }
        }
        state.input_geometry_dirty = false;
    }
    published
}

fn input_bounds(node: &SceneNode, state: &PlaneState) -> [i32; 4] {
    let x = node.position[0] - state.wrapper.base[0];
    let y = node.position[1] - state.wrapper.base[1];
    [
        float_to_i32(x),
        float_to_i32(y),
        float_to_i32(x + node.size[0]),
        float_to_i32(y + node.size[1]),
    ]
}

/// One invocation per ordinary plane in the existing sorted pass. A movie must
/// be drawn by its owner first; this helper does not poll or own movie players.
pub(super) fn draw_plane(
    canvas: &mut Canvas,
    node: &SceneNode,
    services: &PlaneServices,
    canvas_source: Option<&RgbaFrame>,
) -> HostResult<bool> {
    draw_plane_with_view(
        canvas,
        node,
        services,
        canvas_source,
        &BTreeMap::new(),
        &MoviePlaneProjection::default(),
    )
}

pub(super) fn draw_plane_with_view(
    canvas: &mut Canvas,
    node: &SceneNode,
    services: &PlaneServices,
    canvas_source: Option<&RgbaFrame>,
    cameras: &BTreeMap<u32, PlaneCamera>,
    projection: &MoviePlaneProjection,
) -> HostResult<bool> {
    if crate::native_tasks::RenderTaskStore::supports_kind(node.kind) {
        // These owners draw through the single window composition callback.
        // Their scene metadata is not a second standalone primitive. Keeping
        // this concrete classification here also covers older sorted passes
        // that only skipped the original35..39 family themselves.
        return Ok(true);
    }
    if (node.kind == 25 && services.inert_media.contains(&node.id))
        || (node.kind == 18 && services.raster.owns(node.id))
    {
        return Ok(true);
    }
    if node.kind != 2 {
        return Ok(false);
    }
    let fallback;
    let state = match services.planes.get(&node.id) {
        Some(state) => state,
        None => {
            fallback = PlaneState::from_node(node);
            &fallback
        }
    };
    if !node.initialized || state.display == 0 || !node.visible || state.shape == PlaneShape::Empty
    {
        return Ok(true);
    }
    let source = if state.shape == PlaneShape::Canvas {
        Some(canvas_source.context("canvas-textured plane requires a pre-pass canvas snapshot")?)
    } else {
        None
    };
    let mut geometry = state.wrapper.clone();
    geometry.position = node.position;
    geometry.scale = node.scale;
    let matrix = geometry_matrix(&geometry);
    let [width, height] = state.extent;
    let corners = [[0., 0.], [width, 0.], [0., height], [width, height]];
    let mut vertices = [[0f64; 6]; 4];
    let default_camera = PlaneCamera::default();
    let camera = (state.camera != 0)
        .then(|| cameras.get(&state.camera))
        .flatten()
        .unwrap_or(&default_camera);
    let source_matrix = geometry_matrix(&state.source);
    for (index, corner) in corners.into_iter().enumerate() {
        let (local, uv) = if let Some(source) = source {
            // Prepare the canvas source before applying its wrapper. Source
            // rotation/scale changes the prepared polygon, not just its UVs.
            // Source translation moves sampling but is removed from output XY.
            let mut source_point = [
                    corner[0] - state.source.base[0],
                    corner[1] - state.source.base[1],
                    0.,
                    1.,
                ];
            if let Some(callback) = services.raster.callback(node.id, true) {
                callback.apply(&mut source_point)?;
            }
            let prepared = transform(source_point, source_matrix);
            let sampled = [
                prepared[0] + state.source.position[0],
                prepared[1] + state.source.position[1],
            ];
            let offset = source.metadata().offset;
            (
                [
                    sampled[0] - state.source.position[0],
                    sampled[1] - state.source.position[1],
                ],
                [sampled[0] - offset[0] as f32, sampled[1] - offset[1] as f32],
            )
        } else {
            (corner, [0.; 2])
        };
        let mut local_point = [
                local[0] - geometry.base[0],
                local[1] - geometry.base[1],
                0.,
                1.,
            ];
        if let Some(callback) = services.raster.callback(node.id, false) {
            callback.apply(&mut local_point)?;
        }
        let mut point = transform(local_point, matrix);
        for axis in 0..3 {
            point[axis] += geometry.position[axis];
        }
        point[3] += 1.;
        let point = transform(point, projection.parent);
        let projected = project_plane(
            point,
            &geometry,
            state.perspective_origin,
            camera,
            projection,
        );
        vertices[index] = [
            f64::from(projected[0]),
            f64::from(projected[1]),
            f64::from(projected[2]),
            f64::from(projected[3]),
            f64::from(uv[0]) * f64::from(projected[3]),
            f64::from(uv[1]) * f64::from(projected[3]),
        ];
    }
    if vertices.iter().flatten().any(|value| !value.is_finite()) {
        bail!("ordinary plane projection contains non-finite coordinates");
    }
    let color = if state.color_enabled != 0 {
        state.color
    } else {
        node.color
    };
    if let Some(source) = source
        && matches!(state.blend, BlendMode::SourceOver | BlendMode::Multiply)
        && color & 0x00ff_ffff == 0x00ff_ffff
        && draw_unit_canvas_plane(
            canvas, source, services.vram_overflow.as_ref(), vertices, state.clip, (color >> 24) as u8,
            state.blend,
        )
    {
        return Ok(true);
    }
    for indices in [[0, 1, 2], [2, 1, 3]] {
        let polygon = indices.map(|index| vertices[index]).to_vec();
        let polygon = clip_plane_depth(&clip_plane_depth(&polygon, 0., true), 1., false);
        for index in 1..polygon.len().saturating_sub(1) {
            raster_triangle(
                canvas,
                [polygon[0], polygon[index], polygon[index + 1]],
                color,
                source,
                services.vram_overflow.as_ref(),
                state.clip,
                state.blend,
            )?;
        }
    }
    Ok(true)
}

/// Supply this depth to the common scene sort; translation.z is not the plane
/// drawing priority. Default depth is 5000 and is not converted to an integer.
pub(super) fn plane_depth(node: &SceneNode, services: &PlaneServices) -> f32 {
    if node.kind != 2 {
        return node.position[2];
    }
    services
        .planes
        .get(&node.id)
        .map_or(5000., |state| state.wrapper.base[2])
}

pub(super) fn fes_rotation_matrix(rotation: [f32; 3]) -> [f32; 16] {
    geometry_matrix(&Geometry { rotation, matrix: None, ..Geometry::default() })
}

fn geometry_matrix(geometry: &Geometry) -> [f32; 16] {
    let mut scale = IDENTITY;
    for axis in 0..3 {
        scale[axis * 5] = geometry.scale[axis];
    }
    let mut output = multiply(IDENTITY, scale);
    if let Some(explicit) = geometry.matrix {
        return multiply(output, explicit);
    }
    for axis in 0..3 {
        let (sine, cosine) = geometry.rotation[axis].sin_cos();
        let a = (axis + 1) % 3;
        let b = (axis + 2) % 3;
        let mut rotation = IDENTITY;
        rotation[a * 4 + a] = cosine;
        rotation[b * 4 + b] = cosine;
        rotation[a * 4 + b] = sine;
        rotation[b * 4 + a] = -sine;
        output = multiply(output, rotation);
    }
    output
}

fn multiply(left: [f32; 16], right: [f32; 16]) -> [f32; 16] {
    std::array::from_fn(|index| {
        (0..4).fold(0., |sum, inner| {
            sum + left[(index / 4) * 4 + inner] * right[inner * 4 + index % 4]
        })
    })
}

fn transform(vector: [f32; 4], matrix: [f32; 16]) -> [f32; 4] {
    std::array::from_fn(|column| {
        (0..4).fold(0., |sum, row| sum + vector[row] * matrix[row * 4 + column])
    })
}

fn project_plane(
    mut point: [f32; 4],
    geometry: &Geometry,
    origin_mode: u32,
    camera: &PlaneCamera,
    view: &MoviePlaneProjection,
) -> [f32; 4] {
    let perspective = view.perspective && point[2] != 0.;
    if perspective {
        let distance = (65000. - geometry.base[2]) - view.camera_depth;
        let denominator = if point[2] == distance {
            1.
        } else {
            point[2] - distance
        };
        let ratio = (view.camera_depth / denominator).max(0.);
        let origin = match origin_mode {
            0 => [geometry.position[0], geometry.position[1]],
            1 => view.center,
            _ => [0.; 2],
        };
        point = [
            (point[0] - origin[0]) * ratio + origin[0],
            (point[1] - origin[1]) * ratio + origin[1],
            point[2] * (1. / 65000.),
            1. / denominator,
        ];
    } else {
        point[2] = (65000. - (point[2] + geometry.base[2])) * (1. / 65000.);
    }
    let depth = point[2];
    point[0] = (point[0] - view.center[0]) * camera.scale[0];
    point[1] = (point[1] - view.center[1]) * camera.scale[1];
    point = transform(point, camera.matrix);
    point[0] += view.center[0] - camera.offset[0];
    point[1] += view.center[1] - camera.offset[1];
    if !perspective {
        point[2] = depth;
    }
    point = transform(point, view.matrix);
    point[0] += view.translation[0];
    point[1] += view.translation[1];
    point
}

fn clip_plane_depth(input: &[[f64; 6]], edge: f64, lower: bool) -> Vec<[f64; 6]> {
    let mut output = Vec::new();
    let Some(mut previous) = input.last().copied() else {
        return output;
    };
    let inside = |vertex: [f64; 6]| {
        if lower {
            vertex[2] >= edge
        } else {
            vertex[2] <= edge
        }
    };
    for &current in input {
        if inside(previous) != inside(current) {
            let weight = (edge - previous[2]) / (current[2] - previous[2]);
            output.push(std::array::from_fn(|axis| {
                previous[axis] + (current[axis] - previous[axis]) * weight
            }));
        }
        if inside(current) {
            output.push(current);
        }
        previous = current;
    }
    output
}

fn draw_unit_canvas_plane(
    canvas: &mut Canvas,
    source: &RgbaFrame,
    overflow: Option<&VramOverflow>,
    vertices: [[f64; 6]; 4],
    clip: Option<Rect>,
    opacity: u8,
    blend: BlendMode,
) -> bool {
    let [origin, right, bottom, opposite] = vertices;
    let reciprocal_w = origin[3];
    if !matches!(reciprocal_w, 1. | 2. | 4.)
        || vertices.iter().any(|point| point[3] != reciprocal_w || !(0. ..=1.).contains(&point[2]))
    {
        return false;
    }
    let left = origin[0];
    let top = origin[1];
    let end_x = right[0];
    let end_y = bottom[1];
    let source_x = origin[4] / reciprocal_w;
    let source_y = origin[5] / reciprocal_w;
    if [left, top, end_x, end_y, source_x, source_y]
        .iter().any(|value| value.abs() > 32768. || value.fract() != 0.)
        || left >= end_x || top >= end_y
    {
        return false;
    }
    for (point, [x, y]) in [right, bottom, opposite]
        .into_iter().zip([[end_x, top], [left, end_y], [end_x, end_y]])
    {
        if point[0] != x || point[1] != y
            || point[4] != (source_x + x - left) * reciprocal_w
            || point[5] != (source_y + y - top) * reciprocal_w
        {
            return false;
        }
    }
    let mut x0 = (left as i64).max(0);
    let mut y0 = (top as i64).max(0);
    let mut x1 = (end_x as i64).min(i64::from(canvas.width()));
    let mut y1 = (end_y as i64).min(i64::from(canvas.height()));
    if let Some(clip) = clip {
        x0 = x0.max(i64::from(clip.x));
        y0 = y0.max(i64::from(clip.y));
        x1 = x1.min(i64::from(clip.x) + i64::from(clip.width));
        y1 = y1.min(i64::from(clip.y) + i64::from(clip.height));
    }
    if x0 >= x1 || y0 >= y1 || opacity == 0 {
        return true;
    }
    let source_x = source_x as i64 + x0 - left as i64;
    let source_y = source_y as i64 + y0 - top as i64;
    if source_x < 0 || source_y < 0
        || (overflow.is_none() && (source_x + x1 - x0 > i64::from(source.width())
            || source_y + y1 - y0 > i64::from(source.height())))
    {
        return false;
    }

    let source_stride = source.width() as usize * 4;
    let canvas_stride = canvas.width() as usize * 4;
    let width = (x1 - x0) as u32;
    let row_bytes = width as usize * 4;
    let pixels = canvas.pixels_mut();
    for row in 0..(y1 - y0) as u32 {
        let sample_y = source_y as u32 + row;
        let to = (y0 as usize + row as usize) * canvas_stride + x0 as usize * 4;
        let output = &mut pixels[to..to + row_bytes];
        let mut consumed = 0;
        while consumed < width {
            let sample_x = source_x as u32 + consumed;
            let remaining = width - consumed;
            let (count, input) = if sample_y < source.height() && sample_x < source.width() {
                let count = remaining.min(source.width() - sample_x);
                let from = sample_y as usize * source_stride + sample_x as usize * 4;
                (count, Some(&source.pixels()[from..from + count as usize * 4]))
            } else if let Some(overflow) = overflow {
                overflow.row_span(sample_x, sample_y, remaining)
            } else {
                break;
            };
            if let Some(input) = input {
                let start = consumed as usize * 4;
                let target = &mut output[start..start + input.len()];
                if blend == BlendMode::Multiply {
                    multiply_unit_plane_span(input, target, opacity);
                } else {
                    blend_unit_plane_span(input, target, opacity);
                }
            }
            consumed += count;
        }
    }
    true
}

fn blend_unit_plane_span(input: &[u8], output: &mut [u8], opacity: u8) {
    if opacity == 255 && input.chunks_exact(4).all(|pixel| pixel[3] == 255) {
        output.copy_from_slice(input);
        return;
    }
    for (pixel, target) in input.chunks_exact(4).zip(output.chunks_exact_mut(4)) {
        let alpha = (u16::from(pixel[3]) * u16::from(opacity) / 255) as u8;
        if alpha != 0 {
            super::list_native::blend_list_pixel(target, [pixel[0], pixel[1], pixel[2], alpha], 255);
        }
    }
}

fn multiply_unit_plane_span(input: &[u8], output: &mut [u8], opacity: u8) {
    for (source, target) in input.chunks_exact(4).zip(output.chunks_exact_mut(4)) {
        let alpha = (u16::from(source[3]) * u16::from(opacity) / 255) as u8;
        if alpha != 0 {
            for (channel, sample) in target.iter_mut().zip([source[0], source[1], source[2], alpha]) {
                *channel = ((u16::from(*channel) * u16::from(sample) + 127) / 255) as u8;
            }
        }
    }
}

fn raster_triangle(
    canvas: &mut Canvas,
    mut v: [[f64; 6]; 3],
    argb: u32,
    source: Option<&RgbaFrame>,
    overflow: Option<&VramOverflow>,
    clip: Option<Rect>,
    blend: BlendMode,
) -> HostResult<()> {
    let edge = |a: [f64; 6], b: [f64; 6], x: f64, y: f64| {
        (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])
    };
    let mut area = edge(v[0], v[1], v[2][0], v[2][1]);
    if area == 0. {
        return Ok(());
    }
    if v.iter()
        .any(|vertex| vertex[3] <= 0. || !vertex[3].is_finite())
    {
        bail!("visible plane triangle has a non-positive reciprocal W");
    }
    if area < 0. {
        v.swap(1, 2);
        area = -area;
    }
    let mut min_x = v
        .iter()
        .map(|p| p[0])
        .fold(f64::INFINITY, f64::min)
        .floor()
        .max(0.) as u32;
    let mut min_y = v
        .iter()
        .map(|p| p[1])
        .fold(f64::INFINITY, f64::min)
        .floor()
        .max(0.) as u32;
    let mut max_x = v
        .iter()
        .map(|p| p[0])
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil()
        .max(0.)
        .min(f64::from(canvas.width())) as u32;
    let mut max_y = v
        .iter()
        .map(|p| p[1])
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil()
        .max(0.)
        .min(f64::from(canvas.height())) as u32;
    if let Some(clip) = clip {
        min_x = min_x.max(clip.x.max(0) as u32);
        min_y = min_y.max(clip.y.max(0) as u32);
        max_x = max_x.min(
            (i64::from(clip.x) + i64::from(clip.width)).clamp(0, i64::from(canvas.width())) as u32,
        );
        max_y = max_y.min(
            (i64::from(clip.y) + i64::from(clip.height)).clamp(0, i64::from(canvas.height()))
                as u32,
        );
    }
    let top_left = |a: [f64; 6], b: [f64; 6]| b[1] < a[1] || b[1] == a[1] && b[0] > a[0];
    for y in min_y..max_y {
        for x in min_x..max_x {
            let p = [f64::from(x) + 0.5, f64::from(y) + 0.5];
            let weights = [
                edge(v[1], v[2], p[0], p[1]),
                edge(v[2], v[0], p[0], p[1]),
                edge(v[0], v[1], p[0], p[1]),
            ];
            if weights.iter().enumerate().any(|(i, &value)| {
                value < 0. || value == 0. && !top_left(v[(i + 1) % 3], v[(i + 2) % 3])
            }) {
                continue;
            }
            let mut color = Color::rgba(
                (argb >> 16) as u8,
                (argb >> 8) as u8,
                argb as u8,
                (argb >> 24) as u8,
            );
            if let Some(source) = source {
                let rhw: f64 = (0..3).map(|i| weights[i] * v[i][3] / area).sum();
                let uv: [f64; 2] = std::array::from_fn(|axis| {
                    (0..3)
                        .map(|i| weights[i] * v[i][axis + 4] / area)
                        .sum::<f64>()
                        / rhw
                });
                let sx = uv[0].floor() as i64;
                let sy = uv[1].floor() as i64;
                let (Ok(sx), Ok(sy)) = (u32::try_from(sx), u32::try_from(sy)) else {
                    continue;
                };
                let pixel = if sx < source.width() && sy < source.height() {
                    let offset = (sy as usize * source.width() as usize + sx as usize) * 4;
                    let bytes = &source.pixels()[offset..offset + 4];
                    Color::rgba(bytes[0], bytes[1], bytes[2], bytes[3])
                } else if let Some(pixel) = overflow.and_then(|atlas| atlas.pixel(sx, sy)) {
                    pixel
                } else {
                    continue;
                };
                color = Color::rgba(
                    (u16::from(pixel.r) * u16::from(color.r) / 255) as u8,
                    (u16::from(pixel.g) * u16::from(color.g) / 255) as u8,
                    (u16::from(pixel.b) * u16::from(color.b) / 255) as u8,
                    (u16::from(pixel.a) * u16::from(color.a) / 255) as u8,
                );
            }
            canvas.blend_pixel_with_mode(x as i32, y as i32, color, blend)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod unit_canvas_plane_tests {
    use super::*;

    #[test]
    fn unit_canvas_plane_matches_triangle_pixels() -> HostResult<()> {
        let width = 11;
        let height = 9;
        let viewport = [0., 0., 0.25, 2.];
        let vertices = [
            [-2., -1., viewport[2], viewport[3], 2., 0.],
            [7., -1., viewport[2], viewport[3], 20., 0.],
            [-2., 7., viewport[2], viewport[3], 2., 16.],
            [7., 7., viewport[2], viewport[3], 20., 16.],
        ];
        let samples = |seed: u32| {
            (0..width * height).flat_map(move |index| {
                let value = (index * 31 + seed) as u8;
                [value, value.rotate_left(3), value ^ 67, [0, 1, 127, 254, 255][(index % 5) as usize]]
            })
        };
        let background = Canvas::from_rgba8(width, height, samples(12).collect())?;
        for make_opaque in [false, true] {
            let mut rgba: Vec<_> = samples(53).collect();
            if make_opaque {
                rgba.iter_mut().skip(3).step_by(4).for_each(|channel| *channel = 255);
            }
            let source = RgbaFrame::from_rgba8(width, height, rgba)?;
            for opacity in [0, 1, 127, 217, 255] {
                for clip in [None, Some(Rect::new(-3, 2, 7, 6)), Some(Rect::new(8, 8, 2, 2))] {
                    for blend in [BlendMode::SourceOver, BlendMode::Multiply] {
                        let mut reference = background.clone();
                        for indices in [[0, 1, 2], [2, 1, 3]] {
                            raster_triangle(
                                &mut reference, indices.map(|index| vertices[index]),
                                0x00ff_ffff | (u32::from(opacity) << 24), Some(&source), None,
                                clip, blend,
                            )?;
                        }
                        let mut actual = background.clone();
                        assert!(draw_unit_canvas_plane(&mut actual, &source, None, vertices, clip, opacity, blend));
                        assert_eq!(actual.pixels(), reference.pixels(), "{blend:?} {opacity} {clip:?}");
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn unit_canvas_plane_requires_integral_unclipped_unit_mapping() -> HostResult<()> {
        let mut canvas = Canvas::new(6, 6)?;
        let source = RgbaFrame::from_rgba8(3, 3, vec![255; 36])?;
        let vertices = [
            [0., 0., 0.5, 2., 0., 0.],
            [3., 0., 0.5, 2., 6., 0.],
            [0., 3., 0.5, 2., 0., 6.],
            [3., 3., 0.5, 2., 6., 6.],
        ];
        for (vertex, field, value) in [
            (0, 0, 0.25), (1, 4, 4.), (2, 2, -0.25), (3, 3, 1.), (3, 0, 2.),
        ] {
            let mut malformed = vertices;
            malformed[vertex][field] = value;
            assert!(!draw_unit_canvas_plane(&mut canvas, &source, None, malformed, None, 255, BlendMode::SourceOver));
        }
        let mut outside_source = vertices;
        outside_source.iter_mut().for_each(|vertex| vertex[4] += 2.);
        assert!(!draw_unit_canvas_plane(&mut canvas, &source, None, outside_source, None, 255, BlendMode::SourceOver));
        assert!(canvas.pixels().iter().all(|&channel| channel == 0));
        Ok(())
    }
}

/// FES list images use the same textured rasterizer as ordinary planes. The
/// source clip is applied before the plane transform; caller-side VIEWPORT
/// clipping is applied to the resulting list projection.
pub(super) fn draw_fes_motion_frame(
    canvas: &mut Canvas,
    frame: &RgbaFrame,
    options: &cs2_renderer::ImageDrawOptions,
    image_words: Option<&[u32; 36]>,
    plane_words: Option<&[u32; 36]>,
    plane_origin: Option<[f32; 2]>,
) -> HostResult<()> {
    if !options.visible {
        return Ok(());
    }
    let _timing = super::measure_script_detail(super::HostPhase::ScriptMotionRaster);
    let correction = image_words.map_or([0.; 2], |words| {
        [0, 1].map(|axis| {
            let value = f32::from_bits(words[6 + axis]);
            value - float_to_i32(value) as f32
        })
    });
    let transform_2d = options.transform();
    let mut polygon: Vec<[f64; 6]> = [
        [0., 0.],
        [frame.width() as f32, 0.],
        [frame.width() as f32, frame.height() as f32],
        [0., frame.height() as f32],
    ]
    .into_iter()
    .map(|uv| {
        let position = transform_2d.map_source_point(uv);
        [
            f64::from(position[0] + correction[0]),
            f64::from(position[1] + correction[1]),
            0.,
            1.,
            f64::from(uv[0]),
            f64::from(uv[1]),
        ]
    })
    .collect();
    if polygon.iter().flatten().any(|value| !value.is_finite()) {
        bail!("FES image motion contains a non-finite transform");
    }
    if let Some(clip) = options.clip {
        for (axis, edge, lower) in [
            (0, f64::from(clip.x), true),
            (1, f64::from(clip.y), true),
            (0, i64::from(clip.x) as f64 + f64::from(clip.width), false),
            (1, i64::from(clip.y) as f64 + f64::from(clip.height), false),
        ] {
            let mut output = Vec::new();
            if let Some(mut previous) = polygon.last().copied() {
                let inside = |vertex: [f64; 6]| {
                    if lower {
                        vertex[axis] >= edge
                    } else {
                        vertex[axis] <= edge
                    }
                };
                for &current in &polygon {
                    if inside(previous) != inside(current) {
                        let weight = (edge - previous[axis]) / (current[axis] - previous[axis]);
                        output.push(std::array::from_fn(|i| {
                            previous[i] + (current[i] - previous[i]) * weight
                        }));
                    }
                    if inside(current) {
                        output.push(current);
                    }
                    previous = current;
                }
            }
            polygon = output;
        }
    }
    let mut rgb = 0x00ff_ffff;
    if let Some(words) = plane_words {
        if words[35] == 0 {
            return Ok(());
        }
        let geometry = Geometry {
            base: std::array::from_fn(|axis| f32::from_bits(words[3 + axis])),
            position: std::array::from_fn(|axis| f32::from_bits(words[6 + axis])),
            rotation: [0.; 3],
            scale: std::array::from_fn(|axis| f32::from_bits(words[25 + axis])),
            matrix: Some(std::array::from_fn(|index| {
                f32::from_bits(words[9 + index])
            })),
        };
        let matrix = geometry_matrix(&geometry);
        let origin = plane_origin.unwrap_or([geometry.position[0], geometry.position[1]]);
        let camera = PlaneCamera::default();
        let view = MoviePlaneProjection::default();
        for vertex in &mut polygon {
            let mut point = transform(
                [
                    vertex[0] as f32 - float_to_i32(origin[0]) as f32 - geometry.base[0],
                    vertex[1] as f32 - float_to_i32(origin[1]) as f32 - geometry.base[1],
                    0.,
                    1.,
                ],
                matrix,
            );
            for axis in 0..3 {
                point[axis] += geometry.position[axis];
            }
            point[3] += 1.;
            let projected = project_plane(point, &geometry, 0, &camera, &view);
            vertex[0] = f64::from(projected[0]);
            vertex[1] = f64::from(projected[1]);
            vertex[2] = f64::from(projected[2]);
            vertex[3] = f64::from(projected[3]);
            vertex[4] *= vertex[3];
            vertex[5] *= vertex[3];
        }
        rgb = words[28] & 0x00ff_ffff;
        polygon = clip_plane_depth(&clip_plane_depth(&polygon, 0., true), 1., false);
    }
    if polygon.iter().flatten().any(|value| !value.is_finite()) {
        bail!("FES plane motion matrix produced non-finite vertices");
    }
    let color = (u32::from(options.opacity) << 24) | rgb;
    for index in 1..polygon.len().saturating_sub(1) {
        raster_triangle(
            canvas,
            [polygon[0], polygon[index], polygon[index + 1]],
            color,
            Some(frame),
            None,
            None,
            options.blend_mode,
        )?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) struct CopyRect {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: i32,
    pub(super) height: i32,
}

fn request_rect(args: &[u8], offset: usize) -> Result<CopyRect> {
    Ok(CopyRect {
        x: word(args, offset)? as i32,
        y: word(args, offset + 4)? as i32,
        width: word(args, offset + 8)? as i32,
        height: word(args, offset + 12)? as i32,
    })
}

pub(super) fn copy_wrapper(
    target: &RgbaFrame,
    source: &RgbaFrame,
    to: CopyRect,
    from: CopyRect,
    alias: bool,
) -> Result<Option<RgbaFrame>> {
    let local = |request: CopyRect, added: [i32; 2], frame: &RgbaFrame| -> Option<[i32; 4]> {
        let offset = frame.metadata().offset;
        let x = request.x.wrapping_add(added[0]);
        let y = request.y.wrapping_add(added[1]);
        let right = x.wrapping_add(request.width);
        let bottom = y.wrapping_add(request.height);
        let left = x.max(offset[0]);
        let top = y.max(offset[1]);
        let right = right.min(offset[0].wrapping_add(frame.width() as i32));
        let bottom = bottom.min(offset[1].wrapping_add(frame.height() as i32));
        if left >= right || top >= bottom {
            return None;
        }
        Some([
            left.wrapping_sub(offset[0]),
            top.wrapping_sub(offset[1]),
            right.wrapping_sub(offset[0]),
            bottom.wrapping_sub(offset[1]),
        ])
    };
    let Some(mut src) = local(from, [0; 2], source) else {
        return Ok(None);
    };
    let Some(mut dst) = local(to, source.metadata().offset, target) else {
        return Ok(None);
    };
    if from.width == to.width && from.height == to.height {
        let width = (src[2] - src[0]).min(dst[2] - dst[0]);
        let height = (src[3] - src[1]).min(dst[3] - dst[1]);
        src[2] = src[0] + width;
        src[3] = src[1] + height;
        dst[2] = dst[0] + width;
        dst[3] = dst[1] + height;
    }
    let sw = src[2] - src[0];
    let sh = src[3] - src[1];
    let dw = dst[2] - dst[0];
    let dh = dst[3] - dst[1];
    if [sw, sh, dw, dh].iter().any(|&value| value <= 0) {
        return Ok(None);
    }
    let mut pixels = target.pixels().to_vec();
    let offset = |frame: &RgbaFrame, x: i32, y: i32| {
        ((frame.height() as usize - 1 - y as usize) * frame.width() as usize + x as usize) * 4
    };
    if sw == dw && sh == dh {
        for row in 0..dh {
            let s = offset(source, src[0], src[1] + row);
            let d = offset(target, dst[0], dst[1] + row);
            let length = dw as usize * 4;
            if alias {
                pixels.copy_within(s..s + length, d);
            } else {
                pixels[d..d + length].copy_from_slice(&source.pixels()[s..s + length]);
            }
        }
    } else {
        let ratio = [sw as f32 / dw as f32, sh as f32 / dh as f32];
        let average = i64::from(sw) > i64::from(dw) * 4 || i64::from(sh) > i64::from(dh) * 4;
        let step = [
            (i64::from(sw) << 15) / i64::from(dw),
            (i64::from(sh) << 15) / i64::from(dh),
        ];
        for y in 0..dh {
            for x in 0..dw {
                let input = if alias {
                    pixels.as_slice()
                } else {
                    source.pixels()
                };
                let sample = |sx: i32, sy: i32| -> [u8; 4] {
                    let i = offset(source, sx, sy);
                    std::array::from_fn(|axis| input[i + axis])
                };
                let pixel = if average {
                    let sx = src[0] + (x as f32 * ratio[0]) as i32;
                    let sy = src[1] + (y as f32 * ratio[1]) as i32;
                    let nx = (ratio[0] as i32).max(1).min(src[2] - sx);
                    let ny = (ratio[1] as i32).max(1).min(src[3] - sy);
                    let mut sum = [0u64; 4];
                    for dy in 0..ny {
                        for dx in 0..nx {
                            let value = sample(sx + dx, sy + dy);
                            for axis in 0..4 {
                                sum[axis] += u64::from(value[axis]);
                            }
                        }
                    }
                    let count = (i64::from(nx) * i64::from(ny)) as u64;
                    if count == 0 {
                        continue;
                    }
                    sum.map(|value| (value / count) as u8)
                } else {
                    // Baseline phase: origins are added directly to the fixed
                    // accumulator rather than shifted by fifteen bits.
                    let fx = i64::from(src[0]) + i64::from(x) * step[0];
                    let fy = i64::from(src[1]) + i64::from(y) * step[1];
                    let sx = (fx >> 15) as i32;
                    let sy = (fy >> 15) as i32;
                    let sx = sx.clamp(0, source.width() as i32 - 1);
                    let sy = sy.clamp(0, source.height() as i32 - 1);
                    let right = if sx + 1 < src[2] { sx + 1 } else { sx };
                    let bottom = if sy + 1 < src[3] { sy + 1 } else { sy };
                    let values = [
                        sample(sx, sy),
                        sample(right, sy),
                        sample(sx, bottom),
                        sample(right, bottom),
                    ];
                    if values.iter().all(|pixel| pixel[3] == 0) {
                        continue;
                    }
                    let u = fx & 32767;
                    let v = fy & 32767;
                    let weights = [
                        ((32768 - u) * (32768 - v)) >> 15,
                        (u * (32768 - v)) >> 15,
                        ((32768 - u) * v) >> 15,
                        (u * v) >> 15,
                    ];
                    let mut output = std::array::from_fn(|axis| {
                        ((0..4)
                            .map(|i| i64::from(values[i][axis]) * weights[i])
                            .sum::<i64>()
                            >> 15) as u8
                    });
                    if values.iter().all(|pixel| pixel[3] == 255) {
                        output[3] = 255;
                    }
                    output
                };
                let d = offset(target, dst[0] + x, dst[1] + y);
                pixels[d..d + 4].copy_from_slice(&pixel);
            }
        }
    }
    let frame = RgbaFrame::from_rgba8(target.width(), target.height(), pixels)
        .map_err(|_| error("native 116 could not rebuild its image wrapper"))?
        .with_raw_metadata(target.metadata())
        .with_hg_attributes(target.hg_attributes().clone());
    Ok(Some(frame))
}

fn float_to_i32(value: f32) -> i32 {
    if !value.is_finite() || value < i32::MIN as f32 || value >= 2147483648. {
        i32::MIN
    } else {
        value.trunc() as i32
    }
}

pub(crate) fn plane_command_supported(code: u32) -> bool {
    matches!(code, 46..=63 | 66 | 67 | 75 | 76 | 79..=86 | 89 | 591 | 592)
        && !matches!(code, 60 | 83)
}

fn require_buffer<'a>(buffer: &'a Option<&mut [u8]>, length: usize) -> Result<&'a [u8]> {
    buffer
        .as_deref()
        .filter(|bytes| bytes.len() >= length)
        .ok_or_else(|| error("plane command has no sufficiently sized explicit buffer"))
}

fn read_vector_buffer(buffer: &Option<&mut [u8]>) -> Result<[f32; 3]> {
    let bytes = require_buffer(buffer, 12)?;
    Ok([float(bytes, 0)?, float(bytes, 4)?, float(bytes, 8)?])
}

fn read_matrix_buffer(buffer: &Option<&mut [u8]>) -> Result<[f32; 16]> {
    let bytes = require_buffer(buffer, 64)?;
    let mut values = [0.; 16];
    for (axis, value) in values.iter_mut().enumerate() {
        *value = float(bytes, axis * 4)?;
    }
    Ok(values)
}

fn write_vector_buffer(buffer: &mut Option<&mut [u8]>, values: [f32; 3]) -> Result<()> {
    let bytes = buffer
        .as_deref_mut()
        .filter(|bytes| bytes.len() >= 12)
        .ok_or_else(|| error("plane vector output needs twelve writable bytes"))?;
    for (axis, value) in values.into_iter().enumerate() {
        bytes[axis * 4..axis * 4 + 4].copy_from_slice(&value.to_bits().to_le_bytes());
    }
    Ok(())
}

fn output_addresses(vm: &Vm, args: &[u8], count: usize) -> Result<Vec<u32>> {
    let addresses = (0..count)
        .map(|axis| word(args, 4 + axis * 4))
        .collect::<Result<Vec<_>>>()?;
    for &address in &addresses {
        vm.read(address, 1)?;
    }
    Ok(addresses)
}

fn word(args: &[u8], offset: usize) -> Result<u32> {
    let bytes = args
        .get(offset..offset + 4)
        .ok_or(ScriptError::Truncated { offset, needed: 4 })?;
    Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| {
        ScriptError::Truncated { offset, needed: 4 }
    })?))
}

fn float(args: &[u8], offset: usize) -> Result<f32> {
    word(args, offset).map(f32::from_bits)
}

fn error(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}
