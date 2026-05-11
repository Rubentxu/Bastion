//! HeartbeatBridge — bridges worker heartbeat data to MetricsHub.
//!
//! Receives per-sandbox resource usage (CPU, memory, disk, load) from
//! worker heartbeat messages and exposes it for MCP tool queries.

use std::sync::Arc;
use std::sync::RwLock;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

/// Per-sandbox resource snapshot received from worker heartbeats.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerResources {
    /// Sandbox ID.
    pub sandbox_id: String,
    /// CPU usage as percentage (0-100).
    pub cpu_percent: f64,
    /// Memory used in MB.
    pub mem_used_mb: f64,
    /// Memory limit in MB.
    pub mem_limit_mb: f64,
    /// Disk used in MB.
    pub disk_used_mb: f64,
    /// 1-minute load average.
    pub loadavg_1m: f64,
    /// Uptime in seconds.
    pub uptime_seconds: u64,
    /// Timestamp of the last heartbeat (epoch seconds).
    pub last_heartbeat_epoch: i64,
}

/// Aggregate system-wide resource snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemResources {
    /// Total CPU usage across all sandboxes (percentage, 0-100).
    pub total_cpu_percent: f64,
    /// Total memory used in MB across all sandboxes.
    pub total_mem_used_mb: f64,
    /// Total memory limit in MB across all sandboxes.
    pub total_mem_limit_mb: f64,
    /// Total disk used in MB across all sandboxes.
    pub total_disk_used_mb: f64,
    /// Number of active sandboxes reporting heartbeats.
    pub active_sandboxes: usize,
    /// Average 1-minute load average across sandboxes.
    pub avg_loadavg_1m: f64,
    /// Timestamp of the most recent heartbeat (epoch seconds).
    pub last_heartbeat_epoch: i64,
}

/// HeartbeatBridge — receives and indexes per-sandbox resource data from workers.
///
/// This is a concurrency-safe structure that allows:
/// - Worker threads to push heartbeat data (via `update_resources`)
/// - MCP tool handlers to query per-sandbox or system-wide data (via `get_resources`, `get_system_resources`)
pub struct HeartbeatBridge {
    /// Per-sandbox resource snapshots, keyed by sandbox_id.
    resources: Arc<DashMap<String, WorkerResources>>,
    /// Stale threshold in seconds — resources older than this are pruned on query.
    stale_threshold_secs: RwLock<u64>,
}

impl HeartbeatBridge {
    /// Create a new HeartbeatBridge with default stale threshold (30 seconds).
    pub fn new() -> Self {
        Self {
            resources: Arc::new(DashMap::new()),
            stale_threshold_secs: RwLock::new(30),
        }
    }

    /// Create a HeartbeatBridge with a custom stale threshold.
    pub fn with_stale_threshold(stale_threshold_secs: u64) -> Self {
        Self {
            resources: Arc::new(DashMap::new()),
            stale_threshold_secs: RwLock::new(stale_threshold_secs),
        }
    }

    /// Update or insert resource data for a sandbox.
    ///
    /// Called by worker heartbeat handlers each time a heartbeat arrives.
    pub fn update_resources(&self, snapshot: WorkerResources) {
        let sandbox_id = snapshot.sandbox_id.clone();
        self.resources.insert(sandbox_id, snapshot);
    }

    /// Remove a sandbox from tracking (e.g., on termination).
    pub fn remove_resources(&self, sandbox_id: &str) -> bool {
        self.resources.remove(sandbox_id).is_some()
    }

    /// Get a snapshot of resources for a specific sandbox.
    ///
    /// Returns `None` if the sandbox is not tracked or data is stale.
    pub fn get_resources(&self, sandbox_id: &str) -> Option<WorkerResources> {
        self.resources.get(sandbox_id).map(|r| r.value().clone())
    }

    /// Get all currently tracked sandbox IDs.
    pub fn tracked_sandbox_ids(&self) -> Vec<String> {
        self.resources.iter().map(|e| e.key().clone()).collect()
    }

    /// Get aggregate system-wide resources.
    ///
    /// Sums resource usage across all active sandboxes and computes averages.
    pub fn get_system_resources(&self) -> SystemResources {
        let entries: Vec<WorkerResources> =
            self.resources.iter().map(|e| e.value().clone()).collect();

        if entries.is_empty() {
            return SystemResources::default();
        }

        let total_cpu: f64 = entries.iter().map(|e| e.cpu_percent).sum();
        let total_mem_used: f64 = entries.iter().map(|e| e.mem_used_mb).sum();
        let total_mem_limit: f64 = entries.iter().map(|e| e.mem_limit_mb).sum();
        let total_disk: f64 = entries.iter().map(|e| e.disk_used_mb).sum();
        let avg_load: f64 =
            entries.iter().map(|e| e.loadavg_1m).sum::<f64>() / entries.len() as f64;
        let last_epoch = entries
            .iter()
            .map(|e| e.last_heartbeat_epoch)
            .max()
            .unwrap_or(0);

        SystemResources {
            total_cpu_percent: total_cpu,
            total_mem_used_mb: total_mem_used,
            total_mem_limit_mb: total_mem_limit,
            total_disk_used_mb: total_disk,
            active_sandboxes: entries.len(),
            avg_loadavg_1m: avg_load,
            last_heartbeat_epoch: last_epoch,
        }
    }

