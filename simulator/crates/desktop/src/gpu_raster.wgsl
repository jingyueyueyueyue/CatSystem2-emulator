// RGBA is uploaded in the engine's packed little-endian byte order.
@group(0) @binding(0) var<storage, read_write> pixels: array<u32>;
@group(0) @binding(1) var source: texture_2d<u32>;
// A job occupies 4096 bytes, matching ARGS_BYTES in the Rust dispatcher.
struct RasterJob {
    output_rect: vec4u,
    args: array<u32, 1020>,
}
@group(0) @binding(2) var<storage, read> jobs: array<RasterJob>;
@group(0) @binding(3) var canvas_pages: texture_2d_array<u32>;
@group(0) @binding(4) var<storage, read> page_slots: array<u32>;
@group(0) @binding(5) var wipe_mask: texture_2d<u32>;
var<private> job: u32;

fn unpack(value: u32) -> vec4u {
    return (vec4u(value) >> vec4u(0u, 8u, 16u, 24u)) & vec4u(255u);
}

fn pack(value: vec4u) -> u32 {
    return value.r | (value.g << 8u) | (value.b << 16u) | (value.a << 24u);
}

fn sample_source(point: vec2u) -> vec4u {
    if jobs[job].args[1019] != 0u {
        let page = point / 256u;
        let columns = (jobs[job].args[5] - 1u) / 256u + 1u;
        let slot = page_slots[page.y * columns + page.x];
        if slot == 0u { return vec4u(0u); }
        return textureLoad(canvas_pages, vec2i(point % 256u), i32(slot - 1u), 0);
    }
    return textureLoad(source, vec2i(point), 0);
}

fn blend_list(dst: vec4u, src: vec4u, opacity: u32) -> vec4u {
    let a = (((opacity & 65535u) * (src.a + 1u)) >> 8u) & 255u;
    if a == 255u || dst.a == 0u {
        return vec4u(src.rgb, a);
    }
    if a == 0u {
        return dst;
    }
    if dst.a == 255u {
        let rgb = vec3i(dst.rgb) + (((vec3i(src.rgb) - vec3i(dst.rgb)) * i32(a)) >> vec3u(8u));
        return vec4u(vec3u(rgb), 255u);
    }
    let overlap = (a * dst.a) >> 8u;
    let dw = ((256u - a) * dst.a) >> 8u;
    let sw = (((256u - dst.a) * a) >> 8u) + overlap;
    let out_alpha = (sw + dw) | 1u;
    let rgb = ((src.rgb * sw + dst.rgb * dw) * (65536u / out_alpha)) >> vec3u(16u);
    return vec4u(rgb, out_alpha);
}

fn blend_canvas(dst: vec4u, src: vec4u, mode: u32) -> vec4u {
    if mode >= 6u {
        return vec4u(dst.rgb, ((dst.a + 1u) * (src[mode - 6u] + 1u) - 1u) >> 8u);
    }
    if src.a == 0u {
        return dst;
    }
    if mode == 0u {
        return blend_list(dst, src, 255u);
    }
    var product: vec4u;
    switch mode {
        case 1u: { product = src * src.a + dst * 255u; }
        case 2u: { product = src * 255u + dst * (255u - src.a); }
        case 3u: { product = src * dst; }
        case 4u: { product = src * (vec4u(255u) - dst); }
        default: { product = dst * (src + vec4u(255u)); }
    }
    return min((product + vec4u(127u)) / vec4u(255u), vec4u(255u));
}

fn plane_span(dst: vec4u, point: vec2u) -> vec4u {
    let origin = vec2u(jobs[job].args[7], jobs[job].args[8]);
    let dimensions = vec2u(jobs[job].args[5], jobs[job].args[6]);
    if any(origin >= dimensions) || any(point >= dimensions - origin) {
        return dst;
    }
    var src = sample_source(point + origin);
    src.a = (src.a * jobs[job].args[9]) / 255u;
    return blend_canvas(dst, src, select(0u, 3u, jobs[job].args[10] != 0u));
}

