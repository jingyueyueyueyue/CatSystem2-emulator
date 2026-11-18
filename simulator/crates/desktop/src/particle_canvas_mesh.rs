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
        if raw.len() != 0x54 || word(raw, 0)? != 0 { anyhow::bail!("MDL unsupported mesh primitive record"); }
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
