# writing effects

an effect is one `.wgsl` file in the effects folder (`effects/` in jvid's config directory - the bin's effects tab has a button that opens it). jvid ships a few to crib from: vignette, chromatic aberration, film grain, chroma key, gaussian blur and pixelate.

edits are picked up within a second, no restart. delete the whole folder and the examples come back.

## the file

```wgsl
// @jvid effect "Vignette"
// @param strength float 0.0..1.0 = 0.5
// @param softness float 0.0..1.0 = 0.3

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = textureSample(src, smp, uv);
    let d = length((uv - 0.5) * 2.0) / sqrt(2.0);
    let v = smoothstep(0.6 * (1.0 - fx.softness), 1.0, d);
    return vec4<f32>(c.rgb * (1.0 - fx.strength * v), c.a);
}
```

- `// @jvid effect "Name"` - required, names it in the inspector.
- `// @param name float min..max = default` - one per slider, all keyframeable. only `float`. `size`, `texel_size`, `jvid_padding`, `frame` and `pass_index` are taken.
- `// @passes N` - run it N times (1-8), each over the last. gaussian blur does x then y.
- `fn effect(uv)` - return the colour at `uv` (0,0 top left) as straight, not premultiplied, RGBA.

helper functions and constants are fine; your own bindings aren't. what you get:

| name | what |
| --- | --- |
| `src` | the clip's image after earlier effects, straight RGBA |
| `smp` | linear, clamping sampler |
| `jvid_sample(uv)` | alpha-aware bilinear sample of `src` - use this when blurring |
| `fx.size` | full-quality input size in pixels. stays the same at half/quarter preview and with proxies |
| `fx.texel_size` | the actual texture size, for per-texel work |
| `fx.frame` | timeline frame number, for animated noise |
| `fx.pass_index` | which pass this is, from 0 |
| `fx.<param>` | each slider's current value |

effects run on the clip's own image, before its transform and grade, in inspector order.

when averaging samples, weight RGB by alpha and divide by total alpha, or invisible pixels tint your edges.

## no loops

`loop`, `for` and `while` are rejected. a runaway loop on the GPU stalls everything until the driver resets, sometimes taking the desktop with it. unroll a fixed number of samples by hand instead, like chromatic aberration does.

## errors

a file that won't compile shows in red in the effects tab with naga's message and your line numbers. clips using it just skip it until it's fixed.

## hiding something

to blur an email or a face: copy the clip to the track above, mask the copy to the region, add gaussian blur or pixelate to it.
