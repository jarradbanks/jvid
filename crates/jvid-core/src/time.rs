//! Integer time in flicks, frame and sample rates, and non-drop-frame timecode.

use serde::{Deserialize, Serialize};

/// Flicks per second. Divides every common frame and sample rate exactly.
pub const FLICKS_PER_SECOND: i64 = 705_600_000;

/// A point or span on the timeline, in flicks.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Time(pub i64);

impl std::ops::Add for Time {
    type Output = Time;
    fn add(self, rhs: Time) -> Time {
        Time(self.0 + rhs.0)
    }
}

impl std::ops::Sub for Time {
    type Output = Time;
    fn sub(self, rhs: Time) -> Time {
        Time(self.0 - rhs.0)
    }
}

/// A frame or sample rate as an exact fraction, e.g. 24000/1001 for 23.976 fps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rate {
    pub num: u32,
    pub den: u32,
}

impl Rate {
    /// Panics if either part is zero.
    pub const fn new(num: u32, den: u32) -> Self {
        assert!(num > 0 && den > 0, "rate parts must be non-zero");
        Self { num, den }
    }

    /// Start time of frame (or sample) `n`, rounded down to a whole flick.
    pub fn time_floor(self, n: i64) -> Time {
        Time(div_floor(n as i128 * self.flicks_num(), self.num as i128) as i64)
    }

    /// Time of frame (or sample) `n`, rounded to the nearest flick (ties up).
    pub fn time_nearest(self, n: i64) -> Time {
        let d = self.num as i128;
        Time(div_floor(2 * n as i128 * self.flicks_num() + d, 2 * d) as i64)
    }

    /// Index of the frame (or sample) containing `t`.
    pub fn index_floor(self, t: Time) -> i64 {
        div_floor(t.0 as i128 * self.num as i128, self.flicks_num()) as i64
    }

    /// Index of the frame (or sample) boundary nearest to `t` (ties up).
    pub fn index_nearest(self, t: Time) -> i64 {
        let d = self.flicks_num();
        div_floor(2 * t.0 as i128 * self.num as i128 + d, 2 * d) as i64
    }

    /// Whole frames per second used for timecode: 24 for 23.976, 30 for 29.97.
    pub fn timecode_base(self) -> u32 {
        self.num.div_ceil(self.den)
    }

    fn flicks_num(self) -> i128 {
        FLICKS_PER_SECOND as i128 * self.den as i128
    }
}

fn div_floor(a: i128, b: i128) -> i128 {
    a.div_euclid(b)
}

/// Formats frame index `frame` as non-drop-frame `HH:MM:SS:FF`.
pub fn format_timecode(frame: u64, rate: Rate) -> String {
    let fps = rate.timecode_base() as u64;
    let (ff, secs) = (frame % fps, frame / fps);
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60,
        ff
    )
}

/// Parses non-drop-frame `HH:MM:SS:FF` to a frame index. `None` if malformed
/// or a field is out of range.
pub fn parse_timecode(s: &str, rate: Rate) -> Option<u64> {
    let fps = rate.timecode_base() as u64;
    let mut parts = s.split(':').map(|p| {
        (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            .then(|| p.parse::<u64>().ok())
            .flatten()
    });
    let [h, m, sec, f] = [
        parts.next()??,
        parts.next()??,
        parts.next()??,
        parts.next()??,
    ];
    if parts.next().is_some() || m >= 60 || sec >= 60 || f >= fps {
        return None;
    }
    Some(((h * 60 + m) * 60 + sec) * fps + f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flicks_per_frame_are_exact() {
        assert_eq!(Rate::new(24000, 1001).time_floor(1), Time(29_429_400));
        assert_eq!(Rate::new(48000, 1).time_floor(1), Time(14_700));
        assert_eq!(Rate::new(44100, 1).time_floor(1), Time(16_000));
    }

    #[test]
    fn explicit_rounding() {
        let r = Rate::new(24, 1); // 29_400_000 flicks per frame
        assert_eq!(r.index_floor(Time(29_399_999)), 0);
        assert_eq!(r.index_nearest(Time(14_700_000)), 1);
        assert_eq!(r.index_nearest(Time(14_699_999)), 0);
        assert_eq!(r.index_floor(Time(-1)), -1);
        let odd = Rate::new(7, 1); // 100_800_000 flicks per frame
        assert_eq!(Rate::new(3, 1).time_floor(1), Time(235_200_000));
        assert_eq!(odd.time_nearest(1), Time(100_800_000));
        assert_eq!(Rate::new(705_600_001, 1).time_floor(1), Time(0));
        assert_eq!(Rate::new(705_600_001, 1).time_nearest(1), Time(1));
    }

    #[test]
    fn timecode_format_and_parse() {
        let r = Rate::new(30000, 1001);
        assert_eq!(format_timecode(30 * 3661 + 5, r), "01:01:01:05");
        assert_eq!(parse_timecode("01:01:01:05", r), Some(30 * 3661 + 5));
        for bad in [
            "01:60:00:00",
            "00:00:00:30",
            "1:2:3",
            "00:00:00:00:00",
            "a:00:00:00",
            "+1:00:00:00",
            "",
        ] {
            assert_eq!(parse_timecode(bad, r), None, "{bad}");
        }
    }
}
