use {
    log::*,
    reqwest::blocking::Client,
    serde::{Deserialize, Serialize},
    std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
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

/// Configuration for timing export batching
#[derive(Debug, Clone)]
pub struct BatchConfig {
    pub batch_size: usize,       // Number of transactions per batch
    pub batch_timeout_ms: u64,   // Max time to wait before sending partial batch
    pub max_retries: u32,        // Max HTTP retry attempts
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,      // Small batches for sync HTTP to avoid blocking
            batch_timeout_ms: 1000, // 1 second max wait
            max_retries: 3,
        }
    }
}

/// Internal state for TimingExporter
struct TimingExporterState {
    buffer: VecDeque<TransactionTiming>,
    last_export: Instant,
    total_exported: u64,
    total_errors: u64,
}

/// Synchronous HTTP exporter for transaction timing data
pub struct TimingExporter {
    client: Client,
    export_url: String,
    batch_config: BatchConfig,
    state: Arc<Mutex<TimingExporterState>>,
}

impl TimingExporter {
    /// Create a new timing exporter with synchronous HTTP client
    pub fn new(export_url: String, batch_config: Option<BatchConfig>) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

        Ok(Self {
            client,
            export_url,
            batch_config: batch_config.unwrap_or_default(),
            state: Arc::new(Mutex::new(TimingExporterState {
                buffer: VecDeque::new(),
                last_export: Instant::now(),
                total_exported: 0,
                total_errors: 0,
            })),
        })
    }

    /// Add a transaction timing record to the export buffer
    pub fn export(&self, timing: TransactionTiming) {
        let mut state = self.state.lock().unwrap();
        state.buffer.push_back(timing);

        // Check if we should flush the buffer
        let should_flush = state.buffer.len() >= self.batch_config.batch_size
            || state.last_export.elapsed().as_millis() > self.batch_config.batch_timeout_ms as u128;

        if should_flush {
            drop(state); // Release lock before calling flush_buffer
            self.flush_buffer();
        }
    }

    /// Force flush any buffered timing data
    pub fn flush(&self) {
        let state = self.state.lock().unwrap();
        if !state.buffer.is_empty() {
            drop(state);
            self.flush_buffer();
        }
    }

    /// Get export statistics
    pub fn stats(&self) -> (u64, u64) {
        let state = self.state.lock().unwrap();
        (state.total_exported, state.total_errors)
    }

    /// Internal method to flush buffer to HTTP endpoint
    fn flush_buffer(&self) {
        let mut state = self.state.lock().unwrap();
        if state.buffer.is_empty() {
            return;
        }

        // Collect current buffer into a batch
        let batch: Vec<TransactionTiming> = state.buffer.drain(..).collect();
        let batch_size = batch.len();

        // Attempt to send with retries
        let mut attempts = 0;
        let mut success = false;

        while attempts <= self.batch_config.max_retries && !success {
            attempts += 1;

            // Release lock during HTTP call
            drop(state);
            
            match self.send_batch(&batch) {
                Ok(_) => {
                    let mut state = self.state.lock().unwrap();
                    state.total_exported += batch_size as u64;
                    success = true;
                    debug!(
                        "Exported {} timing records (total: {})",
                        batch_size, state.total_exported
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to export timing batch (attempt {}/{}): {}",
                        attempts, self.batch_config.max_retries + 1, e
                    );
                    
                    if attempts <= self.batch_config.max_retries {
                        // Brief delay before retry
                        std::thread::sleep(Duration::from_millis(100 * attempts as u64));
                    }
                }
            }
            
            // Re-acquire lock for next iteration
            state = self.state.lock().unwrap();
        }

        if !success {
            state.total_errors += batch_size as u64;
            error!(
                "Failed to export timing batch after {} attempts, dropping {} records",
                self.batch_config.max_retries + 1, batch_size
            );
        }

        state.last_export = Instant::now();
    }

    /// Send a batch of timing data via synchronous HTTP POST
    fn send_batch(&self, batch: &[TransactionTiming]) -> Result<(), String> {
        let response = self
            .client
            .post(&self.export_url)
            .header("Content-Type", "application/json")
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
    /// Ensure any remaining buffered data is exported when dropping
    fn drop(&mut self) {
        let state = self.state.lock().unwrap();
        if !state.buffer.is_empty() {
            info!(
                "Flushing {} remaining timing records on drop",
                state.buffer.len()
            );
            drop(state);
            self.flush_buffer();
        }
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
    fn test_batch_config_defaults() {
        let config = BatchConfig::default();
        assert_eq!(config.batch_size, 100);
        assert_eq!(config.batch_timeout_ms, 1000);
        assert_eq!(config.max_retries, 3);
    }
}
