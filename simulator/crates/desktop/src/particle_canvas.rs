//! Campanella kcTask3DView: independent MDL locator/camera graph and CPU sink.
//! Evidence is SHA-bound in particle_external; no D3D pointer or DLL is used.
use std::collections::{BTreeMap, BTreeSet};
use cs2_renderer::Canvas;
use cs2_runtime::model_state::{ModelLocalTransforms, IDENTITY_MODEL_TRANSFORM, model_transform};
#[path = "particle_canvas_mesh.rs"]
mod mesh;
#[path = "particle_canvas_shader.rs"]
mod shader;
#[path = "particle_canvas_raster.rs"]
mod raster;
#[path = "model_draw_queue.rs"]
mod draw_queue;
#[path = "particle_canvas_motion.rs"]
mod motion;
#[path = "particle_canvas_fog.rs"]
mod fog;
#[path = "particle_canvas_color.rs"]
mod color;

pub(super) type Matrix = [[f32; 4]; 4];
pub(super) fn identity() -> Matrix {
    std::array::from_fn(|r| std::array::from_fn(|c| if r == c { 1. } else { 0. }))
}
fn multiply(a: Matrix, b: Matrix) -> Matrix {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..4).map(|k| a[r][k] * b[k][c]).sum()))
}
fn transform(v: [f32; 4], m: Matrix) -> [f32; 4] {
    std::array::from_fn(|c| (0..4).map(|r| v[r] * m[r][c]).sum())
}

#[derive(Debug)]
pub(super) struct GraphNode {
    pub name: Vec<u8>,
    pub kind: u32,
    pub parent: Option<usize>,
    pub local: Matrix,
    pub base_transform: [u32; 11],
    pub local_transforms: ModelLocalTransforms,
    // 89160/+74 is a string property count, not animation keys. Motion
    // sources are installed separately by command360 (8ce40).
    pub properties: BTreeMap<Vec<u8>, Vec<u8>>,
    pub visible: u32,
    pub alive: bool,
    pub particle: Option<u32>,
    pub overrides: cs2_runtime::model_state::ModelTransformOverrides,
    motion: motion::MotionSource,
    material_color: color::Color,
    camera: Option<[f32; 5]>,
    meshes: Vec<mesh::Primitive>,
    // A light is not geometry. Keep its original descriptor until its typed
    // directional/point/spot binding is available, never substitute white.
    light: Option<Vec<u8>>,
}
impl GraphNode {
    pub(super) fn is_camera(&self) -> bool { self.camera.is_some() }
    pub(super) fn set_material_color(&mut self, packed: u32) { self.material_color.set(packed); }
    pub(super) fn packed_material_color(&self) -> u32 { self.material_color.packed() }
}

#[derive(Debug, Default)]
pub(super) struct ParticleCanvas {
    pub configured: bool,
    pub texture_directory: Vec<u8>,
    pub texture_archive: Vec<u8>,
    pub depth: f32,
    pub nodes: Vec<GraphNode>,
    pub scene_name: Vec<u8>,
    pub loaded: bool,
    pub selected_camera: Option<usize>,
    pub root_overrides: cs2_runtime::model_state::ModelTransformOverrides,
    pub root_local_transforms: ModelLocalTransforms,
    pub root_visible: u32,
    // Manager+284 survives a graph reload; ctor8c6a7 initializes it to1.
    pub render_enabled: Option<u32>,
    assets: shader::Assets,
    motions: motion::Clips,
    // Manager state survives graph reload, like48c720 (which does not
    // clear+248..260). Full task release drops it with the same owner.
    fog: fog::FogState,
    pub(super) elapsed: u32,
}

