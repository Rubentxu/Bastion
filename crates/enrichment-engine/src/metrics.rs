//! Thread-safe metrics for enrichment pipeline observability.
//!
//! Zero-alloc on hot path (atomics only). Mutex only on histogram vec.
//!
//! # Metrics
//!
//! - `total_success` — Count of successful enrichment runs
//! - `total_failure` — Count of failed enrichment runs
//! - `saturation_drops` — Count of dropped records due to backpressure
//! - `facts_total` — Total count of facts extracted across all runs
//! - `latencies` — Vector of latencies in ms for histogram (Mutex-protected)

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Thread-safe metrics for enrichment pipeline observability.
/// Zero-alloc on hot path (atomics only). Mutex only on histogram vec.
#[derive(Debug)]
pub struct EnrichmentMetrics {
    /// Count of successful enrichment runs.
    total_success: AtomicU64,
    /// Count of failed enrichment runs.
    total_failure: AtomicU64,
    /// Count of dropped records due to backpressure saturation.
    saturation_drops: AtomicU64,
    /// Total count of facts extracted across all runs.
    facts_total: AtomicU64,
    /// Latencies in ms — Mutex only held briefly on record.
    latencies: Mutex<Vec<u64>>,
}

impl EnrichmentMetrics {
    /// Create a new metrics instance.
    pub fn new() -> Self {
        Self {
            total_success: AtomicU64::new(0),
            total_failure: AtomicU64::new(0),
            saturation_drops: AtomicU64::new(0),
            facts_total: AtomicU64::new(0),
            latencies: Mutex::new(Vec::new()),
        }
    }

    /// Record a successful enrichment run.
    pub fn record_success(&self) {
        self.total_success.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a failed enrichment run.
    pub fn record_failure(&self) {
        self.total_failure.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a saturation drop (record dropped due to backpressure).
    pub fn record_saturation_drop(&self) {
        self.saturation_drops.fetch_add(1, Ordering::Relaxed);
    }

    /// Record the number of facts extracted in a run.
    pub fn record_facts(&self, count: u32) {
        self.facts_total.fetch_add(count as u64, Ordering::Relaxed);
    }

    /// Record a latency sample in milliseconds.
    ///
    /// Note: This acquires a Mutex briefly. For high-frequency recording,
    /// consider batching latencies or using a lock-free structure.
    pub fn record_latency(&self, elapsed_ms: u64) {
        let mut latencies = self.latencies.lock().unwrap();
        latencies.push(elapsed_ms);
    }

    /// Take a snapshot of current metrics.
    ///
    /// Returns a clone-safe struct with all counter values and percentile latencies.
    /// Does not modify any counters.
    pub fn snapshot(&self) -> EnrichmentMetricsSnapshot {
        let latencies = self.latencies.lock().unwrap();
        let p50 = percentile(&latencies, 50);
        let p99 = percentile(&latencies, 99);

        EnrichmentMetricsSnapshot {
            total_success: self.total_success.load(Ordering::Relaxed),
            total_failure: self.total_failure.load(Ordering::Relaxed),
            saturation_drops: self.saturation_drops.load(Ordering::Relaxed),
            facts_total: self.facts_total.load(Ordering::Relaxed),
            p50_latency_ms: p50,
            p99_latency_ms: p99,
        }
    }
}

impl Default for EnrichmentMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute percentile from a sorted vec.
/// Returns None if vec is empty.
fn percentile(sorted: &[u64], p: u8) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let len = sorted.len();
    if len == 1 {
        return Some(sorted[0]);
    }
    // Linear interpolation between nearest ranks
    let rank = (p as f64 / 100.0) * (len as f64 - 1.0);
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    if lower == upper {
        Some(sorted[lower])
    } else {
        let weight = rank - lower as f64;
        Some((sorted[lower] as f64 * (1.0 - weight) + sorted[upper] as f64 * weight) as u64)
    }
}

/// Snapshot of enrichment metrics — clone-safe, serializable.
#[derive(Debug, Clone)]
pub struct EnrichmentMetricsSnapshot {
    /// Total successful enrichment runs.
    pub total_success: u64,
    /// Total failed enrichment runs.
    pub total_failure: u64,
    /// Total saturation drops (backpressure).
    pub saturation_drops: u64,
    /// Total facts extracted.
    pub facts_total: u64,
    /// 50th percentile latency in ms.
    pub p50_latency_ms: Option<u64>,
    /// 99th percentile latency in ms.
    pub p99_latency_ms: Option<u64>,
}

impl EnrichmentMetricsSnapshot {
    /// Create a snapshot representing zero state.
    pub fn zero() -> Self {
        Self {
            total_success: 0,
            total_failure: 0,
            saturation_drops: 0,
            facts_total: 0,
            p50_latency_ms: None,
            p99_latency_ms: None,
        }
    }
}

/// Health snapshot for the enrichment adapter.
/// Provides operational status without exposing internal details.
#[derive(Debug, Clone)]
pub struct EnrichmentHealth {
    /// Whether enrichment is enabled.
    pub enabled: bool,
    /// Number of enrichers in the catalog.
    pub catalog_enricher_count: usize,
    /// Number of runs recorded in the last 5 minutes (approximation).
    pub recent_runs_5min: u64,
    /// Number of saturation drop events recorded.
    pub saturation_events: u64,
    /// Current row count in the database, if recorder is available.
    pub db_row_count: Option<u64>,
    /// Whether a recorder is configured.
    pub recorder_available: bool,
}

impl EnrichmentHealth {
    /// Create a health snapshot for a disabled adapter.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            catalog_enricher_count: 0,
            recent_runs_5min: 0,
            saturation_events: 0,
            db_row_count: None,
            recorder_available: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_new_is_zero() {
        let metrics = EnrichmentMetrics::new();
        let snap = metrics.snapshot();
        assert_eq!(snap.total_success, 0);
        assert_eq!(snap.total_failure, 0);
        assert_eq!(snap.saturation_drops, 0);
        assert_eq!(snap.facts_total, 0);
        assert_eq!(snap.p50_latency_ms, None);
        assert_eq!(snap.p99_latency_ms, None);
    }

