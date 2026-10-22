//! Log capture: a `tracing` layer that keeps recent events for the Console panel.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Events kept for the console.
const CAPACITY: usize = 5000;

#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: Level,
    pub target: String,
    pub message: String,
    /// Seconds since the editor started.
    pub time: f32,
}

/// Shared, bounded list of recent log lines.
#[derive(Debug, Clone, Default)]
pub struct LogBuffer {
    lines: Arc<Mutex<VecDeque<LogLine>>>,
}

impl LogBuffer {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<LogLine>> {
        self.lines.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn clear(&self) {
        self.lock().clear();
    }

    fn push(&self, line: LogLine) {
        let mut lines = self.lock();
        if lines.len() == CAPACITY {
            lines.pop_front();
        }
        lines.push_back(line);
    }
}

struct ConsoleLayer {
    buffer: LogBuffer,
    start: Instant,
}

/// Collects the message and the other fields of an event as `key=value`.
#[derive(Default)]
struct Fields {
    message: String,
    rest: String,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.rest, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.rest, " {}={value}", field.name());
        }
    }
}

impl<S: Subscriber> Layer<S> for ConsoleLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let metadata = event.metadata();
        self.buffer.push(LogLine {
            level: *metadata.level(),
            target: metadata.target().to_owned(),
            message: format!("{}{}", fields.message, fields.rest),
            time: self.start.elapsed().as_secs_f32(),
        });
    }
}

/// Installs logging to stderr and to the returned buffer. `RUST_LOG` overrides the filter.
pub fn install() -> LogBuffer {
    let buffer = LogBuffer::default();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info,wgpu_core=warn,wgpu_hal=error,naga=warn,eframe=warn,egui_wgpu=warn")
    });
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .with(ConsoleLayer {
            buffer: buffer.clone(),
            start: Instant::now(),
        })
        .try_init();
    buffer
}
