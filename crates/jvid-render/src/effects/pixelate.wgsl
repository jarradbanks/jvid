// @jvid effect "Pixelate"
// @param block float 1.0..200.0 = 24.0

// Shows the image in `block` logical Full-input pixel squares, keeping alpha so a masked clip pixelates only inside its mask.
fn effect(uv: vec2<f32>) -> vec4<f32> {
    // Stable logical cells; proxy texels and preview scale do not change their count.
    let cell = (floor(uv * fx.size / fx.block) + 0.5) * fx.block / fx.size;
    return vec4<f32>(jvid_sample(cell).rgb, textureSample(src, smp, uv).a);
}
