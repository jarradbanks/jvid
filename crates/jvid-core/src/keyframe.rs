//! Keyframed values and interpolation, and the speed ramps they key.
//!
//! Keyframe times are in the clip's source time, so they travel with the clip
//! when it moves and stay on the same frame of media when it is trimmed.

use std::cell::RefCell;

use serde::{Deserialize, Serialize};

use crate::time::Time;

/// An animatable value. Color is straight RGBA in 0..=1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Scalar(f64),
    Vec2([f64; 2]),
    Color([f64; 4]),
}

impl Value {
    /// Linear blend; mismatched kinds hold `self`.
    fn lerp(self, to: Value, t: f64) -> Value {
        let l = |a: f64, b: f64| a + (b - a) * t;
        match (self, to) {
            (Self::Scalar(a), Self::Scalar(b)) => Self::Scalar(l(a, b)),
            (Self::Vec2(a), Self::Vec2(b)) => Self::Vec2([l(a[0], b[0]), l(a[1], b[1])]),
            (Self::Color(a), Self::Color(b)) => Self::Color(std::array::from_fn(|i| l(a[i], b[i]))),
            _ => self,
        }
    }
}

/// How a keyframe eases into the next one.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum Easing {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// Keeps this keyframe's value until the next one.
    Hold,
    /// CSS-style `cubic-bezier(x1, y1, x2, y2)`; x is clamped to 0..=1.
    Bezier([f64; 4]),
}

impl Easing {
    /// Maps linear progress `t` in 0..=1 to eased progress.
    pub fn apply(self, t: f64) -> f64 {
        match self {
            Self::Linear => t,
            Self::Hold => 0.0,
            e => bezier(e.handles().expect("eased"), t),
        }
    }

    /// The `cubic-bezier` handles of an eased curve; `None` for Linear and Hold.
    fn handles(self) -> Option<[f64; 4]> {
        match self {
            Self::Linear | Self::Hold => None,
            Self::EaseIn => Some([0.42, 0.0, 1.0, 1.0]),
            Self::EaseOut => Some([0.0, 0.0, 0.58, 1.0]),
            Self::EaseInOut => Some([0.42, 0.0, 0.58, 1.0]),
            Self::Bezier(p) => Some(p),
        }
    }
}

fn cubic(a: f64, b: f64, s: f64) -> f64 {
    let r = 1.0 - s;
    3.0 * r * r * s * a + 3.0 * r * s * s * b + s * s * s
}

/// The derivative of [`cubic`] in `s`.
fn cubic_slope(a: f64, b: f64, s: f64) -> f64 {
    let r = 1.0 - s;
    3.0 * r * r * a + 6.0 * r * s * (b - a) + 3.0 * s * s * (1.0 - b)
}

