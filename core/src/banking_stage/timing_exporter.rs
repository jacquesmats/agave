//! Simple synchronous transaction timing exporter for MEV analysis
//!
//! This module provides a lightweight, blocking HTTP client for exporting
//! transaction timing data without requiring async/Tokio runtime context.

use {
    log::{debug, error, info, warn},
    serde::{Deserialize, Serialize},
    std::{
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc,
        },
        time::Duration,
    },
};

/// Transaction timing data structure for MEV analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionTiming {
    /// Transaction signature (base58 encoded)
    pub signature: String,
    /// Slot number when transaction was processed
    pub slot: u64,
    /// POH tick height at commitment time (microsecond precision ordering)
    pub poh_tick: u64,
    /// List of account pubkeys that were read
    pub accounts_read: Vec<String>,
    /// List of account pubkeys that were written
    pub accounts_written: Vec<String>,
    /// Whether the transaction executed successfully
    pub execution_success: bool,
}

/// Configuration for the timing exporter
#[derive(Debug, Clone)]
pub struct TimingExporterConfig {
    /// URL of the analysis service endpoint
    pub export_url: String,
    /// Timeout for HTTP requests to analysis service
    pub request_timeout: Duration,
    /// Whether to enable batching of timing data
    pub enable_batching: bool,
    /// Maximum batch size when batching is enabled
    pub max_batch_size: usize,
}

impl Default for TimingExporterConfig {
    fn default() -> Self {
        Self {
            export_url: "http://localhost:8080/ingest".to_string(),
            request_timeout: Duration::from_millis(1000), // Shorter timeout for blocking calls
            enable_batching: false, // Keep it simple for now
            max_batch_size: 50,
        }
    }
}

/// Metrics for monitoring timing export performance
#[derive(Debug)]
pub struct TimingExporterMetrics {
    /// Total number of timing records exported
    pub total_exported: AtomicU64,
    /// Number of failed export attempts
    pub export_failures: AtomicU64,
    /// Number of dropped records due to errors
    pub dropped_records: AtomicU64,
}

impl Default for TimingExporterMetrics {
    fn default() -> Self {
        Self {
            total_exported: AtomicU64::new(0),
            export_failures: AtomicU64::new(0),
            dropped_records: AtomicU64::new(0),
        }
    }
}

/// Simple synchronous transaction timing data exporter
/// 
/// Uses blocking HTTP calls to avoid Tokio runtime requirements.
/// Designed to be fast and non-blocking for the validator's critical path.
#[derive(Debug)]
pub struct TimingExporter {
    client: reqwest::blocking::Client,
    config: TimingExporterConfig,
    metrics: Arc<TimingExporterMetrics>,
}

impl TimingExporter {
    /// Create a new timing exporter with the given configuration
    pub fn new(config: TimingExporterConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let client = reqwest::blocking::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| {
                error!("Failed to create HTTP client for timing export: {}", e);
                e
            })?;

        let metrics = Arc::new(TimingExporterMetrics::default());

        info!("TimingExporter initialized with endpoint: {}", config.export_url);

        Ok(Self {
            client,
            config,
            metrics,
        })
    }

    /// Export timing data synchronously (blocking but fast)
    /// 
    /// This method is called from the banking stage during transaction processing.
    /// It uses a blocking HTTP call but with a short timeout to minimize impact.
    pub fn export_timing_data(&self, timing_data: TransactionTiming) {
        // Quick debug log for development
        debug!(
            "Exporting timing data: slot={}, tick={}, success={}, sig={}",
            timing_data.slot,
            timing_data.poh_tick,
            timing_data.execution_success,
            &timing_data.signature[..8]
        );

        // Perform blocking HTTP POST
        match self.export_single_blocking(timing_data) {
            Ok(_) => {
                self.metrics.total_exported.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                self.metrics.export_failures.fetch_add(1, Ordering::Relaxed);
                self.metrics.dropped_records.fetch_add(1, Ordering::Relaxed);
                
                // Log error but don't panic - validator must continue
                let failure_count = self.metrics.export_failures.load(Ordering::Relaxed);
                if failure_count < 5 {
                    warn!("TimingExporter: Export failed (attempt {}): {}", failure_count, e);
                } else if failure_count == 5 {
                    error!("TimingExporter: Too many failures, suppressing further error logs");
                }
            }
        }
    }

    /// Perform a single blocking HTTP export
    fn export_single_blocking(&self, timing_data: TransactionTiming) -> Result<(), reqwest::Error> {
        let response = self
            .client
            .post(&self.config.export_url)
            .json(&timing_data)
            .send()?;

        response.error_for_status()?;
        Ok(())
    }

    /// Get current exporter metrics
    pub fn metrics(&self) -> &TimingExporterMetrics {
        &self.metrics
    }

    /// Get the export endpoint URL
    pub fn export_url(&self) -> &str {
        &self.config.export_url
    }

    /// Check if the exporter is healthy (low error rate)
    pub fn is_healthy(&self) -> bool {
        let failures = self.metrics.export_failures.load(Ordering::Relaxed);
        let total = self.metrics.total_exported.load(Ordering::Relaxed);
        
        if total == 0 {
            true // No data yet, assume healthy
        } else {
            let error_rate = failures as f64 / (total + failures) as f64;
            error_rate < 0.1 // Healthy if less than 10% error rate
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timing_exporter_config_default() {
        let config = TimingExporterConfig::default();
        assert_eq!(config.export_url, "http://localhost:8080/ingest");
        assert_eq!(config.request_timeout, Duration::from_millis(1000));
        assert!(!config.enable_batching);
        assert_eq!(config.max_batch_size, 50);
    }

    #[test]
    fn test_timing_exporter_creation() {
        let config = TimingExporterConfig::default();
        let exporter = TimingExporter::new(config).expect("Should create TimingExporter successfully");
        
        assert!(exporter.is_healthy());
        assert_eq!(exporter.export_url(), "http://localhost:8080/ingest");
        
        // Test metrics initialization
        let metrics = exporter.metrics();
        assert_eq!(metrics.total_exported.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.export_failures.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.dropped_records.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_transaction_timing_serialization() {
        let timing_data = TransactionTiming {
            signature: "test_signature".to_string(),
            slot: 12345,
            poh_tick: 67890,
            accounts_read: vec!["account1".to_string(), "account2".to_string()],
            accounts_written: vec!["account3".to_string()],
            execution_success: true,
        };

        let json = serde_json::to_string(&timing_data).expect("Should serialize");
        let deserialized: TransactionTiming = serde_json::from_str(&json).expect("Should deserialize");
        
        assert_eq!(deserialized.signature, timing_data.signature);
        assert_eq!(deserialized.slot, timing_data.slot);
        assert_eq!(deserialized.poh_tick, timing_data.poh_tick);
        assert_eq!(deserialized.execution_success, timing_data.execution_success);
    }
}
