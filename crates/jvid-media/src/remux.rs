//! Lossless export: copies source ranges packet for packet into one file in the source's
//! container. Plain copy snaps each cut to its nearest keyframes; smart render (H.264 only)
//! re-encodes just the partial GOPs at each cut edge so cuts land on the exact frame.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ffmpeg_next::codec::{self, Id};
use ffmpeg_next::format::{self, Pixel, context::Input, context::Output};
use ffmpeg_next::rescale::Rescale;
use ffmpeg_next::{Packet, Rational, decoder, encoder, frame, media};
use jvid_core::time::{FLICKS_PER_SECOND, Rate, Time};

/// A source range `[start, end)` in the file's own time.
#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    pub path: PathBuf,
    pub start: Time,
    pub end: Time,
}

/// Writes `cuts` back to back into `out`. `progress` gets 0..=1 and returns `false` to stop,
/// which fails the export with "cancelled". Call `ffmpeg::init` first.
pub fn export(
    cuts: &[Cut],
    out: &Path,
    smart: bool,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<(), String> {
    let first = cuts.first().ok_or("nothing to export")?;
    let input = format::input(&first.path).map_err(|e| e.to_string())?;
    let mut output = format::output(out).map_err(|e| e.to_string())?;
    // Input stream index to output stream index, for video and audio only.
    let mut map = BTreeMap::new();
    for s in input.streams() {
        if matches!(
            s.parameters().medium(),
            media::Type::Video | media::Type::Audio
        ) {
            let mut o = output
                .add_stream(encoder::find(Id::None))
                .map_err(|e| e.to_string())?;
            o.set_parameters(s.parameters());
            o.set_time_base(s.time_base());
            map.insert(s.index(), o.index());
        }
    }
    drop(input);
    output.write_header().map_err(|e| e.to_string())?;
    let mut w = Writer {
        out: output,
        map,
        offset: Time(0),
        last_dts: BTreeMap::new(),
    };
    let total: i64 = cuts.iter().map(|c| (c.end - c.start).0).sum();
    let mut done = 0;
    for cut in cuts {
        let len = (cut.end - cut.start).0;
        let base = done;
        let mut step = |f: f32| progress((base as f32 + f * len as f32) / total as f32);
        let wrote = copy_cut(&mut w, cut, smart, &mut step)?;
        w.offset = w.offset + wrote;
        done += len;
    }
    w.out.write_trailer().map_err(|e| e.to_string())
}

struct Writer {
    out: Output,
    map: BTreeMap<usize, usize>,
    /// Where the current cut starts in the output.
    offset: Time,
    /// Per output stream, so cuts never step back in decode order.
    last_dts: BTreeMap<usize, i64>,
}

impl Writer {
    /// Writes `p` from input stream `index` (time base `tb`), shifted so input time `from` lands
    /// on the cut's output offset.
    fn write(
        &mut self,
        mut p: Packet,
        index: usize,
        tb: Rational,
        from: Time,
    ) -> Result<(), String> {
        let Some(&o) = self.map.get(&index) else {
            return Ok(());
        };
        let shift = ticks(self.offset, tb) - ticks(from, tb);
        let pts = p.pts().map(|t| t + shift);
        let mut dts = p.dts().or(pts).map(|t| t + shift);
        if let (Some(d), Some(&last)) = (dts, self.last_dts.get(&o))
            && d <= last
        {
            dts = Some(last + 1);
        }
        p.set_pts(pts.max(dts));
        p.set_dts(dts);
        if let Some(d) = dts {
            self.last_dts.insert(o, d);
        }
        p.set_stream(o);
        p.set_position(-1);
        p.rescale_ts(tb, self.out.stream(o).expect("added").time_base());
        p.write_interleaved(&mut self.out)
            .map_err(|e| e.to_string())
    }
}

/// `t` in time base `tb`, to the nearest tick.
fn ticks(t: Time, tb: Rational) -> i64 {
    let (num, den) = (tb.0 as i128, tb.1 as i128);
    let f = FLICKS_PER_SECOND as i128 * num;
    ((t.0 as i128 * den + f / 2).div_euclid(f)) as i64
}

fn time(ticks: i64, tb: Rational) -> Time {
    let (num, den) = (tb.0 as i128, tb.1 as i128);
    Time((ticks as i128 * num * FLICKS_PER_SECOND as i128 / den).min(i64::MAX as i128) as i64)
}

/// The video keyframes at or before `t` and after it (`None` past the last), as ticks.
fn keyframes(input: &mut Input, v: usize, t: i64) -> Result<(i64, Option<i64>), String> {
    let tb = input.stream(v).expect("video").time_base();
    let mut back = ticks(Time(FLICKS_PER_SECOND), tb);
    let mut at = t;
    loop {
        let micros = at.rescale(tb, Rational(1, 1_000_000));
        input
            .seek(micros.max(0), ..micros.max(0))
            .map_err(|e| e.to_string())?;
        let (mut before, mut after) = (None, None);
        for (s, p) in input.packets() {
            if s.index() != v || !p.is_key() {
                continue;
            }
            let pts = p.pts().unwrap_or(0);
            if pts <= t {
                before = Some(pts);
            } else if before.is_some() {
                after = Some(pts);
                break;
            } else {
                break;
            }
        }
        match before {
            Some(b) => return Ok((b, after)),
            None if micros <= 0 => return Err("no keyframe at the start of the video".into()),
            None => {
                at = t - back;
                back *= 2;
            }
        }
    }
}

/// The keyframe nearer `t`; a tie goes earlier.
fn nearest(t: i64, (before, after): (i64, Option<i64>)) -> i64 {
    match after {
        Some(a) if a - t < t - before => a,
        _ => before,
    }
}

/// What to do with the video in `[from, to)` ticks.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    Copy,
    Encode,
}

