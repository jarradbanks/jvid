// Draws the converted image into part of the preview viewport.

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_smp: sampler;
// Where the image goes, in the viewport's clip space: left, top, right, bottom.
@group(0) @binding(2) var<uniform> dst: vec4<f32>;

// A quad over `dst`, as a 4-vertex triangle strip.
@vertex
fn vs_blit(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32(i & 1u), f32(i >> 1u));
    return Out(vec4<f32>(mix(dst.xy, dst.zw, uv), 0.0, 1.0), uv);
}

@fragment
fn fs_blit(in: Out) -> @location(0) vec4<f32> {
    return textureSample(image, image_smp, in.uv);
}
