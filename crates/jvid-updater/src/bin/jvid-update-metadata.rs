//! Release pipeline hook. Seed is read only from the environment, never a CLI argument/log.
use ed25519_dalek::{Signer, SigningKey};
use jvid_updater::{
    MANIFEST, REPOSITORY, SIGNATURE,
    metadata::{Asset, Manifest, PACKAGES, stable_version, unhex},
};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

fn generate() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("Usage: jvid-update-metadata VERSION NOTES_FILE ARTIFACT_DIRECTORY".into());
    }
    let version = stable_version(&args[0].to_string_lossy())?.to_string();
    let notes = fs::read_to_string(&args[1]).map_err(|e| e.to_string())?;
    let dir = PathBuf::from(&args[2]);
    let seed = std::env::var("JVID_UPDATE_SIGNING_KEY")
        .map_err(|_| "Set JVID_UPDATE_SIGNING_KEY to a secret 32-byte Ed25519 seed in hex.")?;
    let signing = SigningKey::from_bytes(&unhex::<32>(seed.trim())?);
    let expected = std::env::var("JVID_UPDATE_PUBLIC_KEY")
        .map_err(|_| "Set JVID_UPDATE_PUBLIC_KEY to the pinned public key used in these builds.")?;
    if signing.verifying_key().to_bytes() != unhex::<32>(expected.trim())? {
        return Err("Signing key does not match the build's public key.".into());
    }
    let mut assets = Vec::new();
    for &(target, distribution, suffix) in PACKAGES {
        let name = format!("jvid-{version}-{suffix}");
        let path = dir.join(&name);
        if !path.exists() {
            continue;
        }
        let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
        let size = file.metadata().map_err(|e| e.to_string())?.len();
        let mut hash = Sha256::new();
        std::io::copy(&mut file, &mut hash).map_err(|e| e.to_string())?;
        assets.push(Asset {
            target: target.into(),
            distribution: distribution.into(),
            name,
            size,
            sha256: format!("{:x}", hash.finalize()),
        });
    }
    let manifest = Manifest {
        schema: 1,
        repository: REPOSITORY.into(),
        version,
        notes,
        assets,
    };
    manifest.validate()?;
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    let signature = signing.sign(&bytes).to_bytes();
    // Verify using the same parser and strict verifier that ship in the app before writing.
    jvid_updater::metadata::verify(&bytes, &signature, &signing.verifying_key().to_bytes())?;
    fs::write(dir.join(MANIFEST), bytes).map_err(|e| e.to_string())?;
    fs::write(dir.join(SIGNATURE), signature).map_err(|e| e.to_string())?;
    println!("Wrote {MANIFEST} and {SIGNATURE}; upload both alongside the exact hashed artifacts.");
    Ok(())
}

fn main() {
    if let Err(e) = generate() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
