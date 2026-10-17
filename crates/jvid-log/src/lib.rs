//! Logging and crash logs shared by the jvid binaries. Kept out of jvid-core (no I/O there)
//! and free of UI deps so jvid-cli can use it.

use std::backtrace::Backtrace;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs, io};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

type PanicCallback = Box<dyn Fn() + Send + Sync>;
static PANIC_CALLBACKS: Mutex<Vec<PanicCallback>> = Mutex::new(Vec::new());

/// `<platform local data dir>/jvid/logs`, or `None` if the platform has no data dir.
pub fn log_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "jvid").map(|d| d.data_local_dir().join("logs"))
}

/// Sets up tracing (level from `JVID_LOG`, default `info`) to stderr and a daily rolling file
/// in `dir`, and installs the crash-log panic hook. Keep the guard alive until exit so
/// buffered log lines are flushed.
pub fn init(app: &str, dir: Option<PathBuf>) -> Option<WorkerGuard> {
    let filter = || EnvFilter::try_from_env("JVID_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_filter(filter());
    let file = dir.as_ref().and_then(|dir| {
        Builder::new()
            .rotation(Rotation::DAILY)
            .filename_prefix(app)
            .filename_suffix("log")
            .max_log_files(7)
            .build(dir)
            .ok()
    });
    let (file_layer, guard) = match file {
        Some(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer)
                .with_filter(filter());
            (Some(layer), Some(guard))
        }
        None => (None, None),
    };
    let _ = tracing_subscriber::registry()
        .with(stderr)
        .with(file_layer)
        .try_init();
    // No data dir: crash logs still go somewhere findable.
    install_panic_hook(
        app,
        dir.unwrap_or_else(|| std::env::temp_dir().join("jvid")),
    );
    guard
}

/// Registers a callback run on panic before the crash log is written (for autosave, #71).
pub fn on_panic(callback: impl Fn() + Send + Sync + 'static) {
    PANIC_CALLBACKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Box::new(callback));
}

/// Installs a panic hook that runs [`on_panic`] callbacks, writes a crash log into `dir`,
/// then chains to the previous hook.
pub fn install_panic_hook(app: &str, dir: PathBuf) {
    let app = app.to_owned();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        for callback in PANIC_CALLBACKS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
        {
            callback();
        }
        match write_crash_log(&app, &dir, info) {
            Ok(path) => tracing::error!("crash log written to {}", path.display()),
            Err(e) => eprintln!("failed to write crash log: {e}"),
        }
        previous(info);
    }));
}

fn write_crash_log(app: &str, dir: &Path, info: &PanicHookInfo) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let path = dir.join(format!("crash-{app}-{secs}-{}.log", std::process::id()));
    let report = format!(
        "{app} {version} crashed\nos: {os}/{arch}\nunix time: {secs}\n{info}\n\nbacktrace:\n{bt}\n",
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        bt = Backtrace::force_capture(),
    );
    fs::write(&path, report)?;
    Ok(path)
}
