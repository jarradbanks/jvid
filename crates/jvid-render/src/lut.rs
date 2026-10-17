//! `.cube` LUTs (the Adobe/Resolve text format): parsed into a 3D grid for the compositor,
//! which samples it with trilinear filtering after a layer's color adjustments.

/// A 3D LUT: `size`³ RGB outputs, red fastest, then green, then blue, as in the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Lut {
    pub size: u32,
    pub rgb: Vec<[f32; 3]>,
}

/// The largest 3D grid accepted (256³ is already 16M entries).
const MAX_3D: u32 = 256;
/// The largest 1D table accepted, and the 3D grid a 1D table is baked into at most.
const MAX_1D: u32 = 65536;
const BAKE: u32 = 65;

/// Parses a `.cube` file. A 1D LUT is baked into a 3D grid so both take one shader path.
pub fn parse(text: &str) -> Result<Lut, String> {
    let (mut one, mut three) = (None, None);
    let (mut min, mut max) = ([0.0f32; 3], [1.0f32; 3]);
    let mut rows = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let err = |e: &str| format!("line {}: {e}", n + 1);
        let line = line.trim();
        let mut words = line.split_whitespace();
        let Some(first) = words.next().filter(|w| !w.starts_with('#')) else {
            continue;
        };
        let floats = |words: std::str::SplitWhitespace| -> Result<Vec<f32>, String> {
            let v = words
                .map(|w| w.parse::<f32>().ok().filter(|f| f.is_finite()))
                .collect::<Option<Vec<_>>>();
            v.ok_or_else(|| err("expected numbers"))
        };
        let triple = |v: Vec<f32>| -> Result<[f32; 3], String> {
            v.try_into().map_err(|_| err("expected three numbers"))
        };
        let size = |max: u32| {
            let s = words.clone().next().and_then(|w| w.parse::<u32>().ok());
            s.filter(|s| (2..=max).contains(s))
                .ok_or_else(|| err(&format!("size must be 2..={max}")))
        };
        match first {
            "TITLE" => {}
            "LUT_1D_SIZE" => one = Some(size(MAX_1D)?),
            "LUT_3D_SIZE" => three = Some(size(MAX_3D)?),
            "DOMAIN_MIN" => min = triple(floats(words)?)?,
            "DOMAIN_MAX" => max = triple(floats(words)?)?,
            // Resolve's one-number form of the domain.
            "LUT_1D_INPUT_RANGE" | "LUT_3D_INPUT_RANGE" => {
                let [a, b] = floats(words)?[..] else {
                    return Err(err("expected two numbers"));
                };
                (min, max) = ([a; 3], [b; 3]);
            }
            w if w.starts_with(|c: char| c.is_ascii_alphabetic()) => {
                // Other keywords (e.g. LUT_IN_VIDEO_RANGE) don't change the table.
                if !rows.is_empty() {
                    return Err(err("keyword after data"));
                }
            }
            _ => rows.push(triple(floats(line.split_whitespace())?)?),
        }
    }
    if (0..3).any(|i| max[i] <= min[i]) {
        return Err("DOMAIN_MAX must exceed DOMAIN_MIN".into());
    }
    let want = |n: u32| {
        if rows.len() == n as usize {
            Ok(())
        } else {
            Err(format!("expected {n} entries, found {}", rows.len()))
        }
    };
    match (one, three) {
        (None, Some(s)) => {
            want(s * s * s)?;
            // shortcut: a 3D LUT's domain must be 0..1; resample others if real files need it.
            if min != [0.0; 3] || max != [1.0; 3] {
                return Err("a 3D LUT's domain must be 0..1".into());
            }
            Ok(Lut { size: s, rgb: rows })
        }
        (Some(s), None) => {
            want(s)?;
            let b = s.min(BAKE);
            // Each channel's output at grid node i: the 1D table read at that input.
            let ch = |c: usize, i: u32| {
                let x = i as f32 / (b - 1) as f32;
                let p = ((x - min[c]) / (max[c] - min[c])).clamp(0.0, 1.0) * (s - 1) as f32;
                let (lo, f) = (p.floor() as usize, p.fract());
                let hi = (lo + 1).min(s as usize - 1);
                rows[lo][c] + (rows[hi][c] - rows[lo][c]) * f
            };
            let rgb = (0..b * b * b)
                .map(|i| [ch(0, i % b), ch(1, i / b % b), ch(2, i / b / b)])
                .collect();
            Ok(Lut { size: b, rgb })
        }
        (None, None) => Err("no LUT_1D_SIZE or LUT_3D_SIZE".into()),
        (Some(_), Some(_)) => Err("both LUT_1D_SIZE and LUT_3D_SIZE".into()),
    }
}