fn word(bytes: &[u8], at: usize) -> anyhow::Result<u32> {
    Ok(u32::from_le_bytes(bytes.get(at..at + 4)
        .ok_or_else(|| anyhow::anyhow!("MDL record exceeds resource bounds"))?.try_into()?))
}
fn floats<const N: usize>(bytes: &[u8], at: usize) -> anyhow::Result<[f32; N]> {
    let mut out = [0.; N];
    for (i, value) in out.iter_mut().enumerate() {
        *value = f32::from_bits(word(bytes, at + i * 4)?);
        if !value.is_finite() { anyhow::bail!("MDL contains non-finite transform/camera data"); }
    }
    Ok(out)
}
fn string(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let end = bytes.iter().position(|&b| b == 0)
        .ok_or_else(|| anyhow::anyhow!("MDL name lacks a bounded terminator"))?;
    Ok(bytes[..end].to_vec())
}
fn local_matrix(position: [f32; 3], scale: [f32; 3], angles: [f32; 3]) -> Matrix {
    // 92a70 appends X/Y/Z rotations; 92cc0 appends scale then translation.
    let mut m = identity();
    for (axis, angle) in angles.into_iter().enumerate() {
        let (s, c) = angle.sin_cos();
        let mut r = identity();
        let (a, b) = match axis { 0 => (1, 2), 1 => (2, 0), _ => (0, 1) };
        r[a][a] = c; r[a][b] = s; r[b][a] = -s; r[b][b] = c;
        m = multiply(m, r);
    }
    for row in &mut m { for axis in 0..3 { row[axis] *= scale[axis]; } }
    m[3][..3].copy_from_slice(&position);
    m
}

