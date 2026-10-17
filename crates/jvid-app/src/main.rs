//! The jvid desktop video editor, built with eframe and suite-ui.

mod app;
mod import;
mod keymap;
mod media_bin;
mod project_file;
mod settings_file;

fn main() -> eframe::Result {
    let _log = jvid_log::init("jvid-app", jvid_log::log_dir());
    // Missing FFmpeg disables media, not the app; the log says why.
    if let Err(e) = jvid_media::ffmpeg::init() {
        tracing::error!("ffmpeg failed to initialise: {e}");
    }
    let _settings = settings_file::load();
    app::run()
}