/// Video parts covering the cut `[s, e)` given the keyframes around each end. Copy snaps to the
/// nearest keyframes; smart encodes the partial GOPs at each end and copies the middle.
fn plan(
    s: i64,
    e: i64,
    at_s: (i64, Option<i64>),
    at_e: (i64, Option<i64>),
    smart: bool,
) -> Vec<(i64, i64, Part)> {
    if !smart {
        let a = nearest(s, at_s);
        let mut b = nearest(e, at_e);
        if b <= a {
            // Shorter than the GOP around it: keep one whole GOP rather than nothing.
            b = at_s.1.filter(|&k| k > a).unwrap_or(i64::MAX);
        }
        return vec![(a, b, Part::Copy)];
    }
    let ka = if at_s.0 == s {
        s
    } else {
        at_s.1.unwrap_or(i64::MAX)
    };
    let kb = at_e.0;
    if ka >= kb {
        return vec![(s, e, Part::Encode)];
    }
    let mut parts = Vec::new();
    if s < ka {
        parts.push((s, ka, Part::Encode));
    }
    parts.push((ka, kb, Part::Copy));
    if kb < e {
        parts.push((kb, e, Part::Encode));
    }
    parts
}

/// Copies one cut; returns how long it lasts in the output.
fn copy_cut(
    w: &mut Writer,
    cut: &Cut,
    smart: bool,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Time, String> {
    let mut input = format::input(&cut.path).map_err(|e| e.to_string())?;
    let streams: Vec<_> = input
        .streams()
        .filter(|s| {
            matches!(
                s.parameters().medium(),
                media::Type::Video | media::Type::Audio
            )
        })
        .map(|s| s.index())
        .collect();
    if streams.len() != w.map.len() {
        return Err(format!("{} has different streams", cut.path.display()));
    }
    let video = input.streams().best(media::Type::Video).ok_or("no video")?;
    let (v, vtb, params) = (video.index(), video.time_base(), video.parameters());
    let rate = video.avg_frame_rate();
    let (s, e) = (ticks(cut.start, vtb), ticks(cut.end, vtb));
    let at_s = keyframes(&mut input, v, s)?;
    let at_e = keyframes(&mut input, v, e)?;
    let parts = plan(s, e, at_s, at_e, smart);
    let encodes = parts.iter().any(|p| p.2 == Part::Encode);
    if encodes && params.id() != Id::H264 {
        return Err("smart render supports H.264 video only".into());
    }
    let (from, to) = (parts[0].0, parts.last().expect("a part").1);
    let (start, end) = (time(from, vtb), time(to, vtb));
    let micros = at_s.0.rescale(vtb, Rational(1, 1_000_000));
    input
        .seek(micros.max(0), ..micros.max(0))
        .map_err(|e| e.to_string())?;
    let mut enc: Option<Smart> = None;
    // Decode delay of the source's video (pts - dts), which encoded packets copy.
    let mut delay = None;
    let mut video_done = false;
    let mut audio_done = BTreeMap::new();
    let tb_of: BTreeMap<_, _> = input
        .streams()
        .map(|s| (s.index(), s.time_base()))
        .collect();
    for (st, p) in input.packets() {
        let i = st.index();
        let Some(&tb) = tb_of.get(&i) else { continue };
        let pts = p.pts().unwrap_or(0);
        if i != v {
            if w.map.contains_key(&i) && audio_done.get(&i) != Some(&true) {
                let t = time(pts, tb);
                if t >= end {
                    audio_done.insert(i, true);
                } else if t >= start {
                    w.write(p, i, tb, start)?;
                }
            }
        } else if !video_done {
            if p.is_key() && pts >= to {
                video_done = true;
            } else {
                let delay = *delay.get_or_insert(pts - p.dts().unwrap_or(pts));
                // Packets outside the parts only feed an end's decoder: from the keyframe before
                // the cut, and references past it that frames inside need.
                let part = parts
                    .iter()
                    .find(|(a, b, _)| *a <= pts && pts < *b)
                    .or_else(|| {
                        if pts < from {
                            parts.first()
                        } else {
                            parts.last()
                        }
                    })
                    .filter(|p| p.2 == Part::Encode || (p.0 <= pts && pts < p.1));
                match part {
                    Some(&(a, b, Part::Encode)) => {
                        if enc.as_ref().is_none_or(|x| x.range != (a, b)) {
                            if let Some(x) = enc.take() {
                                x.finish(w, v, vtb, start, delay)?;
                            }
                            enc = Some(Smart::open(&params, rate, (a, b), annexb(&p))?);
                        }
                        let x = enc.as_mut().expect("opened");
                        x.send(&p, w, v, vtb, start, delay)?;
                    }
                    Some(_) => {
                        if let Some(x) = enc.take() {
                            x.finish(w, v, vtb, start, delay)?;
                        }
                        w.write(p, v, vtb, start)?;
                    }
                    None => {}
                }
                if !progress(((pts - from) as f32 / (to - from).max(1) as f32).clamp(0.0, 1.0)) {
                    return Err("cancelled".into());
                }
            }
        }
        let audio_left = streams
            .iter()
            .any(|s| *s != v && audio_done.get(s) != Some(&true));
        if video_done && !audio_left {
            break;
        }
    }
    if let Some(x) = enc.take() {
        x.finish(w, v, vtb, start, delay.unwrap_or(0))?;
    }
    // `to` can be past the end of the file when the cut runs to it.
    let last = input_duration(&cut.path).unwrap_or(end);
    Ok(end.min(last.max(start)) - start)
}

fn input_duration(path: &Path) -> Option<Time> {
    let d = format::input(path).ok()?.duration();
    (d > 0).then(|| time(d, Rational(1, 1_000_000)))
}

/// Only x264 can number its parameter sets apart from the source's; it needs a GPL build.
pub const SMART_ENCODER: Option<&str> = if cfg!(feature = "gpl-ffmpeg") {
    Some("libx264")
} else {
    None
};

/// Decodes the source over `range` ticks and re-encodes it as H.264 with parameter sets in-band.
struct Smart {
    range: (i64, i64),
    dec: decoder::Video,
    enc: encoder::Video,
    enc_tb: Rational,
    /// The source's packets are Annex B like the encoder's, rather than length-prefixed.
    annexb: bool,
}

impl Smart {
    fn open(
        params: &codec::Parameters,
        rate: Rational,
        range: (i64, i64),
        annexb: bool,
    ) -> Result<Self, String> {
        let dec = codec::Context::from_parameters(params.clone())
            .and_then(|c| c.decoder().video())
            .map_err(|e| e.to_string())?;
        if dec.format() != Pixel::YUV420P {
            return Err("smart render needs 8-bit 4:2:0 video".into());
        }
        let rate = if rate.0 > 0 && rate.1 > 0 {
            Rate::new(rate.0 as u32, rate.1 as u32)
        } else {
            Rate::new(30, 1)
        };
        // Fast, near-transparent, no B-frames so encoded packets decode in order, and parameter set
        // ids 1 so the in-band ones don't replace the source's (id 0) for the copied video.
        let opts = BTreeMap::from([
            ("crf".to_string(), "14".to_string()),
            ("bf".to_string(), "0".to_string()),
            ("preset".to_string(), "ultrafast".to_string()),
            ("x264-params".to_string(), "sps-id=1".to_string()),
        ]);
        let size = (dec.width(), dec.height());
        let enc = SMART_ENCODER
            .and_then(|n| crate::encode::open_video(n, Pixel::YUV420P, size, rate, false, &opts))
            .map(|(_, e)| e)
            .ok_or("smart render needs libx264, which this build doesn't use")?;
        Ok(Self {
            range,
            dec,
            enc,
            enc_tb: Rational(rate.den as i32, rate.num as i32),
            annexb,
        })
    }

    fn send(
        &mut self,
        p: &Packet,
        w: &mut Writer,
        v: usize,
        vtb: Rational,
        from: Time,
        delay: i64,
    ) -> Result<(), String> {
        self.dec.send_packet(p).map_err(|e| e.to_string())?;
        self.pump(w, v, vtb, from, delay)
    }

    fn finish(
        mut self,
        w: &mut Writer,
        v: usize,
        vtb: Rational,
        from: Time,
        delay: i64,
    ) -> Result<(), String> {
        self.dec.send_eof().map_err(|e| e.to_string())?;
        self.pump(w, v, vtb, from, delay)?;
        self.enc.send_eof().map_err(|e| e.to_string())?;
        self.drain(w, v, vtb, from, delay)
    }

    /// Moves decoded frames in range to the encoder, and its packets to the writer.
    fn pump(
        &mut self,
        w: &mut Writer,
        v: usize,
        vtb: Rational,
        from: Time,
        delay: i64,
    ) -> Result<(), String> {
        let mut f = frame::Video::empty();
        while self.dec.receive_frame(&mut f).is_ok() {
            let pts = f.timestamp().or(f.pts()).unwrap_or(0);
            if pts < self.range.0 || pts >= self.range.1 {
                continue;
            }
            f.set_pts(Some(pts.rescale(vtb, self.enc_tb)));
            f.set_kind(ffmpeg_next::picture::Type::None);
            self.enc.send_frame(&f).map_err(|e| e.to_string())?;
        }
        self.drain(w, v, vtb, from, delay)
    }

    fn drain(
        &mut self,
        w: &mut Writer,
        v: usize,
        vtb: Rational,
        from: Time,
        delay: i64,
    ) -> Result<(), String> {
        let mut p = Packet::empty();
        while self.enc.receive_packet(&mut p).is_ok() {
            let pts = p.pts().map(|t| t.rescale(self.enc_tb, vtb));
            // shortcut: assumes 4-byte NAL lengths, as x264, cameras and phones write; read
            // avcC's lengthSizeMinusOne if a source with 1 or 2 turns up.
            let data = match self.annexb {
                true => p.data().unwrap_or_default().to_vec(),
                false => length_prefixed(p.data().unwrap_or_default(), 4),
            };
            let mut out = Packet::copy(&data);
            out.set_pts(pts);
            out.set_dts(pts.map(|t| t - delay));
            out.set_flags(p.flags());
            w.write(out, v, vtb, from)?;
        }
        Ok(())
    }
}

/// Whether `p` holds start-code delimited NALs rather than length-prefixed ones.
fn annexb(p: &Packet) -> bool {
    let d = p.data().unwrap_or_default();
    d.starts_with(&[0, 0, 1]) || d.starts_with(&[0, 0, 0, 1])
}

/// Annex B (start codes) to `n`-byte length-prefixed NALs.
fn length_prefixed(annexb: &[u8], n: usize) -> Vec<u8> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= annexb.len() {
        if annexb[i..i + 3] == [0, 0, 1] {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(annexb.len() + 4);
    for (k, &s) in starts.iter().enumerate() {
        let mut e = starts.get(k + 1).map_or(annexb.len(), |&next| next - 3);
        // A 4-byte start code's leading zero, and trailing zeros, belong to no NAL.
        while e > s && annexb[e - 1] == 0 {
            e -= 1;
        }
        out.extend_from_slice(&((e - s) as u32).to_be_bytes()[4 - n..]);
        out.extend_from_slice(&annexb[s..e]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_media::{corpus, frame_index};

    #[test]
    fn copy_snaps_to_nearest_keyframes_and_smart_encodes_the_edges() {
        let (at_s, at_e) = ((30, Some(60)), (90, Some(120)));
        assert_eq!(plan(40, 100, at_s, at_e, false), [(30, 90, Part::Copy)]);
        assert_eq!(plan(50, 110, at_s, at_e, false), [(60, 120, Part::Copy)]);
        assert_eq!(
            plan(40, 100, at_s, at_e, true),
            [
                (40, 60, Part::Encode),
                (60, 90, Part::Copy),
                (90, 100, Part::Encode)
            ]
        );
        // On keyframes nothing is encoded.
        assert_eq!(
            plan(30, 90, (30, Some(60)), (90, Some(120)), true),
            [(30, 90, Part::Copy)]
        );
        // Inside one GOP: all encoded, or for copy the GOP itself.
        assert_eq!(plan(35, 45, at_s, at_s, true), [(35, 45, Part::Encode)]);
        assert_eq!(plan(35, 45, at_s, at_s, false), [(30, 60, Part::Copy)]);
    }

    #[test]
    fn converts_annex_b_to_length_prefixed() {
        let annexb = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3];
        assert_eq!(
            length_prefixed(&annexb, 4),
            [0, 0, 0, 3, 0x67, 1, 2, 0, 0, 0, 2, 0x68, 3]
        );
    }

    /// The burned-in frame index of every frame of `path`, in order.
    fn indices(path: &Path) -> Vec<u32> {
        let mut input = format::input(path).unwrap();
        let s = input.streams().best(media::Type::Video).unwrap();
        let v = s.index();
        let mut dec = codec::Context::from_parameters(s.parameters())
            .unwrap()
            .decoder()
            .video()
            .unwrap();
        let mut out = Vec::new();
        let mut f = frame::Video::empty();
        let mut take = |dec: &mut decoder::Video, out: &mut Vec<u32>| {
            while dec.receive_frame(&mut f).is_ok() {
                out.push(frame_index(f.data(0), f.stride(0)));
            }
        };
        for (s, p) in input.packets() {
            if s.index() == v {
                dec.send_packet(&p).unwrap();
                take(&mut dec, &mut out);
            }
        }
        dec.send_eof().unwrap();
        take(&mut dec, &mut out);
        out
    }

    fn cut(path: &Path, a: i64, b: i64) -> Cut {
        // Frames at 30 fps.
        let f = FLICKS_PER_SECOND / 30;
        Cut {
            path: path.into(),
            start: Time(a * f),
            end: Time(b * f),
        }
    }

    #[test]
    fn copies_whole_gops_and_smart_render_hits_exact_frames() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        // Keyframes every 30 frames.
        let src = dir.join("frame-index-1080p30.mp4");
        let tmp = std::env::temp_dir().join(format!("jvid-remux-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.join("out.mp4");
        let range = |a: u32, b: u32| (a..b).collect::<Vec<_>>();

        export(&[cut(&src, 40, 140)], &out, false, &mut |_| true).unwrap();
        assert_eq!(indices(&out), range(30, 150));
        if SMART_ENCODER.is_none() {
            return std::fs::remove_dir_all(&tmp).unwrap();
        }

        let cuts = [cut(&src, 45, 135), cut(&src, 200, 215), cut(&src, 250, 300)];
        export(&cuts, &out, true, &mut |_| true).unwrap();
        let want: Vec<_> = [range(45, 135), range(200, 215), range(250, 300)].concat();
        assert_eq!(indices(&out), want);
        let info = crate::probe::probe(&out).unwrap();
        let secs = info.duration.0 as f64 / FLICKS_PER_SECOND as f64;
        assert!((secs - want.len() as f64 / 30.0).abs() < 0.05, "{secs}");

        let err = export(&cuts, &out, true, &mut |_| false).unwrap_err();
        assert_eq!(err, "cancelled");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn keeps_audio_alongside_copied_video() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        let src = dir.join("23.976fps.mp4");
        let out = std::env::temp_dir().join(format!("jvid-remux-a-{}.mp4", std::process::id()));
        let s = |x: f64| Time((x * FLICKS_PER_SECOND as f64) as i64);
        let cuts = [
            Cut {
                path: src.clone(),
                start: s(1.0),
                end: s(2.0),
            },
            Cut {
                path: src,
                start: s(3.0),
                end: s(4.0),
            },
        ];
        let smart = SMART_ENCODER.is_some();
        export(&cuts, &out, smart, &mut |_| true).unwrap();
        let info = crate::probe::probe(&out).unwrap();
        std::fs::remove_file(&out).unwrap();
        assert_eq!(info.audio_streams, 1);
        let secs = info.duration.0 as f64 / FLICKS_PER_SECOND as f64;
        // The file's only keyframe is its first, so plain copy keeps all 5 s of each cut.
        let want = if smart { 2.0 } else { 10.0 };
        assert!((secs - want).abs() < 0.1, "{secs}");
    }

    #[test]
    fn lossless_minute_from_long_recording() {
        if cfg!(debug_assertions) {
            return eprintln!("skipping lossless timing in a debug build; run with --release");
        }
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        let src = dir.join("long-2h-1440p60.mp4");
        let out = std::env::temp_dir().join(format!("jvid-remux-long-{}.mp4", std::process::id()));
        let s = |x: f64| Time((x * FLICKS_PER_SECOND as f64) as i64);
        let cuts = [Cut {
            path: src,
            start: s(3600.3),
            end: s(3660.3),
        }];
        for smart in [false, SMART_ENCODER.is_some()] {
            let t = std::time::Instant::now();
            export(&cuts, &out, smart, &mut |_| true).unwrap();
            let took = t.elapsed();
            eprintln!("lossless minute, smart {smart}: {took:?}");
            // Smart render re-encodes up to two of this file's 10 s GOPs, so only copy is timed.
            assert!(smart || took.as_secs_f64() < 3.0, "{took:?}");
        }
        std::fs::remove_file(&out).unwrap();
    }
}
