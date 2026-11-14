//! jvid's existing release identity. Update, verification and installation live in j-update.
use j_update::{Config, InstallConfig, Package, Result, metadata};

pub const REPOSITORY: &str = "jarradbanks/jvid";
pub const TARGET: &str = env!("JVID_UPDATE_TARGET");
pub const VERSION: &str = match option_env!("JVID_UPDATE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
pub const MANIFEST: &str = "jvid-update.json";
pub const SIGNATURE: &str = "jvid-update.json.sig";

/// Keep the published schema-1 contract understood by already-installed jvid builds.
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

/// App and helper use the same build-time public pin, never configuration from a stage.
pub fn config(distribution: &str) -> Result<Config> {
    let key = metadata::unhex(option_env!("JVID_UPDATE_PUBLIC_KEY").unwrap_or("").trim()).map_err(
        |_| "This build has no valid release verification key; installation is disabled.",
    )?;
    Ok(with_public_key(key, distribution))
}

/// Release tooling supplies the existing repository variable at runtime.
pub fn with_public_key(public_key: [u8; 32], distribution: &str) -> Config {
    Config {
        releases_repo: REPOSITORY.into(),
        public_key,
        asset_prefix: "jvid".into(),
        manifest_name: MANIFEST.into(),
        signature_name: SIGNATURE.into(),
        packages: PACKAGES
            .iter()
            .map(|&(target, distribution, suffix)| Package {
                target: target.into(),
                distribution: distribution.into(),
                suffix: suffix.into(),
            })
            .collect(),
        target: TARGET.into(),
        distribution: distribution.into(),
        install: InstallConfig {
            distribution_marker: "jvid-distribution".into(),
            portable_markers: vec!["jvid.portable".into()],
            executables: vec!["jvid-app.exe".into()],
            helper_name: "jvid-update-helper.exe".into(),
            process_names: vec!["jvid-app".into()],
        },
    }
}

pub fn current_install() -> Result<Config> {
    let mut config = config("external")?;
    if let Ok(exe) = std::env::current_exe() {
        config.distribution = metadata::distribution(&config, &exe);
    }
    Ok(config)
}
