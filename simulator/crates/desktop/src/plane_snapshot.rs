//! PLANE data, history drawables, grids, KX sources and Raster continuations.
//! Movie decoders and movie-bound callbacks remain the media component's job.
use super::*;
use cs2_runtime::graph_serde::{Deserialize, Serialize};
use cs2_runtime::object_graph::{self as wire, ObjectGraphSnapshot};
use crate::movie_plane::{MoviePlanes, PlaneCamera};
use crate::movie_plane_render::MoviePlaneProjection;
use crate::message_platform::kx_native::KxGraphSnapshot;
use crate::media_wrapper_native::RasterParameters;

const TAG: &[u8] = b"CS2PLANEVIS2\0";

pub(super) mod blend {
    use super::*;
    pub fn serialize<S: cs2_runtime::graph_serde::Serializer>(value: &BlendMode, s: S) -> std::result::Result<S::Ok, S::Error> {
        (*value as u32).serialize(s)
    }
    pub fn deserialize<'de, D: cs2_runtime::graph_serde::Deserializer<'de>>(d: D) -> std::result::Result<BlendMode, D::Error> {
        let value = u32::deserialize(d)?;
        if value > 5 { return Err(cs2_runtime::graph_serde::de::Error::custom("invalid plane blend")); }
        Ok(BlendMode::from_raw(value))
    }
}
pub(super) mod rectangle {
    use super::*;
    pub fn serialize<S: cs2_runtime::graph_serde::Serializer>(value: &Option<Rect>, s: S) -> std::result::Result<S::Ok, S::Error> {
        value.map(|r| [r.x as u32, r.y as u32, r.width, r.height]).serialize(s)
    }
    pub fn deserialize<'de, D: cs2_runtime::graph_serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<Rect>, D::Error> {
        Ok(Option::<[u32; 4]>::deserialize(d)?.map(|r| Rect::new(r[0] as i32, r[1] as i32, r[2], r[3])))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Snapshot {
    version: u32, planes: BTreeMap<u32, PlaneState>, order: Vec<u32>,
    order_depths: BTreeMap<u32, u32>, order_dirty: bool,
    view: crate::canvas_native::ViewSnapshot, next_sequence: u64, snapshot_active: bool,
    grid_records: BTreeMap<u32, [u32; 10]>,
    histories: BTreeMap<u32, history::PlaneHistory>, history_owners: BTreeMap<u32, u32>,
    proxy_nodes: BTreeMap<u32, wire::SceneNodeSnapshot>,
    inert_media: std::collections::BTreeSet<u32>, raster: raster::RasterStore,
    raster_parameters: BTreeMap<u32, RasterParameters>, meshes: KxGraphSnapshot,
}

pub(crate) struct PreparedPlaneVisuals {
    services: PlaneServices, projection: MoviePlaneProjection,
    cameras: BTreeMap<u32, PlaneCamera>, next_sequence: u64, snapshot_active: bool,
    proxy_nodes: BTreeMap<u32, SceneNode>,
    raster_parameters: BTreeMap<u32, RasterParameters>, meshes: KxGraphSnapshot,
}

impl PreparedPlaneVisuals {
    pub(crate) fn object_handles(&self) -> impl Iterator<Item = u32> + '_ {
        self.services.history_owners.keys().copied()
    }

    /// Called during the outer prepare stage, before its first live write.
    /// Moving prepared map entries avoids allocating/rebuilding drawables in
    /// install, and keeps these nodes out of the ordinary task hierarchy.
    pub(crate) fn prepare_scene_nodes(&mut self, nodes: &mut BTreeMap<u32, SceneNode>) -> HostResult<()> {
        if self.proxy_nodes.keys().any(|id| nodes.contains_key(id)) {
            bail!("prepared plane proxy overlaps a task scene node");
        }
        nodes.append(&mut self.proxy_nodes);
        Ok(())
    }
}

