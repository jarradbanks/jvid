//! Opt-in real Inno file-upgrade test. No profile installation, shortcuts or registry
//! registration: only the installer identity/integration are isolated from windows.iss.
use super::*;
use crate::metadata::{Asset, Manifest};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

fn command_ok(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture_script() -> String {
    let original = include_str!("../../../../packaging/windows.iss");
    let mut script = String::new();
    let mut section = "";
    for line in original.lines() {
        if line.starts_with('[') {
            section = line;
        }
        // Shortcuts and postinstall UI belong to the disposable-account packaging test.
        if matches!(section, "[Icons]" | "[Run]" | "[Tasks]") {
            continue;
        }
        if line.starts_with("AppId=") {
            script.push_str("AppId=jvid-updater-isolated-inno-fixture\n");
        } else if line.starts_with("DefaultDirName=") {
            // Even accidental omission of /DIR cannot target the real install.
            script.push_str("DefaultDirName={#OutputDir}\\fixture install\n");
        } else if line.starts_with("SetupMutex=") {
            script.push_str("SetupMutex=jvid-updater-isolated-inno-fixture\n");
        } else {
            script.push_str(line);
            script.push('\n');
        }
        if line == "[Setup]" {
            // Inno documents Uninstallable=no as disabling its uninstall registration.
            script.push_str("Uninstallable=no\nCreateUninstallRegKey=no\nUsePreviousAppDir=no\n");
        }
    }
    assert!(script.contains("PrivilegesRequired=lowest"));
    assert!(!script.contains("{localappdata}") && !script.contains("{userprograms}"));
    assert!(!script.contains("[Registry]") && !script.contains("[UninstallRun]"));
    script
}

fn compile_setup(iscc: &Path, script: &Path, stage: &Path, out: &Path, version: &str) -> PathBuf {
    command_ok(
        Command::new(iscc)
            .arg(format!("/DAppVersion={version}"))
            .arg(format!("/DStageDir={}", stage.display()))
            .arg(format!("/DOutputDir={}", out.display()))
            .arg(script),
    );
    out.join(format!("jvid-{version}-windows-x86_64-setup.exe"))
}

fn sign_stage(stage: &Path, setup: &Path, version: &str, key: &SigningKey) {
    let name = setup.file_name().unwrap().to_str().unwrap();
    let payload = fs::read(setup).unwrap();
    fs::write(stage.join(name), &payload).unwrap();
    let manifest = Manifest {
        schema: 1,
        repository: crate::REPOSITORY.into(),
        version: version.into(),
        notes: "Disposable Inno integration fixture".into(),
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
}

#[test]
#[ignore = "requires native Windows and JVID_INNO_ISCC; installs only into a temporary directory"]
fn native_inno_upgrade_waits_replaces_restarts_preserves_and_rejects_portable() {
    let iscc = PathBuf::from(std::env::var_os("JVID_INNO_ISCC").expect("Set JVID_INNO_ISCC"));
    assert!(iscc.is_file(), "Inno compiler missing: {}", iscc.display());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("installed with spaces");
    let stage = dir.path().join("staged download");
    let payload = dir.path().join("program files");
    let out = dir.path().join("setups");
    for path in [&stage, &payload, &out] {
        fs::create_dir(path).unwrap();
    }
    let script = dir.path().join("fixture.iss");
    fs::write(&script, fixture_script()).unwrap();
    fs::write(
        payload.join("DISTRIBUTION-LICENSE.txt"),
        b"Disposable fixture",
    )
    .unwrap();
    fs::write(payload.join("jvid-distribution"), b"windows-installer\n").unwrap();
    // These files must be excluded by the real packaging [Files] section.
    fs::write(payload.join("jvid.portable"), b"must not be installed").unwrap();
    fs::create_dir(payload.join("jvid-data")).unwrap();
    fs::write(
        payload.join("jvid-data/settings.toml"),
        b"must not overwrite",
    )
    .unwrap();
    let source = dir.path().join("app.rs");
    let compile_app = |version: &str| {
        fs::write(
            &source,
            format!(
                r#"
            fn main() {{
                let exe = std::env::current_exe().unwrap();
                let root = exe.parent().unwrap();
                if std::env::args().any(|a| a == "--hold") {{
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }} else {{
                    std::fs::write(root.join("restarted"), "{version}").unwrap();
                }}
            }}
        "#
            ),
        )
        .unwrap();
        command_ok(
            Command::new("rustc")
                .arg(&source)
                .arg("-o")
                .arg(payload.join("jvid-app.exe")),
        );
    };
    compile_app("old");
    let old = compile_setup(&iscc, &script, &payload, &out, "0.0.1");
    command_ok(
        Command::new(old)
            .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
            .arg(format!("/DIR={}", root.display())),
    );
    assert!(!root.join("jvid.portable").exists());
    assert!(!root.join("jvid-data/settings.toml").exists());
    command_ok(&mut Command::new(root.join("jvid-app.exe")));
    assert_eq!(fs::read_to_string(root.join("restarted")).unwrap(), "old");
    fs::remove_file(root.join("restarted")).unwrap();
    // Preservation sentinels represent user-owned data; real Known Folder paths are
    // deliberately reserved for the clean-account manual validation.
    let sentinels = [
        "project.jvid",
        "settings.toml",
        "models/model.bin",
        "jvid-data/settings.toml",
    ];
    for relative in sentinels {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("preserve:{relative}")).unwrap();
    }
    // The upgrade is always newer than this build; version bumps cannot stale the test.
    let mut next = semver::Version::parse(VERSION).unwrap();
    next.major += 1;
    next.minor = 0;
    next.patch = 0;
    next.pre = semver::Prerelease::EMPTY;
    next.build = semver::BuildMetadata::EMPTY;
    let next = next.to_string();
    compile_app(&next);
    let setup = compile_setup(&iscc, &script, &payload, &out, &next);
    let key = SigningKey::from_bytes(&[43; 32]); // Public, disposable fixture only.
    sign_stage(&stage, &setup, &next, &key);
    let mut parent = Command::new(root.join("jvid-app.exe"))
        .arg("--hold")
        .spawn()
        .unwrap();
    let plan = Plan {
        executable: root.join("jvid-app.exe"),
        parent_pid: parent.id(),
        tag: format!("v{next}"),
        report: dir.path().join("update-result.txt"),
    };
    fs::write(stage.join("plan.json"), serde_json::to_vec(&plan).unwrap()).unwrap();
    run_helper_with_key(&stage, &key.verifying_key().to_bytes()).unwrap();
    assert!(parent.try_wait().unwrap().is_some());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while fs::read_to_string(root.join("restarted")).ok().as_deref() != Some(&next)
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(fs::read_to_string(root.join("restarted")).unwrap(), next);
    for relative in sentinels {
        assert_eq!(
            fs::read_to_string(root.join(relative)).unwrap(),
            format!("preserve:{relative}")
        );
    }
    assert!(plan.report.with_extension("installer.log").is_file());
    assert!(
        fs::read_to_string(&plan.report)
            .unwrap()
            .contains("restart was requested")
    );
    // Inno itself must reject a portable directory even when invoked silently.
    fs::write(root.join("jvid.portable"), b"portable").unwrap();
    let rejection_log = dir.path().join("portable-rejection.log");
    let mut rejection = Command::new(setup)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
        .arg(format!("/DIR={}", root.display()))
        .arg(format!("/LOG={}", rejection_log.display()))
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = loop {
        if let Some(status) = rejection.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            // Kill only this test's disposable installer tree; a hidden message box
            // must fail the test rather than wait for input on a developer/CI desktop.
            let taskkill = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
                .join("System32/taskkill.exe");
            let _ = Command::new(taskkill)
                .args(["/PID", &rejection.id().to_string(), "/T", "/F"])
                .output();
            let _ = rejection.wait();
            panic!(
                "Silent portable rejection waited for input or timed out:\n{}",
                fs::read_to_string(&rejection_log).unwrap_or_default()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let log = fs::read_to_string(&rejection_log).unwrap();
    // Initialization/mutex failures must not masquerade as portable rejection.
    assert!(!status.success(), "{status}:\n{log}");
    assert!(
        log.contains("Refusing to install over a portable copy.")
            || log.contains("Cannot install over a portable copy. Choose a different folder."),
        "{status}:\n{log}"
    );
    // The helper also refuses it before invoking any installer or restarting the app.
    fs::remove_file(root.join("restarted")).unwrap();
    assert!(run_helper_with_key(&stage, &key.verifying_key().to_bytes()).is_err());
    assert!(!root.join("restarted").exists());
}
