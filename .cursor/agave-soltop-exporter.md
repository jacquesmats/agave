# Agave Timing Analysis Fork - RPC Replay Mode

## Overview

This fork of the Agave validator client enables **microsecond-precision transaction timing analysis** for historical MEV research and arbitrage analysis. It extracts transaction timing data including POH tick offsets, account access patterns, and commitment ordering from the **replay stage** during historical block processing on RPC nodes.

## Goal

**Primary Objective:** Build a high-resolution historical transaction timing analysis system that captures:

- POH tick offsets (100µs granularity timing) - **commitment order from historical blocks**
- Transaction commitment order from POH entries (during replay stage processing)
- Account access patterns per transaction
- Slot and execution metadata

**Use Case:** Analyze historical arbitrage opportunities and MEV extraction patterns by understanding the precise timing and ordering of transactions that accessed the same market accounts within past blocks.

**Important:** This fork runs on **RPC nodes** and analyzes historical data during ledger replay, not live transaction processing.

## Why This Fork is Necessary

Standard Agave RPC nodes do **not** expose the granular timing data needed for historical MEV analysis:

- ❌ RPC calls only provide slot-level timing (~400ms resolution)
- ❌ Geyser plugins receive post-processed data, missing entry-level POH timing
- ❌ Standard logs don't include POH tick progression during replay
- ✅ **This fork captures data during replay stage execution** with microsecond precision from historical blocks

## Architecture Overview

```
┌──────────────────┐    ┌──────────────────┐    ┌─────────────────────┐
│  Shred Fetch     │    │  Blockstore      │    │  Replay Stage       │
│  Stage           │───▶│  (RocksDB)       │───▶│  + Timing Hook      │
└──────────────────┘    └──────────────────┘    └─────────────────────┘
                                                           │
                                                           ▼
                                                  ┌─────────────────────┐
                                                  │  Analysis Service   │
                                                  │  (Your REST API)    │
                                                  └─────────────────────┘
```

**Data Flow for RPC Nodes:**

1. RPC node receives blocks from network via Turbine (shreds)
2. Shreds are stored in Blockstore (RocksDB database)
3. **Replay stage reads entries from Blockstore and executes transactions**
4. **Timing hook captures POH entry data during replay** (historical commitment order)
5. POH tick height from bank provides microsecond timestamps from original block
6. Data is exported via HTTP to your analysis service
7. Analysis service stores data and provides REST API for historical queries

## RPC Node vs Validator Differences

**Key Understanding:**

| Aspect        | Validator (Leader Mode) | RPC Node (Replay Mode)     |
| ------------- | ----------------------- | -------------------------- |
| Code Path     | Banking Stage (TPU)     | Replay Stage (TVU)         |
| Data Type     | Live transactions       | Historical blocks          |
| Hook Location | `consumer.rs`           | `blockstore_processor.rs`  |
| When Runs     | During leader slots     | During ledger catchup/sync |
| Use Case      | Real-time MEV           | Historical MEV analysis    |

**This fork targets RPC nodes for historical analysis.**

## Files Modified

### Core Implementation Files

#### 1. **`ledger/src/blockstore_processor.rs`** _(MODIFIED)_

- **Primary timing hook location for RPC nodes**
- Captures POH entry timing data during replay stage processing
- Accesses POH tick height from bank during historical transaction execution
- Extracts account access patterns from replayed transactions
- **This is where all the magic happens for RPC nodes**

#### 2. **`ledger/src/timing_exporter.rs`** _(NEW)_

- Handles non-blocking export of timing data during replay
- HTTP client for sending data to analysis service
- Batching support to handle high replay volumes
- JSON serialization of transaction timing data

#### 3. **`ledger/src/lib.rs`** _(MODIFIED)_

- Integrates `TimingExporter` into blockstore_processor module
- Updates module declarations to include timing_exporter

#### 4. **`validator/src/main.rs`** _(MODIFIED)_

- Adds CLI flag `--timing-export-url` for analysis service endpoint
- Adds CLI flag `--timing-export-during-replay` to enable export during catchup
- Passes configuration to blockstore_processor

### Configuration Files

#### 5. **`Cargo.toml`** (ledger package) _(MODIFIED)_

- Adds dependencies: `reqwest`, `tokio`, `serde_json`

