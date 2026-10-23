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
    // From here `out` is ours: a failure or panic removes it, never a file that was there before.
    let mut written = Created(out, false);
    // Input stream index to output stream index, for video and audio only.
    let mut map = BTreeMap::new();
    let mut src = BTreeMap::new();
    for s in input.streams() {
        let params = s.parameters();
        if matches!(params.medium(), media::Type::Video | media::Type::Audio) {
            let mut o = output
                .add_stream(encoder::find(Id::None))
                .map_err(|e| e.to_string())?;
            o.set_parameters(params.clone());
            o.set_time_base(s.time_base());
            if smart
                && params.medium() == media::Type::Video
                && params.id() == Id::H264
                && let Some(avcc) = smart_avcc(&params, s.time_base())?
            {
                set_extradata(&mut o.parameters(), &avcc);
            }
            map.insert(s.index(), o.index());
            src.insert(o.index(), params);
        }
    }
    drop(input);
    output.write_header().map_err(|e| e.to_string())?;
    let mut w = Writer {
        out: output,
        map,
        src,
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
    w.out.write_trailer().map_err(|e| e.to_string())?;
    written.1 = true;
    Ok(())
}

/// Removes the output on drop unless the export finished (`.1`).
struct Created<'a>(&'a Path, bool);

impl Drop for Created<'_> {
    fn drop(&mut self) {
        if !self.1 {
            let _ = std::fs::remove_file(self.0);
        }
    }
}

