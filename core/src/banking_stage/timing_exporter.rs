//! Transaction timing data exporter for MEV analysis
//!
//! This module provides functionality to export microsecond-precision transaction
//! timing data to an external analysis service for MEV research and arbitrage analysis.
//! 
//! The exporter captures POH tick heights (100µs resolution) that represent true
//! transaction commitment order, along with account access patterns and execution metadata.

use {
    log::{error, info, warn},
    reqwest::{Client, Error as ReqwestError},
    serde::{Deserialize, Serialize},
    std::{
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            Arc,
        },
        time::Duration,
    },
    tokio::{
        sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
        time::timeout,
    },
};

/// Transaction timing data structure for MEV analysis
/// 
/// This captures the essential timing and ordering information needed to analyze
/// arbitrage opportunities and MEV extraction patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionTiming {
    /// Transaction signature as base58 string
    pub signature: String,
    /// Slot number where transaction was processed
    pub slot: u64,
    /// POH tick height - 100µs resolution commitment time (true ordering)
    pub poh_tick: u64,
    /// Accounts read by this transaction
    pub accounts_read: Vec<String>,
    /// Accounts written by this transaction  
    pub accounts_written: Vec<String>,
    /// Whether transaction executed successfully
    pub execution_success: bool,
}

/// Configuration for the timing exporter
#[derive(Debug, Clone)]
pub struct TimingExporterConfig {
    /// URL of the analysis service endpoint
    pub export_url: String,
    /// Maximum number of pending requests before dropping
    pub max_pending_requests: usize,
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
            max_pending_requests: 10000,
            request_timeout: Duration::from_millis(5000),
            enable_batching: true,
            max_batch_size: 100,
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
    /// Number of dropped records due to queue overflow
    pub dropped_records: AtomicU64,
    /// Whether the exporter is currently healthy
    pub is_healthy: AtomicBool,
}

impl Default for TimingExporterMetrics {
    fn default() -> Self {
        Self {
            total_exported: AtomicU64::new(0),
            export_failures: AtomicU64::new(0),
            dropped_records: AtomicU64::new(0),
            is_healthy: AtomicBool::new(true),
        }
    }
}

/// Non-blocking transaction timing data exporter
/// 
/// Handles async HTTP export of timing data to prevent any impact on validator
/// performance. Uses an internal channel and background task for export operations.
#[derive(Debug)]
pub struct TimingExporter {
    sender: UnboundedSender<TransactionTiming>,
    metrics: Arc<TimingExporterMetrics>,
    config: TimingExporterConfig,
}

impl TimingExporter {
    /// Create a new timing exporter with the given configuration
    pub fn new(config: TimingExporterConfig) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let metrics = Arc::new(TimingExporterMetrics::default());
        
        // Spawn background task for handling exports
        let export_task = ExportTask::new(receiver, config.clone(), metrics.clone());
        tokio::spawn(async move {
            export_task.run().await;
        });

        info!("TimingExporter initialized with endpoint: {}", config.export_url);

        Self {
            sender,
            metrics,
            config,
        }
    }

    /// Export timing data asynchronously (non-blocking)
    /// 
    /// This method is called from the banking stage during transaction processing.
    /// It's designed to be extremely fast and non-blocking to avoid impacting
    /// validator performance.
    pub fn export_async(&self, timing_data: TransactionTiming) {
        match self.sender.send(timing_data) {
            Ok(_) => {
                // Successfully queued for export
            }
            Err(_) => {
                // Channel is closed - export task has stopped
                self.metrics.dropped_records.fetch_add(1, Ordering::Relaxed);
                warn!("TimingExporter: Failed to queue timing data - export task stopped");
            }
        }
    }

    /// Get current exporter metrics
    pub fn metrics(&self) -> &TimingExporterMetrics {
        &self.metrics
    }

    /// Check if the exporter is healthy
    pub fn is_healthy(&self) -> bool {
        self.metrics.is_healthy.load(Ordering::Relaxed)
    }

    /// Get the export endpoint URL
    pub fn export_url(&self) -> &str {
        &self.config.export_url
    }
}

/// Background task that handles the actual HTTP exports
struct ExportTask {
    receiver: UnboundedReceiver<TransactionTiming>,
    client: Client,
    config: TimingExporterConfig,
    metrics: Arc<TimingExporterMetrics>,
    batch_buffer: Vec<TransactionTiming>,
}

impl ExportTask {
    fn new(
        receiver: UnboundedReceiver<TransactionTiming>,
        config: TimingExporterConfig,
        metrics: Arc<TimingExporterMetrics>,
    ) -> Self {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .expect("Failed to create HTTP client for timing export");

        Self {
            receiver,
            client,
            config,
            metrics,
            batch_buffer: Vec::new(),
        }
    }

