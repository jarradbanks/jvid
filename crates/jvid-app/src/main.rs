//! The jvid desktop video editor, built with eframe and suite-ui.

mod app;
mod keymap;
mod project_file;
mod settings_file;

fn main() -> eframe::Result {
    let _log = jvid_log::init("jvid-app", jvid_log::log_dir());
    let _settings = settings_file::load();
    app::run()
}
