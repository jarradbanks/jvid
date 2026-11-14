//! Keep the protected pipeline's environment/CLI contract; j-update owns signing.
use j_update::{metadata::unhex, release};
use jvid_update_config::with_public_key;
use std::{fs, path::Path};

fn generate() -> j_update::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("Usage: jvid-update-metadata VERSION NOTES_FILE ARTIFACT_DIRECTORY".into());
    }
    let seed = std::env::var("JVID_UPDATE_SIGNING_KEY")
        .map_err(|_| "Set JVID_UPDATE_SIGNING_KEY to a secret 32-byte Ed25519 seed in hex.")?;
    let public = std::env::var("JVID_UPDATE_PUBLIC_KEY")
        .map_err(|_| "Set JVID_UPDATE_PUBLIC_KEY to the pinned public key used in these builds.")?;
    release::generate(
        &with_public_key(unhex(public.trim())?, "external"),
        &args[0].to_string_lossy(),
        &fs::read_to_string(&args[1]).map_err(|e| e.to_string())?,
        Path::new(&args[2]),
        &unhex(seed.trim())?,
    )
}

fn main() {
    if let Err(e) = generate() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
