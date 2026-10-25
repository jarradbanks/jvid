//! User WGSL effects: one `.wgsl` file whose header names it and declares its parameters, run as
//! a full-image pass over a clip before compositing (see [`crate::composite::Compositor::set_effect`]).
//!
//! ```wgsl
//! // @jvid effect "Vignette"
//! // @param strength float 0.0..1.0 = 0.5
//! fn effect(uv: vec2<f32>) -> vec4<f32> {
//!     let c = textureSample(src, smp, uv);
//!     return vec4<f32>(c.rgb * (1.0 - fx.strength * length(uv - 0.5)), c.a);
//! }
//! ```
//!
//! The file defines `fn effect(uv: vec2<f32>) -> vec4<f32>` returning straight RGBA, and may use
//! `src` (the clip's image, straight RGBA), `smp` (a linear sampler) and the uniform `fx`, whose
//! fields are `size` (pixels), `frame` (the timeline frame number), `pass_index` and one `f32`
//! per parameter. `// @passes N` runs the effect N times, each over the last one's output, with
//! `fx.pass_index` counting from 0: a separable blur does x then y.

/// One `@param` line: `@param name float min..max = default`.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

/// A parsed, validated effect.
#[derive(Debug, Clone, PartialEq)]
pub struct Shader {
    /// From `@jvid effect "Name"`.
    pub name: String,
    pub params: Vec<Param>,
    /// From `@passes N`, 1 without it.
    pub passes: u32,
    /// The user's source followed by the generated bindings and entry points.
    pub(crate) module: String,
}

/// Parses the header of `src` and validates the whole effect with naga; the error reads like a
/// compiler message, with line numbers in the user's file.
pub fn parse(src: &str) -> Result<Shader, String> {
    let mut name = None;
    let mut params: Vec<Param> = Vec::new();
    let mut passes = 1;
    for (i, line) in src.lines().enumerate() {
        let err = |m: &str| format!("line {}: {m}", i + 1);
        let Some(rest) = line.trim().strip_prefix("//") else {
            continue;
        };
        let rest = rest.trim();
        if let Some(n) = rest.strip_prefix("@jvid effect") {
            let n = n.trim();
            let n = n.strip_prefix('"').and_then(|n| n.strip_suffix('"'));
            name = Some(
                n.ok_or_else(|| err("expected @jvid effect \"Name\""))?
                    .to_owned(),
            );
        } else if let Some(n) = rest.strip_prefix("@passes") {
            passes = n
                .trim()
                .parse()
                .ok()
                .filter(|n| (1..=8).contains(n))
                .ok_or_else(|| err("expected @passes 1 to 8"))?;
        } else if let Some(p) = rest.strip_prefix("@param") {
            let bad = || err("expected @param name float min..max = default");
            let (decl, default) = p.split_once('=').ok_or_else(bad)?;
            let [pname, ty, range] = decl.split_whitespace().collect::<Vec<_>>()[..] else {
                return Err(bad());
            };
            // shortcut: float only; add int/bool/color types when an effect needs them.
            if ty != "float" {
                return Err(err(&format!(
                    "unsupported type {ty:?}; only float is supported"
                )));
            }
            let (min, max) = range.split_once("..").ok_or_else(bad)?;
            let num = |s: &str| s.trim().parse::<f64>().map_err(|_| bad());
            let (min, max, default) = (num(min)?, num(max)?, num(default)?);
            if min.partial_cmp(&max) != Some(std::cmp::Ordering::Less) {
                return Err(err("min must be less than max"));
            }
            if ["size", "frame", "pass_index"].contains(&pname)
                || params.iter().any(|q| q.name == pname)
            {
                return Err(err(&format!("parameter name {pname:?} is taken")));
            }
            params.push(Param {
                name: pname.to_owned(),
                min,
                max,
                default: default.clamp(min, max),
            });
        }
    }
    let name = name.ok_or("missing the `// @jvid effect \"Name\"` header")?;
    // Appended, so line numbers in errors match the user's file; WGSL declarations are unordered.
    let fields: String = params
        .iter()
        .map(|p| format!(", {}: f32", p.name))
        .collect();
    let module = format!("{src}\n{}", PRELUDE.replace("/*params*/", &fields));
    let parsed = naga::front::wgsl::parse_str(&module).map_err(|e| e.emit_to_string(&module))?;
    let bodies = parsed.functions.iter().map(|(_, f)| &f.body);
    for body in bodies.chain(parsed.entry_points.iter().map(|e| &e.function.body)) {
        if let Some(span) = find_loop(body) {
            return Err(format!(
                "line {}: loops aren't allowed in effects; a long one hangs the GPU. \
                 Unroll it into a fixed number of samples",
                span.location(&module).line_number
            ));
        }
    }
    naga::valid::Validator::new(Default::default(), Default::default())
        .validate(&parsed)
        .map_err(|e| e.emit_to_string(&module))?;
    Ok(Shader {
        name,
        params,
        passes,
        module,
    })
}

/// The first `loop`/`for`/`while` in `block` or its nested blocks.
fn find_loop(block: &naga::Block) -> Option<naga::Span> {
    use naga::Statement as S;
    block.span_iter().find_map(|(st, span)| match st {
        S::Loop { .. } => Some(*span),
        S::Block(b) => find_loop(b),
        S::If { accept, reject, .. } => find_loop(accept).or_else(|| find_loop(reject)),
        S::Switch { cases, .. } => cases.iter().find_map(|c| find_loop(&c.body)),
        _ => None,
    })
}

const PRELUDE: &str = "
struct JvidOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }
struct JvidFx { size: vec2<f32>, frame: f32, pass_index: f32 /*params*/ }
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var<uniform> fx: JvidFx;
@vertex
fn jvid_vs(@builtin(vertex_index) i: u32) -> JvidOut {
    let uv = vec2<f32>(f32(i & 1u), f32(i >> 1u));
    return JvidOut(vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0), uv);
}
@fragment
fn jvid_fs(in: JvidOut) -> @location(0) vec4<f32> { return effect(in.uv); }
";

/// The bundled examples, as (file name, source), copied into a new effects folder.
pub const BUNDLED: [(&str, &str); 6] = [
    ("vignette.wgsl", include_str!("effects/vignette.wgsl")),
    (
        "chromatic_aberration.wgsl",
        include_str!("effects/chromatic_aberration.wgsl"),
    ),
    ("film_grain.wgsl", include_str!("effects/film_grain.wgsl")),
    ("chroma_key.wgsl", include_str!("effects/chroma_key.wgsl")),
    (
        "gaussian_blur.wgsl",
        include_str!("effects/gaussian_blur.wgsl"),
    ),
    ("pixelate.wgsl", include_str!("effects/pixelate.wgsl")),
];

#[cfg(test)]
mod tests;
