
struct Pixel {
    rgba: u32,
    stencil: u32,
}

@group(0) @binding(0) var<storage, read> draw: array<u32>;
@group(0) @binding(1) var<storage, read_write> pixels: array<Pixel>;
@group(0) @binding(2) var atlas: texture_2d<u32>;
@group(0) @binding(3) var<storage, read> batch_offsets: array<u32>;
var<private> command_offset: u32;
override track_repairs: bool = true;

fn command(index: u32) -> u32 {
    return draw[command_offset + index];
}

struct Sample {
    color: vec4<f32>,
    error: vec4<f32>,
}

fn sample_atlas(atlas: texture_2d<u32>, base: vec2<i32>, offset: vec2<f32>, uncertainty: vec2<f32>) -> Sample {
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
    let color = mix(mix(a, b, fraction.x), mix(c, d, fraction.x), fraction.y);
    if !track_repairs { return Sample(color, vec4<f32>(0.0)); }
    let gradient = max(abs(b - a), abs(d - c));
    let gradient_y = max(abs(c - a), abs(d - b));
    let dx = select(gradient, vec4<f32>(255.0),
        min(fraction.x, 1.0 - fraction.x) <= uncertainty.x && low.x != high.x);
    let dy = select(gradient_y, vec4<f32>(255.0),
        min(fraction.y, 1.0 - fraction.y) <= uncertainty.y && low.y != high.y);
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
        let repair = track_repairs && bytes.a == 0u && alpha_error > 0.0 && any(sample.color.rgb >= vec3<f32>(0.5));
        return vec2<u32>(destination, select(0u, COLOR_REPAIR, repair));
    }
    let destination_alpha = f32(bytes.a) / 255.0;
    let below = destination_alpha * (1.0 - alpha);
    let combined = alpha + below;
    let rgb = select(sample.color.rgb,
        (sample.color.rgb * alpha + vec3<f32>(bytes.rgb) * below) / combined, bytes.a != 0u);
    let values = vec4<f32>(rgb, combined * 255.0);
    let result = vec4<u32>(clamp(floor(values + 0.5), vec4<f32>(0.0), vec4<f32>(255.0)));
    let packed = result.r | (result.g << 8u) | (result.b << 16u) | (result.a << 24u);
    if !track_repairs { return vec2<u32>(packed, 0u); }
    let alpha_mix_error = select(0.0,
        alpha_error * destination_alpha * 255.0 / (combined * combined), bytes.a != 0u);
    let error = vec4<f32>(sample.error.rgb + alpha_mix_error + 0.00012, alpha_error * (255.0 - f32(bytes.a)) + 0.00012);
    return vec2<u32>(packed, select(0u, COLOR_REPAIR, rounding_uncertain(values, error)));
}

