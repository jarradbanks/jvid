// @jvid effect "Chroma Key"
// @param key_hue float 0.0..360.0 = 120.0
// @param tolerance float 0.0..1.0 = 0.6
// @param edge_softness float 0.0..1.0 = 0.1
// @param spill_suppression float 0.0..1.0 = 0.5

// Makes colours near `key_hue` (120 green, 240 blue) transparent and suppresses their spill.
fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = textureSample(src, smp, uv);
    let h = radians(fx.key_hue);
    let d = vec2<f32>(cos(h), sin(h));
    // The pixel in an opponent-colour plane where red is 0°, green 120° and blue 240°.
    let ch = vec2<f32>(c.r - (c.g + c.b) * 0.5, (c.g - c.b) * 0.8660254);
    // How far the key colour leads the other two: g - max(r, b) when the key is green.
    let k = dot(ch, d) - abs(d.x * ch.y - d.y * ch.x) * 0.57735027;
    let cut = 1.0 - fx.tolerance;
    let keep = clamp((cut - k) / max(fx.edge_softness, 0.0001), 0.0, 1.0);
    // Removing `k` along the key direction leaves the key colour level with the others.
    let e = vec3<f32>(cos(h), cos(h - 2.0943951), cos(h + 2.0943951)) * (2.0 / 3.0);
    let rgb = c.rgb - e * max(k, 0.0) * fx.spill_suppression;
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), c.a * keep);
}