impl PlaneServices {
    pub(crate) fn check_graph_boundary(&self) -> HostResult<()> {
        self.raster.check_graph_boundary()
    }
}

impl HeadlessPlayer {
    pub(crate) fn check_plane_visuals_graph_boundary(&self) -> HostResult<()> {
        self.data_services.planes.check_graph_boundary()?;
        let services = &self.data_services.planes;
        if self.data_services.tasks.records.iter().any(|(&id, record)| match record.kind {
            18 => !services.raster.owns(id),
            25 => !services.inert_media.contains(&id),
            _ => false,
        }) {
            bail!("graphics media task has no recovered retained state for its PE");
        }
        let movie = &self.movie_host.planes;
        if !movie.movies.is_empty() || !movie.properties.is_empty() || !movie.presentations.is_empty()
            || !movie.frame_snapshots.is_empty() || movie.exclusive_snapshot.is_some() {
            bail!("plain-plane graph cannot discard retained movie-plane state");
        }
        Ok(())
    }

    pub(crate) fn plane_visuals_snapshot(&self) -> HostResult<Vec<u8>> {
        self.check_plane_visuals_graph_boundary()?;
        let services = &self.data_services.planes;
        let movie = &self.movie_host.planes;
        let proxy_nodes = services.history_owners.keys().map(|&id| {
            let node = self.scene_nodes.get(&id)
                .ok_or_else(|| anyhow::anyhow!("plane history proxy node disappeared"))?;
            if node.id != id || node.kind != 2 || self.task_parents.contains_key(&id) {
                bail!("plane history drawable overlaps the task hierarchy");
            }
            Ok((id, capture_proxy_node(node)))
        }).collect::<HostResult<_>>()?;
        let snapshot = Snapshot {
            version: 2, planes: services.planes.clone(), order: services.order.clone(),
            order_depths: services.order_depths.clone(), order_dirty: services.order_dirty,
            view: crate::canvas_native::ViewSnapshot::capture(&movie.projection, &movie.cameras),
            next_sequence: movie.next_sequence, snapshot_active: movie.snapshot_active,
            grid_records: services.grid_records.clone(), histories: services.histories.clone(),
            history_owners: services.history_owners.clone(), proxy_nodes,
            inert_media: services.inert_media.clone(), raster: services.raster.clone(),
            raster_parameters: self.data_services.sound_media.effects.raster_graph_snapshot(),
            meshes: self.data_services.messages.kx.graph_snapshot(),
        };
        let mut bytes = TAG.to_vec();
        bytes.extend_from_slice(&wire::encode_component(&snapshot)?);
        Ok(bytes)
    }

