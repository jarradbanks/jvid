//! Detect Silence: finds stretches under a level in the selected audio clips' waveform peaks,
//! shades them on the timeline while its dialog is open, and ripple deletes them or speeds
//! them up as one edit.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use egui::{Align2, Key, Pos2, Ui};
use jvid_core::history::Command;
use jvid_core::model::{Clip, ClipId, Project, Sequence};
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use jvid_jobs::{Handle, Pool, Priority};
use jvid_media::audio::SAMPLE_RATE;
use jvid_media::waveforms::{BUCKET, Peak, Pyramid};
use suite_ui::theme;
use suite_ui::tokens::{radius, space};
use suite_ui::widgets::{Button, DragField, State};

use crate::i18n::{tr, trn};
use crate::subtitles::AudioSources;

/// A timeline span to cut from a clip and its linked clips.
pub(super) type Span = (ClipId, Time, Time);

/// The computational settings, expressed exactly as the detector consumes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Settings {
    threshold: u32,
    min_len: i64,
    pad: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct AnalysisKey {
    path: PathBuf,
    stream: usize,
    /// Pyramids are immutable; retaining their Arc prevents address reuse in a live key.
    generation: usize,
    settings: Settings,
}

type Intervals = Arc<Vec<(Time, Time)>>;

struct Analysis {
    _peaks: Arc<Pyramid>,
    found: Arc<Mutex<Option<Intervals>>>,
    handle: Handle,
}

/// The media detector's run/padding semantics, polling cancellation every 4096 buckets.
/// Keeping the open run across chunks avoids introducing false boundaries on long silences.
fn detect(
    peaks: &[Peak],
    settings: Settings,
    cancelled: &dyn Fn() -> bool,
) -> Option<Vec<(Time, Time)>> {
    let limit = 10f32.powf(f32::from_bits(settings.threshold) / 20.0);
    let mut out = Vec::new();
    let mut run = None;
    let time = |samples: i64| Time(samples * (FLICKS_PER_SECOND / SAMPLE_RATE as i64));
    for (i, p) in peaks.iter().chain([&[1.0, 1.0]]).enumerate() {
        if i % 4096 == 0 && cancelled() {
            return None;
        }
        let quiet = p[0].abs().max(p[1].abs()) < limit;
        match (quiet, run) {
            (true, None) => run = Some(i as i64 * BUCKET),
            (false, Some(start)) => {
                run = None;
                let end = i as i64 * BUCKET;
                if end - start >= settings.min_len && start + settings.pad < end - settings.pad {
                    out.push((time(start + settings.pad), time(end - settings.pad)));
                }
            }
            _ => {}
        }
    }
    (!cancelled()).then_some(out)
}

/// The open dialog: its settings and the clips it reads.
pub(super) struct Silence {
    /// dBFS a stretch has to stay under.
    #[cfg(test)]
    scans: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    threshold: f64,
    /// Seconds a stretch has to last.
    min_len: f64,
    /// Seconds of each stretch kept at both ends.
    pad: f64,
    /// Speed Up's rate in percent, as in the inspector.
    speed: f64,
    /// One audio clip per link group; cuts carry its linked clips along.
    clips: Vec<ClipId>,
    selected: HashSet<ClipId>,
    sources: AudioSources,
    analyses: HashMap<AnalysisKey, Analysis>,
    jobs: Option<Pool>,
}

impl Silence {
    /// Reads the audio clips among `selected` and their linked partners; `None` if there are
    /// none.
    pub(super) fn open(seq: &Sequence, selected: &HashSet<ClipId>) -> Option<Self> {
        if selected.is_empty() {
            return None;
        }
        let sources = AudioSources::default();
        let clips = sources.clips(seq, selected, true);
        (!clips.is_empty()).then(|| Self {
            #[cfg(test)]
            scans: Default::default(),
            threshold: -40.0,
            min_len: 1.0,
            pad: 0.15,
            speed: 400.0,
            clips: clips.iter().map(|c| c.id).collect(),
            selected: selected.clone(),
            sources,
            analyses: HashMap::new(),
            jobs: None,
        })
    }

