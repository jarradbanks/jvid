//! Periodic autosave to sidecar files beside the project, a last save on panic, and finding a
//! newer autosave to offer on open.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jvid_core::history::History;
use jvid_core::model::Project;
use jvid_jobs::{Pool, Priority};

use crate::project_file;

/// A kill loses at most this much work.
pub const INTERVAL: Duration = Duration::from_secs(60);
/// Autosave versions kept per project; slot 1 is the newest.
pub const KEEP: usize = 3;

/// Where an untitled project autosaves, or `None` without a home dir.
pub fn untitled() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "jvid")
        .map(|d| d.data_dir().join("autosave").join("Untitled.jvid"))
}

/// `<project>.autosave<n>`, `n` in `1..=KEEP`.
pub fn slot(project: &Path, n: usize) -> PathBuf {
    let mut s = project.as_os_str().to_owned();
    s.push(format!(".autosave{n}"));
    PathBuf::from(s)
}

/// Shifts the older versions down a slot, then atomically writes slot 1.
pub fn write(project: &Project, path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    for n in (1..KEEP).rev() {
        let _ = std::fs::rename(slot(path, n), slot(path, n + 1));
    }
    project_file::save(project, &slot(path, 1))
}

/// The newest autosave if it is newer than `path` (or `path` doesn't exist).
pub fn newer(path: &Path) -> Option<PathBuf> {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let auto = slot(path, 1);
    let a = modified(&auto)?;
    modified(path).is_none_or(|p| a > p).then_some(auto)
}

/// Deletes every autosave of `path`.
pub fn clear(path: &Path) {
    for n in 1..=KEEP {
        let _ = std::fs::remove_file(slot(path, n));
    }
}

/// The project as of the last edit and where it autosaves; shared with the panic hook.
type Latest = Arc<Mutex<Option<(Project, PathBuf)>>>;

pub struct Autosave {
    pool: Pool,
    latest: Latest,
    /// Serialises writes between the job and the panic hook.
    writing: Arc<Mutex<()>>,
    /// The history revision last snapshotted into `latest`.
    rev: Option<u64>,
    /// When the oldest unwritten edit was snapshotted.
    pending_since: Option<f64>,
}

impl Default for Autosave {
    fn default() -> Self {
        let latest: Latest = Arc::default();
        let writing = Arc::new(Mutex::new(()));
        let (l, w) = (latest.clone(), writing.clone());
        jvid_log::on_panic(move || {
            // try_lock: the panic may be on the thread already holding either lock.
            let (Ok(_w), Ok(l)) = (w.try_lock(), l.try_lock()) else {
                return;
            };
            if let Some((project, path)) = &*l {
                let _ = write(project, path);
            }
        });
        Self {
            pool: Pool::new(1),
            latest,
            writing,
            rev: None,
            pending_since: None,
        }
    }
}

impl Autosave {
    /// Call every frame with the open project's autosave target; `now` in seconds. Returns how
    /// long until the next autosave is due, for a repaint.
    pub fn tick(&mut self, history: &History, target: Option<&Path>, now: f64) -> Option<Duration> {
        let Some(target) = target.filter(|_| history.is_dirty()) else {
            self.rev = None;
            self.pending_since = None;
            *self.latest.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return None;
        };
        if self.rev != Some(history.rev()) {
            self.rev = Some(history.rev());
            self.pending_since.get_or_insert(now);
            *self.latest.lock().unwrap_or_else(|e| e.into_inner()) =
                Some((history.project().clone(), target.to_owned()));
        }
        let since = self.pending_since?;
        let left = INTERVAL.as_secs_f64() - (now - since);
        if left > 0.0 {
            return Some(Duration::from_secs_f64(left));
        }
        self.pending_since = None;
        let (latest, writing) = (self.latest.clone(), self.writing.clone());
        self.pool.submit("autosave", Priority::Preview, move |_| {
            let _w = writing.lock().unwrap_or_else(|e| e.into_inner());
            let snap = latest.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some((project, path)) = snap
                && let Err(e) = write(&project, &path)
            {
                tracing::warn!("autosave to {} failed: {e}", path.display());
            }
        });
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::history::Command;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jvid-autosave-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn keeps_last_versions_and_finds_newer() {
        let d = dir("keep");
        let path = d.join("p.jvid");
        project_file::save(&Project::default(), &path).unwrap();
        assert_eq!(newer(&path), None);
        std::thread::sleep(Duration::from_millis(20));
        let mut h = History::new(Project::default());
        for i in 0..=KEEP {
            h.apply(Command::AddAsset(d.join(format!("{i}.mkv"))))
                .unwrap();
            write(h.project(), &path).unwrap();
        }
        assert_eq!(newer(&path), Some(slot(&path, 1)));
        assert_eq!(&project_file::load(&slot(&path, 1)).unwrap(), h.project());
        assert_eq!(
            project_file::load(&slot(&path, KEEP))
                .unwrap()
                .assets()
                .len(),
            2
        );
        assert!(!slot(&path, KEEP + 1).exists());
        clear(&path);
        assert_eq!(newer(&path), None);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn tick_writes_once_interval_passes() {
        let d = dir("tick");
        let path = d.join("p.jvid");
        let mut a = Autosave::default();
        let mut h = History::new(Project::default());
        assert_eq!(a.tick(&h, Some(&path), 0.0), None);
        h.apply(Command::AddAsset(d.join("a.mkv"))).unwrap();
        assert!(a.tick(&h, Some(&path), 1.0).is_some());
        assert!(!slot(&path, 1).exists());
        assert_eq!(a.tick(&h, Some(&path), 1.0 + INTERVAL.as_secs_f64()), None);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !slot(&path, 1).exists() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(&project_file::load(&slot(&path, 1)).unwrap(), h.project());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
