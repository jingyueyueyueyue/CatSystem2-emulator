//! Campanella's original model transform tree (8e9c0/8ecc0).
//! Nodes belong to the live model/particle graph. A save never creates them.
use std::collections::BTreeMap;
use cs2_formats::state_tree::{StateNode, StateTree};
use crate::{RuntimeError, RuntimeResult};

const PAYLOAD: usize = 0x188;
const MAX_NODES: usize = 100_000;
const FIELDS: [usize; 11] = [4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 56];
type Matrix = [[f32; 4]; 4];

pub const IDENTITY_MODEL_TRANSFORM: [u32; 11] = [
    0, 0, 0, 0x3f800000, 0x3f800000, 0x3f800000, 0, 0, 0, 0x3f800000, 1,
];

/// Node+d4 and node+154 are live local channels, not the two optional
/// node+25c/+2dc banks serialized by 688. Commands371/372 fold and clear them.
#[derive(Clone, Debug)]
pub struct ModelLocalTransforms {
    pub primary: [u32; 11],
    pub secondary: [u32; 11],
}

impl Default for ModelLocalTransforms {
    fn default() -> Self {
        Self { primary: IDENTITY_MODEL_TRANSFORM, secondary: IDENTITY_MODEL_TRANSFORM }
    }
}

impl ModelLocalTransforms {
    /// 8a930/8aa00 -> 8a540: component position sum, scale product and
    /// quaternion product. This is not matrix decomposition or flag reset.
    pub fn freeze_primary(&mut self, saved: &mut ModelTransformOverrides, base: [u32; 11]) {
        saved.primary = Some(fold_transform(saved.primary.unwrap_or(base), self.primary));
        self.primary = IDENTITY_MODEL_TRANSFORM;
    }

