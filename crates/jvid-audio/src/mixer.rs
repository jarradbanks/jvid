//! Track mixer: interleaved f32 stereo blocks at [`SAMPLE_RATE`] in, one master block out.
//! Pure DSP, no allocation per block; parameter changes ramp so they never click.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use jvid_core::model::AudioFx;

use crate::{Fx, SAMPLE_RATE};

/// Gain changes glide linearly over this long.
pub const RAMP_FRAMES: usize = SAMPLE_RATE as usize / 100;

/// One track's mix settings, as on `jvid_core::Track`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strip {
    /// Linear, 1.0 = unity; includes any per-clip gain.
    pub volume: f32,
    /// -1.0 (left) to 1.0 (right).
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    /// EQ and compressor, before volume and pan.
    pub fx: AudioFx,
}

impl Default for Strip {
    fn default() -> Self {
        Self {
            volume: 1.0,
            pan: 0.0,
            muted: false,
            solo: false,
            fx: AudioFx::default(),
        }
    }
}

/// Constant-power (-3 dB centre) left/right gains for `pan`.
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let a = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    (a.cos(), a.sin())
}

/// Balance gains for a clip's `pan`: centre is unity on both sides, and turning one way
/// fades the other side out.
pub fn balance(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    ((1.0 - p).min(1.0), (1.0 + p).min(1.0))
}

/// Scales interleaved stereo `buf` by `gain(frame)` = (volume, pan), evaluated every frame so
/// automation ramps are sample-accurate rather than stepping per block.
pub fn automate(buf: &mut [f32], mut gain: impl FnMut(usize) -> (f32, f32)) {
    for (i, f) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
        let (v, p) = gain(i);
        let (l, r) = balance(p);
        f[0] *= v * l;
        f[1] *= v * r;
    }
}

/// Whether a strip is heard: not muted, and soloed if any strip is.
pub fn audible(strip: &Strip, any_solo: bool) -> bool {
    !strip.muted && (!any_solo || strip.solo)
}

/// Peak absolute sample per track plus master, held until read. Lock-free, so the UI
/// can read it while the audio thread writes.
#[derive(Debug, Default)]
pub struct Meters {
    tracks: Vec<AtomicU32>,
    master: AtomicU32,
}

impl Meters {
    fn raise(a: &AtomicU32, peak: f32) {
        a.fetch_max(peak.to_bits(), Ordering::Relaxed); // non-negative f32 bits order like u32
    }

    /// Peaks since the last call, per track then master; resets them.
    pub fn take(&self) -> (Vec<f32>, f32) {
        let get = |a: &AtomicU32| f32::from_bits(a.swap(0, Ordering::Relaxed));
        (self.tracks.iter().map(get).collect(), get(&self.master))
    }
}

/// Current and target (left, right) gain for one bus, and the glide between them.
#[derive(Debug, Clone, Copy)]
struct Ramp {
    now: (f32, f32),
    to: (f32, f32),
    step: (f32, f32),
    left: usize,
}

impl Ramp {
    fn new(g: (f32, f32)) -> Self {
        Self {
            now: g,
            to: g,
            step: (0.0, 0.0),
            left: 0,
        }
    }

    /// Glides from the current gain to `to` over [`RAMP_FRAMES`].
    fn set(&mut self, to: (f32, f32)) {
        if to == self.to {
            return;
        }
        let n = RAMP_FRAMES as f32;
        self.to = to;
        self.step = ((to.0 - self.now.0) / n, (to.1 - self.now.1) / n);
        self.left = RAMP_FRAMES;
    }
}

pub struct Mixer {
    strips: Vec<Strip>,
    ramps: Vec<Ramp>,
    fx: Vec<Fx>,
    /// A track's input after its effects.
    scratch: Vec<f32>,
    master: f32,
    master_ramp: Ramp,
    meters: Arc<Meters>,
}

impl Mixer {
    pub fn new(tracks: usize) -> Self {
        let mut m = Self {
            strips: vec![Strip::default(); tracks],
            ramps: Vec::new(),
            fx: (0..tracks).map(|_| Fx::default()).collect(),
            scratch: Vec::new(),
            master: 1.0,
            master_ramp: Ramp::new((1.0, 1.0)),
            meters: Arc::new(Meters {
                tracks: (0..tracks).map(|_| AtomicU32::new(0)).collect(),
                master: AtomicU32::new(0),
            }),
        };
        m.ramps = (0..tracks).map(|i| Ramp::new(m.target(i))).collect();
        m
    }

    /// A mixer already at `strips`, rather than ramping to them from unity.
    pub fn with_strips(strips: &[Strip]) -> Self {
        let mut m = Self::new(strips.len());
        m.strips.copy_from_slice(strips);
        m.ramps = (0..strips.len()).map(|i| Ramp::new(m.target(i))).collect();
        m
    }

    pub fn meters(&self) -> Arc<Meters> {
        self.meters.clone()
    }

