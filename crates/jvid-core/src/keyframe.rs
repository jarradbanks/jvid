//! Keyframed values and interpolation.
//!
//! Keyframe times are in the clip's source time, so they travel with the clip
//! when it moves and stay on the same frame of media when it is trimmed.

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
            Self::EaseIn => bezier([0.42, 0.0, 1.0, 1.0], t),
            Self::EaseOut => bezier([0.0, 0.0, 0.58, 1.0], t),
            Self::EaseInOut => bezier([0.42, 0.0, 0.58, 1.0], t),
            Self::Bezier(p) => bezier(p, t),
        }
    }
}

fn cubic(a: f64, b: f64, s: f64) -> f64 {
    let r = 1.0 - s;
    3.0 * r * r * s * a + 3.0 * r * s * s * b + s * s * s
}

/// Solves x(s) = t by bisection (monotonic in `t` by construction), returns y(s).
fn bezier([x1, y1, x2, y2]: [f64; 4], t: f64) -> f64 {
    let (x1, x2) = (x1.clamp(0.0, 1.0), x2.clamp(0.0, 1.0));
    let t = t.clamp(0.0, 1.0);
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..48 {
        let mid = (lo + hi) / 2.0;
        if cubic(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    cubic(y1, y2, (lo + hi) / 2.0)
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
