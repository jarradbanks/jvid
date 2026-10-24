//! The jvid desktop video editor, built with eframe and suite-ui.

use jvid_app::{app, settings_file};

fn main() -> eframe::Result {
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
