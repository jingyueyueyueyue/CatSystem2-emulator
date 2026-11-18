//! Owner-held drawable history. Snapshot storage and drawable identities are
//! independent of task identities and the input/task traversal indexes.
use super::*;

const MAX_HISTORY_SLOTS: usize = 65_536;

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Snapshot {
    base: [f32; 3],
    position: [f32; 3],
    scale: [f32; 3],
    matrix: [f32; 16],
    color: u32,
    display: u32,
}

impl Snapshot {
    fn capture(state: &PlaneState, node: &SceneNode) -> Self {
        let mut geometry = state.wrapper.clone();
        geometry.position = node.position;
        geometry.scale = [1.; 3];
        Self {
            base: state.wrapper.base,
            position: node.position,
            scale: node.scale,
            matrix: geometry_matrix(&geometry),
            color: if state.color_enabled != 0 {
                state.color
            } else {
                node.color
            },
            display: state.display,
        }
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 128 {
            return Err(error("plane history snapshot needs 128 bytes"));
        }
        Ok(Self {
            base: std::array::from_fn(|axis| f32::from_bits(read_word(bytes, 12 + axis * 4))),
            position: std::array::from_fn(|axis| f32::from_bits(read_word(bytes, 28 + axis * 4))),
            scale: std::array::from_fn(|axis| f32::from_bits(read_word(bytes, 44 + axis * 4))),
            matrix: std::array::from_fn(|axis| f32::from_bits(read_word(bytes, 60 + axis * 4))),
            color: read_word(bytes, 124),
            display: read_word(bytes, 8),
        })
    }
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct PlaneHistory {
    interval: u32,
    alpha: u32,
    scale: [f32; 3],
    offset: [f32; 3],
    mode: u32,
    depth_step: u32,
    snapshots: Vec<Snapshot>,
    proxies: Vec<Option<u32>>,
    shape: PlaneState,
    source: Geometry,
    strip: f32,
    rebuild: bool,
    last_body: Option<u64>,
    pending_body: Option<u64>,
}

impl PlaneHistory {
    pub(super) fn graph_proxies(&self) -> impl Iterator<Item = u32> + '_ {
        self.proxies.iter().flatten().copied()
    }

    /// Validate stored continuation only; never regenerate strips, shift the
    /// history or submit geometry while preparing a replacement graph.
    pub(super) fn validate_graph(&self, frame: u64) -> HostResult<usize> {
        if self.snapshots.is_empty() || self.snapshots.len() > MAX_HISTORY_SLOTS
            || self.proxies.len() != self.snapshots.len() || self.proxies[0].is_some()
            || self.last_body.is_some_and(|body| body > frame)
            || self.pending_body.is_some_and(|body| body > frame)
            || (self.mode == 1 && self.interval != 2)
            || (matches!(self.mode, 2..=5) && self.interval != 1)
        {
            bail!("saved plane history has invalid slots or body continuation");
        }
        for (index, proxy) in self.proxies.iter().enumerate().skip(1) {
            let allocated = self.mode != 0
                || (self.interval != 0 && index as u32 % self.interval == 0);
            if allocated != proxy.is_some() || proxy == &Some(0) {
                bail!("saved plane history proxy slots differ from its configuration");
            }
        }
        Ok(self.snapshots.len())
    }
}

impl PlaneServices {
    pub(super) fn invalidate_history_geometry(&mut self, id: u32) {
        let Some(history) = self.histories.get_mut(&id) else {
            return;
        };
        history.rebuild = true;
        for proxy in history.proxies.iter().flatten() {
            if let Some(state) = self.planes.get_mut(proxy) {
                state.display = 0;
            }
        }
    }
}

impl HeadlessPlayer {
    pub(super) fn release_plane_history(&mut self, id: u32) {
        let Some(history) = self.data_services.planes.histories.remove(&id) else {
            return;
        };
        for proxy in history.proxies.into_iter().flatten() {
            self.scene_nodes.remove(&proxy);
            self.data_services.planes.release(proxy);
            self.clear_kx_geometry(proxy);
        }
    }
}

fn read_word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn interpolate(from: f32, to: f32, at: usize, count: usize) -> f32 {
    if at == 0 {
        from
    } else if at >= count {
        to
    } else {
        (to - from) * (at as f32 / count as f32) + from
    }
}

