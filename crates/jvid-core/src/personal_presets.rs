//! Portable, single-category personal styles. No project or source identity is stored.
//!
//! Animation is normalized to clip-relative timeline flicks by settings transfer. It is
//! optional on capture and application; static application always uses the captured sample.
//! Overflow, boundary sampling and destination validation remain settings-transfer rules.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::keyframe::Keyframe;
use crate::model::{AudioFx, Effect, Text};
use crate::settings_transfer::Category;
use crate::time::Time;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "category",
    content = "properties",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Style {
    Text(#[serde(with = "title_style::TitleStyle")] Text),
    Audio { volume: f64, pan: f64, fx: AudioFx },
    Effects { effects: Vec<Effect> },
}

impl Style {
    pub fn category(&self) -> Category {
        match self {
            Self::Text(_) => Category::Text,
            Self::Audio { .. } => Category::Audio,
            Self::Effects { .. } => Category::Effects,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub style: Style,
    /// A timing envelope for the optional animation, never a destination duration edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation_duration: Option<Time>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keyframes: BTreeMap<String, Vec<Keyframe>>,
}

/// A remote serde representation keeps the model's text style while excluding words even
/// from hand-edited input. The in-memory conversion also requires empty content.
mod title_style {
    use crate::model::{Align, Text};
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    #[serde(remote = "Text", deny_unknown_fields)]
    pub(super) struct TitleStyle {
        #[serde(skip)]
        content: String,
        family: String,
        size: f32,
        weight: u16,
        color: [f64; 4],
        align: Align,
        anchor: [f32; 2],
        outline: f32,
        outline_color: [f64; 4],
        shadow: [f32; 2],
        shadow_color: [f64; 4],
        box_padding: f32,
        box_color: [f64; 4],
    }
}
