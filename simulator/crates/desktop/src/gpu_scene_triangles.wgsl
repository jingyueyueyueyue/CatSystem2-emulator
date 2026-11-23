// Keep integer straight-RGBA blend equations identical to gpu_raster.wgsl.
struct Vertex {
    point_rhw: vec4f,
    uv_over_w: vec4f,
    color: vec4f,
}
struct Triangle {
    vertices: array<Vertex, 3>,
    bounds: vec4u,
}
struct Parameters {
    region: vec4u,
    width: u32,
    blend: u32,
    half_centers: u32,
    flip_v: u32,
    textured: u32,
    tile_columns: u32,
    padding: vec2u,
}
@group(0) @binding(0) var<storage, read_write> pixels: array<u32>;
@group(0) @binding(1) var source: texture_2d<u32>;
@group(0) @binding(2) var<storage, read> triangles: array<Triangle>;
@group(0) @binding(3) var<uniform> parameters: Parameters;
@group(0) @binding(4) var<storage, read> tiles: array<vec2u>;
@group(0) @binding(5) var<storage, read> indices: array<u32>;

fn unpack(value: u32) -> vec4u {
    return (vec4u(value) >> vec4u(0u, 8u, 16u, 24u)) & vec4u(255u);
}
fn pack(value: vec4u) -> u32 {
    return value.r | (value.g << 8u) | (value.b << 16u) | (value.a << 24u);
}
fn blend_list(dst: vec4u, src: vec4u) -> vec4u {
    let a = ((255u * (src.a + 1u)) >> 8u) & 255u;
    if a == 255u || dst.a == 0u { return vec4u(src.rgb, a); }
    if a == 0u { return dst; }
    if dst.a == 255u {
        let rgb = vec3i(dst.rgb) + (((vec3i(src.rgb) - vec3i(dst.rgb)) * i32(a)) >> vec3u(8u));
        return vec4u(vec3u(rgb), 255u);
    }
    let overlap = (a * dst.a) >> 8u;
    let dw = ((256u - a) * dst.a) >> 8u;
    let sw = (((256u - dst.a) * a) >> 8u) + overlap;
    let alpha = (sw + dw) | 1u;
    return vec4u(((src.rgb * sw + dst.rgb * dw) * (65536u / alpha)) >> vec3u(16u), alpha);
}
fn blend(dst: vec4u, src: vec4u) -> vec4u {
    let mode = parameters.blend;
    if mode >= 6u {
        return vec4u(dst.rgb, ((dst.a + 1u) * (src[mode - 6u] + 1u) - 1u) >> 8u);
    }
    if src.a == 0u { return dst; }
    if mode == 0u { return blend_list(dst, src); }
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
fn edge(a: vec2f, b: vec2f, point: vec2f) -> f32 {
    let delta = b - a;
    let offset = point - a;
    return delta.x * offset.y - delta.y * offset.x;
}
fn covered(a: vec2f, b: vec2f, value: f32) -> bool {
    return value > 0.0 || (value == 0.0 && (b.y < a.y || (b.y == a.y && b.x > a.x)));
}
fn bilinear(input_uv: vec2f) -> vec4f {
    let size = textureDimensions(source);
    var uv = input_uv;
    if parameters.flip_v != 0u { uv.y = 1.0 - uv.y; }
    let point = clamp(uv * vec2f(size) - vec2f(0.5), vec2f(0.0), vec2f(size - vec2u(1u)));
    let lo = vec2i(floor(point));
    let hi = min(lo + vec2i(1), vec2i(size) - vec2i(1));
    let fraction = point - vec2f(lo);
    let a = vec4f(textureLoad(source, lo, 0));
    let b = vec4f(textureLoad(source, vec2i(hi.x, lo.y), 0));
    let c = vec4f(textureLoad(source, vec2i(lo.x, hi.y), 0));
    let d = vec4f(textureLoad(source, hi, 0));
    return (a * (1.0 - fraction.x) + b * fraction.x) * (1.0 - fraction.y)
        + (c * (1.0 - fraction.x) + d * fraction.x) * fraction.y;
}

@compute @workgroup_size(8, 8)
fn scene_triangles(@builtin(global_invocation_id) id: vec3u) {
    if any(id.xy >= parameters.region.zw) { return; }
    let point = id.xy + parameters.region.xy;
    let center = vec2f(point) + vec2f(f32(parameters.half_centers) * 0.5);
    let pixel = point.y * parameters.width + point.x;
    var result = unpack(pixels[pixel]);
    let bin = tiles[(id.y / 8u) * parameters.tile_columns + id.x / 8u];
    for (var at = 0u; at < bin.y; at += 1u) {
        let triangle = triangles[indices[bin.x + at]];
        if any(point < triangle.bounds.xy) || any(point >= triangle.bounds.xy + triangle.bounds.zw) { continue; }
        let a = triangle.vertices[0];
        let b = triangle.vertices[1];
        let c = triangle.vertices[2];
        let weights = vec3f(edge(b.point_rhw.xy, c.point_rhw.xy, center),
            edge(c.point_rhw.xy, a.point_rhw.xy, center), edge(a.point_rhw.xy, b.point_rhw.xy, center));
        if !covered(b.point_rhw.xy, c.point_rhw.xy, weights.x)
            || !covered(c.point_rhw.xy, a.point_rhw.xy, weights.y)
            || !covered(a.point_rhw.xy, b.point_rhw.xy, weights.z) { continue; }
        let area = edge(a.point_rhw.xy, b.point_rhw.xy, c.point_rhw.xy);
        let barycentric = weights / area;
        var color = a.color * barycentric.x + b.color * barycentric.y + c.color * barycentric.z;
        if parameters.textured != 0u {
            let rhw = dot(barycentric, vec3f(a.point_rhw.z, b.point_rhw.z, c.point_rhw.z));
            if rhw == 0.0 { continue; }
            let uv = (a.uv_over_w.xy * barycentric.x + b.uv_over_w.xy * barycentric.y
                + c.uv_over_w.xy * barycentric.z) / rhw;
            color *= bilinear(uv) / 255.0;
        }
        // Rust f64::round rounds positive half values away from zero.
        let sample = vec4u(clamp(floor(color + vec4f(0.5)), vec4f(0.0), vec4f(255.0)));
        result = blend(result, sample);
    }
    pixels[pixel] = pack(result);
}
