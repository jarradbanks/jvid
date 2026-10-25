use std::fs::File;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::metadata::{self, Asset};
use crate::{MANIFEST, REPOSITORY, Result, SIGNATURE, error};

#[derive(Clone, Debug)]
pub struct Available {
    pub version: String,
    pub notes: String,
    pub asset: Asset,
    pub tag: String,
    pub manifest: Vec<u8>,
    pub signature: Vec<u8>,
}

impl Available {
    pub fn release_url(&self) -> String {
        format!("https://github.com/{REPOSITORY}/releases/tag/{}", self.tag)
    }
    fn asset_url(&self, name: &str) -> String {
        format!(
            "https://github.com/{REPOSITORY}/releases/download/{}/{name}",
            self.tag
        )
    }
}

#[derive(Debug)]
pub struct Download {
    pub update: Available,
    pub dir: TempDir,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

pub struct Client {
    agent: ureq::Agent,
    api: String,
    downloads: String,
}

impl Default for Client {
    fn default() -> Self {
        Self::new(
            true,
            format!("https://api.github.com/repos/{REPOSITORY}/releases"),
        )
    }
}

impl Client {
    fn new(https_only: bool, api: String) -> Self {
        let t = Some(Duration::from_secs(30));
        Self {
            agent: ureq::Agent::config_builder()
                .https_only(https_only)
                .timeout_connect(t)
                .timeout_recv_response(t)
                .timeout_per_read(t)
                .user_agent(concat!("jvid-updater/", env!("CARGO_PKG_VERSION")))
                .build()
                .into(),
            api,
            downloads: format!("https://github.com/{REPOSITORY}/releases/download"),
        }
    }

    fn bytes(&self, url: &str, max: u64) -> Result<Vec<u8>> {
        let response = self
            .agent
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(error)?;
        let mut bytes = Vec::new();
        response
            .into_body()
            .into_reader()
            .take(max + 1)
            .read_to_end(&mut bytes)
            .map_err(error)?;
        if bytes.len() as u64 > max {
            return Err("Update response is too large.".into());
        }
        Ok(bytes)
    }

    /// GitHub's latest flag can point at a lower version. Search stable releases by SemVer.
    /// A bounded listing fails visibly rather than claiming to be current on a truncated list.
    pub fn check(
        &self,
        current: &str,
        target: &str,
        distribution: &str,
        key: &[u8; 32],
    ) -> Result<Option<Available>> {
        let current = semver::Version::parse(current).map_err(error)?;
        let mut releases = Vec::new();
        for page in 1..=10 {
            let bytes = self.bytes(
                &format!("{}?per_page=100&page={page}", self.api),
                4 * 1024 * 1024,
            )?;
            let batch: Vec<Release> = serde_json::from_slice(&bytes).map_err(error)?;
            let done = batch.len() < 100;
            releases.extend(batch);
            if done {
                break;
            }
            if page == 10 {
                return Err("Release listing exceeded the updater's limit.".into());
            }
        }
        let versions = stable_releases(releases, &current);
        if versions.is_empty() {
            return Ok(None);
        }
        // An unmarked package may check and show notes, but cannot authorize replacement.
        let dist = if distribution == "external" {
            metadata::PACKAGES
                .iter()
                .find(|(t, _, _)| *t == target)
                .map(|(_, d, _)| *d)
                .unwrap_or(distribution)
        } else {
            distribution
        };
        for (_, tag) in versions {
            // Tags are parsed canonical versions, so cannot inject a different URL/path.
            let root = format!("{}/{tag}", self.downloads);
            let manifest = self.bytes(&format!("{root}/{MANIFEST}"), 1024 * 1024)?;
            let signature = self.bytes(&format!("{root}/{SIGNATURE}"), 64)?;
            let m = metadata::verify(&manifest, &signature, key)?;
            if metadata::stable_version(&tag)?.to_string() != m.version {
                return Err("Signed version does not match the GitHub release tag.".into());
            }
            if let Some(asset) = m.asset(target, dist).cloned() {
                return Ok(Some(Available {
                    version: m.version,
                    notes: m.notes,
                    asset,
                    tag,
                    manifest,
                    signature,
                }));
            }
        }
        Err("Newer stable releases have no compatible artifact for this distribution.".into())
    }

    pub fn download(
        &self,
        update: Available,
        cancel: &AtomicBool,
        progress: impl FnMut(u64),
    ) -> Result<Download> {
        let url = update.asset_url(&update.asset.name);
        self.download_at(update, &url, cancel, progress)
    }

    fn download_at(
        &self,
        update: Available,
        url: &str,
        cancel: &AtomicBool,
        progress: impl FnMut(u64),
    ) -> Result<Download> {
        let dir = tempfile::Builder::new()
            .prefix("jvid-update-")
            .tempdir()
            .map_err(error)?;
        let response = self.agent.get(url).call().map_err(error)?;
        let path = dir.path().join(&update.asset.name);
        let mut file = File::create(path).map_err(error)?;
        copy_verified(
            response.into_body().into_reader(),
            &mut file,
            &update.asset,
            cancel,
            progress,
        )?;
        file.sync_all().map_err(error)?;
        std::fs::write(dir.path().join(MANIFEST), &update.manifest).map_err(error)?;
        std::fs::write(dir.path().join(SIGNATURE), &update.signature).map_err(error)?;
        Ok(Download { update, dir })
    }
}

#[cfg(test)]
mod tests;

fn stable_releases(
    releases: Vec<Release>,
    current: &semver::Version,
) -> Vec<(semver::Version, String)> {
    let mut out: Vec<_> = releases
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| {
            metadata::stable_version(&r.tag_name)
                .ok()
                .map(|v| (v, r.tag_name))
        })
        .filter(|(v, _)| v.cmp_precedence(current).is_gt())
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out
}

/// Also used immediately before installer execution, so a staged file is never trusted by name.
pub fn copy_verified(
    mut input: impl Read,
    mut output: impl Write,
    asset: &Asset,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    let expected = metadata::unhex::<32>(&asset.sha256)?;
    let mut hash = Sha256::new();
    let mut received = 0;
    let mut buf = [0; 65536];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Download cancelled.".into());
        }
        let n = input.read(&mut buf).map_err(error)?;
        if n == 0 {
            break;
        }
        received += n as u64;
        if received > asset.size {
            return Err("Artifact exceeds its signed size.".into());
        }
        hash.update(&buf[..n]);
        output.write_all(&buf[..n]).map_err(error)?;
        progress(received);
    }
    if received != asset.size || hash.finalize().as_slice() != expected {
        return Err("Artifact size or SHA-256 does not match signed metadata.".into());
    }
    Ok(())
}
