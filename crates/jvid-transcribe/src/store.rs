//! Persistent recognition in source time. Caption text is an editable derivative and never
//! writes back here. Coverage records successful listening, including intervals with no words.

use std::collections::HashMap;
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use jvid_core::time::Rate;
use jvid_media::audio::SAMPLE_RATE;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Options, Source, Transcript};

/// Bump for decoder, silence removal, timing, or recognizer behavior changes. Includes the
/// whisper-rs/whisper.cpp generation; model weight changes have their own exact fingerprint.
const VERSION: &str = "source-words-1/whisper-rs-0.16";

pub type Shared = Arc<Mutex<Store>>;

/// All recognition inputs that affect words. Auto language and an explicit language differ;
/// prompts are compared verbatim, so filler-preserving and plain recognition cannot mix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub model: String,
    pub options: Options,
    pub version: String,
}

impl Settings {
    pub fn new(model: crate::models::Model, options: Options) -> Self {
        Self {
            model: model.fingerprint().into(),
            options,
            version: VERSION.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Key {
    /// Exact platform-encoded canonical path bytes, including non-Unicode file names.
    path: Vec<u8>,
    content: String,
    stream: usize,
    settings: Settings,
}

/// Verified file bytes, produced outside the shared-store lock and accepted only while the
/// filesystem identity still matches the file that was read.
pub struct Identity {
    path: PathBuf,
    stamp: Stamp,
    content: String,
}

/// A verified source snapshot. Keeping this through a job prevents saving old words under a
/// new content identity when the media is replaced while recognition runs.
#[derive(Debug, Clone)]
pub struct Request {
    pub source: Source,
    key: Key,
    canonical: PathBuf,
    stamp: Stamp,
}

impl Request {
    /// Decode the verified target itself, even if the user's path is a symlink later changed
    /// to another file. Results still require that original path to resolve to this target.
    pub fn decode_source(&self) -> Source {
        Source {
            path: self.canonical.clone(),
            ..self.source.clone()
        }
    }

    fn is_current(&self) -> bool {
        fs::canonicalize(&self.source.path).is_ok_and(|p| p == self.canonical)
            && Stamp::read(&self.canonical).is_ok_and(|s| s == self.stamp)
    }

    pub fn with_range(&self, range: Range<i64>) -> Self {
        Self {
            source: Source {
                range,
                ..self.source.clone()
            },
            ..self.clone()
        }
    }
}

#[derive(Debug, Default)]
pub struct Lookup {
    pub transcript: Transcript,
    pub missing: Vec<Source>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Chunk {
    range: Range<i64>,
    transcript: Transcript,
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    key: Key,
    chunks: Vec<Chunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    change: (u64, u64, i64, i64),
}

impl Stamp {
    fn read(path: &Path) -> io::Result<Self> {
        Self::from_metadata(fs::metadata(path)?)
    }

    fn from_metadata(m: Metadata) -> io::Result<Self> {
        if !m.is_file() {
            return Err(io::Error::other("recognition source is not a file"));
        }
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            len: m.len(),
            modified: m.modified().ok(),
            created: m.created().ok(),
            #[cfg(unix)]
            change: (m.dev(), m.ino(), m.ctime(), m.ctime_nsec()),
        })
    }
}

/// Shared by every speech tool. Disk entries include their full key and are atomically
/// replaced; corrupt, truncated, incompatible, or legacy whole-file caches count as misses.
#[derive(Default)]
pub struct Store {
    dir: Option<PathBuf>,
    identities: HashMap<PathBuf, (Stamp, String)>,
    entries: HashMap<String, Entry>,
    observed: HashMap<PathBuf, Option<Stamp>>,
    revision: u64,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            ..Self::default()
        }
    }

