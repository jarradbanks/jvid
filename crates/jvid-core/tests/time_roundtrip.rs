use jvid_core::time::{Rate, format_timecode, parse_timecode};
use proptest::prelude::*;

const RATES: [Rate; 10] = [
    Rate::new(24000, 1001),
    Rate::new(24, 1),
    Rate::new(25, 1),
    Rate::new(30000, 1001),
    Rate::new(30, 1),
    Rate::new(50, 1),
    Rate::new(60000, 1001),
    Rate::new(60, 1),
    Rate::new(44100, 1),
    Rate::new(48000, 1),
];

/// Indices covering 24 hours at `r`.
fn day(r: Rate) -> i64 {
    24 * 3600 * r.num as i64 / r.den as i64
}

proptest! {
    #[test]
    fn index_time_round_trip(ri in 0..RATES.len(), frac in 0.0..=1.0f64) {
        let r = RATES[ri];
        let n = (day(r) as f64 * frac) as i64;
        let t = r.time_floor(n);
        prop_assert_eq!(t, r.time_nearest(n), "not an exact flick count");
        prop_assert_eq!(r.index_floor(t), n);
        prop_assert_eq!(r.index_nearest(t), n);
    }

    #[test]
    fn timecode_round_trip(ri in 0..8usize, frac in 0.0..=1.0f64) {
        let r = RATES[ri];
        let n = (day(r) as f64 * frac) as u64;
        prop_assert_eq!(parse_timecode(&format_timecode(n, r), r), Some(n));
    }
}

#[test]
fn round_trip_edges_of_the_day() {
    for r in RATES {
        for n in [0, 1, day(r) - 1, day(r)] {
            assert_eq!(r.index_floor(r.time_floor(n)), n);
            assert_eq!(
                parse_timecode(&format_timecode(n as u64, r), r),
                Some(n as u64)
            );
        }
    }
}
