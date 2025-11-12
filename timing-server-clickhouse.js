const express = require("express");
const fs = require("fs");
const path = require("path");
const zlib = require("zlib");
const { createClient } = require("@clickhouse/client");

const app = express();
const PORT = process.env.PORT || 8080;
const CLICKHOUSE_URL = process.env.CLICKHOUSE_URL || "http://localhost:8123";
const CLICKHOUSE_DATABASE = process.env.CLICKHOUSE_DATABASE || "solana";

// Statistics tracking
let stats = {
  totalTransactions: 0,
  totalBatches: 0,
  totalBytes: 0,
  startTime: new Date(),
  lastBatchTime: null,
  slotsProcessed: new Set(),
  voteTransactions: 0,
  nonVoteTransactions: 0,
  clickhouseErrors: 0,
  clickhouseInserts: 0,
};

// ClickHouse client with connection pooling
let clickhouseClient;

// Batch manager for efficient ClickHouse writes
class BatchManager {
  constructor(maxBatchSize = 50000, flushTimeoutMs = 30000) {
    this.records = [];
    this.maxBatchSize = maxBatchSize;
    this.flushTimeoutMs = flushTimeoutMs;
    this.lastFlush = Date.now();
    this.isProcessing = false;
  }

  addRecords(transactions, batchId) {
    const now = Date.now();

    // Transform timing data to ClickHouse format
    const records = transactions.map((tx) => ({
      timestamp: new Date().toISOString().slice(0, 19), // YYYY-MM-DD HH:mm:ss
      slot: tx.slot,
      poh_tick: tx.poh_tick,
      entry_index: tx.entry_index,
      tx_index: tx.tx_index,
      signature: tx.signature,
      signer: tx.signer, // Fee payer wallet address
      accounts_read: tx.accounts_read,
      accounts_written: tx.accounts_written,
      is_vote: tx.is_vote ? 1 : 0,
      accounts_read_count: tx.accounts_read?.length || 0,
      accounts_written_count: tx.accounts_written?.length || 0,
    }));

    this.records.push(...records);

    // Check if we should flush
    const shouldFlushSize = this.records.length >= this.maxBatchSize;
    const shouldFlushTime = now - this.lastFlush >= this.flushTimeoutMs;

    if ((shouldFlushSize || shouldFlushTime) && !this.isProcessing) {
      this.flush(batchId);
    }
  }

  async flush(currentBatchId) {
    if (this.records.length === 0 || this.isProcessing) return;

    this.isProcessing = true;
    const recordsToFlush = [...this.records];
    const recordCount = recordsToFlush.length;
    this.records = [];
    this.lastFlush = Date.now();

    console.log(
      `🔄 Starting ClickHouse insert: ${recordCount} records (Batch ID: ${currentBatchId})`
    );

    try {
      const startTime = Date.now();

      // Bulk insert to ClickHouse
      await clickhouseClient.insert({
        table: "timing_data",
        values: recordsToFlush,
        format: "JSONEachRow",
      });

      const dataInsertTime = Date.now() - startTime;
      console.log(
        `✅ ClickHouse data insert completed: ${recordCount} records in ${dataInsertTime}ms`
      );

      // Update processing state
      const stateStartTime = Date.now();
      await clickhouseClient.insert({
        table: "timing_export_state",
        values: [
          {
            id: 1,
            last_batch_id: currentBatchId,
            last_timestamp: new Date().toISOString().slice(0, 19),
            total_transactions: stats.totalTransactions,
            updated_at: new Date().toISOString().slice(0, 19),
          },
        ],
        format: "JSONEachRow",
      });

      const stateInsertTime = Date.now() - stateStartTime;
      const totalFlushTime = Date.now() - startTime;
      stats.clickhouseInserts++;

      console.log(
        `💾 ClickHouse: Inserted ${recordCount} records in ${totalFlushTime}ms (data: ${dataInsertTime}ms, state: ${stateInsertTime}ms)`
      );

      if (totalFlushTime > 10000) {
        console.warn(
          `⚠️  VERY SLOW ClickHouse insert: ${totalFlushTime}ms for ${recordCount} records`
        );
      } else if (totalFlushTime > 5000) {
        console.warn(`⚠️  Slow ClickHouse insert: ${totalFlushTime}ms`);
      }
    } catch (error) {
      const flushTime = Date.now() - startTime;
      console.error(
        `❌ ClickHouse insert FAILED after ${flushTime}ms: ${error.message}`
      );
      console.error(`   Records: ${recordCount}, Batch ID: ${currentBatchId}`);
      console.error(`   Error details:`, error);
      stats.clickhouseErrors++;

      // Re-add failed records to retry later
      this.records.unshift(...recordsToFlush);
    } finally {
      this.isProcessing = false;
    }
  }