    /// Sets every strip; changes ramp in from the next block.
    pub fn set_strips(&mut self, strips: &[Strip]) {
        self.strips.copy_from_slice(strips);
        for i in 0..self.strips.len() {
            let t = self.target(i);
            self.ramps[i].set(t);
        }
    }

    pub fn set_master(&mut self, gain: f32) {
        self.master = gain;
        self.master_ramp.set((gain, gain));
    }

    pub fn tracks(&self) -> usize {
        self.strips.len()
    }

    /// Pulls each track through its latency-compensated effects before applying the strip and
    /// master gains. Track lookahead composes with the reader's clip lookahead, so cascaded
    /// effects retain their timeline position without delaying any other track.
    pub fn render(
        &mut self,
        start: i64,
        out: &mut [f32],
        mut read: impl FnMut(usize, i64, &mut [f32]),
    ) {
        out.fill(0.0);
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.resize(out.len(), 0.0);
        for i in 0..self.strips.len() {
            self.fx[i].render(&self.strips[i].fx, start, &mut scratch, |at, buf| {
                read(i, at, buf)
            });
            let peak = apply(&mut self.ramps[i], &scratch, out);
            Meters::raise(&self.meters.tracks[i], peak);
        }
        self.scratch = scratch;
        let peak = apply_in_place(&mut self.master_ramp, out);
        Meters::raise(&self.meters.master, peak);
    }

    fn target(&self, i: usize) -> (f32, f32) {
        let s = &self.strips[i];
        if !audible(s, self.strips.iter().any(|s| s.solo)) {
            return (0.0, 0.0);
        }
        let (l, r) = pan_gains(s.pan);
        (l * s.volume, r * s.volume)
    }

    /// Mixes `inputs[i]` (track i, same length as `out`) into `out`, overwriting it.
    pub fn mix(&mut self, inputs: &[&[f32]], out: &mut [f32]) {
        assert_eq!(inputs.len(), self.ramps.len());
        out.fill(0.0);
        for (i, input) in inputs.iter().enumerate() {
            assert_eq!(input.len(), out.len());
            let input = match self.strips[i].fx.is_default() {
                true => input,
                false => {
                    self.scratch.clear();
                    self.scratch.extend_from_slice(input);
                    self.fx[i].process(&self.strips[i].fx, &mut self.scratch);
                    &self.scratch[..]
                }
            };
            let (ramp, meter) = (&mut self.ramps[i], &self.meters.tracks[i]);
            let peak = apply(ramp, input, out);
            Meters::raise(meter, peak);
        }
        let peak = apply_in_place(&mut self.master_ramp, out);
        Meters::raise(&self.meters.master, peak);
    }
}

/// Calls `f(frame, l, r)` for each frame, advancing the ramp.
fn gains(ramp: &mut Ramp, frames: usize, mut f: impl FnMut(usize, f32, f32)) {
    let ramped = frames.min(ramp.left);
    let (mut l, mut r) = ramp.now;
    for i in 0..ramped {
        l += ramp.step.0;
        r += ramp.step.1;
        f(i, l, r);
    }
    ramp.left -= ramped;
    if ramp.left == 0 {
        (l, r) = ramp.to;
    }
    ramp.now = (l, r);
    for i in ramped..frames {
        f(i, l, r);
    }
}

fn apply(ramp: &mut Ramp, input: &[f32], out: &mut [f32]) -> f32 {
    let mut peak = 0f32;
    gains(ramp, out.len() / 2, |i, l, r| {
        let (a, b) = (input[2 * i] * l, input[2 * i + 1] * r);
        out[2 * i] += a;
        out[2 * i + 1] += b;
        peak = peak.max(a.abs()).max(b.abs());
    });
    peak
}