## Implementation Details

### Timing Data Structure

```rust
#[derive(Debug, Clone, Serialize)]
pub struct TransactionTiming {
    pub signature: String,
    pub slot: u64,
    pub poh_tick: u64,           // 100µs resolution - from historical block entry
    pub entry_index: u64,         // Position within slot's entries
    pub accounts_read: Vec<String>,
    pub accounts_written: Vec<String>,
    pub execution_success: bool,
    pub is_vote: bool,            // Flag vote transactions separately
}
```

### Key Hook Location

**In `blockstore_processor.rs` during replay processing:**

```rust
// Location: ledger/src/blockstore_processor.rs
// Function: process_entries() or confirm_slot()

fn process_entries_with_timing(
    bank: &Arc<Bank>,
    entries: &[Entry],
    timing_exporter: &Option<TimingExporter>,
    // ... other params
) -> Result<()> {

    for (entry_idx, entry) in entries.iter().enumerate() {
        // Each entry represents a point in the PoH timeline
        // Transactions within an entry share the same PoH commitment time

        for transaction in &entry.transactions {
            // Execute transaction (existing replay logic)
            let execution_result = execute_transaction(bank, transaction, ...);

            // ✅ Capture timing data from historical block
            if let Some(exporter) = timing_exporter {
                let timing_data = TransactionTiming {
                    signature: transaction.signature().to_string(),
                    slot: bank.slot(),

                    // ✅ POH tick from bank during replay (historical data)
                    poh_tick: bank.tick_height(),

                    entry_index: entry_idx as u64,

                    // ✅ Extract account access patterns
                    accounts_read: transaction
                        .message()
                        .account_keys()
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| !transaction.message().is_writable(*i))
                        .map(|(_, key)| key.to_string())
                        .collect(),

                    accounts_written: transaction
                        .message()
                        .account_keys()
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| transaction.message().is_writable(*i))
                        .map(|(_, key)| key.to_string())
                        .collect(),

                    execution_success: execution_result.is_ok(),
                    is_vote: transaction.is_simple_vote_transaction(),
                };

                // Non-blocking batched export to analysis service
                exporter.export_async(timing_data);
            }
        }
    }

    Ok(())
}
```

**Key Insight:** Replay stage processes historical blocks that were already committed by leaders. The POH tick heights and transaction ordering are preserved from the original block, giving you the exact historical commitment order.

## Setup Instructions

### 1. Clone and Build

```bash
# Clone the Agave repository
git clone https://github.com/anza-xyz/agave.git
cd agave

# Create a new branch for your modifications
git checkout -b timing-analysis-rpc-fork

# Apply the modifications (implement the files described above)
# Key file: ledger/src/blockstore_processor.rs

# Build the modified validator
cargo build --release --bin agave-validator
```

### 2. Set Up Analysis Service

Your analysis service should provide an HTTP endpoint to receive timing data:

```bash
# Example endpoint that receives batched timing data
POST http://localhost:8080/ingest
Content-Type: application/json

[
  {
    "signature": "5D7n...",
    "slot": 123456,
    "poh_tick": 1234567890,
    "entry_index": 42,
    "accounts_read": ["9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"],
    "accounts_written": ["9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"],
    "execution_success": true,
    "is_vote": false
  },
  // ... more transactions (batched)
]
```

### 3. Run Modified RPC Node

```bash
./target/release/agave-validator \
    --timing-export-url "http://localhost:8080/ingest" \
    --timing-export-during-replay \
    --identity validator-keypair.json \
    --no-voting \
    --ledger /mnt/ledger \
    --accounts /mnt/accounts \
    --rpc-port 8899 \
    --rpc-bind-address 0.0.0.0 \
    --full-rpc-api \
    --private-rpc \
    --dynamic-port-range 8000-8020 \
    --entrypoint entrypoint.mainnet-beta.solana.com:8001 \
    --entrypoint entrypoint2.mainnet-beta.solana.com:8001 \
    --expected-genesis-hash 5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d \
    --only-known-rpc \
    --limit-ledger-size \
    --wal-recovery-mode skip_any_corrupted_record
```

**Important Flags:**

- `--timing-export-url`: Your analysis service endpoint
- `--timing-export-during-replay`: Enable timing export during ledger catchup/replay
- `--no-voting`: Confirms this is an RPC node, not a validator
- `--full-rpc-api`: Enable full RPC functionality

