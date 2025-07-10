#[cfg(feature = "system-metrics")]
use prometheus::{Gauge, IntGauge,  Registry};
#[cfg(feature = "system-metrics")]
use sysinfo::{ System, Pid};
use std::sync::Arc;


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
    /// Create a new SystemMetrics instance
    pub fn new() -> crate::Result<Self> {
        let registry = Arc::new(Registry::new());
        // Process metrics
        let process_cpu_usage_percent = Gauge::new(
            "process_cpu_usage_percent",
            "CPU usage percentage of the CDK process (0-100)"
        )?;
        registry.register(Box::new(process_cpu_usage_percent.clone()))?;

        let process_memory_bytes = IntGauge::new(
            "process_memory_bytes",
            "Memory usage of the CDK process in bytes"
        )?;
        registry.register(Box::new(process_memory_bytes.clone()))?;

        let process_memory_percent = Gauge::new(
            "process_memory_percent",
            "Memory usage percentage of the CDK process (0-100)"
        )?;
        registry.register(Box::new(process_memory_percent.clone()))?;

        // Initialize system with all needed refresh kinds
        let system = Arc::new(std::sync::Mutex::new(
            System::new()
        ));

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
    pub fn registry(&self) -> Arc<Registry> {
        self.registry.clone()
    }

    /// Update all system metrics
    pub fn update_metrics(&self) -> crate::Result<()> {
        let mut system = self.system.lock()
            .map_err(|e| crate::error::PrometheusError::SystemMetrics(
                format!("Failed to lock system: {}", e)
            ))?;

        // Refresh system information
        system.refresh_all();

        // Update memory metrics
        let total_memory = system.total_memory() as i64;

        // Update process metrics for the current process
        // This is a simplified approach that may not work perfectly in all cases
        if let Some(process) = system.process(Pid::from(std::process::id() as usize)) {
            // Get CPU usage if available
            let process_cpu = process.cpu_usage();
            self.process_cpu_usage_percent.set(process_cpu as f64);

            // Get memory usage if available
            let process_memory = process.memory() as i64;
            self.process_memory_bytes.set(process_memory);

            // Calculate memory percentage
            if total_memory > 0 {
                let process_memory_percent = (process_memory as f64 / total_memory as f64) * 100.0;
                self.process_memory_percent.set(process_memory_percent);
            }
        }

        Ok(())
    }

}
