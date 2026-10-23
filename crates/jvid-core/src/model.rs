//! The in-memory project: settings, media assets, a sequence of tracks and
//! clips, markers and in/out points. No I/O; probing fills `MediaAsset::probe`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::keyframe::{Keyframe, Value};
use crate::time::{Rate, Time};

macro_rules! id {
    ($name:ident) => {
        /// Stable id, unique within its project or sequence and never reused.
        #[derive(
            Debug,
            Clone,
            Copy,
            Default,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);
    };
}
id!(AssetId);
id!(TrackId);
id!(ClipId);
id!(MarkerId);
id!(LinkId);
id!(SequenceId);

/// Project-wide output settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSettings {
    pub width: u32,
    pub height: u32,
    pub frame_rate: Rate,
    pub sample_rate: u32,
    /// Straight RGBA.
    pub background: [u8; 4],
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            frame_rate: Rate::new(30, 1),
            sample_rate: 48_000,
            background: [0, 0, 0, 255],
        }
    }
}

/// What probing found in a media file.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ProbeInfo {
    pub duration: Time,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<Rate>,
    pub sample_rate: Option<u32>,
    pub audio_streams: u32,
    /// Demuxer name(s), e.g. `mov,mp4,m4a,3gp,3g2,mj2`.
    #[serde(default)]
    pub container: String,
    /// Every stream in file order.
    #[serde(default)]
    pub streams: Vec<StreamInfo>,
    /// Bytes on disk when probed; relink uses it to tell same-named files apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size: Option<u64>,
}

impl ProbeInfo {
    /// The nth audio stream's title, e.g. "Mic", else "Audio n" counting from 1.
    pub fn audio_name(&self, n: u32) -> String {
        let mut titles = self.streams.iter().filter_map(|s| match s {
            StreamInfo::Audio(a) => Some(a.title.as_ref()),
            _ => None,
        });
        let title = titles.nth(n as usize).flatten();
        title.cloned().unwrap_or_else(|| format!("Audio {}", n + 1))
    }
}

/// One stream of a probed file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamInfo {
    Video(VideoInfo),
    Audio(AudioInfo),
    /// Subtitles, data, attachments: kept so stream indices line up, otherwise ignored.
    Other {
        codec: String,
    },
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VideoInfo {
    pub codec: String,
    /// Coded size, before rotation.
    pub width: u32,
    pub height: u32,
    pub sample_aspect: Option<Rate>,
    pub display_aspect: Option<Rate>,
    /// Frames over duration.
    pub avg_frame_rate: Option<Rate>,
    /// Lowest rate that represents every timestamp (FFmpeg's `r_frame_rate`).
    pub real_frame_rate: Option<Rate>,
    /// Variable frame rate: average and real rates disagree.
    pub vfr: bool,
    /// Clockwise degrees to turn decoded frames upright: 0, 90, 180 or 270.
    pub rotation: u32,
    /// FFmpeg names, e.g. `bt709`; `None` when unspecified.
    pub color_primaries: Option<String>,
    pub color_transfer: Option<String>,
    /// `tv` (limited) or `pc` (full).
    pub color_range: Option<String>,
    /// Hash of the profile, pixel format and codec extradata (SPS/PPS); lossless export only
    /// joins files whose hashes match. 0 in projects probed before it existed.
    #[serde(default)]
    pub params: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AudioInfo {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    /// FFmpeg channel mask (`AV_CH_*` bits); 0 when unspecified.
    pub channel_layout: u64,
    /// Stream title tag, e.g. "Mic" in an OBS recording.
    pub title: Option<String>,
}

/// A source file referenced by clips.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaAsset {
    pub id: AssetId,
    pub path: PathBuf,
    /// `None` until the file has been probed.
    pub probe: Option<ProbeInfo>,
    /// The bin name; `None` shows the file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Source in/out marks, set in the Source viewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_point: Option<Time>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_point: Option<Time>,
    /// A generated solid of this straight RGBA (0..=1) instead of a file; `path` is empty.
    /// Clips override it with a `color` keyframe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solid: Option<[f64; 4]>,
    /// A generated text layer instead of a file; `path` is empty. Clips override its fill
    /// with a `color` keyframe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<Text>,
    /// A nested sequence instead of a file; `path` is empty. Its clips play that sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<SequenceId>,
}

/// Horizontal alignment of text lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Align {
    Left,
    #[default]
    Center,
    Right,
}

/// A text/title layer, laid out over the whole frame. Colors are straight RGBA (0..=1);
/// sizes and offsets are in project pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Text {
    pub content: String,
    /// A family name; empty picks the default sans-serif.
    pub family: String,
    pub size: f32,
    /// CSS weight, 100..=900.
    pub weight: u16,
    pub color: [f64; 4],
    pub align: Align,
    /// Center of the text block, normalized 0..=1 across the frame.
    pub anchor: [f32; 2],
    /// Outline width; 0 for none.
    pub outline: f32,
    pub outline_color: [f64; 4],
    /// Drop shadow offset; zero for none.
    pub shadow: [f32; 2],
    pub shadow_color: [f64; 4],
    /// Background box padding around the text, drawn when `box_color` is visible.
    pub box_padding: f32,
    pub box_color: [f64; 4],
}

impl Default for Text {
    fn default() -> Self {
        Self {
            content: "Title".into(),
            family: String::new(),
            size: 96.0,
            weight: 700,
            color: [1.0; 4],
            align: Align::Center,
            anchor: [0.5, 0.5],
            outline: 0.0,
            outline_color: [0.0, 0.0, 0.0, 1.0],
            shadow: [0.0, 0.0],
            shadow_color: [0.0, 0.0, 0.0, 0.6],
            box_padding: 16.0,
            box_color: [0.0; 4],
        }
    }
}

impl Text {
    /// Style properties clips animate with keyframes of these names; fill is the clip's
    /// `color` keyframe, as for a solid. `anchor` and `size` are prefixed so they don't
    /// collide with the transform's.
    pub const ANIMATED: [&'static str; 8] = [
        "text_size",
        "text_anchor",
        "outline",
        "outline_color",
        "shadow",
        "shadow_color",
        "box_padding",
        "box_color",
    ];

    /// The value of `name`, one of [`Self::ANIMATED`].
    pub fn get(&self, name: &str) -> Option<Value> {
        let v2 = |v: [f32; 2]| Value::Vec2(v.map(f64::from));
        Some(match name {
            "text_size" => Value::Scalar(self.size.into()),
            "text_anchor" => v2(self.anchor),
            "outline" => Value::Scalar(self.outline.into()),
            "outline_color" => Value::Color(self.outline_color),
            "shadow" => v2(self.shadow),
            "shadow_color" => Value::Color(self.shadow_color),
            "box_padding" => Value::Scalar(self.box_padding.into()),
            "box_color" => Value::Color(self.box_color),
            _ => return None,
        })
    }

    /// Sets `name` to `v`; unknown names and mismatched kinds are ignored.
    pub fn set(&mut self, name: &str, v: Value) {
        let v2 = |v: [f64; 2]| v.map(|n| n as f32);
        match (name, v) {
            ("text_size", Value::Scalar(n)) => self.size = n.max(1.0) as f32,
            ("text_anchor", Value::Vec2(n)) => self.anchor = v2(n),
            ("outline", Value::Scalar(n)) => self.outline = n.max(0.0) as f32,
            ("outline_color", Value::Color(c)) => self.outline_color = c,
            ("shadow", Value::Vec2(n)) => self.shadow = v2(n),
            ("shadow_color", Value::Color(c)) => self.shadow_color = c,
            ("box_padding", Value::Scalar(n)) => self.box_padding = n.max(0.0) as f32,
            ("box_color", Value::Color(c)) => self.box_color = c,
            _ => {}
        }
    }

    /// This style with `keyed` values (from [`Clip::text_keys`]) applied over it.
    pub fn with(&self, keyed: &[(&str, Value)]) -> Text {
        let mut t = self.clone();
        for (name, v) in keyed {
            t.set(name, *v);
        }
        t
    }

    /// The bin's presets: name and style.
    pub fn presets() -> [(&'static str, Text); 3] {
        let d = Text::default();
        [
            ("Title", d.clone()),
            (
                "Lower Third",
                Text {
                    content: "Name\nRole".into(),
                    size: 48.0,
                    weight: 600,
                    align: Align::Left,
                    anchor: [0.25, 0.8],
                    box_color: [0.0, 0.0, 0.0, 0.7],
                    ..d.clone()
                },
            ),
            (
                "Caption",
                Text {
                    content: "Caption".into(),
                    size: 40.0,
                    weight: 400,
                    anchor: [0.5, 0.9],
                    outline: 2.0,
                    shadow: [2.0, 2.0],
                    ..d
                },
            ),
        ]
    }
}

impl MediaAsset {
    /// Made by jvid (a solid, text or sequence) rather than read from a file.
    pub fn generated(&self) -> bool {
        self.solid.is_some() || self.text.is_some() || self.sequence.is_some()
    }

    pub fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            self.path.file_name().map_or_else(
                || self.path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    pub hidden: bool,
    pub locked: bool,
    pub muted: bool,
    pub solo: bool,
    /// Linear gain, 1.0 = unity.
    pub volume: f32,
    /// -1.0 (left) to 1.0 (right).
    pub pan: f32,
    /// Lane height in points, [`Track::MIN_HEIGHT`] to [`Track::MAX_HEIGHT`].
    #[serde(default = "Track::default_height")]
    pub height: f32,
    /// Insert and overwrite land here; at most one track of each kind.
    #[serde(default)]
    pub target: bool,
    /// Holds imported subtitles: its text clips are the cues an `.srt` sidecar writes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub subtitles: bool,
    /// EQ and compressor on the track's mix, before its volume and pan.
    #[serde(default, skip_serializing_if = "AudioFx::is_default")]
    pub fx: AudioFx,
    /// Sorted by `start`, never overlapping. Mutate through `Sequence`.
    clips: Vec<Clip>,
}

/// A track's settings: everything but its kind and clips.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackSettings {
    pub id: TrackId,
    pub name: String,
    pub hidden: bool,
    pub locked: bool,
    pub muted: bool,
    pub solo: bool,
    pub volume: f32,
    pub pan: f32,
    pub height: f32,
    pub target: bool,
    pub fx: AudioFx,
}

impl Track {
    pub const MIN_HEIGHT: f32 = 24.0;
    pub const MAX_HEIGHT: f32 = 120.0;

    pub fn default_height() -> f32 {
        40.0
    }

