//! Media import: probes run on job workers; a finished batch becomes one undoable `Import`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use jvid_core::model::ProbeInfo;
use jvid_jobs::{Handle, Pool, Priority};
use jvid_media::probe::ProbeError;

use crate::i18n::tr;

pub type Probe = fn(&Path) -> Result<ProbeInfo, String>;

type ResultMessage = (u64, usize, PathBuf, Result<ProbeInfo, String>);

pub struct Importer {
    session: u64,
    handles: Vec<Handle>,
    pool: Pool,
    probe: Probe,
    tx: Sender<ResultMessage>,
    rx: Receiver<ResultMessage>,
    /// Accepted paths stay in submission order until the entire batch is drained.
    queued: Vec<PathBuf>,
    pending: usize,
    done: Vec<(usize, PathBuf, ProbeInfo)>,
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
            session: 0,
            handles: Vec::new(),
            pool,
            probe,
            tx,
            rx,
            queued: Vec::new(),
            pending: 0,
            done: Vec::new(),
        }
    }

    pub fn media() -> Self {
        Self::new(Pool::with_default_threads(), |p| {
            jvid_media::probe::probe(p).map_err(|e| describe(&e))
        })
    }

    /// Abandons queued probes without waiting for an uninterruptible running probe. A late
    /// result cannot clear a new session's pending path, even when the path is reused.
    pub fn reset_session(&mut self, session: u64) {
        self.session = session;
        for handle in self.handles.drain(..) {
            handle.cancel();
        }
        self.queued.clear();
        self.pending = 0;
        self.done.clear();
    }

    #[cfg(test)]
    pub fn jobs_idle(&self) -> bool {
        self.pool.is_idle()
    }

    pub fn is_busy(&self) -> bool {
        self.pending != 0
    }

    /// Queues probes and returns at once. Skips `existing` paths and ones already queued.
    pub fn submit(&mut self, paths: impl IntoIterator<Item = PathBuf>, existing: &[PathBuf]) {
        for path in paths {
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            if existing.contains(&path) || self.queued.contains(&path) {
                continue;
            }
            let ordinal = self.queued.len();
            self.queued.push(path.clone());
            self.pending += 1;
            let (tx, probe) = (self.tx.clone(), self.probe);
            let session = self.session;
            let key = format!("import:{session}:{}", path.display());
            let handle = self.pool.submit(key, Priority::Interactive, move |_| {
                let r = probe(&path);
                let _ = tx.send((session, ordinal, path, r));
            });
            self.handles.push(handle);
        }
    }

    pub fn poll(&mut self) -> Polled {
        let mut out = Polled::default();
        for (session, ordinal, path, r) in self.rx.try_iter() {
            if session != self.session {
                continue;
            }
            self.pending -= 1;
            match r {
                Ok(info) => self.done.push((ordinal, path, info)),
                Err(e) => out
                    .errors
                    .push(tr!("app.import_failed", name = name(&path), error = e)),
            }
        }
        self.handles.retain(|h| !h.is_finished());
        if self.pending == 0 {
            self.queued.clear();
            if !self.done.is_empty() {
                self.done.sort_unstable_by_key(|(ordinal, _, _)| *ordinal);
                out.batch = Some(
                    std::mem::take(&mut self.done)
                        .into_iter()
                        .map(|(_, path, info)| (path, info))
                        .collect(),
                );
            }
        }
        out
    }
}

/// Why a file isn't media, in the UI language; FFmpeg's own reason stays as it gives it.
pub fn describe(e: &ProbeError) -> String {
    match e {
        ProbeError::Unreadable(e) => tr!("app.unreadable", error = e),
        ProbeError::NoMedia => tr!("app.no_media").into(),
        ProbeError::UnsupportedCodec(c) => tr!("app.unsupported_codec", codec = c),
    }
}

