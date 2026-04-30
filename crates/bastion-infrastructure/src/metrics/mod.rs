//! Gateway metrics collection.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Gateway metrics.
///
/// Tracks sandboxes created/terminated, commands executed, errors, and latency.
#[derive(Debug, Clone)]
pub struct GatewayMetrics {
    /// Total sandboxes created.
    sandboxes_created: Arc<AtomicU64>,
    /// Total sandboxes terminated.
    sandboxes_terminated: Arc<AtomicU64>,
    /// Total commands executed.
    commands_executed: Arc<AtomicU64>,
    /// Total command execution time (microseconds, for calculating average).
    total_command_time_us: Arc<AtomicU64>,
    /// Total errors encountered.
    errors_total: Arc<AtomicU64>,
    /// Current active sandboxes.
    sandboxes_active: Arc<AtomicU64>,
}

impl Default for GatewayMetrics {
    fn default() -> Self {
        Self {
            sandboxes_created: Arc::new(AtomicU64::new(0)),
            sandboxes_terminated: Arc::new(AtomicU64::new(0)),
            commands_executed: Arc::new(AtomicU64::new(0)),
            total_command_time_us: Arc::new(AtomicU64::new(0)),
            errors_total: Arc::new(AtomicU64::new(0)),
            sandboxes_active: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl GatewayMetrics {
    /// Record a sandbox creation.
    pub fn record_sandbox_created(&self) {
        self.sandboxes_created.fetch_add(1, Ordering::Relaxed);
        self.sandboxes_active.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a sandbox termination.
    pub fn record_sandbox_terminated(&self) {
        self.sandboxes_terminated.fetch_add(1, Ordering::Relaxed);
        self.sandboxes_active.fetch_sub(1, Ordering::Relaxed);
    }

    /// Record a command execution with its duration in microseconds.
    pub fn record_command(&self, duration_us: u64) {
        self.commands_executed.fetch_add(1, Ordering::Relaxed);
        self.total_command_time_us
            .fetch_add(duration_us, Ordering::Relaxed);
    }

    /// Record an error occurrence.
    pub fn record_error(&self) {
        self.errors_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Calculate average command latency in microseconds.
    #[allow(dead_code)]
    pub fn avg_command_latency_us(&self) -> u64 {
        let total = self.commands_executed.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        self.total_command_time_us.load(Ordering::Relaxed) / total
    }

    /// Get current active sandboxes count.
    #[allow(dead_code)]
    pub fn active_sandboxes(&self) -> u64 {
        self.sandboxes_active.load(Ordering::Relaxed)
    }

    /// Export metrics in Prometheus text format.
    pub fn prometheus_export(&self) -> String {
        let mut out = String::new();

        out.push_str("# HELP bastion_sandboxes_created_total Total sandboxes created\n");
        out.push_str("# TYPE bastion_sandboxes_created_total counter\n");
        out.push_str(&format!(
            "bastion_sandboxes_created_total {}\n",
            self.sandboxes_created.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP bastion_sandboxes_terminated_total Total sandboxes terminated\n");
        out.push_str("# TYPE bastion_sandboxes_terminated_total counter\n");
        out.push_str(&format!(
            "bastion_sandboxes_terminated_total {}\n",
            self.sandboxes_terminated.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP bastion_commands_executed_total Total commands executed\n");
        out.push_str("# TYPE bastion_commands_executed_total counter\n");
        out.push_str(&format!(
            "bastion_commands_executed_total {}\n",
            self.commands_executed.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP bastion_errors_total Total errors\n");
        out.push_str("# TYPE bastion_errors_total counter\n");
        out.push_str(&format!(
            "bastion_errors_total {}\n",
            self.errors_total.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP bastion_sandboxes_active Current active sandboxes\n");
        out.push_str("# TYPE bastion_sandboxes_active gauge\n");
        out.push_str(&format!(
            "bastion_sandboxes_active {}\n",
            self.sandboxes_active.load(Ordering::Relaxed)
        ));

        out.push_str(
            "# HELP bastion_command_latency_us_avg Average command latency in microseconds\n",
        );
        out.push_str("# TYPE bastion_command_latency_us_avg gauge\n");
        out.push_str(&format!(
            "bastion_command_latency_us_avg {}\n",
            self.avg_command_latency_us()
        ));

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_default() {
        let m = GatewayMetrics::default();
        assert_eq!(m.sandboxes_created.load(Ordering::Relaxed), 0);
        assert_eq!(m.sandboxes_active.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_record_sandbox_created() {
        let m = GatewayMetrics::default();
        m.record_sandbox_created();
        assert_eq!(m.sandboxes_created.load(Ordering::Relaxed), 1);
        assert_eq!(m.sandboxes_active.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_record_sandbox_terminated() {
        let m = GatewayMetrics::default();
        m.record_sandbox_created();
        m.record_sandbox_terminated();
        assert_eq!(m.sandboxes_terminated.load(Ordering::Relaxed), 1);
        assert_eq!(m.sandboxes_active.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_record_command() {
        let m = GatewayMetrics::default();
        m.record_command(1000);
        m.record_command(2000);
        assert_eq!(m.commands_executed.load(Ordering::Relaxed), 2);
        assert_eq!(m.total_command_time_us.load(Ordering::Relaxed), 3000);
        assert_eq!(m.avg_command_latency_us(), 1500);
    }

    #[test]
    fn test_record_error() {
        let m = GatewayMetrics::default();
        m.record_error();
        assert_eq!(m.errors_total.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_prometheus_export() {
        let m = GatewayMetrics::default();
        m.record_sandbox_created();
        m.record_command(500);

        let output = m.prometheus_export();
        assert!(output.contains("bastion_sandboxes_created_total 1"));
        assert!(output.contains("bastion_commands_executed_total 1"));
        assert!(output.contains("bastion_sandboxes_active 1"));
    }
}
