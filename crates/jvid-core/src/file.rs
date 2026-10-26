//! The `.jvid` project file: versioned JSON. Pure conversion only; the caller
//! does the reading and writing.
//!
//! Each asset stores its absolute `path` plus `relative_path` (relative to the
//! project file's directory), so a project folder moved with its media still
//! resolves.

use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

use crate::model::Project;

/// The version [`to_json`] writes.
pub const SCHEMA_VERSION: u64 = 1;

#[derive(Debug)]
pub enum FileError {
    Json(serde_json::Error),
    /// The file is newer than this build understands.
    UnsupportedVersion(u64),
    MissingVersion,
    /// A frame or sample rate has a zero part.
    InvalidRate,
    /// A sequence is nested inside itself.
    Cycle,
    /// Sequences nest more than [`crate::model::MAX_NEST_DEPTH`] deep.
    TooDeep,
    /// Sequence ids repeat, or an asset names a missing one.
    BadSequence,
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => write!(f, "invalid project file: {e}"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported project file version {v}"),
            Self::MissingVersion => f.write_str("project file has no schema_version"),
            Self::InvalidRate => f.write_str("project file has a zero frame or sample rate"),
            Self::Cycle => f.write_str("project file nests a sequence inside itself"),
            Self::TooDeep => f.write_str("project file nests sequences too deep"),
            Self::BadSequence => f.write_str("project file has mismatched sequence ids"),
        }
    }
}

impl std::error::Error for FileError {}

impl From<serde_json::Error> for FileError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Serializes `project` for a file in `project_dir`.
pub fn to_json(project: &Project, project_dir: &Path) -> Result<String, FileError> {
    to_json_with_paths(project, project_dir, Path::to_path_buf)
}

/// Like [`to_json`], resolving paths only for relative-link calculation. The caller can
/// resolve filesystem aliases (such as Windows short names) without changing stored paths.
pub fn to_json_with_paths(
    project: &Project,
    project_dir: &Path,
    resolve: impl Fn(&Path) -> PathBuf,
) -> Result<String, FileError> {
    let project_dir = resolve(project_dir);
    let mut p = serde_json::to_value(project)?;
    for a in assets(&mut p) {
        let rel = a["path"]
            .as_str()
            .and_then(|s| relativize(&resolve(Path::new(s)), &project_dir));
        // `/` separators so a project saved on Windows also opens on Unix.
        let rel = rel.and_then(|r| {
            let parts: Option<Vec<_>> = r.iter().map(|c| c.to_str()).collect();
            Some(parts?.join("/"))
        });
        if let Some(rel) = rel {
            a["relative_path"] = rel.into();
        }
    }
    Ok(serde_json::to_string_pretty(
        &json!({ "schema_version": SCHEMA_VERSION, "project": p }),
    )?)
}

/// Parses a project file read from `project_dir`. An asset's relative path wins
/// when `exists` says it resolves; otherwise its absolute path is kept.
pub fn from_json(
    s: &str,
    project_dir: &Path,
    exists: impl Fn(&Path) -> bool,
) -> Result<Project, FileError> {
    let mut root: Value = serde_json::from_str(s)?;
    let version = root["schema_version"]
        .as_u64()
        .ok_or(FileError::MissingVersion)?;
    migrate(&mut root, version)?;
    let mut p = root["project"].take();
    for a in assets(&mut p) {
        if let Some(rel) = a.as_object_mut().and_then(|o| o.remove("relative_path")) {
            let candidate = rebase(Path::new(rel.as_str().unwrap_or_default()), project_dir);
            if exists(&candidate) {
                a["path"] = candidate.to_string_lossy().into_owned().into();
            }
        }
    }
    let project: Project = serde_json::from_value(p)?;
    let s = project.settings();
    let rates = std::iter::once(s.frame_rate).chain(
        project
            .assets()
            .iter()
            .filter_map(|a| a.probe.as_ref()?.frame_rate),
    );
    if s.sample_rate == 0 || rates.into_iter().any(|r| r.num == 0 || r.den == 0) {
        return Err(FileError::InvalidRate);
    }
    // A reused id would let the next Nest play another sequence.
    if !project.sequence_ids_ok() {
        return Err(FileError::BadSequence);
    }
    // Rendering recurses into nested sequences, so a loop would never end.
    if project.has_cycle() {
        return Err(FileError::Cycle);
    }
    if project.too_deep() {
        return Err(FileError::TooDeep);
    }
    Ok(project)
}

