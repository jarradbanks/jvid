//! Reading and atomically saving `.jvid` files; the format lives in `jvid_core::file`.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use jvid_core::file::{from_json, to_json};
use jvid_core::model::Project;

pub fn load(path: &Path) -> io::Result<Project> {
    let s = fs::read_to_string(path)?;
    from_json(&s, &std::path::absolute(dir_of(path))?, |p| p.exists()).map_err(io::Error::other)
}

/// Writes a temp file beside `path`, syncs it, then renames over `path`, so a
/// crash leaves either the old file or the new one, never half of each. A stale
/// temp file from a crashed save is truncated and reused.
pub fn save(project: &Project, path: &Path) -> io::Result<()> {
    let dir = dir_of(path);
    // Not canonicalize: on Windows it adds a `\\?\` prefix that file-dialog asset
    // paths lack, so nothing would relativize.
    let s = to_json(project, &std::path::absolute(dir)?).map_err(io::Error::other)?;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = dir.join(name);
    let mut f = File::create(&tmp)?;
    f.write_all(s.as_bytes())?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)?;
    // Persist the rename itself; Windows has no directory handles to sync.
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    Ok(())
}

fn dir_of(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::history::{Command, History};

    #[test]
    fn moved_folder_keeps_media_links() {
        let root = std::env::temp_dir().join(format!("jvid-move-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let old = root.join("old");
        fs::create_dir_all(old.join("media")).unwrap();
        fs::write(old.join("media/a.mkv"), b"x").unwrap();
        fs::write(old.join("p.jvid.tmp"), b"stale from a crashed save").unwrap();

        // Plain paths, as a file dialog gives them (not canonicalized).
        let mut h = History::new(Project::default());
        h.apply(Command::AddAsset(old.join("media/a.mkv"))).unwrap();
        save(h.project(), &old.join("p.jvid")).unwrap();
        assert!(!old.join("p.jvid.tmp").exists());
        let text = fs::read_to_string(old.join("p.jvid")).unwrap();
        assert!(text.contains(r#""relative_path": "media/a.mkv""#), "{text}");
        assert_eq!(&load(&old.join("p.jvid")).unwrap(), h.project());

        let new = root.join("new");
        fs::rename(&old, &new).unwrap();
        let p = load(&new.join("p.jvid")).unwrap();
        assert_eq!(p.assets()[0].path, new.join("media/a.mkv"));
        assert!(p.assets()[0].path.exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
