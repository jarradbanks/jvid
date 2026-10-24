//! Clip and track audio effects: a 10-band graphic EQ, then a compressor. Interleaved f32 stereo
//! at [`SAMPLE_RATE`]; state carries across blocks, so any block size gives the same output.

use jvid_core::model::{AudioFx, Compressor, Equalizer};

use crate::SAMPLE_RATE;
use crate::loudness::Biquad;

/// One clip's or track's effect state.
#[derive(Default)]
pub struct Fx {
    /// The EQ the filters are tuned for.
    eq: Option<Equalizer>,
    /// Per band, per channel.
    bands: [[Biquad; 2]; 10],
    /// Compressor level detector: peak with instant attack, falling at the release rate.
    level: f32,
    /// Smoothed gain reduction in dB.
    reduction: f32,
}

impl Fx {
    /// Runs `buf` through `p`'s EQ then compressor, skipping either while it's off or neutral.
    pub fn process(&mut self, p: &AudioFx, buf: &mut [f32]) {
        let eq = &p.eq;
        if eq.enabled && !eq.is_flat() {
            self.tune(eq);
            let post = 10f64.powf(eq.post_gain as f64 / 20.0);
            for f in buf.as_chunks_mut::<2>().0 {
                for (c, x) in f.iter_mut().enumerate() {
                    let mut y = *x as f64;
                    for band in &mut self.bands {
                        y = band[c].run(y);
                    }
                    *x = (y * post) as f32;
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
        for (i, band) in self.bands.iter_mut().enumerate() {
            let (b, a) = eq.filter(i, SAMPLE_RATE as f64);
            for q in band {
                (q.b, q.a) = (b, a);
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

    /// Acceptance (#263): only the 1K band at +6 dB lifts a 1 kHz tone by 6 dB.
    #[test]
    fn one_band_raises_its_tone_and_leaves_the_rest() {
        let p = eq(|e| e.bands[5] = 6.0);
        let (at, low) = (gain(&p, 1_000.0), gain(&p, 100.0));
        assert!((at - 6.0).abs() <= 0.5, "1 kHz {at} dB");
        assert!(low.abs() <= 0.5, "100 Hz {low} dB");
        // What plays is the curve the Inspector draws, shelves and Post-Gain included.
        let p = eq(|e| e.bands = [6.0, -3.0, 0.0, 2.0, 0.0, -6.0, 4.0, 0.0, 1.0, -6.0]);
        let p = AudioFx {
            eq: Equalizer {
                post_gain: -2.0,
                ..p.eq
            },
            ..p
        };
        for hz in [25.0, 62.5, 250.0, 1_000.0, 2_000.0, 8_000.0, 18_000.0] {
            let (got, want) = (gain(&p, hz), p.eq.response(hz as f64, SAMPLE_RATE as f64));
            assert!(
                (got as f64 - want).abs() <= 0.25,
                "{hz} Hz: {got} vs {want}"
            );
        }
        let off = eq(|e| (e.enabled, e.bands[5]) = (false, 6.0));
        assert_eq!(gain(&off, 1_000.0), gain(&AudioFx::default(), 1_000.0));
    }

    /// Acceptance (#263): a project saved with #85's three bands plays back as it did, within
    /// 1 dB from 31 Hz to 16 kHz.
    #[test]
    fn three_band_projects_sound_the_same() {
        let three = jvid_core::model::ThreeBand {
            low: 6.0,
            mid: -4.0,
            high: 3.0,
            ..Default::default()
        };
        // What `jvid-core` loads it as.
        let p = AudioFx {
            eq: three.into(),
            ..AudioFx::default()
        };
        for hz in [
            31.0, 63.0, 100.0, 250.0, 700.0, 1_000.0, 2_500.0, 6_000.0, 10_000.0, 16_000.0,
        ] {
            let (got, want) = (gain(&p, hz), three.response(hz as f64, SAMPLE_RATE as f64));
            assert!((got as f64 - want).abs() <= 1.0, "{hz} Hz: {got} vs {want}");
        }
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
