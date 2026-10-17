// One layer drawn fitted into the project frame, blended with its opacity.

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

struct Params {
    // Where the layer goes, in the frame's clip space: left, top, right, bottom.
    dst: vec4<f32>,
    // x: opacity 0..1.
    opacity: vec4<f32>,
}

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_smp: sampler;
@group(0) @binding(2) var<uniform> params: Params;

@vertex
fn vs_layer(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32(i & 1u), f32(i >> 1u));
    return Out(vec4<f32>(mix(params.dst.xy, params.dst.zw, uv), 0.0, 1.0), uv);
}

// Straight-alpha source in, premultiplied out for premultiplied "over" blending.
@fragment
fn fs_layer(in: Out) -> @location(0) vec4<f32> {
    let c = textureSample(image, image_smp, in.uv);
    let a = c.a * params.opacity.x;
    return vec4<f32>(c.rgb * a, a);
}