## Testing & Verification

### 1. Verify Timing Export During Replay

**Watch for initial catchup:**

```bash
# Monitor validator logs during startup/catchup
tail -f /mnt/ledger/validator.log | grep -E "(replay|timing)"

# You should see replay progress and timing exports
# Example log entries:
# [INFO] Replaying slot 123456 from blockstore
# [DEBUG] Exported 1000 timing records for slot 123456
```

**Check analysis service receives historical data:**

```bash
# Monitor your analysis service logs
tail -f /var/log/analysis-service.log

# You should see batched POST requests with timing data
# Example log entry:
# [2025-09-23T10:30:45Z] Received batch: 1000 transactions from slot 123456
```

### 2. Validate Historical Data Quality

**Test POH tick progression in historical blocks:**

```bash
# Query your analysis service for a historical slot
curl "http://localhost:8080/transactions?slot=123456" | jq '.[] | {signature, poh_tick, entry_index}' | head -20

# Verify POH ticks represent historical commitment order
# Expected output showing entry-level grouping:
# {"signature": "abc123", "poh_tick": 1234567890, "entry_index": 0}
# {"signature": "def456", "poh_tick": 1234567890, "entry_index": 0}  # Same entry
# {"signature": "ghi789", "poh_tick": 1234567895, "entry_index": 1}  # Next entry
```

**Verify entry-level grouping:**

```bash
# Transactions in the same entry should have the same POH tick
curl "http://localhost:8080/analyze/entry-groups?slot=123456" | jq

# Expected: Transactions grouped by entry_index with consistent poh_tick
```

### 3. Test Historical Account Access Tracking

**Analyze historical arbitrage patterns:**

```bash
# Find all transactions that touched a specific account in historical data
curl "http://localhost:8080/analyze/account-history" \
  -H "Content-Type: application/json" \
  -d '{
    "account_id": "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
    "slot_range": {
      "start": 123450,
      "end": 123460
    }
  }'

# Expected response with historical timing analysis
{
  "account": "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
  "slot_range": [123450, 123460],
  "transactions": [
    {
      "signature": "historical-tx-1",
      "slot": 123452,
      "poh_tick": 1234567845,
      "entry_index": 3,
      "accounts_accessed": ["9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM", "..."],
      "execution_success": true
    },
    {
      "signature": "historical-tx-2",
      "slot": 123455,
      "poh_tick": 1234568902,
      "tick_delta_us": 1057,  // 1057 microseconds after previous tx
      "accounts_accessed": ["9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM", "..."]
    }
  ]
}
```

### 4. Monitor Replay Performance

**Check replay doesn't slow down significantly:**

```bash
# Monitor slot replay rate
curl http://localhost:8899 -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSlot"}' | jq

# Check every 10 seconds to measure replay speed
# Good: 100+ slots/second during catchup
# Acceptable: 50+ slots/second
# Concerning: <20 slots/second (timing export may be bottleneck)
```

**Monitor validator metrics:**

```bash
# Check validator health
curl http://localhost:8899 -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | jq

# Monitor replay stage performance
ps aux | grep agave-validator
# Watch CPU and memory usage
```

### 5. Estimate Historical Data Volume

**Calculate export volume during catchup:**

```bash
# Monitor network traffic to analysis service during catchup
iftop -i eth0 -f "dst host localhost and port 8080"

# Typical catchup volume estimates:
# ~3000 TPS average = 3000 timing records/second during replay
# ~500 bytes per record = ~1.5MB/second of timing data
# Catching up 1 hour (9,000 slots) = ~32.4 GB of timing data
# Catching up 1 day (216,000 slots) = ~777 GB of timing data

# Plan storage capacity accordingly!
```

### 6. Test Batching Performance

**Verify batching is working:**

```bash
# Check analysis service receives batches, not individual records
curl http://localhost:8080/stats | jq '.batching'

# Expected output:
# {
#   "average_batch_size": 1000,
#   "batches_received": 15234,
#   "total_transactions": 15234000
# }

# If average_batch_size is 1, batching is not working properly
```

## Troubleshooting

### Common Issues

**1. No Timing Data During Replay**

