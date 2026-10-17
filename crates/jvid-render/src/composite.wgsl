// One transformed layer drawn into the project frame, blended with its opacity.

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

struct Params {
    // Corners in triangle-strip order: clip-space xy, then uv.
    corners: array<vec4<f32>, 4>,
    // x: opacity 0..1, y: blend mode (jvid_core::model::Blend's index).
    opacity: vec4<f32>,
}

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_smp: sampler;
@group(0) @binding(2) var<uniform> params: Params;

@vertex
fn vs_layer(@builtin(vertex_index) i: u32) -> Out {
    let c = params.corners[i];
    return Out(vec4<f32>(c.xy, 0.0, 1.0), c.zw);
}

// Straight-alpha source in, premultiplied out for premultiplied "over" blending.
@fragment
fn fs_layer(in: Out) -> @location(0) vec4<f32> {
    let c = textureSample(image, image_smp, in.uv);
    let a = c.a * params.opacity.x;
    return vec4<f32>(c.rgb * a, a);
}

// The frame so far, copied out before a non-normal layer; only the blend pipeline binds it.
@group(0) @binding(3) var backdrop: texture_2d<f32>;

fn mix_mode(mode: u32, b: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 1u: { return b * s; }
        case 2u: { return b + s - b * s; }
        case 3u: { return select(2.0 * b * s, 1.0 - 2.0 * (1.0 - b) * (1.0 - s), b > vec3<f32>(0.5)); }
        case 4u: { return min(b + s, vec3<f32>(1.0)); }
        case 5u: { return min(b, s); }
        case 6u: { return max(b, s); }
        case 7u: { return abs(b - s); }
        default: { return s; }
    }
}

// W3C separable blending then source-over, written over the copied backdrop (no fixed blend).
@fragment
fn fs_blend(in: Out) -> @location(0) vec4<f32> {
    let c = textureSample(image, image_smp, in.uv);
    let a = c.a * params.opacity.x;
    let d = textureLoad(backdrop, vec2<i32>(in.pos.xy), 0);
    let b = select(vec3<f32>(0.0), d.rgb / d.a, d.a > 0.0);
    let s = (1.0 - d.a) * c.rgb + d.a * mix_mode(u32(params.opacity.y), b, c.rgb);
    return vec4<f32>(a * s + (1.0 - a) * d.rgb, a + d.a * (1.0 - a));
}