    pub fn shared(self) -> Shared {
        Arc::new(Mutex::new(self))
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Cheap source-state observation for placed-word memoization. A changed identity drops
    /// that memo even if the project itself is unchanged. Request creation then hashes bytes.
    pub fn observe(&mut self, path: &Path) {
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let current = Stamp::read(&canonical).ok();
        if self.observed.get(&canonical) != Some(&current) {
            self.identities.remove(&canonical);
            self.observed.insert(canonical, current);
            self.revision += 1;
        }
    }

    /// Hashes the complete content on first access or a filesystem change. Explicit workflow
    /// starts call `verify` as well, rather than relying solely on size/mtime hints.
    pub fn request(&mut self, source: &Source, settings: &Settings) -> io::Result<Request> {
        self.request_inner(source, settings, true)
    }

    /// UI lookup after background verification; never hashes media on the calling thread.
    pub fn request_known(&mut self, source: &Source, settings: &Settings) -> io::Result<Request> {
        self.request_inner(source, settings, false)
    }

    fn request_inner(
        &mut self,
        source: &Source,
        settings: &Settings,
        verify: bool,
    ) -> io::Result<Request> {
        let path = fs::canonicalize(&source.path)?;
        self.observe(&path);
        let stamp = Stamp::read(&path)?;
        if !self.identities.contains_key(&path) {
            if !verify {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "source verification pending",
                ));
            }
            self.verify(&path)?;
        }
        let content = self.identities[&path].1.clone();
        let key = Key {
            path: path.as_os_str().as_encoded_bytes().to_vec(),
            content,
            stream: source.nth,
            settings: settings.clone(),
        };
        let id = id(&key);
        if !self.entries.contains_key(&id) {
            let entry = self
                .file(&id)
                .and_then(|p| fs::read(p).ok())
                .and_then(|b| serde_json::from_slice::<Entry>(&b).ok())
                .filter(|e| e.key == key && valid(e))
                .unwrap_or_else(|| Entry {
                    key: key.clone(),
                    chunks: vec![],
                });
            self.entries.insert(id, entry);
        }
        Ok(Request {
            source: Source {
                range: source.range.start.max(0)..source.range.end.max(0),
                ..source.clone()
            },
            key,
            canonical: path,
            stamp,
        })
    }

    pub fn has_identity(&self, path: &Path) -> bool {
        fs::canonicalize(path)
            .ok()
            .is_some_and(|p| self.identities.contains_key(&p))
    }

    pub fn verify(&mut self, path: &Path) -> io::Result<()> {
        self.verify_with_cancel(path, &|| false)
    }

    /// Full content verification belongs on a job, with bounded cancellation between reads.
    pub fn verify_with_cancel(
        &mut self,
        path: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<()> {
        let identity = Self::identify(path, cancelled)?;
        self.accept(identity)
    }

    /// Read outside the shared-store lock so content verification never blocks UI readers.
    pub fn identify(path: &Path, cancelled: &dyn Fn() -> bool) -> io::Result<Identity> {
        let path = fs::canonicalize(path)?;
        let stamp = Stamp::read(&path)?;
        let mut file = File::open(&path)?;
        let mut hash = Sha256::new();
        let mut buf = [0u8; 128 * 1024];
        loop {
            if cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        if Stamp::read(&path)? != stamp {
            return Err(io::Error::other("recognition source changed while reading"));
        }
        Ok(Identity {
            path,
            stamp,
            content: format!("{:x}", hash.finalize()),
        })
    }

    pub fn accept(&mut self, identity: Identity) -> io::Result<()> {
        let Identity {
            path,
            stamp,
            content,
        } = identity;
        if Stamp::read(&path)? != stamp {
            return Err(io::Error::other("recognition source changed while reading"));
        }
        if self
            .identities
            .get(&path)
            .is_none_or(|v| v != &(stamp.clone(), content.clone()))
        {
            self.revision += 1;
        }
        self.observed.insert(path.clone(), Some(stamp.clone()));
        self.identities.insert(path, (stamp, content));
        Ok(())
    }

    pub fn lookup(&self, request: &Request) -> Lookup {
        let mut result = Lookup::default();
        let mut missing = vec![request.source.range.clone()];
        if !request.is_current() {
            result.missing.push(request.source.clone());
            return result;
        }
        if let Some(entry) = self.entries.get(&id(&request.key)) {
            for chunk in &entry.chunks {
                missing = missing
                    .into_iter()
                    .flat_map(|r| subtract(r, &chunk.range))
                    .collect();
                let a = chunk.range.start.max(request.source.range.start);
                let b = chunk.range.end.min(request.source.range.end);
                if a >= b {
                    continue;
                }
                result.transcript.language = result
                    .transcript
                    .language
                    .take()
                    .or_else(|| chunk.transcript.language.clone());
                result
                    .transcript
                    .words
                    .extend(slice(&chunk.transcript, &(a..b)).words);
            }
        }
        result.transcript.words.sort_by_key(|w| w.start);
        result.missing = missing
            .into_iter()
            .filter(|r| r.start < r.end)
            .map(|range| Source {
                range,
                ..request.source.clone()
            })
            .collect();
        result
    }

    /// Replace precisely this interval, preserving coverage and words on either side. An
    /// empty successful transcript is saved just like speech. Failed/cancelled jobs never call
    /// this. Source mutation during a job is an error, never reusable coverage.
    pub fn insert(&mut self, request: &Request, transcript: Transcript) -> io::Result<()> {
        if !request.is_current() {
            return Err(io::Error::other(
                "recognition source changed during transcription",
            ));
        }
        let id = id(&request.key);
        let entry = self
            .entries
            .get_mut(&id)
            .ok_or_else(|| io::Error::other("unknown recognition request"))?;
        let mut chunks = Vec::new();
        for old in &entry.chunks {
            for range in subtract(old.range.clone(), &request.source.range) {
                chunks.push(Chunk {
                    transcript: slice(&old.transcript, &range),
                    range,
                });
            }
        }
        chunks.push(Chunk {
            transcript: slice(&transcript, &request.source.range),
            range: request.source.range.clone(),
        });
        chunks.sort_by_key(|c| c.range.start);
        entry.chunks = chunks;
        self.revision += 1;
        if let Some(file) = self.file(&id) {
            fs::create_dir_all(file.parent().expect("cache parent"))?;
            let mut tmp = tempfile::NamedTempFile::new_in(file.parent().expect("cache parent"))?;
            serde_json::to_writer(&mut tmp, &self.entries[&id])?;
            tmp.flush()?;
            tmp.persist(file).map_err(|e| e.error)?;
        }
        Ok(())
    }

    fn file(&self, id: &str) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(format!("{id}.json")))
    }
}