struct Writer {
    out: Output,
    map: BTreeMap<usize, usize>,
    /// Per output stream, the first file's parameters, which every cut's must match.
    src: BTreeMap<usize, codec::Parameters>,
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
            if pts.is_some_and(|p| p <= last) {
                tracing::warn!(
                    "lossless: packet at a join moved {} ticks later",
                    last + 1 - d
                );
            }
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
        while let Some(p) = read(input)? {
            if p.stream() != v || !p.is_key() {
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

/// The next packet, `None` at the end; unlike `packets()`, a read error fails rather than ends.
fn read(input: &mut Input) -> Result<Option<Packet>, String> {
    let mut p = Packet::empty();
    match p.read(input) {
        Ok(()) => Ok(Some(p)),
        Err(ffmpeg_next::Error::Eof) => Ok(None),
        Err(e) => Err(format!("can't read the source: {e}")),
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
    for &i in &streams {
        let same = w.map.get(&i).is_some_and(|o| {
            same_params(&w.src[o], &input.stream(i).expect("listed").parameters())
        });
        if !same {
            return Err(format!(
                "{} has different encoding settings",
                cut.path.display()
            ));
        }
    }
    let video = input.streams().best(media::Type::Video).ok_or("no video")?;
    let (v, vtb, params) = (video.index(), video.time_base(), video.parameters());
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
    // The last video keyframe's pts, to spot open GOPs: frames after it in decode order that
    // show before it reference the GOP before, which smart render's parts would split.
    let mut key = None;
    let tb_of: BTreeMap<_, _> = input
        .streams()
        .map(|s| (s.index(), s.time_base()))
        .collect();
    while let Some(p) = read(&mut input)? {
        let i = p.stream();
        let Some(&tb) = tb_of.get(&i) else { continue };
        let pts = p.pts().unwrap_or(0);
        if i != v {
            if w.map.contains_key(&i) && audio_done.get(&i) != Some(&true) {
                let t = time(pts, tb);
                // Audio ending past the cut would overlap the next cut's first packet.
                if t >= end || time(pts + p.duration(), tb) > end {
                    audio_done.insert(i, true);
                } else if t >= start {
                    w.write(p, i, tb, start)?;
                }
            }
        } else if !video_done {
            if p.is_key() && pts >= to {
                video_done = true;
            } else {
                if p.is_key() {
                    key = Some(pts);
                } else if encodes && key.is_some_and(|k| pts < k) {
                    return Err(
                        "smart render can't cut open-GOP video; export with plain copy instead"
                            .into(),
                    );
                }
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
                            enc = Some(Smart::open(&params, vtb, (a, b), annexb(&p))?);
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

/// Where the file ends in its own (pts) time: a non-zero start time shifts it.
#[allow(unsafe_code)]
fn input_duration(path: &Path) -> Option<Time> {
    let input = format::input(path).ok()?;
    // SAFETY: a plain read of an open input's context.
    let (d, s) = (input.duration(), unsafe { (*input.as_ptr()).start_time });
    let s = if s == ffmpeg_next::ffi::AV_NOPTS_VALUE {
        0
    } else {
        s
    };
    (d > 0).then(|| time(s + d, Rational(1, 1_000_000)))
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
    /// The source's packets are Annex B like the encoder's, rather than length-prefixed.
    annexb: bool,
}

impl Smart {
    fn open(
        params: &codec::Parameters,
        tb: Rational,
        range: (i64, i64),
        annexb: bool,
    ) -> Result<Self, String> {
        let dec = codec::Context::from_parameters(params.clone())
            .and_then(|c| c.decoder().video())
            .map_err(|e| e.to_string())?;
        if dec.format() != Pixel::YUV420P {
            return Err("smart render needs 8-bit 4:2:0 video".into());
        }
        let enc = smart_encoder(params, (dec.width(), dec.height()), tb, false)?;
        Ok(Self {
            range,
            dec,
            enc,
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
            f.set_pts(Some(pts));
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
            let pts = p.pts();
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

/// x264 in the source stream's time base, so variable frame rate timestamps pass through as they
/// are. Fast, near-transparent, no B-frames so encoded packets decode in order, and parameter set
/// ids 1 so the in-band ones don't replace the source's (id 0) for the copied video.
#[allow(unsafe_code)]
fn smart_encoder(
    src: &codec::Parameters,
    size: (u32, u32),
    tb: Rational,
    global: bool,
) -> Result<encoder::Video, String> {
    // x264 also takes the inverse time base as its frame rate, which with constant quality only
    // feeds its level guess; the source's level is the right one anyway.
    // SAFETY: a plain read of valid parameters.
    let level = unsafe { (*src.as_ptr()).level };
    let x264 = match level > 0 {
        true => format!("sps-id=1:level={level}"),
        false => "sps-id=1".into(),
    };
    let opts = BTreeMap::from([
        ("crf".to_string(), "14".to_string()),
        ("bf".to_string(), "0".to_string()),
        ("preset".to_string(), "ultrafast".to_string()),
        ("x264-params".to_string(), x264),
    ]);
    let rate = Rate::new(tb.1 as u32, tb.0 as u32);
    SMART_ENCODER
        .and_then(|n| {
            crate::encode::open_video(n, Pixel::YUV420P, size, rate, global, false, &opts)
        })
        .map(|(_, e)| e)
        .ok_or_else(|| "smart render needs libx264, which this build doesn't use".into())
}

/// The source's avcC with x264's SPS and PPS (id 1) added, so players that only read parameter
/// sets from avcC (QuickTime, Safari) can decode the re-encoded edges. `None` when there's no
/// avcC to extend or no encoder to ask; fails if the source already uses id 1 itself.
fn smart_avcc(params: &codec::Parameters, tb: Rational) -> Result<Option<Vec<u8>>, String> {
    let Some((head, mut sps, mut pps, tail)) = parse_avcc(extradata(params)) else {
        return Ok(None);
    };
    if sps.iter().chain(&pps).any(|n| param_set_id(n) == Some(1)) {
        return Err("smart render can't use this video: it already uses parameter set id 1".into());
    }
    let dec = codec::Context::from_parameters(params.clone())
        .and_then(|c| c.decoder().video())
        .map_err(|e| e.to_string())?;
    let Ok(enc) = smart_encoder(params, (dec.width(), dec.height()), tb, true) else {
        return Ok(None);
    };
    let ours = length_prefixed(encoder_extradata(&enc), 4);
    let mut i = 0;
    while i + 4 <= ours.len() {
        let n = u32::from_be_bytes(ours[i..i + 4].try_into().expect("4 bytes")) as usize;
        let nal = ours[i + 4..(i + 4 + n).min(ours.len())].to_vec();
        match nal.first().map(|b| b & 0x1f) {
            Some(7) => sps.push(nal),
            Some(8) => pps.push(nal),
            _ => {}
        }
        i += 4 + n;
    }
    let mut out = head.to_vec();
    out.push(0xe0 | sps.len() as u8);
    for n in &sps {
        out.extend_from_slice(&(n.len() as u16).to_be_bytes());
        out.extend_from_slice(n);
    }
    out.push(pps.len() as u8);
    for n in &pps {
        out.extend_from_slice(&(n.len() as u16).to_be_bytes());
        out.extend_from_slice(n);
    }
    out.extend_from_slice(tail);
    Ok(Some(out))
}

/// avcC's 5 header bytes, its SPS and PPS NALs, and whatever follows them.
type Avcc<'a> = (&'a [u8], Vec<Vec<u8>>, Vec<Vec<u8>>, &'a [u8]);

fn parse_avcc(d: &[u8]) -> Option<Avcc<'_>> {
    if d.len() < 7 || d[0] != 1 {
        return None;
    }
    let list = |count: usize, i: &mut usize| -> Option<Vec<Vec<u8>>> {
        (0..count)
            .map(|_| {
                let n = u16::from_be_bytes([*d.get(*i)?, *d.get(*i + 1)?]) as usize;
                let nal = d.get(*i + 2..*i + 2 + n)?.to_vec();
                *i += 2 + n;
                Some(nal)
            })
            .collect()
    };
    let mut i = 6;
    let sps = list((d[5] & 0x1f) as usize, &mut i)?;
    let count = *d.get(i)? as usize;
    i += 1;
    let pps = list(count, &mut i)?;
    Some((&d[..5], sps, pps, &d[i..]))
}

/// An SPS's or PPS's own id: the first Exp-Golomb value after the SPS's 3 fixed bytes, or right
/// after a PPS's NAL header.
fn param_set_id(nal: &[u8]) -> Option<u32> {
    let skip = match nal.first()? & 0x1f {
        7 => 4,
        8 => 1,
        _ => return None,
    };
    let bits = nal.get(skip..)?;
    let bit = |k: usize| bits.get(k / 8).map(|b| (b >> (7 - k % 8)) & 1);
    let mut zeros = 0;
    while bit(zeros)? == 0 {
        zeros += 1;
    }
    let mut v = 1u32;
    for k in 0..zeros {
        v = (v << 1) | bit(zeros + 1 + k)? as u32;
    }
    Some(v - 1)
}

#[allow(unsafe_code)]
fn extradata(p: &codec::Parameters) -> &[u8] {
    // SAFETY: FFmpeg keeps `extradata_size` bytes at `extradata` while `p` lives.
    unsafe {
        let p = &*p.as_ptr();
        match p.extradata.is_null() {
            true => &[],
            false => std::slice::from_raw_parts(p.extradata, p.extradata_size as usize),
        }
    }
}

#[allow(unsafe_code)]
fn encoder_extradata(e: &encoder::Video) -> &[u8] {
    // SAFETY: as for `extradata`, on an opened encoder.
    unsafe {
        let c = &*e.as_ptr();
        match c.extradata.is_null() {
            true => &[],
            false => std::slice::from_raw_parts(c.extradata, c.extradata_size as usize),
        }
    }
}

/// Replaces `p`'s extradata with a padded FFmpeg-owned copy of `data`.
#[allow(unsafe_code)]
fn set_extradata(p: &mut codec::Parameters, data: &[u8]) {
    use ffmpeg_next::ffi;
    // SAFETY: the old buffer is FFmpeg's to free, and the new one is FFmpeg-allocated with the
    // padding its readers expect.
    unsafe {
        let p = &mut *p.as_mut_ptr();
        ffi::av_freep(&mut p.extradata as *mut *mut u8 as *mut std::ffi::c_void);
        let buf =
            ffi::av_mallocz(data.len() + ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize) as *mut u8;
        std::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len());
        p.extradata = buf;
        p.extradata_size = data.len() as i32;
    }
}

/// FNV-1a of the profile, pixel format and extradata: equal for streams whose packets can be
/// copied one after another, as far as a probe can tell.
#[allow(unsafe_code)]
pub(crate) fn params_hash(p: &codec::Parameters) -> u64 {
    // SAFETY: plain reads of valid parameters.
    let (profile, format) = unsafe { ((*p.as_ptr()).profile, (*p.as_ptr()).format) };
    let bytes = profile
        .to_le_bytes()
        .into_iter()
        .chain(format.to_le_bytes());
    bytes
        .chain(extradata(p).iter().copied())
        .fold(0xcbf2_9ce4_8422_2325, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        })
}

/// Whether copied packets of a stream with parameters `b` can follow ones from `a`.
#[allow(unsafe_code)]
fn same_params(a: &codec::Parameters, b: &codec::Parameters) -> bool {
    // SAFETY: plain reads of valid parameters.
    let key = |p: &codec::Parameters| unsafe {
        let p = &*p.as_ptr();
        (
            p.codec_id,
            p.format,
            p.profile,
            p.level,
            p.width,
            p.height,
            p.sample_rate,
        )
    };
    key(a) == key(b) && extradata(a) == extradata(b)
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
    fn reads_parameter_set_ids() {
        // SPS: header, profile, constraints, level, then ue(v) id 1 = bits 010.
        assert_eq!(param_set_id(&[0x67, 100, 0, 40, 0b0100_0000]), Some(1));
        assert_eq!(param_set_id(&[0x67, 100, 0, 40, 0b1000_0000]), Some(0));
        // PPS id 2 = bits 011.
        assert_eq!(param_set_id(&[0x68, 0b0110_0000]), Some(2));
    }

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("jvid-remux-{name}-{}.mp4", std::process::id()))
    }

    #[test]
    fn refuses_files_with_other_encoding_settings_and_cleans_up() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        let out = tmp("mismatch");
        // Same codec, size and rate; Baseline rather than High profile, so another SPS.
        let cuts = [
            cut(&dir.join("frame-index-1080p30.mp4"), 0, 30),
            cut(&dir.join("baseline-1080p30.mp4"), 0, 30),
        ];
        let err = export(&cuts, &out, false, &mut |_| true).unwrap_err();
        assert!(err.ends_with("has different encoding settings"), "{err}");
        assert!(!out.exists());
    }

    #[test]
    fn a_failure_before_writing_leaves_an_existing_file() {
        crate::ffmpeg::init().unwrap();
        let out = tmp("existing");
        std::fs::write(&out, b"mine").unwrap();
        let missing = cut(Path::new("/nonexistent/clip.mp4"), 0, 30);
        assert!(export(&[missing], &out, false, &mut |_| true).is_err());
        assert_eq!(std::fs::read(&out).unwrap(), b"mine");
        std::fs::remove_file(&out).unwrap();
    }

    /// Video packet pts of `path` in presentation order.
    fn video_pts(path: &Path) -> Vec<i64> {
        let mut input = format::input(path).unwrap();
        let v = input.streams().best(media::Type::Video).unwrap().index();
        let mut pts: Vec<_> = input
            .packets()
            .filter(|(s, _)| s.index() == v)
            .filter_map(|(_, p)| p.pts())
            .collect();
        pts.sort();
        pts
    }

    #[test]
    fn smart_render_keeps_variable_frame_timing() {
        let Some(dir) = corpus() else { return };
        if SMART_ENCODER.is_none() {
            return;
        }
        crate::ffmpeg::init().unwrap();
        let src = dir.join("vfr-phone.mp4");
        let out = tmp("vfr");
        let s = |x: f64| Time((x * FLICKS_PER_SECOND as f64) as i64);
        let cuts = [Cut {
            path: src,
            start: s(1.05),
            end: s(3.95),
        }];
        export(&cuts, &out, true, &mut |_| true).unwrap();
        let pts = video_pts(&out);
        // x264's parameter sets join the source's in avcC, for players that read only those.
        let input = format::input(&out).unwrap();
        let params = input
            .streams()
            .best(media::Type::Video)
            .unwrap()
            .parameters();
        let (_, sps, pps, _) = parse_avcc(extradata(&params)).unwrap();
        assert_eq!((sps.len(), pps.len()), (2, 2));
        assert_eq!(param_set_id(&sps[1]), Some(1));
        let info = crate::probe::probe(&out).unwrap();
        std::fs::remove_file(&out).unwrap();
        // Frame gaps alternate 1 and 2 frames at 30 fps: no two frames share a time, and the
        // frame count matches the source's 20 frames a second.
        assert!(pts.windows(2).all(|w| w[0] < w[1]), "{pts:?}");
        assert!(
            (pts.len() as f64 - 2.9 * 20.0).abs() <= 2.0,
            "{}",
            pts.len()
        );
        let secs = info.duration.0 as f64 / FLICKS_PER_SECOND as f64;
        assert!((secs - 2.9).abs() < 0.1, "{secs}");
    }

    #[test]
    fn smart_render_refuses_open_gops() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        let out = tmp("open-gop");
        let cuts = [cut(&dir.join("open-gop.mp4"), 40, 100)];
        if SMART_ENCODER.is_some() {
            let err = export(&cuts, &out, true, &mut |_| true).unwrap_err();
            assert!(err.contains("open-GOP"), "{err}");
            assert!(!out.exists());
        }
        // Plain copy keeps whole GOPs, so open ones are fine.
        export(&cuts, &out, false, &mut |_| true).unwrap();
        std::fs::remove_file(&out).unwrap();
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
