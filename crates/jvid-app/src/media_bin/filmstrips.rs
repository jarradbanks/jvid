//! One decode per source generation, with replay for every interested panel.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};

use egui::{ColorImage, Context, TextureHandle};
use jvid_core::time::Time;
use jvid_jobs::{Handle, Priority};

use super::{Caches, color_image};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Source {
    path: PathBuf,
    stamp: Option<(u64, Option<SystemTime>)>,
}

impl Source {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            stamp: std::fs::metadata(path)
                .ok()
                .map(|m| (m.len(), m.modified().ok())),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Terminal {
    Success,
    Failed(String),
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
            Terminal::Failed("filmstrip decoder panicked".into())
        } else {
            Terminal::Cancelled
        });
    }
}

#[derive(Default)]
pub(crate) struct Filmstrips {
    // Weak ownership: completed strips are kept on disk, not permanently in CPU memory.
    requests: Mutex<HashMap<Source, Weak<Request>>>,
    sequence: std::sync::atomic::AtomicU64,
}

impl Filmstrips {
    fn subscribe(
        &self,
        caches: &Caches,
        source: Source,
        duration: Time,
        ctx: &Context,
    ) -> Subscription {
        let mut requests = self.requests.lock().unwrap();
        requests.retain(|_, request| request.strong_count() > 0);
        let existing = requests
            .get(&source)
            .and_then(Weak::upgrade)
            .filter(|request| {
                !matches!(
                    request.0.lock().unwrap().terminal,
                    Some(Terminal::Failed(_) | Terminal::Cancelled)
                )
            });
        let (request, fresh) = match existing {
            Some(request) => (request, false),
            None => {
                let request = Arc::new(Request::default());
                requests.insert(source.clone(), Arc::downgrade(&request));
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
            let seq = self
                .sequence
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // Registry ownership guarantees one decoder; Pool deduplication is not delivery.
            // A unique key also permits retry after terminal delivery but before Pool cleanup.
            let handle =
                caches
                    .jobs
                    .submit(format!("filmstrip:{seq}"), Priority::Preview, move |job| {
                        let cancelled =
                            || job.is_cancelled() || Source::new(&source.path) != source;
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
                                    Terminal::Failed(error)
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

// Retry on later input/repaint, without a timer repeatedly decoding a broken source.
pub(crate) const RETRY_DELAY: Duration = Duration::from_millis(250);

/// Panel-local subscriptions and generation stamps, separate from reusable GPU textures.
#[derive(Default)]
pub(crate) struct FilmstripRequests {
    pending: HashMap<PathBuf, Subscription>,
    sources: HashMap<PathBuf, Source>,
    retry_after: HashMap<PathBuf, f64>,
}

impl FilmstripRequests {
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
        let source = Source::new(path);
        if self.sources.get(path).is_some_and(|old| *old != source) {
            self.pending.remove(path);
            self.sources.remove(path);
            self.retry_after.remove(path);
            strips.remove(path);
        }
        if subscribe
            && !strips.contains_key(path)
            && self
                .retry_after
                .get(path)
                .is_none_or(|at| ctx.input(|i| i.time) >= *at)
        {
            self.pending.insert(
                path.to_owned(),
                caches
                    .filmstrips
                    .subscribe(caches, source.clone(), duration, ctx),
            );
            self.sources.insert(path.to_owned(), source);
            self.retry_after.remove(path);
            strips.insert(path.to_owned(), None);
        }
        self.poll(ctx, strips);
    }

    /// Drain only pending subscriptions; completed assets cost no per-paint scan.
    pub(crate) fn poll(
        &mut self,
        ctx: &Context,
        strips: &mut HashMap<PathBuf, Option<Vec<TextureHandle>>>,
    ) {
        self.pending.retain(|path, subscription| {
            let (frames, terminal) = subscription.poll();
            if frames.is_empty() && terminal.is_none() {
                return true;
            }
            if self.sources.get(path) != Some(&Source::new(path)) {
                strips.remove(path);
                self.sources.remove(path);
                self.retry_after.remove(path);
                return false;
            }
            if matches!(terminal, Some(Terminal::Failed(_) | Terminal::Cancelled)) {
                strips.remove(path);
                self.retry_after.insert(
                    path.clone(),
                    ctx.input(|i| i.time) + RETRY_DELAY.as_secs_f64(),
                );
                return false;
            }
            if !frames.is_empty() {
                let textures = strips
                    .entry(path.clone())
                    .or_default()
                    .get_or_insert_default();
                for frame in frames {
                    let name = format!("{}#{}", path.display(), textures.len());
                    textures.push(ctx.load_texture(name, (*frame).clone(), Default::default()));
                }
            }
            terminal.is_none()
        });
    }

    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
impl Filmstrips {
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
            .get(&Source::new(path))
            .unwrap()
            .upgrade()
            .unwrap()
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
