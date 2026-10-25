//! The helper is copied out of the installation before the editor closes. Inno owns its
//! file transaction/rollback; we never overwrite program files ourselves or touch user data.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::metadata;
use crate::network::{Download, copy_verified};
use crate::{MANIFEST, Result, SIGNATURE, TARGET, VERSION, error};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub executable: PathBuf,
    pub parent_pid: u32,
    pub tag: String,
    pub report: PathBuf,
}

/// Holds the temporary files until the native window has shut down successfully.
#[derive(Debug)]
pub struct Prepared {
    download: Download,
    helper: PathBuf,
}

pub fn prepare(download: Download, executable: &Path, report: &Path) -> Result<Prepared> {
    if !metadata::self_update(TARGET, &metadata::distribution(executable)) {
        return Err(
            "This distribution must be updated through its documented manual route.".into(),
        );
    }
    let executable = fs::canonicalize(executable).map_err(error)?;
    let parent = executable.parent().ok_or("No installation directory.")?;
    if parent.join("jvid.portable").exists()
        || executable.file_name().and_then(|s| s.to_str()) != Some("jvid-app.exe")
    {
        return Err("The installed Windows layout is not recognized.".into());
    }
    let helper = download.dir.path().join("jvid-update-helper.exe");
    fs::copy(parent.join("jvid-update-helper.exe"), &helper).map_err(error)?;
    let plan = Plan {
        executable,
        parent_pid: std::process::id(),
        tag: download.update.tag.clone(),
        report: report.into(),
    };
    fs::write(
        download.dir.path().join("plan.json"),
        serde_json::to_vec(&plan).map_err(error)?,
    )
    .map_err(error)?;
    // Probe permissions before asking the app to close. The helper writes this after exit too.
    if let Some(p) = report.parent() {
        fs::create_dir_all(p).map_err(error)?;
    }
    fs::write(report, "Update prepared. Installation has not started.").map_err(error)?;
    validate(
        download.dir.path(),
        &plan,
        &crate::public_key()?,
        TARGET,
        VERSION,
    )?;
    Ok(Prepared { download, helper })
}

impl Prepared {
    /// Called after eframe returns, immediately before the desktop process exits.
    pub fn launch(self) -> Result<()> {
        let mut command = Command::new(&self.helper);
        command.arg(self.download.dir.path());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW; installer owns its own UI.
        }
        command.spawn().map_err(error)?;
        // The helper owns cleanup and a persistent report from here. A launch failure retains
        // no partial install and the TempDir above removes the staged files.
        let _ = self.download.dir.keep();
        Ok(())
    }
}

fn validate(
    dir: &Path,
    plan: &Plan,
    key: &[u8; 32],
    target: &str,
    current: &str,
) -> Result<PathBuf> {
    let bytes = fs::read(dir.join(MANIFEST)).map_err(error)?;
    let signature = fs::read(dir.join(SIGNATURE)).map_err(error)?;
    let m = metadata::verify(&bytes, &signature, key)?;
    if metadata::stable_version(&plan.tag)?.to_string() != m.version
        || metadata::stable_version(&m.version)?
            .cmp_precedence(&semver::Version::parse(current).map_err(error)?)
            .is_le()
    {
        return Err("Refusing a mismatched version or downgrade.".into());
    }
    let asset = m
        .asset(target, "windows-installer")
        .ok_or("No compatible installed Windows artifact.")?;
    let payload = dir.join(&asset.name);
    copy_verified(
        fs::File::open(&payload).map_err(error)?,
        std::io::sink(),
        asset,
        &AtomicBool::new(false),
        |_| {},
    )?;
    Ok(payload)
}

/// Always attempt to relaunch the existing path after installer cancellation/failure. Inno
/// leaves the old install in place on a failed transaction. A reboot-required result is
/// reported instead of being mistaken for a completed in-app upgrade.
pub fn install_and_restart(
    install: impl FnOnce() -> Result<i32>,
    restart: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let result = install().and_then(|code| match code {
        0 => Ok(()),
        3010 => Err("Windows must be restarted to finish the update; restart it when your work is saved.".into()),
        n => Err(format!("Installer exited with code {n}; the previous installation should remain available. See the installer log.")),
    });
    let relaunched = restart();
    match (result, relaunched) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(e), Ok(())) => Err(e),
        (Ok(()), Err(e)) => Err(format!(
            "Installation finished, but automatic restart failed: {e}. Launch jvid manually."
        )),
        (Err(e), Err(r)) => Err(format!(
            "{e} Restart also failed: {r}. Launch the existing installation manually."
        )),
    }
}

pub fn run_helper(dir: &Path) -> Result<()> {
    run_helper_with_key(dir, &crate::public_key()?)
}