  async forceFlush(batchId) {
    await this.flush(batchId);
  }

  getStats() {
    return {
      pendingRecords: this.records.length,
      isProcessing: this.isProcessing,
      lastFlush: this.lastFlush,
    };
  }
}

// Initialize batch manager
const batchManager = new BatchManager();

// Initialize ClickHouse client
async function initClickHouse() {
  try {
    clickhouseClient = createClient({
      url: CLICKHOUSE_URL,
      database: CLICKHOUSE_DATABASE,
      clickhouse_settings: {
        async_insert: 1,
        wait_for_async_insert: 0,
      },
      request_timeout: 30000,
      max_open_connections: 10,
    });

    // Test connection
    await clickhouseClient.ping();
    console.log(
      `✅ ClickHouse connected: ${CLICKHOUSE_URL}/${CLICKHOUSE_DATABASE}`
    );

    return true;
  } catch (error) {
    console.error("❌ ClickHouse connection failed:", error.message);
    return false;
  }
}

// Middleware - Handle gzip decompression using streaming
app.use((req, res, next) => {
  if (req.headers["content-encoding"] === "gzip") {
    console.log("🗜️  Decompressing gzip request");

    let rawData = [];

    // Collect raw data first for debugging
    req.on("data", (chunk) => {
      rawData.push(chunk);
    });

    req.on("end", () => {
      try {
        const buffer = Buffer.concat(rawData);
        const decompressed = zlib.gunzipSync(buffer);
        const jsonString = decompressed.toString();
        req.body = JSON.parse(jsonString);
        req._gzipProcessed = true;
        next();
      } catch (error) {
        console.error("❌ Gzip decompression failed:", error.message);
        res
          .status(400)
          .json({ error: "Invalid gzip data", details: error.message });
      }
    });

    req.on("error", (error) => {
      console.error("❌ Request error:", error.message);
      res.status(400).json({ error: "Request error", details: error.message });
    });
  } else {
    next();
  }
});

// Only use express.json for non-gzip requests
app.use((req, res, next) => {
  if (req._gzipProcessed) {
    next(); // Skip express.json() for gzip requests
  } else {
    express.json({ limit: "100mb" })(req, res, next);
  }
});

app.use(express.urlencoded({ extended: true }));

// Request logging middleware
app.use((req, res, next) => {
  const timestamp = new Date().toISOString();
  const contentLength = req.get("content-length") || 0;
  const compression = req.get("content-encoding")
    ? ` (${req.get("content-encoding")})`
    : "";
  console.log(
    `[${timestamp}] ${req.method} ${req.path} - ${contentLength} bytes${compression}`
  );
  next();
});

// Health check endpoint
app.get("/health", (req, res) => {
  const uptime = Math.floor(process.uptime());
  const hours = Math.floor(uptime / 3600);
  const minutes = Math.floor((uptime % 3600) / 60);
  const seconds = uptime % 60;

  res.status(200).json({
    status: "healthy",
    timestamp: new Date().toISOString(),
    uptime: `${hours}h ${minutes}m ${seconds}s`,
    version: "3.0.0-clickhouse",
    features: {
      clickhouse: clickhouseClient ? true : false,
      database: CLICKHOUSE_DATABASE,
      batch_manager: batchManager.getStats(),
    },
  });
});

