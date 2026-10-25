// @jvid effect "Gaussian Blur"
// @param radius float 0.0..100.0 = 20.0
// @passes 2

// Blurs by up to `radius` pixels, keeping alpha so a masked clip blurs only inside its mask.
// Pass 0 blurs along x, pass 1 along y: 128 taps each, at most 1.6 px apart even at radius 100,
// so no pixel falls between taps.
fn tap(uv: vec2<f32>, k: f32) -> vec4<f32> {
    let dir = select(vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), fx.pass_index < 0.5);
    // Taps span +-radius at sigma radius / 2, so the weight depends on k alone.
    let w = exp(-0.5 * (k / 32.0) * (k / 32.0));
    let c = textureSample(src, smp, uv + dir * k * fx.radius / 64.0 / fx.size).rgb;
    return vec4<f32>(c * w, w);
}

fn taps4(uv: vec2<f32>, k: f32) -> vec4<f32> {
    return tap(uv, k) + tap(uv, k + 1.0) + tap(uv, k + 2.0) + tap(uv, k + 3.0);
}

fn taps16(uv: vec2<f32>, k: f32) -> vec4<f32> {
    return taps4(uv, k) + taps4(uv, k + 4.0) + taps4(uv, k + 8.0) + taps4(uv, k + 12.0);
}

fn taps64(uv: vec2<f32>, k: f32) -> vec4<f32> {
    return taps16(uv, k) + taps16(uv, k + 16.0) + taps16(uv, k + 32.0) + taps16(uv, k + 48.0);
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let sum = taps64(uv, -63.5) + taps64(uv, 0.5);
    return vec4<f32>(sum.rgb / sum.a, textureSample(src, smp, uv).a);
}
