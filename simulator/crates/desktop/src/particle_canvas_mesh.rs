//! Owned MDL primitive/material data, independently decoded from 99730/888d5.
use std::collections::{BTreeMap, BTreeSet};
use super::{floats, string, word};

#[derive(Debug)]
pub(super) struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 4],
    pub uv: [f32; 2],
}
#[derive(Debug)]
pub(super) struct Primitive {
    pub material: Vec<u8>,
    pub fvf: u32,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u16>,
}
#[derive(Debug)]
pub(super) struct Material {
    pub diffuse: [f32; 4],
    pub shader: Vec<u8>,
    pub textures: Vec<Vec<u8>>,
}

pub(super) fn materials(bytes: &[u8], start: usize, end: usize) -> anyhow::Result<BTreeMap<Vec<u8>, Material>> {
    let mut out = BTreeMap::new();
    let mut at = start;
    while at < end {
        if end - at < 0xe4 || out.len() >= 65536 { anyhow::bail!("MDL material record exceeds chunk101"); }
        let count = word(bytes, at + 0xe0)? as usize;
        let next = count.checked_mul(32).and_then(|size| at.checked_add(0xe4 + size))
            .filter(|&next| next <= end).ok_or_else(|| anyhow::anyhow!("MDL material textures exceed chunk101"))?;
        if count > 8 { anyhow::bail!("MDL material has more than eight texture stages"); }
        let textures = bytes[at + 0xe4..next].chunks_exact(32).map(string).collect::<anyhow::Result<_>>()?;
        let name = string(&bytes[at + 0x80..at + 0xc0])?;
        if out.insert(name, Material { diffuse: floats(bytes, at + 0x20)?,
            shader: string(&bytes[at + 0xc0..at + 0xe0])?, textures }).is_some() {
            anyhow::bail!("MDL duplicate material identity");
        }
        at = next;
    }
    Ok(out)
}

pub(super) fn record(bytes: &[u8], at: usize, end: usize) -> anyhow::Result<&[u8]> {
    let size = word(bytes, at + 8)? as usize;
    let start = at.checked_add(12).ok_or_else(|| anyhow::anyhow!("MDL record offset overflow"))?;
    let stop = start.checked_add(size).filter(|&stop| stop <= end)
        .ok_or_else(|| anyhow::anyhow!("MDL typed payload exceeds graph chunk"))?;
    Ok(&bytes[start..stop])
}
pub(super) fn children(bytes: &[u8], at: usize, end: usize) -> anyhow::Result<Vec<usize>> {
    let size = record(bytes, at, end)?.len();
    let mut relative = word(bytes, at + 4)? as usize;
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    while relative != 0 {
        let child = at.checked_add(relative).filter(|&child| child >= at + 12 + size && child < end)
            .ok_or_else(|| anyhow::anyhow!("MDL typed child link leaves graph or overlaps parent"))?;
        if !seen.insert(child) || seen.len() > 100_000 { anyhow::bail!("MDL typed sibling cycle/limit"); }
        record(bytes, child, end)?;
        out.push(child);
        let next = word(bytes, child)? as usize;
        if next == 0 { break; }
        relative = relative.checked_add(next).ok_or_else(|| anyhow::anyhow!("MDL typed sibling overflow"))?;
    }
    Ok(out)
}

