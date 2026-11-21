
struct Pixel {
    rgba: u32,
    stencil: u32,
}

@group(0) @binding(0) var<storage, read> draw: array<u32>;
@group(0) @binding(1) var<storage, read_write> pixels: array<Pixel>;
@group(0) @binding(2) var atlas: texture_2d<u32>;

struct Sample {
    color: vec4<f32>,
    error: vec4<f32>,
}

fn sample_atlas(base: vec2<i32>, offset: vec2<f32>, uncertainty: vec2<f32>) -> Sample {
    let last = vec2<i32>(textureDimensions(atlas)) - 1;
    let position = offset - 0.5;
    let integral = floor(position);
    let full = base + vec2<i32>(integral);
    let low = clamp(full, vec2<i32>(0), last);
    let high = clamp(full + 1, vec2<i32>(0), last);
    let fraction = position - integral;
    let a = vec4<f32>(textureLoad(atlas, low, 0));
    let b = vec4<f32>(textureLoad(atlas, vec2<i32>(high.x, low.y), 0));
    let c = vec4<f32>(textureLoad(atlas, vec2<i32>(low.x, high.y), 0));
    let d = vec4<f32>(textureLoad(atlas, high, 0));
    let gradient = max(abs(b - a), abs(d - c));
    let gradient_y = max(abs(c - a), abs(d - b));
    let dx = select(gradient, vec4<f32>(255.0),
        min(fraction.x, 1.0 - fraction.x) <= uncertainty.x && low.x != high.x);
    let dy = select(gradient_y, vec4<f32>(255.0),
        min(fraction.y, 1.0 - fraction.y) <= uncertainty.y && low.y != high.y);
    let color = mix(mix(a, b, fraction.x), mix(c, d, fraction.x), fraction.y);
    return Sample(color, dx * uncertainty.x + dy * uncertainty.y + 0.00012);
}

const COLOR_REPAIR: u32 = 0x80000000u;
const MASK_REPAIR: u32 = 0x40000000u;
const FLAGS: u32 = COLOR_REPAIR | MASK_REPAIR;

fn rounding_uncertain(values: vec4<f32>, error: vec4<f32>) -> bool {
    return any(abs(fract(values) - 0.5) <= error);
}

fn source_over(destination: u32, sample: Sample, alpha: f32, alpha_error: f32) -> vec2<u32> {
    let bytes = (vec4<u32>(destination) >> vec4<u32>(0, 8, 16, 24)) & vec4<u32>(255u);
    if alpha == 0.0 {
        // With no destination alpha, even a positive sub-byte alpha can
        // leave RGB in a pixel whose final quantized alpha is still zero.
        let repair = bytes.a == 0u && alpha_error > 0.0 && any(sample.color.rgb >= vec3<f32>(0.5));
        return vec2<u32>(destination, select(0u, COLOR_REPAIR, repair));
    }
    let destination_alpha = f32(bytes.a) / 255.0;
    let below = destination_alpha * (1.0 - alpha);
    let combined = alpha + below;
    let rgb = select(sample.color.rgb,
        (sample.color.rgb * alpha + vec3<f32>(bytes.rgb) * below) / combined, bytes.a != 0u);
    let values = vec4<f32>(rgb, combined * 255.0);
    let alpha_mix_error = select(0.0,
        alpha_error * destination_alpha * 255.0 / (combined * combined), bytes.a != 0u);
    let error = vec4<f32>(sample.error.rgb + alpha_mix_error + 0.00012, alpha_error * (255.0 - f32(bytes.a)) + 0.00012);
    let result = vec4<u32>(clamp(floor(values + 0.5), vec4<f32>(0.0), vec4<f32>(255.0)));
    let packed = result.r | (result.g << 8u) | (result.b << 16u) | (result.a << 24u);
    return vec2<u32>(packed, select(0u, COLOR_REPAIR, rounding_uncertain(values, error)));
}

