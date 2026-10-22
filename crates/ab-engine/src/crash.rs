//! Crash reports for shipped games: a panic that escapes the game loop, or an error that ends
//! the game, is written to `<data folder>/logs/crash-<time>.txt` with the log file next to it,
//! and shown in a message box on Windows release builds (which have no console).
//!
//! Script panics are caught and recovered by the scene tree; they are logged, not reported
//! as crashes. That is why the panic hook only *remembers* the last panic, and the report is
//! written when the panic reaches [`run_reporting_crashes`].

use std::backtrace::Backtrace;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::error;

/// The last panic seen by the hook: message with location, and its backtrace.
static LAST_PANIC: Mutex<Option<(String, String)>> = Mutex::new(None);
static HOOK: Once = Once::new();

/// Installs the panic hook (once per process; the default hook still prints to stderr).
pub fn install_panic_hook() {
    HOOK.call_once(|| {
        let default = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            default(info);
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "(non-text panic payload)".into());
            let location = info
                .location()
                .map(|l| format!(" at {}:{}", l.file(), l.line()))
                .unwrap_or_default();
            let thread = std::thread::current();
            let summary = format!(
                "panic in thread '{}': {message}{location}",
                thread.name().unwrap_or("<unnamed>")
            );
            error!("{summary}");
            let backtrace = Backtrace::force_capture().to_string();
            *LAST_PANIC.lock().unwrap_or_else(|e| e.into_inner()) = Some((summary, backtrace));
        }));
    });
}

/// Runs `body` and, when it panics or fails, writes a crash report into `log_dir` (and shows
/// it on Windows release builds) before passing the failure on.
pub fn run_reporting_crashes(
    title: &str,
    log_dir: Option<&Path>,
    body: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    install_panic_hook();
    match panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => {
            let text = format!("{err:#}");
            error!("the game stopped with an error: {text}");
            report(title, log_dir, &text, None);
            Err(err)
        }
        Err(payload) => {
            let (summary, backtrace) = LAST_PANIC
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .unwrap_or_else(|| ("panic (no details)".into(), String::new()));
            report(title, log_dir, &summary, Some(&backtrace));
            panic::resume_unwind(payload)
        }
    }
}

/// Writes the report and tells the player (message box on Windows release builds).
fn report(title: &str, log_dir: Option<&Path>, what: &str, backtrace: Option<&str>) {
    let path = log_dir.and_then(|dir| write_report(dir, title, what, backtrace));
    let mut message = format!("{title} stopped unexpectedly.\n\n{what}");
    if let Some(path) = &path {
        message.push_str(&format!(
            "\n\nA crash report was saved to:\n{}",
            path.display()
        ));
    }
    if cfg!(all(windows, not(debug_assertions))) {
        message_box(title, &message);
    }
}

fn write_report(dir: &Path, title: &str, what: &str, backtrace: Option<&str>) -> Option<PathBuf> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let path = dir.join(format!("crash-{millis}.txt"));
    let log = ab_core::log_file_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(no log file)".into());
    let text = format!(
        "{title} crash report\n\
         time (unix ms): {millis}\n\
         version: {version}\n\
         os: {os} {arch}\n\
         log: {log}\n\n\
         {what}\n\n{backtrace}\n",
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        backtrace = backtrace.unwrap_or(""),
    );
    std::fs::create_dir_all(dir).ok()?;
    std::fs::write(&path, text).ok()?;
    error!(report = %path.display(), "crash report written");
    Some(path)
}

/// Shows `text` to the player: a message box on Windows (games have no console there), the
/// terminal elsewhere.
pub fn show_text(title: &str, text: &str) {
    if cfg!(windows) {
        message_box(title, text);
    } else {
        println!("{text}");
    }
}

#[cfg(windows)]
fn message_box(title: &str, text: &str) {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(
            hwnd: *mut core::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            kind: u32,
        ) -> i32;
    }
    const MB_ICONERROR: u32 = 0x10;
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (text, title) = (wide(text), wide(title));
    // SAFETY: both strings are NUL-terminated UTF-16 buffers that outlive the call, and a
    // null owner window is allowed.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn message_box(_title: &str, _text: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_and_escaped_panics_are_reported_caught_ones_are_not() {
        let dir = std::env::temp_dir().join(format!("ab-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let reports = || {
            std::fs::read_dir(&dir).map_or(0, |d| {
                d.filter(|e| {
                    e.as_ref()
                        .is_ok_and(|e| e.file_name().to_string_lossy().starts_with("crash-"))
                })
                .count()
            })
        };

        // A panic caught inside (like a script's) is not a crash.
        run_reporting_crashes("Test", Some(&dir), || {
            let _ = panic::catch_unwind(|| panic!("recovered"));
            Ok(())
        })
        .unwrap();
        assert_eq!(reports(), 0);

        let err = run_reporting_crashes("Test", Some(&dir), || anyhow::bail!("no scene"));
        assert!(err.is_err());
        assert_eq!(reports(), 1);

        let escaped = panic::catch_unwind(|| {
            run_reporting_crashes("Test", Some(&dir), || panic!("boom in the loop"))
        });
        assert!(escaped.is_err(), "the panic still propagates");
        let text: String = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| std::fs::read_to_string(e.unwrap().path()).ok())
            .collect();
        assert!(text.contains("boom in the loop") && text.contains("crash.rs"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
