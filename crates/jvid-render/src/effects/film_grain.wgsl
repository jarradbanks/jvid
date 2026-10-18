// @jvid effect "Film grain"
// @param amount float 0.0..1.0 = 0.2
// @param grain float 1.0..8.0 = 1.5

fn hash(p: vec3<f32>) -> f32 {
    var q = fract(p * 0.1031);
    q += dot(q, q.zyx + 31.32);
    return fract((q.x + q.y) * q.z);
}

// Monochrome noise in `grain`-pixel cells, new each frame, strongest in the midtones.
fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = textureSample(src, smp, uv);
    let cell = floor(uv * fx.size / fx.grain);
    let n = hash(vec3<f32>(cell, fx.frame)) - 0.5;
    let luma = dot(c.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let w = 4.0 * luma * (1.0 - luma);
    return vec4<f32>(clamp(c.rgb + n * fx.amount * w, vec3<f32>(0.0), vec3<f32>(1.0)), c.a);
}
