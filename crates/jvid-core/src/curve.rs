//! Grading curves: points on a small graph, evaluated as monotone cubics and baked to a table
//! the renderer uploads as a lookup texture.

use serde::{Deserialize, Serialize};

/// What a curve maps. A level curve maps an input level to an output level and is neutral on
/// the diagonal. Hue and luma curves map a hue or a luma to an amount, neutral at 0.5; hue
/// curves wrap around.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Level,
    Hue,
    Luma,
}

impl Kind {
    /// The curve's value at `x` when it has no points.
    pub fn neutral(self, x: f64) -> f64 {
        match self {
            Self::Level => x,
            Self::Hue | Self::Luma => 0.5,
        }
    }

    /// The points a curve starts with when the first one is added: neutral ends, or for hue
    /// the six primaries and secondaries, so a new point bends only its neighbourhood.
    pub fn seed(self) -> Curve {
        match self {
            Self::Level => Curve::new(&[[0.0, 0.0], [1.0, 1.0]], self),
            Self::Luma => Curve::new(&[[0.0, 0.5], [1.0, 0.5]], self),
            Self::Hue => Curve::new(
                &(0..6).map(|i| [i as f64 / 6.0, 0.5]).collect::<Vec<_>>(),
                self,
            ),
        }
    }
}

/// Up to [`Curve::MAX`] points, x ascending in 0..=1 (0..1 for hue) and y in 0..=1. No points is
/// neutral. Fixed-size so it stays `Copy` like the other keyframe values.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(from = "Vec<[f64; 2]>", into = "Vec<[f64; 2]>")]
pub struct Curve {
    points: [[f64; 2]; Curve::MAX],
    len: u8,
}

impl Default for Curve {
    fn default() -> Self {
        Self {
            points: [[0.0; 2]; Self::MAX],
            len: 0,
        }
    }
}

impl From<Vec<[f64; 2]>> for Curve {
    /// A loaded curve, sorted and clamped; saved hue points are already inside 0..1.
    fn from(p: Vec<[f64; 2]>) -> Self {
        Self::new(&p, Kind::Level)
    }
}

impl From<Curve> for Vec<[f64; 2]> {
    fn from(c: Curve) -> Self {
        c.points().to_vec()
    }
}

impl Curve {
    pub const MAX: usize = 16;
    /// Closer points than this in x merge, the later one winning.
    const GAP: f64 = 1e-4;

    /// `points` sorted by x, clamped into the graph and merged where they share an x; past
    /// [`Curve::MAX`] the rest are dropped. A hue curve's x wraps into 0..1.
    pub fn new(points: &[[f64; 2]], kind: Kind) -> Self {
        let mut p: Vec<[f64; 2]> = points
            .iter()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .map(|&[x, y]| {
                let x = match kind {
                    Kind::Hue => x.rem_euclid(1.0),
                    _ => x.clamp(0.0, 1.0),
                };
                [x, y.clamp(0.0, 1.0)]
            })
            .collect();
        p.sort_by(|a, b| a[0].total_cmp(&b[0]));
        let mut c = Self::default();
        for q in p {
            match c.points().last() {
                Some(last) if q[0] - last[0] < Self::GAP => c.points[c.len as usize - 1] = q,
                _ if (c.len as usize) < Self::MAX => {
                    c.points[c.len as usize] = q;
                    c.len += 1;
                }
                _ => {}
            }
        }
        c
    }

    pub fn points(&self) -> &[[f64; 2]] {
        &self.points[..self.len as usize]
    }

    /// Point by point, when both have as many points; otherwise holds `self`.
    pub fn lerp(&self, to: &Curve, t: f64) -> Curve {
        if self.len != to.len {
            return *self;
        }
        let mut c = *self;
        for (a, b) in c.points[..c.len as usize].iter_mut().zip(to.points()) {
            *a = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        }
        c
    }

    /// The curve at `x`: a monotone cubic through the points, so it never overshoots them. A
    /// level or luma curve holds its end values past its first and last points.
    pub fn at(&self, x: f64, kind: Kind) -> f64 {
        let p = self.points();
        match p {
            [] => kind.neutral(x),
            [only] => only[1],
            _ if kind == Kind::Hue => {
                // A point at 1 (only from a hand-edited file) would sit on the next lap's 0.
                let p: Vec<_> = p.iter().filter(|q| q[0] < 1.0).copied().collect();
                // Three laps, so every point's slope sees its neighbours across the wrap.
                let lap = |d: f64| p.iter().map(move |&[x, y]| [x + d, y]);
                let p: Vec<_> = lap(-1.0).chain(lap(0.0)).chain(lap(1.0)).collect();
                hermite(&p, x.rem_euclid(1.0))
            }
            _ => hermite(p, x),
        }
    }