    /// The clips whose peaks it needs.
    pub(super) fn clips(&self) -> &[ClipId] {
        &self.clips
    }

    fn settings(&self) -> Settings {
        Settings {
            threshold: (self.threshold as f32).to_bits(),
            min_len: (self.min_len * SAMPLE_RATE as f64) as i64,
            pad: (self.pad * SAMPLE_RATE as f64) as i64,
        }
    }

    /// One source scan per immutable peak generation and settings, shared across every cut.
    /// Only ready results for the current keys can reach Cut / Speed Up. Clip timing edits
    /// reuse source intervals and map them through the current trim, direction and speed ramp.
    pub(super) fn spans(
        &mut self,
        project: &Project,
        peaks: &dyn Fn(&Clip) -> Option<Arc<Pyramid>>,
        ctx: &egui::Context,
    ) -> Option<Vec<Span>> {
        let settings = self.settings();
        let seq = project.sequence();
        let mut needed = HashSet::new();
        let mut placed = Vec::new();
        let mut ready = true;
        for &id in &self.clips {
            let Some((_, c)) = seq.clip(id) else { continue };
            let Some(asset) = project.asset(c.asset) else {
                ready = false;
                continue;
            };
            let Some(p) = peaks(c) else {
                ready = false;
                continue;
            };
            let key = AnalysisKey {
                path: asset.path.clone(),
                stream: c.stream.unwrap_or(0) as usize,
                generation: Arc::as_ptr(&p) as usize,
                settings,
            };
            needed.insert(key.clone());
            if !self.analyses.contains_key(&key) {
                let found = Arc::new(Mutex::new(None));
                let (out, input, ctx) = (found.clone(), p.clone(), ctx.clone());
                #[cfg(test)]
                let scans = self.scans.clone();
                static NEXT: AtomicU64 = AtomicU64::new(0);
                let job_key = format!("silence-{}", NEXT.fetch_add(1, Ordering::Relaxed));
                let jobs = self.jobs.get_or_insert_with(|| Pool::new(1));
                let handle = jobs.submit(job_key, Priority::Interactive, move |job| {
                    #[cfg(test)]
                    scans.fetch_add(1, Ordering::Relaxed);
                    if let Some(intervals) =
                        detect(input.finest(), settings, &|| job.is_cancelled())
                    {
                        *out.lock().unwrap() = Some(Arc::new(intervals));
                        ctx.request_repaint();
                    }
                });
                self.analyses.insert(
                    key.clone(),
                    Analysis {
                        _peaks: p,
                        found,
                        handle,
                    },
                );
            }
            let intervals = self.analyses[&key].found.lock().unwrap().clone();
            if let Some(intervals) = intervals {
                placed.push((c, intervals));
            } else {
                ready = false;
            }
        }
        // Cancel obsolete in-flight work, including when the replacement peaks are loading.
        self.analyses.retain(|key, analysis| {
            let keep = needed.contains(key) || analysis.found.lock().unwrap().is_some();
            if !keep {
                analysis.handle.cancel();
            }
            keep
        });
        if !ready {
            return None;
        }
        let mut out = Vec::new();
        for (c, intervals) in placed {
            let first = intervals.partition_point(|(_, b)| *b <= c.source_in);
            for &(a, b) in intervals[first..]
                .iter()
                .take_while(|(a, _)| *a < c.source_out)
            {
                let (a, b) = (a.max(c.source_in), b.min(c.source_out));
                if a < b {
                    let (x, y) = (c.timeline_at(a), c.timeline_at(b));
                    out.push((c.id, x.min(y), x.max(y)));
                }
            }
        }
        Some(out)
    }