    pub fn settings(&self) -> TrackSettings {
        TrackSettings {
            id: self.id,
            name: self.name.clone(),
            hidden: self.hidden,
            locked: self.locked,
            muted: self.muted,
            solo: self.solo,
            volume: self.volume,
            pan: self.pan,
            height: self.height,
            target: self.target,
            fx: self.fx,
        }
    }

    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    /// The first clip other than `ignore` intersecting `[start, end)`.
    fn overlap(&self, start: Time, end: Time, ignore: Option<ClipId>) -> Option<ClipId> {
        self.clips
            .iter()
            .find(|c| Some(c.id) != ignore && c.start < end && start < c.end())
            .map(|c| c.id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub asset: AssetId,
    pub source_in: Time,
    pub source_out: Time,
    /// Where the clip starts on the timeline.
    pub start: Time,
    /// Clips sharing a link group move and select together.
    pub link: Option<LinkId>,
    pub params: BTreeMap<String, f64>,
    /// Animated params by name, sorted by time; see [`crate::keyframe`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keyframes: BTreeMap<String, Vec<Keyframe>>,
    /// Static transform, crop and opacity; keyframes of the same names override each field.
    #[serde(default, skip_serializing_if = "Transform::is_default")]
    pub transform: Transform,
    /// The audio stream an audio clip plays, the nth in file order, or `None` for all of them
    /// mixed. Older projects lack it and played the first.
    #[serde(default = "first_stream", skip_serializing_if = "is_first_stream")]
    pub stream: Option<u32>,
    /// A disabled clip stays on the timeline but evaluation skips it.
    #[serde(default = "enabled", skip_serializing_if = "is_enabled")]
    pub enabled: bool,
    /// Playback rate, above 0: at 2.0 the source range plays in half the time.
    #[serde(default = "unit_speed", skip_serializing_if = "is_unit_speed")]
    pub speed: f64,
    /// Plays the source range backwards, last frame first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reverse: bool,
    /// A held source frame shown for the whole clip, silent; the source range then only sets
    /// the length, at 1:1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freeze: Option<Time>,
}

fn unit_speed() -> f64 {
    1.0
}

fn is_unit_speed(s: &f64) -> bool {
    *s == 1.0
}

fn enabled() -> bool {
    true
}

fn is_enabled(e: &bool) -> bool {
    *e
}

fn first_stream() -> Option<u32> {
    Some(0)
}

fn is_first_stream(s: &Option<u32>) -> bool {
    *s == Some(0)
}

/// Where a clip lands in the frame, in project pixels. The clip is first fitted whole and
/// centred, as in the compositor; this then applies in the fitted image's pixels.
///
/// Keyframe names: `position`, `anchor` (`Vec2`), `scale` (`Scalar` for linked, `Vec2` for
/// separate X/Y), `rotation`, `opacity`, `crop_left`, `crop_top`, `crop_right`, `crop_bottom`,
/// the color adjustments by field name (`exposure`, `contrast`, ..., `lut_intensity`), effect
/// parameters as `fx{index}.{param}` (see [`Effect::key`]), mask shapes and feather as
/// `mask{index}.{param}` (see [`Mask::key`]), and for audio clips `volume` and `pan`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transform {
    /// Where the anchor lands, as an offset from the frame centre, y down.
    pub position: [f64; 2],
    pub scale: [f64; 2],
    /// Degrees clockwise about the anchor.
    pub rotation: f64,
    /// Offset from the image centre, y down; the point that position places and rotation pivots on.
    pub anchor: [f64; 2],
    /// Pixels removed from the left, top, right and bottom; the rest stays where it was.
    pub crop: [f64; 4],
    /// 0..=1.
    pub opacity: f64,
    /// How the clip mixes with the layers below; not animatable.
    pub blend: Blend,
    /// Audio clip gain, linear, 1.0 = unity; applied before the track's.
    pub volume: f64,
    /// Audio clip balance, -1.0 (left) to 1.0 (right); centre leaves both sides at unity.
    pub pan: f64,
    /// Audio clip EQ and compressor, before its gain; not animatable.
    pub audio_fx: AudioFx,
    /// Color adjustments, applied in linear light before blending; all 0 leaves pixels untouched.
    /// Stops, -4..=4.
    pub exposure: f64,
    /// -1..=1 each: contrast about mid grey, saturation, blue..orange and green..magenta.
    pub contrast: f64,
    pub saturation: f64,
    pub temperature: f64,
    pub tint: f64,
    /// A `.cube` file applied after the adjustments, mixed in by `lut_intensity` (0..=1).
    pub lut: Option<String>,
    pub lut_intensity: f64,
    /// Fade lengths from the clip's start and end; they scale opacity and gain, and trims keep
    /// them since they hang off the edges. Longer than the clip clamps to it.
    pub fade_in: Time,
    pub fade_out: Time,
    pub fade_curve: FadeCurve,
    /// The transition into the next clip on the track, centred on this clip's end; it plays
    /// only while that clip starts exactly there, from the media handles on both sides.
    pub transition: Option<Transition>,
    /// WGSL effects run on the clip's image in order, before its color adjustments.
    pub effects: Vec<Effect>,
    /// Shapes cut out of the clip's image, combined in order, before its effects.
    pub masks: Vec<Mask>,
}

/// A clip's or track's audio effects, in processing order. The defaults change nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioFx {
    pub eq: Equalizer,
    pub compressor: Compressor,
}

impl AudioFx {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Three bands: a low shelf, a peak and a high shelf (RBJ cookbook, shelf slope 1). Gains in dB,
/// frequencies in Hz.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Equalizer {
    /// Off bypasses it, keeping the settings.
    pub enabled: bool,
    pub low: f32,
    pub low_freq: f32,
    pub mid: f32,
    pub mid_freq: f32,
    /// The peak's Q.
    pub mid_q: f32,
    pub high: f32,
    pub high_freq: f32,
}

impl Default for Equalizer {
    fn default() -> Self {
        Self {
            enabled: true,
            low: 0.0,
            low_freq: 100.0,
            mid: 0.0,
            mid_freq: 1_000.0,
            mid_q: 1.0,
            high: 0.0,
            high_freq: 10_000.0,
        }
    }
}

/// A feed-forward peak compressor, stereo-linked. The default 1:1 ratio leaves the sound alone.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Compressor {
    /// Off bypasses it, keeping the settings.
    pub enabled: bool,
    /// dBFS.
    pub threshold: f32,
    /// Input dB over the threshold per output dB, at least 1.
    pub ratio: f32,
    /// Milliseconds.
    pub attack: f32,
    pub release: f32,
    /// dB of gain after compressing.
    pub makeup: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: -20.0,
            ratio: 1.0,
            attack: 10.0,
            release: 100.0,
            makeup: 0.0,
        }
    }
}

/// A user WGSL effect on a clip: the effect file's stem and its parameter values by name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub shader: String,
    pub params: BTreeMap<String, f64>,
    /// A bypassed effect keeps its settings and keyframes but doesn't render.
    #[serde(default = "enabled", skip_serializing_if = "is_enabled")]
    pub enabled: bool,
}

impl Effect {
    /// The keyframe name of parameter `param` of the clip's `index`th effect.
    pub fn key(index: usize, param: &str) -> String {
        format!("fx{index}.{param}")
    }
}

/// A shape that keeps (or, inverted, cuts out) part of a clip's image. Points are in the fitted
/// image's pixels as offsets from its centre, y down, as [`Transform::anchor`] is, so a mask
/// moves with its clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    pub shape: MaskShape,
    /// A rectangle or ellipse: its centre, then its width and height. A bezier: each vertex
    /// then its in and out handles, relative to it; the path closes back to the first vertex.
    pub points: Vec<[f64; 2]>,
    /// Width of the soft edge, centred on the outline, in the same pixels.
    pub feather: f64,
    /// 0..=1.
    pub opacity: f64,
    pub invert: bool,
    pub mode: MaskMode,
    /// A bypassed mask keeps its shape and keyframes but doesn't render.
    #[serde(default = "enabled", skip_serializing_if = "is_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskShape {
    Rectangle,
    Ellipse,
    Bezier,
}

/// How a mask combines with the masks above it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskMode {
    #[default]
    Add,
    Subtract,
}

impl Mask {
    /// A hard-edged, opaque mask.
    pub fn new(shape: MaskShape, points: Vec<[f64; 2]>) -> Self {
        Self {
            shape,
            points,
            feather: 0.0,
            opacity: 1.0,
            invert: false,
            mode: MaskMode::Add,
            enabled: true,
        }
    }

    /// The keyframe name of `param` (`feather`, `opacity`, or `p{n}` for point n) of the
    /// clip's `index`th mask.
    pub fn key(index: usize, param: &str) -> String {
        format!("mask{index}.{param}")
    }

    /// The keyframe name of point `n` of the clip's `index`th mask.
    pub fn point(index: usize, n: usize) -> String {
        format!("mask{index}.p{n}")
    }
}

/// A transition at a cut, `duration` long and centred on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    pub kind: TransitionKind,
    pub duration: Time,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    Crossfade,
    DipToBlack,
    DipToWhite,
}

/// The shape of a clip fade.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FadeCurve {
    #[default]
    Linear,
    /// sin(x·π/2): two overlapping fades keep constant power.
    EqualPower,
}

/// A layer blend mode, as in the W3C compositing spec; the index is the compositor's shader code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blend {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Add,
    Darken,
    Lighten,
    Difference,
}

impl Blend {
    pub const ALL: [Blend; 8] = [
        Blend::Normal,
        Blend::Multiply,
        Blend::Screen,
        Blend::Overlay,
        Blend::Add,
        Blend::Darken,
        Blend::Lighten,
        Blend::Difference,
    ];
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0; 2],
            scale: [1.0; 2],
            rotation: 0.0,
            anchor: [0.0; 2],
            crop: [0.0; 4],
            opacity: 1.0,
            blend: Blend::Normal,
            volume: 1.0,
            pan: 0.0,
            audio_fx: AudioFx::default(),
            exposure: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            temperature: 0.0,
            tint: 0.0,
            lut: None,
            lut_intensity: 1.0,
            fade_in: Time(0),
            fade_out: Time(0),
            fade_curve: FadeCurve::Linear,
            transition: None,
            effects: Vec::new(),
            masks: Vec::new(),
        }
    }
}

impl Transform {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Exposure, contrast, saturation, temperature and tint, the compositor's grade uniform.
    pub fn grade(&self) -> [f64; 5] {
        [
            self.exposure,
            self.contrast,
            self.saturation,
            self.temperature,
            self.tint,
        ]
    }
}

impl Clip {
    /// The source rate: 1 for a freeze, which spends its range 1:1.
    fn rate(&self) -> f64 {
        if self.freeze.is_some() {
            1.0
        } else {
            self.speed
        }
    }

    /// Timeline length of `source` source time. Floors, as [`Clip::source_len`] does, so a
    /// length converted there and back never grows.
    pub fn timeline_len(&self, source: Time) -> Time {
        Time((source.0 as f64 / self.rate()).floor() as i64)
    }

    /// Source time spent over `timeline` timeline time.
    pub fn source_len(&self, timeline: Time) -> Time {
        Time((timeline.0 as f64 * self.rate()).floor() as i64)
    }

    /// Length on the timeline: the source range at the clip's speed.
    pub fn duration(&self) -> Time {
        self.timeline_len(self.source_out - self.source_in)
    }

    /// The source time shown at timeline time `t`, held inside the source range.
    pub fn source_at(&self, t: Time) -> Time {
        if let Some(f) = self.freeze {
            return f;
        }
        let last = Time(self.source_out.0 - 1).max(self.source_in);
        self.source_beyond(t).clamp(self.source_in, last)
    }

    /// [`Clip::source_at`] carried on past the clip's edges into the media's handles, as a
    /// transition shows them; never before the media's start.
    pub fn source_beyond(&self, t: Time) -> Time {
        if let Some(f) = self.freeze {
            return f;
        }
        let d = self.source_len(t - self.start);
        let last = Time(self.source_out.0 - 1).max(self.source_in);
        let s = if self.reverse {
            last - d
        } else {
            self.source_in + d
        };
        s.max(Time(0))
    }

    /// Where source time `s` shows on the timeline, the inverse of [`Clip::source_at`]; a
    /// freeze shows it from the start.
    pub fn timeline_at(&self, s: Time) -> Time {
        let d = match (self.freeze, self.reverse) {
            (Some(_), _) => Time(0),
            (None, false) => s - self.source_in,
            (None, true) => Time(self.source_out.0 - 1) - s,
        };
        self.start + self.timeline_len(d)
    }

    /// Keeps the first `len` of the clip on the timeline; its end lands at or before
    /// `start + len`.
    pub(crate) fn keep_head(&mut self, len: Time) {
        let n = self.source_len(len);
        if self.reverse {
            self.source_in = self.source_out - n;
        } else {
            self.source_out = self.source_in + n;
        }
    }

    /// Keeps the last `len` of the clip on the timeline, its end staying where it is.
    pub(crate) fn keep_tail(&mut self, len: Time) {
        let end = self.end();
        let n = self.source_len(len);
        if self.reverse {
            self.source_out = self.source_in + n;
        } else {
            self.source_in = self.source_out - n;
        }
        self.start = end - self.duration();
    }

    /// The animated value of `param` at timeline time `t`.
    pub fn value_at(&self, param: &str, t: Time) -> Option<crate::keyframe::Value> {
        let keys = self.keyframes.get(param)?;
        crate::keyframe::value_at(keys, self.source_at(t))
    }

    /// The keyed [`Text::ANIMATED`] values at timeline time `t`.
    pub fn text_keys(&self, t: Time) -> Vec<(&'static str, Value)> {
        let keyed = Text::ANIMATED
            .iter()
            .filter_map(|n| Some((*n, self.value_at(n, t)?)));
        keyed.collect()
    }

