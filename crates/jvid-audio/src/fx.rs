//! Clip and track audio effects: a three-band EQ, then a compressor. Interleaved f32 stereo at
//! [`SAMPLE_RATE`]; state carries across blocks, so any block size gives the same output.

use std::f64::consts::{PI, SQRT_2};

use jvid_core::model::{AudioFx, Compressor, Equalizer};

use crate::SAMPLE_RATE;
use crate::loudness::Biquad;

/// One clip's or track's effect state.
#[derive(Default)]
pub struct Fx {
    /// The EQ the filters are tuned for.
    eq: Option<Equalizer>,
    /// Low shelf, peak, high shelf; per channel.
    bands: [[Biquad; 2]; 3],
    /// Compressor level detector: peak with instant attack, falling at the release rate.
    level: f32,
    /// Smoothed gain reduction in dB.
    reduction: f32,
}

impl Fx {
    /// Runs `buf` through `p`'s EQ then compressor, skipping either while it's off or neutral.
    pub fn process(&mut self, p: &AudioFx, buf: &mut [f32]) {
        let eq = &p.eq;
        if eq.enabled && (eq.low != 0.0 || eq.mid != 0.0 || eq.high != 0.0) {
            self.tune(eq);
            for f in buf.as_chunks_mut::<2>().0 {
                for (c, x) in f.iter_mut().enumerate() {
                    let mut y = *x as f64;
                    for band in &mut self.bands {
                        y = band[c].run(y);
                    }
                    *x = y as f32;
                }
            }
        }
        let c = &p.compressor;
        if c.enabled && (c.ratio > 1.0 || c.makeup != 0.0) {
            self.compress(c, buf);
        }
    }

    /// Retunes the filters when `eq` changed, keeping their state so a change doesn't click.
    fn tune(&mut self, eq: &Equalizer) {
        if self.eq.as_ref() == Some(eq) {
            return;
        }
        self.eq = Some(*eq);
        let new = [
            shelf(eq.low_freq, eq.low, false),
            peak(eq.mid_freq, eq.mid, eq.mid_q),
            shelf(eq.high_freq, eq.high, true),
        ];
        for (band, n) in self.bands.iter_mut().zip(new) {
            for q in band {
                (q.b, q.a) = (n.b, n.a);
            }
        }
    }

    fn compress(&mut self, c: &Compressor, buf: &mut [f32]) {
        let coeff = |ms: f32| (-1.0 / (ms.max(0.0) * 0.001 * SAMPLE_RATE as f32)).exp();
        let (attack, release) = (coeff(c.attack), coeff(c.release));
        let slope = 1.0 - 1.0 / c.ratio.max(1.0);
        for f in buf.as_chunks_mut::<2>().0 {
            self.level = f[0].abs().max(f[1].abs()).max(self.level * release);
            let over = 20.0 * self.level.max(1e-9).log10() - c.threshold;
            let target = over.max(0.0) * slope;
            // Reduction rises at the attack rate; the level detector already releases.
            self.reduction = match target > self.reduction {
                true => target + (self.reduction - target) * attack,
                false => target,
            };
            let g = 10f32.powf((c.makeup - self.reduction) / 20.0);
            f[0] *= g;
            f[1] *= g;
        }
    }
}

/// `freq` in radians per sample, held below Nyquist, and its cosine and sine.
fn omega(freq: f32) -> (f64, f64) {
    let w = 2.0 * PI * (freq as f64).clamp(10.0, 0.45 * SAMPLE_RATE as f64) / SAMPLE_RATE as f64;
    (w.cos(), w.sin())
}

fn normalized(b: [f64; 3], a: [f64; 3]) -> Biquad {
    Biquad::new(b.map(|b| b / a[0]), [a[1] / a[0], a[2] / a[0]])
}

/// RBJ cookbook peaking EQ.
fn peak(freq: f32, db: f32, q: f32) -> Biquad {
    let a = 10f64.powf(db as f64 / 40.0);
    let (cos, sin) = omega(freq);
    let alpha = sin / (2.0 * (q as f64).max(0.1));
    normalized(
        [1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a],
        [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a],
    )
}

