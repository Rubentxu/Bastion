//! Bastion Test Harness — Dev-dependency crate for integration testing infrastructure.
//!
//! This crate provides:
//! - `MetricsCollector`: SQLite-backed per-test metrics with latency stats and flakiness detection
//! - `TestTerminal`: Process spawning harness with health checking and output matching
//!
//! # Feature Flags
//!
//! - `test-metrics`: Enables SQLite-backed `MetricsCollector`. When disabled, all metrics
//!   operations compile to no-ops with zero overhead.

#![cfg_attr(docsrs, feature(doc_cfg))]

// Always-visible types (no feature gate needed)
mod types;

// Feature-gated implementation modules
#[cfg(feature = "test-metrics")]
mod schema;

#[cfg(feature = "test-metrics")]
mod metrics;

#[cfg(feature = "test-metrics")]
pub use metrics::MetricsCollector;
#[cfg(not(feature = "test-metrics"))]
pub use types::{MetricsCollector, LatencyStats, MetricsError, RegressionResult};

// Only expose TestTerminal when test-metrics is enabled (it depends on metrics)
#[cfg(feature = "test-metrics")]
mod terminal;
#[cfg(feature = "test-metrics")]
pub use terminal::{TestTerminal, GatewayHandle};

#[cfg(test)]
mod metrics_test;

/// Prelude for test harness usage.
pub mod prelude {
    pub use crate::MetricsCollector;
    #[cfg(feature = "test-metrics")]
    pub use crate::{TestTerminal, GatewayHandle};
}