    /// The transform at timeline time `t`, keyframes applied over the static values.
    pub fn transform_at(&self, t: Time) -> Transform {
        use crate::keyframe::Value::{Scalar, Vec2};
        let mut x = self.transform.clone();
        let vec2 = |name, v: &mut [f64; 2]| {
            if let Some(Vec2(n)) = self.value_at(name, t) {
                *v = n;
            }
        };
        vec2("position", &mut x.position);
        vec2("anchor", &mut x.anchor);
        match self.value_at("scale", t) {
            Some(Scalar(s)) => x.scale = [s; 2],
            Some(Vec2(s)) => x.scale = s,
            _ => {}
        }
        let scalar = |name, v: &mut f64| {
            if let Some(Scalar(n)) = self.value_at(name, t) {
                *v = n;
            }
        };
        let crop = ["crop_left", "crop_top", "crop_right", "crop_bottom"];
        for (name, v) in crop.into_iter().zip(&mut x.crop) {
            scalar(name, v);
        }
        scalar("rotation", &mut x.rotation);
        scalar("opacity", &mut x.opacity);
        x.opacity = x.opacity.clamp(0.0, 1.0);
        scalar("volume", &mut x.volume);
        scalar("pan", &mut x.pan);
        scalar("exposure", &mut x.exposure);
        x.exposure = x.exposure.clamp(-4.0, 4.0);
        for (name, v) in [
            ("contrast", &mut x.contrast),
            ("saturation", &mut x.saturation),
            ("temperature", &mut x.temperature),
            ("tint", &mut x.tint),
            ("lut_intensity", &mut x.lut_intensity),
        ] {
            scalar(name, v);
            *v = v.clamp(-1.0, 1.0);
        }
        x.lut_intensity = x.lut_intensity.clamp(0.0, 1.0);
        for (i, e) in x.effects.iter_mut().enumerate() {
            for (name, v) in &mut e.params {
                if let Some(Scalar(n)) = self.value_at(&Effect::key(i, name), t) {
                    *v = n;
                }
            }
        }
        for (i, m) in x.masks.iter_mut().enumerate() {
            for (n, p) in m.points.iter_mut().enumerate() {
                if let Some(Vec2(v)) = self.value_at(&Mask::point(i, n), t) {
                    *p = v;
                }
            }
            for (name, v) in [("feather", &mut m.feather), ("opacity", &mut m.opacity)] {
                if let Some(Scalar(n)) = self.value_at(&Mask::key(i, name), t) {
                    *v = n;
                }
            }
            m.feather = m.feather.max(0.0);
            m.opacity = m.opacity.clamp(0.0, 1.0);
        }
        x
    }

    /// Volume and pan at timeline time `t`, cheaper than [`Clip::transform_at`] for per-sample use.
    pub fn gain_at(&self, t: Time) -> (f64, f64) {
        use crate::keyframe::Value::Scalar;
        let get = |name, v: f64| match self.value_at(name, t) {
            Some(Scalar(n)) => n,
            _ => v,
        };
        (
            get("volume", self.transform.volume),
            get("pan", self.transform.pan),
        )
    }

    /// Whether the clip has volume or pan keyframes or fades, so its gain changes within a
    /// block.
    pub fn gain_animated(&self) -> bool {
        let x = &self.transform;
        x.fade_in > Time(0)
            || x.fade_out > Time(0)
            || ["volume", "pan"]
                .iter()
                .any(|n| self.keyframes.get(*n).is_some_and(|k| !k.is_empty()))
    }

    /// The fade multiplier at timeline time `t`, 0..=1, for opacity and gain.
    pub fn fade_at(&self, t: Time) -> f64 {
        let d = self.duration().0.max(1);
        let ramp = |len: Time, from: i64| match len.0.min(d) {
            n if n > 0 => (from as f64 / n as f64).clamp(0.0, 1.0),
            _ => 1.0,
        };
        let x = &self.transform;
        // Measured to the last instant shown, so a fade out reaches 0 on the final frame's start.
        let x = ramp(x.fade_in, (t - self.start).0) * ramp(x.fade_out, (self.end() - t).0);
        match self.transform.fade_curve {
            FadeCurve::Linear => x,
            FadeCurve::EqualPower => (x * std::f64::consts::FRAC_PI_2).sin(),
        }
    }

    /// Exclusive end on the timeline.
    pub fn end(&self) -> Time {
        self.start + self.duration()
    }
}

/// A point marker, or a ranged one when `duration` is non-zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: MarkerId,
    pub time: Time,
    pub duration: Time,
    pub name: String,
    /// Index into the app's marker palette.
    #[serde(default)]
    pub color: u8,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelError {
    UnknownTrack(TrackId),
    UnknownClip(ClipId),
    /// The clip would overlap another clip on the same track.
    Overlap(ClipId),
    /// `source_out` is not after `source_in`.
    EmptyClip,
    UnknownMarker(MarkerId),
    UnknownAsset(AssetId),
    /// No keyframe on the clip's param at that source time.
    UnknownKeyframe(ClipId, Time),
    /// The edit touches a locked track.
    Locked(TrackId),
    /// The clip can't go on a track of that kind.
    WrongKind(TrackId),
    /// No gap on the track at that time.
    NoGap(TrackId),
    /// No in/out range is marked.
    NoRange,
    /// A speed that isn't a positive, finite rate.
    BadSpeed,
    /// The time is outside the clip.
    OutsideClip(ClipId),
    /// No media past the cut on one side or both for a transition there.
    NoHandles(ClipId),
    UnknownSequence(SequenceId),
    /// The edit would nest a sequence inside itself, directly or through others.
    Cycle,
    /// The open sequence can't be removed.
    SequenceOpen,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTrack(id) => write!(f, "unknown track {}", id.0),
            Self::UnknownClip(id) => write!(f, "unknown clip {}", id.0),
            Self::Overlap(id) => write!(f, "clip would overlap clip {}", id.0),
            Self::EmptyClip => f.write_str("clip source out must be after source in"),
            Self::UnknownMarker(id) => write!(f, "unknown marker {}", id.0),
            Self::UnknownAsset(id) => write!(f, "unknown asset {}", id.0),
            Self::UnknownKeyframe(id, t) => write!(f, "no keyframe at {} on clip {}", t.0, id.0),
            Self::Locked(id) => write!(f, "track {} is locked", id.0),
            Self::WrongKind(id) => write!(f, "track {} is the wrong kind", id.0),
            Self::NoGap(id) => write!(f, "no gap there on track {}", id.0),
            Self::NoRange => f.write_str("no in/out range is marked"),
            Self::BadSpeed => f.write_str("speed must be above 0"),
            Self::OutsideClip(id) => write!(f, "time is outside clip {}", id.0),
            Self::NoHandles(_) => f.write_str(
                "no media handles at that cut for a transition: trim the clips so media remains past it on both sides",
            ),
            Self::UnknownSequence(id) => write!(f, "unknown sequence {}", id.0),
            Self::Cycle => f.write_str("a sequence can't be nested inside itself"),
            Self::SequenceOpen => f.write_str("the open sequence can't be removed"),
        }
    }
}

impl std::error::Error for ModelError {}

/// Tracks (video top-first, then audio), markers and in/out points.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Sequence {
    /// 0 for a project's first sequence, as in files from before nesting.
    #[serde(default, skip_serializing_if = "is_first")]
    id: SequenceId,
    tracks: Vec<Track>,
    pub(crate) markers: Vec<Marker>,
    pub(crate) in_point: Option<Time>,
    pub(crate) out_point: Option<Time>,
    next_id: u64,
}

/// A clip to place; the sequence assigns its id.
#[derive(Debug, Clone, PartialEq)]
pub struct NewClip {
    pub asset: AssetId,
    pub source_in: Time,
    pub source_out: Time,
    pub start: Time,
    pub link: Option<LinkId>,
}

/// A source range to put on the timeline at `start`, as linked video and audio clips.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub asset: AssetId,
    pub source_in: Time,
    pub source_out: Time,
    pub start: Time,
    pub video: Option<TrackId>,
    /// The track for the first audio stream; the rest go on the audio tracks below it.
    pub audio: Option<TrackId>,
    /// One audio clip per entry: its stream (`None` mixes all) and the name of a track made
    /// for it when there's none below to land on.
    pub streams: Vec<(Option<u32>, String)>,
    /// Make a new track for `video` or `audio` when it's `None`, for a file with no track of
    /// that kind to land on.
    pub add_video: bool,
    pub add_audio: bool,
    /// Insert pushes every unlocked track right to make room; overwrite replaces what's there.
    pub insert: bool,
}

fn is_first(id: &SequenceId) -> bool {
    id.0 == 0
}

impl Sequence {
    pub fn id(&self) -> SequenceId {
        self.id
    }

    fn alloc(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }

    pub fn in_point(&self) -> Option<Time> {
        self.in_point
    }

