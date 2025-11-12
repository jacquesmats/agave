# Agave Timing Analysis Fork - Implementation Action Plan

## Overview

This document provides a comprehensive action plan to implement the microsecond-precision transaction timing analysis system for historical MEV research. The implementation targets RPC nodes during replay stage processing.

## Implementation Phases

### 🚀 **Phase 0: Proof of Concept**

- [ ] **Task 0.1** - Add `--timing-export-url` CLI flag parsing (ignore URL value in POC)
- [ ] **Task 0.2** - Add transaction logging in `process_entries()` using `info!` macro
- [ ] **Task 0.3** - Log first 100 transactions per slot with all timing data
- [ ] **Task 0.4** - Build and test on mainnet RPC during ledger catchup
- [ ] **Task 0.5** - Validate POH tick progression and account data quality

### 📋 **Phase 1: Setup and Dependencies**

- [ ] **Task 1.1** - Add `reqwest` dependency for HTTP client functionality
- [ ] **Task 1.2** - Add `tokio` with async runtime features
- [ ] **Task 1.3** - Add `serde_json` for JSON serialization
- [ ] **Task 1.4** - Update ledger/Cargo.toml with required dependencies

### 🔧 **Phase 2: CLI Infrastructure**

- [ ] **Task 2.1** - Expand `--timing-export-url` CLI argument to handle URL validation
- [ ] **Task 2.2** - Add `--timing-export-during-replay` CLI flag in `validator/src/commands/run/args.rs`
- [ ] **Task 2.3** - Parse timing export arguments in `validator/src/commands/run/execute.rs`
- [ ] **Task 2.4** - Thread timing config through ValidatorConfig struct

### 🏗️ **Phase 3: Core Timing Exporter Module**

- [ ] **Task 3.1** - Create `ledger/src/timing_exporter.rs` with `TransactionTiming` struct
- [ ] **Task 3.2** - Implement `TimingExporter` with async HTTP client and batching
- [ ] **Task 3.3** - Add circuit breaker and error handling in timing_exporter.rs
- [ ] **Task 3.4** - Export timing_exporter module in `ledger/src/lib.rs`

### 🔗 **Phase 4: Blockstore Processor Integration**

- [ ] **Task 4.1** - Replace POC logging with `TimingExporter` in `process_entries` function
- [ ] **Task 4.2** - Add execution success tracking to timing capture logic
- [ ] **Task 4.3** - Refactor account access pattern extraction for production use
- [ ] **Task 4.4** - Remove 100 transaction limit and implement full capture

### 🔌 **Phase 5: Validator Configuration Pipeline**

- [ ] **Task 5.1** - Pass timing export config from `ValidatorConfig` to replay services
- [ ] **Task 5.2** - Initialize `TimingExporter` in blockstore processor setup
- [ ] **Task 5.3** - Thread timing exporter through `process_blockstore_from_root` function
- [ ] **Task 5.4** - Ensure timing exporter is properly passed to replay stage

### 🧪 **Phase 6: Testing Infrastructure**

- [ ] **Task 6.1** - Create simple HTTP server for testing timing data reception
- [ ] **Task 6.2** - Add unit tests for `TransactionTiming` serialization
- [ ] **Task 6.3** - Add integration tests for `TimingExporter` batching logic
- [ ] **Task 6.4** - Test timing export during devnet replay with sample analysis service

### 📊 **Phase 7: Performance Optimization**

- [ ] **Task 7.1** - Implement compression for batched HTTP requests
- [ ] **Task 7.2** - Add configurable batch size and timeout parameters
- [ ] **Task 7.3** - Implement vote transaction filtering to reduce volume
- [ ] **Task 7.4** - Add memory usage monitoring and backpressure handling

### 🔍 **Phase 8: Validation and Testing**

- [ ] **Task 8.1** - Test on devnet and verify POH tick progression accuracy
- [ ] **Task 8.2** - Validate account access pattern extraction completeness
- [ ] **Task 8.3** - Measure replay performance impact and optimize if needed
- [ ] **Task 8.4** - Test error handling and recovery scenarios

### 📚 **Phase 9: Documentation and Deployment**

- [ ] **Task 9.1** - Create deployment guide with configuration examples
- [ ] **Task 9.2** - Document analysis service API requirements
- [ ] **Task 9.3** - Create monitoring and alerting recommendations
- [ ] **Task 9.4** - Test deployment on testnet with full historical catchup

## Key Files to Modify

### Core Implementation Files

1. **`ledger/src/timing_exporter.rs`** _(NEW)_ - Main timing export module
2. **`ledger/src/blockstore_processor.rs`** _(MODIFIED)_ - Add timing hooks during replay
3. **`ledger/src/lib.rs`** _(MODIFIED)_ - Export timing_exporter module
4. **`validator/src/commands/run/args.rs`** _(MODIFIED)_ - Add CLI arguments
5. **`validator/src/commands/run/execute.rs`** _(MODIFIED)_ - Parse CLI and configure validator
6. **`ledger/Cargo.toml`** _(MODIFIED)_ - Add HTTP client dependencies

### Key Integration Points

- **Entry Point**: CLI arguments → ValidatorConfig → Blockstore Processor
- **Hook Location**: `process_entries()` function in `blockstore_processor.rs`
- **Data Flow**: POH entries → Transaction extraction → Timing capture → HTTP export
- **Performance**: Async batching to minimize replay stage impact

## Data Structure Overview

```rust
#[derive(Debug, Clone, Serialize)]
pub struct TransactionTiming {
    pub signature: String,
    pub slot: u64,
    pub poh_tick: u64,           // 100µs resolution from historical block
    pub entry_index: u64,        // Position within slot's entries
    pub accounts_read: Vec<String>,
    pub accounts_written: Vec<String>,
    pub execution_success: bool,
    pub is_vote: bool,           // Flag vote transactions separately
}
```

## Expected Timeline

- **Phase 0**: 1 day (POC validation on mainnet)
- **Phase 1-3**: 2-3 days (Setup and core module)
- **Phase 4-5**: 2-3 days (Integration with replay stage - building on POC)
- **Phase 6-7**: 2-3 days (Testing and optimization)
- **Phase 8-9**: 1-2 days (Validation and documentation)
- **Total**: ~8-12 days for complete implementation

## Critical Success Factors

1. **POC Validation**: Prove data accessibility and quality on mainnet replay
2. **Minimal Performance Impact**: Async export with aggressive batching
3. **Data Quality**: Accurate POH tick capture and account access patterns
4. **Error Resilience**: Circuit breakers and graceful degradation
5. **Production Ready**: Build incrementally from working POC

## Testing Strategy

1. **Unit Tests**: Individual component functionality
2. **Integration Tests**: End-to-end timing export flow
3. **Performance Tests**: Replay speed impact measurement
4. **Devnet Testing**: Real-world validation with sample analysis service
5. **Testnet Testing**: Extended testing with production-like data volumes

---

**Status**: Ready for implementation  
**Priority**: High - Historical MEV analysis capability  
**Risk Level**: Medium - Requires careful integration with replay stage