    /// The dialog, floating at `at` above the lanes so the shaded spans stay in view. Returns
    /// the edit once Cut or Speed Up is picked, and false once it should close.
    pub(super) fn show(
        &mut self,
        ui: &Ui,
        at: Pos2,
        seq: &Sequence,
        spans: Option<&[Span]>,
    ) -> (Option<Command>, bool) {
        let settings = self.settings();
        let mut command = None;
        let mut open = !ui.input(|i| i.key_pressed(Key::Escape));
        egui::Window::new(tr!("silence.title"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .pivot(Align2::RIGHT_BOTTOM)
            .default_pos(at)
            .frame(
                egui::Frame::popup(ui.style())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(space::S16),
            )
            .show(ui.ctx(), |ui| {
                ui.set_width(280.0);
                ui.label(egui::RichText::new(tr!("silence.title")).font(theme::headline()));
                ui.add_space(space::S8);
                let changed = self.sources.show(ui, seq, &self.selected, true);
                if changed {
                    self.clips = self
                        .sources
                        .clips(seq, &self.selected, true)
                        .iter()
                        .map(|c| c.id)
                        .collect();
                }
                let spans = if changed { None } else { spans };
                egui::Grid::new("silence_grid")
                    .spacing([space::S8, space::S8])
                    .show(ui, |ui| {
                        let rows = [
                            (
                                tr!("silence.threshold"),
                                &mut self.threshold,
                                tr!("silence.db_suffix"),
                                -80.0..=-10.0,
                                0,
                                0.25,
                            ),
                            (
                                tr!("silence.min_len"),
                                &mut self.min_len,
                                tr!("silence.seconds_suffix"),
                                0.1..=30.0,
                                2,
                                0.02,
                            ),
                            (
                                tr!("silence.padding"),
                                &mut self.pad,
                                tr!("silence.seconds_suffix"),
                                0.0..=2.0,
                                2,
                                0.01,
                            ),
                            (
                                tr!("silence.speed"),
                                &mut self.speed,
                                "%",
                                110.0..=10000.0,
                                0,
                                5.0,
                            ),
                        ];
                        for (label, v, suffix, range, decimals, speed) in rows {
                            ui.label(label);
                            ui.add(
                                DragField::new(v)
                                    .suffix(suffix)
                                    .range(range)
                                    .decimals(decimals)
                                    .speed(speed)
                                    .width(96.0),
                            );
                            ui.end_row();
                        }
                    });
                ui.add_space(space::S8);
                let spans = if settings == self.settings() {
                    spans
                } else {
                    None
                };
                let found = spans.map(|s| {
                    let total: i64 = s.iter().map(|(_, a, b)| (*b - *a).0).sum();
                    let seconds = format!("{:.1}", total as f64 / FLICKS_PER_SECOND as f64);
                    match s.len() {
                        0 => tr!("silence.none").to_owned(),
                        n => trn!("silence.found", n, seconds = seconds),
                    }
                });
                ui.label(found.unwrap_or_else(|| tr!("silence.reading").to_owned()));
                ui.add_space(space::S16);
                let any = spans.is_some_and(|s| !s.is_empty());
                let state = if any { State::Live } else { State::Disabled };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = space::S8;
                    let mut pick = |ui: &mut Ui, b: Button, tip: &str, speed: Option<f64>| {
                        if ui.add(b.state(state)).on_hover_text(tip).clicked() {
                            let spans = spans.unwrap_or_default().to_vec();
                            command = Some(Command::CutSpans { spans, speed });
                            open = false;
                        }
                    };
                    pick(
                        ui,
                        Button::primary(tr!("silence.cut")),
                        tr!("silence.cut_tooltip"),
                        None,
                    );
                    let speed = Some(self.speed / 100.0);
                    pick(
                        ui,
                        Button::secondary(tr!("silence.speed_up")),
                        tr!("silence.speed_up_tooltip"),
                        speed,
                    );
                    if ui.add(Button::ghost(tr!("prompt.cancel"))).clicked() {
                        open = false;
                    }
                });
            });
        (command, open)
    }
}

