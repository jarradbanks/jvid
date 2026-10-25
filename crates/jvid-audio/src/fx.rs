//! Clip and track audio effects: a high-pass, noise reduction, a 10-band graphic EQ, then a
//! compressor. Interleaved f32 stereo at [`SAMPLE_RATE`]; state carries across blocks, so any
//! block size gives the same output.

use jvid_core::model::{AudioFx, Compressor, Equalizer, HighPass};
use nnnoiseless::DenoiseState;

use crate::SAMPLE_RATE;
use crate::loudness::Biquad;

/// One clip's or track's effect state.
#[derive(Default)]
pub struct Fx {
    /// The EQ the filters are tuned for.
    eq: Option<Equalizer>,
    /// Per band, per channel.
    bands: [[Biquad; 2]; 10],
    /// The high-pass the filters are tuned for, and one per channel.
    hp: Option<HighPass>,
    hp_filters: [Biquad; 2],
    /// Made the first time noise reduction runs, then kept.
    denoise: Option<Box<Denoise>>,
    /// Compressor level detector: peak with instant attack, falling at the release rate.
    level: f32,
    /// Smoothed gain reduction in dB.
    reduction: f32,
    /// Next compensated output position and parameters; a discontinuity resets all DSP state.
    render_at: Option<(i64, AudioFx)>,
    scratch: Vec<f32>,
}

impl Fx {
    /// Bounded history used to settle effects after a seek, in project sample frames.
    pub const PREROLL: usize = SAMPLE_RATE as usize / 10;

    /// Algorithmic input-to-output latency in project sample frames. Filters have their usual
    /// frequency-dependent phase response; RNNoise has a fixed two-frame pipeline delay.
    pub fn latency(p: &AudioFx) -> usize {
        if p.noise_reduction.enabled && p.noise_reduction.amount > 0.0 {
            Denoise::DELAY
        } else {
            0
        }
    }

    /// Renders timeline-aligned output, pulling bounded lookahead from `read`. The reader must
    /// supply silence outside its clip/track. Sequential calls carry state; seeks and parameter
    /// changes reset and prime it with 100 ms of history. Padding after the source end flushes
    /// the pipeline, and only the requested output range is returned.
    pub fn render(
        &mut self,
        p: &AudioFx,
        start: i64,
        out: &mut [f32],
        mut read: impl FnMut(i64, &mut [f32]),
    ) {
        if out.is_empty() {
            return;
        }
        let contiguous = self
            .render_at
            .as_ref()
            .is_some_and(|(at, old)| *at == start && old == p);
        if !contiguous {
            *self = Self::default();
        }
        let frames = out.len() / 2;
        let latency = Self::latency(p);
        if p.is_default() {
            read(start, out);
        } else {
            let preroll = if contiguous { 0 } else { Self::PREROLL };
            let discard = if contiguous { 0 } else { preroll + latency };
            let from = if contiguous {
                start + latency as i64
            } else {
                start - preroll as i64
            };
            let mut scratch = std::mem::take(&mut self.scratch);
            let mut done = 0;
            while done < discard + frames {
                let n = (discard + frames - done).min(2048);
                scratch.clear();
                scratch.resize(n * 2, 0.0);
                read(from + done as i64, &mut scratch);
                self.process(p, &mut scratch);
                let skip = discard.saturating_sub(done).min(n);
                if skip < n {
                    let at = (done + skip - discard) * 2;
                    out[at..at + (n - skip) * 2].copy_from_slice(&scratch[skip * 2..]);
                }
                done += n;
            }
            self.scratch = scratch;
        }
        self.render_at = Some((start + frames as i64, *p));
    }