/// Upgrades `root` from `version` to [`SCHEMA_VERSION`] in place. Add one step
/// per bump, e.g. `if version < 2 { ...; }`.
fn migrate(root: &mut Value, version: u64) -> Result<(), FileError> {
    if version > SCHEMA_VERSION || version == 0 {
        return Err(FileError::UnsupportedVersion(version));
    }
    root["schema_version"] = SCHEMA_VERSION.into();
    Ok(())
}

fn assets(p: &mut Value) -> impl Iterator<Item = &mut Value> {
    p["assets"].as_array_mut().into_iter().flatten()
}

/// `path` relative to `dir`, if both are absolute and share a root.
pub fn relativize(path: &Path, dir: &Path) -> Option<PathBuf> {
    if !path.is_absolute() || !dir.is_absolute() {
        return None;
    }
    let (path, dir) = (normalize(path), normalize(dir));
    let (mut p, mut d) = (path.components().peekable(), dir.components().peekable());
    if !same_root(p.peek(), d.peek()) {
        return None; // different drive on Windows
    }
    // A verbatim prefix and its ordinary spelling identify the same root, but are unequal
    // Components. Consume the checked root before comparing the remaining path components.
    p.next();
    d.next();
    while p.peek().is_some() && p.peek() == d.peek() {
        p.next();
        d.next();
    }
    let mut out: PathBuf = d.map(|_| Component::ParentDir).collect();
    out.extend(p);
    Some(out)
}

fn same_root(left: Option<&Component<'_>>, right: Option<&Component<'_>>) -> bool {
    #[cfg(windows)]
    if let (Some(Component::Prefix(left)), Some(Component::Prefix(right))) = (left, right) {
        use std::path::Prefix;
        return match (left.kind(), right.kind()) {
            (
                Prefix::Disk(a) | Prefix::VerbatimDisk(a),
                Prefix::Disk(b) | Prefix::VerbatimDisk(b),
            ) => a.eq_ignore_ascii_case(&b),
            (
                Prefix::UNC(a, b) | Prefix::VerbatimUNC(a, b),
                Prefix::UNC(c, d) | Prefix::VerbatimUNC(c, d),
            ) => {
                a.as_encoded_bytes()
                    .eq_ignore_ascii_case(c.as_encoded_bytes())
                    && b.as_encoded_bytes()
                        .eq_ignore_ascii_case(d.as_encoded_bytes())
            }
            (a, b) => a == b,
        };
    }
    left == right
}

