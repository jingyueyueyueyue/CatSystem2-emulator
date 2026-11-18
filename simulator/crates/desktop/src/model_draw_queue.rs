//! The kcView queue contains both MDL meshes and submitted particle lists.
//! SHA659d: 48d2e0 builds one list, 48d100 orders it, 48dc64 draws type5.
//! No owner, transform, texture identity or particle VM state is duplicated.
use std::collections::BTreeMap;
use std::borrow::Cow;
use cs2_renderer::{Canvas, RgbaFrame};
use super::{Matrix, ParticleCanvas, local_matrix, multiply, property_number, raster, shader, transform};
use super::super::{ParticleObjectState, particle_external::ExternalParticle};

struct Submission {
    node: usize,
    particle: Option<u32>,
    priority: i32,
    distance: f32,
    world: Matrix,
    features: u32,
}

fn features(canvas: &ParticleCanvas, index: usize) -> anyhow::Result<u32> {
    // 48d0c0 propagates the parent's selector bits by OR. They belong to
    // the model graph, not to the particle snapshot's render_mode/opacity.
    let mut at = Some(index);
    let mut value = 0;
    for _ in 0..=256 {
        let Some(index) = at else { return Ok(value); };
        let node = canvas.nodes.get(index).filter(|node| node.alive)
            .ok_or_else(|| anyhow::anyhow!("kcView shader selector references a released node"))?;
        let enabled = |key: &[u8]| node.properties.get(key).is_some_and(|s| s.first() == Some(&b'1'));
        value |= u32::from(enabled(b"billboard_y"))
            | (u32::from(enabled(b"billboard_xyz")) << 1)
            | (u32::from(enabled(b"comop_add")) << 4);
        at = node.parent;
    }
    anyhow::bail!("kcView shader selector parent chain exceeds256")
}

fn matrix(world: Matrix, view: Matrix, projection: Matrix, camera: Matrix, billboard: u32) -> anyhow::Result<Matrix> {
    let world = match billboard {
        0 => world,
        1 => {
            // a0e1f: the real camera forward, then RY * W * V * P.
            let direction = transform([0., 0., -1., 0.], camera);
            let angle = if direction[0] > 0. {
                -(direction[2] / direction[0]).atan() - std::f32::consts::FRAC_PI_2
            } else if direction[0] < 0. {
                std::f32::consts::FRAC_PI_2 - (direction[2] / direction[0]).atan()
            } else { 0. };
            multiply(local_matrix([0.; 3], [1.; 3], [0., angle, 0.]), world)
        }
        _ => anyhow::bail!("MDL XYZ billboard constant binding is not recovered"),
    };
    Ok(multiply(multiply(world, view), projection))
}

fn particle_texture(frame: &RgbaFrame) -> anyhow::Result<Cow<'_, RgbaFrame>> {
    let [w, h] = frame.metadata().canvas_size;
    let dimensions = (if w == 0 { frame.width() } else { w }, if h == 0 { frame.height() } else { h });
    if dimensions == frame.dimensions() { Ok(Cow::Borrowed(frame)) }
    else { Ok(Cow::Owned(shader::texture_canvas(frame.clone())?)) }
}

