#[cfg(feature = "system-metrics")]
use prometheus::{Gauge, IntGauge, Registry};
#[cfg(feature = "system-metrics")]
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System, Disks};
use std::sync::Arc;

/// System metrics collector that provides CPU, memory, and disk usage metrics
#[cfg(feature = "system-metrics")]
#[derive(Clone, Debug)]
pub struct SystemMetrics {
    registry: Arc<Registry>,
    system: Arc<std::sync::Mutex<System>>,
    
    // CPU metrics
    cpu_usage_percent: Gauge,
    cpu_count: IntGauge,
    
    // Memory metrics
    memory_total_bytes: IntGauge,
    memory_used_bytes: IntGauge,
    memory_available_bytes: IntGauge,
    memory_usage_percent: Gauge,
    
    // Disk metrics
    disk_total_bytes: IntGauge,
    disk_used_bytes: IntGauge,
    disk_available_bytes: IntGauge,
    disk_usage_percent: Gauge,
}

#[cfg(feature = "system-metrics")]
impl SystemMetrics {
    /// Create a new SystemMetrics instance
    pub fn new() -> crate::Result<Self> {
        let registry = Arc::new(Registry::new());
        
        let cpu_usage_percent = Gauge::new(
            "system_cpu_usage_percent",
            "CPU usage percentage (0-100)"
        )?;
        registry.register(Box::new(cpu_usage_percent.clone()))?;
        
        let cpu_count = IntGauge::new(
            "system_cpu_count",
            "Number of CPU cores"
        )?;
        registry.register(Box::new(cpu_count.clone()))?;
        
        let memory_total_bytes = IntGauge::new(
            "system_memory_total_bytes",
            "Total system memory in bytes"
        )?;
        registry.register(Box::new(memory_total_bytes.clone()))?;
        
        let memory_used_bytes = IntGauge::new(
            "system_memory_used_bytes", 
            "Used system memory in bytes"
        )?;
        registry.register(Box::new(memory_used_bytes.clone()))?;
        
        let memory_available_bytes = IntGauge::new(
            "system_memory_available_bytes",
            "Available system memory in bytes"
        )?;
        registry.register(Box::new(memory_available_bytes.clone()))?;
        
        let memory_usage_percent = Gauge::new(
            "system_memory_usage_percent",
            "Memory usage percentage (0-100)"
        )?;
        registry.register(Box::new(memory_usage_percent.clone()))?;
        
        let disk_total_bytes = IntGauge::new(
            "system_disk_total_bytes",
            "Total disk space in bytes"
        )?;
        registry.register(Box::new(disk_total_bytes.clone()))?;
        
        let disk_used_bytes = IntGauge::new(
            "system_disk_used_bytes",
            "Used disk space in bytes"
        )?;
        registry.register(Box::new(disk_used_bytes.clone()))?;
        
        let disk_available_bytes = IntGauge::new(
            "system_disk_available_bytes",
            "Available disk space in bytes"
        )?;
        registry.register(Box::new(disk_available_bytes.clone()))?;
        
        let disk_usage_percent = Gauge::new(
            "system_disk_usage_percent",
            "Disk usage percentage (0-100)"
        )?;
        registry.register(Box::new(disk_usage_percent.clone()))?;
        
        let system = Arc::new(std::sync::Mutex::new(
            System::new_with_specifics(
                RefreshKind::new()
                    .with_cpu(CpuRefreshKind::everything())
                    .with_memory(MemoryRefreshKind::everything())
            )
        ));
        
        let result = Self {
            registry,
            system,
            cpu_usage_percent,
            cpu_count,
            memory_total_bytes,
            memory_used_bytes,
            memory_available_bytes,
            memory_usage_percent,
            disk_total_bytes,
            disk_used_bytes,
            disk_available_bytes,
            disk_usage_percent,
        };
        
        // Set static values
        result.update_static_metrics()?;
        
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
        system.refresh_cpu_all();
        system.refresh_memory();
        
        // Update CPU metrics
        let cpu_usage = system.global_cpu_usage();
        self.cpu_usage_percent.set(cpu_usage as f64);
        
        // Update memory metrics
        let total_memory = system.total_memory() as i64;
        let used_memory = system.used_memory() as i64;
        let available_memory = system.available_memory() as i64;
        
        self.memory_total_bytes.set(total_memory);
        self.memory_used_bytes.set(used_memory);
        self.memory_available_bytes.set(available_memory);
        
        if total_memory > 0 {
            let memory_usage_percent = (used_memory as f64 / total_memory as f64) * 100.0;
            self.memory_usage_percent.set(memory_usage_percent);
        }
        
        // Update disk metrics (for root partition)
        // Note: Using a simple check for the largest disk as a proxy for root
        let disks = Disks::new_with_refreshed_list();
        if let Some(disk) = disks.iter().max_by_key(|d| d.total_space()) {
            let total_space = disk.total_space() as i64;
            let available_space = disk.available_space() as i64;
            let used_space = total_space - available_space;
            
            self.disk_total_bytes.set(total_space);
            self.disk_used_bytes.set(used_space);
            self.disk_available_bytes.set(available_space);
            
            if total_space > 0 {
                let disk_usage_percent = (used_space as f64 / total_space as f64) * 100.0;
                self.disk_usage_percent.set(disk_usage_percent);
            }
        }
        
        Ok(())
    }
    
    /// Update static metrics that don't change
    fn update_static_metrics(&self) -> crate::Result<()> {
        let system = self.system.lock()
            .map_err(|e| crate::error::PrometheusError::SystemMetrics(
                format!("Failed to lock system: {}", e)
            ))?;
        
        // Set CPU count
        let cpu_count = system.cpus().len() as i64;
        self.cpu_count.set(cpu_count);
        
        Ok(())
    }
}