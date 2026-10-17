//! Missing media: which asset paths are gone on load, and where relinking one finds the rest.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use jvid_core::history::Command;
use jvid_core::model::{AssetId, Project};

/// Asset paths with no file behind them. Keyed by path, so undoing a relink shows it offline again.
pub fn missing(project: &Project) -> HashSet<PathBuf> {
    let gone = project.assets().iter().filter(|a| !a.path.is_file());
    gone.map(|a| a.path.clone()).collect()
}

/// Relinks `id` to `chosen`, plus every other missing asset whose file name is in `chosen`'s
/// folder with the size it had when probed (any size if that's unknown).
// shortcut: only the chosen folder, not its subfolders; recurse if users keep media nested.
pub fn plan(
    project: &Project,
    missing: &HashSet<PathBuf>,
    id: AssetId,
    chosen: PathBuf,
) -> Command {
    let dir = chosen.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut items = vec![(id, chosen)];
    for a in project.assets() {
        if a.id == id || !missing.contains(&a.path) {
            continue;
        }
        let Some(name) = a.path.file_name() else {
            continue;
        };
        let candidate = dir.join(name);
        let size = std::fs::metadata(&candidate).ok().filter(|m| m.is_file());
        let want = a.probe.as_ref().and_then(|p| p.file_size);
        if size.is_some_and(|m| want.is_none_or(|w| w == m.len())) {
            items.push((a.id, candidate));
        }
    }
    Command::RelinkAssets(items)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use jvid_core::history::History;

    use super::*;
    use crate::project_file;

    /// The acceptance criterion: media moved to another folder, one relink finds the rest.
    #[test]
    fn relinking_one_moved_file_relinks_the_rest() {
        let Some(corpus) = jvid_media::test_media::corpus() else {
            return;
        };
        jvid_media::ffmpeg::init().unwrap();
        let root = std::env::temp_dir().join(format!("jvid-relink-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (old, new) = (root.join("shoot"), root.join("moved"));
        fs::create_dir_all(&old).unwrap();
        let names = [
            "no-audio.mp4",
            "tone-1khz.wav",
            "still.png",
            "tone-1khz.mp3",
        ];
        let mut items = Vec::new();
        for n in names {
            fs::copy(corpus.join(n), old.join(n)).unwrap();
            items.push((old.join(n), jvid_media::probe::probe(&old.join(n)).unwrap()));
        }
        let mut h = History::new(Project::default());
        h.apply(Command::Import(items)).unwrap();
        project_file::save(h.project(), &root.join("p.jvid")).unwrap();

        // The media moves without the project; the mp3 there is a different file of the same name.
        fs::rename(&old, &new).unwrap();
        fs::write(new.join("tone-1khz.mp3"), b"not the same file").unwrap();
        let mut h = History::new(project_file::load(&root.join("p.jvid")).unwrap());
        let gone = missing(h.project());
        assert_eq!(gone.len(), 4);

        let first = h.project().assets()[0].id;
        let cmd = plan(h.project(), &gone, first, new.join("no-audio.mp4"));
        h.apply(cmd).unwrap();
        let paths: Vec<_> = h
            .project()
            .assets()
            .iter()
            .map(|a| a.path.clone())
            .collect();
        assert_eq!(
            paths,
            [
                new.join("no-audio.mp4"),
                new.join("tone-1khz.wav"),
                new.join("still.png"),
                old.join("tone-1khz.mp3"),
            ]
        );
        assert!(
            h.project().assets()[..3]
                .iter()
                .all(|a| !gone.contains(&a.path))
        );
        h.undo();
        assert!(h.project().assets().iter().all(|a| gone.contains(&a.path)));
        fs::remove_dir_all(&root).unwrap();
    }
}
