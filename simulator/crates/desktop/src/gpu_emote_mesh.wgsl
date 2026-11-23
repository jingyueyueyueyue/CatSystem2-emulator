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

// Keep the existing integer straight-RGBA and serial stencil representation.
// Each ordered mesh is rasterized then composited before the next mesh starts.
@compute @workgroup_size(8, 8)
fn mesh_sprite(@builtin(global_invocation_id) id: vec3u) {
    command_offset = 0u;
    if any(id.xy >= vec2u(command(0u), command(1u))) { return; }
    let pos = id.xy + vec2u(command(2u), command(3u));
    let local = pos - vec2u(command(5u), command(6u));
    let coverage = textureLoad(mesh_coverage, vec2i(local), 0);
    let covered = coverage.z == f32(command(14u));
    let index = local.y * command(4u) + local.x;
    let serial = command(7u);
    let reference = command(9u);
    let operation = command(10u) & 255u;
    var pixel = pixels[index];
    let same_plane = ((pixel.stencil & ~FLAGS) >> 8u) == serial;
    var stencil = select(command(8u), pixel.stencil & 255u, same_plane);
    var uncertain_mask = track_repairs && serial != 0u && same_plane && (pixel.stencil & MASK_REPAIR) != 0u;
    var uncertain_color = select(0u, pixel.stencil & COLOR_REPAIR, track_repairs);
    // Hardware edge setup snaps to subpixels. Mark both sides of coverage
    // boundaries in strict mode so a missing fragment is also repaired.
    if track_repairs {
        let last = vec2i(textureDimensions(mesh_coverage)) - 1;
        var edge = false;
        for (var dy = -1; dy <= 1; dy++) {
            for (var dx = -1; dx <= 1; dx++) {
                let neighbor = textureLoad(mesh_coverage, clamp(vec2i(local) + vec2i(dx, dy), vec2i(0), last), 0);
                edge = edge || (neighbor.z == f32(command(14u))) != covered;
            }
        }
        if edge {
            if operation == 0u { uncertain_color |= COLOR_REPAIR; }
            else { uncertain_mask = true; }
        }
    }
    if !covered {
        if uncertain_mask { pixel.stencil = (serial << 8u) | stencil | MASK_REPAIR | uncertain_color; }
        else { pixel.stencil = (pixel.stencil & ~COLOR_REPAIR) | uncertain_color; }
        pixels[index] = pixel;
        return;
    }
    if serial != 0u && stencil != reference && !uncertain_mask { return; }
    let uncertainty = (abs(coverage.xy) + vec2f(1.0)) * 0.000002;
    let base = vec2i(bitcast<i32>(command(12u)), bitcast<i32>(command(13u)));
    var sample = sample_atlas(base, coverage.xy, uncertainty);
    if (command(10u) & 256u) != 0u { sample = Sample(sample.color.bgra, sample.error.bgra); }
    let alpha = clamp(sample.color.a / 255.0 * bitcast<f32>(command(11u)), 0.0, 1.0);
    let alpha_error = select(sample.error.a / 255.0 * abs(bitcast<f32>(command(11u))) + 0.0000006, 0.0,
        bitcast<f32>(command(11u)) <= 0.0 || (sample.color.a == 0.0 && sample.error.a <= 0.00012));
    if operation == 0u {
        if uncertain_mask { uncertain_color |= COLOR_REPAIR; }
        if serial == 0u || stencil == reference {
            let result = source_over(pixel.rgba, sample, alpha, alpha_error);
            pixel.rgba = result.x;
            uncertain_color |= result.y;
        }
    } else {
        if track_repairs && abs(alpha * 255.0 - 64.5) <= alpha_error * 255.0 { uncertain_mask = true; }
        if stencil == reference && alpha * 255.0 >= 64.5 { stencil = (stencil + operation) & 255u; }
        pixel.stencil = (serial << 8u) | stencil | select(0u, MASK_REPAIR, uncertain_mask);
    }
    pixel.stencil = (pixel.stencil & ~COLOR_REPAIR) | uncertain_color;
    pixels[index] = pixel;
}
