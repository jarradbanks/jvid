use std::collections::HashSet;
use std::path::Path;

use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{REPOSITORY, Result, error};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub repository: String,
    pub version: String,
    pub notes: String,
    pub assets: Vec<Asset>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub target: String,
    pub distribution: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// Package contract with #476. Unknown distributions never gain install capability.
pub const PACKAGES: &[(&str, &str, &str)] = &[
    (
        "x86_64-pc-windows-msvc",
        "windows-installer",
        "windows-x86_64-setup.exe",
    ),
    (
        "x86_64-pc-windows-msvc",
        "windows-portable",
        "windows-x86_64-portable.zip",
    ),
    ("aarch64-apple-darwin", "macos-dmg", "macos-aarch64.dmg"),
    (
        "aarch64-apple-darwin",
        "macos-portable",
        "macos-aarch64-portable.zip",
    ),
    (
        "x86_64-unknown-linux-gnu",
        "linux-portable",
        "linux-x86_64-portable.tar.gz",
    ),
];

pub fn stable_version(tag: &str) -> Result<Version> {
    let v = Version::parse(tag.strip_prefix('v').unwrap_or(tag)).map_err(error)?;
    if !v.pre.is_empty() || !v.build.is_empty() {
        return Err("Release version must be stable, without build metadata.".into());
    }
    Ok(v)
}

pub fn unhex<const N: usize>(s: &str) -> Result<[u8; N]> {
    if s.len() != N * 2 || !s.is_ascii() {
        return Err("Invalid hex length.".into());
    }
    let mut out = [0; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(error)?;
    }
    Ok(out)
}

impl Manifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 || self.repository != REPOSITORY {
            return Err("Unsupported updater schema or repository.".into());
        }
        let v = stable_version(&self.version)?;
        if v.to_string() != self.version || self.notes.len() > 256 * 1024 || self.assets.is_empty()
        {
            return Err("Invalid release metadata.".into());
        }
        let mut seen = HashSet::new();
        for a in &self.assets {
            let suffix = PACKAGES
                .iter()
                .find(|(t, d, _)| *t == a.target && *d == a.distribution)
                .ok_or("Unknown target/distribution pair.")?
                .2;
            if a.name != format!("jvid-{}-{suffix}", self.version)
                || a.size == 0
                || a.size > 8 * 1024 * 1024 * 1024
                || !seen.insert((&a.target, &a.distribution))
            {
                return Err("Invalid or duplicate release asset.".into());
            }
            unhex::<32>(&a.sha256)?;
        }
        Ok(())
    }

    pub fn asset(&self, target: &str, distribution: &str) -> Option<&Asset> {
        self.assets
            .iter()
            .find(|a| a.target == target && a.distribution == distribution)
    }
}

/// Verify the exact bytes before parsing. The pinned key is never taken from the release.
pub fn verify(bytes: &[u8], signature: &[u8], key: &[u8; 32]) -> Result<Manifest> {
    if bytes.len() > 1024 * 1024 {
        return Err("Update metadata is too large.".into());
    }
    VerifyingKey::from_bytes(key)
        .map_err(error)?
        .verify_strict(bytes, &Signature::from_slice(signature).map_err(error)?)
        .map_err(|_| "Release metadata signature is invalid.".to_string())?;
    let m: Manifest = serde_json::from_slice(bytes).map_err(error)?;
    m.validate()?;
    Ok(m)
}

pub fn distribution(executable: &Path) -> String {
    // Unmarked source and external packages retain their package manager's ownership.
    executable
        .parent()
        .and_then(|p| std::fs::read_to_string(p.join("jvid-distribution")).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| PACKAGES.iter().any(|(_, d, _)| *d == s))
        .unwrap_or_else(|| "external".into())
}

pub fn self_update(target: &str, distribution: &str) -> bool {
    target == "x86_64-pc-windows-msvc" && distribution == "windows-installer"
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_external_and_wrong_platform_builds_never_self_install() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("jvid-app.exe");
        assert_eq!(distribution(&exe), "external");
        for &(_, marker, _) in PACKAGES {
            std::fs::write(dir.path().join("jvid-distribution"), format!("{marker}\n")).unwrap();
            assert_eq!(distribution(&exe), marker);
            assert_eq!(
                self_update("x86_64-pc-windows-msvc", marker),
                marker == "windows-installer"
            );
        }
        for target in [
            "aarch64-pc-windows-msvc",
            "aarch64-apple-darwin",
            "x86_64-unknown-linux-gnu",
        ] {
            assert!(!self_update(target, "windows-installer"));
        }
        assert!(!self_update("x86_64-pc-windows-msvc", "external"));
        assert!(!self_update("x86_64-pc-windows-msvc", "unknown"));
        std::fs::write(dir.path().join("jvid-distribution"), "unknown\n").unwrap();
        assert_eq!(distribution(&exe), "external");
    }
}