fn name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into(),
    )
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::*;

    type Gate = (PathBuf, Receiver<Result<ProbeInfo, String>>);
    static GATES: Mutex<Vec<Gate>> = Mutex::new(Vec::new());
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    /// Real background jobs, released individually only after the test permits completion.
    pub struct GatedProbes {
        pub paths: Vec<PathBuf>,
        senders: Vec<Sender<Result<ProbeInfo, String>>>,
        handles: Vec<Handle>,
    }

    impl GatedProbes {
        pub fn new(names: &[&str]) -> Self {
            let root = std::env::temp_dir().join(format!(
                "jvid-gated-import-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let paths: Vec<_> = names.iter().map(|name| root.join(name)).collect();
            Self::for_paths(paths)
        }

        pub fn for_paths(paths: Vec<PathBuf>) -> Self {
            let mut gates = GATES.lock().unwrap();
            let senders = paths
                .iter()
                .map(|path| {
                    let (tx, rx) = channel();
                    gates.push((path.clone(), rx));
                    tx
                })
                .collect();
            Self {
                paths,
                senders,
                handles: Vec::new(),
            }
        }

        pub fn importer(&self) -> Importer {
            Importer::new(Pool::new(self.paths.len()), |path| {
                let rx = {
                    let mut gates = GATES.lock().unwrap();
                    let index = gates
                        .iter()
                        .position(|(p, _)| p == path)
                        .expect("probe gate");
                    gates.swap_remove(index).1
                };
                rx.recv_timeout(Duration::from_secs(10))
                    .expect("probe never released")
            })
        }

        pub fn capture_jobs(&mut self, importer: &Importer) {
            assert_eq!(importer.handles.len(), self.paths.len());
            self.handles = importer.handles.clone();
            let until = Instant::now() + Duration::from_secs(5);
            while GATES
                .lock()
                .unwrap()
                .iter()
                .any(|(p, _)| self.paths.contains(p))
            {
                assert!(Instant::now() < until, "probes did not start");
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        pub fn finish(&self, index: usize, result: Result<ProbeInfo, String>) {
            self.senders[index].send(result).expect("probe timed out");
            // Waiting for the job handle also guarantees the importer result was sent.
            let until = Instant::now() + Duration::from_secs(5);
            while !self.handles[index].is_finished() {
                assert!(Instant::now() < until, "probe did not finish");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    impl Drop for GatedProbes {
        fn drop(&mut self) {
            GATES
                .lock()
                .unwrap()
                .retain(|(p, _)| !self.paths.contains(p));
        }
    }
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

    #[test]
    fn audit_462_reversed_probes_preserve_batch_and_added_order() {
        let mut gates = test_support::GatedProbes::new(&["z.mkv", "bad.txt", "a.mkv"]);
        let mut imp = gates.importer();
        let existing = PathBuf::from("already-imported.mkv");
        imp.submit(
            [
                gates.paths[0].clone(),
                gates.paths[1].clone(),
                gates.paths[0].clone(),
                existing.clone(),
                gates.paths[2].clone(),
            ],
            &[existing],
        );
        gates.capture_jobs(&imp);
        assert!(imp.is_busy(), "submit returns while every probe is gated");
        for index in [2, 1] {
            let result = if index == 1 {
                Err("not media".into())
            } else {
                Ok(ProbeInfo::default())
            };
            gates.finish(index, result);
            let polled = imp.poll();
            assert!(polled.batch.is_none(), "no partial asset batch");
            assert_eq!(polled.errors.len(), usize::from(index == 1));
            assert!(imp.is_busy());
            let jobs = imp.handles.len();
            imp.submit([gates.paths[index].clone()], &[]);
            assert_eq!(
                imp.handles.len(),
                jobs,
                "completed batch paths stay deduplicated"
            );
        }
        gates.finish(0, Ok(ProbeInfo::default()));
        let batch = imp.poll().batch.unwrap();
        let expected = [&gates.paths[0], &gates.paths[2]];
        assert_eq!(batch.iter().map(|(p, _)| p).collect::<Vec<_>>(), expected);
        let mut h = History::new(Project::default());
        h.apply(Command::Import(batch)).unwrap();
        assert_eq!(
            crate::media_bin::visible(h.project().assets(), "", 0)
                .iter()
                .map(|a| &a.path)
                .collect::<Vec<_>>(),
            expected,
            "Added uses submitted order, rather than name or completion order"
        );
        assert!(h.undo());
        assert!(
            h.project().assets().is_empty(),
            "one undo removes the batch"
        );
        assert!(!h.undo());
    }

    #[test]
    fn audit_462_all_failed_batch_clears_paths_for_the_next_submission() {
        let mut imp = Importer::new(Pool::new(1), slow_probe);
        for _ in 0..2 {
            imp.submit([PathBuf::from("bad.txt")], &[]);
            assert!(imp.is_busy());
            let result = wait(&mut imp);
            assert!(result.batch.is_none());
            assert_eq!(result.errors.len(), 1);
            let until = Instant::now() + Duration::from_secs(5);
            while !imp.jobs_idle() {
                assert!(Instant::now() < until, "probe worker did not finish");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    #[test]
    fn audit_462_abandoned_result_cannot_finish_same_path_in_new_session() {
        let mut old = test_support::GatedProbes::new(&["reused.mkv"]);
        let mut imp = old.importer();
        imp.submit(old.paths.clone(), &[]);
        old.capture_jobs(&imp);
        imp.reset_session(1);
        let mut fresh = test_support::GatedProbes::for_paths(old.paths.clone());
        imp.submit(fresh.paths.clone(), &[]);
        old.finish(
            0,
            Ok(ProbeInfo {
                width: Some(111),
                ..Default::default()
            }),
        );
        let stale = imp.poll();
        assert!(stale.batch.is_none() && stale.errors.is_empty());
        assert!(
            imp.is_busy(),
            "old ordinal zero cannot finish new ordinal zero"
        );
        fresh.capture_jobs(&imp);
        fresh.finish(
            0,
            Ok(ProbeInfo {
                width: Some(222),
                ..Default::default()
            }),
        );
        let batch = imp.poll().batch.unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].0, fresh.paths[0]);
        assert_eq!(batch[0].1.width, Some(222));
        assert!(!imp.is_busy());
    }
}