/// RBJ cookbook low or high shelf with slope 1.
fn shelf(freq: f32, db: f32, high: bool) -> Biquad {
    let a = 10f64.powf(db as f64 / 40.0);
    let (cos, sin) = omega(freq);
    // A high shelf is the low shelf with cos negated and b1, a1 flipped.
    let (cos, s) = if high { (-cos, -1.0) } else { (cos, 1.0) };
    let k = 2.0 * a.sqrt() * sin / 2.0 * SQRT_2;
    normalized(
        [
            a * ((a + 1.0) - (a - 1.0) * cos + k),
            s * 2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - k),
        ],
        [
            (a + 1.0) + (a - 1.0) * cos + k,
            s * -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - k,
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One second of a `freq` Hz sine at `amp` on both channels.
    fn sine(freq: f32, amp: f32) -> Vec<f32> {
        (0..SAMPLE_RATE as usize)
            .flat_map(|i| {
                let s =
                    amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE as f32).sin();
                [s, s]
            })
            .collect()
    }

    /// Peak level in dB of the last quarter second.
    fn tail_db(buf: &[f32]) -> f32 {
        let peak = buf[buf.len() * 3 / 4..]
            .iter()
            .fold(0f32, |m, x| m.max(x.abs()));
        20.0 * peak.log10()
    }

    fn eq(f: impl FnOnce(&mut Equalizer)) -> AudioFx {
        let mut p = AudioFx::default();
        f(&mut p.eq);
        p
    }

    /// Gain in dB `p` gives a `freq` Hz sine.
    fn gain(p: &AudioFx, freq: f32) -> f32 {
        let mut buf = sine(freq, 0.25);
        Fx::default().process(p, &mut buf);
        tail_db(&buf) - 20.0 * 0.25f32.log10()
    }

    #[test]
    fn peak_raises_its_band_and_leaves_the_rest() {
        let p = eq(|e| e.mid = 6.0);
        let (at, low) = (gain(&p, 1_000.0), gain(&p, 100.0));
        assert!((at - 6.0).abs() <= 0.5, "1 kHz {at} dB");
        assert!(low.abs() <= 0.5, "100 Hz {low} dB");
    }

    #[test]
    fn shelves_lift_their_side_only() {
        let p = eq(|e| (e.low, e.high) = (6.0, -6.0));
        let (low, mid, high) = (gain(&p, 30.0), gain(&p, 1_000.0), gain(&p, 18_000.0));
        assert!((low - 6.0).abs() <= 0.5, "{low}");
        assert!(mid.abs() <= 0.5, "{mid}");
        assert!((high + 6.0).abs() <= 0.5, "{high}");
        let off = eq(|e| (e.enabled, e.mid) = (false, 6.0));
        assert_eq!(gain(&off, 1_000.0), gain(&AudioFx::default(), 1_000.0));
    }

    #[test]
    fn compressor_holds_12_db_over_to_3_at_4_to_1() {
        let mut p = AudioFx::default();
        p.compressor.ratio = 4.0;
        let amp = 10f32.powf((p.compressor.threshold + 12.0) / 20.0);
        let mut buf = sine(1_000.0, amp);
        // In blocks, as the mixer runs it.
        let mut fx = Fx::default();
        buf.chunks_mut(512).for_each(|b| fx.process(&p, b));
        let over = tail_db(&buf) - p.compressor.threshold;
        assert!((over - 3.0).abs() <= 0.25, "{over} dB over");
        // Makeup adds on top; bypass leaves the signal alone.
        p.compressor.makeup = 3.0;
        assert!((gain(&p, 1_000.0) - gain(&AudioFx::default(), 1_000.0)).abs() > 1.0);
        p.compressor.enabled = false;
        assert_eq!(gain(&p, 1_000.0), gain(&AudioFx::default(), 1_000.0));
    }
}