/// The `s` in `lo..hi` where increasing `f(s)` reaches `t`, by bisection.
fn solve(mut lo: f64, mut hi: f64, t: f64, f: impl Fn(f64) -> f64) -> f64 {
    for _ in 0..48 {
        let mid = (lo + hi) / 2.0;
        if f(mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}

/// Solves x(s) = t by bisection (monotonic in `t` by construction), returns y(s).
fn bezier([x1, y1, x2, y2]: [f64; 4], t: f64) -> f64 {
    let (x1, x2) = (x1.clamp(0.0, 1.0), x2.clamp(0.0, 1.0));
    let t = t.clamp(0.0, 1.0);
    cubic(y1, y2, solve(0.0, 1.0, t, |s| cubic(x1, x2, s)))
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Source time of the clip's media.
    pub time: Time,
    pub value: Value,
    /// Easing toward the next keyframe.
    pub easing: Easing,
}

/// The value at source time `t`; keyframes are sorted by time, one per time.
/// Before the first and after the last, the end values hold.
pub fn value_at(keys: &[Keyframe], t: Time) -> Option<Value> {
    let i = keys.partition_point(|k| k.time <= t);
    match (i.checked_sub(1).map(|j| &keys[j]), keys.get(i)) {
        (None, b) => b.map(|k| k.value),
        (Some(a), None) => Some(a.value),
        (Some(a), Some(b)) => {
            let p = (t - a.time).0 as f64 / (b.time - a.time).0 as f64;
            Some(a.value.lerp(b.value, a.easing.apply(p)))
        }
    }
}

/// The param that keys a clip's speed: source time per timeline time, so 2.0 plays twice as fast.
pub const SPEED: &str = "speed";

/// The slowest and fastest a keyed speed plays, as the Inspector allows. Also keeps the speed
/// integrals finite.
pub const SPEEDS: std::ops::RangeInclusive<f64> = 0.01..=100.0;

/// A keyed speed held to [`SPEEDS`]; a value of another kind plays at 1.
pub fn speed(v: Value) -> f64 {
    match v {
        Value::Scalar(s) => s.clamp(*SPEEDS.start(), *SPEEDS.end()),
        _ => 1.0,
    }
}

/// The speed keyed in `keys` at source time `s`.
pub fn speed_at(keys: &[Keyframe], s: Time) -> f64 {
    value_at(keys, s).map_or(1.0, speed)
}

// Speed ramps. Timeline time is the integral of 1/speed over source time. Linear and Hold
// segments integrate exactly. An eased one takes 3-point Gauss-Legendre over pieces of its
// bezier parameter: split where the speed turns or meets an end of `SPEEDS`, so the integrand
// is smooth and monotone on each, then halved until one piece agrees with its two halves to
// `TOL`. That holds relative error under 1e-5 for any handles the easing editor allows and any
// speeds in `SPEEDS` (tested), so under a 60 fps frame for any segment up to 25 minutes long on
// the timeline. Each thread integrates a set of keys once and looks it up after.

/// The most the speed may change by, as a ratio, across one piece of an eased segment.
const PIECE: f64 = 1.5;
/// The most a piece's integral may differ from the sum over its halves, relative to it.
const TOL: f64 = 1e-7;
/// How many integrated ramps each thread keeps.
const CACHED: usize = 64;

thread_local! {
    /// Recently integrated ramps and the keys they come from, oldest first.
    static RAMPS: RefCell<Vec<(Vec<Keyframe>, Ramp)>> = const { RefCell::new(Vec::new()) };
}

#[cfg(debug_assertions)]
thread_local! {
    static INTEGRATED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many pieces of eased ramps this thread has integrated, so a test can check that a hot
/// path doesn't integrate per call.
#[cfg(debug_assertions)]
pub fn integrated() -> usize {
    INTEGRATED.get()
}

/// Calls `f` with the ramp `keys` key, integrating it only if this thread hasn't lately.
fn ramp<R>(keys: &[Keyframe], f: impl FnOnce(&Ramp) -> R) -> R {
    RAMPS.with_borrow_mut(|cache| {
        let i = match cache.iter().position(|(k, _)| k == keys) {
            Some(i) => i,
            None => {
                if cache.len() == CACHED {
                    cache.remove(0);
                }
                cache.push((keys.to_vec(), Ramp::new(keys)));
                cache.len() - 1
            }
        };
        f(&cache[i].1)
    })
}

/// Timeline flicks from the first of `keys` to source time `s` (negative before it) at the
/// speed they key; the end speeds hold beyond them. `keys` must not be empty.
pub fn ramp_time(keys: &[Keyframe], s: f64) -> f64 {
    let (first, last) = (&keys[0], &keys[keys.len() - 1]);
    if s <= first.time.0 as f64 {
        return (s - first.time.0 as f64) / speed(first.value);
    }
    ramp(keys, |r| {
        let i = r.segs.partition_point(|g| g.s0 + g.len <= s);
        match r.segs.get(i) {
            Some(g) => r.at[i] + g.time(s - g.s0),
            None => r.at[i] + (s - last.time.0 as f64) / speed(last.value),
        }
    })
}

/// The source time `t` timeline flicks after the first of `keys`, the inverse of [`ramp_time`].
pub fn ramp_source(keys: &[Keyframe], t: f64) -> f64 {
    let (first, last) = (&keys[0], &keys[keys.len() - 1]);
    if t <= 0.0 {
        return first.time.0 as f64 + t * speed(first.value);
    }
    ramp(keys, |r| {
        let i = r.at[1..].partition_point(|&a| a <= t);
        match r.segs.get(i) {
            Some(g) => g.s0 + g.source(t - r.at[i]),
            None => last.time.0 as f64 + (t - r.at[i]) * speed(last.value),
        }
    })
}

/// A speed curve integrated once: its segments and the timeline flicks from the first key to
/// each key.
struct Ramp {
    segs: Vec<Segment>,
    at: Vec<f64>,
}

impl Ramp {
    fn new(keys: &[Keyframe]) -> Self {
        let segs: Vec<_> = keys.windows(2).map(Segment::new).collect();
        let mut at = vec![0.0];
        for g in &segs {
            at.push(at[at.len() - 1] + g.total());
        }
        Self { segs, at }
    }
}

/// One keyframe segment of a speed curve: source time `s0..s0 + len` from speed `a` to `b`.
struct Segment {
    s0: f64,
    len: f64,
    a: f64,
    b: f64,
    easing: Easing,
    /// An eased segment's pieces of the bezier parameter in order, each as its end and the
    /// timeline time from the segment's start to there; empty otherwise.
    pieces: Vec<(f64, f64)>,
}

impl Segment {
    fn new(w: &[Keyframe]) -> Self {
        let mut g = Segment {
            s0: w[0].time.0 as f64,
            len: (w[1].time - w[0].time).0 as f64,
            a: speed(w[0].value),
            b: speed(w[1].value),
            easing: w[0].easing,
            pieces: Vec::new(),
        };
        if let Some(h) = g.easing.handles().filter(|_| g.a != g.b) {
            g.pieces = g.split(h);
        }
        g
    }

    fn total(&self) -> f64 {
        self.time(self.len)
    }

    /// Timeline time over the first `d` of the segment's source.
    fn time(&self, d: f64) -> f64 {
        let (a, k) = (self.a, (self.b - self.a) / self.len);
        match self.easing.handles() {
            _ if self.easing == Easing::Hold || k == 0.0 => d / a,
            None => (k * d / a).ln_1p() / k,
            Some(h) => {
                let [x1, _, x2, _] = clamped(h);
                let u = solve(0.0, 1.0, d / self.len, |u| cubic(x1, x2, u));
                let i = self.pieces.partition_point(|p| p.0 < u);
                let (u0, t0) = i.checked_sub(1).map_or((0.0, 0.0), |j| self.pieces[j]);
                t0 + self.piece(h, u0, u)
            }
        }
    }

    /// The source time, from the segment's start, `t` timeline time into it.
    fn source(&self, t: f64) -> f64 {
        let (a, k) = (self.a, (self.b - self.a) / self.len);
        match self.easing.handles() {
            _ if self.easing == Easing::Hold || k == 0.0 => a * t,
            None => a * (k * t).exp_m1() / k,
            Some(h) => {
                let [x1, _, x2, _] = clamped(h);
                let i = self.pieces.partition_point(|p| p.1 <= t);
                let (u0, t0) = i.checked_sub(1).map_or((0.0, 0.0), |j| self.pieces[j]);
                let u = self.pieces.get(i).map_or(1.0, |&(u1, _)| {
                    solve(u0, u1, t - t0, |u| self.piece(h, u0, u))
                });
                self.len * cubic(x1, x2, u)
            }
        }
    }

    /// The speed at bezier parameter `u`, before it is held to [`SPEEDS`].
    fn raw_speed(&self, [_, y1, _, y2]: [f64; 4], u: f64) -> f64 {
        self.a + (self.b - self.a) * cubic(y1, y2, u)
    }

    /// The speed at bezier parameter `u`.
    fn speed_at(&self, h: [f64; 4], u: f64) -> f64 {
        self.raw_speed(h, u).clamp(*SPEEDS.start(), *SPEEDS.end())
    }

    /// Timeline time over parameter `u0..u1`, by 3-point Gauss-Legendre.
    fn piece(&self, h: [f64; 4], u0: f64, u1: f64) -> f64 {
        #[cfg(debug_assertions)]
        INTEGRATED.set(INTEGRATED.get() + 1);
        let [x1, _, x2, _] = clamped(h);
        let f = |u| self.len * cubic_slope(x1, x2, u) / self.speed_at(h, u);
        let (c, r, x) = ((u0 + u1) / 2.0, (u1 - u0) / 2.0, 0.6f64.sqrt());
        r * (5.0 * f(c - r * x) + 8.0 * f(c) + 5.0 * f(c + r * x)) / 9.0
    }

    /// The pieces of the parameter, as [`Segment::pieces`] holds them. Each is at most a
    /// quarter long, the speed changes by at most `PIECE` across it, and it agrees with the sum
    /// over its halves to `TOL`.
    fn split(&self, h: [f64; 4]) -> Vec<(f64, f64)> {
        let mut todo: Vec<_> = self
            .breaks(h)
            .windows(2)
            .rev()
            .map(|w| (w[0], w[1], 0))
            .collect();
        let (mut out, mut t) = (Vec::new(), 0.0);
        while let Some((u0, u1, depth)) = todo.pop() {
            let (v0, v1, mid) = (self.speed_at(h, u0), self.speed_at(h, u1), (u0 + u1) / 2.0);
            let whole = self.piece(h, u0, u1);
            let coarse = u1 - u0 > 0.25 || v0.max(v1) > PIECE * v0.min(v1);
            if depth < 16
                && (coarse
                    || (whole - self.piece(h, u0, mid) - self.piece(h, mid, u1)).abs()
                        > TOL * whole)
            {
                todo.extend([(mid, u1, depth + 1), (u0, mid, depth + 1)]);
            } else {
                t += whole;
                out.push((u1, t));
            }
        }
        out
    }

    /// 0, 1 and where the speed turns or meets an end of [`SPEEDS`] between them, in order.
    fn breaks(&self, h: [f64; 4]) -> Vec<f64> {
        let [_, y1, _, y2] = h;
        // The turns, where y'(u) / 3 = p u² + q u + y1 is 0.
        let (p, q) = (3.0 * (y1 - y2) + 1.0, 2.0 * (y2 - 2.0 * y1));
        let d = q * q - 4.0 * p * y1;
        let turns = match () {
            _ if p == 0.0 => vec![-y1 / q],
            _ if d > 0.0 => vec![(-q - d.sqrt()) / (2.0 * p), (-q + d.sqrt()) / (2.0 * p)],
            _ => Vec::new(),
        };
        let mut u: Vec<f64> = turns.into_iter().filter(|u| 0.0 < *u && *u < 1.0).collect();
        u.extend([0.0, 1.0]);
        u.sort_by(f64::total_cmp);
        // The speed is monotone between turns, so it meets each end at most once there.
        let mut out = vec![0.0];
        for w in u.windows(2) {
            for end in [*SPEEDS.start(), *SPEEDS.end()] {
                let (v0, v1) = (self.raw_speed(h, w[0]) - end, self.raw_speed(h, w[1]) - end);
                if v0 * v1 < 0.0 {
                    let rising = |u| (self.raw_speed(h, u) - end) * v1.signum();
                    out.push(solve(w[0], w[1], 0.0, rising));
                }
            }
            out.push(w[1]);
        }
        out.sort_by(f64::total_cmp);
        out
    }
}

/// Bezier handles with x held to 0..=1, as [`bezier`] holds them, so the curve runs forward.
fn clamped([x1, y1, x2, y2]: [f64; 4]) -> [f64; 4] {
    [x1.clamp(0.0, 1.0), y1, x2.clamp(0.0, 1.0), y2]
}

/// Inserts `k`, replacing any keyframe at the same time.
pub(crate) fn insert(keys: &mut Vec<Keyframe>, k: Keyframe) {
    match keys.binary_search_by(|x| x.time.cmp(&k.time)) {
        Ok(i) => keys[i] = k,
        Err(i) => keys.insert(i, k),
    }
}

pub(crate) fn remove(keys: &mut Vec<Keyframe>, time: Time) -> Option<Keyframe> {
    let i = keys.binary_search_by(|x| x.time.cmp(&time)).ok()?;
    Some(keys.remove(i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn key(t: i64, v: f64, easing: Easing) -> Keyframe {
        Keyframe {
            time: Time(t),
            value: Value::Scalar(v),
            easing,
        }
    }

    #[test]
    fn interpolates_and_holds_ends() {
        let mut keys = Vec::new();
        assert_eq!(value_at(&keys, Time(0)), None);
        insert(&mut keys, key(20, 10.0, Easing::Hold));
        insert(&mut keys, key(10, 0.0, Easing::Linear));
        insert(&mut keys, key(30, 99.0, Easing::Linear));
        assert_eq!(value_at(&keys, Time(0)), Some(Value::Scalar(0.0)));
        assert_eq!(value_at(&keys, Time(15)), Some(Value::Scalar(5.0)));
        assert_eq!(value_at(&keys, Time(20)), Some(Value::Scalar(10.0)));
        assert_eq!(value_at(&keys, Time(29)), Some(Value::Scalar(10.0)));
        assert_eq!(value_at(&keys, Time(40)), Some(Value::Scalar(99.0)));
        insert(&mut keys, key(30, 1.0, Easing::Linear)); // replaces
        assert_eq!(keys.len(), 3);
        assert_eq!(
            remove(&mut keys, Time(20)).unwrap().value,
            Value::Scalar(10.0)
        );
        assert_eq!(remove(&mut keys, Time(20)), None);
    }

    #[test]
    fn vec2_and_color_lerp() {
        let a = Value::Vec2([0.0, 10.0]).lerp(Value::Vec2([10.0, 0.0]), 0.5);
        assert_eq!(a, Value::Vec2([5.0, 5.0]));
        let c = Value::Color([0.0; 4]).lerp(Value::Color([1.0; 4]), 0.25);
        assert_eq!(c, Value::Color([0.25; 4]));
        assert_eq!(
            Value::Scalar(1.0).lerp(Value::Vec2([0.0; 2]), 0.5),
            Value::Scalar(1.0)
        );
    }

    #[test]
    fn presets_hit_endpoints_and_bend() {
        for e in [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
        ] {
            assert!(e.apply(0.0).abs() < 1e-9 && (e.apply(1.0) - 1.0).abs() < 1e-9);
        }
        assert!(Easing::EaseIn.apply(0.5) < 0.5);
        assert!(Easing::EaseOut.apply(0.5) > 0.5);
        assert!((Easing::EaseInOut.apply(0.5) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn linear_ramp_integrates_exactly() {
        // 100% to 200% over 10 s of source: t(s) = L ln(1 + s / L), so 10 s plays in 10 ln 2.
        let l = 10.0 * 705_600_000.0;
        let keys = [
            key(0, 1.0, Easing::Linear),
            key(l as i64, 2.0, Easing::Linear),
        ];
        for f in [0.0f64, 0.1, 0.5, 0.9, 1.0] {
            let want = l * (1.0 + f).ln();
            assert!((ramp_time(&keys, f * l) - want).abs() < 1e-3, "{f}");
            assert!((ramp_source(&keys, want) - f * l).abs() < 1e-3, "{f}");
        }
        // The end speeds hold beyond the keys.
        assert_eq!(ramp_time(&keys, -100.0), -100.0);
        assert!((ramp_time(&keys, l + 200.0) - (l * 2f64.ln() + 100.0)).abs() < 1e-3);
    }

    #[test]
    fn eased_ramps_stay_within_the_stated_bound() {
        // Simpson's rule over `n` steps of `f` on 0..=1.
        let simpson = |n: usize, f: &dyn Fn(f64) -> f64| {
            let h = 1.0 / n as f64;
            let w = |i| {
                if i == 0 || i == n {
                    1.0
                } else {
                    2.0 + 2.0 * (i % 2) as f64
                }
            };
            (0..=n).map(|i| w(i) * f(i as f64 * h)).sum::<f64>() * h / 3.0
        };
        let len = 705_600_000_000; // 1000 s keeps flick rounding out of the reference.
        // The easing editor's handles go from y -1 to 2: a back ease and an overshoot run the
        // speed past the keyed ones and into the ends of SPEEDS.
        for (easing, within) in [
            (Easing::EaseIn, true),
            (Easing::EaseOut, true),
            (Easing::EaseInOut, true),
            (Easing::Bezier([0.3, 0.1, 0.7, 0.9]), true),
            (Easing::Bezier([0.0, 1.0, 1.0, 0.0]), true),
            (Easing::Bezier([0.68, -0.55, 0.27, 1.55]), false),
            (Easing::Bezier([0.5, -1.0, 0.5, 2.0]), false),
        ] {
            let h = clamped(easing.handles().unwrap());
            for (a, b) in [(1.0, 2.0), (0.01, 100.0), (100.0, 0.01), (0.25, 4.0)] {
                let keys = [key(0, a, easing), key(len, b, Easing::Linear)];
                let got = ramp_time(&keys, len as f64);
                // Reference: dense Simpson over the bezier parameter, where 1% peaks stay smooth.
                let v = |u| (a + (b - a) * cubic(h[1], h[3], u)).clamp(0.01, 100.0);
                let want = simpson(400_000, &|u| len as f64 * cubic_slope(h[0], h[2], u) / v(u));
                assert!((got - want).abs() < 1e-5 * want, "{easing:?} {a}->{b}");
                if within && a.max(b) / a.min(b) < 10.0 {
                    // And over source time at the keyed speed, which checks the substitution.
                    let s = simpson(20_000, &|x| {
                        1.0 / speed_at(&keys, Time((x * len as f64) as i64))
                    });
                    assert!(
                        (got - s * len as f64).abs() < 1e-6 * want,
                        "{easing:?} {a}->{b}"
                    );
                }
                let mid = ramp_time(&keys, len as f64 / 3.0);
                let back = ramp_source(&keys, mid);
                assert!((back - len as f64 / 3.0).abs() < 1.0, "{easing:?} {a}->{b}");
            }
        }
    }

    #[test]
    fn ramps_round_trip_across_segments() {
        let keys = [
            key(1_000, 0.5, Easing::Hold),
            key(5_000, 3.0, Easing::EaseInOut),
            key(9_000, 1.0, Easing::Linear),
            key(12_000, 1.0, Easing::Linear),
        ];
        assert_eq!(ramp_time(&keys, 1_000.0), 0.0);
        assert_eq!(ramp_time(&keys, 3_000.0), 4_000.0); // held at 50%
        assert_eq!(ramp_time(&keys, 0.0), -2_000.0);
        for s in (0..14_000).step_by(250) {
            let t = ramp_time(&keys, s as f64);
            assert!((ramp_source(&keys, t) - s as f64).abs() < 1e-6, "{s}");
        }
    }

    proptest! {
        #[test]
        fn bezier_is_monotonic_in_time(
            p in (0f64..=1.0, 0f64..=1.0, 0f64..=1.0, 0f64..=1.0),
            a in 0f64..=1.0,
            b in 0f64..=1.0,
        ) {
            let e = Easing::Bezier([p.0, p.1, p.2, p.3]);
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(e.apply(lo) <= e.apply(hi) + 1e-12);
        }
    }
}
