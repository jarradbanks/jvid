# Writing effects

An effect is one `.wgsl` file in the effects folder (`effects/` in jvid's config directory;
the bin's Effects tab has a button that opens it). jvid creates the folder with three examples
the first time it runs: vignette, chromatic aberration and film grain. Read those next to this.

New and edited files are picked up within a second while jvid runs; no restart. Deleting the
whole folder brings the three examples back on the next check. An export uses the effects as they
were when it started; edits made during it show in the next one.

## The file

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

- `// @jvid effect "Name"` names the effect in the Inspector. Required.
- `// @param name float min..max = default`, one per parameter. Each becomes a keyframeable
  slider in the Inspector. Only `float` is supported; `size` and `frame` are reserved names.
- `fn effect(uv: vec2<f32>) -> vec4<f32>` returns the colour at `uv` (0,0 top left to 1,1
  bottom right) as straight (not premultiplied) RGBA in 0..1.

The file can declare its own helper functions and constants, but not its own bindings
(`@group`/`@binding`): only the ones below exist, and a file that uses another fails to load. It
can read:

| Name | What |
| --- | --- |
| `src` | The clip's image (`texture_2d<f32>`), straight RGBA, after earlier effects. |
| `smp` | A linear, clamping sampler. |
| `fx.size` | The image size in pixels (`vec2<f32>`). |
| `fx.frame` | The timeline frame number (`f32`), for animated noise. |
| `fx.<param>` | Each `@param`'s current value (`f32`), keyframes applied. |

## Where it runs

Effects run on the clip's own image, before its transform, color adjustments and LUT, in the
order they're stacked in the Inspector. They show in the viewer and in exports.

## Errors

A file that doesn't parse or validate (with naga, the compiler wgpu uses) is listed in red in
the Effects tab with the compiler's message; line numbers are your file's. Clips using it draw as
if the effect weren't there ("passthrough" in the Inspector) until the file is fixed.

## No loops

`loop`, `for` and `while` are rejected (`line N: loops aren't allowed in effects`). A shader runs
once per pixel per frame, and one long or endless loop stalls the whole GPU until the driver
resets it, which can take the app (and on some systems the desktop) down with it. Write a fixed
number of samples out by hand instead, as chromatic aberration does.
