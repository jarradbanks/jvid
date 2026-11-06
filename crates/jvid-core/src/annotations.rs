//! Generated vector content. Coordinates are project pixels from frame centre, y down.
//! Clip transforms, source-time keyframes, trims and fades remain ordinary clip properties.

use serde::{Deserialize, Serialize};

use crate::keyframe::Value;
use crate::model::{AssetId, Clip, Project};
use crate::time::Time;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationKind {
    #[default]
    Arrow,
    Rectangle,
    Callout,
}

/// Reusable appearance only. A style never carries points, numbering, kind or clip timing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnnotationStyle {
    pub stroke_color: [f64; 4],
    pub width: f64,
    pub arrow_head: f64,
    pub callout_radius: f64,
    pub fill_color: [f64; 4],
    pub text_color: [f64; 4],
}

impl Default for AnnotationStyle {
    fn default() -> Self {
        Self {
            stroke_color: [1.0, 0.3, 0.1, 1.0],
            width: 6.0,
            arrow_head: 28.0,
            callout_radius: 28.0,
            fill_color: [0.03, 0.03, 0.03, 1.0],
            text_color: [1.0; 4],
        }
    }
}

impl AnnotationStyle {
    pub const KEYS: [&'static str; 6] = [
        "annotation_stroke_color",
        "annotation_width",
        "annotation_arrow_head",
        "annotation_callout_radius",
        "annotation_fill_color",
        "annotation_text_color",
    ];

    pub fn get(&self, name: &str) -> Option<Value> {
        Some(match name {
            "annotation_stroke_color" => Value::Color(self.stroke_color),
            "annotation_width" => Value::Scalar(self.width),
            "annotation_arrow_head" => Value::Scalar(self.arrow_head),
            "annotation_callout_radius" => Value::Scalar(self.callout_radius),
            "annotation_fill_color" => Value::Color(self.fill_color),
            "annotation_text_color" => Value::Color(self.text_color),
            _ => return None,
        })
    }

    /// Unknown names, mismatched types and invalid values leave the style unchanged.
    pub fn set(&mut self, name: &str, value: Value) {
        let mut next = self.clone();
        match (name, value) {
            ("annotation_stroke_color", Value::Color(c)) => next.stroke_color = c,
            ("annotation_width", Value::Scalar(n)) => next.width = n,
            ("annotation_arrow_head", Value::Scalar(n)) => next.arrow_head = n,
            ("annotation_callout_radius", Value::Scalar(n)) => next.callout_radius = n,
            ("annotation_fill_color", Value::Color(c)) => next.fill_color = c,
            ("annotation_text_color", Value::Color(c)) => next.text_color = c,
            _ => return,
        }
        if next.valid() {
            *self = next;
        }
    }

    pub fn valid(&self) -> bool {
        [self.stroke_color, self.fill_color, self.text_color]
            .into_iter()
            .flatten()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(&v))
            && [self.width, self.arrow_head, self.callout_radius]
                .into_iter()
                .all(|v| v.is_finite() && (0.1..=10_000.0).contains(&v))
    }
}

/// Arrow tail/tip, rectangle opposite corners, or callout badge centre/leader tip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Annotation {
    pub kind: AnnotationKind,
    pub points: [[f64; 2]; 2],
    pub number: u32,
    pub style: AnnotationStyle,
}

impl Default for Annotation {
    fn default() -> Self {
        Self {
            kind: AnnotationKind::Arrow,
            points: [[-160.0, 0.0], [160.0, 0.0]],
            number: 1,
            style: AnnotationStyle::default(),
        }
    }
}