    /// Whether the curve changes nothing, to within a 16-bit step.
    pub fn is_neutral(&self, kind: Kind) -> bool {
        (0..=256).all(|i| {
            let x = i as f64 / 256.0;
            (self.at(x, kind) - kind.neutral(x)).abs() < 1e-5
        })
    }
}

/// The monotone (Fritsch-Butland) cubic through `p`, x ascending, at `x`; the ends hold.
fn hermite(p: &[[f64; 2]], x: f64) -> f64 {
    let n = p.len();
    if x <= p[0][0] {
        return p[0][1];
    }
    if x >= p[n - 1][0] {
        return p[n - 1][1];
    }
    let slope = |i: usize| (p[i + 1][1] - p[i][1]) / (p[i + 1][0] - p[i][0]);
    // A point's tangent: the harmonic mean of its secants, flat at a turn.
    let tangent = |i: usize| match i {
        0 => slope(0),
        _ if i == n - 1 => slope(n - 2),
        _ => {
            let (a, b) = (slope(i - 1), slope(i));
            if a * b <= 0.0 {
                0.0
            } else {
                2.0 * a * b / (a + b)
            }
        }
    };
    let i = p.partition_point(|q| q[0] <= x) - 1;
    let h = p[i + 1][0] - p[i][0];
    let s = (x - p[i][0]) / h;
    let (s2, s3) = (s * s, s * s * s);
    let y = (2.0 * s3 - 3.0 * s2 + 1.0) * p[i][1]
        + (s3 - 2.0 * s2 + s) * h * tangent(i)
        + (-2.0 * s3 + 3.0 * s2) * p[i + 1][1]
        + (s3 - s2) * h * tangent(i + 1);
    y.clamp(0.0, 1.0)
}

/// Master, Red, Green and Blue level curves, then Hue vs Sat, Hue vs Hue and Luma vs Sat. Each
/// is keyframed under its name in [`Curves::NAMES`] as a [`crate::keyframe::Value::Curve`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Curves {
    pub master: Curve,
    pub red: Curve,
    pub green: Curve,
    pub blue: Curve,
    pub hue_sat: Curve,
    pub hue_hue: Curve,
    pub luma_sat: Curve,
}

impl Curves {
    pub const NAMES: [&str; 7] = [
        "curve_master",
        "curve_red",
        "curve_green",
        "curve_blue",
        "hue_vs_sat",
        "hue_vs_hue",
        "luma_vs_sat",
    ];
    pub const KINDS: [Kind; 7] = [
        Kind::Level,
        Kind::Level,
        Kind::Level,
        Kind::Level,
        Kind::Hue,
        Kind::Hue,
        Kind::Luma,
    ];
    /// The baked table's width per row.
    pub const SIZE: usize = 256;

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn all(&self) -> [&Curve; 7] {
        [
            &self.master,
            &self.red,
            &self.green,
            &self.blue,
            &self.hue_sat,
            &self.hue_hue,
            &self.luma_sat,
        ]
    }

    /// The curve and its kind for a name in [`Curves::NAMES`].
    pub fn get_mut(&mut self, name: &str) -> Option<(&mut Curve, Kind)> {
        let i = Self::NAMES.iter().position(|n| *n == name)?;
        let c = match i {
            0 => &mut self.master,
            1 => &mut self.red,
            2 => &mut self.green,
            3 => &mut self.blue,
            4 => &mut self.hue_sat,
            5 => &mut self.hue_hue,
            _ => &mut self.luma_sat,
        };
        Some((c, Self::KINDS[i]))
    }

    pub fn is_neutral(&self) -> bool {
        self.all()
            .iter()
            .zip(Self::KINDS)
            .all(|(c, k)| c.is_neutral(k))
    }

