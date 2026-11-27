// Hardware coverage/linear UV interpolation. Positions have a half pixel shift
// so the runtime's integer sample locations coincide with hardware centers.
struct MeshVertex {
    @builtin(position) position: vec4f,
    @location(0) @interpolate(linear) uv: vec2f,
    @location(1) @interpolate(flat) generation: u32,
}
@vertex
fn mesh_vertex(@location(0) position: vec2f, @location(1) uv: vec2f,
    @builtin(instance_index) generation: u32) -> MeshVertex {
    return MeshVertex(vec4f(position, 0.0, 1.0), uv, generation);
}
@fragment
fn mesh_fragment(vertex: MeshVertex) -> @location(0) vec4f {
    return vec4f(vertex.uv, f32(vertex.generation), 0.0);
}

@group(0) @binding(4) var mesh_coverage: texture_2d<f32>;
@group(0) @binding(5) var<storage, read_write> mesh_rgba: array<u32>;
@group(0) @binding(6) var<storage, read_write> mesh_stencil: array<u32>;
@group(1) @binding(0) var mesh_atlas: texture_2d<u32>;

// Keep the existing integer straight-RGBA and serial stencil representation.
// Each ordered mesh is rasterized then composited before the next mesh starts.
@compute @workgroup_size(8, 8)
fn mesh_sprite(@builtin(global_invocation_id) id: vec3u) {
    command_offset = 0u;
    if any(id.xy >= vec2u(command(0u), command(1u))) { return; }
    let pos = id.xy + vec2u(command(2u), command(3u));
    let local = pos - vec2u(command(5u), command(6u));
    let coverage = textureLoad(mesh_coverage, vec2i(local), 0);
    if coverage.z != f32(command(14u)) { return; }
    let index = local.y * command(4u) + local.x;
    let serial = command(7u);
    let reference = command(9u);
    let operation = command(10u) & 255u;
    if serial != 0u {
        let previous = mesh_stencil[index];
        let stencil = select(command(8u), previous & 255u, (previous >> 8u) == serial);
        if stencil != reference { return; }
    }
    let base = vec2i(bitcast<i32>(command(12u)), bitcast<i32>(command(13u)));
    var sample = sample_atlas(mesh_atlas, base, coverage.xy, vec2f(0.0));
    if (command(10u) & 256u) != 0u { sample = Sample(sample.color.bgra, sample.error.bgra); }
    let alpha = clamp(sample.color.a / 255.0 * bitcast<f32>(command(11u)), 0.0, 1.0);
    if operation == 0u {
        if alpha != 0.0 {
            mesh_rgba[index] = source_over(mesh_rgba[index], sample, alpha, 0.0).x;
        }
    } else {
        let stencil = select(reference, (reference + operation) & 255u, alpha * 255.0 >= 64.5);
        mesh_stencil[index] = (serial << 8u) | stencil;
    }
}