```bash
# Check if timing export is enabled
grep "timing-export-during-replay" validator.log

# Verify flag is set when starting validator
ps aux | grep agave-validator | grep timing-export

# Check blockstore_processor logs
grep "timing_exporter" validator.log
```

**2. Analysis Service Connection Failed**

```bash
# Check network connectivity from RPC node
curl -v http://localhost:8080/health

# Verify timing export URL configuration
grep "timing-export-url" validator.log

# Check for firewall issues
sudo iptables -L | grep 8080
```

**3. Slow Replay Performance**

```bash
# Monitor replay bottleneck
# Check if timing export is causing backpressure
curl http://localhost:8899/metrics | grep replay

# Reduce batch size or increase batch interval
# Modify timing_exporter.rs configuration
```

**4. Missing Historical Data**

```bash
# Check if RPC node started from snapshot
grep "snapshot" validator.log

# If using snapshots, historical data before snapshot won't be replayed
# To get full historical data, start from genesis (not recommended for mainnet)
# Or download historical data from archives
```

**5. High Memory Usage During Catchup**

```bash
# Monitor memory usage during replay
watch -n 1 'ps aux | grep agave-validator'

# If memory spikes during catchup:
# 1. Reduce timing export batch size
# 2. Add memory limits to timing_exporter
# 3. Implement circuit breaker to pause exports under memory pressure
```

## Performance Considerations

**Impact on RPC Node During Replay:**

- ⚠️ **Moderate performance impact** - async HTTP export with batching during intensive replay
- ⚠️ **Additional memory usage** - ~10-50MB for timing export buffers (batching)
- ⚠️ **Network bandwidth** - ~1.5MB/s timing data export during catchup
- ⚠️ **Replay slowdown** - Expect 5-15% slower replay during catchup
- ✅ **No impact after catchup** - RPC node operates normally once caught up

**Optimization Tips:**

1. **Batching Configuration:**

```rust
// In timing_exporter.rs
pub struct BatchConfig {
    batch_size: usize,      // 500-2000 transactions per batch
    batch_timeout_ms: u64,  // 100-500ms max wait time
    max_buffer_size: usize, // 50MB max memory for buffering
}
```

2. **Conditional Export:**

```rust
// Skip vote transactions to reduce volume by 70%+
if !transaction.is_simple_vote_transaction() {
    timing_exporter.export(timing_data);
}
```

3. **Circuit Breaker:**

```rust
// Pause exports if analysis service is down
if timing_exporter.consecutive_failures() > 10 {
    warn!("Analysis service unreachable, pausing timing export");
    timing_exporter.pause();
}
```

4. **Compression:**

```rust
// Use gzip compression for batch HTTP requests
// Can reduce bandwidth by 80%+
client.post(url)
    .header("Content-Encoding", "gzip")
    .body(compress_batch(timing_data))
```

## Data Volume Planning

### Storage Requirements

**For your analysis service:**

```
Historical data retention:
├── 1 hour:   32.4 GB
├── 1 day:    777 GB
├── 1 week:   5.4 TB
└── 1 month:  23.3 TB

Compressed (gzip):
├── 1 hour:   6.5 GB (80% reduction)
├── 1 day:    155 GB
├── 1 week:   1.1 TB
└── 1 month:  4.7 TB
```

**Optimization strategies:**

- Filter vote transactions: -70% volume
- Store only MEV-relevant accounts: -50% volume
- Compress historical data: -80% size
- **Combined:** 97% reduction → 1 month = ~700 GB

### Catchup Scenarios

**Starting a new RPC node:**

| Scenario             | Slots to Replay | Time Estimate | Data Volume |
| -------------------- | --------------- | ------------- | ----------- |
| From 1-hour snapshot | ~9,000          | 10-20 minutes | 32 GB       |
| From 1-day snapshot  | ~216,000        | 4-8 hours     | 777 GB      |
| From 1-week snapshot | ~1,512,000      | 1-3 days      | 5.4 TB      |

**Recommendation:** Start from a recent snapshot (1-24 hours old) and incrementally backfill historical data as needed.

## Use Cases for Historical Analysis

### 1. MEV Pattern Analysis

```bash
# Analyze historical sandwich attacks
curl "http://localhost:8080/analyze/mev-patterns" \
  -d '{
    "pattern": "sandwich",
    "slot_range": [123000, 124000],
    "min_profit_lamports": 1000000
  }'
```

