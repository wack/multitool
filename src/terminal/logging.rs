use std::sync::Once;

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
            // Scope the subscriber to ONLY the multitool module.
            .with_env_filter(format!("multitool={}", level.to_string()))
            .compact()
            .finish();

        tracing::subscriber::set_global_default(subscriber)
            .expect("setting tracing default failed");
    });
}