impl ParticleCanvas {
    pub(super) fn clear_graph(&mut self) {
        self.nodes.clear(); self.selected_camera = None; self.loaded = false; self.scene_name.clear();
        self.root_overrides = Default::default();
        self.root_local_transforms = Default::default();
        self.root_visible = 1;
        self.assets = Default::default();
        self.motions = Default::default();
        self.elapsed = 0;
    }
    pub(super) fn load(&mut self, name: Vec<u8>, bytes: &[u8]) -> anyhow::Result<()> {
        // 89120: chunks start at8, tag/count/next-length are three DWORDs.
        if bytes.get(..8) != Some(b"MDL\0\0\0\x01\0") { anyhow::bail!("unsupported MDL signature/version"); }
        let mut offset = 8usize;
        let mut root = None;
        let mut materials = BTreeMap::new();
        let mut motions = motion::Clips::default();
        while offset < bytes.len() {
            let tag = word(bytes, offset)?;
            let length = word(bytes, offset + 8)? as usize;
            // A final motion chunk104 can have a zero next-length.
            let length = if length == 0 { bytes.len() - offset } else { length };
            if length < 12 || offset.checked_add(length).is_none_or(|end| end > bytes.len()) {
                anyhow::bail!("MDL chunk length is invalid");
            }
            if tag == 100 {
                if root.replace((offset + 12, offset + length)).is_some() { anyhow::bail!("MDL duplicate graph chunk100"); }
            } else if tag == 101 {
                for (name, material) in mesh::materials(bytes,offset+12,offset+length)? {
                    if materials.insert(name,material).is_some() {anyhow::bail!("MDL duplicate material identity across chunks");}
                }
            } else if tag == 104 {
                motions.read_chunk(bytes, offset + 12, offset + length)?;
            }
            offset += length;
        }
        let (root, end) = root.ok_or_else(|| anyhow::anyhow!("MDL lacks its graph chunk100"))?;
        let mut nodes = Vec::new();
        let mut stack = vec![(root, None, 0usize)];
        let mut visited = BTreeSet::new();
        while let Some((at, parent, nesting)) = stack.pop() {
            if nesting > 256 || visited.len() >= 100_000 || nodes.len() >= 65536 || !visited.insert(at) {
                anyhow::bail!("MDL graph exceeds bounds or contains a cyclic relative link");
            }
            let size = word(bytes, at + 8)? as usize;
            if at.checked_add(12).and_then(|start| start.checked_add(size)).is_none_or(|last| last > end) {
                anyhow::bail!("MDL node exceeds graph chunk");
            }
            let raw = &bytes[at + 12..at + 12 + size];
            // Payload siblings (mesh buffers, light/camera data) share the
            // wire tree but 89160 only creates objects for record type0.
            // They must neither become nodes nor swallow later siblings.
            if word(raw, 0)? != 0 {
                let sibling = word(bytes, at)? as usize;
                if sibling != 0 { stack.push((at.checked_add(sibling).ok_or_else(|| anyhow::anyhow!("MDL sibling overflow"))?, parent, nesting)); }
                continue;
            }
            let kind = word(raw, 0x48)?;
            if kind > 3 { anyhow::bail!("MDL object type is not identified"); }
            let mut camera = None;
            let mut meshes = Vec::new();
            let mut light = None;
            let mut child = word(bytes, at + 4)? as usize;
            if child != 0 {
                let mut child_at = at.checked_add(child).ok_or_else(|| anyhow::anyhow!("MDL child overflow"))?;
                let mut siblings = BTreeSet::new();
                loop {
                    if !siblings.insert(child_at) || siblings.len() > 100_000 { anyhow::bail!("MDL cyclic/excessive payload siblings"); }
                    if child_at < at + 12 + size || child_at >= end { anyhow::bail!("MDL child link overlaps its parent data or leaves chunk"); }
                    let child_size = word(bytes, child_at + 8)? as usize;
                    if child_at.checked_add(12).and_then(|start| start.checked_add(child_size)).is_none_or(|last| last > end) {
                        anyhow::bail!("MDL child record exceeds parent bounds");
                    }
                    let record = &bytes[child_at + 12..child_at + 12 + child_size];
                    if word(record, 0)? == 3 && kind == 3 {
                        // 89411..89440 copies +0c twice, exactly as this PE.
                        let f = floats::<5>(record, 4)?;
                        camera = Some([f[0], f[1], f[2], f[2], f[4]]);
                    } else if word(record,0)? == 1 && kind == 1 {
                        meshes.extend(mesh::primitives(bytes,child_at,end)?);
                    } else if word(record,0)? == 2 && kind == 2 {
                        if record.len()<0x30 || light.replace(record.to_vec()).is_some() {anyhow::bail!("MDL light payload is truncated/duplicated");}
                    }
                    child = word(bytes, child_at)? as usize;
                    if child == 0 { break; }
                    child_at = child_at.checked_add(child).ok_or_else(|| anyhow::anyhow!("MDL sibling overflow"))?;
                }
            }
            let node_index = nodes.len();
            let position = floats(raw, 0x4c)?;
            let scale = floats(raw, 0x58)?;
            let angles = floats(raw, 0x64)?;
            let count = word(raw, 0x74)? as usize;
            let property_end = count.checked_mul(64).and_then(|n| n.checked_add(0x78))
                .filter(|&n| n <= raw.len()).ok_or_else(|| anyhow::anyhow!("MDL properties exceed object record"))?;
            let mut properties = BTreeMap::new();
            for record in raw[0x78..property_end].chunks_exact(64) {
                // 95f00 maps the first bounded name to the second string;
                // uninitialized bytes after either NUL are not data.
                let key = string(&record[..32])?;
                let value = string(&record[32..])?;
                properties.insert(key, value);
            }
            let mut base_transform = model_transform(position, scale, angles)?;
            base_transform[10] = word(raw, 0x70)?;
            // 492a70 stores the Euler-derived quaternion in the source;
            // 48a819 -> source+14 -> 492cc0 draws that stored transform.
            // Use the same quaternion matrix as motion/command371, so the
            // initial pose does not take a separate direct-Euler path.
            let local = cs2_runtime::model_state::ModelTransformOverrides {
                primary: Some(base_transform), secondary: None,
            }.local_matrix(identity())?;
            // 494250 reads this source's local "disp" property; 494403..
            // 494414 initializes the same live draw word as message336.
            // Keep the source/header draw component for command371 intact.
            let visible=if properties.contains_key(b"disp".as_slice()) {property_number(&properties,b"disp")?}
                else if properties.contains_key(b"e_draw".as_slice()) {property_number(&properties,b"e_draw")?}
                else {word(raw,0x70)?};
            nodes.push(GraphNode {
                name: string(raw.get(8..0x48).ok_or_else(|| anyhow::anyhow!("MDL object name truncated"))?)?,
                kind,
                parent, local,
                base_transform,
                local_transforms: Default::default(),
                properties,
                visible,
                alive: true, particle: None, camera,
                overrides: Default::default(),
                motion: Default::default(),
                material_color: Default::default(),
                meshes, light,
            });
            let sibling = word(bytes, at)? as usize;
            if sibling != 0 { stack.push((at.checked_add(sibling).ok_or_else(|| anyhow::anyhow!("MDL sibling overflow"))?, parent, nesting)); }
            // In serialized MDL the next tree level includes typed payloads.
            // 945f0 only instantiates type0 object children; payloads are skipped.
            let mut child = word(bytes, at + 4)? as usize;
            let mut children = Vec::new();
            let mut siblings = BTreeSet::new();
            while child != 0 {
                let child_at = at.checked_add(child).ok_or_else(|| anyhow::anyhow!("MDL child overflow"))?;
                if !siblings.insert(child_at) || siblings.len() > 100_000 || child_at < at + 12 + size || child_at + 12 > end {
                    anyhow::bail!("MDL object child link is cyclic or outside graph");
                }
                if word(bytes, child_at + 12)? == 0 { children.push(child_at); }
                let next = word(bytes, child_at)? as usize;
                if next == 0 { break; }
                child = child.checked_add(next).ok_or_else(|| anyhow::anyhow!("MDL sibling overflow"))?;
            }
            // Sibling links are already followed by the stack above.
            if let Some(first) = children.first() { stack.push((*first, Some(node_index), nesting + 1)); }
        }
        // 1fe80 always inserts a separate default camera after MDL loading.
        let default = nodes.len();
        nodes.push(GraphNode { name: b"default".to_vec(), kind:3, parent: None, local: identity(), alive: true,
            base_transform: IDENTITY_MODEL_TRANSFORM, local_transforms: Default::default(),
            properties: BTreeMap::new(),
            visible: 1,
            overrides: Default::default(),
            motion: Default::default(),
            material_color: Default::default(),
            particle: None, camera: Some([0., f32::from_bits(0x3f20_d97c), 4. / 3., 1., 10000.]), meshes:Vec::new(), light:None });
        let total_vertices=nodes.iter().flat_map(|n|&n.meshes).map(|p|p.vertices.len()).sum::<usize>();
        if total_vertices>2_000_000 {anyhow::bail!("MDL graph mesh working set exceeds two million vertices");}
        self.assets=Default::default();
        self.assets.materials=materials;
        self.motions=motions;
        self.nodes = nodes;
        self.root_overrides = Default::default();
        self.root_local_transforms = Default::default();
        self.root_visible = 1;
        self.selected_camera = Some(default);
        self.scene_name = name;
        self.loaded = true;
        Ok(())
    }

