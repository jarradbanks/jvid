//! The app-specific helper pins its own config; j-update owns the installer protocol.
use jvid_update_config::{VERSION, config};

fn run() -> j_update::Result<()> {
    let dir = std::env::args_os()
        .nth(1)
        .ok_or("Missing update directory.")?;
    j_update::install::run_helper(&config("windows-installer")?, VERSION, dir.as_ref(), &())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