struct Vertex {
    point: vec2f,
    reciprocal_w: f32,
    uv_over_w: vec2f,
}

fn vertex(index: u32) -> Vertex {
    return Vertex(
        vec2f(bitcast<f32>(jobs[job].args[index]), bitcast<f32>(jobs[job].args[index + 1u])),
        bitcast<f32>(jobs[job].args[index + 3u]),
        vec2f(bitcast<f32>(jobs[job].args[index + 4u]), bitcast<f32>(jobs[job].args[index + 5u])),
    );
}

fn edge(a: vec2f, b: vec2f, point: vec2f) -> f32 {
    let d = b - a;
    let p = point - a;
    return d.x * p.y - d.y * p.x;
}

fn covered(a: vec2f, b: vec2f, weight: f32) -> bool {
    return weight > 0.0 || (weight == 0.0 && (b.y < a.y || (b.y == a.y && b.x > a.x)));
}

fn plane_triangles(dst: vec4u, point: vec2f) -> vec4u {
    let argb = unpack(jobs[job].args[7]);
    let tint = argb.bgra;
    var result = dst;
    for (var triangle = 0u; triangle < jobs[job].args[9]; triangle += 1u) {
        let offset = 12u + triangle * 18u;
        let a = vertex(offset);
        let b = vertex(offset + 6u);
        let c = vertex(offset + 12u);
        let w = vec3f(edge(b.point, c.point, point), edge(c.point, a.point, point),
                      edge(a.point, b.point, point));
        if !covered(b.point, c.point, w.x) || !covered(c.point, a.point, w.y)
            || !covered(a.point, b.point, w.z) {
            continue;
        }
        var src = tint;
        if jobs[job].args[5] != 0u {
            let reciprocal_w = dot(w, vec3f(a.reciprocal_w, b.reciprocal_w, c.reciprocal_w));
            let uv = floor((a.uv_over_w * w.x + b.uv_over_w * w.y + c.uv_over_w * w.z) / reciprocal_w);
            let origin = vec2u(jobs[job].args[10], jobs[job].args[11]);
            let dimensions = vec2u(jobs[job].args[5], jobs[job].args[6]);
            if any(uv < vec2f(0.0)) || any(uv >= vec2f(dimensions - origin)) {
                continue;
            }
            src = (sample_source(vec2u(uv) + origin) * tint) / vec4u(255u);
        }
        result = blend_canvas(result, src, jobs[job].args[8]);
    }
    return result;
}

fn list_span(dst: vec4u, point: vec2u) -> vec4u {
    // Neighbour RGB at zero alpha is observable during later filtering.
    // A previous pixel's write cannot affect blending into a zero-alpha
    // destination, so each invocation can reproduce both edge writes locally.
    let column = i32(point.x) - i32(jobs[job].args[10]);
    let width = i32(jobs[job].args[9]);
    let start = vec2u(jobs[job].args[7], jobs[job].args[8] + point.y);
    if column < 0 || column >= width {
        if pack(dst) == 0u {
            let neighbor = start + vec2u(u32(clamp(column, 0, width - 1)), 0u);
            return vec4u(sample_source(neighbor).rgb, 0u);
        }
        return dst;
    }
    let sample_at = start + vec2u(u32(column), 0u);
    let result = blend_list(dst, sample_source(sample_at), jobs[job].args[11]);
    if pack(result) == 0u && column + 1 < width {
        return vec4u(sample_source(sample_at + vec2u(1u, 0u)).rgb, 0u);
    }
    return result;
}

fn multiply15(left: i32, right: i32) -> i32 {
    // Low 32 bits of a signed 64-bit product shifted right by 15.
    let low_l = left & 32767;
    let low_r = right & 32767;
    let high_l = left >> 15;
    let high_r = right >> 15;
    return ((high_l * high_r) << 15u) + high_l * low_r + low_l * high_r + ((low_l * low_r) >> 15);
}

