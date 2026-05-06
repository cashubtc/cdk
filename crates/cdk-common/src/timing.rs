//! Simple timing utilities for measuring method durations

/// A drop-based timer that logs method entry and exit with elapsed time.
///
/// Create at the start of a method to automatically log duration on exit,
/// including early returns. Logs at error level if the method took longer
/// than 100ms.
#[derive(Debug)]
pub struct MethodTimer {
    name: &'static str,
    start: std::time::Instant,
}

impl MethodTimer {
    /// Creates a new timer and logs method entry
    pub fn new(name: &'static str) -> Self {
        tracing::info!("[TIMER {}] START", name);
        Self {
            name,
            start: std::time::Instant::now(),
        }
    }
}

impl Drop for MethodTimer {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        if elapsed.as_millis() > 100 {
            tracing::error!("[TIMER {}] END took {:?}", self.name, elapsed);
        } else {
            tracing::info!("[TIMER {}] END took {:?}", self.name, elapsed);
        }
    }
}