impl Drop for Silence {
    fn drop(&mut self) {
        for analysis in self.analyses.values() {
            analysis.handle.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use jvid_core::history::History;
    use jvid_core::model::TrackKind;
    use jvid_core::time::FLICKS_PER_SECOND as S;
    use jvid_media::waveforms::{BUCKET, Pyramid};

    use super::super::Timeline;
    use super::super::tests::project;
    use super::*;

    fn wait_spans(
        s: &mut Silence,
        p: &Project,
        peaks: &dyn Fn(&Clip) -> Option<Arc<Pyramid>>,
    ) -> Vec<Span> {
        let start = std::time::Instant::now();
        loop {
            if let Some(spans) = s.spans(p, peaks, &egui::Context::default()) {
                return spans;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "detector did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn wait_timeline(t: &mut Timeline, p: &Project) -> Vec<Span> {
        let start = std::time::Instant::now();
        loop {
            if let Some(spans) = t.silence_spans(p, &egui::Context::default()) {
                return spans;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "timeline detector did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn audit_465_one_scan_for_100_cuts_and_unchanged_frames() {
        let p = project(200);
        let mut t = Timeline::default();
        t.sync(&p);
        t.selected = p.sequence().tracks()[1]
            .clips()
            .iter()
            .map(|c| c.id)
            .collect();
        let peaks = Arc::new(Pyramid::new(vec![
            [-0.5, 0.5];
            2 * 60 * 60 * SAMPLE_RATE as usize
                / BUCKET as usize
        ]));
        t.media
            .peaks
            .insert((p.assets()[0].path.clone(), 0), Some(peaks));
        assert!(t.action("detect_silence", &p));
        assert!(wait_timeline(&mut t, &p).is_empty());
        let count = |t: &Timeline| t.silence.as_ref().unwrap().scans.load(Ordering::Relaxed);
        assert_eq!(
            count(&t),
            1,
            "100 cuts of a two-hour source share one detector scan"
        );
        for _ in 0..10 {
            assert!(
                t.silence_spans(&p, &egui::Context::default())
                    .unwrap()
                    .is_empty()
            );
        }
        assert_eq!(count(&t), 1, "unchanged frames do not rescan source peaks");
        t.silence.as_mut().unwrap().threshold = -30.0;
        wait_timeline(&mut t, &p);
        assert_eq!(count(&t), 2);
        t.silence.as_mut().unwrap().threshold = -40.0;
        wait_timeline(&mut t, &p);
        assert_eq!(
            count(&t),
            2,
            "returning to cached settings reuses their intervals"
        );
        t.silence.as_mut().unwrap().min_len = 2.0;
        wait_timeline(&mut t, &p);
        assert_eq!(count(&t), 3);
        t.silence.as_mut().unwrap().pad = 0.2;
        wait_timeline(&mut t, &p);
        assert_eq!(count(&t), 4);
        t.silence.as_mut().unwrap().speed = 800.0;
        wait_timeline(&mut t, &p);
        assert_eq!(
            count(&t),
            4,
            "the edit speed does not change source analysis"
        );
        t.media.peaks.insert(
            (p.assets()[0].path.clone(), 0),
            Some(Arc::new(Pyramid::new(vec![
                [0.0; 2];
                2 * 60
                    * 60
                    * SAMPLE_RATE as usize
                    / BUCKET as usize
            ]))),
        );
        assert_eq!(wait_timeline(&mut t, &p).len(), 100);
        assert_eq!(
            count(&t),
            5,
            "a new peak generation invalidates source analysis"
        );
    }

    #[test]
    fn audit_465_cancellable_detector_matches_media_reference_at_boundaries() {
        let mut peaks = vec![[0.0; 2]; 12_000];
        for (i, peak) in peaks.iter_mut().enumerate() {
            if (i / 2300) % 2 == 1 {
                *peak = [-0.01, 0.01];
            }
        }
        let time = |n| Time(n * (S / SAMPLE_RATE as i64));
        for threshold in [-60.0_f32, -40.0, -20.0] {
            for min_len in [0, 48_000, 200_000] {
                for pad in [0, 7200, 200_000] {
                    let settings = Settings {
                        threshold: threshold.to_bits(),
                        min_len,
                        pad,
                    };
                    let expected: Vec<_> =
                        jvid_media::silence::detect(&peaks, threshold, min_len, pad)
                            .into_iter()
                            .map(|(a, b)| (time(a), time(b)))
                            .collect();
                    assert_eq!(detect(&peaks, settings, &|| false).unwrap(), expected);
                }
            }
        }
        let polls = std::cell::Cell::new(0);
        let settings = Settings {
            threshold: (-40.0_f32).to_bits(),
            min_len: 0,
            pad: 0,
        };
        assert!(
            detect(&peaks, settings, &|| {
                polls.set(polls.get() + 1);
                polls.get() >= 2
            })
            .is_none()
        );
        assert_eq!(
            polls.get(),
            2,
            "cancellation is checked within a bounded chunk"
        );
        assert_eq!(detect(&[], settings, &|| false), Some(Vec::new()));
    }

    #[test]
    fn audit_465_settings_changes_cancel_pending_work_without_publishing_stale_results() {
        let p = project(2);
        let audio = p.sequence().tracks()[1].clips()[0].id;
        let mut s = Silence::open(p.sequence(), &HashSet::from([audio])).unwrap();
        let jobs = Pool::new(1);
        let (started, begun) = std::sync::mpsc::channel();
        let (release, gate) = std::sync::mpsc::channel();
        jobs.submit("silence_test_gate", Priority::Interactive, move |_| {
            started.send(()).unwrap();
            gate.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        });
        begun
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        s.jobs = Some(jobs);
        let peaks = Arc::new(Pyramid::new(vec![
            [0.002, 0.002];
            2 * SAMPLE_RATE as usize / BUCKET as usize
        ]));
        let ctx = egui::Context::default();
        assert!(s.spans(&p, &|_| Some(peaks.clone()), &ctx).is_none());
        let old = s.analyses.values().next().unwrap().handle.clone();
        s.threshold = -20.0;
        assert!(s.spans(&p, &|_| Some(peaks.clone()), &ctx).is_none());
        assert!(old.is_cancelled());
        release.send(()).unwrap();
        assert!(!wait_spans(&mut s, &p, &|_| Some(peaks.clone())).is_empty());
        assert_eq!(
            s.scans.load(Ordering::Relaxed),
            1,
            "the obsolete queued detector never ran"
        );
        assert_eq!(s.analyses.len(), 1);
        assert_eq!(s.analyses.keys().next().unwrap().settings, s.settings());
    }

    #[test]
    fn audit_465_trim_reverse_and_speed_ramps_match_reference_and_undo() {
        use jvid_core::keyframe::{Easing, Keyframe, Value};
        let (p, mic, _) = crate::subtitles::test_support::two_streams(false);
        let peaks = Arc::new(Pyramid::new(
            (0..6 * SAMPLE_RATE as i64 / BUCKET)
                .map(|i| {
                    let sample = i * BUCKET;
                    if sample < 3 * SAMPLE_RATE as i64 / 2 || sample >= 3 * SAMPLE_RATE as i64 {
                        [0.0; 2]
                    } else {
                        [-0.5, 0.5]
                    }
                })
                .collect(),
        ));
        let mut s = Silence::open(p.sequence(), &HashSet::from([mic])).unwrap();
        (s.min_len, s.pad) = (0.25, 0.15);
        for reverse in [false, true] {
            for ramp in [false, true] {
                let mut h = History::new(p.clone());
                h.apply(Command::SetClipSpeed {
                    id: mic,
                    speed: 1.5,
                    reverse,
                    ripple: true,
                })
                .unwrap();
                if ramp {
                    h.apply(Command::SetSpeedKeys {
                        id: mic,
                        speed: 1.5,
                        keys: vec![
                            Keyframe {
                                time: Time(S),
                                value: Value::Scalar(0.75),
                                easing: Easing::Linear,
                            },
                            Keyframe {
                                time: Time(5 * S),
                                value: Value::Scalar(2.5),
                                easing: Easing::Linear,
                            },
                        ],
                        ripple: true,
                    })
                    .unwrap();
                }
                let before = h.project().clone();
                let c = before.sequence().clip(mic).unwrap().1;
                assert_eq!(c.reverse, reverse);
                let spans = wait_spans(&mut s, &before, &|_| Some(peaks.clone()));
                let expected: Vec<_> = jvid_media::silence::detect(
                    peaks.finest(),
                    -40.0,
                    SAMPLE_RATE as i64 / 4,
                    (0.15 * SAMPLE_RATE as f64) as i64,
                )
                .into_iter()
                .filter_map(|(a, b)| {
                    let a = Time(a * (S / SAMPLE_RATE as i64)).max(c.source_in);
                    let b = Time(b * (S / SAMPLE_RATE as i64)).min(c.source_out);
                    if a >= b {
                        return None;
                    }
                    let (x, y) = (c.timeline_at(a), c.timeline_at(b));
                    Some((mic, x.min(y), x.max(y)))
                })
                .collect();
                assert_eq!(spans, expected, "reverse={reverse}, ramp={ramp}");
                assert_eq!(spans.len(), 2);
                for speed in [None, Some(4.0)] {
                    h.apply(Command::CutSpans {
                        spans: spans.clone(),
                        speed,
                    })
                    .unwrap();
                    assert!(h.project().sequence().duration() < before.sequence().duration());
                    assert!(h.undo());
                    assert_eq!(h.project(), &before);
                }
                assert_eq!(s.scans.load(Ordering::Relaxed), 1);
            }
        }
    }

    #[test]
    fn audit_465_source_and_stream_identity_invalidate_analysis() {
        let (p, mic, game) = crate::subtitles::test_support::two_streams(false);
        let peaks = Arc::new(Pyramid::new(vec![
            [0.0; 2];
            6 * SAMPLE_RATE as usize / BUCKET as usize
        ]));
        let mut s = Silence::open(p.sequence(), &HashSet::from([mic])).unwrap();
        wait_spans(&mut s, &p, &|_| Some(peaks.clone()));
        s.clips = vec![game];
        wait_spans(&mut s, &p, &|_| Some(peaks.clone()));
        assert_eq!(
            s.scans.load(Ordering::Relaxed),
            2,
            "stream identity matters even with identical peaks"
        );
        let mut h = History::new(p);
        h.apply(Command::Import(vec![(
            PathBuf::from("/fake/relinked.mkv"),
            jvid_core::model::ProbeInfo::default(),
        )]))
        .unwrap();
        h.apply(Command::SetClipAsset {
            id: game,
            asset: h.project().assets()[1].id,
        })
        .unwrap();
        wait_spans(&mut s, h.project(), &|_| Some(peaks.clone()));
        assert_eq!(
            s.scans.load(Ordering::Relaxed),
            3,
            "relinking invalidates source identity"
        );
    }

    #[test]
    fn audit_465_rewritten_source_discards_old_peaks_and_late_waveform_results() {
        let dir = std::env::temp_dir().join(format!("jvid-silence-source-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("source.wav");
        std::fs::write(&path, b"original").unwrap();
        let p = project(2);
        let mut asset = p.assets()[0].clone();
        asset.path = path.clone();
        let mut media = super::super::Media::default();
        let key = (path.clone(), 0);
        media.refresh_peaks(&path, 0);
        let old_stamp = super::super::source_stamp(&path);
        media
            .peaks
            .insert(key.clone(), Some(Arc::new(Pyramid::new(vec![[0.0; 2]; 8]))));
        std::fs::write(&path, b"new generation with different length").unwrap();
        media.refresh_peaks(&path, 0);
        assert!(!media.peaks.contains_key(&key));
        media.peaks_done.lock().unwrap().push((
            (key.clone(), old_stamp),
            Some(Pyramid::new(vec![[0.0; 2]; 8])),
        ));
        assert!(
            media
                .pyramid(&egui::Context::default(), &asset, 0)
                .is_none()
        );
        assert!(
            !media.peaks.contains_key(&key),
            "a superseded waveform completion cannot republish old peaks"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn audit_442_silence_uses_selected_mic_and_cuts_linked_video() {
        for mic_first in [false, true] {
            let (p, mic, game) = crate::subtitles::test_support::two_streams(mic_first);
            let quiet = vec![[0.0; 2]; 6 * SAMPLE_RATE as usize / BUCKET as usize];
            let mut speech = quiet.clone();
            // Speech is only on Mic, apart from one known silent second.
            for (i, peak) in speech.iter_mut().enumerate() {
                let sec = i as i64 * BUCKET / SAMPLE_RATE as i64;
                if sec != 3 {
                    *peak = [-0.5, 0.5];
                }
            }
            let mut s = Silence::open(p.sequence(), &HashSet::from([mic])).unwrap();
            (s.min_len, s.pad) = (0.5, 0.0);
            assert_eq!(s.clips(), &[mic]);
            let speech = Arc::new(Pyramid::new(speech));
            let quiet = Arc::new(Pyramid::new(quiet));
            let spans = wait_spans(&mut s, &p, &|c| {
                Some(if c.id == mic {
                    speech.clone()
                } else {
                    quiet.clone()
                })
            });
            assert_eq!(spans.len(), 1);
            assert_eq!(spans[0].0, mic);
            let mut h = History::new(p.clone());
            h.apply(Command::CutSpans { spans, speed: None }).unwrap();
            for t in h.project().sequence().tracks() {
                assert_eq!(t.clips().len(), 2);
            }
            assert!(h.undo());
            assert_eq!(h.project(), &p);
            let h = super::super::highlights::Highlights::open(p.sequence(), &HashSet::from([mic]))
                .unwrap();
            assert_eq!(h.audio(), &[mic]);
            assert_ne!(mic, game);
        }
    }

    /// Acceptance: Cut leaves two adjacent clips with the silence gone, undone in one step,
    /// and Speed Up shortens the timeline by what the speed saves.
    #[test]
    fn cut_or_speed_up_the_detected_silence() {
        let p = project(2);
        let track = &p.sequence().tracks()[1];
        assert_eq!(track.kind, TrackKind::Audio);
        let audio = track.clips()[0].id;
        // Two seconds: loud, then silent from 0.5 s to 1.5 s, then loud.
        let rate = SAMPLE_RATE as i64;
        let peaks = (0..2 * rate / BUCKET)
            .map(|i| {
                let quiet = i * BUCKET >= rate / 2 && (i + 1) * BUCKET <= 3 * rate / 2;
                if quiet { [0.0; 2] } else { [-0.5, 0.5] }
            })
            .collect();
        let mut t = Timeline::default();
        let path = (p.assets()[0].path.clone(), 0);
        t.media
            .peaks
            .insert(path, Some(Arc::new(Pyramid::new(peaks))));
        t.sync(&p);
        assert!(!t.can("detect_silence", p.sequence()));
        t.selected.insert(audio);
        assert!(t.can("detect_silence", p.sequence()));
        assert!(t.action("detect_silence", &p));
        let s = t.silence.as_mut().unwrap();
        // Whole quiet buckets only: the 1 s silence reads a few ms short.
        (s.pad, s.min_len) = (0.0, 0.5);

        let spans = wait_timeline(&mut t, &p);
        let [(id, a, b)] = spans[..] else {
            panic!("{spans:?}")
        };
        assert_eq!(id, audio);
        let frame = S / 30;
        assert!((a.0 - S / 2).abs() <= frame && (b.0 - 3 * S / 2).abs() <= frame);

        let mut h = History::new(p.clone());
        let cut = Command::CutSpans {
            spans: spans.clone(),
            speed: None,
        };
        h.apply(cut).unwrap();
        let clips = h.project().sequence().tracks()[1].clips();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].end(), clips[1].start);
        assert_eq!(clips[1].end(), Time(2 * S) - (b - a));
        assert!(h.undo());
        assert_eq!(h.project(), &p);

        let speed = Some(4.0);
        h.apply(Command::CutSpans { spans, speed }).unwrap();
        let clips = h.project().sequence().tracks()[1].clips();
        let saved = (b - a).0 - (b - a).0 / 4;
        assert!((clips.last().unwrap().end().0 - (2 * S - saved)).abs() <= 1);
        assert!(h.undo());
        assert_eq!(h.project(), &p);
    }
}
