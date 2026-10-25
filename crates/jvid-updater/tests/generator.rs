use ed25519_dalek::SigningKey;
use jvid_updater::{
    MANIFEST, SIGNATURE,
    metadata::{self, PACKAGES},
};
use std::{fs, process::Command};

#[test]
fn pipeline_generator_signs_exact_final_artifacts_and_rejects_a_mismatched_key() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes.txt");
    fs::write(&notes, "Release notes\n").unwrap();
    for &(_, _, suffix) in PACKAGES {
        fs::write(
            dir.path().join(format!("jvid-1.2.0-{suffix}")),
            format!("payload:{suffix}"),
        )
        .unwrap();
    }
    let signing = SigningKey::from_bytes(&[43; 32]);
    let key = signing.verifying_key().to_bytes();
    let public = key.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let invoke = |public: &str| {
        Command::new(env!("CARGO_BIN_EXE_jvid-update-metadata"))
            .arg("v1.2.0")
            .arg(&notes)
            .arg(dir.path())
            .env("JVID_UPDATE_SIGNING_KEY", "2b".repeat(32))
            .env("JVID_UPDATE_PUBLIC_KEY", public)
            .output()
            .unwrap()
    };
    let good = invoke(&public);
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    let bytes = fs::read(dir.path().join(MANIFEST)).unwrap();
    let sig = fs::read(dir.path().join(SIGNATURE)).unwrap();
    let manifest = metadata::verify(&bytes, &sig, &key).unwrap();
    assert_eq!(manifest.assets.len(), PACKAGES.len());
    for asset in &manifest.assets {
        let payload = fs::read(dir.path().join(&asset.name)).unwrap();
        jvid_updater::network::copy_verified(
            payload.as_slice(),
            std::io::sink(),
            asset,
            &std::sync::atomic::AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    }
    let bad = invoke(&"00".repeat(32));
    assert!(!bad.status.success());
    assert!(!String::from_utf8_lossy(&bad.stderr).contains(&"2b".repeat(32)));
    assert_eq!(fs::read(dir.path().join(MANIFEST)).unwrap(), bytes);
    assert_eq!(fs::read(dir.path().join(SIGNATURE)).unwrap(), sig);
}

#[test]
fn pipeline_generator_without_production_configuration_never_emits_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes.txt");
    fs::write(&notes, "Release notes").unwrap();
    for (seed, public) in [
        (None, None),
        (Some("2b".repeat(32)), None),
        (None, Some("00".repeat(32))),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jvid-update-metadata"));
        command
            .args(["1.2.0"])
            .arg(&notes)
            .arg(dir.path())
            .env_remove("JVID_UPDATE_SIGNING_KEY")
            .env_remove("JVID_UPDATE_PUBLIC_KEY");
        if let Some(seed) = seed {
            command.env("JVID_UPDATE_SIGNING_KEY", seed);
        }
        if let Some(public) = public {
            command.env("JVID_UPDATE_PUBLIC_KEY", public);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(!dir.path().join(MANIFEST).exists());
        assert!(!dir.path().join(SIGNATURE).exists());
        assert!(!String::from_utf8_lossy(&output.stderr).contains(&"2b".repeat(32)));
    }
}
