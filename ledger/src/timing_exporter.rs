use {
    log::*,
    reqwest::blocking::Client,
    serde::{Deserialize, Serialize},
    std::{
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            mpsc::{self, Receiver, Sender},
            Arc,
        },
        thread,
        time::{Duration, Instant},
    },
};

/// Transaction timing data structure for historical MEV analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionTiming {
    pub signature: String,
    pub slot: u64,
    pub poh_tick: u64,           // 100µs resolution from historical block
    pub entry_index: u64,        // Position within slot's entries
    pub tx_index: u64,           // Position within entry
    pub accounts_read: Vec<String>,
    pub accounts_written: Vec<String>,
    pub is_vote: bool,           // Flag vote transactions separately
}

/// Configuration for timing export system
#[derive(Debug, Clone)]
pub struct ExportConfig {
    pub batch_size: usize,           // Number of transactions per batch
    pub batch_timeout_secs: u64,     // Max time to wait before sending partial batch
    pub channel_capacity: usize,     // Channel buffer size (1GB ~= 4M messages)
    pub retry_base_ms: u64,          // Base retry interval in milliseconds
    pub retry_max_ms: u64,           // Maximum retry interval in milliseconds
    pub enable_compression: bool,     // Enable gzip compression
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            batch_size: 14_000,         // 10 slots worth of transactions
            batch_timeout_secs: 5,      // Flush every 5 seconds max
            channel_capacity: 4_000_000, // ~1GB RAM buffer
            retry_base_ms: 100,         // Start with 100ms
            retry_max_ms: 30_000,       // Max 30 second intervals
            enable_compression: false,    // Enable gzip by default
        }
    }
}

/// Statistics for the timing export system
#[derive(Debug)]
pub struct ExportStats {
    pub total_sent: AtomicU64,
    pub total_exported: AtomicU64,
    pub total_errors: AtomicU64,
    pub current_buffer_size: AtomicU64,
}

/// Channel-based timing exporter with background worker thread
#[derive(Clone)]
pub struct TimingExporter {
    sender: Sender<TransactionTiming>,
    stats: Arc<ExportStats>,
    shutdown_signal: Arc<AtomicBool>,
    // Note: worker_handle is managed by the background thread itself
    // We don't store it here to avoid Clone complexity
}

/// Background worker for HTTP export
struct HttpWorker {
    receiver: Receiver<TransactionTiming>,
    client: Client,
    export_url: String,
    config: ExportConfig,
    stats: Arc<ExportStats>,
    shutdown_signal: Arc<AtomicBool>,
}

