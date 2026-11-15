//! The jvid desktop video editor, built with eframe and j-ui.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

// Subsecond patches the binary crate; the CLI and normal app still share the library.
#[cfg(all(debug_assertions, feature = "hot-reload", not(test)))]
#[path = "lib.rs"]
pub mod desktop;
#[cfg(all(debug_assertions, feature = "hot-reload", not(test)))]
pub(crate) use desktop::*;
#[cfg(not(all(debug_assertions, feature = "hot-reload", not(test))))]
use jvid_app as desktop;

fn main() -> eframe::Result {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--smoke-test")) {
        let Some(dir) = args.next() else {
            eprintln!("--smoke-test requires an output directory");
            std::process::exit(2);
        };
        let result = desktop::package_smoke::run(std::path::Path::new(&dir));
        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                eprintln!("package smoke test failed: {error}");
                std::process::exit(1);
            }
        }
    }
    #[cfg(all(debug_assertions, feature = "hot-reload"))]
    dioxus_devtools::connect_subsecond();
    if let Err(error) = jvid_paths::check_portable_writable() {
        rfd::MessageDialog::new()
            .set_title(desktop::i18n::verbatim("jvid"))
            .set_description(j_ui::tr!(
                desktop::i18n::CATALOG,
                "components.portable_error",
                error = error
            ))
            .set_level(rfd::MessageLevel::Error)
            .show();
        return Ok(());
    }
    let _log = jvid_log::init("jvid-app", jvid_log::log_dir());
    // Missing FFmpeg disables media, not the app; the log says why.
    if let Err(e) = jvid_media::ffmpeg::init() {
        tracing::error!("ffmpeg failed to initialise: {e}");
    }
    if let Some(dir) = desktop::settings_file::themes_dir()
        && let Err(e) = j_ui::themes::install_bundled(&dir)
    {
        tracing::warn!("themes: can't set up {}: {e}", dir.display());
    }
    desktop::app::run(desktop::settings_file::load())
}