impl Annotation {
    pub const POINT_KEYS: [&'static str; 2] = ["annotation_a", "annotation_b"];

    pub fn valid(&self) -> bool {
        self.points.into_iter().flatten().all(f64::is_finite)
            && (1..=9999).contains(&self.number)
            && self.style.valid()
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        match name {
            "annotation_a" => Some(Value::Vec2(self.points[0])),
            "annotation_b" => Some(Value::Vec2(self.points[1])),
            "annotation_number" => Some(Value::Scalar(f64::from(self.number))),
            _ => self.style.get(name),
        }
    }

    pub fn set(&mut self, name: &str, value: Value) {
        match (name, value) {
            ("annotation_number", Value::Scalar(n)) if (1.0..=9999.0).contains(&n) => {
                self.number = n.round() as u32;
            }
            ("annotation_a" | "annotation_b", Value::Vec2(p))
                if p.into_iter().all(f64::is_finite) =>
            {
                self.points[usize::from(name == "annotation_b")] = p;
            }
            _ => self.style.set(name, value),
        }
    }

    pub fn with(&self, keyed: &[(&str, Value)]) -> Self {
        let mut annotation = self.clone();
        for (name, value) in keyed {
            annotation.set(name, *value);
        }
        annotation
    }

    /// Loose content bounds; hit testing refines these in the editor. Independent of view.
    pub fn bounds(&self) -> [f64; 4] {
        let [a, b] = self.points;
        let pad = self.style.width / 2.0
            + match self.kind {
                AnnotationKind::Arrow => self.style.arrow_head,
                AnnotationKind::Rectangle => 0.0,
                AnnotationKind::Callout => self.style.callout_radius,
            };
        [
            a[0].min(b[0]) - pad,
            a[1].min(b[1]) - pad,
            a[0].max(b[0]) + pad,
            a[1].max(b[1]) + pad,
        ]
    }
}

impl Clip {
    /// Evaluates in timeline time using the same source mapping as every other clip key.
    pub fn annotation_keys(&self, at: Time) -> Vec<(&'static str, Value)> {
        Annotation::POINT_KEYS
            .into_iter()
            .chain(AnnotationStyle::KEYS)
            .chain(["annotation_number"])
            .filter_map(|name| self.value_at(name, at).map(|v| (name, v)))
            .collect()
    }
}

/// Plan an atomic, non-destructive overlay insertion. Rehearsing IDs on a copy keeps
/// creation, placement and participation in a solo video mix in one ordinary undo step.
pub fn create_command(
    project: &Project,
    name: String,
    annotation: Annotation,
    start: Time,
    duration: Time,
) -> Result<(crate::history::Command, crate::model::ClipId), crate::model::ModelError> {
    use crate::history::{Command, Created, History};
    use crate::model::{ModelError, Placement, TrackKind};

    if !annotation.valid() {
        return Err(ModelError::BadAnnotation);
    }
    if duration <= Time(0) || start < Time(0) || start.0.checked_add(duration.0).is_none() {
        return Err(ModelError::EmptyClip);
    }
    let add = Command::AddAnnotation { name, annotation };
    let mut trial = History::new(project.clone());
    let Created::Asset(asset) = trial.apply(add.clone())? else {
        unreachable!()
    };
    let seq = trial.project().sequence();
    let solo = seq
        .tracks()
        .iter()
        .any(|t| t.kind == TrackKind::Video && t.solo);
    let top = seq
        .tracks()
        .iter()
        .find(|t| t.kind == TrackKind::Video && !t.hidden && (!solo || t.solo));
    let video = top
        .filter(|t| {
            !t.locked
                && !t.subtitles
                && t.clips()
                    .iter()
                    .all(|c| c.end() <= start || c.start >= start + duration)
        })
        .map(|t| t.id);
    let put = Command::Place(Placement {
        asset,
        source_in: Time(0),
        source_out: duration,
        start,
        video,
        audio: None,
        streams: Vec::new(),
        add_video: video.is_none(),
        add_audio: false,
        insert: false,
    });
    let Created::Clip(clip) = trial.apply(put.clone())? else {
        unreachable!()
    };
    let mut commands = vec![add, put];
    if solo {
        let (track, _) = trial
            .project()
            .sequence()
            .clip(clip)
            .expect("placed annotation");
        let mut settings = trial
            .project()
            .sequence()
            .track(track)
            .expect("annotation track")
            .settings();
        if !settings.solo {
            settings.solo = true;
            commands.push(Command::SetTrack(settings));
        }
    }
    Ok((Command::Batch(commands), clip))
}

impl Project {
    /// The caller validates user data before issuing an undoable AddAnnotation command.
    pub fn add_annotation(&mut self, name: String, annotation: Annotation) -> AssetId {
        let id = self.add_asset(std::path::PathBuf::new());
        let asset = self.asset_mut(id).expect("new annotation asset");
        asset.name = Some(name);
        asset.annotation = Some(annotation);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_keyed_styles_preserve_content() {
        let base: Annotation = serde_json::from_str("{}").unwrap();
        assert_eq!(base, Annotation::default());
        assert!(base.valid());
        let shown = base.with(&[("annotation_width", Value::Scalar(12.0))]);
        assert_eq!(shown.points, base.points);
        assert_eq!(shown.number, base.number);
        assert_eq!(shown.style.width, 12.0);
        for key in Annotation::POINT_KEYS
            .into_iter()
            .chain(AnnotationStyle::KEYS)
        {
            let mut copy = base.clone();
            copy.set(key, base.get(key).unwrap());
            assert_eq!(copy, base, "{key}");
        }
        let mut invalid = base.clone();
        invalid.set("annotation_width", Value::Scalar(f64::NAN));
        invalid.set("annotation_a", Value::Vec2([f64::INFINITY, 1.0]));
        invalid.set("annotation_fill_color", Value::Color([2.0; 4]));
        assert_eq!(invalid, base);
    }

    #[test]
    fn insertions_stack_without_overwrite_and_undo_atomically() {
        use crate::history::{Command, History};
        use crate::model::TrackKind;
        let mut h = History::new(Project::default());
        let before = h.project().clone();
        let mut ids = Vec::new();
        for kind in [
            AnnotationKind::Arrow,
            AnnotationKind::Rectangle,
            AnnotationKind::Callout,
        ] {
            let (command, clip) = create_command(
                h.project(),
                "Shape".into(),
                Annotation {
                    kind,
                    ..Default::default()
                },
                Time(10),
                Time(100),
            )
            .unwrap();
            h.apply(command).unwrap();
            ids.push(clip);
        }
        let shown = crate::eval::video_at(h.project().sequence(), Time(20));
        assert_eq!(
            shown.iter().map(|l| l.clip).collect::<Vec<_>>(),
            ids.iter().rev().copied().collect::<Vec<_>>()
        );
        assert!(crate::eval::video_at(h.project().sequence(), Time(9)).is_empty());
        assert!(crate::eval::video_at(h.project().sequence(), Time(110)).is_empty());
        assert!(
            h.project()
                .assets()
                .iter()
                .all(|a| a.generated() && a.path.as_os_str().is_empty())
        );
        for _ in 0..3 {
            h.undo();
        }
        assert_eq!(h.project(), &before);
        for _ in 0..3 {
            h.redo();
        }
        // Locked/solo content stays intact; a new top overlay participates in the solo mix.
        let top = h
            .project()
            .sequence()
            .tracks()
            .iter()
            .find(|t| t.kind == TrackKind::Video)
            .unwrap();
        let mut settings = top.settings();
        settings.locked = true;
        settings.solo = true;
        h.apply(Command::SetTrack(settings)).unwrap();
        let (command, clip) = create_command(
            h.project(),
            "Overlay".into(),
            Annotation::default(),
            Time(10),
            Time(100),
        )
        .unwrap();
        h.apply(command).unwrap();
        let layers = crate::eval::video_at(h.project().sequence(), Time(20));
        assert_eq!(layers[0].clip, clip);
        assert_eq!(layers.len(), 2);
    }

    #[test]
    fn keys_follow_trim_speed_and_fades_use_timeline_edges() {
        use crate::history::{Command, History};
        use crate::keyframe::{Easing, Keyframe};
        let mut h = History::new(Project::default());
        let (command, clip) = create_command(
            h.project(),
            "Arrow".into(),
            Annotation::default(),
            Time(10),
            Time(100),
        )
        .unwrap();
        h.apply(command).unwrap();
        for (time, width) in [(0, 2.0), (100, 12.0)] {
            h.apply(Command::AddKeyframe {
                id: clip,
                param: "annotation_width".into(),
                key: Keyframe {
                    time: Time(time),
                    value: Value::Scalar(width),
                    easing: Easing::Linear,
                },
            })
            .unwrap();
        }
        h.apply(Command::SetClipSpeed {
            id: clip,
            speed: 2.0,
            reverse: false,
            ripple: false,
        })
        .unwrap();
        let c = h.project().sequence().clip(clip).unwrap().1;
        let mut x = c.transform.clone();
        x.fade_in = Time(10);
        x.fade_out = Time(10);
        h.apply(Command::SetClipProps(vec![(clip, x, c.keyframes.clone())]))
            .unwrap();
        let layer = crate::eval::video_at(h.project().sequence(), Time(15)).remove(0);
        assert_eq!(layer.transform.opacity, 0.5);
        assert_eq!(
            Annotation::default().with(&layer.annotation).style.width,
            3.0
        );
        assert_eq!(
            crate::eval::video_at(h.project().sequence(), Time(55))[0]
                .transform
                .opacity,
            0.5
        );
        h.apply(Command::TrimClip {
            id: clip,
            start: Time(20),
            source_in: Time(20),
            source_out: Time(100),
        })
        .unwrap();
        let layer = crate::eval::video_at(h.project().sequence(), Time(20)).remove(0);
        assert_eq!(layer.source_time, Time(20));
        assert_eq!(layer.transform.opacity, 0.0);
        assert_eq!(
            Annotation::default().with(&layer.annotation).style.width,
            4.0
        );
    }

    #[test]
    fn shape_schema_roundtrips_legacy_stays_one_and_future_rejects() {
        use crate::file::{self, FileError};
        use crate::history::History;
        use std::path::Path;
        let dir = Path::new("/project");
        let mut h = History::new(Project::default());
        let legacy = file::to_json(h.project(), dir).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&legacy).unwrap()["schema_version"],
            1
        );
        assert_eq!(
            file::from_json(&legacy, dir, |_| false).unwrap(),
            *h.project()
        );
        let (command, _) = create_command(
            h.project(),
            "Badge".into(),
            Annotation {
                kind: AnnotationKind::Callout,
                number: 23,
                ..Default::default()
            },
            Time(0),
            Time(100),
        )
        .unwrap();
        h.apply(command).unwrap();
        let json = file::to_json(h.project(), dir).unwrap();
        let mut root: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(root["schema_version"], 2);
        assert_eq!(
            file::from_json(&json, dir, |_| false).unwrap(),
            *h.project()
        );
        root["schema_version"] = 3.into();
        assert!(matches!(
            file::from_json(&root.to_string(), dir, |_| false),
            Err(FileError::UnsupportedVersion(3))
        ));
        root["schema_version"] = 2.into();
        root["project"]["assets"][0]["annotation"]["style"]["width"] = (-1).into();
        assert!(matches!(
            file::from_json(&root.to_string(), dir, |_| false),
            Err(FileError::BadAnnotation)
        ));
        root["schema_version"] = 1.into();
        root["project"]["assets"][0]
            .as_object_mut()
            .unwrap()
            .remove("annotation");
        assert!(
            file::from_json(&root.to_string(), dir, |_| false)
                .unwrap()
                .assets()[0]
                .annotation
                .is_none()
        );
    }
}
