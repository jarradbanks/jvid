//! Plain text from the displayed, timeline-placed words, and safe text-file export.
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use jvid_core::model::{ClipId, Project};
use jvid_transcribe::Word;

use crate::i18n::tr;

/// Match the transcript's paragraph spacing while avoiding spaces before punctuation.
pub(super) fn plain(words: &[(ClipId, Word)]) -> String {
    let mut output = String::new();
    let mut previous = None;
    for (_, word) in words {
        let text = word.text.trim();
        if text.is_empty() {
            continue;
        }
        if let Some(end) = previous {
            if word.start - end >= super::PARAGRAPH {
                output.push_str("\n\n");
            } else if !text.starts_with([
                ',', '.', '!', '?', ':', ';', ')', ']', '}', '%', '。', '，', '！', '？', '、',
            ]) && !output.ends_with(['(', '[', '{', '¿', '¡'])
            {
                output.push(' ');
            }
        }
        output.push_str(text);
        previous = Some(word.end);
    }
    output
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b || same_file::is_same_file(a, b).unwrap_or(false)
}

/// The save dialog confirms replacement. Publish a complete UTF-8 file atomically;
/// project/source paths and non-text extensions can never become text outputs.
pub(crate) fn export(
    path: &Path,
    text: &str,
    project: &Project,
    project_path: Option<&Path>,
) -> io::Result<()> {
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_none_or(|e| !e.eq_ignore_ascii_case("txt"))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            tr!("transcript.txt_required"),
        ));
    }
    if project_path.is_some_and(|p| same_file(path, p))
        || project
            .assets()
            .iter()
            .any(|asset| same_file(path, &asset.path))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            tr!("transcript.protected_destination"),
        ));
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                tr!("export.sidecar_file_target"),
            ));
        }
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary = parent.join(format!(
        ".jvid-transcript-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(text.as_bytes())?;
        if !text.ends_with('\n') {
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::time::{FLICKS_PER_SECOND as S, Time};

    fn word(text: &str, at: i64) -> (ClipId, Word) {
        (
            ClipId(1),
            Word {
                text: text.into(),
                start: Time(at * S),
                end: Time(at * S + S / 2),
                confidence: 1.0,
            },
        )
    }

    #[test]
    fn readable_unicode_punctuation_paragraphs_and_selected_range() {
        let words = vec![
            word("¡", 0),
            word(" Hola", 0),
            word(",", 0),
            word(" mundo", 0),
            word("!", 0),
            word("こんにちは。", 3),
        ];
        assert_eq!(plain(&words), "¡Hola, mundo!\n\nこんにちは。");
        assert_eq!(plain(&words[1..4]), "Hola, mundo");
        assert_eq!(plain(&[]), "");
    }

    #[test]
    fn utf8_export_replaces_text_and_protects_projects_and_source_aliases() {
        let root = std::env::temp_dir().join(format!(
            "jvid-transcript-text-export-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("transcript.txt");
        fs::write(&path, "old").unwrap();
        let project = Project::default();
        export(&path, "Héllo 世界", &project, None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "Héllo 世界\n");
        assert!(export(&path, "changed", &project, Some(&path)).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "Héllo 世界\n");
        let protected = root.join("project.jvid");
        fs::write(&protected, "project data").unwrap();
        assert!(export(&protected, "changed", &project, None).is_err());
        assert_eq!(fs::read_to_string(&protected).unwrap(), "project data");
        #[cfg(unix)]
        {
            let alias = root.join("alias.txt");
            std::os::unix::fs::symlink(&protected, &alias).unwrap();
            assert!(export(&alias, "changed", &project, Some(&protected)).is_err());
            assert_eq!(fs::read_to_string(&protected).unwrap(), "project data");
        }
        let hardlink = root.join("hardlink.txt");
        fs::hard_link(&protected, &hardlink).unwrap();
        assert!(export(&hardlink, "changed", &project, Some(&protected)).is_err());
        assert_eq!(fs::read_to_string(&protected).unwrap(), "project data");
        let destination = root.join("directory.txt");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("keep"), "keep").unwrap();
        assert!(export(&destination, "changed", &project, None).is_err());
        assert_eq!(
            fs::read_to_string(destination.join("keep")).unwrap(),
            "keep"
        );
        assert!(!fs::read_dir(&root).unwrap().any(|p| {
            p.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".jvid-transcript-")
        }));
        fs::remove_dir_all(root).unwrap();
    }
}
