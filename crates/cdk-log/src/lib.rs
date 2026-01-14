//! Conditional logging macros for CDK.
//!
//! This crate provides logging macros that work in both native Rust and FFI contexts.
//!
//! ## Feature Flags
//!
//! - `ffi`: When enabled, uses `println!`/`eprintln!` for output. This is useful for FFI
//!   bindings where tracing subscribers may not be available or log output doesn't appear
//!   in the host language's console.
//!
//! ## Usage
//!
//! ```rust
//! use cdk_log::{log_info, log_error, log_warn, log_debug, log_trace};
//!
//! log_info!("Processing payment for amount: {}", 100);
//! log_error!("Failed to process: {:?}", "some error");
//! log_debug!("Debug information: {}", "details");
//! ```
//!
//! ## FFI Builds
//!
//! When building for FFI, enable the `ffi` feature in your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! cdk-log = { workspace = true, features = ["ffi"] }
//! ```

// Re-export tracing for use in macros when not in FFI mode
#[cfg(not(feature = "ffi"))]
#[doc(hidden)]
pub use tracing;

/// Internal function to log info messages in FFI mode
#[cfg(feature = "ffi")]
#[doc(hidden)]
#[inline]
pub fn _log_info_impl(msg: std::fmt::Arguments<'_>) {
    println!("[INFO] {}", msg);
}

/// Internal function to log error messages in FFI mode
#[cfg(feature = "ffi")]
#[doc(hidden)]
#[inline]
pub fn _log_error_impl(msg: std::fmt::Arguments<'_>) {
    eprintln!("[ERROR] {}", msg);
}

/// Internal function to log warning messages in FFI mode
#[cfg(feature = "ffi")]
#[doc(hidden)]
#[inline]
pub fn _log_warn_impl(msg: std::fmt::Arguments<'_>) {
    eprintln!("[WARN] {}", msg);
}

/// Internal function to log debug messages in FFI mode
#[cfg(feature = "ffi")]
#[doc(hidden)]
#[inline]
pub fn _log_debug_impl(msg: std::fmt::Arguments<'_>) {
    #[cfg(debug_assertions)]
    println!("[DEBUG] {}", msg);
    #[cfg(not(debug_assertions))]
    let _ = msg;
}

/// Internal function to log trace messages in FFI mode (no-op)
#[cfg(feature = "ffi")]
#[doc(hidden)]
#[inline]
pub fn _log_trace_impl(_msg: std::fmt::Arguments<'_>) {
    // Trace is too verbose for FFI, skip entirely
}

/// Log an info-level message.
///
/// In FFI mode, outputs to stdout with `[INFO]` prefix.
/// In native mode, delegates to `tracing::info!`.
#[macro_export]
#[cfg(feature = "ffi")]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::_log_info_impl(format_args!($($arg)*))
    };
}

/// Log an info-level message.
///
/// In FFI mode, outputs to stdout with `[INFO]` prefix.
/// In native mode, delegates to `tracing::info!`.
#[macro_export]
#[cfg(not(feature = "ffi"))]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::tracing::info!($($arg)*)
    };
}

/// Log an error-level message.
///
/// In FFI mode, outputs to stderr with `[ERROR]` prefix.
/// In native mode, delegates to `tracing::error!`.
#[macro_export]
#[cfg(feature = "ffi")]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::_log_error_impl(format_args!($($arg)*))
    };
}

/// Log an error-level message.
///
/// In FFI mode, outputs to stderr with `[ERROR]` prefix.
/// In native mode, delegates to `tracing::error!`.
#[macro_export]
#[cfg(not(feature = "ffi"))]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::tracing::error!($($arg)*)
    };
}

/// Log a warning-level message.
///
/// In FFI mode, outputs to stderr with `[WARN]` prefix.
/// In native mode, delegates to `tracing::warn!`.
#[macro_export]
#[cfg(feature = "ffi")]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::_log_warn_impl(format_args!($($arg)*))
    };
}

/// Log a warning-level message.
///
/// In FFI mode, outputs to stderr with `[WARN]` prefix.
/// In native mode, delegates to `tracing::warn!`.
#[macro_export]
#[cfg(not(feature = "ffi"))]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::tracing::warn!($($arg)*)
    };
}

/// Log a debug-level message.
///
/// In FFI mode, outputs to stdout with `[DEBUG]` prefix (only in debug builds).
/// In native mode, delegates to `tracing::debug!`.
#[macro_export]
#[cfg(feature = "ffi")]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        $crate::_log_debug_impl(format_args!($($arg)*))
    };
}

/// Log a debug-level message.
///
/// In FFI mode, outputs to stdout with `[DEBUG]` prefix (only in debug builds).
/// In native mode, delegates to `tracing::debug!`.
#[macro_export]
#[cfg(not(feature = "ffi"))]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        $crate::tracing::debug!($($arg)*)
    };
}

/// Log a trace-level message.
///
/// In FFI mode, this is a no-op (trace is typically too verbose).
/// In native mode, delegates to `tracing::trace!`.
#[macro_export]
#[cfg(feature = "ffi")]
macro_rules! log_trace {
    ($($arg:tt)*) => {
        $crate::_log_trace_impl(format_args!($($arg)*))
    };
}

/// Log a trace-level message.
///
/// In FFI mode, this is a no-op (trace is typically too verbose).
/// In native mode, delegates to `tracing::trace!`.
#[macro_export]
#[cfg(not(feature = "ffi"))]
macro_rules! log_trace {
    ($($arg:tt)*) => {
        $crate::tracing::trace!($($arg)*)
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_log_info() {
        log_info!("Test info message: {}", 42);
    }

    #[test]
    fn test_log_error() {
        log_error!("Test error message: {}", "error");
    }

    #[test]
    fn test_log_warn() {
        log_warn!("Test warning message");
    }

    #[test]
    fn test_log_debug() {
        log_debug!("Test debug message: {:?}", vec![1, 2, 3]);
    }

    #[test]
    fn test_log_trace() {
        log_trace!("Test trace message");
    }
}