    pub fn out_point(&self) -> Option<Time> {
        self.out_point
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// Track settings (name, flags, volume, pan) are free to edit; clips are not.
    pub(crate) fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// A new video track goes on top of the stack, a new audio track at the bottom.
    pub(crate) fn add_track(&mut self, kind: TrackKind, name: impl Into<String>) -> TrackId {
        let id = TrackId(self.alloc());
        let at = match kind {
            TrackKind::Video => 0,
            TrackKind::Audio => self.tracks.len(),
        };
        self.tracks.insert(
            at,
            Track {
                id,
                kind,
                name: name.into(),
                hidden: false,
                locked: false,
                muted: false,
                solo: false,
                volume: 1.0,
                pan: 0.0,
                height: Track::default_height(),
                target: false,
                subtitles: false,
                fx: AudioFx::default(),
                clips: Vec::new(),
            },
        );
        id
    }

    /// Replaces a track's settings, keeping one target per kind.
    pub(crate) fn set_track(&mut self, s: TrackSettings) -> Result<(), ModelError> {
        let t = self.track_mut(s.id).ok_or(ModelError::UnknownTrack(s.id))?;
        let kind = t.kind;
        t.name = s.name;
        t.hidden = s.hidden;
        t.locked = s.locked;
        t.muted = s.muted;
        t.solo = s.solo;
        t.volume = s.volume;
        t.pan = s.pan;
        t.height = s.height.clamp(Track::MIN_HEIGHT, Track::MAX_HEIGHT);
        t.target = s.target;
        t.fx = s.fx;
        if s.target {
            for o in self
                .tracks
                .iter_mut()
                .filter(|o| o.kind == kind && o.id != s.id)
            {
                o.target = false;
            }
        }
        Ok(())
    }

    /// Moves a track to `index` among the tracks of its kind (0 = top).
    pub(crate) fn move_track(&mut self, id: TrackId, index: usize) -> Result<(), ModelError> {
        let t = self.remove_track(id)?;
        let first = self.tracks.iter().position(|o| o.kind == t.kind);
        let count = self.tracks.iter().filter(|o| o.kind == t.kind).count();
        let first = first.unwrap_or(match t.kind {
            TrackKind::Video => 0,
            TrackKind::Audio => self.tracks.len(),
        });
        self.tracks.insert(first + index.min(count), t);
        Ok(())
    }

    /// Removes the track and its clips.
    pub(crate) fn remove_track(&mut self, id: TrackId) -> Result<Track, ModelError> {
        let i = self
            .tracks
            .iter()
            .position(|t| t.id == id)
            .ok_or(ModelError::UnknownTrack(id))?;
        Ok(self.tracks.remove(i))
    }

    pub(crate) fn new_link(&mut self) -> LinkId {
        LinkId(self.alloc())
    }

    pub(crate) fn add_clip(&mut self, track: TrackId, new: NewClip) -> Result<ClipId, ModelError> {
        if new.source_out <= new.source_in {
            return Err(ModelError::EmptyClip);
        }
        let end = new.start + (new.source_out - new.source_in);
        self.check_free(track, new.start, end, None)?;
        let id = ClipId(self.alloc());
        let t = self.track_mut(track).expect("checked above");
        insert_sorted(
            &mut t.clips,
            Clip {
                id,
                asset: new.asset,
                source_in: new.source_in,
                source_out: new.source_out,
                start: new.start,
                link: new.link,
                params: BTreeMap::new(),
                keyframes: BTreeMap::new(),
                transform: Transform::default(),
                stream: Some(0),
                enabled: true,
                speed: 1.0,
                reverse: false,
                freeze: None,
            },
        );
        Ok(id)
    }

    pub fn clip(&self, id: ClipId) -> Option<(TrackId, &Clip)> {
        self.tracks
            .iter()
            .find_map(|t| t.clips.iter().find(|c| c.id == id).map(|c| (t.id, c)))
    }

    /// Fields that cannot break the overlap invariant: asset, link, params.
    pub(crate) fn with_clip_mut<R>(
        &mut self,
        id: ClipId,
        f: impl FnOnce(&mut ClipProps<'_>) -> R,
    ) -> Result<R, ModelError> {
        let c = self
            .tracks
            .iter_mut()
            .find_map(|t| t.clips.iter_mut().find(|c| c.id == id))
            .ok_or(ModelError::UnknownClip(id))?;
        Ok(f(&mut ClipProps {
            asset: &mut c.asset,
            link: &mut c.link,
            params: &mut c.params,
            keyframes: &mut c.keyframes,
            transform: &mut c.transform,
            stream: &mut c.stream,
            enabled: &mut c.enabled,
        }))
    }

    pub(crate) fn remove_clip(&mut self, id: ClipId) -> Result<(TrackId, Clip), ModelError> {
        for t in &mut self.tracks {
            if let Some(i) = t.clips.iter().position(|c| c.id == id) {
                return Ok((t.id, t.clips.remove(i)));
            }
        }
        Err(ModelError::UnknownClip(id))
    }

    /// Moves a clip to `start` on `track` (which may be its current track).
    pub(crate) fn move_clip(
        &mut self,
        id: ClipId,
        track: TrackId,
        start: Time,
    ) -> Result<(), ModelError> {
        let (_, c) = self.clip(id).ok_or(ModelError::UnknownClip(id))?;
        let end = start + c.duration();
        self.check_free(track, start, end, Some(id))?;
        let (_, mut c) = self.remove_clip(id)?;
        c.start = start;
        insert_sorted(&mut self.track_mut(track).expect("checked").clips, c);
        Ok(())
    }

    /// Trims a clip: new source range and timeline start.
    pub(crate) fn set_clip_range(
        &mut self,
        id: ClipId,
        start: Time,
        source_in: Time,
        source_out: Time,
    ) -> Result<(), ModelError> {
        if source_out <= source_in {
            return Err(ModelError::EmptyClip);
        }
        let (track, c) = self.clip(id).ok_or(ModelError::UnknownClip(id))?;
        let len = c.timeline_len(source_out - source_in);
        self.check_free(track, start, start + len, Some(id))?;
        let (_, mut c) = self.remove_clip(id)?;
        c.start = start;
        c.source_in = source_in;
        c.source_out = source_out;
        insert_sorted(&mut self.track_mut(track).expect("checked").clips, c);
        Ok(())
    }

    fn check_free(
        &self,
        track: TrackId,
        start: Time,
        end: Time,
        ignore: Option<ClipId>,
    ) -> Result<(), ModelError> {
        let t = self.track(track).ok_or(ModelError::UnknownTrack(track))?;
        match t.overlap(start, end, ignore) {
            Some(other) => Err(ModelError::Overlap(other)),
            None => Ok(()),
        }
    }

    pub(crate) fn add_marker(
        &mut self,
        time: Time,
        duration: Time,
        name: impl Into<String>,
    ) -> MarkerId {
        let id = MarkerId(self.alloc());
        self.markers.push(Marker {
            id,
            time,
            duration,
            name: name.into(),
            color: 0,
            note: String::new(),
        });
        id
    }

    /// Replaces the marker with `m`'s id.
    pub(crate) fn set_marker(&mut self, m: Marker) -> Option<()> {
        let id = m.id;
        *self.markers.iter_mut().find(|o| o.id == id)? = m;
        Some(())
    }

    pub(crate) fn remove_marker(&mut self, id: MarkerId) -> Option<Marker> {
        let i = self.markers.iter().position(|m| m.id == id)?;
        Some(self.markers.remove(i))
    }

    /// Clips covering `t` (start inclusive, end exclusive), in track order.
    pub fn clips_at(&self, t: Time) -> impl Iterator<Item = (TrackId, &Clip)> {
        self.clips_in(t, t + Time(1))
    }

    /// Clips intersecting the half-open range `[start, end)`, in track order.
    pub fn clips_in(&self, start: Time, end: Time) -> impl Iterator<Item = (TrackId, &Clip)> {
        self.tracks.iter().flat_map(move |tr| {
            tr.clips
                .iter()
                .filter(move |c| start < end && c.start < end && start < c.end())
                .map(move |c| (tr.id, c))
        })
    }

    fn edit_points(&self) -> impl Iterator<Item = Time> + '_ {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter().flat_map(|c| [c.start, c.end()]))
    }

    /// The first clip start or end strictly after `t`.
    pub fn next_edit(&self, t: Time) -> Option<Time> {
        self.edit_points().filter(|&p| p > t).min()
    }

    /// The last clip start or end strictly before `t`.
    pub fn previous_edit(&self, t: Time) -> Option<Time> {
        self.edit_points().filter(|&p| p < t).max()
    }

    /// End of the last clip, or zero when empty.
    pub fn duration(&self) -> Time {
        self.edit_points().max().unwrap_or_default()
    }
}

impl Sequence {
    fn index(&self, id: TrackId) -> Result<usize, ModelError> {
        let i = self.tracks.iter().position(|t| t.id == id);
        i.ok_or(ModelError::UnknownTrack(id))
    }

    fn kind_check(&self, id: TrackId, kind: TrackKind) -> Result<usize, ModelError> {
        let i = self.index(id)?;
        if self.tracks[i].kind != kind {
            return Err(ModelError::WrongKind(id));
        }
        Ok(i)
    }

    /// Cuts clip `id` on track `ti` at `at`. The tail gets a new id and, through `links`,
    /// a new link shared with the other tails of the same edit.
    fn split(&mut self, ti: usize, id: ClipId, at: Time, links: &mut BTreeMap<LinkId, LinkId>) {
        let tail_id = ClipId(self.alloc());
        let link = self.tracks[ti]
            .clips
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| c.link);
        let link = link.map(|l| *links.entry(l).or_insert_with(|| LinkId(self.alloc())));
        let clips = &mut self.tracks[ti].clips;
        let i = clips
            .iter()
            .position(|c| c.id == id)
            .expect("caller found it");
        let head = &mut clips[i];
        let mut tail = head.clone();
        tail.keep_tail(head.end() - at);
        head.keep_head(at - head.start);
        // The cut's transition stays with the tail, which now ends there.
        head.transform.transition = None;
        (tail.id, tail.link) = (tail_id, link);
        clips.insert(i + 1, tail);
    }

    /// Empties `[start, end)` on track `ti`, trimming or splitting clips that cross it.
    fn clear(&mut self, ti: usize, start: Time, end: Time, links: &mut BTreeMap<LinkId, LinkId>) {
        let hit = self.tracks[ti]
            .clips
            .iter()
            .filter(|c| c.start < end && start < c.end());
        let hit: Vec<_> = hit.map(|c| (c.id, c.start, c.end())).collect();
        for (id, s, e) in hit {
            if s < start && e > end {
                self.split(ti, id, end, links);
            }
            let clips = &mut self.tracks[ti].clips;
            let i = clips.iter().position(|c| c.id == id).expect("listed above");
            let c = &mut clips[i];
            if s >= start && e <= end {
                clips.remove(i);
            } else if s < start {
                c.keep_head(start - s);
            } else {
                c.keep_tail(e - end);
            }
        }
    }

    /// Insert or overwrite edit, as Premiere's `,` and `.`: insert splits every unlocked
    /// track at `start` and pushes the rest right by the clip's length.
    pub(crate) fn place(&mut self, p: Placement) -> Result<Vec<ClipId>, ModelError> {
        if p.source_out <= p.source_in {
            return Err(ModelError::EmptyClip);
        }
        let len = p.source_out - p.source_in;
        let mut s = self.clone();
        let mut links = BTreeMap::new();
        let mut targets = Vec::new();
        match p.video {
            None if p.add_video => {
                targets.push((s.add_track(TrackKind::Video, ""), TrackKind::Video, Some(0)))
            }
            Some(t) => targets.push((t, TrackKind::Video, Some(0))),
            None => {}
        }
        let mut audio = p.audio;
        for (i, (stream, name)) in p.streams.iter().enumerate() {
            audio = match audio {
                None if i == 0 && p.add_audio => Some(s.add_track(TrackKind::Audio, name.clone())),
                None => break,
                Some(a) if i == 0 => Some(a),
                // The next unlocked audio track below the previous stream's, else a new one.
                Some(a) => {
                    let at = s.index(a)?;
                    let below = s.tracks[at + 1..]
                        .iter()
                        .find(|t| t.kind == TrackKind::Audio && !t.locked)
                        .map(|t| t.id);
                    Some(below.unwrap_or_else(|| s.add_track(TrackKind::Audio, name.clone())))
                }
            };
            targets.push((audio.expect("set above"), TrackKind::Audio, *stream));
        }
        let mut tracks = Vec::new();
        for (t, kind, stream) in targets {
            tracks.push((t, s.kind_check(t, kind)?, stream));
        }
        if p.insert {
            for ti in 0..s.tracks.len() {
                if s.tracks[ti].locked {
                    continue;
                }
                let crossing = s.tracks[ti].overlap(p.start, p.start, None);
                if let Some(id) = crossing {
                    s.split(ti, id, p.start, &mut links);
                }
                for c in &mut s.tracks[ti].clips {
                    if c.start >= p.start {
                        c.start = c.start + len;
                    }
                }
            }
        }
        let link = (tracks.len() > 1).then(|| s.new_link());
        let mut ids = Vec::new();
        for (t, ti, stream) in tracks {
            s.clear(ti, p.start, p.start + len, &mut links);
            let clip = NewClip {
                asset: p.asset,
                source_in: p.source_in,
                source_out: p.source_out,
                start: p.start,
                link,
            };
            let id = s.add_clip(t, clip)?;
            s.with_clip_mut(id, |c| *c.stream = stream)?;
            ids.push(id);
        }
        *self = s;
        Ok(ids)
    }

    /// The clip and its linked partners.
    pub fn linked(&self, id: ClipId) -> Vec<(TrackId, &Clip)> {
        let link = self.clip(id).and_then(|(_, c)| c.link);
        let all = self
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| (t.id, c)));
        all.filter(|(_, c)| c.id == id || (link.is_some() && c.link == link))
            .collect()
    }

    /// Drags clip `id` to `start` on `track`, its linked clips by the same time on their own
    /// tracks, overwriting whatever they land on. Nothing moves before zero.
    pub(crate) fn drag_clip(
        &mut self,
        id: ClipId,
        track: TrackId,
        start: Time,
    ) -> Result<(), ModelError> {
        let (from, c) = self.clip(id).ok_or(ModelError::UnknownClip(id))?;
        let kind = self.tracks[self.index(from)?].kind;
        self.kind_check(track, kind)?;
        let group: Vec<_> = self
            .linked(id)
            .iter()
            .map(|(t, c)| (*t, c.id, c.start))
            .collect();
        let earliest = group.iter().map(|g| g.2).min().expect("has the clip");
        let by = (start - c.start).max(Time(0) - earliest);
        let mut s = self.clone();
        let mut moved = Vec::new();
        for (t, cid, _) in group {
            let (_, c) = s.remove_clip(cid)?;
            moved.push((if cid == id { track } else { t }, c));
        }
        let mut links = BTreeMap::new();
        for (t, mut c) in moved {
            c.start = c.start + by;
            let ti = s.index(t)?;
            s.clear(ti, c.start, c.end(), &mut links);
            insert_sorted(&mut s.tracks[ti].clips, c);
        }
        *self = s;
        Ok(())
    }
}

impl Sequence {
    /// Removes the clips. Ripple closes each gap by pulling later clips on its track left.
    pub(crate) fn remove_clips(&mut self, ids: &[ClipId], ripple: bool) -> Result<(), ModelError> {
        let mut s = self.clone();
        let mut gone = Vec::new();
        for &id in ids {
            gone.push(s.remove_clip(id)?);
        }
        if ripple {
            gone.sort_by_key(|(_, c)| std::cmp::Reverse(c.start));
            for (t, c) in gone {
                let ti = s.index(t)?;
                for later in &mut s.tracks[ti].clips {
                    if later.start >= c.start {
                        later.start = later.start - c.duration();
                    }
                }
            }
        }
        *self = s;
        Ok(())
    }

    /// Overwrites copies of `clips` onto their tracks, the earliest starting at `at`. Copies
    /// get new ids, and new links shared the way the originals shared theirs.
    pub(crate) fn paste(
        &mut self,
        clips: &[(TrackId, Clip)],
        at: Time,
    ) -> Result<Vec<ClipId>, ModelError> {
        let Some(earliest) = clips.iter().map(|(_, c)| c.start).min() else {
            return Ok(Vec::new());
        };
        let mut s = self.clone();
        let mut links = BTreeMap::new();
        let mut fresh = BTreeMap::new();
        let mut ids = Vec::new();
        for (t, c) in clips {
            let ti = s.index(*t)?;
            let mut c = c.clone();
            c.id = ClipId(s.alloc());
            c.start = c.start + (at - earliest);
            c.link = c
                .link
                .map(|l| *fresh.entry(l).or_insert_with(|| s.new_link()));
            s.clear(ti, c.start, c.end(), &mut links);
            ids.push(c.id);
            insert_sorted(&mut s.tracks[ti].clips, c);
        }
        *self = s;
        Ok(ids)
    }

