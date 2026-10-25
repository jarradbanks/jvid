// A clip's image with its alpha scaled by its masks' combined coverage (see mask.rs).

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

struct Mask {
    // x: shape (0 rectangle, 1 ellipse, 2 path), y: first path vertex, z: vertex count,
    // w: feather in fitted pixels.
    shape: vec4<f32>,
    // x: opacity, y: 1 when inverted, z: 1 when subtracting.
    mode: vec4<f32>,
    // A rectangle's or ellipse's centre in xy and half size in zw.
    rect: vec4<f32>,
}

struct Masks {
    // xy: the fitted image size, z: mask count, w: fitted pixels per output pixel.
    info: vec4<f32>,
    masks: array<Mask, 8>,
    // Flattened path vertices, two per element.
    points: array<vec4<f32>, 256>,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var<uniform> m: Masks;

@vertex
fn vs_mask(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32(i & 1u), f32(i >> 1u));
    return Out(vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0), uv);
}

fn vertex(k: u32) -> vec2<f32> {
    let v = m.points[k / 2u];
    return select(v.xy, v.zw, k % 2u == 1u);
}

// Signed distance to a closed polygon, negative inside (even-odd).
fn path(p: vec2<f32>, first: u32, n: u32) -> f32 {
    var d = 1e30;
    var s = 1.0;
    var b = vertex(first + n - 1u);
    for (var i = 0u; i < n; i++) {
        let a = vertex(first + i);
        let e = b - a;
        let w = p - a;
        let q = w - e * clamp(dot(w, e) / max(dot(e, e), 1e-12), 0.0, 1.0);
        d = min(d, dot(q, q));
        let c = vec3<bool>(p.y >= a.y, (p.y < b.y), (e.x * w.y > e.y * w.x));
        if all(c) || all(!c) {
            s = -s;
        }
        b = a;
    }
    return s * sqrt(d);
}

fn rectangle(p: vec2<f32>, h: vec2<f32>) -> f32 {
    let q = abs(p) - h;
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
}

// Close to the true distance near the outline, which is all the feather needs.
fn ellipse(p: vec2<f32>, h: vec2<f32>) -> f32 {
    let r = max(h, vec2<f32>(1e-3));
    let k0 = length(p / r);
    let k1 = length(p / (r * r));
    if k1 < 1e-6 {
        return -min(r.x, r.y);
    }
    return k0 * (k0 - 1.0) / k1;
}

@fragment
fn fs_mask(in: Out) -> @location(0) vec4<f32> {
    let c = textureSample(src, smp, in.uv);
    let p = (in.uv - 0.5) * m.info.xy;
    let count = u32(m.info.z);
    // A first subtracting mask cuts from the whole image.
    var acc = select(0.0, 1.0, m.masks[0].mode.z > 0.0);
    for (var i = 0u; i < count; i++) {
        let k = m.masks[i];
        var d: f32;
        switch u32(k.shape.x) {
            case 0u: { d = rectangle(p - k.rect.xy, k.rect.zw); }
            case 1u: { d = ellipse(p - k.rect.xy, k.rect.zw); }
            default: { d = path(p, u32(k.shape.y), u32(k.shape.z)); }
        }
        // At least one output pixel wide, so a hard edge is still antialiased.
        let w = max(k.shape.w, m.info.w);
        var v = 1.0 - smoothstep(-w / 2.0, w / 2.0, d);
        v = select(v, 1.0 - v, k.mode.y > 0.0) * k.mode.x;
        acc = select(acc + v - acc * v, acc * (1.0 - v), k.mode.z > 0.0);
    }
    return vec4<f32>(c.rgb, c.a * acc);
}

// An effect limited to a mask: `src` (the effected image) over `orig` by `cover`'s alpha, so
// pixels the mask leaves out stay exactly as they were.
@group(0) @binding(3) var orig: texture_2d<f32>;
@group(0) @binding(4) var cover: texture_2d<f32>;

@fragment
fn fs_mix(in: Out) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    return mix(textureLoad(orig, p, 0), textureLoad(src, p, 0), textureSample(cover, smp, in.uv).a);
}
