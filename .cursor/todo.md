# Agave Timing Analysis Fork - Implementation Todo List

## Overview

This todo list provides a step-by-step action plan to implement the microsecond-precision transaction timing analysis system for MEV research and arbitrage analysis in the Agave validator.

## Implementation Tasks

### Phase 1: Research & Setup

- [ ] **Research Dependencies** - Investigate existing dependencies in Cargo.toml for HTTP client, async runtime, and JSON serialization requirements
- [ ] **Add Cargo Dependencies** - Add required dependencies (reqwest, tokio, serde_json) to core/Cargo.toml for HTTP export functionality

### Phase 2: Core Implementation

- [ ] **Create TimingExporter Module** - Create the new TimingExporter module in `core/src/banking_stage/timing_exporter.rs` with HTTP client and async data export
- [ ] **Update Core Library** - Update `core/src/lib.rs` to include the new timing_exporter module in banking_stage
- [ ] **Modify Consumer** - Modify `core/src/banking_stage/consumer.rs` to integrate timing hooks and capture POH tick data during transaction processing

### Phase 3: CLI Integration

- [ ] **Add CLI Flag** - Add `--timing-export-url` CLI argument to validator command line parsing in `validator/src/commands/run/args.rs`
- [ ] **Pass Configuration** - Modify validator startup flow to pass timing export URL configuration from CLI to banking stage components

### Phase 4: Testing & Validation

- [ ] **Test Compilation** - Test compilation of modified validator with `cargo build --release --bin solana-validator`
- [ ] **Create Test Service** - Create a simple HTTP test service to receive timing data for testing and validation
- [ ] **Test Integration** - Test the complete integration by running modified validator with test analysis service
- [ ] **Validate POH Data** - Validate that POH tick heights are correctly captured and represent true commitment order
- [ ] **Performance Testing** - Conduct performance testing to ensure timing export doesn't impact validator performance

## Technical Implementation Details

### Files to Modify/Create:

#### 1. New File: `core/src/banking_stage/timing_exporter.rs`

**Purpose**: Handles non-blocking export of timing data via HTTP
**Key Components**:

- `TransactionTiming` struct with fields: signature, slot, poh_tick, accounts_read, accounts_written, execution_success
- `TimingExporter` struct with HTTP client and async export methods
- Error handling and circuit breaker for analysis service downtime

#### 2. Modified: `core/src/banking_stage/consumer.rs`

**Purpose**: Integrate timing hooks during transaction processing
**Key Changes**:

- Add timing exporter field to Consumer struct
- Insert timing data capture in `execute_and_commit_transactions_locked` method
- Access `bank.tick_height()` for POH entry commitment time
- Extract account access patterns from transaction metadata

#### 3. Modified: `core/src/lib.rs`

**Purpose**: Include new timing_exporter module
**Key Changes**:

- Add `pub mod timing_exporter;` within banking_stage module

#### 4. Modified: `validator/src/commands/run/args.rs`

**Purpose**: Add CLI argument for timing export URL
**Key Changes**:

- Add `--timing-export-url` argument with URL validator
- Optional parameter with help text for analysis service endpoint

#### 5. Modified: `validator/src/commands/run/execute.rs`

**Purpose**: Pass timing configuration to validator
**Key Changes**:

- Extract timing export URL from CLI matches
- Pass configuration through ValidatorConfig or validator constructor

#### 6. Modified: `core/Cargo.toml`

**Purpose**: Add required dependencies
**Key Dependencies**:

- `reqwest = { version = "0.12", features = ["json"] }`
- `tokio = { version = "1.45", features = ["rt", "rt-multi-thread"] }`
- `serde_json = "1.0"`

### Data Structure Definition:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct TransactionTiming {
    pub signature: String,
    pub slot: u64,
    pub poh_tick: u64,           // 100µs resolution - POH entry commitment time
    pub accounts_read: Vec<String>,
    pub accounts_written: Vec<String>,
    pub execution_success: bool,
}
```

### Key Hook Location:

The primary timing data capture occurs in `consumer.rs` within the `execute_and_commit_transactions_locked` method, after transaction execution but during the commit phase when POH tick height represents true commitment order.

### Performance Considerations:

- Use async/non-blocking HTTP requests to prevent validator performance impact
- Implement connection pooling and batching for efficiency
- Add circuit breaker pattern for analysis service downtime
- Monitor memory usage and network bandwidth (~1.5MB/s estimated)

### Testing Strategy:

1. **Unit Tests**: Test TimingExporter functionality in isolation
2. **Integration Tests**: Test complete data flow from banking stage to analysis service
3. **Performance Tests**: Ensure <1% performance impact on validator metrics
4. **Data Quality Tests**: Validate POH tick ordering and account access accuracy

### Security Notes:

- Timing data includes transaction signatures and account addresses
- Use HTTPS for production timing export
- Implement proper access controls on analysis service
- Consider data retention and privacy policies

---

**Note**: This implementation captures POH entry timing data during banking stage processing. Banking stage executes transactions in parallel for performance, but POH tick heights provide the true commitment order needed for MEV analysis.

**Status**: Ready for implementation - all tasks defined with clear technical specifications.
