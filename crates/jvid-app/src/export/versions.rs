//! Independent, immutable version exports. Editing and preview state never reach the source.

use std::path::{Path, PathBuf};

use jvid_core::history::History;
use jvid_core::model::{MarkerId, Project};
use jvid_core::preset::Preset;
use jvid_core::time::Time;

use crate::highlight_review::{HighlightSelectionSnapshot, ReviewRevision};
use crate::i18n::tr;

use super::Request;

/// Captions are a per-version choice, including omission without changing the edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Captions {
    Burn,
    Sidecar,
    Omit,
}

/// Frozen marker identity, name and half-open range; overlaps stay separate outputs.
#[derive(Clone, Debug, PartialEq)]
pub struct VersionRange {
    pub marker: Option<MarkerId>,
    pub name: String,
    pub span: (Time, Time),
}

/// Validated H handoff plus the exact project it describes. No shared mutable configuration.
#[derive(Clone)]
pub struct VersionSource {
    project: Project,
    revision: ReviewRevision,
    ranges: Vec<VersionRange>,
}

impl VersionSource {
    /// Validate H immediately before copying the source, never re-resolve silently.
    pub fn highlights(
        session: u64,
        history: &History,
        selection: &HighlightSelectionSnapshot,
    ) -> Result<Self, String> {
        selection
            .validate(session, history)
            .map_err(|_| tr!("highlights.stale").to_owned())?;
        let ranges = selection
            .highlights()
            .iter()
            .map(|h| VersionRange {
                marker: Some(h.marker().id),
                name: h.marker().name.clone(),
                span: h.span(),
            })
            .collect::<Vec<_>>();
        if ranges.is_empty() {
            return Err(tr!("highlights.no_selected").into());
        }
        Ok(Self {
            project: history.project().clone(),
            revision: selection.revision(),
            ranges,
        })
    }

    pub fn range(session: u64, history: &History, span: (Time, Time)) -> Result<Self, String> {
        if span.0 < Time(0) || span.0 >= span.1 || span.1 > history.project().sequence().duration()
        {
            return Err(tr!("export.nothing").into());
        }
        Ok(Self {
            project: history.project().clone(),
            revision: ReviewRevision::current(session, history),
            ranges: vec![VersionRange {
                marker: None,
                name: String::new(),
                span,
            }],
        })
    }

    pub fn revision(&self) -> ReviewRevision {
        self.revision
    }

    pub fn ranges(&self) -> &[VersionRange] {
        &self.ranges
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Check before accepting the setup; an edit requires reopening and reviewing new copies.
    pub fn validate(&self, session: u64, history: &History) -> Result<(), String> {
        if self.revision != ReviewRevision::current(session, history) {
            return Err(tr!("highlights.stale").into());
        }
        Ok(())
    }
}

/// Queue handoff. Private fields and read-only access keep each job independently immutable.
/// Framing is already applied to `project`; the request still uses normal sizing/target-size.
#[derive(Clone)]
pub struct VersionJob {
    project: Project,
    request: Request,
    revision: ReviewRevision,
    marker: Option<MarkerId>,
    captions: Captions,
    label: String,
}

impl VersionJob {
    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn request(&self) -> &Request {
        &self.request
    }

    pub fn revision(&self) -> ReviewRevision {
        self.revision
    }

    pub fn marker(&self) -> Option<MarkerId> {
        self.marker
    }

    pub fn captions(&self) -> Captions {
        self.captions
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn destinations(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.request.path.clone()];
        if self.request.sidecar {
            paths.push(self.request.path.with_extension("srt"));
        }
        paths
    }

    /// Name allocation happens before submission; jobs cannot be retargeted by the editor.
    pub(super) fn at_path(mut self, path: &Path) -> Self {
        self.request.path = path.to_owned();
        self
    }
}

/// Reuse the preset's existing width/size semantics; reject invalid encoder dimensions early.
fn dimensions(preset: &Preset, project: &Project) -> Result<(u32, u32), String> {
    let settings = project.settings();
    let (w, h) = preset.output_size((settings.width, settings.height));
    if w < 2 || h < 2 || w > 8192 || h > 8192 || w % 2 != 0 || h % 2 != 0 {
        return Err(tr!("export.nothing").into());
    }
    Ok((w, h))
}
