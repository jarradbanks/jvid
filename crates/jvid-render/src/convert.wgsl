// YUV 4:2:0 to RGB, drawn over the whole target.

struct Convert {
    // rgb[i] = dot(rows[i].xyz, yuv) + rows[i].w, with yuv sampled as 0..1.
    rows: array<vec4<f32>, 3>,
}

@group(0) @binding(0) var y_plane: texture_2d<f32>;
@group(0) @binding(1) var u_plane: texture_2d<f32>;
@group(0) @binding(2) var v_plane: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;
@group(0) @binding(4) var<uniform> convert: Convert;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// One triangle covering the target.
@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return Out(vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0), uv);
}

@fragment
fn fs_convert(in: Out) -> @location(0) vec4<f32> {
    let yuv = vec3<f32>(
        textureSample(y_plane, smp, in.uv).r,
        textureSample(u_plane, smp, in.uv).r,
        textureSample(v_plane, smp, in.uv).r,
    );
    let rgb = vec3<f32>(
        dot(convert.rows[0].xyz, yuv) + convert.rows[0].w,
        dot(convert.rows[1].xyz, yuv) + convert.rows[1].w,
        dot(convert.rows[2].xyz, yuv) + convert.rows[2].w,
    );
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
