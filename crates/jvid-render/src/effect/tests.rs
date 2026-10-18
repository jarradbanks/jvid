use super::*;

const INVERT: &str = "// @jvid effect \"Invert\"
// @param amount float 0.0..1.0 = 2.0
fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = textureSample(src, smp, uv);
    return vec4<f32>(mix(c.rgb, 1.0 - c.rgb, fx.amount), c.a);
}
";

#[test]
fn parses_header_and_clamps_default() {
    let s = parse(INVERT).unwrap();
    assert_eq!(s.name, "Invert");
    assert_eq!(
        s.params,
        [Param {
            name: "amount".into(),
            min: 0.0,
            max: 1.0,
            default: 1.0
        }]
    );
}

#[test]
fn bundled_effects_validate() {
    for (file, src) in BUNDLED {
        let s = parse(src).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(!s.params.is_empty(), "{file}");
    }
}

#[test]
fn header_errors_name_the_line() {
    let e = parse("fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(0.0); }").unwrap_err();
    assert!(e.contains("@jvid effect"), "{e}");
    let bad = |line: &str| parse(&format!("// @jvid effect \"X\"\n{line}\n")).unwrap_err();
    assert!(bad("// @param a float 0..1").starts_with("line 2:"));
    assert!(bad("// @param a int 0..1 = 0").contains("only float"));
    assert!(bad("// @param a float 1..0 = 0").contains("less than"));
    assert!(bad("// @param a float NaN..1 = 0").contains("less than"));
    assert!(bad("// @param size float 0..1 = 0").contains("taken"));
}

#[test]
fn wgsl_errors_are_readable_with_user_line_numbers() {
    let src =
        "// @jvid effect \"X\"\nfn effect(uv: vec2<f32>) -> vec4<f32> {\n    return nope;\n}\n";
    let e = parse(src).unwrap_err();
    assert!(e.contains("nope") && e.contains(":3:"), "{e}");
    let e = parse("// @jvid effect \"X\"\n").unwrap_err();
    assert!(e.contains("effect"), "{e}");
}

#[test]
fn loops_are_rejected_with_their_line() {
    for l in [
        "loop { }",
        "for (var i = 0; i < 9; i++) { }",
        "if true { while true { } }",
    ] {
        let src = format!(
            "// @jvid effect \"X\"\nfn effect(uv: vec2<f32>) -> vec4<f32> {{\n    {l}\n    return vec4<f32>(0.0);\n}}\n"
        );
        let e = parse(&src).unwrap_err();
        assert!(e.starts_with("line 3:") && e.contains("loops"), "{l}: {e}");
    }
}
