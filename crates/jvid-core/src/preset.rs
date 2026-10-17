//! Export presets: codecs, output size and rate, and encoder options. Built-ins live here; user
//! presets are TOML files read by jvid-app.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::time::Rate;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264,
    Hevc,
    ProRes,
    Gif,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    Aac,
    Mp3,
    /// 16-bit PCM.
    Pcm,
    /// 24-bit PCM.
    Pcm24,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    /// File extension, which picks the container.
    pub extension: String,
    pub video: Option<VideoCodec>,
    pub audio: Option<AudioCodec>,
    /// Output `[width, height]`; `None` keeps the project size.
    pub size: Option<[u32; 2]>,
    /// `None` keeps the project rate.
    pub frame_rate: Option<Rate>,
    /// Encoder options (e.g. `crf`, `preset`, `tune`); encoders ignore ones they don't have.
    #[serde(default)]
    pub options: BTreeMap<String, String>,
}

impl Preset {
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// A warning when the output size has a different aspect ratio than the project's.
    pub fn aspect_warning(&self, (w, h): (u32, u32)) -> Option<String> {
        let [pw, ph] = self.size.filter(|_| self.video.is_some())?;
        (pw as u64 * h as u64 != ph as u64 * w as u64).then(|| {
            format!(
                "{} is {pw}×{ph}, which doesn't match the project's {w}×{h}; the picture will be stretched",
                self.name
            )
        })
    }
}

/// The built-in presets, in menu order.
pub fn builtin() -> Vec<Preset> {
    use AudioCodec::*;
    use VideoCodec::*;
    let p = |name: &str, ext: &str, video, audio, size, fps: Option<u32>, opts: &[(&str, &str)]| {
        Preset {
            name: name.into(),
            extension: ext.into(),
            video,
            audio,
            size,
            frame_rate: fps.map(|f| Rate::new(f, 1)),
            options: opts
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    };
    let hq = &[("crf", "18"), ("preset", "medium")][..];
    vec![
        p(
            "Highlight 1080p60",
            "mp4",
            Some(H264),
            Some(Aac),
            Some([1920, 1080]),
            Some(60),
            hq,
        ),
        p(
            "Highlight 1440p60",
            "mp4",
            Some(H264),
            Some(Aac),
            Some([2560, 1440]),
            Some(60),
            hq,
        ),
        // Source size, low crf and no deblocking blur keep small text crisp.
        p(
            "Screen recording (sharp text)",
            "mp4",
            Some(H264),
            Some(Aac),
            None,
            None,
            &[("crf", "14"), ("preset", "medium"), ("tune", "stillimage")],
        ),
        p(
            "YouTube 1080p",
            "mp4",
            Some(H264),
            Some(Aac),
            Some([1920, 1080]),
            None,
            hq,
        ),
        p(
            "YouTube 4K",
            "mp4",
            Some(H264),
            Some(Aac),
            Some([3840, 2160]),
            None,
            hq,
        ),
        p(
            "Shorts/Reels 9:16",
            "mp4",
            Some(H264),
            Some(Aac),
            Some([1080, 1920]),
            None,
            hq,
        ),
        p(
            "ProRes master",
            "mov",
            Some(ProRes),
            Some(Pcm),
            None,
            None,
            &[("profile", "3")],
        ),
        p("GIF", "gif", Some(Gif), None, None, Some(25), &[]),
        p("Audio only (WAV)", "wav", None, Some(Pcm), None, None, &[]),
        p(
            "Audio only (WAV 24-bit)",
            "wav",
            None,
            Some(Pcm24),
            None,
            None,
            &[],
        ),
        p("Audio only (MP3)", "mp3", None, Some(Mp3), None, None, &[]),
        p("Audio only (AAC)", "m4a", None, Some(Aac), None, None, &[]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_preset_parses_and_round_trips() {
        let p = Preset::from_toml(
            "name = \"Mine\"\nextension = \"mkv\"\nvideo = \"h264\"\naudio = \"aac\"\n\
             size = [1280, 720]\nframe_rate = { num = 30000, den = 1001 }\n\
             [options]\ncrf = \"22\"\n",
        )
        .unwrap();
        assert_eq!(p.size, Some([1280, 720]));
        assert_eq!(p.frame_rate, Some(Rate::new(30000, 1001)));
        assert_eq!(p.options["crf"], "22");
        assert_eq!(Preset::from_toml(&toml::to_string(&p).unwrap()).unwrap(), p);
        let a = Preset::from_toml("name = \"A\"\nextension = \"wav\"\naudio = \"pcm\"").unwrap();
        assert_eq!((a.video, a.size), (None, None));
    }

    #[test]
    fn aspect_warning_only_on_mismatch() {
        let all = builtin();
        let find = |n: &str| all.iter().find(|p| p.name.starts_with(n)).unwrap();
        assert!(find("YouTube 4K").aspect_warning((1920, 1080)).is_none());
        assert!(find("Shorts").aspect_warning((1920, 1080)).is_some());
        assert!(find("Shorts").aspect_warning((720, 1280)).is_none());
        assert!(find("Screen").aspect_warning((1000, 1)).is_none());
        assert!(find("Audio only (WAV)").aspect_warning((1, 1)).is_none());
    }
}
