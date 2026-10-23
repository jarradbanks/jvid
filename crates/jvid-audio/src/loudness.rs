//! EBU R128 loudness: an integrated-loudness meter (ITU-R BS.1770-4 K-weighting and gating) and
//! a look-ahead true-peak limiter, for normalizing exports. Both take 48 kHz interleaved stereo.

use std::collections::VecDeque;

/// Frames per 100 ms gating step at 48 kHz.
const STEP: usize = 4_800;

/// Direct form II transposed, in f64 so low cutoffs stay accurate.
#[derive(Clone, Copy, Default)]
pub(crate) struct Biquad {
    pub(crate) b: [f64; 3],
    pub(crate) a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    pub(crate) fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Self { b, a, z: [0.0; 2] }
    }

    pub(crate) fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Measures integrated loudness (LUFS) of everything pushed to it.
pub struct Meter {
    /// Per channel: the K-weighting shelf, then its high-pass.
    k: [[Biquad; 2]; 2],
    sum: f64,
    frames: usize,
    /// Mean square (summed over channels) of each complete 100 ms step.
    steps: Vec<f64>,
}

impl Default for Meter {
    fn default() -> Self {
        // BS.1770-4's 48 kHz coefficients.
        let shelf = Biquad::new(
            [1.53512485958697, -2.69169618940638, 1.19839281085285],
            [-1.69065929318241, 0.73248077421585],
        );
        let hp = Biquad::new([1.0, -2.0, 1.0], [-1.99004745483398, 0.99007225036621]);
        Self {
            k: [[shelf, hp]; 2],
            sum: 0.0,
            frames: 0,
            steps: Vec::new(),
        }
    }
}

fn lufs(mean_square: f64) -> f64 {
    -0.691 + 10.0 * mean_square.log10()
}

impl Meter {
    pub fn push(&mut self, samples: &[f32]) {
        for f in samples.as_chunks::<2>().0 {
            for (c, k) in self.k.iter_mut().enumerate() {
                let s = k[0].run(f[c] as f64);
                let y = k[1].run(s);
                self.sum += y * y;
            }
            self.frames += 1;
            if self.frames == STEP {
                self.steps.push(self.sum / STEP as f64);
                (self.sum, self.frames) = (0.0, 0);
            }
        }
    }

    /// Gated integrated loudness, or `None` for silence or under 400 ms.
    pub fn integrated(&self) -> Option<f64> {
        // 400 ms blocks overlapping by 75%.
        let blocks: Vec<f64> = self
            .steps
            .windows(4)
            .map(|w| w.iter().sum::<f64>() / 4.0)
            .collect();
        let gated = |min: f64| {
            let kept: Vec<f64> = blocks.iter().copied().filter(|&b| lufs(b) > min).collect();
            (!kept.is_empty()).then(|| kept.iter().sum::<f64>() / kept.len() as f64)
        };
        let relative = lufs(gated(-70.0)?) - 10.0;
        gated(relative.max(-70.0)).map(lufs)
    }
}

/// Look-ahead frames (1 ms).
const LOOK: usize = 48;
/// Frames the interpolator needs after the sample it's checking.
const AHEAD: usize = 4;
/// Per-frame release toward unity gain (about a 50 ms time constant).
const RELEASE: f32 = 1.0 / 2_400.0;

/// Applies a gain, then holds true peaks (4x oversampled) to a ceiling. Output trails input by
/// [`Limiter::delay`] frames internally but comes out aligned: the first frames are swallowed and
/// [`Limiter::flush`] returns the tail, so output length equals input length.
pub struct Limiter {
    gain: f32,
    ceiling: f32,
    /// Interpolation taps for the 3 points between two samples, over the 8 samples around them.
    taps: [[f32; 8]; 3],
    x: VecDeque<[f32; 2]>,
    /// Gain each frame needs on its own.
    need: VecDeque<f32>,
    /// `need`'s look-ahead minimum, with release applied.
    held: VecDeque<f32>,
    skip: usize,
}

impl Limiter {
    pub fn new(gain: f32, ceiling_db: f32) -> Self {
        let taps = std::array::from_fn(|p| {
            let t = (p + 1) as f32 / 4.0;
            std::array::from_fn(|j| {
                let d = t - (j as f32 - 3.0);
                let sinc = if d == 0.0 {
                    1.0
                } else {
                    (std::f32::consts::PI * d).sin() / (std::f32::consts::PI * d)
                };
                let hann = 0.5 + 0.5 * (std::f32::consts::PI * d / 4.5).cos();
                sinc * hann
            })
        });
        Self {
            gain,
            ceiling: 10f32.powf(ceiling_db / 20.0),
            taps,
            x: std::iter::repeat_n([0.0; 2], Self::delay()).collect(),
            need: std::iter::repeat_n(1.0, LOOK).collect(),
            held: std::iter::repeat_n(1.0, LOOK).collect(),
            skip: Self::delay(),
        }
    }

    pub const fn delay() -> usize {
        LOOK + AHEAD - 1
    }

    /// The highest of a channel's sample at `i` and the 3 interpolated points after it.
    fn peak(&self, i: usize, c: usize) -> f32 {
        let mut p = self.x[i][c].abs();
        for taps in &self.taps {
            let v: f32 = (0..8).map(|j| self.x[i + j - 3][c] * taps[j]).sum();
            p = p.max(v.abs());
        }
        p
    }