    /// Two rows of [`Curves::SIZE`] RGBA samples at x = i / (SIZE - 1): Red, Green, Blue and
    /// Master, then Hue vs Sat, Hue vs Hue, Luma vs Sat and 0.
    pub fn bake(&self) -> Vec<[f32; 4]> {
        let c = self.all();
        let row = |idx: [usize; 3], last: Option<usize>| {
            (0..Self::SIZE).map(move |i| {
                let x = i as f64 / (Self::SIZE - 1) as f64;
                let v = |j: usize| c[j].at(x, Self::KINDS[j]) as f32;
                [v(idx[0]), v(idx[1]), v(idx[2]), last.map_or(0.0, v)]
            })
        };
        row([1, 2, 3], Some(0))
            .chain(row([4, 5, 6], None))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_pass_through_their_points_without_overshoot_and_wrap_hue() {
        let level = Curve::new(
            &[[0.0, 0.0], [0.25, 0.15], [0.75, 0.85], [1.0, 1.0]],
            Kind::Level,
        );
        for &[x, y] in level.points() {
            assert!((level.at(x, Kind::Level) - y).abs() < 1e-12);
        }
        // Monotone points give a monotone curve.
        let ys: Vec<f64> = (0..=100)
            .map(|i| level.at(i as f64 / 100.0, Kind::Level))
            .collect();
        assert!(ys.windows(2).all(|w| w[1] >= w[0]));
        assert!(!level.is_neutral(Kind::Level));
        // A peak doesn't overshoot.
        let peak = Curve::new(&[[0.0, 0.0], [0.5, 1.0], [1.0, 0.0]], Kind::Level);
        assert!((0..=100).all(|i| peak.at(i as f64 / 100.0, Kind::Level) <= 1.0));
        // Ends hold past the first and last points.
        let short = Curve::new(&[[0.2, 0.1], [0.8, 0.9]], Kind::Level);
        assert_eq!(
            (short.at(0.0, Kind::Level), short.at(1.0, Kind::Level)),
            (0.1, 0.9)
        );

        // Points on the diagonal, or none, are neutral; so is a flat seeded hue curve.
        assert!(Kind::Level.seed().is_neutral(Kind::Level));
        assert!(Curve::default().is_neutral(Kind::Hue));
        assert!(Kind::Hue.seed().is_neutral(Kind::Hue));
        assert!(Kind::Luma.seed().is_neutral(Kind::Luma));

        // Hue wraps: x 1.1 is 0.1, and the curve meets itself at 0 and 1.
        let hue = Curve::new(&[[0.1, 0.9], [0.5, 0.2], [1.25, 0.5]], Kind::Hue);
        assert_eq!(hue.points()[1], [0.25, 0.5]);
        assert!((hue.at(0.0, Kind::Hue) - hue.at(1.0 - 1e-9, Kind::Hue)).abs() < 1e-6);
        assert!((hue.at(1.1, Kind::Hue) - 0.9).abs() < 1e-12);
        let loaded: Curve = serde_json::from_str("[[0.0,0.2],[1.0,0.8]]").unwrap();
        assert_eq!(
            loaded.at(0.5, Kind::Hue),
            0.2,
            "a saved point at 1 is ignored"
        );
    }

    #[test]
    fn curves_sort_merge_and_cap_their_points_and_lerp_pointwise() {
        let c = Curve::new(
            &[[0.5, 2.0], [0.1, 0.3], [0.5, 0.4], [f64::NAN, 0.0]],
            Kind::Level,
        );
        assert_eq!(c.points(), [[0.1, 0.3], [0.5, 0.4]]);
        let many: Vec<_> = (0..40).map(|i| [i as f64 / 40.0, 0.5]).collect();
        assert_eq!(Curve::new(&many, Kind::Level).points().len(), Curve::MAX);
        let a = Kind::Level.seed();
        let b = Curve::new(&[[0.0, 0.2], [1.0, 0.8]], Kind::Level);
        assert_eq!(a.lerp(&b, 0.5).points(), [[0.0, 0.1], [1.0, 0.9]]);
        let three = Curve::new(&[[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]], Kind::Level);
        assert_eq!(a.lerp(&three, 0.5), a, "mismatched counts hold");
        // Saved as a plain list of points.
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(json, "[[0.0,0.2],[1.0,0.8]]");
        assert_eq!(serde_json::from_str::<Curve>(&json).unwrap(), b);
    }

    #[test]
    fn baked_rows_are_neutral_by_default() {
        let t = Curves::default().bake();
        assert_eq!(t.len(), 2 * Curves::SIZE);
        let x = (100.0f64 / 255.0) as f32;
        assert_eq!(t[100], [x, x, x, x]);
        assert_eq!(t[Curves::SIZE + 100], [0.5, 0.5, 0.5, 0.0]);
        assert!(Curves::default().is_neutral());
    }
}
