struct Dimensions {
    game: vec2u,
    surface: vec2u,
    srgb: u32,
}

@group(0) @binding(0) var<storage, read> pixels: array<u32>;
@group(0) @binding(1) var<uniform> sizes: Dimensions;

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let point = array(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
    return vec4f(point[index], 0.0, 1.0);
}

@fragment
fn fragment(@builtin(position) point: vec4f) -> @location(0) vec4f {
    let pixel = min(vec2u(point.xy * vec2f(sizes.game) / vec2f(sizes.surface)), sizes.game - vec2u(1u));
    let word = pixels[pixel.y * sizes.game.x + pixel.x];
    let rgba = vec4f((vec4u(word) >> vec4u(0u, 8u, 16u, 24u)) & vec4u(255u)) / 255.0;
    if sizes.srgb == 0u { return rgba; }
    let linear = select(pow((rgba.rgb + vec3f(0.055)) / 1.055, vec3f(2.4)), rgba.rgb / 12.92, rgba.rgb <= vec3f(0.04045));
    return vec4f(linear, rgba.a);
}
