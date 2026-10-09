//! A bridge from the `log` facade into the editor's Console pane.
//!
//! The engine logs through `log`, so a runtime warning or a scene that failed
//! to deserialize would otherwise go to a terminal the editor does not have.
//! [`install`] sets a global logger that parks records in a buffer; the editor
//! drains that buffer into `state.console` once a frame. Panics route through
//! the same path, so a crash in a step is visible in the Console instead of
//! only on stderr.
//!
//! This is deliberately the thinnest thing that works: records are buffered as
//! formatted lines between drains, not kept as structured spans. The Console is
//! a log view, so a line is all it needs.

use std::panic;
use std::sync::{Mutex, OnceLock};

use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::state::{EditorState, LogLevel};

/// Cap on records held between drains. Matches the Console's own retained-line
/// cap, so a log storm in a single frame cannot grow the buffer without bound.
const MAX_BUFFERED: usize = 500;

struct LogBuffer(Mutex<Vec<(LogLevel, String)>>);

impl LogBuffer {
    fn push(&self, level: LogLevel, text: String) {
        let mut lines = self.0.lock().expect("log buffer poisoned");
        lines.push((level, text));
        if lines.len() > MAX_BUFFERED {
            let overflow = lines.len() - MAX_BUFFERED;
            lines.drain(..overflow);
        }
    }

    fn drain(&self) -> Vec<(LogLevel, String)> {
        std::mem::take(&mut *self.0.lock().expect("log buffer poisoned"))
    }
}

/// The single global buffer, created on first use so [`Logger`] can hold a
/// `'static` reference to it.
fn buffer() -> &'static LogBuffer {
    static BUFFER: OnceLock<LogBuffer> = OnceLock::new();
    BUFFER.get_or_init(|| LogBuffer(Mutex::new(Vec::new())))
}

struct Logger;

static LOGGER: Logger = Logger;

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= LevelFilter::Info
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = match record.level() {
            Level::Error => LogLevel::Error,
            Level::Warn => LogLevel::Warn,
            _ => LogLevel::Info,
        };
        buffer().push(level, record.args().to_string());
    }

    fn flush(&self) {}
}

/// Install the global logger and panic hook, once per process.
///
/// Idempotent: a second call (a second editor window) is a no-op rather than an
/// error, and if some other part of the process already claimed the global
/// logger, the panic hook still routes to our buffer.
pub fn install() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        // Another logger may already own the facade (a test harness, say).
        // Losing the race is fine — the panic hook below still runs.
        let _ = log::set_logger(&LOGGER);
        log::set_max_level(LevelFilter::Info);

        // Chain instead of replace: whatever the process had (eframe's own
        // default, typically) still runs after we record the panic.
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            log::error!("panic: {info}");
            previous(info);
        }));
    });
}

/// Move everything buffered since the last call into the Console.
pub fn drain_into(state: &mut EditorState) {
    for (level, text) in buffer().drain() {
        state.log(level, text);
    }
}
