use std::sync::Arc;

#[cfg(feature = "system-metrics")]
use prometheus::{Gauge, IntGauge, Registry};
#[cfg(feature = "system-metrics")]
use sysinfo::{Pid, System};

/// System metrics collector that provides CPU, memory, disk, network, and process metrics
#[cfg(feature = "system-metrics")]
#[derive(Clone, Debug)]
pub struct SystemMetrics {
    registry: Arc<Registry>,
    system: Arc<std::sync::Mutex<System>>,

    // Process metrics (for the CDK process)
    process_cpu_usage_percent: Gauge,
    process_memory_bytes: IntGauge,
    process_memory_percent: Gauge,
}

#[cfg(feature = "system-metrics")]
impl SystemMetrics {
    /// Create a new `SystemMetrics` instance
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    pub fn new() -> crate::Result<Self> {
        let registry = Arc::new(Registry::new());
        // Process metrics
        let process_cpu_usage_percent = Gauge::new(
            "process_cpu_usage_percent",
            "CPU usage percentage of the CDK process (0-100)",
        )?;
        registry.register(Box::new(process_cpu_usage_percent.clone()))?;

        let process_memory_bytes = IntGauge::new(
            "process_memory_bytes",
            "Memory usage of the CDK process in bytes",
        )?;
        registry.register(Box::new(process_memory_bytes.clone()))?;

        let process_memory_percent = Gauge::new(
            "process_memory_percent",
            "Memory usage percentage of the CDK process (0-100)",
        )?;
        registry.register(Box::new(process_memory_percent.clone()))?;

        // Initialize system with all needed refresh kinds
        let system = Arc::new(std::sync::Mutex::new(System::new()));

        let result = Self {
            registry,
            system,
            process_cpu_usage_percent,
            process_memory_bytes,
            process_memory_percent,
        };

        Ok(result)
    }

    /// Get the metrics registry
    #[must_use]
    pub fn registry(&self) -> Arc<Registry> {
        Arc::<Registry>::clone(&self.registry)
    }

    /// Update all system metrics
    ///
    /// # Errors
    /// Returns an error if the system mutex cannot be locked
    pub fn update_metrics(&self) -> crate::Result<()> {
        let mut system = self.system.lock().map_err(|e| {
            crate::error::PrometheusError::SystemMetrics(format!("Failed to lock system: {e}"))
        })?;
        // Refresh system information
        system.refresh_all();

        // Update memory metrics
        let total_memory = i64::try_from(system.total_memory()).unwrap_or(i64::MAX);

        // Update process metrics for the current process
        // This is a simplified approach that may not work perfectly in all cases
        if let Some(process) = system.process(Pid::from(std::process::id() as usize)) {
            // Get CPU usage if available
            let process_cpu = process.cpu_usage();
            self.process_cpu_usage_percent.set(f64::from(process_cpu));

            // Get memory usage if available
            let process_memory = i64::try_from(process.memory()).unwrap_or(i64::MAX);
            self.process_memory_bytes.set(process_memory);

            // Calculate memory percentage
            if total_memory > 0 {
                // Precision loss is acceptable for percentage calculation
                #[allow(clippy::cast_precision_loss)]
                let process_memory_percent = (process_memory as f64 / total_memory as f64) * 100.0;
                self.process_memory_percent.set(process_memory_percent);
            }
        }

        // Drop the system lock early to avoid resource contention
        drop(system);

        Ok(())
    }
}
