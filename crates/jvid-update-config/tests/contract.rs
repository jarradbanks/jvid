use ed25519_dalek::{Signer, SigningKey};
use j_update::{install::Plan, metadata};
use jvid_update_config::{MANIFEST, SIGNATURE, with_public_key};

#[test]
fn legacy_signed_release_contract_retains_all_five_packages() {
    let key = SigningKey::from_bytes(&[43; 32]);
    let config = with_public_key(key.verifying_key().to_bytes(), "external");
    config.validate().unwrap();
    assert_eq!(config.releases_repo, "jarradbanks/jvid");
    assert_eq!(config.asset_prefix, "jvid");
    assert_eq!(MANIFEST, "jvid-update.json");
    assert_eq!(SIGNATURE, "jvid-update.json.sig");
    let packages = [
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
    assert_eq!(config.packages.len(), packages.len());
    // Construct the old schema independently of Config and the shared generator.
    let legacy = serde_json::json!({
        "schema": 1, "repository": "jarradbanks/jvid", "version": "1.2.0",
        "notes": "Legacy signed notes", "assets": packages.map(|(target, distribution, suffix)| {
            serde_json::json!({"target": target, "distribution": distribution,
                "name": format!("jvid-1.2.0-{suffix}"), "size": 100, "sha256": "00".repeat(32)})
        }),
    });
    let bytes = serde_json::to_vec(&legacy).unwrap();
    let manifest = metadata::verify(&bytes, &key.sign(&bytes).to_bytes(), &config).unwrap();
    assert_eq!(serde_json::to_value(&manifest).unwrap(), legacy);
    for (target, distribution, suffix) in packages {
        let asset = manifest.asset(target, distribution).unwrap();
        assert_eq!(asset.name, format!("jvid-1.2.0-{suffix}"));
        assert_eq!(
            metadata::self_update(target, distribution),
            distribution == "windows-installer"
        );
    }
}

#[test]
fn installed_and_portable_markers_and_helper_paths_keep_their_names() {
    let config = with_public_key([0; 32], "windows-installer");
    assert_eq!(config.install.distribution_marker, "jvid-distribution");
    assert_eq!(config.install.portable_markers, ["jvid.portable"]);
    assert_eq!(config.install.executables, ["jvid-app.exe"]);
    assert_eq!(config.install.helper_name, "jvid-update-helper.exe");
    assert_eq!(config.install.process_names, ["jvid-app"]);
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("jvid-app.exe");
    assert_eq!(metadata::distribution(&config, &exe), "external");
    std::fs::write(dir.path().join("jvid-distribution"), "windows-installer\n").unwrap();
    assert_eq!(metadata::distribution(&config, &exe), "windows-installer");
    std::fs::write(dir.path().join("jvid.portable"), "").unwrap();
    assert_eq!(metadata::distribution(&config, &exe), "windows-portable");
}

#[test]
fn legacy_staged_plan_keeps_executable_pid_tag_and_report_paths() {
    let legacy = serde_json::json!({
        "executable": "C:\\Users\\fixture\\jvid\\jvid-app.exe",
        "parent_pid": 123, "tag": "v1.2.0",
        "report": "C:\\Users\\fixture\\jvid\\update-result.txt",
    });
    let plan: Plan = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(plan.parent_pid, 123);
    assert_eq!(plan.tag, "v1.2.0");
    assert_eq!(serde_json::to_value(plan).unwrap(), legacy);
}
