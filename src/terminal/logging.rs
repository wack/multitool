use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, Once, OnceLock};

use tokio::sync::mpsc::UnboundedSender;
use tracing_subscriber::{filter::LevelFilter, fmt::time::ChronoLocal};

static LOGGER_READY: Once = Once::new();

/// This string is our default local formatter, putting the local time
/// into a human-readinable timestamp. In the future we can add support for
/// other timestamp formats that are more machine readable, but our initial
/// MVP release is focused on human operators.
const CHRONO_LOCAL_FMT: &str = "%c %Z";
// const CHRONO_LOCAL_FMT: &str = "%x %Z";

/// `setup_logger` initializes the Wintermute global logger. This function
/// can be called multiple times; each subsequent call after the first
/// has no effect.
/// # Panics
/// Panics if we cannot initialize the logger.
pub(super) fn setup_logger(level: LevelFilter) {
    LOGGER_READY.call_once(|| {
        let subscriber = tracing_subscriber::fmt::Subscriber::builder()
            .pretty()
            .with_max_level(level)
            .with_timer(ChronoLocal::new(CHRONO_LOCAL_FMT.to_owned()))
            .with_file(false)
            .with_line_number(false)
            .with_target(false)
            // Route every formatted line through `RoutingWriter` instead of
            // writing straight to stdout — see its docs for why.
            .with_writer(RoutingWriter::default)
            // Scope the subscriber to ONLY the multitool module.
            .with_env_filter(format!("multitool={}", level))
            .compact()
            .finish();

        tracing::subscriber::set_global_default(subscriber)
            .expect("setting tracing default failed");
    });
}

/// The currently-registered log sink, tagged with the generation that installed
/// it. Tagging lets a stale guard's `Drop` recognize it's no longer current and
/// skip clearing a newer registration — otherwise two overlapping presenter runs
/// in the same process (e.g. concurrent tests) could race to uninstall each
/// other's sink.
type Sink = (u64, UnboundedSender<String>);
static LOG_SINK: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(0);

fn sink() -> &'static Mutex<Option<Sink>> {
    LOG_SINK.get_or_init(|| Mutex::new(None))
}

/// Route every subsequently-formatted log line to `tx` instead of stdout, until
/// the returned guard drops.
///
/// The live presenter (MULTI-1369 follow-up) becomes the terminal's sole writer
/// for the run — without this, a `tracing::info!` fired mid-run (e.g. "retrying
/// check whose agent did not report") writes raw bytes straight over the inline
/// TUI's cursor-managed viewport and corrupts it. Routing lets the presenter
/// fold log lines into its own display instead.
pub(crate) fn route_logs(tx: UnboundedSender<String>) -> LogRouteGuard {
    let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    *sink().lock().unwrap() = Some((generation, tx));
    LogRouteGuard { generation }
}

/// Restores direct stdout logging when dropped — but only if no newer
/// registration has since replaced this one.
pub(crate) struct LogRouteGuard {
    generation: u64,
}

impl Drop for LogRouteGuard {
    fn drop(&mut self) {
        let mut guard = sink().lock().unwrap();
        if matches!(&*guard, Some((generation, _)) if *generation == self.generation) {
            *guard = None;
        }
    }
}

/// Forward one complete, already-formatted log line to the active
/// [`route_logs`] sink, falling back to stdout when none is registered (i.e.
/// outside a live presenter run — today's unchanged behavior).
fn emit(text: String) {
    let sender = sink().lock().unwrap().as_ref().map(|(_, tx)| tx.clone());
    match sender {
        Some(tx) => {
            // A closed receiver only happens mid-teardown; dropping the line is
            // fine since the presenter is on its way out anyway.
            let _ = tx.send(text);
        }
        None => {
            let _ = writeln!(io::stdout(), "{text}");
        }
    }
}

/// A [`tracing_subscriber`] writer that buffers until it sees a complete line,
/// then hands that line to [`emit`]. `tracing-subscriber`'s formatter isn't
/// guaranteed to make one `write` call per event, so buffering (rather than
/// treating each `write` as a line) is what keeps a single log record intact.
#[derive(Default)]
struct RoutingWriter {
    buf: Vec<u8>,
}

impl Write for RoutingWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            emit(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned());
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for RoutingWriter {
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            emit(String::from_utf8_lossy(&self.buf).into_owned());
        }
    }
}