fn rotate_z(matrix: [f32; 16], angle: f32) -> [f32; 16] {
    let (sine, cosine) = angle.sin_cos();
    let mut rotation = IDENTITY;
    rotation[0] = cosine;
    rotation[1] = sine;
    rotation[4] = -sine;
    rotation[5] = cosine;
    multiply(matrix, rotation)
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let delta: [f32; 3] = std::array::from_fn(|axis| a[axis] - b[axis]);
    f64::from((delta[1] * delta[1] + delta[0] * delta[0]) + delta[2] * delta[2]).sqrt() as f32
}

impl LoggingHost<'_> {
    pub(super) fn plane_history_accepts_snapshot(&self, id: u32) -> bool {
        self.data_services.planes.histories.contains_key(&id)
            && self
                .scene_nodes
                .get(&id)
                .is_some_and(|node| node.initialized)
            && self
                .data_services
                .planes
                .planes
                .get(&id)
                .is_some_and(|state| state.display != 0)
    }

    pub(super) fn plane_history_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &Vm,
    ) -> Result<Option<NativeReturn>> {
        let id = word(args, 0)?;
        if index == 398 && id == 0 {
            return Ok(Some(NativeReturn::Continue));
        }
        if (id != 0 && self.scene_nodes.get(&id).is_some_and(|node| node.kind != 2))
            || self
                .movie_host
                .as_ref()
                .is_some_and(|movie| movie.plane_target_handle(id).is_some())
        {
            return Ok(None);
        }
        let (code, data, mut record) = if index == 398 {
            let values: Vec<_> = (1..12)
                .map(|index| word(args, index * 4))
                .collect::<Result<_>>()?;
            let words = [
                values[0], values[1], values[5], values[6], values[7], values[8], 0, values[2],
                values[3], values[4], 0, values[9], values[10],
            ];
            (
                79,
                [u32::from(values[0] != 0), 0, 0],
                (values[0] != 0).then(|| {
                    words
                        .into_iter()
                        .flat_map(u32::to_le_bytes)
                        .collect::<Vec<_>>()
                }),
            )
        } else {
            let addresses = [
                word(args, 4)?,
                word(args, 8)?,
                word(args, 12)?,
                word(args, 16)?,
            ];
            // Resolve all four addresses before dereferencing any vector.
            for address in addresses {
                vm.read(address, 0)?;
            }
            let mut bytes = vec![0; 128];
            for (address, at, length) in [
                (addresses[0], 12, 12),
                (addresses[1], 28, 12),
                (addresses[2], 44, 12),
                (addresses[3], 60, 64),
            ] {
                bytes[at..at + length].copy_from_slice(&vm.read(address, length)?);
            }
            bytes[124..128].copy_from_slice(&word(args, 20)?.to_le_bytes());
            bytes[8..12].copy_from_slice(&word(args, 24)?.to_le_bytes());
            (80, [0; 3], Some(bytes))
        };
        self.dispatch_plane_command(
            TaskCommand {
                target: id as i32,
                sender: self.current_task_id as i32,
                code,
                data,
            },
            &mut PlaneCommandBuffers {
                data0: None,
                data1: record.as_deref_mut(),
            },
            &mut 0,
        )?;
        Ok(Some(NativeReturn::Continue))
    }

    pub(crate) fn release_plane_history(&mut self, id: u32) {
        let Some(history) = self.data_services.planes.histories.remove(&id) else {
            return;
        };
        for proxy in history.proxies.into_iter().flatten() {
            self.scene_nodes.remove(&proxy);
            self.data_services.planes.release(proxy);
            self.clear_plane_mesh(proxy);
        }
    }

    pub(super) fn configure_plane_history(&mut self, id: u32, bytes: Option<&[u8]>) -> Result<()> {
        self.release_plane_history(id);
        let Some(bytes) = bytes else { return Ok(()) };
        if bytes.len() < 52 {
            return Err(error("plane history configuration needs 52 bytes"));
        }
        let Some(node) = self
            .scene_nodes
            .get(&id)
            .filter(|node| node.initialized)
            .cloned()
        else {
            return Ok(());
        };
        let shape = self
            .data_services
            .planes
            .planes
            .get(&id)
            .cloned()
            .unwrap_or_else(|| PlaneState::from_node(&node));
        let mode = read_word(bytes, 44);
        let interval = match mode {
            1 => 2,
            2..=5 => 1,
            _ => read_word(bytes, 4),
        };
        let count = u64::from(read_word(bytes, 0))
            .checked_mul(u64::from(interval))
            .filter(|&count| count < MAX_HISTORY_SLOTS as u64)
            .ok_or_else(|| error("plane history exceeds 65536 snapshots"))?
            as usize;
        if let Some(mesh) = self.data_services.messages.kx.objects.get(&id) {
            let copies = if mode == 0 && interval != 0 {
                count / interval as usize
            } else {
                count
            };
            let bytes =
                mesh.mesh
                    .triangles
                    .len()
                    .checked_mul(48)
                    .and_then(|size| size.checked_mul(copies))
                    .and_then(|size| {
                        self.data_services.messages.kx.objects.values().try_fold(
                            size,
                            |sum, object| {
                                sum.checked_add(object.mesh.triangles.len().checked_mul(48)?)
                            },
                        )
                    })
                    .filter(|&size| size <= 256 * 1024 * 1024);
            if bytes.is_none() {
                return Err(error("plane history KX storage exceeds 256 MiB"));
            }
        }
        let current = Snapshot::capture(&shape, &node);
        let mut history = PlaneHistory {
            interval,
            alpha: read_word(bytes, 8),
            scale: std::array::from_fn(|axis| f32::from_bits(read_word(bytes, 12 + axis * 4))),
            offset: std::array::from_fn(|axis| f32::from_bits(read_word(bytes, 28 + axis * 4))),
            mode,
            depth_step: read_word(bytes, 48),
            snapshots: vec![current; count + 1],
            proxies: vec![None; count + 1],
            source: shape.geometry(1).clone(),
            shape,
            strip: 0.,
            rebuild: false,
            last_body: None,
            pending_body: None,
        };
        for index in 1..=count {
            history.snapshots[index].position = std::array::from_fn(|axis| {
                history.snapshots[index - 1].position[axis] - history.offset[axis]
            });
            let periodic = interval != 0 && index as u32 % interval == 0;
            if periodic || mode != 0 {
                let proxy = self.allocate_handle();
                let mut node = node.clone();
                node.id = proxy;
                node.parent = id;
                node.visible = false;
                node.initialized = true;
                let mut state = history.shape.clone();
                state.display = 0;
                state.input_configured = false;
                state.receiver = None;
                state.parent_offset = None;
                self.scene_nodes.insert(proxy, node);
                self.data_services.planes.planes.insert(proxy, state);
                self.data_services.planes.history_owners.insert(proxy, id);
                self.data_services.planes.register_drawable(proxy);
                history.proxies[index] = Some(proxy);
                if let Some(mesh) = self.data_services.messages.kx.objects.get(&id).cloned() {
                    self.data_services.messages.kx.objects.insert(proxy, mesh);
                }
            }
        }
        self.prepare_history_strips(id, &mut history);
        self.data_services.planes.histories.insert(id, history);
        Ok(())
    }

    fn prepare_history_strips(&mut self, id: u32, history: &mut PlaneHistory) {
        let Some(axis) = (match history.mode {
            4 => Some(1),
            5 => Some(0),
            _ => None,
        }) else {
            history.strip = history.shape.extent[0];
            return;
        };
        let dimension = history.shape.extent[axis];
        let pixels = float_to_i32(dimension);
        let (half, step) = if pixels & 1 != 0 {
            (pixels / 2, 1)
        } else {
            (pixels / 2 - 1, 2)
        };
        history.strip = step as f32;
        let origin = history.source.position;
        if let Some(state) = self.data_services.planes.planes.get_mut(&id) {
            state.geometry_mut(1).scale[axis] = half as f32 / dimension;
            if state.shape != PlaneShape::Canvas
                && let Some(node) = self.scene_nodes.get_mut(&id)
            {
                node.scale = state.wrapper.scale;
            }
        }
        let last = history.proxies.len() - 1;
        for index in 1..=last {
            if let Some(proxy) = history.proxies[index]
                && let Some(state) = self.data_services.planes.planes.get_mut(&proxy)
            {
                let source = state.geometry_mut(1);
                source.position = origin;
                source.position[axis] += if index == last {
                    (half + step) as f32
                } else {
                    half as f32
                };
                source.scale = [1.; 3];
                source.scale[axis] = if index == last {
                    (dimension - (half - step) as f32) / dimension
                } else {
                    step as f32 / dimension
                };
                if state.shape != PlaneShape::Canvas
                    && let Some(node) = self.scene_nodes.get_mut(&proxy)
                {
                    node.position = state.wrapper.position;
                    node.scale = state.wrapper.scale;
                }
            }
        }
    }

    pub(super) fn push_plane_history_snapshot(
        &mut self,
        id: u32,
        bytes: Option<&[u8]>,
    ) -> Result<()> {
        let Some(mut history) = self.data_services.planes.histories.remove(&id) else {
            return Ok(());
        };
        let result = (|| {
            let Some(node) = self.scene_nodes.get(&id) else {
                return Ok(());
            };
            if self
                .data_services
                .planes
                .planes
                .get(&id)
                .is_none_or(|state| state.display == 0)
                || !node.initialized
            {
                return Ok(());
            }
            let next = Snapshot::decode(
                bytes.ok_or_else(|| error("plane history snapshot buffer is absent"))?,
            )?;
            shift_history(&mut history, next);
            Ok(())
        })();
        self.data_services.planes.histories.insert(id, history);
        result
    }

    pub(super) fn synchronize_plane_history_control(&mut self, id: u32, code: u32) -> Result<()> {
        let Some(history) = self.data_services.planes.histories.get(&id) else {
            return Ok(());
        };
        let Some(owner) = self.data_services.planes.planes.get(&id) else {
            return Ok(());
        };
        let display = owner.display;
        let blend = owner.blend;
        let proxies: Vec<_> = history
            .proxies
            .iter()
            .enumerate()
            .filter_map(|(index, proxy)| proxy.map(|proxy| (index, proxy)))
            .collect();
        for (index, proxy) in proxies {
            if let Some(state) = self.data_services.planes.planes.get_mut(&proxy) {
                if code == 81 {
                    state.blend = blend;
                } else if history.interval != 0 && index as u32 % history.interval == 0 {
                    state.display = u32::from(display != 0);
                    if let Some(node) = self.scene_nodes.get_mut(&proxy) {
                        node.visible = display != 0;
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn advance_plane_history(&mut self, id: u32, _now: u32) -> Result<()> {
        let Some(mut history) = self.data_services.planes.histories.remove(&id) else {
            return Ok(());
        };
        let result = (|| {
            if history.last_body == Some(self.frame) {
                return Ok(());
            }
            let Some(node) = self
                .scene_nodes
                .get(&id)
                .filter(|node| node.initialized)
                .cloned()
            else {
                return Ok(());
            };
            let Some(state) = self.data_services.planes.planes.get(&id).cloned() else {
                return Ok(());
            };
            if state.display == 0 {
                return Ok(());
            }
            if history.rebuild {
                if let Some(mesh) = self.data_services.messages.kx.objects.get(&id) {
                    let proxies: std::collections::BTreeSet<_> =
                        history.proxies.iter().flatten().copied().collect();
                    let total = mesh
                        .mesh
                        .triangles
                        .len()
                        .checked_mul(48)
                        .and_then(|size| size.checked_mul(proxies.len()))
                        .and_then(|size| {
                            self.data_services
                                .messages
                                .kx
                                .objects
                                .iter()
                                .filter(|(key, _)| !proxies.contains(*key))
                                .try_fold(size, |sum, (_, object)| {
                                    sum.checked_add(object.mesh.triangles.len().checked_mul(48)?)
                                })
                        })
                        .filter(|&size| size <= 256 * 1024 * 1024);
                    if total.is_none() {
                        return Err(error("rebuilt plane history KX storage exceeds 256 MiB"));
                    }
                }
                history.shape = state.clone();
                history.source = state.geometry(1).clone();
                let mesh = self.data_services.messages.kx.objects.get(&id).cloned();
                for proxy in history.proxies.iter().flatten() {
                    let mut replacement = state.clone();
                    replacement.display = 0;
                    replacement.input_configured = false;
                    replacement.receiver = None;
                    if let Some(proxy_node) = self.scene_nodes.get_mut(proxy) {
                        proxy_node.initialized = true;
                        proxy_node.size = node.size;
                        self.data_services.planes.planes.insert(*proxy, replacement);
                    }
                    self.clear_plane_mesh(*proxy);
                    if let Some(mesh) = &mesh {
                        self.data_services
                            .messages
                            .kx
                            .objects
                            .insert(*proxy, mesh.clone());
                    }
                }
                self.prepare_history_strips(id, &mut history);
                history.rebuild = false;
            }
            if history.pending_body != Some(self.frame) {
                shift_history(&mut history, Snapshot::capture(&state, &node));
                history.pending_body = Some(self.frame);
            }
            self.submit_plane_history(id, &history)?;
            history.last_body = Some(self.frame);
            history.pending_body = None;
            Ok(())
        })();
        self.data_services.planes.histories.insert(id, history);
        result
    }

    fn submit_plane_history(&mut self, id: u32, history: &PlaneHistory) -> Result<()> {
        if history.mode > 5 {
            return Ok(());
        }
        let count = history.snapshots.len() - 1;
        for index in 1..=count {
            let Some(proxy) = history.proxies[index] else {
                continue;
            };
            let snapshot = &history.snapshots[index];
            let periodic = history.interval != 0 && index as u32 % history.interval == 0;
            if history.mode == 0 && !periodic {
                continue;
            }
            if matches!(history.mode, 2..=5) && index == count && history.mode < 4 {
                continue;
            }
            let mut base = snapshot.base;
            base[2] += history.depth_step.wrapping_mul(index as u32) as i32 as f32;
            let mut matrix = snapshot.matrix;
            let mut scale = std::array::from_fn(|axis| {
                let target = if history.mode == 0 {
                    snapshot.scale[axis] * history.scale[axis]
                } else {
                    history.scale[axis]
                };
                interpolate(snapshot.scale[axis], target, index, count)
            });
            if history.mode >= 2 || (history.mode == 1 && !periodic) {
                let (first, second) = if history.mode >= 4 {
                    (&history.snapshots[index - 1], snapshot)
                } else {
                    (&history.snapshots[index - 1], &history.snapshots[index + 1])
                };
                let angle = ((second.position[1] - first.position[1]) as f64)
                    .atan2((second.position[0] - first.position[0]) as f64)
                    as f32;
                let ribbon = history.mode >= 4;
                matrix = rotate_z(
                    matrix,
                    if ribbon {
                        angle - std::f32::consts::FRAC_PI_2
                    } else {
                        angle
                    },
                );
                let multiplier = if history.mode == 2 { 0.5 } else { 1. };
                let width = if ribbon {
                    history.strip
                } else {
                    history.shape.extent[0]
                };
                let ratio =
                    (width + distance(second.position, first.position) * multiplier) / width;
                scale[if ribbon { 1 } else { 0 }] = ratio;
                if ribbon {
                    base[1] = if index == count {
                        0.
                    } else {
                        history.strip * 0.5
                    };
                }
            }
            let alpha = float_to_i32(interpolate(
                (snapshot.color >> 24) as f32,
                history.alpha as f32,
                index,
                count,
            )) as u32;
            let state = self
                .data_services
                .planes
                .planes
                .get_mut(&proxy)
                .ok_or_else(|| error("plane history drawable state disappeared"))?;
            state.wrapper.base = base;
            state.wrapper.position = snapshot.position;
            state.wrapper.scale = scale;
            state.wrapper.matrix = Some(matrix);
            state.color = (snapshot.color & 0x00ff_ffff) | (alpha << 24);
            state.color_enabled = 1;
            state.display = snapshot.display;
            let node = self
                .scene_nodes
                .get_mut(&proxy)
                .ok_or_else(|| error("plane history drawable owner disappeared"))?;
            node.position = snapshot.position;
            node.scale = scale;
            node.visible = snapshot.display != 0;
        }
        if history.mode >= 4 && count != 0 {
            let first = &history.snapshots[0];
            let second = &history.snapshots[1];
            let angle = ((second.position[1] - first.position[1]) as f64)
                .atan2((second.position[0] - first.position[0]) as f64)
                as f32;
            if let Some(state) = self.data_services.planes.planes.get_mut(&id) {
                state.wrapper.matrix =
                    Some(rotate_z(first.matrix, angle - std::f32::consts::FRAC_PI_2));
            }
        }
        Ok(())
    }
}

fn shift_history(history: &mut PlaneHistory, next: Snapshot) {
    for index in (1..history.snapshots.len()).rev() {
        let mut previous = history.snapshots[index - 1].clone();
        previous.position =
            std::array::from_fn(|axis| previous.position[axis] + history.offset[axis]);
        history.snapshots[index] = previous;
    }
    history.snapshots[0] = next;
}