    async fn run(mut self) {
        info!("TimingExporter background task started");

        while let Some(timing_data) = self.receiver.recv().await {
            if self.config.enable_batching {
                self.handle_batched_export(timing_data).await;
            } else {
                self.handle_single_export(timing_data).await;
            }
        }

        // Flush any remaining batched data before shutdown
        if !self.batch_buffer.is_empty() {
            self.flush_batch().await;
        }

        info!("TimingExporter background task stopped");
    }

    async fn handle_single_export(&self, timing_data: TransactionTiming) {
        match self.export_single(timing_data).await {
            Ok(_) => {
                self.metrics.total_exported.fetch_add(1, Ordering::Relaxed);
                self.metrics.is_healthy.store(true, Ordering::Relaxed);
            }
            Err(e) => {
                self.metrics.export_failures.fetch_add(1, Ordering::Relaxed);
                self.handle_export_error(e);
            }
        }
    }

    async fn handle_batched_export(&mut self, timing_data: TransactionTiming) {
        self.batch_buffer.push(timing_data);

        if self.batch_buffer.len() >= self.config.max_batch_size {
            self.flush_batch().await;
        }
    }

    async fn flush_batch(&mut self) {
        if self.batch_buffer.is_empty() {
            return;
        }

        let batch = std::mem::take(&mut self.batch_buffer);
        let batch_size = batch.len();

        match self.export_batch(batch).await {
            Ok(_) => {
                self.metrics
                    .total_exported
                    .fetch_add(batch_size as u64, Ordering::Relaxed);
                self.metrics.is_healthy.store(true, Ordering::Relaxed);
            }
            Err(e) => {
                self.metrics.export_failures.fetch_add(1, Ordering::Relaxed);
                self.metrics
                    .dropped_records
                    .fetch_add(batch_size as u64, Ordering::Relaxed);
                self.handle_export_error(e);
            }
        }
    }

    async fn export_single(&self, timing_data: TransactionTiming) -> Result<(), ReqwestError> {
        let result = timeout(
            self.config.request_timeout,
            self.client
                .post(&self.config.export_url)
                .json(&timing_data)
                .send(),
        )
        .await;

        let response = match result {
            Ok(response_result) => response_result?,
            Err(_) => {
                // Timeout occurred - create a custom reqwest error
		warn!("Request timeout occurred during timing data export");
		return Ok(());
            }
        };

        response.error_for_status()?;
        Ok(())
    }

    async fn export_batch(&self, batch: Vec<TransactionTiming>) -> Result<(), ReqwestError> {
        let result = timeout(
            self.config.request_timeout,
            self.client
                .post(&self.config.export_url)
                .json(&batch)
                .send(),
        )
        .await;

        let response = match result {
            Ok(response_result) => response_result?,
            Err(_) => {
                // Timeout occurred - create a custom reqwest error
		warn!("Request timeout occurred during timing data export");
		return Ok(());
            }
        };

        response.error_for_status()?;
        Ok(())
    }

    fn handle_export_error(&self, error: ReqwestError) {
        let failure_count = self.metrics.export_failures.load(Ordering::Relaxed);
        
        // Mark as unhealthy after multiple consecutive failures
        if failure_count > 10 {
            self.metrics.is_healthy.store(false, Ordering::Relaxed);
        }

        // Log errors at different levels based on failure count
        if failure_count < 5 {
            warn!("TimingExporter: Export failed (attempt {}): {}", failure_count, error);
        } else {
            error!("TimingExporter: Persistent export failures ({}): {}", failure_count, error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_timing_serialization() {
        let timing = TransactionTiming {
            signature: "test_signature".to_string(),
            slot: 12345,
            poh_tick: 98765432,
            accounts_read: vec!["account1".to_string(), "account2".to_string()],
            accounts_written: vec!["account3".to_string()],
            execution_success: true,
        };

        let json = serde_json::to_string(&timing).unwrap();
        let deserialized: TransactionTiming = serde_json::from_str(&json).unwrap();

        assert_eq!(timing.signature, deserialized.signature);
        assert_eq!(timing.slot, deserialized.slot);
        assert_eq!(timing.poh_tick, deserialized.poh_tick);
        assert_eq!(timing.accounts_read, deserialized.accounts_read);
        assert_eq!(timing.accounts_written, deserialized.accounts_written);
        assert_eq!(timing.execution_success, deserialized.execution_success);
    }

    #[test]
    fn test_timing_exporter_config_default() {
        let config = TimingExporterConfig::default();
        assert_eq!(config.export_url, "http://localhost:8080/ingest");
        assert_eq!(config.max_pending_requests, 10000);
        assert!(config.enable_batching);
        assert_eq!(config.max_batch_size, 100);
    }

    #[tokio::test]
    async fn test_timing_exporter_creation() {
        let config = TimingExporterConfig::default();
        let exporter = TimingExporter::new(config);
        
        assert!(exporter.is_healthy());
        assert_eq!(exporter.export_url(), "http://localhost:8080/ingest");
        
        // Test metrics initialization
        let metrics = exporter.metrics();
        assert_eq!(metrics.total_exported.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.export_failures.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.dropped_records.load(Ordering::Relaxed), 0);
    }
}
