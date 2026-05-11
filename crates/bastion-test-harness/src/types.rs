//! Shared types for MetricsCollector — always available regardless of feature flags.
//!
//! Note: serde derives require `test-metrics` feature to be enabled.

#[cfg(feature = "test-metrics")]
use serde::{Deserialize, Serialize};

#[cfg(not(feature = "test-metrics"))]
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Types — always available
// ─────────────────────────────────────────────────────────────────────────────

/// Errors returned by MetricsCollector methods.
#[cfg_attr(feature = "test-metrics", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub enum MetricsError {
    /// Not enough samples to compute statistics (minimum 3 required).
    InsufficientSamples { have: usize, need: usize },
    /// SQLite database error.
    Database(String),
    /// Regression detection failed.
    RegressionDetect(String),
}

impl std::fmt::Display for MetricsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetricsError::InsufficientSamples { have, need } => {
                write!(
                    f,
                    "Insufficient samples: have {}, need at least {}",
                    have, need
                )
            }
            MetricsError::Database(s) => write!(f, "Database error: {}", s),
            MetricsError::RegressionDetect(s) => write!(f, "Regression detection error: {}", s),
        }
    }
}

impl std::error::Error for MetricsError {}

/// Latency statistics for a single test.
#[cfg_attr(feature = "test-metrics", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct LatencyStats {
    /// 50th percentile latency in milliseconds.
    pub p50_ms: u64,
    /// 95th percentile latency in milliseconds.
    pub p95_ms: u64,
    /// 99th percentile latency in milliseconds.
    pub p99_ms: u64,
    /// Total number of samples.
    pub sample_count: usize,
}

/// Result of a regression detection check for a single test.
#[cfg_attr(feature = "test-metrics", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct RegressionResult {
    /// Name of the test that regressed.
    pub test_name: String,
    /// P95 latency in baseline (ms).
    pub baseline_p95: u64,
    /// P95 latency in current (ms).
    pub current_p95: u64,
    /// Percentage delta from baseline to current (can be negative).
    pub delta_pct: f64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Noop MetricsCollector (used when test-metrics feature is disabled)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(feature = "test-metrics"))]
#[derive(Debug, Clone, Default)]
pub struct MetricsCollector;

#[cfg(not(feature = "test-metrics"))]
impl MetricsCollector {
    /// Create a no-op collector.
    pub fn new(_path: impl AsRef<Path>) -> Result<Self, MetricsError> {
        Ok(Self)
    }

    /// Create a no-op collector (alias for `new`).
    pub fn noop() -> Self {
        Self
    }

    /// No-op: does nothing.
    pub fn record_test(
        &self,
        _test_name: &str,
        _duration_ms: u64,
        _status: &str,
        _crate_name: &str,
        _file_path: &str,
    ) {
    }

    /// Returns an error for noop collector.
    pub fn latency_stats(&self, _test_name: &str) -> Result<LatencyStats, MetricsError> {
        Err(MetricsError::Database(
            "MetricsCollector is disabled".to_string(),
        ))
    }

    /// Returns 0.0 for noop collector.
    pub fn flakiness_score(&self, _test_name: &str) -> Result<f32, MetricsError> {
        Ok(0.0)
    }

    /// Returns empty vec for noop collector.
    pub fn regression_detect(
        _baseline: impl AsRef<Path>,
        _current: impl AsRef<Path>,
    ) -> Result<Vec<RegressionResult>, MetricsError> {
        Ok(Vec::new())
    }
}
