//! One decode per source generation, with replay for every interested panel.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};

use egui::{ColorImage, Context, TextureHandle};
use jvid_core::time::Time;
use jvid_jobs::{Handle, Priority};

use super::{Caches, color_image};

#[cfg(test)]
thread_local! {
    static METADATA_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static GENERATION_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn metadata_reads() -> usize {
    METADATA_READS.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn generation_checks() -> usize {
    GENERATION_CHECKS.with(std::cell::Cell::get)
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Source {
    path: PathBuf,
    stamp: Option<(u64, Option<SystemTime>)>,
}

impl Source {
    fn new(path: &Path) -> Self {
        #[cfg(test)]
        METADATA_READS.with(|reads| reads.set(reads.get() + 1));
        Self {
            path: path.to_owned(),
            stamp: std::fs::metadata(path)
                .ok()
                .map(|m| (m.len(), m.modified().ok())),
        }
    }
}

/// Shared in-memory identity. Only workers read the filesystem; painters compare a version.
#[derive(Default)]
struct Generation {
    source: Mutex<Option<Source>>,
    version: AtomicU64,
    subscribers: Mutex<HashMap<u64, Context>>,
    next_subscriber: AtomicU64,
    refreshing: AtomicBool,
    refresh_pass: Mutex<Option<(Context, u64)>>,
    #[cfg(test)]
    metadata_reads: AtomicU64,
}

impl Generation {
    fn read_source(&self, path: &Path) -> Source {
        // Serialize observations so concurrent request/refresh jobs cannot publish stamps
        // out of order. Paint never takes this lock or performs this I/O.
        let mut previous = self.source.lock().unwrap();
        #[cfg(test)]
        self.metadata_reads.fetch_add(1, Ordering::Relaxed);
        let source = Source::new(path);
        if previous.as_ref().is_some_and(|old| *old != source) {
            self.version.fetch_add(1, Ordering::Release);
            for ctx in self.subscribers.lock().unwrap().values() {
                ctx.request_repaint();
            }
        }
        *previous = Some(source.clone());
        source
    }
}

// Keep invalidation delivery alive after decode subscriptions have released their CPU frames.
struct SourceSubscription {
    generation: Arc<Generation>,
    version: u64,
    id: u64,
}

impl Drop for SourceSubscription {
    fn drop(&mut self) {
        self.generation.subscribers.lock().unwrap().remove(&self.id);
    }
}

struct Refresh(Arc<Generation>);

impl Drop for Refresh {
    fn drop(&mut self) {
        self.0.refreshing.store(false, Ordering::Release);
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Terminal {
    Success,
    // Decode errors are logged by the worker; consumers only need the classification.
    Failed,
    Cancelled,
}

#[derive(Default)]
struct ResultState {
    // At most STRIP_MAX frames. Consumers release these CPU images after uploading textures.
    frames: Vec<Arc<ColorImage>>,
    terminal: Option<Terminal>,
    subscribers: HashMap<u64, Context>,
    next_subscriber: u64,
    job: Option<Handle>,
}

#[derive(Default)]
struct Request(Mutex<ResultState>);

impl Request {
    fn frame(&self, image: &image::RgbaImage) {
        let mut state = self.0.lock().unwrap();
        state.frames.push(Arc::new(color_image(image)));
        for ctx in state.subscribers.values() {
            ctx.request_repaint();
        }
    }

    fn finish(&self, terminal: Terminal) {
        let mut state = self.0.lock().unwrap();
        if state.terminal.is_none() {
            state.terminal = Some(terminal);
            for ctx in state.subscribers.values() {
                ctx.request_repaint();
            }
        }
    }
}

// Also runs when Pool skips a cancelled queued closure, or the decoder panics. Neither case
// may strand a subscriber, and terminal notifications always schedule a repaint.
struct Completion(Arc<Request>);

impl Drop for Completion {
    fn drop(&mut self) {
        self.0.finish(if std::thread::panicking() {
            Terminal::Failed
        } else {
            Terminal::Cancelled
        });
    }
}

#[derive(Default)]
pub(crate) struct Filmstrips {
    // Weak ownership: completed strips are kept on disk, not permanently in CPU memory.
    requests: Mutex<HashMap<(PathBuf, u64), Weak<Request>>>,
    sources: Mutex<HashMap<PathBuf, Weak<Generation>>>,
    sequence: AtomicU64,
    refresh_sequence: AtomicU64,
}

impl Filmstrips {
    fn source(&self, path: &Path, ctx: &Context) -> SourceSubscription {
        let mut sources = self.sources.lock().unwrap();
        sources.retain(|_, generation| generation.strong_count() > 0);
        let generation = sources
            .get(path)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let generation = Arc::new(Generation::default());
                sources.insert(path.to_owned(), Arc::downgrade(&generation));
                generation
            });
        let id = generation.next_subscriber.fetch_add(1, Ordering::Relaxed);
        generation
            .subscribers
            .lock()
            .unwrap()
            .insert(id, ctx.clone());
        SourceSubscription {
            version: generation.version.load(Ordering::Acquire),
            generation,
            id,
        }
    }

    /// Explicit request/invalidation boundary, coalesced across panels and never a paint timer.
    /// Completed and pending consumers both receive a repaint if the observed source changes.
    pub(crate) fn refresh_source(&self, caches: &Caches, path: &Path, ctx: &Context) {
        let Some(generation) = self
            .sources
            .lock()
            .unwrap()
            .get(path)
            .and_then(Weak::upgrade)
        else {
            return;
        };
        let pass = (ctx.clone(), ctx.cumulative_pass_nr());
        let mut previous = generation.refresh_pass.lock().unwrap();
        if previous.as_ref() == Some(&pass) || generation.refreshing.swap(true, Ordering::AcqRel) {
            return;
        }
        *previous = Some(pass);
        let refresh = Refresh(generation.clone());
        let path = path.to_owned();
        let seq = self.refresh_sequence.fetch_add(1, Ordering::Relaxed);
        caches.jobs.submit(
            format!("filmstrip-refresh:{seq}"),
            Priority::Preview,
            move |_| {
                refresh.0.read_source(&path);
            },
        );
    }

    fn subscribe(
        &self,
        caches: &Caches,
        path: &Path,
        source: &SourceSubscription,
        duration: Time,
        ctx: &Context,
    ) -> Subscription {
        let mut requests = self.requests.lock().unwrap();
        requests.retain(|_, request| request.strong_count() > 0);
        let key = (path.to_owned(), source.version);
        let existing = requests
            .get(&key)
            .and_then(Weak::upgrade)
            .filter(|request| {
                !matches!(
                    request.0.lock().unwrap().terminal,
                    Some(Terminal::Failed | Terminal::Cancelled)
                )
            });
        let (request, fresh) = match existing {
            Some(request) => (request, false),
            None => {
                let request = Arc::new(Request::default());
                requests.insert(key, Arc::downgrade(&request));
                (request, true)
            }
        };
        let id = {
            let mut state = request.0.lock().unwrap();
            state.next_subscriber += 1;
            let id = state.next_subscriber;
            state.subscribers.insert(id, ctx.clone());
            id
        };
        if fresh {
            let completion = Completion(request.clone());
            let cache = caches.thumbs.clone();
            let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
            let generation = source.generation.clone();
            let version = source.version;
            let path = path.to_owned();
            // Registry ownership guarantees one decoder; Pool deduplication is not delivery.
            // A unique key also permits retry after terminal delivery but before Pool cleanup.
            let handle =
                caches
                    .jobs
                    .submit(format!("filmstrip:{seq}"), Priority::Preview, move |job| {
                        let source = generation.read_source(&path);
                        let cancelled = || {
                            job.is_cancelled()
                                || generation.version.load(Ordering::Acquire) != version
                                || generation.read_source(&path) != source
                        };
                        let mut sent = 0;
                        let result = cache.filmstrip_until(
                            &source.path,
                            duration,
                            |image| {
                                completion.0.frame(image);
                                sent += 1;
                            },
                            cancelled,
                        );
                        let terminal = if cancelled() {
                            Terminal::Cancelled
                        } else {
                            match result {
                                Ok(frames) => {
                                    // Cache hits don't call progress. Deliver them through the same state.
                                    for image in &frames[sent..] {
                                        completion.0.frame(image);
                                    }
                                    Terminal::Success
                                }
                                Err(error) => {
                                    tracing::warn!(
                                        "filmstrip for {}: {error}",
                                        source.path.display()
                                    );
                                    Terminal::Failed
                                }
                            }
                        };
                        completion.0.finish(terminal);
                    });
            request.0.lock().unwrap().job = Some(handle);
        }
        Subscription {
            request,
            id,
            cursor: 0,
        }
    }
}

struct Subscription {
    request: Arc<Request>,
    id: u64,
    cursor: usize,
}

impl Subscription {
    fn poll(&mut self) -> (Vec<Arc<ColorImage>>, Option<Terminal>) {
        let state = self.request.0.lock().unwrap();
        let frames = state.frames[self.cursor..].to_vec();
        self.cursor = state.frames.len();
        (frames, state.terminal.clone())
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut state = self.request.0.lock().unwrap();
        state.subscribers.remove(&self.id);
        // Dropping one panel must never cancel work another panel still needs. Keep the
        // cancelled flight registered until it stops, so a new subscriber cannot race a writer.
        if state.subscribers.is_empty()
            && state.terminal.is_none()
            && let Some(job) = &state.job
        {
            job.cancel();
        }
    }
}

// Retry on later UI activity, without a timer repeatedly decoding a broken source.
pub(crate) const RETRY_DELAY: Duration = Duration::from_millis(250);

/// Panel-local subscriptions and generation stamps, separate from reusable GPU textures.
#[derive(Default)]
pub(crate) struct FilmstripRequests {
    pending: HashMap<PathBuf, Subscription>,
    sources: HashMap<PathBuf, SourceSubscription>,
    retry_after: HashMap<PathBuf, f64>,
    polled_pass: Option<(Context, u64)>,
    focused: Option<bool>,
}

impl FilmstripRequests {
    /// Returning from another application is a bounded source-refresh boundary. Do this
    /// once at panel entry, not for every visible clip, and share checks with the bin.
    pub(crate) fn on_focus(&mut self, ctx: &Context, caches: Option<&Caches>) {
        let focused = ctx.input(|input| input.focused);
        if self.focused.replace(focused) == Some(false)
            && focused
            && let Some(caches) = caches
        {
            for path in self.sources.keys() {
                caches.filmstrips.refresh_source(caches, path, ctx);
            }
        }
    }

    pub(crate) fn refresh(
        &mut self,
        ctx: &Context,
        caches: Option<&Caches>,
        path: &Path,
        duration: Time,
        strips: &mut HashMap<PathBuf, Option<Vec<TextureHandle>>>,
        subscribe: bool,
    ) {
        let Some(caches) = caches else { return };
        if subscribe
            && !strips.contains_key(path)
            && self.retry_after.get(path).is_none_or(|at| {
                // Completion requests multiple egui passes. At a low frame rate those
                // passes can cross the backoff and sustain their own retry loop. Let
                // them drain; input or a later paint after idle can start another try.
                ctx.input(|i| i.time >= *at)
                    && (ctx.input(|i| !i.events.is_empty()) || !ctx.requested_repaint_last_pass())
            })
        {
            let source = self
                .sources
                .entry(path.to_owned())
                .or_insert_with(|| caches.filmstrips.source(path, ctx));
            let mut subscription = caches
                .filmstrips
                .subscribe(caches, path, source, duration, ctx);
            self.retry_after.remove(path);
            strips.insert(path.to_owned(), None);
            // A late subscriber can replay a finished request in this same pass. Ordinary
            // paints drain each pending source only once, regardless of its clip count.
            if Self::receive(ctx, path, &mut subscription, strips, &mut self.retry_after) {
                self.pending.insert(path.to_owned(), subscription);
            }
        }
    }

    /// Compare in-memory generations and drain pending results once per panel/pass.
    pub(crate) fn poll(
        &mut self,
        ctx: &Context,
        strips: &mut HashMap<PathBuf, Option<Vec<TextureHandle>>>,
    ) {
        let pass = (ctx.clone(), ctx.cumulative_pass_nr());
        if self.polled_pass.as_ref() == Some(&pass) {
            return;
        }
        self.polled_pass = Some(pass);
        for (path, source) in &mut self.sources {
            #[cfg(test)]
            GENERATION_CHECKS.with(|checks| checks.set(checks.get() + 1));
            let version = source.generation.version.load(Ordering::Acquire);
            if source.version != version {
                source.version = version;
                self.pending.remove(path);
                strips.remove(path);
                self.retry_after.remove(path);
            }
        }
        self.pending.retain(|path, subscription| {
            Self::receive(ctx, path, subscription, strips, &mut self.retry_after)
        });
    }

    fn receive(
        ctx: &Context,
        path: &Path,
        subscription: &mut Subscription,
        strips: &mut HashMap<PathBuf, Option<Vec<TextureHandle>>>,
        retry_after: &mut HashMap<PathBuf, f64>,
    ) -> bool {
        let (frames, terminal) = subscription.poll();
        if matches!(terminal, Some(Terminal::Failed | Terminal::Cancelled)) {
            strips.remove(path);
            retry_after.insert(
                path.to_owned(),
                ctx.input(|i| i.time) + RETRY_DELAY.as_secs_f64(),
            );
            return false;
        }
        if !frames.is_empty() {
            let textures = strips
                .entry(path.to_owned())
                .or_default()
                .get_or_insert_default();
            for frame in frames {
                let name = format!("{}#{}", path.display(), textures.len());
                textures.push(ctx.load_texture(name, (*frame).clone(), Default::default()));
            }
        }
        terminal.is_none()
    }

    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
impl Filmstrips {
    pub(crate) fn requests_started(&self) -> u64 {
        self.sequence.load(Ordering::Relaxed)
    }

    pub(crate) fn metadata_reads(&self) -> u64 {
        self.sources
            .lock()
            .unwrap()
            .values()
            .filter_map(Weak::upgrade)
            .map(|generation| generation.metadata_reads.load(Ordering::Relaxed))
            .sum()
    }

    pub(crate) fn refreshes_started(&self) -> u64 {
        self.refresh_sequence.load(Ordering::Relaxed)
    }

    pub(crate) fn settled(&self) -> bool {
        self.requests
            .lock()
            .unwrap()
            .values()
            .filter_map(Weak::upgrade)
            .all(|request| request.0.lock().unwrap().terminal.is_some())
    }

    pub(crate) fn job(&self, path: &Path) -> Handle {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|((p, _), _)| p == path)
            .filter_map(|((_, version), request)| {
                request.upgrade().map(|request| (*version, request))
            })
            .max_by_key(|(version, _)| *version)
            .unwrap()
            .1
            .0
            .lock()
            .unwrap()
            .job
            .clone()
            .unwrap()
    }

    pub(crate) fn retained(&self) -> (usize, usize) {
        let requests = self.requests.lock().unwrap();
        requests.values().filter_map(Weak::upgrade).fold(
            (0, 0),
            |(frames, subscribers), request| {
                let state = request.0.lock().unwrap();
                (
                    frames + state.frames.len(),
                    subscribers + state.subscribers.len(),
                )
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_unwind_delivers_failure_and_wakes_the_subscriber() {
        let request = Arc::new(Request::default());
        let ctx = Context::default();
        let wakes = Arc::new(AtomicU64::new(0));
        let count = wakes.clone();
        ctx.set_request_repaint_callback(move |_| {
            count.fetch_add(1, Ordering::Relaxed);
        });
        request.0.lock().unwrap().subscribers.insert(1, ctx);
        let mut subscriber = Subscription {
            request: request.clone(),
            id: 1,
            cursor: 0,
        };
        let completion = Completion(request.clone());
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _completion = completion;
            panic!("injected decoder panic");
        }));
        assert!(unwind.is_err());
        assert_eq!(subscriber.poll(), (Vec::new(), Some(Terminal::Failed)));
        assert!(wakes.load(Ordering::Relaxed) > 0);
        drop(subscriber);
        assert!(request.0.lock().unwrap().subscribers.is_empty());
    }
}