    /// Prune stale sandbox entries that haven't reported within the threshold.
    ///
    /// Returns the number of pruned entries.
    pub fn prune_stale(&self) -> usize {
        let threshold = *self.stale_threshold_secs.read().unwrap();
        let now_epoch = chrono::Utc::now().timestamp();
        let cutoff = now_epoch - threshold as i64;

        let stale_ids: Vec<String> = self
            .resources
            .iter()
            .filter(|e| e.value().last_heartbeat_epoch < cutoff)
            .map(|e| e.key().clone())
            .collect();

        let pruned = stale_ids.len();
        for id in stale_ids {
            self.resources.remove(&id);
        }
        pruned
    }

    /// Set the stale threshold in seconds.
    pub fn set_stale_threshold(&self, secs: u64) {
        *self.stale_threshold_secs.write().unwrap() = secs;
    }

    /// Get the current stale threshold in seconds.
    #[allow(dead_code)]
    pub fn stale_threshold(&self) -> u64 {
        *self.stale_threshold_secs.read().unwrap()
    }

    /// Get the count of tracked sandboxes.
    pub fn tracked_count(&self) -> usize {
        self.resources.len()
    }
}

impl Default for HeartbeatBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_resources(sandbox_id: &str, cpu: f64, mem: f64, epoch: i64) -> WorkerResources {
        WorkerResources {
            sandbox_id: sandbox_id.to_string(),
            cpu_percent: cpu,
            mem_used_mb: mem,
            mem_limit_mb: 512.0,
            disk_used_mb: 50.0,
            loadavg_1m: 0.5,
            uptime_seconds: 100,
            last_heartbeat_epoch: epoch,
        }
    }

    #[test]
    fn test_heartbeat_bridge_new() {
        let bridge = HeartbeatBridge::new();
        assert_eq!(bridge.tracked_count(), 0);
    }

    #[test]
    fn test_update_and_get_resources() {
        let bridge = HeartbeatBridge::new();
        let now = chrono::Utc::now().timestamp();

        bridge.update_resources(make_resources("sb-1", 25.0, 128.0, now));
        assert_eq!(bridge.tracked_count(), 1);

        let res = bridge.get_resources("sb-1").unwrap();
        assert!((res.cpu_percent - 25.0).abs() < 0.01);
        assert!((res.mem_used_mb - 128.0).abs() < 0.01);
    }

    #[test]
    fn test_update_overwrites() {
        let bridge = HeartbeatBridge::new();
        let now = chrono::Utc::now().timestamp();

        bridge.update_resources(make_resources("sb-1", 10.0, 64.0, now));
        bridge.update_resources(make_resources("sb-1", 50.0, 256.0, now));

        let res = bridge.get_resources("sb-1").unwrap();
        assert!((res.cpu_percent - 50.0).abs() < 0.01);
        assert!((res.mem_used_mb - 256.0).abs() < 0.01);
    }

    #[test]
    fn test_remove_resources() {
        let bridge = HeartbeatBridge::new();
        let now = chrono::Utc::now().timestamp();

        bridge.update_resources(make_resources("sb-1", 25.0, 128.0, now));
        assert!(bridge.remove_resources("sb-1"));
        assert!(!bridge.remove_resources("sb-1")); // already removed
        assert_eq!(bridge.tracked_count(), 0);
    }

    #[test]
    fn test_system_resources_empty() {
        let bridge = HeartbeatBridge::new();
        let sys = bridge.get_system_resources();
        assert_eq!(sys.active_sandboxes, 0);
        assert_eq!(sys.total_cpu_percent, 0.0);
    }

    #[test]
    fn test_system_resources_aggregation() {
        let bridge = HeartbeatBridge::new();
        let now = chrono::Utc::now().timestamp();

        bridge.update_resources(make_resources("sb-1", 25.0, 128.0, now));
        bridge.update_resources(make_resources("sb-2", 50.0, 256.0, now));

        let sys = bridge.get_system_resources();
        assert_eq!(sys.active_sandboxes, 2);
        assert!((sys.total_cpu_percent - 75.0).abs() < 0.01);
        assert!((sys.total_mem_used_mb - 384.0).abs() < 0.01);
        assert!((sys.total_mem_limit_mb - 1024.0).abs() < 0.01);
        assert!((sys.avg_loadavg_1m - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_prune_stale() {
        let bridge = HeartbeatBridge::with_stale_threshold(10);
        let now = chrono::Utc::now().timestamp();
        let old = now - 60; // 60 seconds ago

        bridge.update_resources(make_resources("sb-old", 10.0, 64.0, old));
        bridge.update_resources(make_resources("sb-fresh", 30.0, 128.0, now));

        let pruned = bridge.prune_stale();
        assert_eq!(pruned, 1);
        assert_eq!(bridge.tracked_count(), 1);
        assert!(bridge.get_resources("sb-fresh").is_some());
        assert!(bridge.get_resources("sb-old").is_none());
    }

    #[test]
    fn test_tracked_sandbox_ids() {
        let bridge = HeartbeatBridge::new();
        let now = chrono::Utc::now().timestamp();

        bridge.update_resources(make_resources("alpha", 10.0, 64.0, now));
        bridge.update_resources(make_resources("beta", 20.0, 128.0, now));

        let ids = bridge.tracked_sandbox_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"alpha".to_string()));
        assert!(ids.contains(&"beta".to_string()));
    }
}