    pub(super) fn prepare_assets(&mut self, resources:&cs2_runtime::ResourceIndex, encoding:crate::ScriptTextEncoding, texture_policy:[u32;2])->anyhow::Result<Vec<String>> {
        // A locator-only MDL (battlefield) can later receive669/type5 lists.
        // Its PL programs must exist before the first real particle submission.
        if self.loaded {
            if !self.configured {anyhow::bail!("MDL mesh texture manager has not received command332");}
            let config=resources.startup_configuration()?.ok_or_else(||anyhow::anyhow!("MDL texture manager lacks startup routing"))?;
            let mut matching=false;
            for slot in 0..128 {
                let key=format!("int{slot:02}");
                if !config.text(&[b"document",b"INT",key.as_bytes(),b"name"]).unwrap_or_default().eq_ignore_ascii_case(b"texture") {continue;}
                matching=config.text(&[b"document",b"INT",key.as_bytes(),b"dir"]).unwrap_or_default()==self.texture_directory
                    &&config.text(&[b"document",b"INT",key.as_bytes(),b"int"]).unwrap_or_default()==self.texture_archive;
                break;
            }
            if !matching {anyhow::bail!("MDL dynamic texture manager configuration needs its explicit resource mount");}
            return self.assets.prepare(resources,encoding,texture_policy);
        }
        Ok(Vec::new())
    }

    pub(super) fn resolve(&self, path: &[u8]) -> Option<usize> {
        self.resolve_object(path).flatten()
    }

