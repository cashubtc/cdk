//! A logger implementation that writes log messages to stdout. This struct implements the
//! `LogWriter` trait, which defines a way to handle log records. The log records are formatted,
//! assigned a severity level, and emitted as structured tracing events.
pub struct StdoutLogWriter;
impl crate::LogWriter for StdoutLogWriter {
    /// Logs a given `LogRecord` instance using structured tracing events.
    fn log(&self, record: ldk_node::logger::LogRecord) {
        let level = match record.level.to_string().to_ascii_lowercase().as_str() {
            "error" => tracing::Level::ERROR,
            "warn" | "warning" => tracing::Level::WARN,
            "debug" => tracing::Level::DEBUG,
            "trace" => tracing::Level::TRACE,
            _ => tracing::Level::INFO,
        };

        // Format message once
        let msg = record.args.to_string();
        // Emit as a structured tracing event.
        // Use level-specific macros (require compile-time level) and record the original module path as a field.
        match level {
            tracing::Level::ERROR => {
                tracing::error!(
                    module_path = record.module_path,
                    line = record.line,
                    "{msg}"
                );
            }
            tracing::Level::WARN => {
                tracing::warn!(
                    module_path = record.module_path,
                    line = record.line,
                    "{msg}"
                );
            }
            tracing::Level::INFO => {
                tracing::info!(
                    module_path = record.module_path,
                    line = record.line,
                    "{msg}"
                );
            }
            tracing::Level::DEBUG => {
                tracing::debug!(
                    module_path = record.module_path,
                    line = record.line,
                    "{msg}"
                );
            }
            tracing::Level::TRACE => {
                tracing::trace!(
                    module_path = record.module_path,
                    line = record.line,
                    "{msg}"
                );
            }
        }
    }
}

impl Default for StdoutLogWriter {
    /// Provides the default implementation for the struct.
    fn default() -> Self {
        Self {}
    }
}
