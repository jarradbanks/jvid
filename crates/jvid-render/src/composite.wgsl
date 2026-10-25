// One transformed layer drawn into the project frame, blended with its opacity.

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

struct Params {
    // Corners in triangle-strip order: clip-space xy, then uv.
    corners: array<vec4<f32>, 4>,
    // x: opacity 0..1, y: blend mode (jvid_core::model::Blend's index), z: 1 when the image is
    // premultiplied (a nested sequence's frame).
    opacity: vec4<f32>,
    // Color adjustments (jvid_core::model::Transform::grade): exposure, contrast, saturation,
    // temperature in grade.xyzw, tint in grade2.x; grade2.y is 1 when any is non-neutral.
    // grade2.z is the LUT's intensity (0 without one) and grade2.w its size.
    grade: vec4<f32>,
    grade2: vec4<f32>,
    // Lift, gamma, gain and offset per channel (jvid_core::model::Wheel::rgb) in xyz.
    wheels: array<vec4<f32>, 4>,
    // x is 1 when the curves are non-neutral and y the curve table's width.
    curves: vec4<f32>,
}

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_smp: sampler;
@group(0) @binding(2) var<uniform> params: Params;
// The layer's .cube LUT, or a 1x1x1 stand-in when grade2.z is 0.
@group(0) @binding(4) var lut: texture_3d<f32>;
// The layer's baked curves (jvid_core::curve::Curves::bake): Red, Green, Blue and Master in row
// 0, then Hue vs Sat, Hue vs Hue and Luma vs Sat; a 1x1 stand-in when curves.x is 0.
@group(0) @binding(5) var curve_table: texture_2d<f32>;

@vertex
fn vs_layer(@builtin(vertex_index) i: u32) -> Out {
    let c = params.corners[i];
    return Out(vec4<f32>(c.xy, 0.0, 1.0), c.zw);
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3<f32>(0.0031308));
}

// The layer's color adjustments on a straight-alpha sample, in linear light, then its LUT with
// trilinear filtering. Each is skipped when neutral so untouched clips stay bit-identical.
fn graded(c: vec4<f32>) -> vec4<f32> {
    var rgb = curved(adjusted(c.rgb));
    if params.grade2.z > 0.0 {
        // Texel centres: input 0 and 1 land on the first and last nodes.
        let n = params.grade2.w;
        let uvw = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * ((n - 1.0) / n) + 0.5 / n;
        rgb = mix(rgb, textureSampleLevel(lut, image_smp, uvw, 0.0).rgb, params.grade2.z);
    }
    return vec4<f32>(rgb, c.a);
}

fn adjusted(c: vec3<f32>) -> vec3<f32> {
    if params.grade2.y == 0.0 {
        return c;
    }
    let g = params.grade;
    var l = to_linear(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0))) * exp2(g.x);
    l *= vec3<f32>(1.0 + 0.3 * g.w, 1.0 - 0.3 * params.grade2.x, 1.0 - 0.3 * g.w);
    let luma = dot(l, vec3<f32>(0.2126, 0.7152, 0.0722));
    l = max(mix(vec3<f32>(luma), l, 1.0 + g.z), vec3<f32>(0.0));
    l = 0.18 * pow(l / 0.18, vec3<f32>(1.0 + g.y));
    // Offset shifts everything, lift raises black and pins white, gain scales, gamma bends mids.
    let w = params.wheels;
    l += 0.1 * w[3].xyz;
    l = max(l + 0.25 * w[0].xyz * (1.0 - l), vec3<f32>(0.0));
    l = pow(l * exp2(w[2].xyz), exp2(-w[1].xyz));
    return to_srgb(clamp(l, vec3<f32>(0.0), vec3<f32>(1.0)));
}

// Row `row` of the curve table at `x` in 0..1, linear between its samples.
fn curve(x: f32, row: i32) -> vec4<f32> {
    let n = params.curves.y;
    let f = clamp(x, 0.0, 1.0) * (n - 1.0);
    let i = i32(f);
    let a = textureLoad(curve_table, vec2<i32>(i, row), 0);
    let b = textureLoad(curve_table, vec2<i32>(min(i + 1, i32(n) - 1), row), 0);
    return mix(a, b, f - f32(i));
}