    /// Some(None) is the actual dummy root, distinct from an absent path.
    pub(super) fn resolve_object(&self, path: &[u8]) -> Option<Option<usize>> {
        let mut parent: Option<usize> = None;
        for part in path.split(|b| matches!(b, b'/' | b'\\')).filter(|p| !p.is_empty()) {
            if part == b".." { parent = self.nodes.get(parent?)?.parent; }
            else { parent = self.nodes.iter().enumerate().rev()
                .find(|(_, n)| n.alive && n.parent == parent && n.name == part).map(|(i, _)| i); parent?; }
        }
        Some(parent)
    }

    pub(super) fn add_particle(&mut self, target: u32, path: &[u8]) -> usize {
        let parent = self.resolve(path);
        let index = self.nodes.len();
        self.nodes.push(GraphNode { name: format!("pl{}", target as i32).into_bytes(), kind:5, parent,
            base_transform: IDENTITY_MODEL_TRANSFORM, local_transforms: Default::default(),
            properties: BTreeMap::new(),
            visible: 1,
            overrides: Default::default(),
            motion: Default::default(),
            material_color: Default::default(),
            local: identity(), alive: true, particle: Some(target), camera: None, meshes:Vec::new(), light:None });
        index
    }

    pub(super) fn remove_particle(&mut self, target: u32) -> Vec<u32> {
        let mut removed = BTreeSet::new();
        for (i, node) in self.nodes.iter_mut().enumerate() {
            if node.particle == Some(target) { node.alive = false; removed.insert(i); }
        }
        // 93fd0 ->94850 recursively deletes children, never reparents them.
        let mut ids = vec![target];
        for _ in 0..=self.nodes.len() {
            let mut changed = false;
            for (i, node) in self.nodes.iter_mut().enumerate() {
                if node.alive && node.parent.is_some_and(|p| removed.contains(&p)) {
                    node.alive = false; removed.insert(i); changed = true;
                    if let Some(id) = node.particle { ids.push(id); }
                }
            }
            if !changed { break; }
        }
        ids
    }

    fn world(&self, index: usize) -> anyhow::Result<Matrix> {
        let mut chain = Vec::new();
        let mut at = Some(index);
        let mut depth = 0;
        while let Some(i) = at {
            let node = self.nodes.get(i).filter(|n| n.alive).ok_or_else(|| anyhow::anyhow!("kcView graph node has been released"))?;
            // A parent's geometry type does not invalidate its transform.
            // 8a7e0 always composes the same model/local/override channels.
            chain.push(i); at = node.parent; depth += 1;
            if depth > 256 { anyhow::bail!("kcView graph depth exceeds256"); }
        }
        let mut result = self.root_overrides.world_matrix(identity(), identity(), &self.root_local_transforms)?;
        for i in chain.into_iter().rev() {
            let node = &self.nodes[i];
            let source = if node.overrides.primary.is_some() {node.local}
                else {node.motion.matrix(&self.motions, node.local, &node.base_transform)?};
            result = node.overrides.world_matrix(source, result, &node.local_transforms)?;
        }
        Ok(result)
    }

    pub(super) fn bind_motion(&mut self, index: usize, name: Option<&[u8]>) {
        self.nodes[index].motion.bind(&self.motions, name);
    }

    pub(super) fn configure_fog(&mut self, mode: u32, enabled: u32, record: Option<&[u8]>) -> anyhow::Result<()> {
        self.fog.configure(mode, enabled, record)
    }

    pub(super) fn play_motion(&mut self, index: usize, words: [u32; 4]) -> anyhow::Result<()> {
        self.nodes[index].motion.play(self.elapsed, words)
    }

    pub(super) fn motion_pending(&self, index: usize) -> u32 {
        self.nodes[index].motion.pending()
    }

    pub(super) fn motion_end_word(&self, index: usize) -> u32 {
        self.nodes[index].motion.end_word()
    }

    /// Command371's component fold must see the same sampled source as draw,
    /// before mutably borrowing the local/override banks. No matrix decomposition.
    pub(super) fn source_transform(&self, index: usize) -> anyhow::Result<[u32; 11]> {
        let node=&self.nodes[index];
        Ok(node.motion.transform(&self.motions,&node.base_transform)?.unwrap_or(node.base_transform))
    }