    /// Puts the clips in one new link group, or takes them out of theirs.
    pub(crate) fn link_clips(&mut self, ids: &[ClipId], linked: bool) -> Result<(), ModelError> {
        let mut s = self.clone();
        let link = linked.then(|| s.new_link());
        for &id in ids {
            s.with_clip_mut(id, |c| *c.link = link)?;
        }
        *self = s;
        Ok(())
    }
}

/// The clip fields `Sequence::with_clip_mut` exposes.
pub(crate) struct ClipProps<'a> {
    pub asset: &'a mut AssetId,
    pub link: &'a mut Option<LinkId>,
    pub params: &'a mut BTreeMap<String, f64>,
    pub keyframes: &'a mut BTreeMap<String, Vec<Keyframe>>,
    pub transform: &'a mut Transform,
    pub stream: &'a mut Option<u32>,
    pub enabled: &'a mut bool,
}

fn insert_sorted(clips: &mut Vec<Clip>, c: Clip) {
    let i = clips.partition_point(|x| x.start < c.start);
    clips.insert(i, c);
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Project {
    pub(crate) settings: ProjectSettings,
    pub(crate) assets: Vec<MediaAsset>,
    /// The open sequence: the one the Timeline shows and edits change.
    pub(crate) sequence: Sequence,
    /// The other sequences; opening one swaps it with `sequence`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) sequences: Vec<Sequence>,
    next_asset: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    next_sequence: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Read-only from outside the crate; edit it through [`crate::history::History`].
impl Project {
    pub fn settings(&self) -> &ProjectSettings {
        &self.settings
    }

    pub fn assets(&self) -> &[MediaAsset] {
        &self.assets
    }

    /// The open sequence.
    pub fn sequence(&self) -> &Sequence {
        &self.sequence
    }

    /// Every sequence, the open one included, oldest first.
    pub fn sequences(&self) -> Vec<&Sequence> {
        let mut all: Vec<_> = std::iter::once(&self.sequence)
            .chain(&self.sequences)
            .collect();
        all.sort_by_key(|s| s.id);
        all
    }

    pub fn find_sequence(&self, id: SequenceId) -> Option<&Sequence> {
        std::iter::once(&self.sequence)
            .chain(&self.sequences)
            .find(|s| s.id == id)
    }

    /// The sequence a sequence asset's clips play.
    pub fn nested(&self, asset: AssetId) -> Option<&Sequence> {
        self.find_sequence(self.asset(asset)?.sequence?)
    }

    /// The bin entry for sequence `id`; a project's only sequence has none.
    pub fn sequence_asset(&self, id: SequenceId) -> Option<&MediaAsset> {
        self.assets.iter().find(|a| a.sequence == Some(id))
    }

    /// Whether sequence `from` is `to` or holds a clip of it, however deep.
    pub fn nests(&self, from: SequenceId, to: SequenceId) -> bool {
        let (mut stack, mut seen) = (vec![from], std::collections::BTreeSet::new());
        while let Some(id) = stack.pop() {
            if id == to {
                return true;
            }
            if let Some(s) = self.find_sequence(id).filter(|_| seen.insert(id)) {
                let clips = s.tracks.iter().flat_map(|t| &t.clips);
                stack.extend(clips.filter_map(|c| self.asset(c.asset)?.sequence));
            }
        }
        false
    }

    /// Whether a clip of `asset` in the open sequence would nest that sequence inside itself.
    pub fn would_cycle(&self, asset: AssetId) -> bool {
        self.nested(asset)
            .is_some_and(|n| self.nests(n.id, self.sequence.id))
    }

    /// Whether some sequence holds itself, which edits refuse; only a damaged file has one.
    pub fn has_cycle(&self) -> bool {
        self.sequences().into_iter().any(|s| {
            let mut clips = s.tracks.iter().flat_map(|t| &t.clips);
            clips.any(|c| self.nested(c.asset).is_some_and(|n| self.nests(n.id, s.id)))
        })
    }

    /// Opens sequence `id` in the Timeline.
    pub(crate) fn open(&mut self, id: SequenceId) -> Result<(), ModelError> {
        if self.sequence.id != id {
            let i = self.sequences.iter().position(|s| s.id == id);
            let i = i.ok_or(ModelError::UnknownSequence(id))?;
            std::mem::swap(&mut self.sequence, &mut self.sequences[i]);
        }
        Ok(())
    }

    /// Nest: moves clips `ids` of the open sequence into a new sequence named `name` and puts
    /// one clip of it where they were, on the top track of each kind they were on, linked.
    /// Their tracks come along in order, the earliest clip starting at zero; an unchosen clip
    /// in the way fails it. `outer` names the open sequence in the bin if it isn't there yet.
    pub(crate) fn nest(
        &mut self,
        ids: &[ClipId],
        name: String,
        outer: String,
    ) -> Result<ClipId, ModelError> {
        let mut p = self.clone();
        let s = &p.sequence;
        let mut picked = Vec::new();
        for &id in ids {
            picked.push(s.clip(id).ok_or(ModelError::UnknownClip(id))?.1);
        }
        let start = picked.iter().map(|c| c.start).min();
        let start = start.ok_or(ModelError::EmptyClip)?;
        let end = picked.iter().map(|c| c.end()).max().expect("not empty");
        p.next_sequence += 1;
        let mut inner = Sequence {
            id: SequenceId(p.next_sequence),
            next_id: s.next_id,
            ..Sequence::default()
        };
        for t in &s.tracks {
            let clips = t.clips.iter().filter(|c| ids.contains(&c.id)).cloned();
            let clips: Vec<_> = clips
                .map(|c| Clip {
                    start: c.start - start,
                    ..c
                })
                .collect();
            // Mixed again on the outer track, so the inner one starts at unity.
            if !clips.is_empty() {
                inner.tracks.push(Track {
                    id: t.id,
                    kind: t.kind,
                    name: t.name.clone(),
                    hidden: t.hidden,
                    locked: false,
                    muted: t.muted,
                    solo: false,
                    volume: 1.0,
                    pan: 0.0,
                    height: t.height,
                    target: false,
                    subtitles: t.subtitles,
                    clips,
                });
            }
        }
        let top = |kind| inner.tracks.iter().find(|t| t.kind == kind).map(|t| t.id);
        let tops = [top(TrackKind::Video), top(TrackKind::Audio)];
        p.sequence.remove_clips(ids, false)?;
        for (id, name) in [(inner.id, name), (p.sequence.id, outer)] {
            if p.sequence_asset(id).is_none() {
                let a = p.add_asset(PathBuf::new());
                let a = p.asset_mut(a).expect("just added");
                (a.name, a.sequence) = (Some(name), Some(id));
            }
        }
        let asset = p.sequence_asset(inner.id).expect("added above").id;
        let link = tops
            .iter()
            .all(Option::is_some)
            .then(|| p.sequence.new_link());
        let mut made = Vec::new();
        for t in tops.into_iter().flatten() {
            let clip = NewClip {
                asset,
                source_in: Time(0),
                source_out: end - start,
                start,
                link,
            };
            made.push(p.sequence.add_clip(t, clip)?);
        }
        p.sequences.push(inner);
        *self = p;
        Ok(made[0])
    }

    /// Removes the asset and every clip of it in every sequence; a sequence asset takes its
    /// sequence with it, unless that's the open one.
    pub(crate) fn remove_asset(&mut self, id: AssetId) -> Result<(), ModelError> {
        let i = self.assets.iter().position(|a| a.id == id);
        let i = i.ok_or(ModelError::UnknownAsset(id))?;
        if let Some(s) = self.assets[i].sequence {
            if s == self.sequence.id {
                return Err(ModelError::SequenceOpen);
            }
            self.sequences.retain(|o| o.id != s);
        }
        self.assets.remove(i);
        for s in std::iter::once(&mut self.sequence).chain(&mut self.sequences) {
            for t in &mut s.tracks {
                t.clips.retain(|c| c.asset != id);
            }
        }
        Ok(())
    }

    pub(crate) fn add_asset(&mut self, path: impl Into<PathBuf>) -> AssetId {
        self.next_asset += 1;
        let id = AssetId(self.next_asset);
        self.assets.push(MediaAsset {
            id,
            path: path.into(),
            probe: None,
            name: None,
            in_point: None,
            out_point: None,
            solid: None,
            text: None,
            sequence: None,
        });
        id
    }

    pub fn asset(&self, id: AssetId) -> Option<&MediaAsset> {
        self.assets.iter().find(|a| a.id == id)
    }

    /// How many clips use the asset, in every sequence; removing it removes them too.
    pub fn clips_using(&self, id: AssetId) -> usize {
        let tracks = self.sequences().into_iter().flat_map(|s| &s.tracks);
        let clips = tracks.flat_map(|t| &t.clips);
        clips.filter(|c| c.asset == id).count()
    }

    /// Places `asset`'s source in/out (default the whole file) at `start`: on `on` for its
    /// kind, and on the targeted track, else the top unlocked one, for the other kind the
    /// file has, adding a track of a kind the sequence lacks. Each audio stream gets its own
    /// clip and track unless `merge` mixes them into one. `None` when the file has no duration.
    pub fn placement(
        &self,
        asset: AssetId,
        start: Time,
        insert: bool,
        on: Option<TrackId>,
        merge: bool,
    ) -> Option<Placement> {
        let a = self.asset(asset)?;
        // Text and solids have no file to probe: a video-only source of the default still length.
        // A sequence is as long as it is, with each kind it has clips of.
        let generated = a.generated().then(|| match self.nested(asset) {
            Some(s) => {
                let has = |k| s.tracks.iter().any(|t| t.kind == k && !t.clips.is_empty());
                ProbeInfo {
                    duration: s.duration(),
                    width: has(TrackKind::Video).then_some(1),
                    audio_streams: has(TrackKind::Audio).into(),
                    ..ProbeInfo::default()
                }
            }
            None => ProbeInfo {
                duration: Time(
                    crate::time::FLICKS_PER_SECOND * i64::from(crate::settings::DEFAULT_STILL_SECS),
                ),
                width: Some(1),
                ..ProbeInfo::default()
            },
        });
        let probe = a.probe.as_ref().or(generated.as_ref())?;
        let source_in = a.in_point.unwrap_or_default();
        let source_out = a.out_point.unwrap_or(probe.duration);
        if source_out <= source_in {
            return None;
        }
        let tracks = self.sequence.tracks();
        let pick = |kind: TrackKind, has: bool| {
            let of_kind = || tracks.iter().filter(move |t| t.kind == kind);
            has.then(|| {
                of_kind()
                    .find(|t| Some(t.id) == on)
                    .or_else(|| of_kind().find(|t| t.target))
                    .or_else(|| of_kind().find(|t| !t.locked))
                    .map(|t| t.id)
            })
            .flatten()
        };
        let (has_video, has_audio) = (probe.width.is_some(), probe.audio_streams > 0);
        let video = pick(TrackKind::Video, has_video);
        let audio = pick(TrackKind::Audio, has_audio);
        let lacks = |kind| !tracks.iter().any(|t| t.kind == kind);
        let add_video = has_video && lacks(TrackKind::Video);
        let add_audio = has_audio && lacks(TrackKind::Audio);
        (video.is_some() || audio.is_some() || add_video || add_audio).then_some(Placement {
            asset,
            source_in,
            source_out,
            start: start.max(Time(0)),
            video,
            audio,
            streams: audio_streams(probe, merge),
            add_video,
            add_audio,
            insert,
        })
    }

    /// The enabled text clips on subtitle tracks as cues, sorted by start.
    pub fn subtitle_cues(&self) -> Vec<crate::srt::Cue> {
        let tracks = self.sequence.tracks.iter().filter(|t| t.subtitles);
        let mut cues: Vec<_> = tracks
            .flat_map(|t| &t.clips)
            .filter(|c| c.enabled)
            .filter_map(|c| {
                let text = self.asset(c.asset)?.text.as_ref()?;
                Some(crate::srt::Cue {
                    start: c.start,
                    end: c.end(),
                    text: text.content.clone(),
                })
            })
            .collect();
        cues.sort_by_key(|c| c.start);
        cues
    }

    /// A copy with subtitle tracks hidden, for exporting them as a sidecar instead of burnt in.
    pub fn without_subtitles(&self) -> Project {
        let mut p = self.clone();
        for t in p.sequence.tracks.iter_mut().filter(|t| t.subtitles) {
            t.hidden = true;
        }
        p
    }

    pub(crate) fn asset_mut(&mut self, id: AssetId) -> Option<&mut MediaAsset> {
        self.assets.iter_mut().find(|a| a.id == id)
    }
}

/// Each audio clip a placement makes: its stream and a name, the stream's title or "Audio n".
fn audio_streams(probe: &ProbeInfo, merge: bool) -> Vec<(Option<u32>, String)> {
    if merge || probe.audio_streams < 2 {
        return (probe.audio_streams > 0)
            .then(|| (if merge { None } else { Some(0) }, String::new()))
            .into_iter()
            .collect();
    }
    (0..probe.audio_streams)
        .map(|n| (Some(n), probe.audio_name(n)))
        .collect()
}

/// Which end of a clip a trim moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Head,
    Tail,
}

impl Sequence {
    /// Cuts each listed clip that crosses `at`; tails of linked clips share a new link.
    pub(crate) fn split_at(&mut self, ids: &[ClipId], at: Time) -> Result<(), ModelError> {
        let mut s = self.clone();
        let mut links = BTreeMap::new();
        for &id in ids {
            let (t, c) = s.clip(id).ok_or(ModelError::UnknownClip(id))?;
            if c.start < at && at < c.end() {
                let ti = s.index(t)?;
                s.split(ti, id, at, &mut links);
            }
        }
        // A tail whose partner wasn't cut has nothing to be linked to.
        for l in links.into_values() {
            let tails = s
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .filter(|c| c.link == Some(l));
            if tails.count() == 1 {
                let t = s.tracks.iter_mut().flat_map(|t| &mut t.clips);
                t.filter(|c| c.link == Some(l)).for_each(|c| c.link = None);
            }
        }
        *self = s;
        Ok(())
    }

    /// Sets the speed and direction of `id` and its linked clips, each keeping its source
    /// range so its length follows the speed. Ripple shifts the later clips on their tracks
    /// by the change in length; without it a longer clip must fit before the next.
    pub(crate) fn set_speed(
        &mut self,
        id: ClipId,
        speed: f64,
        reverse: bool,
        ripple: bool,
    ) -> Result<(), ModelError> {
        if !(speed > 0.0 && speed.is_finite()) {
            return Err(ModelError::BadSpeed);
        }
        let (_, c) = self.clip(id).ok_or(ModelError::UnknownClip(id))?;
        let old = c.duration();
        let mut s = self.clone();
        let group: Vec<_> = s
            .linked(id)
            .iter()
            .map(|(t, c)| (*t, c.id, c.end()))
            .collect();
        let mut later = Vec::new();
        let mut grow = Time(0);
        for (t, cid, end) in group {
            let ti = s.index(t)?;
            for c in &mut s.tracks[ti].clips {
                if c.id == cid {
                    (c.speed, c.reverse) = (speed, reverse);
                    if cid == id {
                        grow = c.duration() - old;
                    }
                } else if ripple && c.start >= end {
                    later.push(c.id);
                }
            }
        }
        s.shift_synced(later, grow)?;
        *self = s;
        Ok(())
    }

    /// Cuts each `(clip, start, end)` timeline span out of its clip and linked clips with a
    /// split at both ends, then ripple deletes the middle or, with `speed`, sets its speed
    /// with ripple. Spans are clamped to their clip; empty ones are skipped.
    pub(crate) fn cut_spans(
        &mut self,
        mut spans: Vec<(ClipId, Time, Time)>,
        speed: Option<f64>,
    ) -> Result<(), ModelError> {
        let mut s = self.clone();
        // Latest first, so no edit moves a span still to come.
        spans.sort_by_key(|&(_, a, _)| std::cmp::Reverse(a));
        for (id, a, b) in spans {
            let (t, c) = s.clip(id).ok_or(ModelError::UnknownClip(id))?;
            let (a, b) = (a.max(c.start), b.min(c.end()));
            if a >= b {
                continue;
            }
            let group: Vec<_> = s.linked(id).iter().map(|(_, c)| c.id).collect();
            s.split_at(&group, b)?;
            s.split_at(&group, a)?;
            // The head keeps `id`, so the middle is whatever now starts at `a`.
            let mid = s
                .track(t)
                .and_then(|t| t.clips.iter().find(|c| c.start == a));
            let mid = mid.ok_or(ModelError::UnknownClip(id))?;
            match speed {
                None => {
                    let ids: Vec<_> = s.linked(mid.id).iter().map(|(_, c)| c.id).collect();
                    s.remove_clips(&ids, true)?;
                }
                Some(v) => s.set_speed(mid.id, v, mid.reverse, true)?,
            }
        }
        *self = s;
        Ok(())
    }

    /// Holds the frame `id` shows at `at` for the rest of the clip, cutting it there first.
    /// The held part keeps its length and drops its link, as it no longer plays in sync.
    pub(crate) fn freeze_frame(&mut self, id: ClipId, at: Time) -> Result<(), ModelError> {
        let (t, c) = self.clip(id).ok_or(ModelError::UnknownClip(id))?;
        if !(c.start <= at && at < c.end()) {
            return Err(ModelError::OutsideClip(id));
        }
        let frame = c.source_at(at);
        let cut = c.start < at;
        let mut s = self.clone();
        let ti = s.index(t)?;
        if cut {
            s.split(ti, id, at, &mut BTreeMap::new());
        }
        let clips = &mut s.tracks[ti].clips;
        let i = clips.iter().position(|c| c.id == id).expect("found above") + cut as usize;
        let tail = &mut clips[i];
        let len = tail.duration();
        tail.freeze = Some(frame);
        tail.keep_head(len);
        if cut {
            tail.link = None;
        }
        *self = s;
        Ok(())
    }

    /// The empty span on `track` containing `at`, from the previous clip's end (or zero) to
    /// the next clip's start. `None` inside a clip or past the last one.
    pub fn gap_at(&self, track: TrackId, at: Time) -> Option<(Time, Time)> {
        let clips = self.track(track)?.clips();
        let next = clips.iter().find(|c| c.end() > at)?;
        if next.start <= at || at < Time(0) {
            return None;
        }
        let prev = clips.iter().rev().find(|c| c.end() <= at);
        Some((prev.map_or(Time(0), Clip::end), next.start))
    }

    /// Lift (`;`), or with `ripple` extract (`'`): empties the in/out range on every unlocked
    /// track, pulling later clips left when rippling, then clears the marks. A missing mark
    /// stands for the sequence's start or end.
    pub(crate) fn remove_range(&mut self, ripple: bool) -> Result<(), ModelError> {
        if self.in_point.is_none() && self.out_point.is_none() {
            return Err(ModelError::NoRange);
        }
        let start = self.in_point.unwrap_or_default();
        let end = self.out_point.unwrap_or(self.duration());
        if end <= start {
            return Err(ModelError::NoRange);
        }
        let mut links = BTreeMap::new();
        for ti in 0..self.tracks.len() {
            if self.tracks[ti].locked {
                continue;
            }
            self.clear(ti, start, end, &mut links);
            if ripple {
                for c in self.tracks[ti].clips.iter_mut().filter(|c| c.start >= end) {
                    c.start = c.start - (end - start);
                }
            }
        }
        (self.in_point, self.out_point) = (None, None);
        Ok(())
    }

    /// Ripple deletes the gap containing `at`: later clips on the track move left to close it.
    pub(crate) fn close_gap(&mut self, track: TrackId, at: Time) -> Result<(), ModelError> {
        let ti = self.index(track)?;
        let (a, b) = self.gap_at(track, at).ok_or(ModelError::NoGap(track))?;
        let later = self.tracks[ti].clips.iter().filter(|c| c.start >= b);
        let later: Vec<_> = later.map(|c| c.id).collect();
        let mut s = self.clone();
        s.shift_synced(later, a - b)?;
        *self = s;
        Ok(())
    }

    /// Moves `ids` and their linked partners on other tracks by `by`, so ripples keep sync.
    /// Fails, leaving `self` half-moved, if a moved clip would land on another or before zero.
    fn shift_synced(&mut self, mut ids: Vec<ClipId>, by: Time) -> Result<(), ModelError> {
        for id in ids.clone() {
            for (_, c) in self.linked(id) {
                if !ids.contains(&c.id) {
                    ids.push(c.id);
                }
            }
        }
        for t in &mut self.tracks {
            for c in t.clips.iter_mut().filter(|c| ids.contains(&c.id)) {
                c.start = c.start + by;
                if c.start < Time(0) {
                    return Err(ModelError::Overlap(c.id));
                }
            }
            t.clips.sort_by_key(|c| c.start);
            if let Some(w) = t.clips.windows(2).find(|w| w[0].end() > w[1].start) {
                let hit = if ids.contains(&w[0].id) {
                    w[1].id
                } else {
                    w[0].id
                };
                return Err(ModelError::Overlap(hit));
            }
        }
        Ok(())
    }
}

impl Project {
    /// How far `id`'s `edge`, with its linked clips', can move from `by`: held to the media,
    /// a one-frame minimum length and, without ripple, the neighbouring clips and zero.
    pub fn trim_limit(&self, id: ClipId, edge: Edge, by: Time, ripple: bool) -> Time {
        let s = &self.sequence;
        let min = self.settings.frame_rate.time_floor(1);
        let (mut lo, mut hi) = (Time(i64::MIN / 4), Time(i64::MAX / 4));
        for (t, c) in s.linked(id) {
            let clips = s.track(t).expect("listed").clips();
            // Stills and solids report no duration and extend freely, as does a freeze; a
            // sequence is held to its length.
            let media = self.asset(c.asset).filter(|a| !a.generated());
            let media = media.and_then(|a| a.probe.as_ref()).map(|p| p.duration);
            let media = media.or(self.nested(c.asset).map(Sequence::duration));
            let media = media.filter(|d| *d > Time(0)).map(|d| d - c.source_out);
            // Source to spare before the first frame shown and after the last.
            let (before, after) = match (c.freeze, c.reverse) {
                (Some(_), _) => (None, None),
                (None, false) => (Some(c.source_in), media),
                (None, true) => (media, Some(c.source_in)),
            };
            match edge {
                Edge::Head => {
                    if let Some(b) = before {
                        lo = lo.max(Time(0) - c.timeline_len(b));
                    }
                    hi = hi.min(c.duration() - min);
                    if !ripple {
                        let prev = clips.iter().map(Clip::end).filter(|&e| e <= c.start);
                        lo = lo.max(prev.max().unwrap_or(Time(0)) - c.start);
                    }
                }
                Edge::Tail => {
                    lo = lo.max(min - c.duration());
                    if let Some(a) = after {
                        hi = hi.min(c.timeline_len(a));
                    }
                    let next = clips.iter().map(|o| o.start).filter(|&o| o >= c.end());
                    if let (false, Some(n)) = (ripple, next.min()) {
                        hi = hi.min(n - c.end());
                    }
                }
            }
        }
        if lo > hi { Time(0) } else { by.clamp(lo, hi) }
    }

    /// The longest transition at the cut after `id` (the end it shares with the next clip),
    /// a whole even number of frames held to the media handles on both sides.
    pub fn transition_room(&self, id: ClipId) -> Result<Time, ModelError> {
        let s = &self.sequence;
        let (t, c) = s.clip(id).ok_or(ModelError::UnknownClip(id))?;
        let mut next = s.track(t).expect("listed").clips().iter();
        let next = next
            .find(|n| n.start == c.end())
            .ok_or(ModelError::NoHandles(id))?;
        let far = Time(i64::MAX / 8);
        let after = self.trim_limit(id, Edge::Tail, far, true);
        let before = Time(0) - self.trim_limit(next.id, Edge::Head, Time(0) - far, true);
        let r = self.settings.frame_rate;
        // Stills and solids have endless handles; a minute either side is plenty.
        let minute = Time(60 * crate::time::FLICKS_PER_SECOND);
        let half = r.index_floor(after.min(before).min(minute));
        if half < 1 {
            return Err(ModelError::NoHandles(id));
        }
        Ok(r.time_floor(2 * half))
    }

    /// Moves each clip's edge by its amount, as far as [`trim_limit`](Self::trim_limit)
    /// allows, its linked clips' with it. Ripple keeps the head in place and shifts the
    /// later clips on each trimmed track by the change in length.
    pub(crate) fn trim(
        &mut self,
        trims: &[(ClipId, Edge, Time)],
        ripple: bool,
    ) -> Result<(), ModelError> {
        let mut p = self.clone();
        let mut done = Vec::new();
        for &(id, edge, by) in trims {
            p.sequence.clip(id).ok_or(ModelError::UnknownClip(id))?;
            if done.contains(&id) {
                continue;
            }
            let by = p.trim_limit(id, edge, by, ripple);
            let group = p.sequence.linked(id).into_iter();
            let group: Vec<_> = group.map(|(t, c)| (t, c.id, c.end())).collect();
            let mut later = Vec::new();
            for (t, cid, end) in group {
                done.push(cid);
                let s = &mut p.sequence;
                let ti = s.index(t)?;
                for c in &mut s.tracks[ti].clips {
                    if c.id == cid {
                        match edge {
                            Edge::Head => {
                                let start = c.start;
                                c.keep_tail(c.duration() - by);
                                if ripple {
                                    c.start = start;
                                }
                            }
                            Edge::Tail => c.keep_head(c.duration() + by),
                        }
                    } else if ripple && c.start >= end {
                        later.push(c.id);
                    }
                }
            }
            let shift = match edge {
                Edge::Head => Time(0) - by,
                Edge::Tail => by,
            };
            p.sequence.shift_synced(later, shift)?;
        }
        *self = p;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(start: i64, len: i64) -> NewClip {
        NewClip {
            asset: AssetId(1),
            source_in: Time(100),
            source_out: Time(100 + len),
            start: Time(start),
            link: None,
        }
    }

    fn seq() -> (Sequence, TrackId, TrackId) {
        let mut s = Sequence::default();
        let v = s.add_track(TrackKind::Video, "V1");
        let a = s.add_track(TrackKind::Audio, "A1");
        (s, v, a)
    }

    fn ids<'a>(it: impl Iterator<Item = (TrackId, &'a Clip)>) -> Vec<ClipId> {
        it.map(|(_, c)| c.id).collect()
    }

    /// (start, source_in, source_out) of each clip on the track.
    fn spans(s: &Sequence, t: TrackId) -> Vec<(i64, i64, i64)> {
        let clips = s.track(t).unwrap().clips();
        clips
            .iter()
            .map(|c| (c.start.0, c.source_in.0, c.source_out.0))
            .collect()
    }

    fn placement(v: TrackId, a: TrackId, start: i64, insert: bool) -> Placement {
        Placement {
            asset: AssetId(2),
            source_in: Time(1000),
            source_out: Time(1010),
            start: Time(start),
            video: Some(v),
            audio: Some(a),
            streams: vec![(Some(0), String::new())],
            add_video: false,
            add_audio: false,
            insert,
        }
    }

    // Premiere, one clip at 0..30 on V1 and A1, source 1010..1020 placed at 10:
    // insert gives 0..10 | new 10..20 | tail 20..40; overwrite gives 0..10 | new | 20..30.
    #[test]
    fn insert_and_overwrite_match_premiere() {
        let (mut s, v, a) = seq();
        let l = s.new_link();
        for t in [v, a] {
            s.add_clip(
                t,
                NewClip {
                    link: Some(l),
                    ..clip(0, 30)
                },
            )
            .unwrap();
        }
        let mut ins = s.clone();
        let new = ins.place(placement(v, a, 10, true)).unwrap();
        for t in [v, a] {
            assert_eq!(
                spans(&ins, t),
                [(0, 100, 110), (10, 1000, 1010), (20, 110, 130)]
            );
        }
        // The two tails are linked to each other, not to the heads or the new clips.
        let tails: Vec<_> = [v, a].map(|t| ins.track(t).unwrap().clips()[2].link).into();
        assert_eq!(tails[0], tails[1]);
        assert_ne!(tails[0], Some(l));
        let new_link = ins.clip(new[0]).unwrap().1.link;
        assert!(new_link.is_some() && new_link == ins.clip(new[1]).unwrap().1.link);

        let mut ow = s.clone();
        ow.place(placement(v, a, 10, false)).unwrap();
        for t in [v, a] {
            assert_eq!(
                spans(&ow, t),
                [(0, 100, 110), (10, 1000, 1010), (20, 120, 130)]
            );
        }
        // Overwrite over a whole clip and part of the next.
        ow.place(Placement {
            start: Time(5),
            source_out: Time(1020),
            ..placement(v, a, 0, false)
        })
        .unwrap();
        assert_eq!(
            spans(&ow, v),
            [(0, 100, 105), (5, 1000, 1020), (25, 125, 130)]
        );
    }

    #[test]
    fn insert_skips_locked_tracks_and_checks_kind() {
        let (mut s, v, a) = seq();
        let a2 = s.add_track(TrackKind::Audio, "A2");
        s.add_clip(a2, clip(20, 10)).unwrap();
        s.track_mut(a2).unwrap().locked = true;
        s.add_clip(v, clip(20, 10)).unwrap();
        s.place(Placement {
            audio: None,
            ..placement(v, a, 0, true)
        })
        .unwrap();
        assert_eq!(spans(&s, v), [(0, 1000, 1010), (30, 100, 110)]);
        assert_eq!(spans(&s, a2), [(20, 100, 110)]);
        let wrong = Placement {
            video: Some(a),
            audio: None,
            ..placement(v, a, 0, false)
        };
        let before = s.clone();
        assert_eq!(s.place(wrong), Err(ModelError::WrongKind(a)));
        assert_eq!(s, before);
    }

    #[test]
    fn drag_moves_linked_clips_and_overwrites() {
        let (mut s, v, a) = seq();
        let v2 = s.add_track(TrackKind::Video, "V2");
        let l = s.new_link();
        let cv = s
            .add_clip(
                v,
                NewClip {
                    link: Some(l),
                    ..clip(0, 10)
                },
            )
            .unwrap();
        s.add_clip(
            a,
            NewClip {
                link: Some(l),
                ..clip(0, 10)
            },
        )
        .unwrap();
        s.add_clip(v2, clip(15, 10)).unwrap();
        s.add_clip(a, clip(15, 10)).unwrap();
        s.drag_clip(cv, v2, Time(10)).unwrap();
        assert_eq!(spans(&s, v), []);
        assert_eq!(spans(&s, v2), [(10, 100, 110), (20, 105, 110)]);
        assert_eq!(spans(&s, a), [(10, 100, 110), (20, 105, 110)]);
        // Never before zero; video can't land on audio.
        s.drag_clip(cv, v2, Time(-50)).unwrap();
        assert_eq!(s.clip(cv).unwrap().1.start, Time(0));
        assert_eq!(s.drag_clip(cv, a, Time(0)), Err(ModelError::WrongKind(a)));
    }

    #[test]
    fn delete_ripple_paste_and_link() {
        let (mut s, v, a) = seq();
        let l = s.new_link();
        let linked = NewClip {
            link: Some(l),
            ..clip(0, 10)
        };
        let cv = s.add_clip(v, linked.clone()).unwrap();
        let ca = s.add_clip(a, linked).unwrap();
        let later = s.add_clip(v, clip(20, 10)).unwrap();

        // Pasted copies overwrite at `at` and share a new link of their own.
        let copied: Vec<_> = [cv, ca]
            .map(|c| s.clip(c).map(|(t, c)| (t, c.clone())).unwrap())
            .into();
        let new = s.paste(&copied, Time(25)).unwrap();
        assert_eq!(
            spans(&s, v),
            [(0, 100, 110), (20, 100, 105), (25, 100, 110)]
        );
        let links: Vec<_> = new.iter().map(|c| s.clip(*c).unwrap().1.link).collect();
        assert!(links[0].is_some() && links[0] == links[1] && links[0] != Some(l));

        // Delete leaves the gap; ripple closes it on the clip's own track.
        let mut gap = s.clone();
        gap.remove_clips(&[cv], false).unwrap();
        assert_eq!(spans(&gap, v)[0], (20, 100, 105));
        s.remove_clips(&[cv, ca], true).unwrap();
        assert_eq!(spans(&s, v), [(10, 100, 105), (15, 100, 110)]);
        assert_eq!(spans(&s, a), [(15, 100, 110)]);
        let before = s.clone();
        assert_eq!(
            s.remove_clips(&[later, cv], false),
            Err(ModelError::UnknownClip(cv))
        );
        assert_eq!(s, before);

        // Unlink then relink.
        s.link_clips(&new, false).unwrap();
        assert_eq!(s.linked(new[0]).len(), 1);
        s.link_clips(&new, true).unwrap();
        assert_eq!(s.linked(new[0]).len(), 2);
    }

    #[test]
    fn placement_uses_source_marks_and_target_tracks() {
        let mut p = Project::default();
        let (v1, v2, a1) = (
            p.sequence.add_track(TrackKind::Video, "V1"),
            p.sequence.add_track(TrackKind::Video, "V2"),
            p.sequence.add_track(TrackKind::Audio, "A1"),
        );
        p.sequence.track_mut(v2).unwrap().target = true;
        let id = p.add_asset("a.mp4");
        let probe = ProbeInfo {
            duration: Time(100),
            width: Some(4),
            audio_streams: 1,
            ..Default::default()
        };
        p.asset_mut(id).unwrap().probe = Some(probe.clone());
        let pl = p.placement(id, Time(5), true, None, false).unwrap();
        assert_eq!((pl.source_in, pl.source_out), (Time(0), Time(100)));
        assert_eq!((pl.video, pl.audio), (Some(v2), Some(a1)));
        // A drop picks the track under the pointer for its kind.
        assert_eq!(
            p.placement(id, Time(5), false, Some(v1), false)
                .unwrap()
                .video,
            Some(v1)
        );
        let a = p.asset_mut(id).unwrap();
        (a.in_point, a.out_point) = (Some(Time(10)), Some(Time(20)));
        let pl = p.placement(id, Time(-5), false, None, false).unwrap();
        assert_eq!(
            (pl.source_in, pl.source_out, pl.start),
            (Time(10), Time(20), Time(0))
        );
        // Audio-only file: no video clip.
        p.asset_mut(id).unwrap().probe = Some(ProbeInfo {
            width: None,
            ..probe
        });
        assert_eq!(
            p.placement(id, Time(0), false, None, false).unwrap().video,
            None
        );
    }

    #[test]
    fn placement_on_an_empty_sequence_adds_tracks() {
        let mut p = Project::default();
        let id = p.add_asset("a.mp4");
        p.asset_mut(id).unwrap().probe = Some(ProbeInfo {
            duration: Time(100),
            width: Some(4),
            audio_streams: 1,
            ..Default::default()
        });
        let pl = p.placement(id, Time(5), false, None, false).unwrap();
        assert_eq!(p.sequence.place(pl).unwrap().len(), 2);
        let kinds: Vec<_> = p.sequence.tracks().iter().map(|t| t.kind).collect();
        assert_eq!(kinds, [TrackKind::Video, TrackKind::Audio]);
    }

    #[test]
    fn transform_keyframes_override_static_values() {
        use crate::keyframe::{Easing, Value};
        let key = |t, value| Keyframe {
            time: Time(t),
            value,
            easing: Easing::Linear,
        };
        let mut c = Clip {
            id: ClipId(1),
            asset: AssetId(1),
            source_in: Time(100),
            source_out: Time(200),
            start: Time(0),
            link: None,
            params: BTreeMap::new(),
            keyframes: BTreeMap::new(),
            transform: Transform {
                position: [5.0, 6.0],
                rotation: 30.0,
                ..Default::default()
            },
            stream: Some(0),
            enabled: true,
            speed: 1.0,
            reverse: false,
            freeze: None,
        };
        assert_eq!(c.transform_at(Time(0)), c.transform);
        let mut k = |name: &str, keys: Vec<Keyframe>| c.keyframes.insert(name.into(), keys);
        // Keyframe times are source time: timeline 10 is source 110.
        k(
            "scale",
            vec![key(100, Value::Scalar(1.0)), key(120, Value::Scalar(3.0))],
        );
        k(
            "anchor",
            vec![
                key(100, Value::Vec2([0.0, 0.0])),
                key(120, Value::Vec2([10.0, -20.0])),
            ],
        );
        k("crop_top", vec![key(100, Value::Scalar(4.0))]);
        k("opacity", vec![key(100, Value::Scalar(2.0))]);
        k(
            "exposure",
            vec![key(100, Value::Scalar(0.0)), key(120, Value::Scalar(2.0))],
        );
        k("tint", vec![key(100, Value::Scalar(-3.0))]);
        let x = c.transform_at(Time(10));
        assert_eq!(x.scale, [2.0, 2.0]);
        assert_eq!(x.anchor, [5.0, -10.0]);
        assert_eq!(x.crop, [0.0, 4.0, 0.0, 0.0]);
        assert_eq!((x.position, x.rotation, x.opacity), ([5.0, 6.0], 30.0, 1.0));
        assert_eq!(x.grade(), [1.0, 0.0, 0.0, 0.0, -1.0], "keyed and clamped");
        c.keyframes
            .insert("scale".into(), vec![key(0, Value::Vec2([0.5, 2.0]))]);
        assert_eq!(c.transform_at(Time(10)).scale, [0.5, 2.0]);
        assert!(!c.gain_animated());
        assert_eq!(c.gain_at(Time(10)), (1.0, 0.0));
        c.keyframes.insert(
            "volume".into(),
            vec![key(100, Value::Scalar(0.0)), key(120, Value::Scalar(1.0))],
        );
        assert!(c.gain_animated());
        assert_eq!(c.gain_at(Time(5)), (0.25, 0.0));
        assert_eq!(c.transform_at(Time(5)).volume, 0.25);
        // Old files without a transform load with the identity.
        let json =
            r#"{"id":1,"asset":1,"source_in":0,"source_out":1,"start":0,"link":null,"params":{}}"#;
        let old: Clip = serde_json::from_str(json).unwrap();
        assert_eq!(old.transform, Transform::default());
    }

    #[test]
    fn audio_fx_default_off_the_page_and_round_trip() {
        let (mut s, _, a) = seq();
        let mut track = s.track(a).unwrap().clone();
        // Projects from before audio effects load with none, and saving leaves them out again.
        let json = serde_json::to_string(&track).unwrap();
        assert!(!json.contains("fx"), "{json}");
        assert_eq!(serde_json::from_str::<Track>(&json).unwrap(), track);
        track.fx.eq.mid = 6.0;
        track.fx.compressor.enabled = false;
        let json = serde_json::to_string(&track).unwrap();
        assert_eq!(serde_json::from_str::<Track>(&json).unwrap(), track);
        let mut st = track.settings();
        st.fx.compressor.ratio = 4.0;
        s.set_track(st.clone()).unwrap();
        assert_eq!(s.track(a).unwrap().fx, st.fx);
    }

    #[test]
    fn keyed_mask_points_interpolate_and_masks_round_trip() {
        use crate::keyframe::{Easing, Value};
        let mut c: Clip = serde_json::from_str(
            r#"{"id":1,"asset":1,"source_in":100,"source_out":200,"start":0,"link":null,"params":{}}"#,
        )
        .unwrap();
        assert!(
            c.transform.masks.is_empty(),
            "older clips load without masks"
        );
        let rect = Mask::new(MaskShape::Rectangle, vec![[0.0, 0.0], [100.0, 50.0]]);
        c.transform.masks.push(rect);
        let key = |t, value| Keyframe {
            time: Time(t),
            value,
            easing: Easing::Linear,
        };
        // Source 100..120 is timeline 0..20; the size doubles across it.
        c.keyframes.insert(
            Mask::point(0, 1),
            vec![
                key(100, Value::Vec2([100.0, 50.0])),
                key(120, Value::Vec2([200.0, 100.0])),
            ],
        );
        c.keyframes.insert(
            Mask::key(0, "feather"),
            vec![key(100, Value::Scalar(-5.0)), key(120, Value::Scalar(15.0))],
        );
        let at = |t| c.transform_at(Time(t)).masks[0].clone();
        assert_eq!(at(0).points, [[0.0, 0.0], [100.0, 50.0]]);
        assert_eq!(at(10).points, [[0.0, 0.0], [150.0, 75.0]]);
        assert_eq!(at(20).points[1], [200.0, 100.0]);
        assert_eq!((at(0).feather, at(10).feather), (0.0, 5.0), "clamped at 0");
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Clip>(&json).unwrap(), c);
    }

    #[test]
    fn ids_are_unique_and_stable() {
        let (mut s, v, _) = seq();
        let a = s.add_clip(v, clip(0, 10)).unwrap();
        let b = s.add_clip(v, clip(10, 10)).unwrap();
        assert_ne!(a, b);
        s.remove_clip(a).unwrap();
        let c = s.add_clip(v, clip(0, 10)).unwrap();
        assert!(c != a && c != b);
        s.move_clip(b, v, Time(50)).unwrap();
        assert_eq!(s.clip(b).unwrap().1.start, Time(50));
    }

    #[test]
    fn add_rejects_overlap_but_allows_touching() {
        let (mut s, v, a) = seq();
        let x = s.add_clip(v, clip(10, 10)).unwrap();
        assert_eq!(s.add_clip(v, clip(15, 10)), Err(ModelError::Overlap(x)));
        assert_eq!(s.add_clip(v, clip(5, 10)), Err(ModelError::Overlap(x)));
        assert_eq!(s.add_clip(v, clip(0, 100)), Err(ModelError::Overlap(x)));
        assert_eq!(s.add_clip(v, clip(12, 2)), Err(ModelError::Overlap(x)));
        s.add_clip(v, clip(0, 10)).unwrap();
        s.add_clip(v, clip(20, 5)).unwrap();
        s.add_clip(a, clip(15, 10)).unwrap(); // other track is fine
        assert_eq!(s.add_clip(v, clip(30, 0)), Err(ModelError::EmptyClip));
        assert_eq!(
            s.add_clip(TrackId(99), clip(30, 1)),
            Err(ModelError::UnknownTrack(TrackId(99)))
        );
    }

    #[test]
    fn move_and_trim_reject_overlap() {
        let (mut s, v, a) = seq();
        let x = s.add_clip(v, clip(0, 10)).unwrap();
        let y = s.add_clip(v, clip(20, 10)).unwrap();
        assert_eq!(s.move_clip(x, v, Time(15)), Err(ModelError::Overlap(y)));
        s.move_clip(x, v, Time(5)).unwrap(); // overlapping its own old position is fine
        s.move_clip(x, a, Time(25)).unwrap();
        assert_eq!(s.clip(x).unwrap().0, a);
        s.move_clip(x, v, Time(0)).unwrap();
        assert_eq!(
            s.set_clip_range(x, Time(0), Time(0), Time(21)),
            Err(ModelError::Overlap(y))
        );
        s.set_clip_range(x, Time(0), Time(0), Time(20)).unwrap();
        assert_eq!(s.clip(x).unwrap().1.end(), Time(20));
        assert_eq!(
            s.set_clip_range(x, Time(0), Time(5), Time(5)),
            Err(ModelError::EmptyClip)
        );
        // failed mutations leave the clip where it was
        assert_eq!(s.clip(x).unwrap().1.start, Time(0));
        let starts: Vec<_> = s
            .track(v)
            .unwrap()
            .clips()
            .iter()
            .map(|c| c.start)
            .collect();
        assert_eq!(starts, [Time(0), Time(20)]);
    }

    #[test]
    fn clips_at_and_in_range() {
        let (mut s, v, a) = seq();
        let x = s.add_clip(v, clip(0, 10)).unwrap();
        let y = s.add_clip(v, clip(10, 10)).unwrap();
        let z = s.add_clip(a, clip(5, 10)).unwrap();
        assert_eq!(ids(s.clips_at(Time(0))), [x]);
        assert_eq!(ids(s.clips_at(Time(9))), [x, z]);
        assert_eq!(ids(s.clips_at(Time(10))), [y, z]); // end is exclusive
        assert_eq!(ids(s.clips_at(Time(20))), []);
        assert_eq!(ids(s.clips_in(Time(9), Time(11))), [x, y, z]);
        assert_eq!(ids(s.clips_in(Time(15), Time(30))), [y]);
        assert_eq!(ids(s.clips_in(Time(20), Time(30))), []);
        assert_eq!(ids(s.clips_in(Time(5), Time(5))), []);
    }

    #[test]
    fn next_and_previous_edit() {
        let (mut s, v, a) = seq();
        assert_eq!(s.next_edit(Time(0)), None);
        s.add_clip(v, clip(10, 10)).unwrap();
        s.add_clip(a, clip(15, 20)).unwrap();
        assert_eq!(s.next_edit(Time(0)), Some(Time(10)));
        assert_eq!(s.next_edit(Time(10)), Some(Time(15)));
        assert_eq!(s.next_edit(Time(15)), Some(Time(20)));
        assert_eq!(s.next_edit(Time(35)), None);
        assert_eq!(s.previous_edit(Time(35)), Some(Time(20)));
        assert_eq!(s.previous_edit(Time(15)), Some(Time(10)));
        assert_eq!(s.previous_edit(Time(10)), None);
        assert_eq!(s.duration(), Time(35));
    }

    #[test]
    fn markers_in_out_and_project() {
        let mut p = Project::default();
        let asset = p.add_asset("rec.mkv");
        p.asset_mut(asset).unwrap().probe = Some(ProbeInfo {
            duration: Time(1000),
            ..Default::default()
        });
        assert_eq!(
            p.asset(asset).unwrap().probe.as_ref().unwrap().duration,
            Time(1000)
        );
        let s = &mut p.sequence;
        let m = s.add_marker(Time(5), Time(0), "kill");
        s.add_marker(Time(9), Time(3), "round");
        assert_eq!(s.remove_marker(m).unwrap().name, "kill");
        assert_eq!(s.markers.len(), 1);
        s.in_point = Some(Time(1));
        s.out_point = Some(Time(4));
        let t = s.add_track(TrackKind::Video, "V1");
        let c = s
            .add_clip(
                t,
                NewClip {
                    asset,
                    ..clip(0, 10)
                },
            )
            .unwrap();
        let l = s.new_link();
        s.with_clip_mut(c, |p| {
            *p.link = Some(l);
            p.params.insert("opacity".into(), 0.5);
        })
        .unwrap();
        assert_eq!(s.clip(c).unwrap().1.link, Some(l));
        s.track_mut(t).unwrap().muted = true;
        assert_eq!(s.remove_track(t).unwrap().clips().len(), 1);
        assert_eq!(s.clip(c), None);
    }

    #[test]
    fn time_arithmetic() {
        assert_eq!(Time(5) + Time(3), Time(8));
        assert_eq!(Time(5) - Time(8), Time(-3));
    }

    #[test]
    fn text_places_as_video_only_with_the_still_length() {
        let mut p = Project::default();
        let v1 = p.sequence.add_track(TrackKind::Video, "V1");
        p.sequence.add_track(TrackKind::Audio, "A1");
        let id = p.add_asset("");
        p.asset_mut(id).unwrap().text = Some(Text::default());
        let pl = p.placement(id, Time(0), false, None, false).unwrap();
        assert_eq!((pl.video, pl.audio), (Some(v1), None));
        let secs = i64::from(crate::settings::DEFAULT_STILL_SECS);
        assert_eq!(pl.source_out, Time(secs * crate::time::FLICKS_PER_SECOND));
    }

    #[test]
    fn audio_streams_get_their_own_tracks_below_the_target() {
        let mut p = Project::default();
        let a1 = p.sequence.add_track(TrackKind::Audio, "A1");
        let locked = p.sequence.add_track(TrackKind::Audio, "A2");
        p.sequence
            .tracks
            .iter_mut()
            .for_each(|t| t.locked = t.id == locked);
        let a3 = p.sequence.add_track(TrackKind::Audio, "A3");
        let id = p.add_asset("obs.mkv");
        let audio = |title: Option<&str>| {
            StreamInfo::Audio(AudioInfo {
                title: title.map(Into::into),
                ..Default::default()
            })
        };
        p.asset_mut(id).unwrap().probe = Some(ProbeInfo {
            duration: Time(100),
            audio_streams: 3,
            streams: vec![audio(Some("Game")), audio(None), audio(Some("Chat"))],
            ..Default::default()
        });
        let pl = p.placement(id, Time(0), false, None, false).unwrap();
        let names: Vec<_> = pl.streams.iter().map(|s| s.1.as_str()).collect();
        assert_eq!(names, ["Game", "Audio 2", "Chat"]);
        let ids = p.sequence.place(pl).unwrap();
        // Skips the locked track, reuses A3, then adds one named for the stream.
        let tracks: Vec<_> = ids.iter().map(|c| p.sequence.clip(*c).unwrap().0).collect();
        assert_eq!(tracks[..2], [a1, a3]);
        assert_eq!(p.sequence.track(tracks[2]).unwrap().name, "Chat");
        assert_eq!(p.sequence.linked(ids[0]).len(), 3);
        let pl = p.placement(id, Time(0), false, None, true).unwrap();
        assert_eq!(pl.streams, [(None, String::new())]);
    }
}
