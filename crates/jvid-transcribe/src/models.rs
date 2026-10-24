//! Whisper models, downloaded only when asked into the data folder and checked by SHA-256.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

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
        self.path(dir).is_file()
    }

    /// Downloads into `dir` on `jobs` with progress, then hands `done` the model's path. Cancel
    /// through the handle; `done` isn't called for a cancelled download.
    pub fn download(
        self,
        jobs: &Pool,
        dir: &Path,
        done: impl FnOnce(Result<PathBuf, Error>) + Send + 'static,
    ) -> Handle {
        let dest = self.path(dir);
        jobs.submit(
            format!("model:{}", dest.display()),
            Priority::Interactive,
            move |ctx| match download(&self.url(), self.file().2, &dest, ctx) {
                Err(Error::Cancelled) => {}
                r => done(r.map(|()| dest)),
            },
        )
    }
}

/// `<data folder>/models`, or `None` if the platform has no data folder.
pub fn dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "jvid").map(|d| d.data_dir().join("models"))
}

/// Fetches `url` to `dest` through a `.part` file that only becomes `dest` once its hash matches.
fn download(url: &str, sha256: &str, dest: &Path, ctx: &Ctx) -> Result<(), Error> {
    let part = dest.with_extension("part");
    let r = fetch(url, sha256, &part, ctx)
        .and_then(|()| fs::rename(&part, dest).map_err(|e| Error::Failed(e.to_string())));
    if r.is_err() {
        let _ = fs::remove_file(&part);
    }
    r
}

fn fetch(url: &str, sha256: &str, part: &Path, ctx: &Ctx) -> Result<(), Error> {
    let failed = |e: &dyn std::fmt::Display| Error::Failed(format!("{url}: {e}"));
    let res = ureq::get(url).call().map_err(|e| failed(&e))?;
    let total = res.body().content_length();
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
        hash.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| failed(&e))?;
        got += n as u64;
        if let Some(t) = total {
            ctx.set_progress(got as f32 / t as f32);
        }
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
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// Serves `body` over HTTP to one request; returns its URL.
    fn serve(body: &'static [u8]) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.bin", l.local_addr().unwrap());
        std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            while r.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            let mut s = s;
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            s.write_all(body).unwrap();
        });
        url
    }

    fn get(url: String, sha256: &str, dest: PathBuf) -> Result<(), Error> {
        let jobs = Pool::new(1);
        let (tx, rx) = mpsc::channel();
        let sha256 = sha256.to_owned();
        jobs.submit("dl", Priority::Interactive, move |ctx| {
            tx.send(download(&url, &sha256, &dest, ctx)).unwrap();
        });
        rx.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    #[test]
    fn checked_download_lands_and_corrupt_one_is_rejected() {
        let dir = std::env::temp_dir().join(format!("jvid-models-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let dest = Model::Base.path(&dir);
        let sha = format!("{:x}", Sha256::digest(b"hello model"));

        get(serve(b"hello model"), &sha, dest.clone()).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"hello model");
        assert!(Model::Base.is_downloaded(&dir));
        fs::remove_file(&dest).unwrap();

        // Acceptance: a corrupt download is rejected and leaves nothing behind.
        assert_eq!(
            get(serve(b"hello modem"), &sha, dest.clone()),
            Err(Error::Corrupt)
        );
        assert!(!dest.exists() && !dest.with_extension("part").exists());
        assert!(!Model::Base.is_downloaded(&dir));
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
