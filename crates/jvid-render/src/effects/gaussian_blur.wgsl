// @jvid effect "Gaussian Blur"
// @param radius float 0.0..100.0 = 20.0

// Blurs by up to `radius` pixels, keeping alpha so a masked clip blurs only inside its mask.
// shortcut: one 7x7 pass, so big radii show faint steps; go separable two-pass if that shows.
fn tap(uv: vec2<f32>, x: f32, y: f32) -> vec3<f32> {
    return textureSample(src, smp, uv + vec2<f32>(x, y) * fx.radius / 3.0 / fx.size).rgb;
}

// Seven taps along x at row `y`, Gaussian weighted (sigma 1.5 taps).
fn row(uv: vec2<f32>, y: f32) -> vec3<f32> {
    return tap(uv, 0.0, y) + (tap(uv, -1.0, y) + tap(uv, 1.0, y)) * 0.8007
        + (tap(uv, -2.0, y) + tap(uv, 2.0, y)) * 0.4111
        + (tap(uv, -3.0, y) + tap(uv, 3.0, y)) * 0.1353;
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let rgb = row(uv, 0.0) + (row(uv, -1.0) + row(uv, 1.0)) * 0.8007
        + (row(uv, -2.0) + row(uv, 2.0)) * 0.4111
        + (row(uv, -3.0) + row(uv, 3.0)) * 0.1353;
    // 3.6942 is one row's weight sum.
    return vec4<f32>(rgb / (3.6942 * 3.6942), textureSample(src, smp, uv).a);
}