    pub(crate) fn prepare_plane_visuals_restore(bytes: &[u8], saved: &ObjectGraphSnapshot) -> HostResult<PreparedPlaneVisuals> {
        let payload = bytes.strip_prefix(TAG)
            .ok_or_else(|| anyhow::anyhow!("unsupported plane component tag; expected CS2PLANEVIS2"))?;
        let s: Snapshot = wire::decode_component(payload)?;
        if s.version != 2 || s.planes.len() > 1_000_000 || s.order.len() > 1_000_000
            || s.order_depths.len() > 1_000_000 || s.proxy_nodes.len() > 1_000_000
            || s.histories.len() > 65_536 || s.grid_records.len() > 1_000_000
            || s.inert_media.len() > 65_536 || s.raster_parameters.len() > 65_536 {
            bail!("saved plane table exceeds its limits");
        }
        let node = |id: &u32| saved.nodes.get(id).or_else(|| s.proxy_nodes.get(id));
        for (&id, proxy) in &s.proxy_nodes {
            if id == 0 || proxy.id != id || proxy.kind != 2
                || saved.nodes.contains_key(&id) || saved.records.contains_key(&id)
                || saved.parents.contains_key(&id) || saved.reserved.contains(&id)
                || saved.wrapper_pixels.contains_key(&id) || saved.image_pixels.contains_key(&id)
                || saved.image_resources.contains_key(&id)
                || proxy.name.len() > 65_536 || proxy.fes_parameters.len() != 512
                || s.history_owners.get(&id) != Some(&proxy.parent)
            {
                bail!("saved history proxy has invalid or overlapping drawable identity");
            }
        }
        let mut expected_owners = BTreeMap::new();
        let mut slots = 0usize;
        for (&owner, history) in &s.histories {
            if saved.nodes.get(&owner).is_none_or(|n| n.kind != 2)
                || !s.planes.contains_key(&owner) {
                bail!("saved plane history owner is absent");
            }
            slots = slots.checked_add(history.validate_graph(saved.state.clock.frame)?)
                .filter(|&count| count <= 1_000_000)
                .ok_or_else(|| anyhow::anyhow!("saved plane history storage exceeds limit"))?;
            for proxy in history.graph_proxies() {
                if expected_owners.insert(proxy, owner).is_some()
                    || !s.proxy_nodes.contains_key(&proxy) || !s.planes.contains_key(&proxy) {
                    bail!("saved history proxy is duplicated or lacks its stored geometry");
                }
            }
        }
        if expected_owners != s.history_owners || expected_owners.keys().ne(s.proxy_nodes.keys()) {
            bail!("saved history slots, drawable nodes and owner index disagree");
        }
        for (id, record) in &s.grid_records {
            let count = u64::from((record[8] as i32).max(0) as u32)
                * u64::from((record[9] as i32).max(0) as u32);
            if saved.nodes.get(id).is_none_or(|n| n.kind != 2)
                || !s.planes.contains_key(id) || count > 65_536 {
                bail!("saved plane grid has invalid owner or constructor record");
            }
            // Children are real saved task/node records. Scripts may delete
            // or edit them later; never recreate a grid from its old record.
        }
        if (!s.inert_media.is_empty() && !matches!(saved.pe_sha256.as_str(),
                "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f"
                | "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"))
            || s.inert_media.iter().any(|id| saved.records.get(id).is_none_or(|r| r.kind != 25))
            || saved.records.iter().any(|(id, r)| r.kind == 25 && !s.inert_media.contains(id))
            || s.raster_parameters.keys().any(|id| saved.records.get(id).is_none_or(|r| r.kind != 18)) {
            bail!("saved graphics media configuration lost its task owner");
        }
        s.raster.validate_graph(saved, &s.proxy_nodes)?;
        validate_meshes(&s.meshes, &node)?;
        let order = s.order.iter().copied().collect::<std::collections::BTreeSet<_>>();
        // The scene sort includes script, input and list nodes as well as
        // planes. Entries can retain their prior depth until the next draw.
        if order.len() != s.order.len() || order.iter().any(|id| node(id).is_none())
            || s.order_depths.keys().any(|id| node(id).is_none())
            || s.planes.keys().any(|id| node(id).is_none_or(|n| n.kind != 2))
            || saved.nodes.iter().any(|(id, n)| n.kind == 2 && n.initialized && !s.planes.contains_key(id)) {
            bail!("saved plane owner/draw ordering differs from the task tree");
        }
        for p in s.planes.values() {
            if p.receiver.is_some_and(|owner| !saved.records.contains_key(&owner)) {
                bail!("saved plane input receiver is absent");
            }
            // Input removal does not clear input_configured. The flag and
            // the independently saved region table must remain independent.
        }
        let (projection, cameras) = s.view.restore()?;
        let proxy_nodes = s.proxy_nodes.iter().map(|(&id, node)|
            Ok((id, restore_proxy_node(node)?))).collect::<HostResult<_>>()?;
        Ok(PreparedPlaneVisuals {
            services: PlaneServices { planes: s.planes, order: s.order,
                order_depths: s.order_depths, order_dirty: s.order_dirty,
                grid_records: s.grid_records, histories: s.histories,
                history_owners: s.history_owners, inert_media: s.inert_media, raster: s.raster,
                #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
                sampled_planes: Default::default(),
                vram_overflow: None },
            projection, cameras, next_sequence: s.next_sequence, snapshot_active: s.snapshot_active,
            proxy_nodes, raster_parameters: s.raster_parameters, meshes: s.meshes,
        })
    }

