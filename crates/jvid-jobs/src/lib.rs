//! One place to run slow work (thumbnails, waveforms, proxies, exports) off the UI thread.
//!
//! Jobs are cooperative: a job polls [`Ctx::is_cancelled`] and reports through
//! [`Ctx::set_progress`]. Submitting never blocks on running work, so the UI thread can call
//! [`Pool::submit`] and read [`Handle`]s every frame.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

/// Lower variants run first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// Work the user is waiting on right now.
    Interactive,
    /// Thumbnails and waveforms.
    Preview,
    /// Proxy transcodes.
    Proxy,
}

#[derive(Default)]
struct State {
    cancelled: AtomicBool,
    finished: AtomicBool,
    progress: AtomicU32,
}

/// Shared view of a submitted job. Clones of a de-duplicated job share one state.
#[derive(Clone)]
pub struct Handle(Arc<State>);

impl Handle {
    /// Asks the job to stop. A queued job is skipped; a running one stops at its next poll.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Relaxed)
    }

    /// True once the job has returned or was skipped after cancellation.
    pub fn is_finished(&self) -> bool {
        self.0.finished.load(Ordering::Acquire)
    }

    /// Last reported progress in `0.0..=1.0`.
    pub fn progress(&self) -> f32 {
        f32::from_bits(self.0.progress.load(Ordering::Relaxed))
    }
}

/// What a running job sees. Clones share the job, for callbacks that must be `'static`.
#[derive(Clone)]
pub struct Ctx(Arc<State>);