fn apply_in_place(ramp: &mut Ramp, buf: &mut [f32]) -> f32 {
    let mut peak = 0f32;
    gains(ramp, buf.len() / 2, |i, l, r| {
        buf[2 * i] *= l;
        buf[2 * i + 1] *= r;
        peak = peak.max(buf[2 * i].abs()).max(buf[2 * i + 1].abs());
    });
    peak
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: usize = 512;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn rendered_tracks_with_different_latency_stay_aligned() {
        let mut cleanup = AudioFx::default();
        cleanup.noise_reduction.amount = 25.0;
        let strips = [
            Strip {
                pan: -1.0,
                fx: cleanup,
                ..Strip::default()
            },
            Strip {
                pan: 1.0,
                ..Strip::default()
            },
        ];
        let mut mixer = Mixer::with_strips(&strips);
        let mut out = vec![0.0; 2000 * 2];
        for start in [0, 2000, 0] {
            mixer.render(start, &mut out, |_, at, buf| {
                for (i, f) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                    f.fill(if at + i as i64 == 1500 { 0.5 } else { 0.0 });
                }
            });
            if start == 0 {
                for channel in 0..2 {
                    let peak = out
                        .iter()
                        .skip(channel)
                        .step_by(2)
                        .enumerate()
                        .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
                        .unwrap()
                        .0;
                    assert_eq!(peak, 1500, "channel {channel}");
                }
            }
        }
    }

    #[test]
    fn constant_power_pan() {
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(close(pan_gains(0.0).0, h) && close(pan_gains(0.0).1, h));
        let (l, r) = pan_gains(-1.0);
        assert!(close(l, 1.0) && close(r, 0.0));
        let (l, r) = pan_gains(1.0);
        assert!(close(l, 0.0) && close(r, 1.0));
        for p in [-0.7, -0.2, 0.3, 0.9, 5.0] {
            let (l, r) = pan_gains(p);
            assert!(close(l * l + r * r, 1.0));
        }
    }

    #[test]
    fn balance_and_per_frame_automation() {
        assert_eq!(balance(0.0), (1.0, 1.0));
        assert_eq!(balance(1.0), (0.0, 1.0));
        assert_eq!(balance(-0.5), (1.0, 0.5));
        let mut buf = vec![1.0f32; 8];
        automate(&mut buf, |i| (i as f32, 0.5));
        assert_eq!(buf, [0.0, 0.0, 0.5, 1.0, 1.0, 2.0, 1.5, 3.0]);
    }

    #[test]
    fn mute_solo_truth_table() {
        let s = |muted, solo| Strip {
            muted,
            solo,
            ..Strip::default()
        };
        // (muted, solo, any_solo) -> audible
        for (m, so, any, want) in [
            (false, false, false, true),
            (true, false, false, false),
            (false, false, true, false),
            (false, true, true, true),
            (true, true, true, false),
        ] {
            assert_eq!(audible(&s(m, so), any), want, "{m} {so} {any}");
        }
    }

    #[test]
    fn mix_settles_and_meters() {
        let mut m = Mixer::new(2);
        let a = vec![0.5f32; BLOCK * 2];
        let b = vec![0.25f32; BLOCK * 2];
        let mut out = vec![0.0; BLOCK * 2];
        m.set_strips(&[
            Strip {
                pan: -1.0,
                ..Strip::default()
            },
            Strip {
                solo: false,
                muted: true,
                ..Strip::default()
            },
        ]);
        m.mix(&[&a, &b], &mut out);
        let (tracks, _) = m.meters().take();
        assert!(tracks[1] > 0.0 && tracks[1] < 0.25); // ramping down from centre
        m.mix(&[&a, &b], &mut out);
        assert!(close(out[0], 0.5) && close(out[1], 0.0), "{:?}", &out[..2]);
        let (tracks, master) = m.meters().take();
        assert!(
            close(tracks[0], 0.5) && close(master, 0.5),
            "{tracks:?} {master}"
        );
        assert_eq!(tracks[1], 0.0);
        m.mix(&[&a, &b], &mut out);
        let (tracks, _) = m.meters().take();
        assert_eq!(tracks[1], 0.0);
    }

    #[test]
    fn ramp_has_no_steps() {
        let mut m = Mixer::new(1);
        let dc = vec![1.0f32; 64 * 2]; // small blocks: the ramp spans several
        let mut out = vec![0.0; dc.len()];
        let mut samples: Vec<f32> = Vec::new();
        for i in 0..40 {
            if i == 5 {
                m.set_strips(&[Strip {
                    muted: true,
                    ..Strip::default()
                }]);
            }
            if i == 20 {
                m.set_strips(&[Strip {
                    volume: 2.0,
                    ..Strip::default()
                }]);
            }
            m.mix(&[&dc], &mut out);
            samples.extend(out.iter().step_by(2));
        }
        let bound = 2.0 / RAMP_FRAMES as f32 + 1e-6;
        let mut prev = samples[0];
        for &s in &samples {
            assert!((s - prev).abs() <= bound, "{prev} -> {s}");
            prev = s;
        }
        assert!(close(
            *samples.last().unwrap(),
            2.0 * std::f32::consts::FRAC_1_SQRT_2
        ));
    }

    #[test]
    #[cfg_attr(debug_assertions, ignore = "timing needs release mode")]
    fn sixteen_tracks_under_5_percent_of_real_time() {
        let tracks = 16;
        let mut m = Mixer::new(tracks);
        let inputs: Vec<Vec<f32>> = (0..tracks)
            .map(|t| {
                (0..BLOCK * 2)
                    .map(|i| ((i * (t + 1)) as f32 * 0.01).sin())
                    .collect()
            })
            .collect();
        let refs: Vec<&[f32]> = inputs.iter().map(|v| v.as_slice()).collect();
        let mut out = vec![0.0; BLOCK * 2];
        let blocks = 10 * SAMPLE_RATE as usize / BLOCK; // 10 s of audio
        let start = std::time::Instant::now();
        for b in 0..blocks {
            if b % 50 == 0 {
                let pan = (b as f32 / blocks as f32) * 2.0 - 1.0;
                m.set_strips(&vec![
                    Strip {
                        pan,
                        ..Strip::default()
                    };
                    tracks
                ]);
            }
            m.mix(&refs, &mut out);
        }
        let used = start.elapsed().as_secs_f64() / 10.0;
        println!("16 tracks: {:.3}% of real time", used * 100.0);
        assert!(used < 0.05, "{used}");
    }
}
