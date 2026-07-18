//! TPS Benchmarking Module for Rinku Node
//!
//! Provides tools for measuring transaction throughput, latency, and performance
//! under various load conditions. Targets: 1,000-10,000+ TPS at mainnet scale.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Benchmark configuration
#[derive(Debug, Clone)]
pub struct BenchmarkConfig {
    /// Number of transactions to submit
    pub tx_count: u64,
    /// Number of concurrent workers
    pub workers: usize,
    /// Whether to use batch API
    pub use_batch: bool,
    /// Batch size (only used when use_batch is true)
    pub batch_size: usize,
    /// Warmup transactions (not counted in metrics)
    pub warmup_count: u64,
    /// Target TPS (0 = unlimited)
    pub target_tps: u64,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            tx_count: 10000,
            workers: 4,
            use_batch: true,
            batch_size: 100,
            warmup_count: 100,
            target_tps: 0,
        }
    }
}

/// Benchmark results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResults {
    /// Total transactions processed
    pub total_txs: u64,
    /// Total duration in milliseconds
    pub duration_ms: u64,
    /// Transactions per second
    pub tps: f64,
    /// Average latency in microseconds
    pub avg_latency_us: f64,
    /// P50 latency in microseconds
    pub p50_latency_us: f64,
    /// P95 latency in microseconds
    pub p95_latency_us: f64,
    /// P99 latency in microseconds
    pub p99_latency_us: f64,
    /// Number of successful transactions
    pub success_count: u64,
    /// Number of failed transactions
    pub error_count: u64,
    /// Configuration used
    pub config: BenchmarkConfigSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkConfigSummary {
    pub tx_count: u64,
    pub workers: usize,
    pub use_batch: bool,
    pub batch_size: usize,
}

/// Latency tracker for collecting timing metrics
#[derive(Default)]
pub struct LatencyTracker {
    latencies: std::sync::Mutex<Vec<u64>>,
}

