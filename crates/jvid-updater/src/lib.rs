//! Only signed metadata can authorize an install. Network transport is not a trust root.
pub mod install;
pub mod metadata;
pub mod network;
pub mod state;

pub const REPOSITORY: &str = "jarradbanks/jvid";
pub const TARGET: &str = env!("JVID_UPDATE_TARGET");
pub const VERSION: &str = match option_env!("JVID_UPDATE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
pub const MANIFEST: &str = "jvid-update.json";
pub const SIGNATURE: &str = "jvid-update.json.sig";
pub type Result<T> = std::result::Result<T, String>;

pub fn public_key() -> Result<[u8; 32]> {
    metadata::unhex(option_env!("JVID_UPDATE_PUBLIC_KEY").unwrap_or("").trim()).map_err(|_| {
        "This build has no valid release verification key; installation is disabled.".into()
    })
}

pub(crate) fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