fn run_helper_with_key(dir: &Path, key: &[u8; 32]) -> Result<()> {
    if !cfg!(target_os = "windows") {
        return Err("The install helper supports Windows installer builds only.".into());
    }
    let plan: Plan =
        serde_json::from_slice(&fs::read(dir.join("plan.json")).map_err(error)?).map_err(error)?;
    let result = (|| {
        validate(dir, &plan, key, TARGET, VERSION)?;
        let root = plan
            .executable
            .parent()
            .ok_or("Missing install directory.")?;
        if metadata::distribution(&plan.executable) != "windows-installer"
            || root.join("jvid.portable").exists()
        {
            return Err("The install location is no longer an installed Windows build.".into());
        }
        // Fixed script; no path or untrusted text is interpolated into PowerShell code.
        let shell = PathBuf::from(std::env::var_os("SystemRoot").ok_or("No SystemRoot.")?)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let status = Command::new(shell).args(["-NoProfile", "-NonInteractive", "-Command",
            "$ErrorActionPreference='Stop'; $p=Get-Process -Id ([int]$env:JVID_UPDATE_PARENT) -ErrorAction SilentlyContinue; if ($p) { $p | Wait-Process -Timeout 120 }"])
            .env("JVID_UPDATE_PARENT", plan.parent_pid.to_string()).status().map_err(error)?;
        if !status.success() {
            return Err("Editor did not exit; installation was deferred.".into());
        }
        let payload = validate(dir, &plan, key, TARGET, VERSION)?;
        let log = plan.report.with_extension("installer.log");
        install_and_restart(
            || {
                let status = Command::new(payload)
                    .args([
                        "/VERYSILENT",
                        "/SUPPRESSMSGBOXES",
                        "/NOCANCEL",
                        "/NORESTART",
                        "/RESTARTEXITCODE=3010",
                        "/NOCLOSEAPPLICATIONS",
                        "/NORESTARTAPPLICATIONS",
                    ])
                    .arg(format!("/DIR={}", root.display()))
                    .arg(format!("/LOG={}", log.display()))
                    .status()
                    .map_err(error)?;
                let code = status.code().unwrap_or(-1);
                // Written before restart so a fast app launch sees the outcome.
                fs::write(&plan.report, format!("Installer exit code: {code}. Restarting jvid; check the installed version. Installer log: {}", log.display())).map_err(error)?;
                Ok(code)
            },
            || {
                Command::new(&plan.executable)
                    .spawn()
                    .map(|_| ())
                    .map_err(error)
            },
        )
    })();
    // The report is outside the install tree and is shown on the next app launch.
    let message = match &result {
        Ok(()) => "Installer completed and restart was requested.".into(),
        Err(e) => e.clone(),
    };
    fs::write(&plan.report, message).map_err(error)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{Asset, Manifest};
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};

    #[test]
    fn staged_payload_is_reverified_and_version_bound_before_execution() {
        let dir = tempfile::tempdir().unwrap();
        let payload = b"signed installer fixture";
        let asset = Asset {
            target: "x86_64-pc-windows-msvc".into(),
            distribution: "windows-installer".into(),
            name: "jvid-1.2.0-windows-x86_64-setup.exe".into(),
            size: payload.len() as u64,
            sha256: format!("{:x}", Sha256::digest(payload)),
        };
        let m = Manifest {
            schema: 1,
            repository: crate::REPOSITORY.into(),
            version: "1.2.0".into(),
            notes: String::new(),
            assets: vec![asset.clone()],
        };
        let bytes = serde_json::to_vec(&m).unwrap();
        let key = SigningKey::from_bytes(&[43; 32]);
        fs::write(dir.path().join(MANIFEST), &bytes).unwrap();
        fs::write(dir.path().join(SIGNATURE), key.sign(&bytes).to_bytes()).unwrap();
        fs::write(dir.path().join(&asset.name), payload).unwrap();
        let mut plan = Plan {
            executable: dir.path().join("jvid-app.exe"),
            parent_pid: 42,
            tag: "v1.2.0".into(),
            report: dir.path().join("report"),
        };
        let verify = |p: &Plan, current| {
            validate(
                dir.path(),
                p,
                &key.verifying_key().to_bytes(),
                "x86_64-pc-windows-msvc",
                current,
            )
        };
        assert!(verify(&plan, "1.0.0").is_ok());
        assert!(verify(&plan, "1.2.0").is_err());
        assert!(verify(&plan, "2.0.0").is_err());
        plan.tag = "v1.3.0".into();
        assert!(verify(&plan, "1.0.0").is_err());
        plan.tag = "v1.2.0".into();
        assert!(
            validate(
                dir.path(),
                &plan,
                &key.verifying_key().to_bytes(),
                "aarch64-apple-darwin",
                "1.0.0"
            )
            .is_err()
        );
        // Tampering after the initial download check is caught on the helper path.
        fs::write(dir.path().join(&asset.name), b"corrupt").unwrap();
        assert!(verify(&plan, "1.0.0").is_err());
    }

    #[test]
    fn installer_failure_cancel_and_permission_errors_attempt_old_app_restart() {
        for code in [0, 1, 2, 5, 3010] {
            let mut restarted = false;
            let result = install_and_restart(
                || Ok(code),
                || {
                    restarted = true;
                    Ok(())
                },
            );
            assert!(restarted);
            assert_eq!(result.is_ok(), code == 0);
        }
        let mut restarted = false;
        assert!(
            install_and_restart(
                || Err("Access denied".into()),
                || {
                    restarted = true;
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(restarted);
        assert!(
            install_and_restart(|| Ok(0), || Err("missing executable".into()))
                .unwrap_err()
                .contains("Launch jvid manually")
        );
        let err = install_and_restart(
            || Err("installer failed".into()),
            || Err("restart failed".into()),
        )
        .unwrap_err();
        assert!(err.contains("installer failed") && err.contains("restart failed"));
    }

    /// Exercises native Windows waiting, .exe invocation, arguments, and relaunch using a
    /// harmless locally compiled fixture. This is deliberately not an Inno upgrade test.
    #[cfg(windows)]
    #[test]
    fn native_windows_helper_waits_runs_verified_fixture_and_restarts_after_success_or_failure() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("installed with spaces");
        fs::create_dir(&root).unwrap();
        let stage = dir.path().join("stage");
        fs::create_dir(&stage).unwrap();
        let source = dir.path().join("fixture.rs");
        fs::write(&source, r#"
            fn main() {
                let here = std::env::current_exe().unwrap();
                if let Some(dir) = std::env::args().find_map(|a| a.strip_prefix("/DIR=").map(String::from)) {
                    let root = std::path::Path::new(&dir);
                    let code: i32 = std::fs::read_to_string(root.join("exit-code")).unwrap().parse().unwrap();
                    if code == 0 { std::fs::write(root.join("version"), b"1.2.0").unwrap(); }
                    std::process::exit(code);
                }
                let root = here.parent().unwrap();
                let version = std::fs::read(root.join("version")).unwrap();
                std::fs::write(root.join("restarted"), version).unwrap();
            }
        "#).unwrap();
        let exe = root.join("jvid-app.exe");
        let compiled = Command::new("rustc")
            .arg(&source)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        fs::write(root.join("jvid-distribution"), b"windows-installer\n").unwrap();
        let name = "jvid-1.2.0-windows-x86_64-setup.exe";
        let payload = fs::read(&exe).unwrap();
        fs::write(stage.join(name), &payload).unwrap();
        let key = SigningKey::from_bytes(&[43; 32]);
        let manifest = Manifest {
            schema: 1,
            repository: crate::REPOSITORY.into(),
            version: "1.2.0".into(),
            notes: "Fixture".into(),
            assets: vec![Asset {
                target: TARGET.into(),
                distribution: "windows-installer".into(),
                name: name.into(),
                size: payload.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&payload)),
            }],
        };
        let bytes = serde_json::to_vec(&manifest).unwrap();
        fs::write(stage.join(MANIFEST), &bytes).unwrap();
        fs::write(stage.join(SIGNATURE), key.sign(&bytes).to_bytes()).unwrap();
        let powershell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        for code in [0, 5] {
            fs::write(root.join("version"), b"1.0.0").unwrap();
            fs::write(root.join("exit-code"), code.to_string()).unwrap();
            let _ = fs::remove_file(root.join("restarted"));
            // Helper must wait for an actual Windows process, not a WSL PID.
            let mut parent = Command::new(&powershell)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "Start-Sleep -Seconds 2",
                ])
                .spawn()
                .unwrap();
            let plan = Plan {
                executable: exe.clone(),
                parent_pid: parent.id(),
                tag: "v1.2.0".into(),
                report: dir.path().join("result.txt"),
            };
            fs::write(stage.join("plan.json"), serde_json::to_vec(&plan).unwrap()).unwrap();
            assert_eq!(
                run_helper_with_key(&stage, &key.verifying_key().to_bytes()).is_ok(),
                code == 0
            );
            assert!(parent.try_wait().unwrap().is_some());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !root.join("restarted").is_file() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            assert_eq!(
                fs::read_to_string(root.join("restarted")).unwrap(),
                if code == 0 { "1.2.0" } else { "1.0.0" }
            );
            assert!(exe.is_file());
        }
        // A changed staged executable cannot run, and the existing install stays usable.
        fs::write(stage.join(name), b"corrupt").unwrap();
        assert!(run_helper_with_key(&stage, &key.verifying_key().to_bytes()).is_err());
        assert!(exe.is_file());
    }
}