impl LatencyTracker {
    pub fn new() -> Self {
        Self {
            latencies: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn record(&self, latency_us: u64) {
        self.latencies.lock().unwrap().push(latency_us);
    }

    pub fn compute_stats(&self) -> LatencyStats {
        let mut latencies = self.latencies.lock().unwrap().clone();
        if latencies.is_empty() {
            return LatencyStats::default();
        }

        latencies.sort_unstable();
        let len = latencies.len();

        let sum: u64 = latencies.iter().sum();
        let avg = sum as f64 / len as f64;

        // Safe percentile calculation that works for any sample size >= 1
        let p50 = latencies[len / 2];
        let p95_idx = ((len as u64 * 95) / 100) as usize;
        let p99_idx = ((len as u64 * 99) / 100) as usize;
        let p95 = latencies[p95_idx.min(len - 1)];
        let p99 = latencies[p99_idx.min(len - 1)];
        let min = latencies[0];
        let max = latencies[len - 1];

        LatencyStats {
            avg,
            p50,
            p95,
            p99,
            min,
            max,
            count: len as u64,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct LatencyStats {
    pub avg: f64,
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
    pub min: u64,
    pub max: u64,
    pub count: u64,
}

/// Throughput meter for tracking TPS in real-time
pub struct ThroughputMeter {
    start_time: Instant,
    tx_count: AtomicU64,
    last_report_time: std::sync::Mutex<Instant>,
    last_report_count: AtomicU64,
}

impl ThroughputMeter {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            start_time: now,
            tx_count: AtomicU64::new(0),
            last_report_time: std::sync::Mutex::new(now),
            last_report_count: AtomicU64::new(0),
        }
    }

    pub fn record_tx(&self, count: u64) {
        self.tx_count.fetch_add(count, Ordering::Relaxed);
    }

    pub fn total_count(&self) -> u64 {
        self.tx_count.load(Ordering::Relaxed)
    }

    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    pub fn overall_tps(&self) -> f64 {
        let elapsed = self.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            self.total_count() as f64 / elapsed
        } else {
            0.0
        }
    }

    pub fn instant_tps(&self) -> f64 {
        let mut last_time = self.last_report_time.lock().unwrap();
        let now = Instant::now();
        let elapsed = now.duration_since(*last_time).as_secs_f64();

        let current = self.tx_count.load(Ordering::Relaxed);
        let last = self.last_report_count.load(Ordering::Relaxed);

        if elapsed > 0.0 {
            let tps = (current - last) as f64 / elapsed;
            *last_time = now;
            self.last_report_count.store(current, Ordering::Relaxed);
            tps
        } else {
            0.0
        }
    }
}

impl Default for ThroughputMeter {
    fn default() -> Self {
        Self::new()
    }
}

/// Transaction generator for benchmarking
pub struct TxGenerator {
    counter: AtomicU64,
    from_address: String,
    to_address: String,
}

impl TxGenerator {
    pub fn new(from: &str, to: &str) -> Self {
        Self {
            counter: AtomicU64::new(0),
            from_address: from.to_string(),
            to_address: to.to_string(),
        }
    }

    pub fn next_tx(&self) -> TestTransaction {
        let nonce = self.counter.fetch_add(1, Ordering::Relaxed);
        TestTransaction {
            from: self.from_address.clone(),
            to: self.to_address.clone(),
            amount: 100_000,
            nonce,
            gas_limit: 21000,
        }
    }

    pub fn next_batch(&self, size: usize) -> Vec<TestTransaction> {
        (0..size).map(|_| self.next_tx()).collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TestTransaction {
    pub from: String,
    pub to: String,
    pub amount: u64,
    pub nonce: u64,
    pub gas_limit: u64,
}

/// Benchmark runner
pub struct BenchmarkRunner {
    config: BenchmarkConfig,
    latency_tracker: Arc<LatencyTracker>,
    throughput_meter: Arc<ThroughputMeter>,
    success_count: AtomicU64,
    error_count: AtomicU64,
}

impl BenchmarkRunner {
    pub fn new(config: BenchmarkConfig) -> Self {
        Self {
            config,
            latency_tracker: Arc::new(LatencyTracker::new()),
            throughput_meter: Arc::new(ThroughputMeter::new()),
            success_count: AtomicU64::new(0),
            error_count: AtomicU64::new(0),
        }
    }

    /// Record a successful transaction with latency
    pub fn record_success(&self, latency_us: u64) {
        self.latency_tracker.record(latency_us);
        self.throughput_meter.record_tx(1);
        self.success_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a batch of successful transactions
    pub fn record_batch_success(&self, count: u64, total_latency_us: u64) {
        let avg_latency = total_latency_us / count.max(1);
        for _ in 0..count {
            self.latency_tracker.record(avg_latency);
        }
        self.throughput_meter.record_tx(count);
        self.success_count.fetch_add(count, Ordering::Relaxed);
    }

    /// Record a failed transaction
    pub fn record_error(&self) {
        self.error_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get current TPS
    pub fn current_tps(&self) -> f64 {
        self.throughput_meter.overall_tps()
    }

    /// Get instant TPS (since last call)
    pub fn instant_tps(&self) -> f64 {
        self.throughput_meter.instant_tps()
    }

    /// Get final results
    pub fn results(&self) -> BenchmarkResults {
        let latency_stats = self.latency_tracker.compute_stats();
        let elapsed = self.throughput_meter.elapsed();
        let total = self.throughput_meter.total_count();

        BenchmarkResults {
            total_txs: total,
            duration_ms: elapsed.as_millis() as u64,
            tps: if elapsed.as_secs_f64() > 0.0 {
                total as f64 / elapsed.as_secs_f64()
            } else {
                0.0
            },
            avg_latency_us: latency_stats.avg,
            p50_latency_us: latency_stats.p50 as f64,
            p95_latency_us: latency_stats.p95 as f64,
            p99_latency_us: latency_stats.p99 as f64,
            success_count: self.success_count.load(Ordering::Relaxed),
            error_count: self.error_count.load(Ordering::Relaxed),
            config: BenchmarkConfigSummary {
                tx_count: self.config.tx_count,
                workers: self.config.workers,
                use_batch: self.config.use_batch,
                batch_size: self.config.batch_size,
            },
        }
    }

    /// Print progress update
    pub fn print_progress(&self, prefix: &str) {
        let total = self.throughput_meter.total_count();
        let tps = self.current_tps();
        let success = self.success_count.load(Ordering::Relaxed);
        let errors = self.error_count.load(Ordering::Relaxed);

        println!(
            "{} Progress: {} txs, {:.0} TPS, {} success, {} errors",
            prefix, total, tps, success, errors
        );
    }
}

/// Simple in-memory benchmark for testing transaction validation speed
pub fn benchmark_validation_speed(iterations: u64) -> BenchmarkResults {
    use sha2::{Digest, Sha256};

    let config = BenchmarkConfig {
        tx_count: iterations,
        workers: 1,
        use_batch: false,
        batch_size: 1,
        warmup_count: 0,
        target_tps: 0,
    };

    let runner = BenchmarkRunner::new(config);

    for _ in 0..iterations {
        let start = Instant::now();

        // Simulate transaction validation:
        // 1. Hash computation
        let mut hasher = Sha256::new();
        hasher.update(b"test transaction data");
        let _hash = hasher.finalize();

        // 2. Signature verification (simulated with hash)
        let mut hasher2 = Sha256::new();
        hasher2.update(&_hash);
        let _sig_check = hasher2.finalize();

        let elapsed = start.elapsed();
        runner.record_success(elapsed.as_micros() as u64);
    }

    runner.results()
}

/// Benchmark merkle tree operations
pub fn benchmark_merkle_operations(leaf_count: usize, iterations: u64) -> BenchmarkResults {
    use sha2::{Digest, Sha256};

    let config = BenchmarkConfig {
        tx_count: iterations,
        workers: 1,
        use_batch: false,
        batch_size: 1,
        warmup_count: 0,
        target_tps: 0,
    };

    let runner = BenchmarkRunner::new(config);

    // Generate test leaves
    let leaves: Vec<Vec<u8>> = (0..leaf_count)
        .map(|i| {
            let mut hasher = Sha256::new();
            hasher.update(format!("leaf_{}", i).as_bytes());
            hasher.finalize().to_vec()
        })
        .collect();

    for _ in 0..iterations {
        let start = Instant::now();

        // Build merkle tree
        let mut current_level = leaves.clone();
        while current_level.len() > 1 {
            let mut next_level = Vec::new();
            for chunk in current_level.chunks(2) {
                let mut hasher = Sha256::new();
                hasher.update(&chunk[0]);
                if chunk.len() > 1 {
                    hasher.update(&chunk[1]);
                } else {
                    hasher.update(&chunk[0]);
                }
                next_level.push(hasher.finalize().to_vec());
            }
            current_level = next_level;
        }

        let elapsed = start.elapsed();
        runner.record_success(elapsed.as_micros() as u64);
    }

    runner.results()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latency_tracker() {
        let tracker = LatencyTracker::new();

        for i in 1..=100 {
            tracker.record(i);
        }

        let stats = tracker.compute_stats();
        assert_eq!(stats.count, 100);
        assert!(stats.avg > 0.0);
        assert!(stats.p50 > 0);
        assert!(stats.p95 > stats.p50);
        assert!(stats.p99 >= stats.p95);
    }

    #[test]
    fn test_throughput_meter() {
        let meter = ThroughputMeter::new();

        meter.record_tx(100);
        meter.record_tx(200);

        assert_eq!(meter.total_count(), 300);
        assert!(meter.overall_tps() > 0.0);
    }

    #[test]
    fn test_tx_generator() {
        let gen = TxGenerator::new("from_addr", "to_addr");

        let tx1 = gen.next_tx();
        let tx2 = gen.next_tx();

        assert_eq!(tx1.nonce, 0);
        assert_eq!(tx2.nonce, 1);
        assert_eq!(tx1.from, "from_addr");
    }

    #[test]
    fn test_benchmark_runner() {
        let config = BenchmarkConfig {
            tx_count: 100,
            workers: 2,
            ..Default::default()
        };

        let runner = BenchmarkRunner::new(config);

        for i in 0..100 {
            runner.record_success((i + 1) * 10);
        }

        let results = runner.results();
        assert_eq!(results.success_count, 100);
        assert!(results.tps > 0.0);
    }

    #[test]
    fn test_validation_benchmark() {
        let results = benchmark_validation_speed(1000);

        assert_eq!(results.total_txs, 1000);
        assert!(results.tps > 0.0);
        println!(
            "Validation benchmark: {:.0} TPS, avg latency: {:.2} us",
            results.tps, results.avg_latency_us
        );
    }

    #[test]
    fn test_merkle_benchmark() {
        let results = benchmark_merkle_operations(100, 100);

        assert_eq!(results.total_txs, 100);
        assert!(results.tps > 0.0);
        println!(
            "Merkle benchmark (100 leaves): {:.0} TPS, avg latency: {:.2} us",
            results.tps, results.avg_latency_us
        );
    }
}
