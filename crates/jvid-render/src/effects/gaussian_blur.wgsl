// @jvid effect "Gaussian Blur"
// @param radius float 0.0..100.0 = 20.0
// @passes 2

// Blurs by up to `radius` pixels, keeping alpha so a masked clip blurs only inside its mask.
// Pass 0 blurs along x, pass 1 along y. Taps per pass scale with the radius (4 to 128), at most
// 1.6 px apart, so no pixel falls between taps and small radii stay cheap. Effects can't loop,
// so the taps are unrolled and groups past the count are skipped on a uniform branch.
fn tap(uv: vec2<f32>, i: f32, n: f32) -> vec4<f32> {
    let dir = select(vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), fx.pass_index < 0.5);
    let k = i - (n - 1.0) * 0.5;
    // Taps span +-radius at sigma radius / 2, so the weight depends on k / n alone.
    let w = exp(-0.5 * (4.0 * k / n) * (4.0 * k / n));
    let c = textureSampleLevel(src, smp, uv + dir * k * 2.0 * fx.radius / n / fx.size, 0.0).rgb;
    return vec4<f32>(c * w, w);
}

fn taps4(uv: vec2<f32>, i: f32, n: f32) -> vec4<f32> {
    if i >= n {
        return vec4<f32>(0.0);
    }
    return tap(uv, i, n) + tap(uv, i + 1.0, n) + tap(uv, i + 2.0, n) + tap(uv, i + 3.0, n);
}

fn taps16(uv: vec2<f32>, i: f32, n: f32) -> vec4<f32> {
    return taps4(uv, i, n) + taps4(uv, i + 4.0, n) + taps4(uv, i + 8.0, n) + taps4(uv, i + 12.0, n);
}

fn taps64(uv: vec2<f32>, i: f32, n: f32) -> vec4<f32> {
    return taps16(uv, i, n) + taps16(uv, i + 16.0, n) + taps16(uv, i + 32.0, n) + taps16(uv, i + 48.0, n);
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    // ceil(radius * 1.3) rounded up to whole groups of 4: spacing 2 * radius / n stays under 1.6 px.
    let n = clamp(ceil(fx.radius * 1.3 / 4.0) * 4.0, 4.0, 128.0);
    let sum = taps64(uv, 0.0, n) + taps64(uv, 64.0, n);
    return vec4<f32>(sum.rgb / sum.a, textureSample(src, smp, uv).a);
}
