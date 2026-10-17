//! The jvid desktop video editor, built with eframe and suite-ui.

mod project_file;
mod settings_file;

fn main() {
    let _log = jvid_log::init("jvid-app", jvid_log::log_dir());
    let _settings = settings_file::load();
}
