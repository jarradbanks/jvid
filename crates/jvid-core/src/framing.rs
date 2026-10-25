//! Clip framing in project pixels, independent of monitor zoom or decode/proxy resolution.

use crate::model::{Clip, Project, Transform};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
    Fit,
    Fill,
    Stretch,
}

impl Framing {
    /// A centred, uncropped whole-source preset. The compositor first contains the source,
    /// so these scales are relative to that fitted size, rather than its coded pixels.
    pub fn transform(self, image: [f64; 2], frame: [f64; 2]) -> Option<Transform> {
        if image
            .into_iter()
            .chain(frame)
            .any(|v| !v.is_finite() || v <= 0.0)
        {
            return None;
        }
        let contain = (frame[0] / image[0]).min(frame[1] / image[1]);
        let stretch = [
            frame[0] / (image[0] * contain),
            frame[1] / (image[1] * contain),
        ];
        let scale = match self {
            Self::Fit => [1.0; 2],
            Self::Fill => [stretch[0].max(stretch[1]); 2],
            Self::Stretch => stretch,
        };
        Some(Transform {
            scale,
            ..Default::default()
        })
    }
}

/// The source's display dimensions; generated and nested layers have project dimensions.
pub fn image_size(project: &Project, clip: &Clip) -> Option<[f64; 2]> {
    let asset = project.asset(clip.asset)?;
    if asset.solid.is_some() || asset.text.is_some() || asset.sequence.is_some() {
        let s = project.settings();
        Some([s.width as f64, s.height as f64])
    } else {
        asset.probe.as_ref()?.display_size()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ProbeInfo, StreamInfo, VideoInfo};
    use crate::time::Rate;

    #[test]
    fn exact_four_three_framing() {
        let image = [1440.0, 1080.0];
        let frame = [1920.0, 1080.0];
        for (mode, size) in [
            (Framing::Fit, [1440.0, 1080.0]),
            (Framing::Fill, [1920.0, 1440.0]),
            (Framing::Stretch, [1920.0, 1080.0]),
        ] {
            let x = mode.transform(image, frame).unwrap();
            assert_eq!([image[0] * x.scale[0], image[1] * x.scale[1]], size);
            assert_eq!(x.position, [0.0; 2]);
            assert_eq!(mode.transform(image, image).unwrap(), Transform::default());
        }
        assert!(Framing::Fit.transform([0.0, 1080.0], frame).is_none());
        assert!(Framing::Fill.transform(image, [f64::NAN, 1080.0]).is_none());
    }

    #[test]
    fn pixel_aspect_then_source_rotation() {
        let mut p = ProbeInfo {
            width: Some(1440),
            height: Some(1080),
            streams: vec![StreamInfo::Video(VideoInfo {
                sample_aspect: Some(Rate::new(4, 3)),
                ..Default::default()
            })],
            ..Default::default()
        };
        assert_eq!(p.display_size(), Some([1920.0, 1080.0]));
        for rotation in [90, 270] {
            let StreamInfo::Video(v) = &mut p.streams[0] else {
                unreachable!()
            };
            v.rotation = rotation;
            assert_eq!(p.display_size(), Some([1080.0, 1920.0]));
            assert_eq!(
                Framing::Stretch
                    .transform(p.display_size().unwrap(), [1920.0, 1080.0])
                    .unwrap()
                    .scale,
                [256.0 / 81.0, 1.0]
            );
        }
        let StreamInfo::Video(v) = &mut p.streams[0] else {
            unreachable!()
        };
        v.rotation = 180;
        v.sample_aspect = None;
        v.display_aspect = Some(Rate::new(16, 9));
        assert_eq!(p.display_size(), Some([1920.0, 1080.0]));
        p.streams.clear();
        assert_eq!(p.display_size(), Some([1440.0, 1080.0]));
    }
}