    /// Runs `buf` through `p`'s high-pass, noise reduction, EQ then compressor, skipping each
    /// while it's off or neutral.
    pub fn process(&mut self, p: &AudioFx, buf: &mut [f32]) {
        let hp = &p.high_pass;
        if hp.enabled && hp.freq > 0.0 {
            if self.hp.as_ref() != Some(hp) {
                self.hp = Some(*hp);
                let (b, a) = hp.filter(SAMPLE_RATE as f64);
                for q in &mut self.hp_filters {
                    (q.b, q.a) = (b, a);
                }
            }
            for f in buf.as_chunks_mut::<2>().0 {
                for (c, x) in f.iter_mut().enumerate() {
                    *x = self.hp_filters[c].run(*x as f64) as f32;
                }
            }
        }
        let nr = &p.noise_reduction;
        if nr.enabled && nr.amount > 0.0 {
            let d = self.denoise.get_or_insert_with(Denoise::new);
            d.process((nr.amount / 100.0).clamp(0.0, 1.0), buf);
        }
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

/// RNNoise's frame: 10 ms at 48 kHz, the mixer's rate, so no resampling.
const FRAME: usize = DenoiseState::FRAME_SIZE;
/// RNNoise wants 16-bit sample values.
const PCM: f32 = 32_768.0;

/// Per channel denoiser state. Samples are buffered into whole frames, so the output lags the
/// input by [`Denoise::DELAY`] frames, the dry sound with it so the mix lines up.
struct Denoise {
    states: [Box<DenoiseState<'static>>; 2],
    /// The frame filling up, in 16-bit units.
    input: [[f32; FRAME]; 2],
    /// The last frame out, in 16-bit units.
    output: [[f32; FRAME]; 2],
    /// The last two frames in, the older one lined up with `output`.
    dry: [[[f32; FRAME]; 2]; 2],
    /// Position in the frame.
    at: usize,
}

impl Denoise {
    /// Input to output, in samples: one frame buffering here, one inside RNNoise.
    const DELAY: usize = 2 * FRAME;

    fn new() -> Box<Self> {
        Box::new(Self {
            states: [DenoiseState::new(), DenoiseState::new()],
            input: [[0.0; FRAME]; 2],
            output: [[0.0; FRAME]; 2],
            dry: [[[0.0; FRAME]; 2]; 2],
            at: 0,
        })
    }

    /// Mixes `wet` (0 to 1) of the denoised sound with the rest dry.
    fn process(&mut self, wet: f32, buf: &mut [f32]) {
        for f in buf.as_chunks_mut::<2>().0 {
            for (c, x) in f.iter_mut().enumerate() {
                self.input[c][self.at] = *x * PCM;
                let (y, dry) = (self.output[c][self.at], self.dry[c][1][self.at]);
                *x = (dry + (y - dry) * wet) / PCM;
            }
            self.at += 1;
            if self.at == FRAME {
                self.at = 0;
                for c in 0..2 {
                    self.dry[c] = [self.input[c], self.dry[c][0]];
                    self.states[c].process_frame(&mut self.output[c], &self.input[c]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_reports_active_pipeline_delay() {
        let mut p = AudioFx::default();
        assert_eq!(Fx::latency(&p), 0);
        p.noise_reduction.amount = 25.0;
        assert_eq!(Fx::latency(&p), 960);
        p.noise_reduction.enabled = false;
        assert_eq!(Fx::latency(&p), 0);
    }

    #[test]
    fn aligned_render_flushes_short_input_and_resets_on_seeks() {
        let mut p = AudioFx::default();
        p.noise_reduction.amount = 25.0;
        let input = |start: i64, buf: &mut [f32]| {
            assert!(buf.len() <= 4096, "reader work stays bounded");
            for (i, f) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let at = start + i as i64;
                f.fill(if at == 13 {
                    0.5
                } else if at == 256 {
                    0.25
                } else {
                    0.0
                });
            }
        };
        let mut fx = Fx::default();
        let mut out = vec![0.0; 257 * 2];
        fx.render(&p, 0, &mut out, input);
        let peak = |b: &[f32]| {
            b.as_chunks::<2>()
                .0
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a[0].abs().total_cmp(&b[0].abs()))
                .unwrap()
                .0
        };
        assert_eq!(peak(&out), 13);
        assert!(
            out[512] > 0.15,
            "the last frame of a clip shorter than the latency survives"
        );
        for start in [200, 0, 200] {
            let mut got = vec![0.0; (257 - start) as usize * 2];
            fx.render(&p, start, &mut got, input);
            assert_eq!(peak(&got), if start == 0 { 13 } else { 56 });
        }
        // Changing the cleanup controls resets its pipeline coherently.
        p.noise_reduction.enabled = false;
        fx.render(&p, 0, &mut out, input);
        assert_eq!(out[26], 0.5);
        assert_eq!(out[512], 0.25);
    }

    #[test]
    fn aligned_noise_reduction_preserves_floor_reduction_and_wet_mix() {
        let input = noisy_voice();
        let read = |start: i64, buf: &mut [f32]| {
            for (i, f) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let at = start + i as i64;
                *f = if at >= 0 && (at as usize) < input.len() / 2 {
                    [input[at as usize * 2], input[at as usize * 2 + 1]]
                } else {
                    [0.0; 2]
                };
            }
        };
        let mut p = AudioFx::default();
        p.noise_reduction.amount = 100.0;
        let mut wet = vec![0.0; input.len()];
        Fx::default().render(&p, 0, &mut wet, read);
        assert!(floor_db(&input) - floor_db(&wet) >= 10.0);
        p.noise_reduction.amount = 25.0;
        let mut mixed = vec![0.0; input.len()];
        Fx::default().render(&p, 0, &mut mixed, read);
        assert!(
            mixed
                .iter()
                .zip(input.iter().zip(&wet))
                .all(|(got, (dry, wet))| (got - (dry * 0.75 + wet * 0.25)).abs() < 1e-6)
        );
    }

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

    #[test]
    fn high_pass_cuts_below_its_cutoff() {
        let mut p = AudioFx::default();
        p.high_pass.freq = 100.0;
        assert!(gain(&p, 30.0) < -18.0, "30 Hz {}", gain(&p, 30.0));
        assert!(
            (gain(&p, 100.0) + 3.0).abs() <= 0.5,
            "100 Hz {}",
            gain(&p, 100.0)
        );
        assert!(
            gain(&p, 1_000.0).abs() <= 0.1,
            "1 kHz {}",
            gain(&p, 1_000.0)
        );
        p.high_pass.enabled = false;
        assert_eq!(gain(&p, 30.0), gain(&AudioFx::default(), 30.0));
    }

    /// RMS in dB of each 50 ms of a stereo buffer.
    fn windows_db(buf: &[f32]) -> Vec<f32> {
        const W: usize = SAMPLE_RATE as usize / 10;
        buf.as_chunks::<W>()
            .0
            .iter()
            .map(|w| {
                let ms = w.iter().map(|x| x * x).sum::<f32>() / W as f32;
                10.0 * ms.max(1e-12).log10()
            })
            .collect()
    }

    /// The quietest tenth of the windows: the level between words.
    fn floor_db(buf: &[f32]) -> f32 {
        let mut w = windows_db(buf);
        w.sort_by(f32::total_cmp);
        w[w.len() / 10]
    }

    /// The bundled voice sample with steady hiss at -45 dBFS RMS mixed in.
    fn noisy_voice() -> Vec<f32> {
        jvid_media::ffmpeg::init().unwrap();
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../jvid-app/assets/samples/voice.m4a"
        );
        let mut dec = jvid_media::audio::AudioDecoder::open(path.as_ref(), 0).unwrap();
        let mut buf = Vec::new();
        while let Some(c) = dec.next_chunk().unwrap() {
            buf.extend(c.samples);
        }
        // A fixed LCG, so the fixture is the same every run; uniform noise of RMS a / √3.
        let (mut seed, a) = (1u32, 10f32.powf(-45.0 / 20.0) * 3f32.sqrt());
        for x in &mut buf {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *x += a * (seed as f32 / u32::MAX as f32 * 2.0 - 1.0);
        }
        buf
    }

    /// Acceptance (#346): noise reduction lowers a noisy recording's floor by 10 dB or more,
    /// and keeps the voice.
    #[test]
    fn noise_reduction_drops_the_floor_10_db() {
        let noisy = noisy_voice();
        let mut p = AudioFx::default();
        p.noise_reduction.amount = 100.0;
        let mut out = noisy.clone();
        let mut fx = Fx::default();
        out.chunks_mut(512).for_each(|b| fx.process(&p, b));
        // Line the output up with the input before comparing windows.
        let out = &out[Denoise::DELAY * 2..];
        let noisy = &noisy[..out.len()];
        let (before, after) = (floor_db(noisy), floor_db(out));
        assert!(before - after >= 10.0, "floor {before:.1} -> {after:.1} dB");
        let loud = |b: &[f32]| windows_db(b).into_iter().fold(f32::MIN, f32::max);
        let (speech, kept) = (loud(noisy), loud(out));
        assert!(speech - kept <= 3.0, "speech {speech:.1} -> {kept:.1} dB");
    }

    /// At 0% the denoiser only delays: the dry sound comes out [`Denoise::DELAY`] late.
    #[test]
    fn dry_mix_lines_up_with_the_delay() {
        let mut d = Denoise::new();
        let mut buf = sine(440.0, 0.5);
        let dry = buf.clone();
        buf.chunks_mut(300).for_each(|b| d.process(0.0, b));
        let n = Denoise::DELAY * 2;
        for (y, x) in buf[n..].iter().zip(&dry) {
            assert!((y - x).abs() < 1e-6);
        }
        assert!(buf[..n].iter().all(|y| *y == 0.0));
    }
}