fn to_hsv(c: vec3<f32>) -> vec3<f32> {
    let hi = max(c.r, max(c.g, c.b));
    let d = hi - min(c.r, min(c.g, c.b));
    var h = 0.0;
    if d > 0.0 {
        if hi == c.r {
            h = (c.g - c.b) / d;
        } else if hi == c.g {
            h = (c.b - c.r) / d + 2.0;
        } else {
            h = (c.r - c.g) / d + 4.0;
        }
    }
    return vec3<f32>(fract(h / 6.0), select(0.0, d / hi, hi > 0.0), hi);
}

fn from_hsv(c: vec3<f32>) -> vec3<f32> {
    let k = (vec3<f32>(5.0, 3.0, 1.0) + c.x * 6.0) % 6.0;
    return c.z - c.z * c.y * clamp(min(k, 4.0 - k), vec3<f32>(0.0), vec3<f32>(1.0));
}

// The layer's curves in display light: Master then each channel's own, then saturation by hue
// and luma and the hue shift, where 0.5 changes nothing. Skipped when all are neutral.
fn curved(c: vec3<f32>) -> vec3<f32> {
    if params.curves.x == 0.0 {
        return c;
    }
    let v = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let m = vec3<f32>(curve(v.r, 0).a, curve(v.g, 0).a, curve(v.b, 0).a);
    let rgb = vec3<f32>(curve(m.r, 0).r, curve(m.g, 0).g, curve(m.b, 0).b);
    var hsv = to_hsv(rgb);
    let by_hue = curve(hsv.x, 1);
    let by_luma = curve(dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 1).b;
    hsv.y = clamp(hsv.y * 4.0 * by_hue.r * by_luma, 0.0, 1.0);
    hsv.x = fract(hsv.x + by_hue.g - 0.5);
    return from_hsv(hsv);
}

// The image at `uv` with straight alpha.
fn straight(uv: vec2<f32>) -> vec4<f32> {
    let c = textureSample(image, image_smp, uv);
    if params.opacity.z > 0.0 && c.a > 0.0 {
        return vec4<f32>(c.rgb / c.a, c.a);
    }
    return c;
}

// Straight-alpha source in, premultiplied out for premultiplied "over" blending.
@fragment
fn fs_layer(in: Out) -> @location(0) vec4<f32> {
    let c = graded(straight(in.uv));
    let a = c.a * params.opacity.x;
    return vec4<f32>(c.rgb * a, a);
}

// The frame so far, copied out before a non-normal layer; only the blend pipeline binds it.
@group(0) @binding(3) var backdrop: texture_2d<f32>;

fn mix_mode(mode: u32, b: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 1u: { return b * s; }
        case 2u: { return b + s - b * s; }
        case 3u: { return select(2.0 * b * s, 1.0 - 2.0 * (1.0 - b) * (1.0 - s), b > vec3<f32>(0.5)); }
        case 4u: { return min(b + s, vec3<f32>(1.0)); }
        case 5u: { return min(b, s); }
        case 6u: { return max(b, s); }
        case 7u: { return abs(b - s); }
        default: { return s; }
    }
}

// W3C separable blending then source-over, written over the copied backdrop (no fixed blend).
@fragment
fn fs_blend(in: Out) -> @location(0) vec4<f32> {
    let c = graded(straight(in.uv));
    let a = c.a * params.opacity.x;
    let d = textureLoad(backdrop, vec2<i32>(in.pos.xy), 0);
    let b = select(vec3<f32>(0.0), d.rgb / d.a, d.a > 0.0);
    let s = (1.0 - d.a) * c.rgb + d.a * mix_mode(u32(params.opacity.y), b, c.rgb);
    return vec4<f32>(a * s + (1.0 - a) * d.rgb, a + d.a * (1.0 - a));
}