    pub(crate) fn install_plane_visuals_restore(&mut self, p: PreparedPlaneVisuals) {
        self.data_services.planes = p.services;
        // The enclosing prepare_scene_nodes moved the complete proxy nodes
        // into its replacement map, committed before this component.
        self.data_services.messages.kx.install_graph(p.meshes);
        self.data_services.sound_media.effects.install_raster_graph(p.raster_parameters);
        let movie: &mut MoviePlanes = &mut self.movie_host.planes;
        movie.projection = p.projection; movie.cameras = p.cameras;
        movie.next_sequence = p.next_sequence; movie.snapshot_active = p.snapshot_active;
    }
}

fn capture_proxy_node(n: &SceneNode) -> wire::SceneNodeSnapshot {
    wire::SceneNodeSnapshot { id: n.id, kind: n.kind, parent: n.parent, name: n.name.clone(),
        size_bits: n.size.map(f32::to_bits), position_bits: n.position.map(f32::to_bits),
        scale_bits: n.scale.map(f32::to_bits), color: n.color, overlay_color: n.overlay_color,
        visible: n.visible, initialized: n.initialized, fes_block_name: n.fes_block_name.clone(),
        fes_object_prefix: n.fes_object_prefix.clone(), fes_bank: n.fes_bank,
        fes_parameters: n.fes_parameters.to_vec() }
}

fn restore_proxy_node(n: &wire::SceneNodeSnapshot) -> HostResult<SceneNode> {
    Ok(SceneNode { id: n.id, kind: n.kind, parent: n.parent, name: n.name.clone(),
        size: n.size_bits.map(f32::from_bits), position: n.position_bits.map(f32::from_bits),
        scale: n.scale_bits.map(f32::from_bits), color: n.color, overlay_color: n.overlay_color,
        visible: n.visible, initialized: n.initialized, fes_block_name: n.fes_block_name.clone(),
        fes_object_prefix: n.fes_object_prefix.clone(), fes_bank: n.fes_bank,
        fes_parameters: n.fes_parameters.as_slice().try_into()? })
}

fn validate_meshes<'a>(s: &KxGraphSnapshot,
    node: &impl Fn(&u32) -> Option<&'a wire::SceneNodeSnapshot>) -> HostResult<()> {
    if s.objects.len() > 1_000_000 || s.names.len() > 1_000_000 || s.metadata.len() > 1_000_000
        || s.objects.keys().chain(s.names.keys()).chain(s.metadata.keys())
            .any(|id| node(id).is_none_or(|n| n.kind != 2))
        || s.names.values().any(|name| name.len() >= 260 || name.contains(&0))
        || s.metadata.values().any(|(_, kind)| !matches!(kind, 6 | 7)) {
        bail!("saved KX source identities or retained file metadata are invalid");
    }
    let mut bytes = 0usize;
    for object in s.objects.values() {
        let length = object.mesh.triangles.len().checked_mul(48)
            .filter(|&length| length <= 64 * 1024 * 1024 - 24)
            .ok_or_else(|| anyhow::anyhow!("saved KX source exceeds its file allocation limit"))?;
        bytes = bytes.checked_add(length).filter(|&total| total <= 256 * 1024 * 1024)
            .ok_or_else(|| anyhow::anyhow!("saved KX source copies exceed storage limit"))?;
        if object.name.len() >= 260 || object.name.contains(&0) {
            bail!("saved KX source name exceeds its native buffer");
        }
    }
    Ok(())
}
