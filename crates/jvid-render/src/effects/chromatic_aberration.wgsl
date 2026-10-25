// @jvid effect "Chromatic aberration"
// @param amount float 0.0..20.0 = 4.0

// Splits red and blue outwards from the centre by up to `amount` pixels at the edges.
fn effect(uv: vec2<f32>) -> vec4<f32> {
    let off = (uv - 0.5) * 2.0 * fx.amount / fx.size;
    let g = textureSample(src, smp, uv);
    let r = jvid_sample(uv + off).r;
    let b = jvid_sample(uv - off).b;
    return vec4<f32>(r, g.g, b, g.a);
}