/// Uploads `lut` as an `Rgba8Unorm` 3D texture for [`crate::composite::Compositor::add_lut`].
///
/// shortcut: 8-bit nodes clip outputs to 0..1 and quantize them, fine for the 8-bit composite;
/// switch to `Rgba16Float` with the linear-light pipeline.
pub fn texture(device: &wgpu::Device, queue: &wgpu::Queue, lut: &Lut) -> wgpu::Texture {
    use wgpu::util::DeviceExt;
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let data: Vec<u8> = lut
        .rgb
        .iter()
        .flat_map(|[r, g, b]| [q(*r), q(*g), q(*b), 255])
        .collect();
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("lut"),
            size: wgpu::Extent3d {
                width: lut.size,
                height: lut.size,
                depth_or_array_layers: lut.size,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        Default::default(),
        &data,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_3d() {
        let text = "# comment\nTITLE \"swap\"\nLUT_3D_SIZE 2\n\n\
            0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        let lut = parse(text).unwrap();
        assert_eq!(lut.size, 2);
        assert_eq!(lut.rgb[1], [1.0, 0.0, 0.0], "red varies fastest");
        assert_eq!(lut.rgb[4], [0.0, 0.0, 1.0]);
    }

    #[test]
    fn bakes_1d() {
        // Inverts each channel over a 0..2 domain: input 0.5 reads the table at 0.25.
        let text = "LUT_1D_SIZE 3\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2\n1 1 1\n0.5 0.5 0.5\n0 0 0\n";
        let lut = parse(text).unwrap();
        assert_eq!(lut.size, 3);
        assert_eq!(lut.rgb.len(), 27);
        assert_eq!(lut.rgb[0], [1.0; 3]);
        assert_eq!(lut.rgb[1], [0.75, 1.0, 1.0]);
        assert_eq!(lut.rgb[26], [0.5; 3]);
    }

    #[test]
    fn malformed_input_errors_without_panicking() {
        let bad = [
            "",
            "garbage",
            "LUT_3D_SIZE",
            "LUT_3D_SIZE 1",
            "LUT_3D_SIZE 99999999999",
            "LUT_3D_SIZE -2",
            "LUT_3D_SIZE 2\n0 0 0\n",
            "LUT_3D_SIZE 2\n0 0\n",
            "LUT_3D_SIZE 2\n0 0 nan\n",
            "LUT_3D_SIZE 2\n0 0 inf\n",
            "LUT_3D_SIZE 2\n0 0 x\n",
            "LUT_1D_SIZE 2\nLUT_3D_SIZE 2\n0 0 0\n1 1 1\n",
            "LUT_1D_SIZE 2\nDOMAIN_MIN 1 1 1\n0 0 0\n1 1 1\n",
            "LUT_1D_SIZE 2\nDOMAIN_MIN 0 0\n0 0 0\n1 1 1\n",
            "LUT_1D_SIZE 2\nLUT_1D_INPUT_RANGE 0\n0 0 0\n1 1 1\n",
            "LUT_1D_SIZE 2\n0 0 0\nFOO\n1 1 1\n",
            "LUT_3D_SIZE 2\nDOMAIN_MAX 2 2 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
            "\u{feff}\u{0}\u{1}",
        ];
        for text in bad {
            assert!(parse(text).is_err(), "{text:?}");
        }
        // Truncations of a valid file never panic.
        let good = "LUT_1D_SIZE 3\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n0 0 0\n0.5 0.5 0.5\n1 1 1\n";
        for i in 0..good.len() {
            let _ = parse(&good[..i]);
        }
        assert!(parse(good).is_ok());
    }
}
