//! Independently recovered kcView particle producer, Campanella SHA below.
//! Creation text is a graph path, not a surface-size/configuration language.
use std::collections::BTreeMap;
use cs2_renderer::RgbaFrame;
use super::particle_vm::{ParticleRenderSnapshot, ParticleVertex};

pub(super) const CAMPANELLA_SHA: &str =
    "659d5f069f3c97f69a61e12449cf1510b45e48a10649b9ac6977c2b93d6d28ad";

#[derive(Default)]
pub(super) struct ParticleExternalStore {
    legacy_kc_view: bool,
    objects: BTreeMap<u32, ExternalParticle>,
    pub canvases: BTreeMap<u32, super::particle_canvas::ParticleCanvas>,
    binding: Option<(u32, [f32; 2])>,
}

#[derive(Debug)]
pub(super) struct ExternalParticle {
    pub name: Vec<u8>,
    pub requested_parent: Vec<u8>,
    pub parent: Option<u32>,
    pub extent: [f32; 2],
    pub canvas: u32,
    pub node: usize,
    scale: [f32; 2],
    // A real submission owns all six vertices, independent of the VM's grid.
    pub triangles: Vec<[ParticleVertex; 3]>,
}

impl ParticleExternalStore {
    pub(super) fn identify(&mut self, sha256: &str) {
        self.legacy_kc_view = sha256 == CAMPANELLA_SHA;
    }

    pub(super) fn legacy_kc_view(&self) -> bool { self.legacy_kc_view }

    /// 848a0 creates type5/pl%d in @kcView; 84810 resolves a path from the
    /// graph root. An absent path leaves the original root parent untouched.
    /// Only actual shared graph objects participate, never task-tree names.
    pub(super) fn bound_canvas(&self) -> cs2_script::Result<u32> {
        self.binding.filter(|(id, _)| self.canvases.get(id).is_some_and(|c| c.loaded && c.configured))
            .map(|(id, _)| id).ok_or_else(|| super::native_state_error("external669 requires543 loaded kcTask3DView and652 shared graph binding"))
    }

    pub(super) fn bind(&mut self, target: u32, scale: [f32; 2]) -> cs2_script::Result<()> {
        if scale.iter().any(|v| !v.is_finite()) { return Err(super::native_state_error("kcView binding contains non-finite scale")); }
        if !self.canvases.get(&target).is_some_and(|c| c.loaded && c.configured) {
            return Err(super::native_state_error("kcView cannot bind an unloaded graph/texture manager"));
        }
        self.binding = Some((target, scale)); Ok(())
    }

    pub(super) fn release_canvas(&mut self, target: u32) {
        self.canvases.remove(&target);
        self.objects.retain(|_, object| object.canvas != target);
        if self.binding.is_some_and(|(id, _)| id == target) { self.binding = None; }
    }

    pub(super) fn clear_canvas_graph(&mut self, target: u32) {
        if let Some(canvas) = self.canvases.get_mut(&target) { canvas.clear_graph(); }
        self.objects.retain(|_, object| object.canvas != target);
        if self.binding.is_some_and(|(id, _)| id == target) { self.binding = None; }
    }

    pub(super) fn draw(&self, target: u32, destination: &mut cs2_renderer::Canvas,
        particles: &BTreeMap<u32, super::ParticleObjectState>) -> anyhow::Result<bool> {
        let Some(canvas) = self.canvases.get(&target) else { return Ok(false); };
        canvas.draw(destination, &self.objects, particles, target, None)?; Ok(true)
    }

    pub(super) fn draw_particle(&self, canvas: u32, particle: u32, destination: &mut cs2_renderer::Canvas,
        particles: &BTreeMap<u32, super::ParticleObjectState>) -> anyhow::Result<()> {
        let view = self.canvases.get(&canvas).ok_or_else(|| anyhow::anyhow!("external particle canvas was released"))?;
        view.draw(destination, &self.objects, particles, canvas, Some(particle))
    }

    pub(super) fn create(&mut self, target: u32, path: &[u8], texture: Option<&RgbaFrame>) -> cs2_script::Result<()> {
        let canvas_id = self.bound_canvas()?;
        let scale = self.binding.unwrap().1;
        self.release(target);
        let canvas = self.canvases.get_mut(&canvas_id).unwrap();
        let node = canvas.add_particle(target, path);
        let parent = canvas.nodes[node].parent.and_then(|p| canvas.nodes[p].particle);
        self.objects.insert(target, ExternalParticle {
            name: format!("pl{}", target as i32).into_bytes(),
            requested_parent: path.to_vec(), parent,
            extent: texture.map_or([0.; 2], |image| [image.width() as f32, image.height() as f32]),
            triangles: Vec::new(), canvas: canvas_id, node, scale,
        });
        Ok(())
    }

    pub(super) fn get(&self, target: u32) -> Option<&ExternalParticle> { self.objects.get(&target) }

    pub(super) fn release(&mut self, target: u32) {
        if let Some(object) = self.objects.remove(&target) {
            if let Some(canvas) = self.canvases.get_mut(&object.canvas) {
                for id in canvas.remove_particle(target) { self.objects.remove(&id); }
            }
        }
    }

    /// Body 358e4..3596c -> 366f0 -> 94c60 -> 94ad0. This is NOT the
    /// ordinary particle projector: RHW/particle opacity/sort are not consumed.
    pub(super) fn submit(&mut self, target: u32, snapshot: &ParticleRenderSnapshot) -> cs2_script::Result<()> {
        let object = self.objects.get_mut(&target).ok_or_else(||
            super::native_state_error("external particle has no identified kcView receiver"))?;
        if !snapshot.frame_complete { return Err(super::native_state_error("external particle frame is still yielding")); }
        let count = snapshot.quads.iter().filter(|quad| quad.flag != 0).count();
        if count.checked_mul(6 * 28).is_none_or(|bytes| bytes > 64 * 1024 * 1024) {
            return Err(super::native_state_error("external particle submitted mesh exceeds64 MiB"));
        }
        let mut triangles = Vec::with_capacity(count * 2);
        for quad in snapshot.quads.iter().filter(|quad| quad.flag != 0) {
            // Exact PE double constant at VA5dc7b0 (promoted 0.01f).
            let offset = quad.offset.map(|value| (f64::from(value) * f64::from(0.01f32)) as f32);
            let vertices = quad.vertices.map(|mut vertex| {
                vertex.position = std::array::from_fn(|axis|
                    vertex.position[axis] * if axis < 2 { object.scale[axis] } else { 1. } + offset[axis]);
                vertex.rhw = 1.;
                vertex
            });
            // 94c60 expands [0,1,2,1,2,3]; 94ad0 emits that list backwards.
            triangles.push([vertices[3], vertices[2], vertices[1]]);
            triangles.push([vertices[2], vertices[1], vertices[0]]);
        }
        object.triangles = triangles;
        Ok(())
    }
}