### 2. Arbitrage Opportunity Detection

```bash
# Find missed arbitrage opportunities in historical data
curl "http://localhost:8080/analyze/arbitrage-opportunities" \
  -d '{
    "pools": ["Orca", "Raydium"],
    "token_pair": "SOL/USDC",
    "time_range": "2024-09-20T00:00:00Z"
  }'
```

### 3. Account Contention Analysis

```bash
# Analyze which accounts had the most contention
curl "http://localhost:8080/analyze/account-contention" \
  -d '{
    "slot_range": [123000, 124000],
    "min_transactions": 10
  }'
```

### 4. Transaction Ordering Analysis

```bash
# Understand how transactions were ordered within blocks
curl "http://localhost:8080/analyze/ordering" \
  -d '{
    "slot": 123456,
    "account": "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"
  }'
```

## Security Notes

- **Data exposure:** This fork exports transaction signatures and account addresses from historical blocks (already public data)
- **Network security:** Use HTTPS for timing export in production
- **Access control:** Secure your analysis service endpoint with API keys
- **Data retention:** Implement appropriate data retention policies and comply with regulations
- **Resource limits:** Implement rate limiting on timing export to prevent DoS on analysis service

## Incremental Rollout Strategy

### Phase 1: Test on Devnet (1-2 days)

```bash
# Start RPC node on devnet with timing export
agave-validator \
    --timing-export-url "http://localhost:8080/ingest" \
    --timing-export-during-replay \
    ... # devnet configs

# Verify:
# ✅ Timing data exports correctly
# ✅ Replay performance acceptable
# ✅ No memory leaks during catchup
```

### Phase 2: Test on Testnet (1 week)

```bash
# Run on testnet with production-like traffic
# Monitor for 7 days to ensure stability
# Verify data quality and completeness
```

### Phase 3: Mainnet with Limited Scope (2 weeks)

```bash
# Start on mainnet but only export non-vote transactions
# Monitor performance and data volume
# Tune batching and compression settings
```

### Phase 4: Full Mainnet Deployment

```bash
# Deploy with optimized settings
# Monitor long-term stability
# Scale analysis service as needed
```

## Next Steps

1. **Implement the core modifications** in `ledger/src/blockstore_processor.rs`
2. **Create timing_exporter module** in `ledger/src/timing_exporter.rs`
3. **Build and test on devnet** with a local analysis service
4. **Validate historical data quality** using the testing procedures above
5. **Optimize batching and compression** based on devnet testing results
6. **Deploy to testnet** for extended testing
7. **Scale analysis service** to handle mainnet historical data volumes
8. **Deploy to mainnet** and start building your historical MEV analysis tools

## Support

For issues specific to this timing analysis fork:

- Review the implementation in `ledger/src/blockstore_processor.rs`
- Check validator logs for timing export errors during replay
- Verify analysis service connectivity and data reception
- Monitor RPC node performance metrics during catchup
- Check data completeness in your analysis service

## FAQ

**Q: Will this work during normal operation after catchup?**
A: Yes! The replay stage continues to process new blocks as they arrive. Your timing hooks will capture all historical transactions, both during initial catchup and ongoing operation.

**Q: Can I backfill older historical data?**
A: Yes, but you'll need to replay from an older snapshot or ledger backup. The RPC node will re-process those blocks through replay stage, triggering your timing exports.

**Q: What about vote transactions?**
A: Vote transactions represent ~70% of transaction volume. Consider filtering them out (`is_vote` flag) unless you specifically need voting pattern analysis.

**Q: How do I minimize impact on RPC performance?**
A: Use aggressive batching (1000+ transactions per batch), enable compression, and implement a circuit breaker to pause exports under load.

**Q: Can I run this alongside Geyser plugins?**
A: Yes! This fork is complementary to Geyser plugins. Geyser provides high-level notifications, while this fork gives you microsecond POH timing data.

---

**⚠️ Important:** This is a custom modification of Agave that captures historical POH timing data during replay stage on RPC nodes. This is for **historical analysis only** - you're analyzing what already happened, not capturing live transaction flow. Thoroughly test on devnet/testnet before deploying to mainnet.
