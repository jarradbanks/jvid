//! Reading and atomically saving `.jvid` files; the format lives in `jvid_core::file`.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use jvid_core::file::{FileError, from_json, to_json_with_paths};
use jvid_core::model::{MAX_NEST_DEPTH, Project};

use crate::i18n::tr;

pub fn load(path: &Path) -> io::Result<Project> {
    let s = fs::read_to_string(path)?;
    from_json(&s, &std::path::absolute(dir_of(path))?, |p| p.exists())
        .map_err(|e| io::Error::other(describe(&e)))
}

/// A file error in the UI language; jvid-core words it in English for logs.
fn describe(e: &FileError) -> String {
    match e {
        FileError::Json(e) => tr!("file.invalid", error = e),
        FileError::UnsupportedVersion(v) => tr!("file.newer", version = v),
        FileError::MissingVersion => tr!("file.no_version").to_owned(),
        FileError::InvalidRate => tr!("file.zero_rate").to_owned(),
        FileError::Cycle => tr!("file.cycle").to_owned(),
        FileError::TooDeep => tr!("file.too_deep", max = MAX_NEST_DEPTH),
        FileError::BadSequence => tr!("file.bad_sequence").to_owned(),
    }
}

/// Writes a temp file beside `path`, syncs it, then renames over `path`, so a
/// crash leaves either the old file or the new one, never half of each. A stale
/// temp file from a crashed save is truncated and reused.
pub fn save(project: &Project, path: &Path) -> io::Result<()> {
    let dir = dir_of(path);
    // Compare physical paths: Windows temp directories can use short names while imported
    // media is canonicalized. Resolve both sides, retaining lexical paths for offline media.
    let s = to_json_with_paths(project, &std::path::absolute(dir)?, |path| {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    })
    .map_err(io::Error::other)?;
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

    #[cfg(windows)]
    #[test]
    fn canonicalized_windows_import_survives_project_folder_move() {
        use crate::import::Importer;
        use std::time::{Duration, Instant};

        let root = std::env::temp_dir().join(format!("jvid-import-move-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let old = root.join("old");
        fs::create_dir_all(old.join("media")).unwrap();
        let media = old.join("media/take.wav");
        // A real 100 ms, 48 kHz mono PCM WAV, probed by the production importer.
        let size = 4_800u32 * 2;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + size).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&96_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&size.to_le_bytes());
        wav.resize(44 + size as usize, 0);
        fs::write(&media, wav).unwrap();
        jvid_media::ffmpeg::init().unwrap();
        let mut importer = Importer::new(jvid_jobs::Pool::new(1), |path| {
            jvid_media::probe::probe(path).map_err(|error| error.to_string())
        });
        importer.submit([media.clone()], &[]);
        let deadline = Instant::now() + Duration::from_secs(10);
        let batch = loop {
            let result = importer.poll();
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            if let Some(batch) = result.batch {
                break batch;
            }
            assert!(Instant::now() < deadline, "import did not finish");
            std::thread::sleep(Duration::from_millis(1));
        };
        drop(importer);
        assert_eq!(batch[0].0, fs::canonicalize(&media).unwrap());
        assert!(
            matches!(batch[0].0.components().next(), Some(std::path::Component::Prefix(prefix))
            if matches!(prefix.kind(), std::path::Prefix::VerbatimDisk(_) | std::path::Prefix::VerbatimUNC(_, _)))
        );
        let mut history = History::new(Project::default());
        history.apply(Command::Import(batch)).unwrap();
        save(history.project(), &old.join("p.jvid")).unwrap();
        let text = fs::read_to_string(old.join("p.jvid")).unwrap();
        assert!(
            text.contains(r#""relative_path": "media/take.wav""#),
            "{text}"
        );
        let moved = root.join("moved");
        fs::rename(&old, &moved).unwrap();
        let project = load(&moved.join("p.jvid")).unwrap();
        assert_eq!(project.assets()[0].path, moved.join("media/take.wav"));
        assert!(project.assets()[0].path.exists());
        fs::remove_dir_all(root).unwrap();
    }

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