    pub(super) fn advance(&mut self, elapsed: u32) -> anyhow::Result<()> {
        let mut prepared=Vec::new();
        for (index,node) in self.nodes.iter().enumerate().filter(|(_,node)|node.alive) {
            let mut motion=node.motion.clone();
            motion.advance(elapsed)?;
            prepared.push((index,motion));
        }
        for (index,motion) in prepared {self.nodes[index].motion=motion;}
        self.elapsed = elapsed;
        Ok(())
    }

    pub(super) fn visible(&self, index: usize) -> anyhow::Result<bool> {
        if self.root_visible == 0 || self.render_enabled == Some(0) { return Ok(false); }
        let mut at = Some(index);
        for _ in 0..=256 {
            let Some(i) = at else { return Ok(true); };
            let node = self.nodes.get(i).filter(|n| n.alive)
                .ok_or_else(|| anyhow::anyhow!("kcView visibility references a released node"))?;
            if node.visible == 0 { return Ok(false); }
            at = node.parent;
        }
        anyhow::bail!("kcView visibility parent chain exceeds256")
    }

    pub(super) fn draw(&self, output: &mut Canvas,
        objects: &BTreeMap<u32, super::particle_external::ExternalParticle>,
        particles: &BTreeMap<u32, super::ParticleObjectState>, owner: u32, only_particle: Option<u32>,
    ) -> anyhow::Result<()> {
        if !self.loaded || self.render_enabled == Some(0) || self.root_visible == 0 { return Ok(()); }
        let camera = self.selected_camera.ok_or_else(|| anyhow::anyhow!("kcView has no selected camera"))?;
        let params = self.nodes[camera].camera.ok_or_else(|| anyhow::anyhow!("selected graph object is not a camera"))?;
        let view = inverse(self.world(camera)?)?;
        let [_, fov, aspect, near, far] = params;
        if !(fov > 0. && fov < std::f32::consts::PI && aspect > 0. && near > 0. && far > near) {
            anyhow::bail!("kcView camera projection is invalid");
        }
        let y = 1. / (fov * 0.5).tan();
        let z = far / (near - far);
        let projection = [[y / aspect,0.,0.,0.],[0.,y,0.,0.],[0.,0.,z,-1.],[0.,0.,z * near,0.]];
        draw_queue::draw(self, output, objects, particles, owner, only_particle, view, projection, camera)?;
        Ok(())
    }
}

fn property_number(properties:&BTreeMap<Vec<u8>,Vec<u8>>,key:&[u8])->anyhow::Result<u32> {
    let Some(value)=properties.get(key) else {return Ok(0);};
    // 472070 is atoi: leading space/sign, decimal prefix, DWORD wrap.
    let mut at=0;
    while value.get(at).is_some_and(u8::is_ascii_whitespace) {at+=1;}
    let negative=value.get(at)==Some(&b'-');
    if matches!(value.get(at),Some(b'-'|b'+')) {at+=1;}
    let mut number=0u32;
    while let Some(&b)=value.get(at).filter(|b|b.is_ascii_digit()) {
        number=number.wrapping_mul(10).wrapping_add(u32::from(b-b'0'));at+=1;
    }
    Ok(if negative {number.wrapping_neg()} else {number})
}

fn inverse(m: Matrix) -> anyhow::Result<Matrix> {
    let mut a = [[0f64; 8]; 4];
    for r in 0..4 { for c in 0..4 { a[r][c] = f64::from(m[r][c]); } a[r][r + 4] = 1.; }
    for c in 0..4 {
        let pivot = (c..4).max_by(|&r, &s| a[r][c].abs().total_cmp(&a[s][c].abs())).unwrap();
        if a[pivot][c] == 0. { anyhow::bail!("kcView camera transform is singular"); }
        a.swap(c,pivot); let divisor = a[c][c]; for value in &mut a[c] { *value /= divisor; }
        let row = a[c];
        for r in 0..4 { if r != c { let k = a[r][c]; for j in 0..8 { a[r][j] -= k * row[j]; } } }
    }
    Ok(std::array::from_fn(|r| std::array::from_fn(|c| a[r][c + 4] as f32)))
}
