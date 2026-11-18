//! Portable KX2 file records, independently decoded from the public format.
//! Coordinates remain DWORDs so snapshots preserve NaN payloads and signs.

use cs2_script::{Result, ScriptError};
use serde::{Deserialize, Serialize};

const MAX_BYTES: usize = 64 * 1024 * 1024;

fn invalid(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KxVertex {
    pub position: [u32; 3],
    pub argb: u32,
}

impl KxVertex {
    pub fn xyz(self) -> [f32; 3] {
        self.position.map(f32::from_bits)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KxMesh {
    pub bounds: [u32; 4],
    pub triangles: Vec<[KxVertex; 3]>,
}

impl KxMesh {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(invalid("KX geometry resource exceeds 64 MiB"));
        }
        if bytes.get(..4) != Some(b"KX2\0") {
            return Err(invalid(
                "KX geometry has an unrecovered or invalid signature",
            ));
        }
        let header = bytes
            .get(..24)
            .ok_or_else(|| invalid("KX2 header is truncated"))?;
        let word =
            |offset: usize| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
        let count = word(20) as usize;
        let length = count
            .checked_mul(48)
            .and_then(|n| n.checked_add(24))
            .filter(|&n| n <= MAX_BYTES)
            .ok_or_else(|| invalid("KX2 triangle storage exceeds its limit"))?;
        let data = bytes
            .get(24..length)
            .ok_or_else(|| invalid("KX2 triangle records are truncated"))?;
        let mut triangles = Vec::new();
        triangles
            .try_reserve_exact(count)
            .map_err(|_| invalid("KX2 triangle allocation failed"))?;
        for record in data.chunks_exact(48) {
            triangles.push(std::array::from_fn(|vertex| {
                let at = vertex * 16;
                let word = |n| u32::from_le_bytes(record[at + n..at + n + 4].try_into().unwrap());
                KxVertex {
                    position: [word(0), word(4), word(8)],
                    argb: word(12),
                }
            }));
        }
        Ok(Self {
            bounds: [word(4), word(8), word(12), word(16)],
            triangles,
        })
    }

    pub fn rectangle(&self) -> [f32; 4] {
        self.bounds.map(f32::from_bits)
    }

    pub fn extent(&self) -> [f32; 2] {
        let [left, top, right, bottom] = self.rectangle();
        [(right - left).abs(), (bottom - top).abs()]
    }
}