fn arg_i(index: u32) -> i32 {
    return bitcast<i32>(jobs[job].args[index]);
}

fn list_affine(dst: vec4u, point: vec2u) -> vec4u {
    let relative = (vec2i(point) << vec2u(15u)) + vec2i(arg_i(11u), arg_i(12u));
    let transformed = vec2i(
        multiply15(arg_i(7u), relative.x) + multiply15(arg_i(8u), relative.y),
        multiply15(arg_i(9u), relative.x) + multiply15(arg_i(10u), relative.y),
    );
    let p = (transformed >> vec2u(15u)) + vec2i(arg_i(13u), arg_i(14u));
    let limit = vec2i(i32(jobs[job].args[5]), i32(jobs[job].args[6]));
    if any(p < vec2i(0)) || any(p >= limit) {
        return dst;
    }
    let first = sample_source(vec2u(p));
    var right = first;
    var bottom = first;
    var diagonal = first;
    if p.x + 1 < limit.x { right = sample_source(vec2u(p + vec2i(1, 0))); }
    if p.y + 1 < limit.y { bottom = sample_source(vec2u(p + vec2i(0, 1))); }
    if all(p + vec2i(1) < limit) { diagonal = sample_source(vec2u(p + vec2i(1))); }
    if (first.a | right.a | bottom.a | diagonal.a) == 0u {
        return dst;
    }
    let frac = transformed & vec2i(32767);
    let inv = vec2i(32768) - frac;
    let blended = (first * u32(multiply15(inv.x, inv.y))
                  + right * u32(multiply15(frac.x, inv.y))
                  + bottom * u32(multiply15(inv.x, frac.y))
                  + diagonal * u32(multiply15(frac.x, frac.y))) >> vec4u(15u);
    if jobs[job].args[16] >= 6u {
        return blend_canvas(dst, blended, jobs[job].args[16]);
    }
    return blend_list(dst, blended, jobs[job].args[15]);
}

@compute @workgroup_size(8, 8)
fn raster(@builtin(global_invocation_id) id: vec3u) {
    job = id.z;
    if id.x >= jobs[job].args[3] || id.y >= jobs[job].args[4] {
        return;
    }
    let canvas_point = id.xy + vec2u(jobs[job].args[1], jobs[job].args[2]);
    let local = canvas_point - jobs[job].output_rect.xy;
    let index = jobs[job].output_rect.w + local.y * jobs[job].output_rect.z + local.x;
    let dst = unpack(pixels[index]);
    var result: vec4u;
    switch jobs[job].args[0] {
        case 0u: { result = plane_span(dst, id.xy); }
        case 1u: { result = plane_triangles(dst, vec2f(canvas_point) + vec2f(0.5)); }
        case 2u: { result = list_span(dst, id.xy); }
        case 3u: { result = list_affine(dst, canvas_point); }
        case 4u: { result = sample_source(id.xy + vec2u(jobs[job].args[7], jobs[job].args[8])); }
        case 5u: {
            var src = sample_source(id.xy + vec2u(jobs[job].args[7], jobs[job].args[8]));
            src.a = (src.a * jobs[job].args[9] + 127u) / 255u;
            result = blend_canvas(dst, src, 0u);
        }
        case 6u: {
            let alpha = jobs[job].args[7];
            let sample = vec4u(sample_source(id.xy).rgb, alpha);
            result = (sample * alpha + dst * (255u - alpha) + 127u) / 255u;
        }
        default: {
            let step = jobs[job].args[8];
            let blue = textureLoad(wipe_mask, vec2i(id.xy), 0).b;
            var sample = sample_source(id.xy);
            if jobs[job].args[7] == 0u {
                if blue >= step { sample.a = 0u; }
            } else {
                sample.a = 255u - u32(clamp(i32(step) + i32(blue) - 256, 0, 255));
            }
            result = (sample * sample.a + dst * (255u - sample.a) + 127u) / 255u;
        }
    }
    pixels[index] = pack(result);
}
