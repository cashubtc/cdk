//! FFI Logging configuration
//!
//! Provides functions to initialize tracing subscriber for stdout logging.

use std::sync::Once;

static INIT: Once = std::sync::Once::new();

/// Initialize the tracing subscriber for stdout logging.
///
/// This function sets up a tracing subscriber that outputs logs to stdout,
/// making them visible when using the FFI from other languages.
///
/// Call this function once at application startup, before creating
/// any wallets. Subsequent calls are safe but have no effect.
///
/// # Arguments
///
/// * `level` - Log level filter (e.g., "debug", "info", "warn", "error", "trace")
///
/// # Example (from Flutter/Dart)
///
/// ```dart
/// await CdkFfi.initLogging("debug");
/// // Now all logs will be visible in stdout
/// final wallet = await WalletRepository.create(...);
/// ```
#[uniffi::export]
pub fn init_logging(level: String) {
    INIT.call_once(|| {
        #[cfg(target_os = "android")]
        {
            use android_logger::{Config, FilterBuilder};
            use log::LevelFilter;

            android_logger::init_once(
                Config::default()
                    .with_max_level(LevelFilter::Trace)
                    .with_tag("cdk")
                    .format(|f: &mut dyn std::fmt::Write, record: &log::Record| {
                        write!(f, "{}", record.args())
                    })
                    .with_filter(FilterBuilder::new().parse(&level).build()),
            );
        }

        #[cfg(not(target_os = "android"))]
        {
            use tracing_subscriber::{fmt, EnvFilter};

            let filter = EnvFilter::try_new(&level).unwrap_or_else(|_| EnvFilter::new("info"));

            fmt()
                .with_env_filter(filter)
                .with_target(true)
                .with_ansi(false)
                .init();
        }
    });
}

/// Initialize logging with default "info" level
#[uniffi::export]
pub fn init_default_logging() {
    init_logging("info".to_string());
}