    pub fn freeze_secondary(&mut self, saved: &mut ModelTransformOverrides) {
        saved.secondary = Some(match saved.secondary {
            Some(previous) => fold_transform(previous, self.secondary),
            None => self.secondary,
        });
        self.secondary = IDENTITY_MODEL_TRANSFORM;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelTransformOverrides {
    pub primary: Option<[u32; 11]>,
    pub secondary: Option<[u32; 11]>,
}

impl ModelTransformOverrides {
    /// The two serialized banks replace the model source and precede the
    /// parent's world matrix. Live local channels are applied by world_matrix.
    pub fn local_matrix(&self, base: Matrix) -> RuntimeResult<Matrix> {
        let primary = match &self.primary { Some(values) => matrix(values)?, None => base };
        match &self.secondary {
            Some(values) => Ok(multiply(primary, matrix(values)?)),
            None => Ok(primary),
        }
    }

    /// 8a7e0: local-primary * model/override * saved-secondary * parent-world
    /// * local-secondary. In particular the last channel follows the parent.
    pub fn world_matrix(&self, base: Matrix, parent: Matrix, local: &ModelLocalTransforms) -> RuntimeResult<Matrix> {
        let source = match &self.primary { Some(values) => matrix(values)?, None => base };
        let first = multiply(matrix(&local.primary)?, source);
        let inherited = match &self.secondary {
            Some(values) => multiply(matrix(values)?, parent),
            None => parent,
        };
        Ok(multiply(multiply(first, inherited), matrix(&local.secondary)?))
    }
}

/// Commands344/346 and369/370 exchange three padded vectors (48 bytes).
/// Quaternion bits are retained, including non-unit quaternions; validation
/// of finite geometry belongs to the actual render consumer.
pub fn decode_local_transform(bytes: &[u8]) -> RuntimeResult<[u32; 11]> {
    if bytes.len() < 48 { return Err(invalid("model local transform requires 48 bytes")); }
    let mut values = IDENTITY_MODEL_TRANSFORM;
    for (to, from) in [0, 4, 8, 16, 20, 24, 32, 36, 40, 44].into_iter().enumerate() {
        values[to] = word(bytes, from)?;
    }
    Ok(values)
}

pub fn encode_local_transform(values: &[u32; 11]) -> [u8; 48] {
    let mut bytes = [0; 48];
    for (value, to) in values.iter().zip([0, 4, 8, 16, 20, 24, 32, 36, 40, 44]) {
        put(&mut bytes, to, *value);
    }
    bytes
}

/// MDL's XYZ angles use92a70: build Rx*Ry*Rz, then extract a quaternion.
/// Matrix-to-quaternion uses the positive-trace/largest-diagonal branches;
/// it does not normalize, clamp or select a new sign after extraction.
pub fn model_transform(position: [f32; 3], scale: [f32; 3], angles: [f32; 3]) -> RuntimeResult<[u32; 11]> {
    if position.iter().chain(&scale).chain(&angles).any(|f| !f.is_finite()) {
        return Err(invalid("model source transform is non-finite"));
    }
    let mut rotation = identity_matrix();
    for (axis, angle) in angles.into_iter().enumerate() {
        let (s, c) = angle.sin_cos();
        let mut next = identity_matrix();
        let (a, b) = match axis { 0 => (1, 2), 1 => (2, 0), _ => (0, 1) };
        next[a][a] = c; next[a][b] = s; next[b][a] = -s; next[b][b] = c;
        rotation = multiply(rotation, next);
    }
    model_transform_from_rotation_matrix(position, scale, rotation)
}

/// Old model natives560/561 take an independent 4x4 rotation matrix, not
/// XYZ angles or a decomposable world transform. Preserve its quaternion
/// extraction and the separately supplied position/scale channels.
pub fn model_transform_from_rotation_matrix(
    position: [f32; 3], scale: [f32; 3], rotation: [[f32; 4]; 4],
) -> RuntimeResult<[u32; 11]> {
    if position.iter().chain(&scale).chain(rotation.iter().flatten()).any(|f| !f.is_finite()) {
        return Err(invalid("model rotation matrix or vector is non-finite"));
    }
    let mut q = [0.; 4];
    let trace = rotation[0][0] + rotation[1][1] + rotation[2][2];
    if trace > 0. {
        let t = (1. + trace).sqrt();
        q[3] = t * 0.5;
        let k = 0.5 / t;
        q[0] = (rotation[1][2] - rotation[2][1]) * k;
        q[1] = (rotation[2][0] - rotation[0][2]) * k;
        q[2] = (rotation[0][1] - rotation[1][0]) * k;
    } else {
        let mut i = 0;
        if rotation[1][1] > rotation[0][0] { i = 1; }
        if rotation[2][2] > rotation[i][i] { i = 2; }
        let j = (i + 1) % 3;
        let k = (i + 2) % 3;
        let t = (1. + rotation[i][i] - rotation[j][j] - rotation[k][k]).sqrt();
        q[i] = t * 0.5;
        let factor = 0.5 / t;
        q[j] = (rotation[i][j] + rotation[j][i]) * factor;
        q[k] = (rotation[i][k] + rotation[k][i]) * factor;
        q[3] = (rotation[j][k] - rotation[k][j]) * factor;
    }
    let mut values = IDENTITY_MODEL_TRANSFORM;
    for (output, f) in values[..10].iter_mut().zip(position.into_iter().chain(scale).chain(q)) {
        *output = f.to_bits();
    }
    Ok(values)
}

fn identity_matrix() -> Matrix {
    std::array::from_fn(|r| std::array::from_fn(|c| if r == c { 1. } else { 0. }))
}

fn fold_transform(base: [u32; 11], local: [u32; 11]) -> [u32; 11] {
    let a = base.map(|bits| f32::from_bits(bits) as f64);
    let b = local.map(|bits| f32::from_bits(bits) as f64);
    let mut out = IDENTITY_MODEL_TRANSFORM;
    for i in 0..3 { out[i] = ((a[i] + b[i]) as f32).to_bits(); }
    for i in 3..6 { out[i] = ((a[i] * b[i]) as f32).to_bits(); }
    // 8a540 passes local then base to D3DXQuaternionMultiply: base * local.
    let q = [
        a[9]*b[6] + a[6]*b[9] + a[7]*b[8] - a[8]*b[7],
        a[9]*b[7] - a[6]*b[8] + a[7]*b[9] + a[8]*b[6],
        a[9]*b[8] + a[6]*b[7] - a[7]*b[6] + a[8]*b[9],
        a[9]*b[9] - a[6]*b[6] - a[7]*b[7] - a[8]*b[8],
    ];
    for (out, value) in out[6..10].iter_mut().zip(q) { *out = (value as f32).to_bits(); }
    out
}

/// Allocation order. New siblings are inserted at their parent's head, so
/// the original lookup walks these indices backwards (94790).
#[derive(Clone, Debug)]
pub struct ModelStateNode {
    pub name: Vec<u8>,
    pub parent: Option<usize>,
    pub alive: bool,
    pub overrides: ModelTransformOverrides,
}

pub struct LegacyModelState {
    root: ModelTransformOverrides,
    nodes: Vec<ModelStateNode>,
    children: BTreeMap<Option<usize>, Vec<usize>>,
}

impl LegacyModelState {
    pub fn from_live_nodes(root: ModelTransformOverrides, nodes: Vec<ModelStateNode>) -> RuntimeResult<Self> {
        if nodes.len() > MAX_NODES { return Err(invalid("model state exceeds 100000 live records")); }
        let mut children: BTreeMap<Option<usize>, Vec<usize>> = BTreeMap::new();
        for (id, node) in nodes.iter().enumerate().filter(|(_, node)| node.alive) {
            if node.name.len() >= 128 || node.name.contains(&0) {
                return Err(invalid("live model name exceeds its 128-byte field"));
            }
            let mut at = node.parent;
            let mut depth = 0;
            while let Some(parent) = at {
                let parent = nodes.get(parent).filter(|node| node.alive)
                    .ok_or_else(|| invalid("live model parent is absent or released"))?;
                depth += 1;
                if depth >= 128 { return Err(invalid("live model parent chain is cyclic or excessive")); }
                at = parent.parent;
            }
            children.entry(node.parent).or_default().push(id);
        }
        for siblings in children.values_mut() { siblings.reverse(); }
        Ok(Self { root, nodes, children })
    }

    pub fn capture(&self) -> RuntimeResult<StateTree> {
        let mut flat = Vec::new();
        self.capture_overrides(b"", &self.root, &mut flat);
        self.capture_children(None, b"", &mut flat)?;
        // 8e9c0 inserts each saved record at the dummy root's head too.
        flat.reverse();
        Ok(StateTree { roots: vec![StateNode { payload: Vec::new(), children: flat }] })
    }

    fn capture_children(&self, parent: Option<usize>, prefix: &[u8], output: &mut Vec<StateNode>) -> RuntimeResult<()> {
        for &id in self.children.get(&parent).into_iter().flatten() {
            let node = &self.nodes[id];
            let mut path = prefix.to_vec();
            if !path.is_empty() { path.push(b'\\'); }
            path.extend_from_slice(&node.name);
            if path.len() >= 128 { return Err(invalid("model save path exceeds its 128-byte field")); }
            self.capture_overrides(&path, &node.overrides, output);
            self.capture_children(Some(id), &path, output)?;
        }
        Ok(())
    }

    fn capture_overrides(&self, path: &[u8], state: &ModelTransformOverrides, output: &mut Vec<StateNode>) {
        if state.primary.is_none() && state.secondary.is_none() { return; }
        let mut payload = vec![0; PAYLOAD];
        payload[..path.len()].copy_from_slice(path);
        for (bank, flag, begin) in [(&state.primary, 0x80, 0x88), (&state.secondary, 0x84, 0x108)] {
            // 92990 supplies identity for an inactive bank. The flag decides
            // whether restore applies it; no modern pointer is serialized.
            let values = bank.unwrap_or([0, 0, 0, 0x3f800000, 0x3f800000, 0x3f800000,
                0, 0, 0, 0x3f800000, 1]);
            put(&mut payload, flag, u32::from(bank.is_some()));
            for (&offset, value) in FIELDS.iter().zip(values) { put(&mut payload, begin + offset, value); }
            put(&mut payload, begin + 0x3c, 1);
        }
        output.push(StateNode { payload, children: Vec::new() });
    }

    /// Prepare all writes, including repeated entries for the same live node.
    /// Missing names are skipped; zero presence flags preserve active banks.
    pub fn restore(&mut self, tree: &StateTree) -> RuntimeResult<()> {
        let mut root = self.root.clone();
        let mut changes: BTreeMap<usize, ModelTransformOverrides> = BTreeMap::new();
        if let Some(saved_root) = tree.roots.first() {
            for saved in &saved_root.children {
                let path = c_string(&saved.payload)?;
                let Some(found) = self.resolve(path) else { continue; };
                if saved.payload.len() < PAYLOAD { return Err(invalid("original model transform payload is truncated")); }
                let next = match found {
                    Some(id) => changes.entry(id).or_insert_with(|| self.nodes[id].overrides.clone()),
                    None => &mut root,
                };
                for (bank, flag, begin) in [(&mut next.primary, 0x80, 0x88), (&mut next.secondary, 0x84, 0x108)] {
                    if word(&saved.payload, flag)? != 0 {
                        let mut values = [0; 11];
                        for (value, offset) in values.iter_mut().zip(FIELDS) { *value = word(&saved.payload, begin + offset)?; }
                        *bank = Some(values);
                    }
                }
            }
        }
        self.root = root;
        for (id, next) in changes { self.nodes[id].overrides = next; }
        Ok(())
    }

    pub fn into_overrides(self) -> (ModelTransformOverrides, Vec<ModelTransformOverrides>) {
        (self.root, self.nodes.into_iter().map(|node| node.overrides).collect())
    }

    /// View command373 -> 8b140 clears both presence flags on exactly the
    /// resolved object. It leaves the model transform and descendants alone.
    pub fn reset_overrides(&mut self, path: &[u8]) -> RuntimeResult<bool> {
        if path.len() >= 128 || path.contains(&0) {
            return Err(invalid("model reset path exceeds its 128-byte field"));
        }
        let Some(found) = self.resolve(path) else { return Ok(false); };
        match found {
            Some(id) => self.nodes[id].overrides = ModelTransformOverrides::default(),
            None => self.root = ModelTransformOverrides::default(),
        }
        Ok(true)
    }

    // Outer None is absent; Some(None) is the real dummy model root.
    fn resolve(&self, path: &[u8]) -> Option<Option<usize>> {
        let mut current: Option<usize> = None;
        for part in path.split(|&b| b == b'\\' || b == b'/').filter(|part| !part.is_empty()) {
            if part == b".." { current = self.nodes.get(current?)?.parent; }
            else {
                current = Some(*self.children.get(&current)?.iter().find(|&&id| self.nodes[id].name == part)?);
            }
        }
        Some(current)
    }
}

/// Construct the row-major D3DX rotation matrix without normalizing the quaternion.
pub fn model_rotation_matrix([x, y, z, w]: [f32; 4]) -> Matrix {
    [
        [1. - 2.*(y*y+z*z), 2.*(x*y+z*w), 2.*(x*z-y*w), 0.],
        [2.*(x*y-z*w), 1. - 2.*(x*x+z*z), 2.*(y*z+x*w), 0.],
        [2.*(x*z+y*w), 2.*(y*z-x*w), 1. - 2.*(x*x+y*y), 0.],
        [0., 0., 0., 1.],
    ]
}

fn matrix(values: &[u32; 11]) -> RuntimeResult<Matrix> {
    let f = values[..10].iter().map(|&bits| f32::from_bits(bits)).collect::<Vec<_>>();
    if f.iter().any(|value| !value.is_finite()) { return Err(invalid("model override contains non-finite render geometry")); }
    // 92cc0: quaternion rotation, then right-multiplied scale and translation.
    let mut m = model_rotation_matrix([f[6], f[7], f[8], f[9]]);
    for row in &mut m { for axis in 0..3 { row[axis] *= f[3 + axis]; } }
    m[3][..3].copy_from_slice(&f[..3]);
    if m.iter().flatten().any(|value| !value.is_finite()) { return Err(invalid("model override matrix overflows render geometry")); }
    Ok(m)
}

fn multiply(a: Matrix, b: Matrix) -> Matrix {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..4).map(|k| a[r][k] * b[k][c]).sum()))
}
fn word(bytes: &[u8], at: usize) -> RuntimeResult<u32> {
    let raw = bytes.get(at..at + 4).ok_or_else(|| invalid("original model state DWORD is truncated"))?;
    Ok(u32::from_le_bytes(raw.try_into().unwrap()))
}
fn put(bytes: &mut [u8], at: usize, value: u32) { bytes[at..at + 4].copy_from_slice(&value.to_le_bytes()); }
fn c_string(bytes: &[u8]) -> RuntimeResult<&[u8]> {
    bytes.iter().take(128).position(|&b| b == 0).map(|end| &bytes[..end])
        .ok_or_else(|| invalid("original model state name is not terminated within 128 bytes"))
}
fn invalid(message: &str) -> RuntimeError { RuntimeError::Host(message.into()) }
