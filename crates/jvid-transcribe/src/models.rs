//! Whisper models, downloaded only when asked into the data folder and checked by SHA-256.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use jvid_jobs::{Ctx, Handle, Pool, Priority};
use sha2::{Digest, Sha256};

use crate::Error;

/// The whisper.cpp Hugging Face revision the URLs and hashes below are pinned to.
const REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Model {
    Base,
    Small,
    /// large-v3-turbo, 5-bit quantised.
    LargeV3Turbo,
}

impl Model {
    /// Smallest and fastest first.
    pub const ALL: [Model; 3] = [Model::Base, Model::Small, Model::LargeV3Turbo];

    /// (file, bytes, SHA-256)
    fn file(self) -> (&'static str, u64, &'static str) {
        match self {
            Model::Base => (
                "ggml-base.bin",
                147_951_465,
                "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
            ),
            Model::Small => (
                "ggml-small.bin",
                487_601_967,
                "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
            ),
            Model::LargeV3Turbo => (
                "ggml-large-v3-turbo-q5_0.bin",
                574_041_195,
                "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
            ),
        }
    }

    /// Download size in bytes, to show before asking.
    pub fn size(self) -> u64 {
        self.file().1
    }

    /// Exact identity of the pinned model weights, for recognition cache compatibility.
    pub fn fingerprint(self) -> &'static str {
        self.file().2
    }

    pub fn url(self) -> String {
        format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/{REVISION}/{}",
            self.file().0
        )
    }

    /// Where the model lives in `dir` (normally [`dir`]); only a checked download is ever there.
    pub fn path(self, dir: &Path) -> PathBuf {
        dir.join(self.file().0)
    }

    pub fn is_downloaded(self, dir: &Path) -> bool {
        fs::metadata(self.path(dir)).is_ok_and(|m| m.is_file() && m.len() == self.size())
    }

    /// Downloads into `dir` on `jobs` with progress, then hands `done` the model's path. Cancel
    /// through the handle; `done` isn't called for a cancelled download.
    pub fn download(
        self,
        jobs: &Pool,
        dir: &Path,
        done: impl FnOnce(Result<PathBuf, Error>) + Send + 'static,
    ) -> Handle {
        static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| agent(true));
        let dest = self.path(dir);
        let (_, size, sha256) = self.file();
        jobs.submit(
            format!("model:{}", dest.display()),
            Priority::Interactive,
            move |ctx| match download(&AGENT, &self.url(), size, sha256, &dest, ctx) {
                Err(Error::Cancelled) => {}
                r => done(r.map(|()| dest)),
            },
        )
    }
}

/// `<local data folder>/models`, or `None` if the platform has no data folder. Local, so hundreds
/// of MB don't roam with a Windows profile.
pub fn dir() -> Option<PathBuf> {
    jvid_paths::data_local_dir().map(|d| d.join("models"))
}

/// Every network wait is bounded, so a stalled download fails and sees Cancel within 30 s.
fn agent(https_only: bool) -> ureq::Agent {
    let t = Some(Duration::from_secs(30));
    ureq::Agent::config_builder()
        .https_only(https_only)
        .timeout_connect(t)
        .timeout_recv_response(t)
        .timeout_per_read(t)
        .build()
        .into()
}

/// Fetches `url` (`size` bytes) to `dest` through a `.part` file that only becomes `dest` once its
/// hash matches. Each download has its own `.part`, so a cancelled one still finishing can't
/// delete its retry's.
fn download(
    agent: &ureq::Agent,
    url: &str,
    size: u64,
    sha256: &str,
    dest: &Path,
    ctx: &Ctx,
) -> Result<(), Error> {
    static N: AtomicU64 = AtomicU64::new(0);
    let ours = format!("part-{}-", std::process::id());
    remove_parts(dest, &ours);
    let part = dest.with_extension(format!("{ours}{}", N.fetch_add(1, Ordering::Relaxed)));
    let r = fetch(agent, url, size, sha256, &part, ctx)
        .and_then(|()| fs::rename(&part, dest).map_err(|e| Error::Failed(e.to_string())));
    if r.is_err() {
        let _ = fs::remove_file(&part);
    }
    r
}

/// Removes `.part` files of `dest` left by earlier runs that crashed, keeping this process's
/// (`ours` prefix), which may still be in use.
fn remove_parts(dest: &Path, ours: &str) {
    let (Some(dir), Some(stem)) = (dest.parent(), dest.file_stem()) else {
        return;
    };
    for p in fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
    {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if p.file_stem() == Some(stem) && ext.starts_with("part-") && !ext.starts_with(ours) {
            let _ = fs::remove_file(p);
        }
    }
}

