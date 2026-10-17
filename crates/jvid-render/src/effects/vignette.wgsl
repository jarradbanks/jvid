// @jvid effect "Vignette"
// @param strength float 0.0..1.0 = 0.5
// @param softness float 0.0..1.0 = 0.3

// Darkens towards the corners: full strength at the corners, none inside the radius.
fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = textureSample(src, smp, uv);
    // Distance from the centre, 0 there and 1 at a corner, round whatever the aspect.
    let d = length((uv - 0.5) * 2.0) / sqrt(2.0);
    let edge = 1.0 - fx.softness;
    let v = smoothstep(edge * 0.6, 1.0 + fx.softness * 0.2, d);
    return vec4<f32>(c.rgb * (1.0 - fx.strength * v), c.a);
}