/// `rel` resolved against `dir`, with `.` and `..` removed lexically.
pub fn rebase(rel: &Path, dir: &Path) -> PathBuf {
    normalize(&dir.join(rel))
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir if out.file_name().is_some() => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Unix-style absolute path, on drive `C:` under Windows.
    fn abs(p: &str) -> PathBuf {
        if cfg!(windows) {
            format!("C:{p}").into()
        } else {
            p.into()
        }
    }

    #[test]
    fn relativize_and_rebase() {
        let dir = &abs("/home/me/proj");
        let cases = [
            ("/home/me/proj/media/a.mkv", "media/a.mkv"),
            ("/home/me/clips/b.mkv", "../clips/b.mkv"),
            ("/c.mkv", "../../../c.mkv"),
        ];
        for (a, rel) in cases {
            assert_eq!(relativize(&abs(a), dir).unwrap(), Path::new(rel));
            assert_eq!(rebase(Path::new(rel), dir), abs(a));
        }
        assert_eq!(relativize(Path::new("a.mkv"), dir), None);
    }

    #[cfg(windows)]
    #[test]
    fn other_drive_keeps_only_absolute() {
        let dir = Path::new(r"C:\proj");
        assert_eq!(relativize(Path::new(r"D:\media\a.mkv"), dir), None);
        assert_eq!(relativize(Path::new(r"\\?\D:\media\a.mkv"), dir), None);
        let mut p = Project::default();
        p.add_asset(r"D:\media\a.mkv");
        assert!(!to_json(&p, dir).unwrap().contains("relative_path"));
    }

    #[cfg(windows)]
    #[test]
    fn ordinary_and_verbatim_windows_roots_are_equivalent() {
        for (source, directory, expected) in [
            (r"\\?\C:\proj\media\a.mkv", r"C:\proj", r"media\a.mkv"),
            (r"C:\proj\media\a.mkv", r"\\?\C:\proj", r"media\a.mkv"),
            (r"\\?\c:\clips\a.mkv", r"C:\proj", r"..\clips\a.mkv"),
            (
                r"\\?\UNC\server\share\proj\media\a.mkv",
                r"\\server\share\proj",
                r"media\a.mkv",
            ),
            (
                r"\\server\share\proj\media\a.mkv",
                r"\\?\UNC\SERVER\SHARE\proj",
                r"media\a.mkv",
            ),
        ] {
            assert_eq!(
                relativize(Path::new(source), Path::new(directory)),
                Some(expected.into())
            );
        }
        for (source, directory) in [
            (r"\\?\UNC\server\other\a.mkv", r"\\server\share\proj"),
            (r"\\?\UNC\other\share\a.mkv", r"\\server\share\proj"),
            (r"\\?\Volume{123}\a.mkv", r"C:\proj"),
            (r"\\.\C:\proj\a.mkv", r"C:\proj"),
        ] {
            assert_eq!(relativize(Path::new(source), Path::new(directory)), None);
        }
    }

    #[test]
    fn relative_links_resolve_directory_and_media_aliases_without_rewriting_paths() {
        let mut p = Project::default();
        let source = abs("/media-alias/proj/media/a.mkv");
        p.add_asset(source.clone());
        let dir = abs("/dir-alias/proj");
        let physical = abs("/physical");
        let s = to_json_with_paths(&p, &dir, |path| {
            for alias in [abs("/media-alias"), abs("/dir-alias")] {
                if let Ok(rest) = path.strip_prefix(alias) {
                    return physical.join(rest);
                }
            }
            path.to_path_buf()
        })
        .unwrap();
        assert!(s.contains(r#""relative_path": "media/a.mkv""#), "{s}");
        let value: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(
            value["project"]["assets"][0]["path"],
            source.to_str().unwrap()
        );
        let moved = abs("/moved");
        let loaded = from_json(&s, &moved, |_| true).unwrap();
        assert_eq!(loaded.assets()[0].path, moved.join("media/a.mkv"));
    }

    #[test]
    fn moved_folder_keeps_links() {
        let mut p = Project::default();
        p.add_asset(abs("/old/proj/media/a.mkv"));
        p.add_asset(abs("/library/b.mkv")); // outside the folder, not moved
        let s = to_json(&p, &abs("/old/proj")).unwrap();
        assert!(s.contains(r#""relative_path": "media/a.mkv""#));
        let moved = &abs("/new/place/proj");
        let on_disk = [moved.join("media/a.mkv"), abs("/library/b.mkv")];
        let loaded = from_json(&s, moved, |f| on_disk.iter().any(|d| d == f)).unwrap();
        let paths: Vec<_> = loaded.assets().iter().map(|a| a.path.clone()).collect();
        assert_eq!(paths, on_disk);
        // nothing resolves relatively: keep the stored absolute paths
        let loaded = from_json(&s, moved, |_| false).unwrap();
        assert_eq!(loaded, p);
    }

    #[test]
    fn rejects_bad_versions_and_rates() {
        let ok = to_json(&Project::default(), Path::new("/p")).unwrap();
        let load = |s: &str| from_json(s, Path::new("/p"), |_| true);
        assert_eq!(load(&ok).unwrap(), Project::default());
        let v2 = ok.replace("\"schema_version\": 1", "\"schema_version\": 2");
        assert!(matches!(load(&v2), Err(FileError::UnsupportedVersion(2))));
        let none = ok.replace("\"schema_version\"", "\"version\"");
        assert!(matches!(load(&none), Err(FileError::MissingVersion)));
        let zero = ok.replace("\"den\": 1", "\"den\": 0");
        assert!(matches!(load(&zero), Err(FileError::InvalidRate)));
        assert!(matches!(load("{"), Err(FileError::Json(_))));
    }
}
