//! The jvid desktop video editor, built with eframe and suite-ui.

use jvid_app::{app, settings_file};

fn main() -> eframe::Result {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--smoke-test")) {
        let Some(dir) = args.next() else {
            eprintln!("--smoke-test requires an output directory");
            std::process::exit(2);
        };
        let result = jvid_app::package_smoke::run(std::path::Path::new(&dir));
        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                eprintln!("package smoke test failed: {error}");
                std::process::exit(1);
            }
        }
    }
    if let Err(error) = jvid_paths::check_portable_writable() {
        rfd::MessageDialog::new()
            .set_title(jvid_app::i18n::verbatim("jvid"))
            .set_description(suite_ui::tr!(
                jvid_app::i18n::CATALOG,
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
    if let Some(dir) = settings_file::themes_dir()
        && let Err(e) = suite_ui::themes::install_bundled(&dir)
    {
        tracing::warn!("themes: can't set up {}: {e}", dir.display());
    }
    app::run(settings_file::load())
}
