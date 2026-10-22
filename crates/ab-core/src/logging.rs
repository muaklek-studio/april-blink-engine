use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

const DEFAULT_FILTER: &str = "info,wgpu_core=warn,wgpu_hal=warn,naga=warn";

/// Log output kept in memory until [`log_to_directory`] opens the file, so the first lines
/// of a run (before the game knows its data folder) are not lost.
const MAX_PENDING_LOG: usize = 256 * 1024;

/// Name of the current run's log file in the log folder; the run before is `previous.log`.
pub const LOG_FILE_NAME: &str = "latest.log";

#[derive(Debug, Default)]
struct LogSink {
    file: Option<(File, PathBuf)>,
    pending: Vec<u8>,
}

static LOG_SINK: Mutex<LogSink> = Mutex::new(LogSink {
    file: None,
    pending: Vec::new(),
});

fn sink() -> MutexGuard<'static, LogSink> {
    // A panic while logging must not disable logging for the rest of the run.
    LOG_SINK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The `tracing` writer behind the log file.
#[derive(Debug, Clone, Copy)]
struct FileWriter;

impl Write for FileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut sink = sink();
        let sink = &mut *sink;
        match &mut sink.file {
            // A full disk must not take the game down: drop the line.
            Some((file, _)) => {
                let _ = file.write_all(buf);
            }
            None if sink.pending.len() + buf.len() <= MAX_PENDING_LOG => {
                sink.pending.extend_from_slice(buf)
            }
            None => {}
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some((file, _)) = &mut sink().file {
            let _ = file.flush();
        }
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for FileWriter {
    type Writer = FileWriter;

    fn make_writer(&'a self) -> Self::Writer {
        *self
    }
}

/// Installs the global `tracing` subscriber: the console, plus the log file once
/// [`log_to_directory`] is called.
///
/// The filter is read from `RUST_LOG` and defaults to `info`, with the very chatty GPU stack
/// (wgpu, naga) limited to warnings. Calling this more than once is
/// harmless: later calls are ignored instead of panicking, which keeps tests and tools simple.
pub fn init_logging() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(true))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(true)
                .with_writer(FileWriter),
        )
        .try_init();
}

/// Writes the log to `<dir>/latest.log` from now on (with everything logged so far), and keeps
/// the previous run's log as `previous.log`. Players' bug reports, and the only place a shipped
/// Windows game, which has no console, leaves its log. Returns the log file's path.
pub fn log_to_directory(dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(LOG_FILE_NAME);
    let mut sink = sink();
    if sink.file.as_ref().is_some_and(|(_, p)| *p == path) {
        return Ok(path);
    }
    if path.exists() {
        let _ = std::fs::rename(&path, dir.join("previous.log"));
    }
    let mut file = File::create(&path)?;
    let pending = std::mem::take(&mut sink.pending);
    let _ = file.write_all(&pending);
    sink.file = Some((file, path.clone()));
    Ok(path)
}

/// The log file [`log_to_directory`] opened, if any.
pub fn log_file_path() -> Option<PathBuf> {
    sink().file.as_ref().map(|(_, path)| path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_lines_reach_the_file_and_runs_rotate() {
        let dir = std::env::temp_dir().join(format!("ab-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        FileWriter.write_all(b"before the file\n").unwrap();
        let path = log_to_directory(&dir).unwrap();
        FileWriter.write_all(b"after\n").unwrap();
        FileWriter.flush().unwrap();
        assert_eq!(log_file_path().as_deref(), Some(path.as_path()));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("before the file\n") && text.ends_with("after\n"));
        // Same folder again: keeps writing to the same file.
        assert_eq!(log_to_directory(&dir).unwrap(), path);
        assert!(!dir.join("previous.log").exists());

        // A folder holding an older run's log keeps it as `previous.log`.
        let other = dir.join("next");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(LOG_FILE_NAME), "old run").unwrap();
        log_to_directory(&other).unwrap();
        assert_eq!(
            std::fs::read_to_string(other.join("previous.log")).unwrap(),
            "old run"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
