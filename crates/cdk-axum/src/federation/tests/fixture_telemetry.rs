//! Prometheus metric readers and log capture for federation tests.

use super::*;

#[cfg(feature = "prometheus")]
pub(super) fn prometheus_metric_sum(metric: &str, labels: &[(&str, &str)]) -> f64 {
    let registry = cdk_prometheus::METRICS.registry();
    let scrape = cdk_prometheus::prometheus::TextEncoder::new()
        .encode_to_string(&registry.gather())
        .expect("encode prometheus metrics");
    scrape
        .lines()
        .filter_map(|line| {
            if !line.starts_with(metric) {
                return None;
            }
            for (key, value) in labels {
                let fragment = format!("{key}=\"{value}\"");
                if !line.contains(&fragment) {
                    return None;
                }
            }
            line.rsplit_once(' ')
                .and_then(|(_, value)| value.parse::<f64>().ok())
        })
        .sum()
}

#[cfg(feature = "prometheus")]
#[derive(Debug, Clone, Default)]
pub(super) struct CapturedLogWriter(pub(super) Arc<Mutex<Vec<u8>>>);

#[cfg(feature = "prometheus")]
impl io::Write for CapturedLogWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("captured log lock")
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(feature = "prometheus")]
impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CapturedLogWriter {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}
