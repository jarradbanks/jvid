fn main() {
    let Some(dir) = std::env::args_os().nth(1).map(std::path::PathBuf::from) else {
        std::process::exit(2);
    };
    if let Err(e) = jvid_updater::install::run_helper(&dir) {
        eprintln!("Update failed: {e}");
        std::process::exit(1);
    }
    // Windows cannot unlink the running helper; its private stage is cleaned on a future
    // check by the OS temp policy. Never recursively remove a caller-provided path here.
}