// Main timing data endpoint
app.post("/ingest", async (req, res) => {
  const requestId = Math.random().toString(36).substr(2, 9);
  const startTime = process.hrtime.bigint();

  // Add timeout handling
  const timeout = setTimeout(() => {
    if (!res.headersSent) {
      console.error(`⏰ [${requestId}] Request timeout after 30 seconds`);
      res.status(408).json({
        request_id: requestId,
        error: "Request timeout",
      });
    }
  }, 30000); // 30 second timeout

  try {
    const data = req.body;
    const contentLength = req.get("content-length") || 0;

    console.log(
      `📥 [${requestId}] Request started: ${data.length} transactions`
    );

    // Validate batch data
    if (!Array.isArray(data)) {
      console.log(`❌ [${requestId}] Invalid data format: expected array`);
      clearTimeout(timeout);
      return res.status(400).json({
        request_id: requestId,
        error: "Expected array of transactions",
      });
    }

    if (data.length === 0) {
      console.log(`⚠️  [${requestId}] Empty batch received`);
      clearTimeout(timeout);
      return res.status(400).json({
        request_id: requestId,
        error: "Empty batch",
      });
    }

    // Process batch statistics
    const batchStats = processBatchStats(data);
    updateGlobalStats(batchStats, contentLength);

    // Log batch info
    console.log(
      `📦 [${requestId}] Batch #${stats.totalBatches}: ${data.length} transactions`
    );
    console.log(
      `   [${requestId}] Slots: ${batchStats.uniqueSlots.size} | Vote: ${batchStats.voteCount} | Non-vote: ${batchStats.nonVoteCount}`
    );
    console.log(
      `   [${requestId}] POH Range: ${batchStats.pohTickMin} → ${batchStats.pohTickMax}`
    );

    // Sample first few transactions for verification
    if (data.length > 0) {
      const sample = data[0];
      console.log(
        `   [${requestId}] Sample: slot=${sample.slot}, tick=${
          sample.poh_tick
        }, entry=${sample.entry_index}, sig=${sample.signature?.substring(
          0,
          12
        )}..., signer=${sample.signer?.substring(0, 8)}...`
      );
    }

    // Add to ClickHouse batch manager (non-blocking)
    if (clickhouseClient) {
      batchManager.addRecords(data, stats.totalBatches);
    } else {
      console.warn(
        `⚠️  [${requestId}] ClickHouse not available, data will be lost`
      );
    }

    // Performance metrics
    const processingTime =
      Number(process.hrtime.bigint() - startTime) / 1_000_000; // Convert to ms

    console.log(
      `✅ [${requestId}] Request completed in ${
        Math.round(processingTime * 100) / 100
      }ms`
    );

    // Fast response
    clearTimeout(timeout);
    res.status(200).json({
      request_id: requestId,
      received: data.length,
      batch_id: stats.totalBatches,
      processing_time_ms: Math.round(processingTime * 100) / 100,
      clickhouse_pending: batchManager.getStats().pendingRecords,
      timestamp: new Date().toISOString(),
    });

    // Progress logging every 10 batches
    if (stats.totalBatches % 10 === 0) {
      logProgress();
    }
  } catch (error) {
    const processingTime =
      Number(process.hrtime.bigint() - startTime) / 1_000_000;
    console.error(
      `❌ [${requestId}] Request failed after ${
        Math.round(processingTime * 100) / 100
      }ms:`,
      error.message
    );
    clearTimeout(timeout);

    if (!res.headersSent) {
      res.status(500).json({
        request_id: requestId,
        error: "Processing failed",
        details: error.message,
      });
    }
  }
});

// Process batch statistics
function processBatchStats(data) {
  const uniqueSlots = new Set();
  let voteCount = 0;
  let nonVoteCount = 0;
  let pohTickMin = Infinity;
  let pohTickMax = -Infinity;
  let totalAccounts = 0;

  data.forEach((tx) => {
    uniqueSlots.add(tx.slot);

    if (tx.is_vote) {
      voteCount++;
    } else {
      nonVoteCount++;
    }

    pohTickMin = Math.min(pohTickMin, tx.poh_tick);
    pohTickMax = Math.max(pohTickMax, tx.poh_tick);

    totalAccounts +=
      (tx.accounts_read?.length || 0) + (tx.accounts_written?.length || 0);
  });

  return {
    uniqueSlots,
    voteCount,
    nonVoteCount,
    pohTickMin: pohTickMin === Infinity ? 0 : pohTickMin,
    pohTickMax: pohTickMax === -Infinity ? 0 : pohTickMax,
    totalAccounts,
    batchSize: data.length,
  };
}

