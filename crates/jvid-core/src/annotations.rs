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
            _ => self.style.get(name),
        }
    }

    pub fn set(&mut self, name: &str, value: Value) {
        match (name, value) {
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
            .filter_map(|name| self.value_at(name, at).map(|v| (name, v)))
            .collect()
    }
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
}
