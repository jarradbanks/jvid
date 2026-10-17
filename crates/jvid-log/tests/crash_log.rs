//! Own test binary: the panic hook is process-global.

use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn forced_panic_writes_crash_log_with_backtrace() {
    let dir = std::env::temp_dir().join(format!("jvid-log-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    static CALLED: AtomicBool = AtomicBool::new(false);
    jvid_log::on_panic(|| CALLED.store(true, Ordering::SeqCst));
    jvid_log::install_panic_hook("test", dir.clone());

    assert!(std::panic::catch_unwind(|| panic!("forced")).is_err());

    let _ = std::panic::take_hook();
    assert!(CALLED.load(Ordering::SeqCst));
    let log = std::fs::read_dir(&dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let text = std::fs::read_to_string(&log).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(text.contains("forced"), "{text}");
    assert!(text.contains("backtrace:"), "{text}");
    assert!(
        text.contains("forced_panic_writes_crash_log_with_backtrace"),
        "{text}"
    );
    assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
    assert!(text.contains(std::env::consts::OS), "{text}");
}