fn id(key: &Key) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(key).expect("recognition key"))
    )
}

fn subtract(r: Range<i64>, covered: &Range<i64>) -> Vec<Range<i64>> {
    if covered.end <= r.start || covered.start >= r.end {
        return vec![r];
    }
    let mut left = Vec::new();
    if r.start < covered.start {
        left.push(r.start..covered.start);
    }
    if covered.end < r.end {
        left.push(covered.end..r.end);
    }
    left
}

fn slice(t: &Transcript, range: &Range<i64>) -> Transcript {
    let rate = Rate::new(SAMPLE_RATE, 1);
    let time = |n: i64| {
        if n == i64::MAX {
            jvid_core::time::Time(i64::MAX)
        } else {
            rate.time_floor(n)
        }
    };
    let (a, b) = (time(range.start), time(range.end));
    Transcript {
        language: t.language.clone(),
        words: t
            .words
            .iter()
            .filter(|w| a <= w.start && w.start < b)
            .cloned()
            .map(|mut w| {
                w.end = w.end.min(b);
                w
            })
            .collect(),
    }
}

fn valid(e: &Entry) -> bool {
    e.chunks.iter().all(|c| {
        c.range.start >= 0
            && c.range.start < c.range.end
            && c.transcript
                .words
                .iter()
                .all(|w| w.start <= w.end && w.confidence.is_finite())
    }) && e
        .chunks
        .windows(2)
        .all(|w| w[0].range.end <= w[1].range.start)
}