@compute @workgroup_size(8, 8)
fn sprite(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(draw[0], draw[1]);
    if any(id.xy >= size) { return; }
    let pos = id.xy + vec2<u32>(draw[2], draw[3]);
    let x = pos.x;
    let index = (pos.y - draw[6]) * draw[4] + x - draw[5];
    let serial = draw[7];
    let reference = draw[9];
    let operation = draw[10];
    let opacity = bitcast<f32>(draw[11]);
    var pixel = pixels[index];
    let same_plane = ((pixel.stencil & ~FLAGS) >> 8u) == serial;
    var stencil = select(draw[8], pixel.stencil & 255u, same_plane);
    var uncertain_mask = serial != 0u && same_plane && (pixel.stencil & MASK_REPAIR) != 0u;
    var uncertain_color = pixel.stencil & COLOR_REPAIR;
    var wrote_mask = false;
    let row = 12u + id.y;
    for (var i = draw[row]; i < draw[row + 1u]; i += 8u) {
        if x < draw[i] || x >= draw[i + 1u] { continue; }
        if serial != 0u && stencil != reference && !uncertain_mask { continue; }
        let start = vec2<f32>(bitcast<f32>(draw[i + 2u]), bitcast<f32>(draw[i + 3u]));
        let advance = f32(x - draw[i]) * vec2<f32>(bitcast<f32>(draw[i + 4u]), bitcast<f32>(draw[i + 5u]));
        let offset = start + advance;
        let base = vec2<i32>(bitcast<i32>(draw[i + 6u]), bitcast<i32>(draw[i + 7u]));
        let uncertainty = (abs(start) + abs(advance) + abs(offset)) * 0.00000013 + 0.0000002;
        let sample = sample_atlas(base, offset, uncertainty);
        let alpha = clamp(sample.color.a / 255.0 * opacity, 0.0, 1.0);
        // All four zero texels give exact zero inside the interpolation cell.
        let alpha_error = select(sample.error.a / 255.0 * abs(opacity) + 0.0000006, 0.0,
            opacity <= 0.0 || (sample.color.a == 0.0 && sample.error.a <= 0.00012));
        if operation == 0u {
            if uncertain_mask {
                uncertain_color |= select(0u, COLOR_REPAIR, alpha > 0.0 || alpha_error > 0.0);
            }
            if serial == 0u || stencil == reference {
                if !uncertain_mask && alpha == 1.0 && sample.error.a <= 0.00012 {
                    // Uniform opaque coverage replaces the earlier color.
                    uncertain_color = 0u;
                }
                let result = source_over(pixel.rgba, sample, alpha, alpha_error);
                pixel.rgba = result.x;
                uncertain_color |= result.y;
            }
        } else {
            if abs(alpha * 255.0 - 64.5) <= alpha_error * 255.0 {
                uncertain_mask = true;
                wrote_mask = true;
            }
            if stencil == reference && alpha * 255.0 >= 64.5 {
                stencil = (stencil + operation) & 255u;
                wrote_mask = true;
            }
        }
    }
    if wrote_mask {
        pixel.stencil = (serial << 8u) | stencil | select(0u, MASK_REPAIR, uncertain_mask);
    }
    pixel.stencil = (pixel.stencil & ~COLOR_REPAIR) | uncertain_color;
    pixels[index] = pixel;
}

struct OutputSize { size: u32 }
@group(0) @binding(0) var<uniform> output_size: OutputSize;
@group(0) @binding(1) var<storage, read> accumulated: array<Pixel>;
@group(0) @binding(2) var<storage, read_write> rgba: array<u32>;

@compute @workgroup_size(64)
fn pack(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = output_size.size;
    if id.x >= size { return; }
    rgba[id.x] = accumulated[id.x].rgba;
    if (id.x & 31u) == 0u {
        var mask = 0u;
        for (var i = 0u; i < min(32u, size - id.x); i++) {
            mask |= ((accumulated[id.x + i].stencil & COLOR_REPAIR) >> 31u) << i;
        }
        rgba[size + id.x / 32u] = mask;
    }
}

