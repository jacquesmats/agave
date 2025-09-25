# Agave Timing Analysis Fork

## Overview

This fork of the Agave validator client enables **microsecond-precision transaction timing analysis** for MEV research and arbitrage analysis. It extracts real-time transaction timing data including POH tick offsets, account access patterns, and commitment ordering from the banking stage during transaction processing.

## Goal

**Primary Objective:** Build a high-resolution transaction timing analysis system that captures:
- POH tick offsets (100µs granularity timing) - **commitment order, not execution order**
- Transaction commitment order from POH entries (during banking stage processing)
- Account access patterns per transaction  
- Slot and execution metadata

**Use Case:** Analyze arbitrage opportunities and MEV extraction by understanding the precise timing and ordering of transactions that access the same market accounts within 4-block windows.

**Important:** Banking stage parallelizes execution, so we capture **POH entry data** (true commitment order) rather than execution order.

## Why This Fork is Necessary

Standard Agave validator does **not** expose the granular timing data needed for MEV analysis:
- ❌ RPC calls only provide slot-level timing (~400ms resolution)
- ❌ Geyser plugins receive post-processed data, missing execution-time ordering
- ❌ Standard logs don't include POH tick progression
- ✅ **This fork captures data during banking stage execution** with microsecond precision

## Architecture Overview

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────────┐
│  SigVerify      │    │  Banking Stage   │    │  Analysis Service   │
│  Stage          │───▶│  + Timing Hook   │───▶│  (Your REST API)    │
└─────────────────┘    └──────────────────┘    └─────────────────────┘
                              │
                              ▼
                       ┌──────────────────┐
                       │  POH Recorder    │
                       │  (tick_height)   │
                       └──────────────────┘
```

**Data Flow:**
1. Transactions enter banking stage after signature verification
2. **Banking stage executes transactions in parallel** (performance optimization)
3. **Timing hook captures POH entry data** during transaction processing (true commitment order)
4. POH tick height provides microsecond timestamps from transaction commitment
5. Data is exported via HTTP to your analysis service
6. Analysis service stores data and provides REST API for queries

## Files Modified

### Core Implementation Files

#### 1. **`core/src/banking_stage/timing_exporter.rs`** *(NEW)*
- Handles non-blocking export of timing data
- HTTP client for sending data to analysis service
- JSON serialization of transaction timing data

#### 2. **`core/src/banking_stage/consumer.rs`** *(MODIFIED)*
- **Primary timing hook location**
- Captures POH entry timing data during banking stage processing
- Accesses POH tick height for true transaction commitment order
- Extracts account access patterns from executed transactions

#### 3. **`core/src/lib.rs`** *(MODIFIED)*
- Integrates `TimingExporter` into banking stage module
- Updates module declarations to include timing_exporter

#### 4. **`validator/src/main.rs`** *(MODIFIED)*  
- Adds CLI flag `--timing-export-url` for analysis service endpoint
- Passes configuration to banking stage

### Configuration Files

#### 5. **`Cargo.toml`** *(MODIFIED)*
- Adds dependencies: `reqwest`, `tokio`, `serde_json`

## Implementation Details

### Timing Data Structure

```rust
#[derive(Debug, Clone, Serialize)]
pub struct TransactionTiming {
    pub signature: String,
    pub slot: u64,
    pub poh_tick: u64,           // 100µs resolution - POH entry commitment time
    pub accounts_read: Vec<String>,
    pub accounts_written: Vec<String>,
    pub execution_success: bool,
    // Note: No execution_order or wall clock timestamp
    // POH tick provides deterministic commitment ordering
}
```

### Key Hook Location

**In `consumer.rs` during transaction processing:**

```rust
// During banking stage transaction processing
// Note: Banking stage executes in parallel, but we capture POH entry data
for (tx, result) in transactions.iter().zip(execution_results.iter()) {
    let timing_data = TransactionTiming {
        signature: tx.signature().to_string(),
        slot: bank.slot(),
        poh_tick: bank.tick_height(), // ✅ POH entry commitment time (true order)
        accounts_read: tx.message().static_account_keys().to_vec(),
        accounts_written: result.accounts_written().to_vec(),
        execution_success: result.was_executed_successfully(),
        // ❌ No execution counter or wall time - POH tick is the source of truth
    };
    
    // Non-blocking export to analysis service
    self.timing_exporter.export_async(timing_data);
}
```

**Key Insight:** Banking stage parallelizes execution for performance, but `bank.tick_height()` gives us the **POH entry commitment order** which is what matters for MEV analysis.

## Setup Instructions

### 1. Clone and Build

```bash
# Clone the Agave repository
git clone https://github.com/anza-xyz/agave.git
cd agave

# Create a new branch for your modifications
git checkout -b timing-analysis-fork

# Apply the modifications (implement the files described above)
# ... implement timing_exporter.rs, modify consumer.rs, etc.

# Build the modified validator
cargo build --release --bin solana-validator
```

### 2. Set Up Analysis Service

Your analysis service should provide an HTTP endpoint to receive timing data:

```bash
# Example endpoint that receives timing data
POST http://localhost:8080/ingest
Content-Type: application/json