/// Union overlapping/adjacent requests for the same path and stream, including repeated clips.
pub fn merge_sources(mut sources: Vec<Source>) -> Vec<Source> {
    sources.sort_by(|a, b| (&a.path, a.nth, a.range.start).cmp(&(&b.path, b.nth, b.range.start)));
    let mut merged: Vec<Source> = Vec::new();
    for source in sources {
        if let Some(last) = merged.last_mut().filter(|last| {
            last.path == source.path
                && last.nth == source.nth
                && last.range.end >= source.range.start
        }) {
            last.range.end = last.range.end.max(source.range.end);
        } else if source.range.start < source.range.end {
            merged.push(source);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Word;
    use crate::models::Model;
    use jvid_core::time::{FLICKS_PER_SECOND as S, Time};

    fn fixture() -> (tempfile::TempDir, Store, Source, Settings) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.wav");
        fs::write(&path, b"first take").unwrap();
        let store = Store::new(dir.path().join("cache"));
        let source = Source {
            path,
            nth: 0,
            range: 0..8 * SAMPLE_RATE as i64,
        };
        let settings = Settings::new(
            Model::Base,
            Options {
                initial_prompt: Some("Um, uh".into()),
                ..Default::default()
            },
        );
        (dir, store, source, settings)
    }

    fn spoken(at: i64) -> Transcript {
        Transcript {
            language: Some("en".into()),
            words: vec![Word {
                text: "um".into(),
                start: Time(at * S),
                end: Time(at * S + S / 2),
                confidence: 0.875,
            }],
        }
    }

    #[test]
    fn partial_and_silent_coverage_survive_reopen_without_claiming_unheard_audio() {
        let (dir, mut store, source, settings) = fixture();
        let full = store.request(&source, &settings).unwrap();
        let p = SAMPLE_RATE as i64;
        store
            .insert(&full.with_range(2 * p..4 * p), spoken(2))
            .unwrap();
        store
            .insert(&full.with_range(4 * p..6 * p), Transcript::default())
            .unwrap();
        let silent = store.lookup(&full.with_range(4 * p..6 * p));
        assert!(silent.missing.is_empty());
        assert!(silent.transcript.words.is_empty());
        let got = store.lookup(&full);
        assert_eq!(
            got.missing
                .iter()
                .map(|s| s.range.clone())
                .collect::<Vec<_>>(),
            [0..2 * p, 6 * p..8 * p]
        );
        assert_eq!(got.transcript, spoken(2));
        let mut reopened = Store::new(dir.path().join("cache"));
        let request = reopened.request(&source, &settings).unwrap();
        assert_eq!(reopened.lookup(&request).transcript, spoken(2));
        assert_eq!(reopened.lookup(&request).missing, got.missing);
        // Deliberate replacement of only one second preserves coverage on both sides.
        reopened
            .insert(&request.with_range(3 * p..4 * p), spoken(3))
            .unwrap();
        assert_eq!(reopened.lookup(&request).transcript.words.len(), 2);
        assert_eq!(reopened.lookup(&request).missing, got.missing);
    }

    #[test]
    fn every_recognition_input_and_live_content_identity_invalidate() {
        let (_dir, mut store, source, settings) = fixture();
        let original = store.request(&source, &settings).unwrap();
        store.insert(&original, spoken(2)).unwrap();
        let mut changes = Vec::new();
        let mut changed = settings.clone();
        changed.options.language = Some("es".into());
        changes.push(changed);
        let mut changed = settings.clone();
        changed.options.translate = true;
        changes.push(changed);
        let mut changed = settings.clone();
        changed.options.initial_prompt = None;
        changes.push(changed);
        let mut changed = settings.clone();
        changed.model = Model::Small.fingerprint().into();
        changes.push(changed);
        let mut changed = settings.clone();
        changed.version += "-new";
        changes.push(changed);
        for changed in changes {
            let request = store.request(&source, &changed).unwrap();
            let got = store.lookup(&request);
            assert_eq!(got.missing.as_slice(), std::slice::from_ref(&source));
            assert!(got.transcript.words.is_empty());
        }
        let stream = Source {
            nth: 1,
            ..source.clone()
        };
        let request = store.request(&stream, &settings).unwrap();
        assert_eq!(store.lookup(&request).missing, [stream]);
        let other = source.path.with_file_name("other.wav");
        fs::write(&other, b"first take").unwrap();
        let request = store
            .request(
                &Source {
                    path: other,
                    ..source.clone()
                },
                &settings,
            )
            .unwrap();
        assert!(store.lookup(&request).transcript.words.is_empty());
        // Same byte count, changed bytes: both the live in-memory view and disk lookup miss.
        fs::write(&source.path, b"other take").unwrap();
        assert!(
            store.insert(&original, spoken(3)).is_err(),
            "a job cannot save after source mutation"
        );
        assert!(store.lookup(&original).transcript.words.is_empty());
        let request = store.request(&source, &settings).unwrap();
        assert_eq!(
            store.lookup(&request).missing.as_slice(),
            std::slice::from_ref(&source)
        );
        fs::remove_file(&source.path).unwrap();
        assert!(store.request(&source, &settings).is_err());
    }

    #[test]
    fn truncated_cache_is_a_miss_and_failed_writes_leave_no_half_entry() {
        let (dir, mut store, source, settings) = fixture();
        let request = store.request(&source, &settings).unwrap();
        store.insert(&request, spoken(2)).unwrap();
        let path = store.file(&id(&request.key)).unwrap();
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        let mut store = Store::new(dir.path().join("cache"));
        let request = store.request(&source, &settings).unwrap();
        assert_eq!(store.lookup(&request).missing, [source]);
        assert_eq!(fs::read_dir(dir.path().join("cache")).unwrap().count(), 1);
    }

    #[test]
    fn ui_lookup_never_hashes_and_cancelled_verification_is_not_coverage() {
        let (_dir, mut store, source, settings) = fixture();
        assert_eq!(
            store.request_known(&source, &settings).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(
            matches!(Store::identify(&source.path, &|| true), Err(e) if e.kind() == io::ErrorKind::Interrupted)
        );
        assert!(store.request_known(&source, &settings).is_err());
        let identity = Store::identify(&source.path, &|| false).unwrap();
        store.accept(identity).unwrap();
        let request = store.request_known(&source, &settings).unwrap();
        assert_eq!(store.lookup(&request).missing, [source]);
    }

    #[cfg(unix)]
    #[test]
    fn exact_source_paths_are_persistent_identities() {
        use std::os::unix::ffi::OsStringExt;
        let (dir, mut store, mut source, settings) = fixture();
        // APFS requires valid UTF-8 names. Other Unix filesystems also exercise
        // byte paths that would collide under a lossy Unicode conversion.
        let names: [&[u8]; 2] = if cfg!(target_os = "macos") {
            ["voice-é.wav".as_bytes(), "声.wav".as_bytes()]
        } else {
            [b"voice-\xff.wav", b"voice-\xfe.wav"]
        };
        let mut expected = Vec::new();
        for (name, start) in names.into_iter().zip([2, 4]) {
            source.path = dir.path().join(std::ffi::OsString::from_vec(name.to_vec()));
            fs::write(&source.path, b"same content, distinct source").unwrap();
            let request = store.request(&source, &settings).unwrap();
            let transcript = spoken(start);
            store.insert(&request, transcript.clone()).unwrap();
            expected.push((source.clone(), transcript));
        }
        let mut reopened = Store::new(dir.path().join("cache"));
        for (source, transcript) in expected {
            let request = reopened.request(&source, &settings).unwrap();
            assert_eq!(reopened.lookup(&request).transcript, transcript);
        }
    }

    #[cfg(unix)]
    #[test]
    fn redirected_source_symlink_cannot_contaminate_the_original_content() {
        use std::os::unix::fs::symlink;
        let (dir, mut store, source, settings) = fixture();
        let link = dir.path().join("linked.wav");
        symlink(&source.path, &link).unwrap();
        let linked = Source {
            path: link.clone(),
            ..source.clone()
        };
        let request = store.request(&linked, &settings).unwrap();
        assert_eq!(
            request.decode_source().path,
            fs::canonicalize(&source.path).unwrap()
        );
        store.insert(&request, spoken(2)).unwrap();
        let other = dir.path().join("replacement.wav");
        fs::write(&other, b"other take").unwrap();
        fs::remove_file(&link).unwrap();
        symlink(other, &link).unwrap();
        assert!(store.insert(&request, spoken(3)).is_err());
        assert!(store.lookup(&request).transcript.words.is_empty());
        let original = store.request(&source, &settings).unwrap();
        assert_eq!(store.lookup(&original).transcript, spoken(2));
    }

    #[test]
    fn repeated_and_overlapping_ranges_are_unioned_per_stream() {
        let (_dir, _, source, _) = fixture();
        let source = |nth, range| Source {
            nth,
            range,
            ..source.clone()
        };
        assert_eq!(
            merge_sources(vec![
                source(0, 5..8),
                source(0, 1..6),
                source(0, 1..6),
                source(0, 8..10),
                source(1, 2..3)
            ]),
            [source(0, 1..10), source(1, 2..3)]
        );
    }
}
