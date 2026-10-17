//! Media import: probes run on job workers; a finished batch becomes one undoable `Import`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use jvid_core::model::ProbeInfo;
use jvid_jobs::{Pool, Priority};

pub type Probe = fn(&Path) -> Result<ProbeInfo, String>;

pub struct Importer {
    pool: Pool,
    probe: Probe,
    tx: Sender<(PathBuf, Result<ProbeInfo, String>)>,
    rx: Receiver<(PathBuf, Result<ProbeInfo, String>)>,
    pending: Vec<PathBuf>,
    done: Vec<(PathBuf, ProbeInfo)>,
}

/// What [`Importer::poll`] collected since the last call.
#[derive(Default)]
pub struct Polled {
    /// Every probe of the batch has finished; apply these as one `Command::Import`.
    pub batch: Option<Vec<(PathBuf, ProbeInfo)>>,
    pub errors: Vec<String>,
}

impl Importer {
    pub fn new(pool: Pool, probe: Probe) -> Self {
        let (tx, rx) = channel();
        Self {
            pool,
            probe,
            tx,
            rx,
            pending: Vec::new(),
            done: Vec::new(),
        }
    }

    pub fn media() -> Self {
        Self::new(Pool::with_default_threads(), |p| {
            jvid_media::probe::probe(p).map_err(|e| e.to_string())
        })
    }

    pub fn is_busy(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Queues probes and returns at once. Skips `existing` paths and ones already queued.
    pub fn submit(&mut self, paths: impl IntoIterator<Item = PathBuf>, existing: &[PathBuf]) {
        for path in paths {
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            if existing.contains(&path) || self.pending.contains(&path) {
                continue;
            }
            self.pending.push(path.clone());
            let (tx, probe) = (self.tx.clone(), self.probe);
            let key = format!("import:{}", path.display());
            self.pool.submit(key, Priority::Interactive, move |_| {
                let r = probe(&path);
                let _ = tx.send((path, r));
            });
        }
    }

    pub fn poll(&mut self) -> Polled {
        let mut out = Polled::default();
        for (path, r) in self.rx.try_iter() {
            self.pending.retain(|p| *p != path);
            match r {
                Ok(info) => self.done.push((path, info)),
                Err(e) => out
                    .errors
                    .push(format!("Couldn't import {}: {e}", name(&path))),
            }
        }
        if self.pending.is_empty() && !self.done.is_empty() {
            out.batch = Some(std::mem::take(&mut self.done));
        }
        out
    }
}

fn name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into(),
    )
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use jvid_core::history::{Command, History};
    use jvid_core::model::Project;

    use super::*;

    fn slow_probe(p: &Path) -> Result<ProbeInfo, String> {
        std::thread::sleep(Duration::from_millis(50));
        if p.extension().is_some_and(|e| e == "txt") {
            return Err("not media".into());
        }
        Ok(ProbeInfo::default())
    }

    fn wait(imp: &mut Importer) -> Polled {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut errors = Vec::new();
        loop {
            let mut p = imp.poll();
            errors.append(&mut p.errors);
            if !imp.is_busy() {
                return Polled { errors, ..p };
            }
            assert!(Instant::now() < deadline, "import never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn twenty_files_import_off_thread_as_one_undo_step() {
        let mut imp = Importer::new(Pool::new(2), slow_probe);
        let mut h = History::new(Project::default());
        let paths: Vec<_> = (0..20)
            .map(|i| PathBuf::from(format!("clip{i}.mkv")))
            .collect();

        let t = Instant::now();
        imp.submit(
            paths.iter().cloned().chain([PathBuf::from("clip0.mkv")]),
            &[],
        );
        // 20 probes at 50ms would take 1s on the calling thread.
        assert!(
            t.elapsed() < Duration::from_millis(100),
            "{:?}",
            t.elapsed()
        );
        assert!(imp.is_busy());

        let batch = wait(&mut imp).batch.unwrap();
        h.apply(Command::Import(batch)).unwrap();
        assert_eq!(h.project().assets().len(), 20);

        let existing: Vec<_> = h
            .project()
            .assets()
            .iter()
            .map(|a| a.path.clone())
            .collect();
        imp.submit(paths.clone(), &existing);
        assert!(!imp.is_busy(), "duplicates are skipped");

        assert!(h.undo());
        assert!(h.project().assets().is_empty());
    }

    #[test]
    fn failed_probe_reports_an_error_and_imports_the_rest() {
        let mut imp = Importer::new(Pool::new(2), slow_probe);
        imp.submit([PathBuf::from("a.mkv"), PathBuf::from("notes.txt")], &[]);
        let p = wait(&mut imp);
        assert_eq!(p.batch.unwrap().len(), 1);
        assert_eq!(p.errors, ["Couldn't import notes.txt: not media"]);
    }
}