pub(super) fn primitives(bytes: &[u8], container: usize, end: usize) -> anyhow::Result<Vec<Primitive>> {
    let mut out = Vec::new();
    let mut total_vertices = 0usize;
    for at in children(bytes, container, end)? {
        let raw = record(bytes, at, end)?;
        // 99730 reads the material at +4 and geometry at +0x44 onward.
        // The first DWORD is unused here and varies in the original assets.
        if raw.len() != 0x54 { anyhow::bail!("MDL unsupported mesh primitive record"); }
        let vertex_count = word(raw, 0x44)? as usize;
        let index_count = word(raw, 0x48)? as usize;
        let fvf = word(raw, 0x4c)?;
        let stride = word(raw, 0x50)? as usize;
        total_vertices = total_vertices.checked_add(vertex_count).ok_or_else(|| anyhow::anyhow!("MDL vertex count overflow"))?;
        if vertex_count > 65536 || total_vertices > 2_000_000 || index_count > 6_000_000 || index_count % 3 != 0 {
            anyhow::bail!("MDL triangle-list count exceeds bounds");
        }
        let expected_stride = match fvf { 0x112 => 32, 0x152 => 36, 0x52 => 28,
            _ => anyhow::bail!("MDL mesh FVF {fvf:#x} has no recovered vertex layout") };
        if stride != expected_stride { anyhow::bail!("MDL FVF/stride mismatch"); }
        let mut vertices = None;
        let mut indices = None;
        for buffer in children(bytes, at, end)? {
            let data = record(bytes, buffer, end)?;
            match word(data, 0)? {
                0 => {
                    if vertices.is_some() || data.len() != 4 + vertex_count * stride { anyhow::bail!("MDL vertex-buffer length/identity mismatch"); }
                    let mut parsed = Vec::with_capacity(vertex_count);
                    for v in data[4..].chunks_exact(stride) {
                        let mut offset = 24;
                        let mut color = [1.; 4];
                        if fvf & 0x40 != 0 {
                            let packed = word(v, offset)?; offset += 4;
                            color = [16,8,0,24].map(|shift| ((packed >> shift) & 255) as f32 / 255.);
                        }
                        parsed.push(Vertex { position: floats(v,0)?,normal: floats(v,12)?,color,
                            uv: if fvf & 0x100 != 0 { floats(v,offset)? } else { [0.;2] } });
                    }
                    vertices = Some(parsed);
                }
                1 => {
                    if indices.is_some() || data.len() != 4 + index_count * 2 { anyhow::bail!("MDL index-buffer length/identity mismatch"); }
                    let parsed = data[4..].chunks_exact(2).map(|b| u16::from_le_bytes([b[0],b[1]])).collect::<Vec<_>>();
                    if parsed.iter().any(|&index| index as usize >= vertex_count) { anyhow::bail!("MDL triangle index leaves vertex buffer"); }
                    indices = Some(parsed);
                }
                kind => anyhow::bail!("MDL mesh buffer type {kind} is not recovered"),
            }
        }
        out.push(Primitive { material: string(&raw[4..0x44])?,fvf,
            vertices: vertices.ok_or_else(|| anyhow::anyhow!("MDL primitive lacks vertex buffer"))?,
            indices: indices.ok_or_else(|| anyhow::anyhow!("MDL primitive lacks index buffer"))? });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh_with_triangle(first_word: u32) -> Vec<u8> {
        const CONTAINER: usize = 0;
        const PRIMITIVE: usize = 16;
        const INDEX_BUFFER: usize = PRIMITIVE + 12 + 0x54;
        const VERTEX_BUFFER: usize = INDEX_BUFFER + 12 + 4 + 6;
        const STRIDE: usize = 36;
        let mut bytes = vec![0; VERTEX_BUFFER + 12 + 4 + 3 * STRIDE];
        for (offset, value) in [
            (CONTAINER + 4, PRIMITIVE as u32),
            (CONTAINER + 8, 4),
            (CONTAINER + 12, 1),
            (PRIMITIVE + 4, (INDEX_BUFFER - PRIMITIVE) as u32),
            (PRIMITIVE + 8, 0x54),
            (PRIMITIVE + 12, first_word),
            (PRIMITIVE + 12 + 0x44, 3),
            (PRIMITIVE + 12 + 0x48, 3),
            (PRIMITIVE + 12 + 0x4c, 0x152),
            (PRIMITIVE + 12 + 0x50, STRIDE as u32),
            (INDEX_BUFFER, (VERTEX_BUFFER - INDEX_BUFFER) as u32),
            (INDEX_BUFFER + 8, 10),
            (INDEX_BUFFER + 12, 1),
            (VERTEX_BUFFER + 8, (4 + 3 * STRIDE) as u32),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[PRIMITIVE + 16..PRIMITIVE + 24].copy_from_slice(b"material");
        bytes[INDEX_BUFFER + 16..INDEX_BUFFER + 22].copy_from_slice(&[0, 0, 1, 0, 2, 0]);
        bytes
    }

    #[test]
    fn primitive_accepts_nonzero_first_word() {
        for first_word in [0, 1, 7] {
            let bytes = mesh_with_triangle(first_word);
            let meshes = primitives(&bytes, 0, bytes.len()).unwrap();
            assert_eq!(meshes.len(), 1);
            let mesh = &meshes[0];
            assert_eq!(mesh.material, b"material");
            assert_eq!(mesh.fvf, 0x152);
            assert_eq!(mesh.vertices.len(), 3);
            assert_eq!(mesh.indices, [0, 1, 2]);
        }
    }

    #[test]
    fn primitive_rejects_invalid_record_length() {
        let mut bytes = mesh_with_triangle(1);
        bytes[24..28].copy_from_slice(&0x50_u32.to_le_bytes());
        assert_eq!(
            primitives(&bytes, 0, bytes.len()).unwrap_err().to_string(),
            "MDL unsupported mesh primitive record"
        );
    }
}