    /// Appends the limited counterpart of `input` (interleaved stereo) to `out`, `delay` late.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for f in input.as_chunks::<2>().0 {
            self.x.push_back([f[0] * self.gain, f[1] * self.gain]);
            let i = self.x.len() - 1 - AHEAD;
            let peak = self.peak(i, 0).max(self.peak(i, 1));
            self.need.pop_front();
            self.need.push_back((self.ceiling / peak).min(1.0));
            let min = self.need.iter().copied().fold(1.0, f32::min);
            let prev = *self.held.back().unwrap();
            self.held.pop_front();
            self.held.push_back(min.min(prev + (1.0 - prev) * RELEASE));
            // Every held value covering this frame is at most its own need, so their mean is too.
            let g = self.held.iter().sum::<f32>() / LOOK as f32;
            let y = self.x.pop_front().unwrap();
            if self.skip > 0 {
                self.skip -= 1;
            } else {
                out.extend([y[0] * g, y[1] * g]);
            }
        }
    }

    /// Appends the last `delay` frames.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        self.process(&vec![0.0; Self::delay() * 2], out);
    }

    /// True peak of interleaved stereo, as this limiter measures it.
    pub fn true_peak(samples: &[f32]) -> f32 {
        let mut l = Self::new(1.0, 0.0);
        let mut peak = 0f32;
        let tail = [[0.0; 2]; AHEAD];
        for f in samples.as_chunks::<2>().0.iter().chain(&tail) {
            l.x.push_back(*f);
            let i = l.x.len() - 1 - AHEAD;
            peak = peak.max(l.peak(i, 0)).max(l.peak(i, 1));
            l.x.pop_front();
        }
        peak
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(db: f64, secs: f64) -> Vec<f32> {
        let a = 10f64.powf(db / 20.0);
        (0..(secs * 48_000.0) as usize)
            .flat_map(|n| {
                let s = (a * (std::f64::consts::TAU * 1_000.0 * n as f64 / 48_000.0).sin()) as f32;
                [s, s]
            })
            .collect()
    }

    fn measure(s: &[f32]) -> f64 {
        let mut m = Meter::default();
        m.push(s);
        m.integrated().unwrap()
    }

    /// EBU Tech 3341 cases 1 to 3.
    #[test]
    fn matches_tech_3341() {
        assert!((measure(&sine(-23.0, 20.0)) + 23.0).abs() < 0.1);
        assert!((measure(&sine(-33.0, 20.0)) + 33.0).abs() < 0.1);
        let mut gated = sine(-36.0, 10.0);
        gated.extend(sine(-23.0, 60.0));
        gated.extend(sine(-36.0, 10.0));
        assert!((measure(&gated) + 23.0).abs() < 0.1, "{}", measure(&gated));
        assert_eq!(Meter::default().integrated(), None);
        let mut m = Meter::default();
        m.push(&vec![0.0; 96_000 * 2]);
        assert_eq!(m.integrated(), None);
    }

    #[test]
    fn finds_inter_sample_peaks() {
        // fs/4 at 45°: every sample is at 0.707 but the wave peaks at 1.
        let s: Vec<f32> = (0..4_800)
            .flat_map(|n| {
                let v =
                    (std::f32::consts::FRAC_PI_2 * n as f32 + std::f32::consts::FRAC_PI_4).sin();
                [v, v]
            })
            .collect();
        let p = Limiter::true_peak(&s);
        assert!(p > 0.97 && p < 1.03, "{p}");
    }

    fn limit(gain: f32, input: &[f32]) -> Vec<f32> {
        let mut l = Limiter::new(gain, -1.0);
        let mut out = Vec::new();
        for chunk in input.chunks(3_200) {
            l.process(chunk, &mut out);
        }
        l.flush(&mut out);
        out
    }

    #[test]
    fn leaves_quiet_audio_aligned_and_untouched() {
        let s = sine(-20.0, 1.0);
        let out = limit(1.0, &s);
        assert_eq!(out.len(), s.len());
        assert!(s.iter().zip(&out).all(|(a, b)| (a - b).abs() < 1e-6));
    }

    /// Normalizing peaky material to -14 LUFS lands within 0.5 LU, under -1 dBTP.
    #[test]
    fn normalizes_to_target_under_ceiling() {
        let mut s = sine(-30.0, 10.0);
        // Short loud bursts that the gain would push well over full scale.
        for n in (0..s.len()).step_by(48_000 * 2) {
            for v in &mut s[n..n + 960] {
                *v *= 6.0;
            }
        }
        let gain = 10f64.powf((-14.0 - measure(&s)) / 20.0) as f32;
        let loud: Vec<f32> = s.iter().map(|v| v * gain).collect();
        assert!(
            Limiter::true_peak(&loud) > 1.0,
            "the limiter has work to do"
        );
        let out = limit(gain, &s);
        assert!((measure(&out) + 14.0).abs() < 0.5, "{}", measure(&out));
        let peak = 20.0 * Limiter::true_peak(&out).log10();
        assert!(peak <= -1.0 + 0.05, "{peak}");
    }
}