impl Ctx {
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Relaxed)
    }

    pub fn set_progress(&self, p: f32) {
        self.0
            .progress
            .store(p.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

type Work = Box<dyn FnOnce(&Ctx) + Send>;

struct Queued {
    priority: Priority,
    seq: u64,
    key: String,
    state: Arc<State>,
    work: Work,
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}
impl Eq for Queued {}
impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Queued {
    // BinaryHeap pops the max, so the highest priority and then the oldest job must compare largest.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (Reverse(self.priority), Reverse(self.seq))
            .cmp(&(Reverse(other.priority), Reverse(other.seq)))
    }
}

#[derive(Default)]
struct Inner {
    queue: BinaryHeap<Queued>,
    /// Queued or running jobs by key, for de-duplication.
    live: HashMap<String, Arc<State>>,
    seq: u64,
    shutdown: bool,
}

#[derive(Default)]
struct Shared {
    inner: Mutex<Inner>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panicking job never holds the lock, so a poisoned lock still has consistent data.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A fixed set of worker threads pulling jobs in priority order.
pub struct Pool {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl Pool {
    /// Starts `threads` workers (at least one).
    pub fn new(threads: usize) -> Self {
        let shared = Arc::new(Shared::default());
        let workers = (0..threads.max(1))
            .map(|i| {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name(format!("jvid-job-{i}"))
                    .spawn(move || worker(&shared))
                    .expect("spawn job worker")
            })
            .collect();
        Self { shared, workers }
    }

    /// One worker per core, leaving one core for the UI.
    pub fn with_default_threads() -> Self {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        Self::new(cores.saturating_sub(1))
    }

    /// Queues `work` unless a live, uncancelled job has the same `key`, in which case that job's
    /// handle is returned and `work` is dropped.
    pub fn submit(
        &self,
        key: impl Into<String>,
        priority: Priority,
        work: impl FnOnce(&Ctx) + Send + 'static,
    ) -> Handle {
        let key = key.into();
        let mut inner = self.shared.lock();
        if let Some(state) = inner.live.get(&key)
            && !state.cancelled.load(Ordering::Relaxed)
        {
            return Handle(state.clone());
        }
        let state = Arc::new(State::default());
        inner.live.insert(key.clone(), state.clone());
        inner.seq += 1;
        let seq = inner.seq;
        inner.queue.push(Queued {
            priority,
            seq,
            key,
            state: state.clone(),
            work: Box::new(work),
        });
        drop(inner);
        self.shared.wake.notify_one();
        Handle(state)
    }

    /// No job is queued or running.
    pub fn is_idle(&self) -> bool {
        self.shared.lock().live.is_empty()
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let mut inner = self.shared.lock();
        inner.shutdown = true;
        for job in inner.queue.drain() {
            job.state.cancelled.store(true, Ordering::Relaxed);
            job.state.finished.store(true, Ordering::Release);
        }
        for state in inner.live.values() {
            state.cancelled.store(true, Ordering::Relaxed);
        }
        drop(inner);
        self.shared.wake.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn worker(shared: &Shared) {
    loop {
        let job = {
            let mut inner = shared.lock();
            loop {
                if inner.shutdown {
                    return;
                }
                if let Some(job) = inner.queue.pop() {
                    break job;
                }
                inner = shared.wake.wait(inner).unwrap_or_else(|e| e.into_inner());
            }
        };
        if !job.state.cancelled.load(Ordering::Relaxed) {
            let ctx = Ctx(job.state.clone());
            // A panicking job must not kill the worker or leave its handle unfinished forever.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (job.work)(&ctx)));
        }
        let mut inner = shared.lock();
        // A cancelled job may already have been replaced under the same key.
        if inner
            .live
            .get(&job.key)
            .is_some_and(|s| Arc::ptr_eq(s, &job.state))
        {
            inner.live.remove(&job.key);
        }
        drop(inner);
        job.state.finished.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn wait(h: &Handle) {
        let start = Instant::now();
        while !h.is_finished() {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "job never finished"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Occupies the single worker until the returned sender fires.
    fn block(pool: &Pool) -> (Handle, mpsc::Sender<()>) {
        let (tx, rx) = mpsc::channel::<()>();
        let (started_tx, started_rx) = mpsc::channel();
        let h = pool.submit("block", Priority::Interactive, move |_| {
            started_tx.send(()).unwrap();
            let _ = rx.recv();
        });
        started_rx.recv().unwrap();
        (h, tx)
    }

    #[test]
    fn idle_only_once_running_and_queued_jobs_finish() {
        let pool = Pool::new(1);
        assert!(pool.is_idle());
        let (blocker, tx) = block(&pool);
        let queued = pool.submit("queued", Priority::Preview, |_| {});
        assert!(!pool.is_idle());
        tx.send(()).unwrap();
        wait(&blocker);
        wait(&queued);
        assert!(pool.is_idle());
    }

    #[test]
    fn cancel_stops_running_job_within_100ms() {
        let pool = Pool::new(1);
        let (started_tx, started_rx) = mpsc::channel();
        let h = pool.submit("long", Priority::Proxy, move |ctx| {
            started_tx.send(()).unwrap();
            while !ctx.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        started_rx.recv().unwrap();
        let start = Instant::now();
        h.cancel();
        wait(&h);
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn cancelled_queued_job_never_runs() {
        let pool = Pool::new(1);
        let (_b, release) = block(&pool);
        let ran = Arc::new(AtomicBool::new(false));
        let r = ran.clone();
        let h = pool.submit("x", Priority::Preview, move |_| {
            r.store(true, Ordering::SeqCst)
        });
        h.cancel();
        release.send(()).unwrap();
        wait(&h);
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[test]
    fn runs_by_priority_then_submission_order() {
        let pool = Pool::new(1);
        let (b, release) = block(&pool);
        let order = Arc::new(Mutex::new(Vec::new()));
        let jobs = [
            ("p1", Priority::Proxy),
            ("t1", Priority::Preview),
            ("i1", Priority::Interactive),
            ("t2", Priority::Preview),
            ("i2", Priority::Interactive),
        ];
        let handles: Vec<_> = jobs
            .iter()
            .map(|&(k, p)| {
                let order = order.clone();
                pool.submit(k, p, move |_| order.lock().unwrap().push(k))
            })
            .collect();
        release.send(()).unwrap();
        wait(&b);
        handles.iter().for_each(wait);
        assert_eq!(*order.lock().unwrap(), ["i1", "i2", "t1", "t2", "p1"]);
    }

    #[test]
    fn identical_jobs_are_deduplicated() {
        let pool = Pool::new(1);
        let (_b, release) = block(&pool);
        let runs = Arc::new(AtomicU32::new(0));
        let submit = || {
            let runs = runs.clone();
            pool.submit("thumb:a.mp4", Priority::Preview, move |_| {
                runs.fetch_add(1, Ordering::SeqCst);
            })
        };
        let (a, b) = (submit(), submit());
        assert!(Arc::ptr_eq(&a.0, &b.0));
        release.send(()).unwrap();
        wait(&a);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        // Finished jobs leave the table, so the same key can run again later.
        wait(&submit());
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn resubmit_after_cancel_runs_fresh_job() {
        let pool = Pool::new(1);
        let (_b, release) = block(&pool);
        let old = pool.submit("k", Priority::Preview, |_| {});
        old.cancel();
        let new = pool.submit("k", Priority::Preview, |_| {});
        assert!(!Arc::ptr_eq(&old.0, &new.0));
        release.send(()).unwrap();
        wait(&new);
        assert!(!new.is_cancelled());
    }

    #[test]
    fn progress_is_reported_and_clamped() {
        let pool = Pool::new(1);
        let h = pool.submit("p", Priority::Preview, |ctx| ctx.set_progress(1.5));
        wait(&h);
        assert_eq!(h.progress(), 1.0);
    }

    #[test]
    fn panicking_job_finishes_and_worker_survives() {
        let pool = Pool::new(1);
        let h = pool.submit("boom", Priority::Preview, |_| panic!("boom"));
        wait(&h);
        wait(&pool.submit("after", Priority::Preview, |_| {}));
    }

    #[test]
    fn submit_does_not_block_while_eight_jobs_run() {
        let pool = Pool::new(8);
        let handles: Vec<_> = (0..8)
            .map(|i| {
                pool.submit(format!("busy{i}"), Priority::Proxy, |ctx| {
                    while !ctx.is_cancelled() {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                })
            })
            .collect();
        let start = Instant::now();
        for i in 0..1000 {
            pool.submit(format!("q{i}"), Priority::Preview, |_| {});
        }
        // 1000 submits must fit well inside one 16 ms frame.
        assert!(
            start.elapsed() < Duration::from_millis(16),
            "took {:?}",
            start.elapsed()
        );
        handles.iter().for_each(Handle::cancel);
    }
}