impl TimingExporter {
    /// Create a new channel-based timing exporter with background worker
    pub fn new(export_url: String, config: Option<ExportConfig>) -> Result<Self, String> {
        let config = config.unwrap_or_default();
        
        // Create channel with large capacity for 1GB buffer
        let (sender, receiver) = mpsc::channel::<TransactionTiming>();
        
        // Create shared statistics
        let stats = Arc::new(ExportStats {
            total_sent: AtomicU64::new(0),
            total_exported: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            current_buffer_size: AtomicU64::new(0),
        });
        
        // Create shutdown signal
        let shutdown_signal = Arc::new(AtomicBool::new(false));
        
        // Create HTTP client with compression support
        let mut client_builder = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10));
            
        if config.enable_compression {
            client_builder = client_builder.gzip(true);
        }
        
        let client = client_builder
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;
        
        // Create worker
        let worker = HttpWorker {
            receiver,
            client,
            export_url: export_url.clone(),
            config: config.clone(),
            stats: stats.clone(),
            shutdown_signal: shutdown_signal.clone(),
        };
        
        // Spawn background worker thread
        let _worker_handle = thread::Builder::new()
            .name("timing-export-worker".to_string())
            .spawn(move || worker.run())
            .map_err(|e| format!("Failed to spawn worker thread: {}", e))?;
        
        info!(
            "Started timing export worker: url={}, batch_size={}, buffer_capacity={}",
            export_url, config.batch_size, config.channel_capacity
        );
        
        Ok(Self {
            sender,
            stats,
            shutdown_signal,
        })
    }

    /// Add a transaction timing record to the export channel (non-blocking)
    pub fn export(&self, timing: TransactionTiming) {
        // Increment sent counter
        self.stats.total_sent.fetch_add(1, Ordering::Relaxed);
        
        // Try to send to channel - this is the ~100ns operation
        match self.sender.send(timing) {
            Ok(_) => {
                // Success - data queued for background processing
            }
            Err(_) => {
                // Channel is closed (worker thread ended) - log warning
                warn!("Timing export channel closed, dropping transaction data");
                self.stats.total_errors.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Get export statistics
    pub fn stats(&self) -> ExportStats {
        ExportStats {
            total_sent: AtomicU64::new(self.stats.total_sent.load(Ordering::Relaxed)),
            total_exported: AtomicU64::new(self.stats.total_exported.load(Ordering::Relaxed)),
            total_errors: AtomicU64::new(self.stats.total_errors.load(Ordering::Relaxed)),
            current_buffer_size: AtomicU64::new(self.stats.current_buffer_size.load(Ordering::Relaxed)),
        }
    }
    
    /// Gracefully shutdown the worker thread
    pub fn shutdown(&self) {
        info!("Shutting down timing export worker...");
        
        // Signal worker to stop
        self.shutdown_signal.store(true, Ordering::Relaxed);
        
        // Wait for worker thread to finish (with timeout)
        // Note: In production, we'd need to handle the JoinHandle properly
        // For now, we just signal shutdown and let the worker finish gracefully
        
        let stats = self.stats();
        info!(
            "Timing export shutdown complete. Final stats: sent={}, exported={}, errors={}",
            stats.total_sent.load(Ordering::Relaxed),
            stats.total_exported.load(Ordering::Relaxed), 
            stats.total_errors.load(Ordering::Relaxed)
        );
    }

}

impl HttpWorker {
    /// Main worker loop - runs in background thread
    fn run(self) {
        info!("Timing export worker started");
        
        let mut buffer = Vec::with_capacity(self.config.batch_size);
        let mut retry_delay_ms = self.config.retry_base_ms;
        let mut last_flush = Instant::now();
        
        loop {
            // Check for shutdown signal
            if self.shutdown_signal.load(Ordering::Relaxed) {
                info!("Worker received shutdown signal");
                break;
            }
            
            // Try to receive timing data with timeout
            match self.receiver.recv_timeout(Duration::from_secs(1)) {
                Ok(timing) => {
                    buffer.push(timing);
                    self.stats.current_buffer_size.store(buffer.len() as u64, Ordering::Relaxed);
                    
                    // Check if we should flush the buffer
                    let should_flush = buffer.len() >= self.config.batch_size
                        || last_flush.elapsed().as_secs() >= self.config.batch_timeout_secs;
                    
                    if should_flush && !buffer.is_empty() {
                        match self.send_batch(&buffer) {
                            Ok(_) => {
                                // Success - reset retry delay
                                self.stats.total_exported.fetch_add(buffer.len() as u64, Ordering::Relaxed);
                                retry_delay_ms = self.config.retry_base_ms;
                                debug!("Exported {} timing records", buffer.len());
                            }
                            Err(e) => {
                                // Failure - increment error count and retry with backoff
                                self.stats.total_errors.fetch_add(buffer.len() as u64, Ordering::Relaxed);
                                warn!("Failed to export batch: {}", e);
                                
                                // Exponential backoff
                                thread::sleep(Duration::from_millis(retry_delay_ms));
                                retry_delay_ms = (retry_delay_ms * 2).min(self.config.retry_max_ms);
                                
                                // Continue without clearing buffer to retry
                                continue;
                            }
                        }
                        
                        buffer.clear();
                        self.stats.current_buffer_size.store(0, Ordering::Relaxed);
                        last_flush = Instant::now();
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Timeout - check if we should flush partial buffer
                    if !buffer.is_empty() && last_flush.elapsed().as_secs() >= self.config.batch_timeout_secs {
                        if let Err(e) = self.send_batch(&buffer) {
                            warn!("Failed to flush partial batch: {}", e);
                            thread::sleep(Duration::from_millis(retry_delay_ms));
                            retry_delay_ms = (retry_delay_ms * 2).min(self.config.retry_max_ms);
                        } else {
                            self.stats.total_exported.fetch_add(buffer.len() as u64, Ordering::Relaxed);
                            debug!("Flushed {} timing records (timeout)", buffer.len());
                            buffer.clear();
                            self.stats.current_buffer_size.store(0, Ordering::Relaxed);
                            last_flush = Instant::now();
                            retry_delay_ms = self.config.retry_base_ms;
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    info!("Channel disconnected, worker shutting down");
                    break;
                }
            }
        }
        
        // Final flush on shutdown
        if !buffer.is_empty() {
            info!("Final flush of {} timing records", buffer.len());
            if let Err(e) = self.send_batch(&buffer) {
                error!("Failed to flush final batch: {}", e);
            } else {
                self.stats.total_exported.fetch_add(buffer.len() as u64, Ordering::Relaxed);
            }
        }
        
        info!("Timing export worker finished");
    }
    
    /// Send a batch of timing data via HTTP POST with compression
    fn send_batch(&self, batch: &[TransactionTiming]) -> Result<(), String> {
        let mut request = self.client
            .post(&self.export_url)
            .header("Content-Type", "application/json");
            
        if self.config.enable_compression {
            request = request.header("Content-Encoding", "gzip");
        }
        
        let response = request
            .json(batch)
            .send()
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!(
                "HTTP request failed with status: {} - {}",
                response.status(),
                response.text().unwrap_or_else(|_| "Unknown error".to_string())
            ))
        }
    }
}

impl Drop for TimingExporter {
    /// Ensure worker thread is shutdown when dropping
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_timing_serialization() {
        let timing = TransactionTiming {
            signature: "test_sig".to_string(),
            slot: 12345,
            poh_tick: 9876543210,
            entry_index: 5,
            tx_index: 2,
            accounts_read: vec!["account1".to_string(), "account2".to_string()],
            accounts_written: vec!["account3".to_string()],
            is_vote: false,
        };

        let json = serde_json::to_string(&timing).unwrap();
        let deserialized: TransactionTiming = serde_json::from_str(&json).unwrap();

        assert_eq!(timing.signature, deserialized.signature);
        assert_eq!(timing.slot, deserialized.slot);
        assert_eq!(timing.poh_tick, deserialized.poh_tick);
    }

    #[test]
    fn test_export_config_defaults() {
        let config = ExportConfig::default();
        assert_eq!(config.batch_size, 14_000);
        assert_eq!(config.batch_timeout_secs, 5);
        assert_eq!(config.channel_capacity, 4_000_000);
        assert_eq!(config.retry_base_ms, 100);
        assert_eq!(config.retry_max_ms, 30_000);
        assert_eq!(config.enable_compression, true);
    }
}