// Update global statistics
function updateGlobalStats(batchStats, contentLength) {
  stats.totalBatches++;
  stats.totalTransactions += batchStats.batchSize;
  stats.totalBytes += parseInt(contentLength);
  stats.lastBatchTime = new Date();
  stats.voteTransactions += batchStats.voteCount;
  stats.nonVoteTransactions += batchStats.nonVoteCount;

  // Add new slots to global set
  batchStats.uniqueSlots.forEach((slot) => stats.slotsProcessed.add(slot));
}

// Progress logging
function logProgress() {
  const runtime = Math.floor((new Date() - stats.startTime) / 1000);
  const avgBatchSize = Math.round(stats.totalTransactions / stats.totalBatches);
  const avgTPS = Math.round(stats.totalTransactions / runtime);
  const dataSizeGB = (stats.totalBytes / (1024 * 1024 * 1024)).toFixed(2);
  const batchStats = batchManager.getStats();

  console.log("\n📊 === PROGRESS REPORT ===");
  console.log(`⏱️  Runtime: ${Math.floor(runtime / 60)}m ${runtime % 60}s`);
  console.log(
    `📦 Batches: ${stats.totalBatches} | Avg size: ${avgBatchSize} txs`
  );
  console.log(
    `📈 Transactions: ${stats.totalTransactions.toLocaleString()} | Avg TPS: ${avgTPS}`
  );
  console.log(
    `🗳️  Vote: ${stats.voteTransactions.toLocaleString()} | Non-vote: ${stats.nonVoteTransactions.toLocaleString()}`
  );
  console.log(`🎰 Slots: ${stats.slotsProcessed.size} unique slots processed`);
  console.log(`💽 Data: ${dataSizeGB} GB received`);
  console.log(
    `🏛️  ClickHouse: ${stats.clickhouseInserts} inserts, ${stats.clickhouseErrors} errors`
  );
  console.log(`📤 Pending: ${batchStats.pendingRecords} records in buffer`);
  console.log("========================\n");
}

// Metrics endpoint for monitoring
app.get("/metrics", (req, res) => {
  const runtime = Math.floor((new Date() - stats.startTime) / 1000);
  const avgTPS =
    runtime > 0 ? Math.round(stats.totalTransactions / runtime) : 0;
  const batchStats = batchManager.getStats();

  res.status(200).json({
    system: {
      nodejs_version: process.version,
      memory_usage: process.memoryUsage(),
      cpu_usage: process.cpuUsage(),
      uptime_seconds: Math.floor(process.uptime()),
    },
    timing_stats: {
      total_transactions: stats.totalTransactions,
      total_batches: stats.totalBatches,
      vote_transactions: stats.voteTransactions,
      non_vote_transactions: stats.nonVoteTransactions,
      unique_slots: stats.slotsProcessed.size,
      total_bytes: stats.totalBytes,
      average_tps: avgTPS,
      last_batch: stats.lastBatchTime,
      runtime_seconds: runtime,
    },
    clickhouse: {
      connected: clickhouseClient ? true : false,
      database: CLICKHOUSE_DATABASE,
      successful_inserts: stats.clickhouseInserts,
      failed_inserts: stats.clickhouseErrors,
      pending_records: batchStats.pendingRecords,
      is_processing: batchStats.isProcessing,
    },
    timestamp: new Date().toISOString(),
  });
});