@compute @workgroup_size(8, 8)
fn sprite(@builtin(global_invocation_id) id: vec3<u32>) {
    command_offset = batch_offsets[id.z];
    let size = vec2<u32>(command(0u), command(1u));
    if any(id.xy >= size) { return; }
    let pos = id.xy + vec2<u32>(command(2u), command(3u));
    let x = pos.x;
    let index = (pos.y - command(6u)) * command(4u) + x - command(5u);
    let serial = command(7u);
    let reference = command(9u);
    let operation = command(10u) & 255u;
    let bgra = (command(10u) & 256u) != 0u;
    let opacity = bitcast<f32>(command(11u));
    var pixel = pixels[index];
    let same_plane = ((pixel.stencil & ~FLAGS) >> 8u) == serial;
    var stencil = select(command(8u), pixel.stencil & 255u, same_plane);
    var uncertain_mask = track_repairs && serial != 0u && same_plane && (pixel.stencil & MASK_REPAIR) != 0u;
    var uncertain_color = select(0u, pixel.stencil & COLOR_REPAIR, track_repairs);
    var wrote_mask = false;
    let row = 12u + id.y;
    for (var i = command(row); i < command(row + 1u); i += 8u) {
        if x < command(i) || x >= command(i + 1u) { continue; }
        if serial != 0u && stencil != reference && !uncertain_mask { continue; }
        let start = vec2<f32>(bitcast<f32>(command(i + 2u)), bitcast<f32>(command(i + 3u)));
        let advance = f32(x - command(i)) * vec2<f32>(bitcast<f32>(command(i + 4u)), bitcast<f32>(command(i + 5u)));
        let offset = start + advance;
        let base = vec2<i32>(bitcast<i32>(command(i + 6u)), bitcast<i32>(command(i + 7u)));
        let uncertainty = (abs(start) + abs(advance) + abs(offset)) * 0.00000013 + 0.0000002;
        var sample = sample_atlas(atlas, base, offset, uncertainty);
        if bgra {
            sample = Sample(sample.color.bgra, sample.error.bgra);
        }
        let alpha = clamp(sample.color.a / 255.0 * opacity, 0.0, 1.0);
        // All four zero texels give exact zero inside the interpolation cell.
        let alpha_error = select(sample.error.a / 255.0 * abs(opacity) + 0.0000006, 0.0,
            opacity <= 0.0 || (sample.color.a == 0.0 && sample.error.a <= 0.00012));
        if operation == 0u {
            if track_repairs && uncertain_mask {
                uncertain_color |= select(0u, COLOR_REPAIR, alpha > 0.0 || alpha_error > 0.0);
            }
            if serial == 0u || stencil == reference {
                if track_repairs && !uncertain_mask && alpha == 1.0 && sample.error.a <= 0.00012 {
                    // Uniform opaque coverage replaces the earlier color.
                    uncertain_color = 0u;
                }
                let result = source_over(pixel.rgba, sample, alpha, alpha_error);
                pixel.rgba = result.x;
                uncertain_color |= result.y;
            }
        } else {
            if track_repairs && abs(alpha * 255.0 - 64.5) <= alpha_error * 255.0 {
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
    if track_repairs && (id.x & 31u) == 0u {
        var mask = 0u;
        for (var i = 0u; i < min(32u, size - id.x); i++) {
            mask |= ((accumulated[id.x + i].stencil & COLOR_REPAIR) >> 31u) << i;
        }
        rgba[size + id.x / 32u] = mask;
    }
}



@group(0) @binding(0) var<uniform> repair_count: OutputSize;
@group(0) @binding(1) var<storage, read> corrections: array<vec2u>;
@group(0) @binding(2) var<storage, read_write> corrected_rgba: array<u32>;

@compute @workgroup_size(64)
fn repair(@builtin(global_invocation_id) id: vec3u) {
    if id.x >= repair_count.size { return; }
    let correction = corrections[id.x];
    corrected_rgba[correction.x] = correction.y;
}

struct FadeParams {
    size: vec2u,
    weight: u32,
    transparent: u32,
    old_origin: vec2i,
    old_size: vec2u,
}
@group(0) @binding(0) var<uniform> fade: FadeParams;
@group(0) @binding(1) var<storage, read> fade_old: array<u32>;
@group(0) @binding(2) var<storage, read> fade_new: array<u32>;
@group(0) @binding(3) var<storage, read_write> fade_output: array<u32>;
@group(0) @binding(4) var<storage, read_write> fade_prepared_new: array<u32>;
@group(0) @binding(5) var<storage, read_write> fade_new_alpha: atomic<u32>;
@group(0) @binding(6) var<storage, read> fade_alpha_class: u32;
var<workgroup> fade_group_alpha: atomic<u32>;

@compute @workgroup_size(8, 8)
fn surface_expand(@builtin(global_invocation_id) id: vec3u) {
    if any(id.xy >= fade.size) { return; }
    let source = vec2i(id.xy) - fade.old_origin;
    var pixel = 0u;
    if all(source >= vec2i(0)) && all(vec2u(source) < fade.old_size) {
        pixel = fade_old[u32(source.y) * fade.old_size.x + u32(source.x)];
    }
    fade_output[id.y * fade.size.x + id.x] = pixel;
}

@compute @workgroup_size(8, 8)
fn surface_tint(@builtin(global_invocation_id) id: vec3u) {
    if any(id.xy >= fade.size) { return; }
    let index = id.y * fade.size.x + id.x;
    let shifts = vec4u(0u, 8u, 16u, 24u);
    let channels = (vec4u(fade_old[index]) >> shifts) & vec4u(255u);
    let tint = (vec4u(fade.weight) >> vec4u(16u, 8u, 0u, 24u)) & vec4u(255u);
    let result = (channels * tint + vec4u(127u)) / vec4u(255u);
    fade_output[index] = result.r | (result.g << 8u) | (result.b << 16u) | (result.a << 24u);
}

@compute @workgroup_size(8, 8)
fn surface_prepare(@builtin(global_invocation_id) id: vec3u) {
    if any(id.xy >= fade.size) { return; }
    let index = id.y * fade.size.x + id.x;
    let prior = fade_old[index];
    let next = fade_new[index];
    let mask = 0x00ffffffu;
    fade_output[index] = select(prior, next & mask, prior >> 24u == 0u && next >> 24u != 0u);
    fade_prepared_new[index] = select(next, prior & mask, next >> 24u == 0u && prior >> 24u != 0u);
}

@compute @workgroup_size(8, 8)
fn surface_alpha(@builtin(global_invocation_id) id: vec3u, @builtin(local_invocation_index) lane: u32) {
    if all(id.xy < fade.size) {
        let index = id.y * fade.size.x + id.x;
        if fade_new[index] >> 24u != 0u {
            atomicOr(&fade_group_alpha, 1u);
        }
    }
    workgroupBarrier();
    if lane == 0u && atomicLoad(&fade_group_alpha) != 0u {
        atomicOr(&fade_new_alpha, 1u);
    }
}

@compute @workgroup_size(8, 8)
fn surface_fade(@builtin(global_invocation_id) id: vec3u) {
    if any(id.xy >= fade.size) { return; }
    let index = id.y * fade.size.x + id.x;
    let old_weight = 256u - fade.weight;
    let prior = fade_old[index];
    if fade.transparent == 1u || (fade.transparent == 2u && fade_alpha_class == 0u) {
        let alpha = ((prior >> 24u) * old_weight) >> 8u;
        fade_output[index] = (prior & 0xffffffu) | (alpha << 24u);
        return;
    }
    let shifts = vec4u(0u, 8u, 16u, 24u);
    let old_rgba = (vec4u(prior) >> shifts) & vec4u(255u);
    let new_rgba = (vec4u(fade_new[index]) >> shifts) & vec4u(255u);
    let pixel = ((old_rgba * old_weight + new_rgba * fade.weight) >> vec4u(8u)) << shifts;
    fade_output[index] = pixel.x | pixel.y | pixel.z | pixel.w;
}
