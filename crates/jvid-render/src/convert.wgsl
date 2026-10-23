// YUV 4:2:0 to RGB, drawn over the whole target; HDR tone-mapped to SDR.

struct Convert {
    // rgb[i] = dot(rows[i].xyz, yuv) + rows[i].w, with yuv sampled as 0..1.
    rows: array<vec4<f32>, 3>,
}

@group(0) @binding(0) var y_plane: texture_2d<f32>;
@group(0) @binding(1) var u_plane: texture_2d<f32>;
@group(0) @binding(2) var v_plane: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;
@group(0) @binding(4) var<uniform> convert: Convert;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// One triangle covering the target.
@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return Out(vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0), uv);
}

fn to_rgb(yuv: vec3<f32>) -> vec3<f32> {
    let rgb = vec3<f32>(
        dot(convert.rows[0].xyz, yuv) + convert.rows[0].w,
        dot(convert.rows[1].xyz, yuv) + convert.rows[1].w,
        dot(convert.rows[2].xyz, yuv) + convert.rows[2].w,
    );
    return clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_convert(in: Out) -> @location(0) vec4<f32> {
    let yuv = vec3<f32>(
        textureSample(y_plane, smp, in.uv).r,
        textureSample(u_plane, smp, in.uv).r,
        textureSample(v_plane, smp, in.uv).r,
    );
    return vec4<f32>(to_rgb(yuv), 1.0);
}

// HDR (PQ or HLG, BT.2020) tone-mapped to SDR BT.709.

// A 10-bit sample arrives as its two bytes in an rg8 texel, sampled nearest: GPUs filter 8-bit
// texels at 8-bit precision, which the high byte's ×256 would blow up.
// Over 4 it reads as an 8-bit-scale code, which lines the 64-940 range up with 16-235.
fn deep(t: vec4<f32>) -> f32 {
    return (t.r + t.g * 256.0) * 0.25;
}

// Non-linear BT.2020 R'G'B'.
fn signal(uv: vec2<f32>) -> vec3<f32> {
    return to_rgb(vec3<f32>(
        deep(textureSample(y_plane, smp, uv)),
        deep(textureSample(u_plane, smp, uv)),
        deep(textureSample(v_plane, smp, uv)),
    ));
}

// PQ (SMPTE ST 2084).
const M1: f32 = 0.1593017578125;
const M2: f32 = 78.84375;
const C1: f32 = 0.8359375;
const C2: f32 = 18.8515625;
const C3: f32 = 18.6875;

fn pq_to_nits(e: vec3<f32>) -> vec3<f32> {
    let p = pow(clamp(e, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / M2));
    return 10000.0 * pow(max(p - C1, vec3<f32>(0.0)) / (C2 - C3 * p), vec3<f32>(1.0 / M1));
}

fn nits_to_pq(nits: vec3<f32>) -> vec3<f32> {
    let y = pow(clamp(nits / 10000.0, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(M1));
    return pow((C1 + C2 * y) / (1.0 + C3 * y), vec3<f32>(M2));
}

// HLG to display light (BT.2100): inverse OETF, then the OOTF for a 1000 cd/m² display
// (system gamma 1.2), which puts 75% signal at the 203 cd/m² reference white.
fn hlg_to_nits(e: vec3<f32>) -> vec3<f32> {
    let a = 0.17883277;
    let b = 0.28466892;
    let c = 0.55991073;
    let scene = select((exp((e - c) / a) + b) / 12.0, e * e / 3.0, e <= vec3<f32>(0.5));
    let ys = dot(vec3<f32>(0.2627, 0.6780, 0.0593), scene);
    return 1000.0 * pow(max(ys, 1e-6), 0.2) * scene;
}

// The tone map is BT.2390's EETF, the ITU's reference HDR-to-SDR curve: below a knee (about
// 88 cd/m² here) PQ passes through untouched, so mid-grey and skin keep their brightness, and
// above it a Hermite spline rolls highlights off into the SDR peak. Hable or Reinhard would
// compress the whole range and darken mid-tones. Source peak 1000 cd/m² (typical HDR10
// mastering and HLG's nominal display); SDR peak the 203 cd/m² reference white (BT.2408), so
// SDR exported as HDR comes back where it started. Applied per R'G'B' channel.
const SRC_PEAK: f32 = 0.7518271; // PQ of 1000 cd/m²
const MAX_LUM: f32 = 0.7723702; // PQ of 203 cd/m², over SRC_PEAK
const KNEE: f32 = 0.6585554; // 1.5 MAX_LUM - 0.5

fn eetf(pq: vec3<f32>) -> vec3<f32> {
    let e = min(pq / SRC_PEAK, vec3<f32>(1.0));
    let t = (e - KNEE) / (1.0 - KNEE);
    let t2 = t * t;
    let t3 = t2 * t;
    let spline = (2.0 * t3 - 3.0 * t2 + 1.0) * KNEE + (t3 - 2.0 * t2 + t) * (1.0 - KNEE)
        + (-2.0 * t3 + 3.0 * t2) * MAX_LUM;
    return select(spline, e, e < vec3<f32>(KNEE)) * SRC_PEAK;
}

// Columns are the rows of linear BT.2020 to BT.709 (BT.2087), so `v * TO_BT709` applies it.
const TO_BT709 = mat3x3<f32>(
    vec3<f32>(1.6605, -0.5876, -0.0728),
    vec3<f32>(-0.1246, 1.1329, -0.0083),
    vec3<f32>(-0.0182, -0.1006, 1.1187),
);

// Display light to BT.709 R'G'B' for a BT.1886 (gamma 2.4) display, out-of-gamut colours clipped.
fn to_sdr(nits: vec3<f32>) -> vec4<f32> {
    let linear = clamp(nits / 203.0 * TO_BT709, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(pow(linear, vec3<f32>(1.0 / 2.4)), 1.0);
}

@fragment
fn fs_pq(in: Out) -> @location(0) vec4<f32> {
    return to_sdr(pq_to_nits(eetf(signal(in.uv))));
}

@fragment
fn fs_hlg(in: Out) -> @location(0) vec4<f32> {
    return to_sdr(pq_to_nits(eetf(nits_to_pq(hlg_to_nits(signal(in.uv))))));
}