// ClickHouse query endpoint for analysis
app.get("/query/:type", async (req, res) => {
  if (!clickhouseClient) {
    return res.status(503).json({ error: "ClickHouse not available" });
  }

  const queryType = req.params.type || "summary";

  try {
    let result;

    switch (queryType) {
      case "summary":
        result = await clickhouseClient.query({
          query: `
            SELECT 
              count() as total_records,
              uniq(slot) as unique_slots,
              countIf(is_vote = 1) as vote_count,
              countIf(is_vote = 0) as non_vote_count,
              avg(accounts_read_count) as avg_accounts_read,
              avg(accounts_written_count) as avg_accounts_written,
              min(timestamp) as earliest_record,
              max(timestamp) as latest_record
            FROM timing_data
            WHERE timestamp > now() - INTERVAL 1 HOUR
          `,
          format: "JSONEachRow",
        });
        break;

      case "slots":
        result = await clickhouseClient.query({
          query: `
            SELECT 
              slot,
              count() as tx_count,
              countIf(is_vote = 1) as vote_count,
              countIf(is_vote = 0) as non_vote_count,
              max(poh_tick) - min(poh_tick) as poh_tick_range
            FROM timing_data
            WHERE timestamp > now() - INTERVAL 1 HOUR
            GROUP BY slot
            ORDER BY slot DESC
            LIMIT 20
          `,
          format: "JSONEachRow",
        });
        break;

      case "hourly":
        result = await clickhouseClient.query({
          query: `
            SELECT * FROM timing_hourly_agg
            WHERE hour > now() - INTERVAL 24 HOUR
            ORDER BY hour DESC
          `,
          format: "JSONEachRow",
        });
        break;

      case "signers":
        result = await clickhouseClient.query({
          query: `
            SELECT 
              signer,
              count() as tx_count,
              countIf(is_vote = 1) as vote_count,
              countIf(is_vote = 0) as non_vote_count,
              uniq(slot) as unique_slots,
              min(timestamp) as first_seen,
              max(timestamp) as last_seen
            FROM timing_data
            WHERE timestamp > now() - INTERVAL 1 HOUR
            GROUP BY signer
            ORDER BY tx_count DESC
            LIMIT 50
          `,
          format: "JSONEachRow",
        });
        break;

      default:
        return res.status(400).json({
          error: "Invalid query type",
          available: ["summary", "slots", "hourly", "signers"],
        });
    }

    const data = await result.json();
    res.json({
      query_type: queryType,
      timestamp: new Date().toISOString(),
      data: data,
    });
  } catch (error) {
    console.error("❌ ClickHouse query failed:", error.message);
    res.status(500).json({
      error: "Query failed",
      details: error.message,
    });
  }
});

// Alternative endpoint for testing
app.post("/timing", (req, res) => {
  console.log("📈 Received timing data on /timing endpoint");
  res.status(200).json({ status: "received", endpoint: "/timing" });
});

// Error handling middleware
app.use((err, req, res, next) => {
  console.error("❌ Unhandled error:", err.message);
  res.status(500).json({
    error: "Internal server error",
    details: err.message,
  });
});

// 404 handler
app.use((req, res) => {
  console.log(`❓ 404: ${req.method} ${req.path}`);
  res.status(404).json({
    error: "Endpoint not found",
    available_endpoints: ["/ingest", "/health", "/metrics", "/query"],
  });
});

// Graceful shutdown
async function gracefulShutdown() {
  console.log("\n🛑 Shutting down timing server...");
  logProgress();

  // Force flush any pending records
  if (batchManager) {
    console.log("🔄 Flushing pending ClickHouse records...");
    await batchManager.forceFlush(stats.totalBatches);
  }

  // Close ClickHouse connection
  if (clickhouseClient) {
    await clickhouseClient.close();
    console.log("✅ ClickHouse connection closed");
  }

  console.log("👋 Goodbye!");
  process.exit(0);
}

process.on("SIGINT", gracefulShutdown);
process.on("SIGTERM", gracefulShutdown);

// Start server
async function startServer() {
  console.log("\n🚀 Agave Timing Export Server v3.0 (ClickHouse Edition)");

  // Initialize ClickHouse
  const clickhouseConnected = await initClickHouse();
  if (!clickhouseConnected) {
    console.warn("⚠️  Starting without ClickHouse - data will be lost!");
  }

  app.listen(PORT, "0.0.0.0", () => {
    console.log(`📍 Listening on: http://localhost:${PORT}`);
    console.log(`📊 Main endpoint: http://localhost:${PORT}/ingest`);
    console.log(`🏥 Health check: http://localhost:${PORT}/health`);
    console.log(`📈 Metrics: http://localhost:${PORT}/metrics`);
    console.log(
      `🔍 Query API: http://localhost:${PORT}/query/{summary|slots|hourly|signers}`
    );
    console.log(`🏛️  ClickHouse: ${CLICKHOUSE_URL}/${CLICKHOUSE_DATABASE}`);
    console.log(`📤 Batch size: ${batchManager.maxBatchSize} records`);
    console.log(`⏰ Auto-flush: ${batchManager.flushTimeoutMs}ms`);
    console.log(
      "✅ Ready to receive 14k transaction batches from Agave validator\n"
    );
  });
}

startServer().catch(console.error);
