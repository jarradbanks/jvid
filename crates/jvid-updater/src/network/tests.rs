use super::*;
use crate::metadata::{Manifest, PACKAGES};
use ed25519_dalek::{Signer, SigningKey};
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::sync::Arc;

fn fixture(version: &str) -> (Manifest, Vec<u8>, Vec<u8>, [u8; 32]) {
    let payload = b"signed installer fixture";
    let m = Manifest {
        schema: 1,
        repository: REPOSITORY.into(),
        version: version.into(),
        notes: "Fixes and improvements".into(),
        assets: PACKAGES
            .iter()
            .map(|(t, d, s)| Asset {
                target: (*t).into(),
                distribution: (*d).into(),
                name: format!("jvid-{version}-{s}"),
                size: payload.len() as u64,
                sha256: format!("{:x}", Sha256::digest(payload)),
            })
            .collect(),
    };
    let bytes = serde_json::to_vec(&m).unwrap();
    // This key is test-only, never an app default or a release key.
    let key = SigningKey::from_bytes(&[43; 32]);
    let sig = key.sign(&bytes).to_bytes().to_vec();
    (m, bytes, sig, key.verifying_key().to_bytes())
}

#[test]
fn authentic_metadata_is_bound_to_repository_schema_target_format_name_and_version() {
    let (m, bytes, sig, key) = fixture("1.2.0");
    let verified = metadata::verify(&bytes, &sig, &key).unwrap();
    for &(target, dist, _) in PACKAGES {
        assert_eq!(verified.asset(target, dist), m.asset(target, dist));
    }
    assert!(
        verified
            .asset("aarch64-pc-windows-msvc", "windows-installer")
            .is_none()
    );
    assert!(
        verified
            .asset("x86_64-pc-windows-msvc", "linux-portable")
            .is_none()
    );
    assert!(metadata::verify(&bytes, &sig, &[9; 32]).is_err());
    assert!(metadata::verify(&bytes, &sig[..63], &key).is_err());
    let mut tampered = bytes.clone();
    tampered[1] ^= 1;
    assert!(metadata::verify(&tampered, &sig, &key).is_err());
    for mutate in [
        |m: &mut Manifest| m.repository = "attacker/jvid".into(),
        |m: &mut Manifest| m.schema = 2,
        |m: &mut Manifest| m.version = "1.2.0-beta.1".into(),
        |m: &mut Manifest| m.assets[0].name = "../setup.exe".into(),
        |m: &mut Manifest| m.assets[0].size = 0,
        |m: &mut Manifest| m.assets[0].sha256 = "wrong".into(),
        |m: &mut Manifest| m.assets.push(m.assets[0].clone()),
        |m: &mut Manifest| m.assets[0].target = "aarch64-pc-windows-msvc".into(),
    ] {
        let mut invalid = m.clone();
        mutate(&mut invalid);
        assert!(invalid.validate().is_err());
    }
}

#[test]
fn stable_selection_uses_semver_and_ignores_drafts_prereleases_and_downgrades() {
    let releases: Vec<Release> = serde_json::from_str(
        r#"[
        {"tag_name":"v1.9.0", "draft":false, "prerelease":false},
        {"tag_name":"v1.10.0", "draft":false, "prerelease":false},
        {"tag_name":"v3.0.0", "draft":true, "prerelease":false},
        {"tag_name":"v2.0.0", "draft":false, "prerelease":true},
        {"tag_name":"v9.0.0-beta.1", "draft":false, "prerelease":false},
        {"tag_name":"../../evil", "draft":false, "prerelease":false},
        {"tag_name":"v1.1.0", "draft":false, "prerelease":false}]
    "#,
    )
    .unwrap();
    let selected = stable_releases(releases, &semver::Version::parse("1.1.0").unwrap());
    assert_eq!(
        selected.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(),
        ["v1.10.0", "v1.9.0"]
    );
    assert!(
        stable_releases(
            vec![Release {
                tag_name: "v1.1.0".into(),
                draft: false,
                prerelease: false
            }],
            &semver::Version::parse("1.1.0").unwrap()
        )
        .is_empty()
    );
}

#[test]
fn corruption_truncation_overrun_read_write_failures_and_cancellation_are_rejected() {
    let (m, _, _, _) = fixture("1.2.0");
    let a = &m.assets[0];
    let payload = b"signed installer fixture";
    let cancel = AtomicBool::new(false);
    for bad in [
        &payload[..5],
        b"signed installer fixturX",
        b"signed installer fixture extra",
    ] {
        assert!(copy_verified(bad, Vec::new(), a, &cancel, |_| {}).is_err());
    }
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("offline"))
        }
    }
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("disk full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert!(copy_verified(Broken, Vec::new(), a, &cancel, |_| {}).is_err());
    assert!(copy_verified(payload.as_slice(), Broken, a, &cancel, |_| {}).is_err());
    assert!(
        copy_verified(
            payload.as_slice(),
            Vec::new(),
            a,
            &AtomicBool::new(true),
            |_| {}
        )
        .is_err()
    );
    let mut out = Vec::new();
    let mut progress = Vec::new();
    copy_verified(payload.as_slice(), &mut out, a, &cancel, |n| {
        progress.push(n)
    })
    .unwrap();
    assert_eq!(out, payload);
    assert_eq!(progress.last(), Some(&a.size));
}

/// An actual local HTTP server exercises the same byte caps, listing, signature validation,
/// staging and retry used in production, with no public release and no real installer.
#[test]
fn signed_check_download_cancel_failure_and_retry_over_http() {
    let (m, bytes, sig, key) = fixture("1.2.0");
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.local_addr().unwrap());
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = requests.clone();
    let server = std::thread::spawn(move || {
        for stream in server.incoming().take(7) {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
            }
            captured.lock().unwrap().push(request.clone());
            let payload: &[u8] = if request.contains("/releases?") {
                br#"[{"tag_name":"v1.2.0","draft":false,"prerelease":false}]"#
            } else if request.contains(".json.sig ") {
                &sig
            } else if request.contains(".json ") {
                &bytes
            } else if request.contains("bad ") {
                b"corrupt"
            } else {
                b"signed installer fixture"
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            )
            .unwrap();
            stream.write_all(payload).unwrap();
        }
    });
    let mut client = Client::new(false, format!("{base}/releases"));
    client.downloads = format!("{base}/downloads");
    let available = client
        .check("1.0.0", "x86_64-pc-windows-msvc", "windows-installer", &key)
        .unwrap()
        .unwrap();
    assert_eq!(available.asset, m.assets[0]);
    assert!(
        client
            .download_at(
                available.clone(),
                &format!("{base}/bad"),
                &AtomicBool::new(false),
                |_| {}
            )
            .is_err()
    );
    assert!(
        client
            .download_at(
                available.clone(),
                &format!("{base}/ok"),
                &AtomicBool::new(true),
                |_| {}
            )
            .is_err()
    );
    let downloaded = client
        .download_at(
            available.clone(),
            &format!("{base}/ok"),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    let stage = downloaded.dir.path().to_owned();
    assert_eq!(
        std::fs::read(stage.join(&available.asset.name)).unwrap(),
        b"signed installer fixture"
    );
    drop(downloaded);
    assert!(!stage.exists());
    // Up-to-date does not fetch metadata or payload.
    assert!(
        client
            .check("1.2.0", "x86_64-pc-windows-msvc", "windows-installer", &key)
            .unwrap()
            .is_none()
    );
    server.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 7);
}