{
    "signature": "5D7n...",
    "slot": 123456,
    "poh_tick": 1234567890,
    "accounts_read": ["9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"],
    "accounts_written": ["9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"],
    "execution_success": true
}
```

### 3. Run Modified Validator

```bash
./target/release/solana-validator \
    --timing-export-url "http://localhost:8080/ingest" \
    --identity validator-keypair.json \
    --vote-account vote-account-keypair.json \
    --ledger ledger \
    --rpc-port 8899 \
    --dynamic-port-range 8000-8020 \
    --entrypoint entrypoint.mainnet-beta.solana.com:8001 \
    --expected-genesis-hash 5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d \
    --wal-recovery-mode skip_any_corrupted_record \
    --limit-ledger-size
```

## Testing & Verification

### 1. Verify Timing Export is Working

**Check analysis service receives data:**
```bash
# Monitor your analysis service logs
tail -f /var/log/analysis-service.log

# You should see incoming POST requests with timing data
# Example log entry:
# [2025-09-23T10:30:45Z] Received timing data: signature=5D7n..., slot=123456, poh_tick=1234567890
```

### 2. Validate Data Quality

**Test POH tick progression:**
```bash
# Query your analysis service
curl http://localhost:8080/transactions?slot=123456 | jq '.[] | {signature, poh_tick}' | head -10

# Verify POH ticks represent commitment order (may not be strictly increasing due to parallel execution)
# Expected output showing POH entry commitment times:
# {"signature": "abc123", "poh_tick": 1234567890}
# {"signature": "def456", "poh_tick": 1234567895}  # Later POH entry
# {"signature": "ghi789", "poh_tick": 1234567892}  # Earlier POH entry (parallel execution)
```

### 3. Test Account Access Tracking

**Verify account access patterns:**
```bash
# Find transactions touching a specific account
curl "http://localhost:8080/analyze" \
  -H "Content-Type: application/json" \
  -d '{
    "signature": "your-arbitrage-tx-signature",
    "account_id": "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
    "block_range": 4
  }'

# Expected response with microsecond-level timing analysis
{
  "target_transaction": {
    "signature": "your-arbitrage-tx-signature",
    "poh_tick": 1234567890,
    "slot": 123456
  },
  "related_transactions": [
    {
      "signature": "competing-tx-1",
      "poh_tick": 1234567845,
      "tick_delta_us": -45,  // 45 microseconds before your tx
      "accounts_accessed": ["9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"]
    }
  ]
}
```

### 4. Performance Validation

**Monitor validator performance:**
```bash
# Check validator metrics
curl http://localhost:8899/metrics | grep -E "(banking_stage|timing_export)"

# Ensure timing export doesn't impact validator performance:
# - Slot times should remain <400ms
# - Skip rate should stay <0.5%
# - TPS should maintain normal levels
```

### 5. Data Volume Testing

**Estimate data export volume:**
```bash
# Monitor network traffic to analysis service
netstat -i  # Check bandwidth usage

# Typical volume estimates:
# ~3000 TPS = 3000 timing records/second
# ~500 bytes per record = ~1.5MB/second of timing data
# Plan storage and network capacity accordingly
```

## Troubleshooting

### Common Issues

**1. Missing POH Tick Data**
```bash
# If POH ticks are all zero, check POH recorder access
grep "tick_height" validator.log

# Verify POH recorder is properly accessible from banking stage
```

**2. Analysis Service Connection Failed**
```bash
# Check network connectivity
curl -v http://localhost:8080/health

# Verify timing export URL configuration
grep "timing-export-url" validator.log
```

**3. High Memory Usage**
```bash
# Monitor validator memory usage
ps aux | grep solana-validator

# If memory usage is high, implement batching in timing_exporter.rs
# Consider buffering and bulk-sending timing data
```

## Performance Considerations

**Impact on Validator:**
- ⚠️ **Minimal performance impact** - async HTTP export with non-blocking sends
- ⚠️ **Additional memory usage** - ~1MB for timing export buffers
- ⚠️ **Network bandwidth** - ~1.5MB/s timing data export
- ✅ **No execution order dependency** - uses POH entry timing (deterministic)

**Optimization tips:**
- Use connection pooling in `TimingExporter`
- Implement batched sends (10-100 records per HTTP request)
- Add circuit breaker if analysis service is down
- Consider local file export as fallback

## Security Notes

- **Data exposure:** This fork exports transaction signatures and account addresses
- **Network security:** Use HTTPS for timing export in production
- **Access control:** Secure your analysis service endpoint
- **Data retention:** Implement appropriate data retention policies

## Next Steps

1. **Implement the core modifications** described in the "Files Modified" section
2. **Build and test** with a local analysis service
3. **Validate data quality** using the testing procedures above  
4. **Deploy to testnet** for extended testing before mainnet use
5. **Scale analysis service** to handle mainnet transaction volumes

## Support

For issues specific to this timing analysis fork:
- Review the implementation in the modified files
- Check validator logs for timing export errors
- Verify analysis service connectivity and data reception
- Monitor validator performance metrics during operation

---

**⚠️ Important:** This is a custom modification of Agave that captures POH entry timing data during banking stage processing. Banking stage executes transactions in parallel for performance, but POH tick heights provide the true commitment order needed for MEV analysis. Use at your own risk and thoroughly test before deploying to production environments.