    #[test]
    fn test_record_success_increments() {
        let metrics = EnrichmentMetrics::new();
        metrics.record_success();
        metrics.record_success();
        let snap = metrics.snapshot();
        assert_eq!(snap.total_success, 2);
    }

    #[test]
    fn test_record_failure_increments() {
        let metrics = EnrichmentMetrics::new();
        metrics.record_failure();
        let snap = metrics.snapshot();
        assert_eq!(snap.total_failure, 1);
    }

    #[test]
    fn test_record_saturation_drop_increments() {
        let metrics = EnrichmentMetrics::new();
        metrics.record_saturation_drop();
        metrics.record_saturation_drop();
        metrics.record_saturation_drop();
        let snap = metrics.snapshot();
        assert_eq!(snap.saturation_drops, 3);
    }

    #[test]
    fn test_record_facts_accumulates() {
        let metrics = EnrichmentMetrics::new();
        metrics.record_facts(5);
        metrics.record_facts(3);
        let snap = metrics.snapshot();
        assert_eq!(snap.facts_total, 8);
    }

    #[test]
    fn test_record_latency_empty_returns_none() {
        let metrics = EnrichmentMetrics::new();
        let snap = metrics.snapshot();
        assert_eq!(snap.p50_latency_ms, None);
        assert_eq!(snap.p99_latency_ms, None);
    }

    #[test]
    fn test_record_latency_single_sample() {
        let metrics = EnrichmentMetrics::new();
        metrics.record_latency(100);
        let snap = metrics.snapshot();
        assert_eq!(snap.p50_latency_ms, Some(100));
        assert_eq!(snap.p99_latency_ms, Some(100));
    }

    #[test]
    fn test_record_latency_p50() {
        let metrics = EnrichmentMetrics::new();
        // 10 samples: 10, 20, 30, 40, 50, 60, 70, 80, 90, 100
        for i in 1..=10 {
            metrics.record_latency(i * 10);
        }
        let snap = metrics.snapshot();
        // p50 of 10 samples (index 4.5): average of 5th and 6th = 50 and 60 = 55
        assert_eq!(snap.p50_latency_ms, Some(55));
    }

    #[test]
    fn test_record_latency_p99() {
        let metrics = EnrichmentMetrics::new();
        // 100 samples: 1, 2, ..., 100
        for i in 1..=100 {
            metrics.record_latency(i);
        }
        let snap = metrics.snapshot();
        // p99 of 100 samples (index 98.01): roughly 99th element
        assert_eq!(snap.p99_latency_ms, Some(99));
    }

    #[test]
    fn test_concurrent_record_success() {
        use std::sync::Arc;
        let metrics = Arc::new(EnrichmentMetrics::new());
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let m = metrics.clone();
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        m.record_success();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let snap = metrics.snapshot();
        assert_eq!(snap.total_success, 1000);
    }

    #[test]
    fn test_snapshot_is_clone() {
        let metrics = EnrichmentMetrics::new();
        metrics.record_success();
        let snap = metrics.snapshot();
        let snap2 = snap.clone();
        assert_eq!(snap.total_success, snap2.total_success);
    }

    #[test]
    fn test_snapshot_zero() {
        let snap = EnrichmentMetricsSnapshot::zero();
        assert_eq!(snap.total_success, 0);
        assert_eq!(snap.total_failure, 0);
    }

    #[test]
    fn test_percentile_empty() {
        let result = percentile(&[], 50);
        assert_eq!(result, None);
    }

    #[test]
    fn test_percentile_single() {
        let result = percentile(&[42], 50);
        assert_eq!(result, Some(42));
    }

    #[test]
    fn test_percentile_even_length() {
        // [1, 2, 3, 4], p50 should be (2+3)/2 = 2.5
        let result = percentile(&[1, 2, 3, 4], 50);
        assert_eq!(result, Some(2));
    }
}