fn fetch(
    agent: &ureq::Agent,
    url: &str,
    size: u64,
    sha256: &str,
    part: &Path,
    ctx: &Ctx,
) -> Result<(), Error> {
    let failed = |e: &dyn std::fmt::Display| Error::Failed(format!("{url}: {e}"));
    let res = agent.get(url).call().map_err(|e| failed(&e))?;
    let mut body = res.into_body().into_reader();
    if let Some(d) = part.parent() {
        fs::create_dir_all(d).map_err(|e| failed(&e))?;
    }
    let mut file = BufWriter::new(File::create(part).map_err(|e| failed(&e))?);
    let mut hash = Sha256::new();
    let mut buf = vec![0; 1 << 16];
    let mut got = 0u64;
    loop {
        if ctx.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let n = body.read(&mut buf).map_err(|e| failed(&e))?;
        if n == 0 {
            break;
        }
        got += n as u64;
        // Stops a misbehaving server filling the disk before the hash could catch it.
        if got > size {
            return Err(failed(&format!("more than the expected {size} bytes")));
        }
        hash.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| failed(&e))?;
        ctx.set_progress(got as f32 / size as f32);
    }
    if got != size {
        return Err(failed(&format!("got {got} of {size} bytes")));
    }
    // On disk before the rename, so a crash can't leave a short file under the model's name.
    file.into_inner()
        .map_err(|e| failed(&e))?
        .sync_all()
        .map_err(|e| failed(&e))?;
    if format!("{:x}", hash.finalize()) != sha256 {
        return Err(Error::Corrupt);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// Answers one HTTP request with `reply`; returns its URL.
    fn serve(reply: impl FnOnce(TcpStream) + Send + 'static) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.bin", l.local_addr().unwrap());
        std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            while r.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            reply(s);
        });
        url
    }

    fn head(s: &mut TcpStream, len: usize) {
        write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
    }

    /// Serves `body` whole.
    fn serve_body(body: &'static [u8]) -> String {
        serve(move |mut s| {
            head(&mut s, body.len());
            // The client may hang up early, on a body over its size.
            let _ = s.write_all(body);
        })
    }

    fn start(
        jobs: &Pool,
        url: String,
        size: u64,
        sha256: &str,
        dest: PathBuf,
    ) -> mpsc::Receiver<Result<(), Error>> {
        let (tx, rx) = mpsc::channel();
        let sha256 = sha256.to_owned();
        jobs.submit(url.clone(), Priority::Interactive, move |ctx| {
            tx.send(download(&agent(false), &url, size, &sha256, &dest, ctx))
                .unwrap();
        });
        rx
    }

    fn get(url: String, size: u64, sha256: &str, dest: PathBuf) -> Result<(), Error> {
        let jobs = Pool::new(1);
        let rx = start(&jobs, url, size, sha256, dest);
        rx.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jvid-models-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// `.part` files in `dir`.
    fn parts(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().unwrap().to_str().unwrap().starts_with("part"))
            .collect()
    }

    const BODY: &[u8] = b"hello model";

    fn sha() -> String {
        format!("{:x}", Sha256::digest(BODY))
    }

    #[test]
    fn checked_download_lands_and_corrupt_one_is_rejected() {
        let dir = tmp("check");
        let dest = Model::Base.path(&dir);

        get(serve_body(BODY), 11, &sha(), dest.clone()).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), BODY);
        // Not the base model's size, so not taken for it.
        assert!(!Model::Base.is_downloaded(&dir));
        fs::remove_file(&dest).unwrap();

        // Acceptance: a corrupt download is rejected and leaves nothing behind.
        assert_eq!(
            get(serve_body(b"hello modem"), 11, &sha(), dest.clone()),
            Err(Error::Corrupt)
        );
        assert!(!dest.exists() && parts(&dir).is_empty());
    }

    #[test]
    fn download_stops_at_the_expected_size() {
        let dir = tmp("size");
        let dest = Model::Base.path(&dir);
        let failed = |r| match r {
            Err(Error::Failed(e)) => e,
            r => panic!("{r:?}"),
        };
        let e = failed(get(serve_body(BODY), 5, &sha(), dest.clone()));
        assert!(e.contains("more than the expected 5 bytes"), "{e}");
        let e = failed(get(serve_body(BODY), 20, &sha(), dest.clone()));
        assert!(e.contains("got 11 of 20 bytes"), "{e}");
        assert!(!dest.exists() && parts(&dir).is_empty());
    }

    #[test]
    fn a_failing_download_leaves_a_concurrent_one_of_the_same_model_alone() {
        let dir = tmp("race");
        let dest = Model::Base.path(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Left by a run that crashed: pid 1 is never this test.
        let stale = dest.with_extension("part-1-0");
        fs::write(&stale, b"hello").unwrap();

        // B sends half its body, then waits until A has failed.
        let (go, wait) = mpsc::channel::<()>();
        let b_url = serve(move |mut s| {
            head(&mut s, BODY.len());
            s.write_all(&BODY[..5]).unwrap();
            wait.recv().unwrap();
            s.write_all(&BODY[5..]).unwrap();
        });
        let jobs = Pool::new(2);
        let b = start(&jobs, b_url, 11, &sha(), dest.clone());
        while parts(&dir).iter().all(|p| *p == stale) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!stale.exists(), "a crashed run's .part is cleared");

        // A, a retry of the same model, is cut off mid-body and removes its .part.
        let a_url = serve(|mut s| {
            head(&mut s, BODY.len());
            s.write_all(&BODY[..3]).unwrap();
        });
        let a = start(&jobs, a_url, 11, &sha(), dest.clone());
        let a = a.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(a, Err(Error::Failed(_))), "{a:?}");

        go.send(()).unwrap();
        assert_eq!(b.recv_timeout(Duration::from_secs(10)).unwrap(), Ok(()));
        assert_eq!(fs::read(&dest).unwrap(), BODY);
        assert!(parts(&dir).is_empty());
    }

    #[test]
    fn urls_are_pinned_and_hashes_well_formed() {
        for m in Model::ALL {
            let (file, size, sha) = m.file();
            assert!(m.url().ends_with(file) && m.url().contains(REVISION));
            assert!(size > 100_000_000);
            assert!(sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }
}