pub(super) fn draw(
    canvas: &ParticleCanvas, output: &mut Canvas,
    objects: &BTreeMap<u32, ExternalParticle>, particles: &BTreeMap<u32, ParticleObjectState>,
    owner: u32, only_particle: Option<u32>, view: Matrix, projection: Matrix, camera: usize,
) -> anyhow::Result<()> {
    let camera_world = canvas.world(camera)?;
    let mut submissions = Vec::new();
    // Original graph traversal is shared by all kinds; handle order must not
    // put every external particle after the last mesh.
    for (index, node) in canvas.nodes.iter().enumerate() {
        if !node.alive || !canvas.visible(index)? { continue; }
        let particle = if let Some(target) = node.particle {
            if only_particle.is_some_and(|only| only != target) { continue; }
            let object = objects.get(&target).filter(|o| o.canvas == owner && o.node == index)
                .ok_or_else(|| anyhow::anyhow!("kcView particle graph/submission identity disagrees"))?;
            if object.triangles.is_empty() || !particles.get(&target).is_some_and(|p| p.enabled) { continue; }
            Some(target)
        } else {
            if only_particle.is_some() { continue; }
            if node.light.is_some() { anyhow::bail!("MDL light descriptor needs its directional/point/spot live binding"); }
            if node.meshes.is_empty() { continue; }
            None
        };
        let world = canvas.world(index)?;
        let distance = (0..3).map(|i| (world[3][i] - camera_world[3][i]).powi(2)).sum::<f32>();
        anyhow::ensure!(distance.is_finite(), "kcView submission has a non-finite sort distance");
        submissions.push(Submission { node: index, particle, world, distance,
            priority: property_number(&node.properties, b"priority")? as i32,
            features: features(canvas, index)? });
    }
    if submissions.is_empty() { return Ok(()); }
    // 48d100: explicit priorities first/ascending, otherwise far to near.
    submissions.sort_by(|a, b| match (a.priority != 0, b.priority != 0) {
        (true, true) => a.priority.cmp(&b.priority),
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => b.distance.total_cmp(&a.distance),
    });
    let count = (output.width() as usize).checked_mul(output.height() as usize)
        .filter(|&n| n <= 16_777_216).ok_or_else(|| anyhow::anyhow!("MDL shared depth surface exceeds16M pixels"))?;
    let mut depth = vec![1.; count];
    for submission in submissions {
        let node = &canvas.nodes[submission.node];
        if let Some(target) = submission.particle {
            // 48dcf4 explicitly requests FVF142, independently of snapshot
            // projection/opacity. PL SHD owns cull, alpha, blend and ZWrite.
            let program = canvas.assets.particle_program(submission.features)?;
            let matrix = matrix(submission.world, view, projection, camera_world, program.billboard)?;
            let frame = particles[&target].texture.as_ref()
                .ok_or_else(|| anyhow::anyhow!("kcView FVF142 particle has no bound texture pixels"))?;
            let frame = particle_texture(frame)?;
            for triangle in &objects[&target].triangles {
                let vertices = triangle.map(|v| raster::Vertex {
                    clip: transform([v.position[0], v.position[1], v.position[2], 1.], matrix),
                    // 948e6 constructs material at object+ac. 9a55e..9a5bb
                    // writes its +20 RGBA from fld1, not the +10 RGB=0.25.
                    // a128b reads +20 for V_DIFFUSE; task opacity is absent.
                    color: [16, 8, 0, 24].map(|s| ((v.color >> s) & 255) as f32 / 255.),
                    uv: v.uv,
                    fog_w: 0., // PL SHD explicitly has FogEnable=false.
                });
                raster::triangle(output, &mut depth, vertices, Some(frame.as_ref()), program, canvas.assets.linear_sampling, None)?;
            }
        } else {
            let periods = [property_number(&node.properties, b"scroll_u")?, property_number(&node.properties, b"scroll_v")?];
            let uv_add = periods.map(|p| if p == 0 { 0. } else { (canvas.elapsed % p) as f32 / p as f32 });
            // 48a6ed defaults the object's fog DWORD to1; 4943e0 reads its
            // own property. This state does not inherit the parent's flags.
            let object_fog = !node.properties.contains_key(b"fog".as_slice())
                || property_number(&node.properties,b"fog")? != 0;
            // M_WORLDVIEW (c10..13) remains the original W*V binding even
            // when the position shader selects a billboard WVP constant.
            let world_view = multiply(submission.world,view);
            let object_color = node.material_color.rgba();
            for mesh in &node.meshes {
                let material = canvas.assets.materials.get(&mesh.material)
                    .ok_or_else(|| anyhow::anyhow!("MDL mesh references an absent material: {}", String::from_utf8_lossy(&mesh.material)))?;
                let program = canvas.assets.program(mesh.fvf, submission.features, material)?;
                // a12df..a13c1: V_DIFCOL is the material's +20 RGBA times
                // this type1 source's +ac RGBA, including alpha. A shader
                // that emits v5 directly does not consume this constant.
                let diffuse: [f32;4] = std::array::from_fn(|i|material.diffuse[i]*object_color[i]);
                let matrix = matrix(submission.world, view, projection, camera_world, program.billboard)?;
                // The compiler has validated this exact key's full equation
                // sequence. Only its missing oFog uses the observed host
                // compatibility policy; no generic unwritten-output default.
                let campanella_billboard_y_add = mesh.fvf == 0x112 && submission.features == 0x11
                    && program.source.eq_ignore_ascii_case("BillboardYAdd.shd");
                let fog = canvas.fog.vertex(program.fog,object_fog,program.writes_fog,
                    campanella_billboard_y_add)
                    .map_err(|error|anyhow::anyhow!("MDL shader={} FVF={:#x} TYPE={:#x} material={:?}: {error}",
                        program.source,mesh.fvf,submission.features,mesh.material))?;
                let texture = material.textures.first().map(|name| canvas.assets.textures.get(name)
                    .map(|frame| frame.as_ref()).ok_or_else(|| anyhow::anyhow!(
                        "MDL active material texture has no loaded pixels: material={:?}, texture={name:?}; live missing-texture shader binding is not recovered",
                        mesh.material))).transpose()?;
                if (mesh.fvf & 0x100 != 0) != texture.is_some() {
                    anyhow::bail!("MDL FVF/texture-stage combination lacks its original SHD variant");
                }
                for indices in mesh.indices.chunks_exact(3) {
                    let mut vertices = [raster::Vertex { clip: [0.; 4], color: [0.; 4], uv: [0.; 2], fog_w:0. }; 3];
                    for (out, &index) in vertices.iter_mut().zip(indices) {
                        let vertex = &mesh.vertices[index as usize];
                        let color = match program.diffuse {
                            shader::Diffuse::Material => diffuse,
                            shader::Diffuse::Vertex => vertex.color,
                            shader::Diffuse::Product => std::array::from_fn(|i| diffuse[i] * vertex.color[i]),
                            shader::Diffuse::Directional { .. } => anyhow::bail!("MDL SHD directional equation lacks its real light owner"),
                        };
                        let position = [vertex.position[0], vertex.position[1], vertex.position[2], 1.];
                        let factor = fog.map(|fog|fog.factor(transform(position,world_view)[2])).transpose()?.unwrap_or(1.);
                        let clip = transform(position,matrix);
                        *out = raster::Vertex { clip,
                            color, uv: [vertex.uv[0] + uv_add[0], vertex.uv[1] + uv_add[1]], fog_w:factor*clip[3] };
                    }
                    raster::triangle(output, &mut depth, vertices, texture, program, canvas.assets.linear_sampling, fog.map(|fog|fog.rgb))?;
                }
            }
        }
    }
    Ok(())
}
