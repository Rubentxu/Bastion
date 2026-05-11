//! MetricsHub — aggregated observability data store.
//!
//! Combines in-memory GatewayMetrics with SQLite-backed historical metrics
//! and worker heartbeat data, providing a unified interface for MCP tools
//! to query system state and configuration.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use bastion_domain::orientation::ConfigChange;

use super::GatewayMetrics;
use super::heartbeat_bridge::{HeartbeatBridge, WorkerResources};

/// Per-sandbox resource usage from worker heartbeats.
///
/// This is the MCP-facing type with a proper datetime timestamp.
/// Converts from `WorkerResources` (which uses an epoch i64).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxResources {
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
    /// Last heartbeat timestamp.
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
}

impl From<WorkerResources> for SandboxResources {
    fn from(w: WorkerResources) -> Self {
        let last_heartbeat = chrono::DateTime::from_timestamp(w.last_heartbeat_epoch, 0)
            .unwrap_or_else(chrono::Utc::now);
        Self {
            sandbox_id: w.sandbox_id,
            cpu_percent: w.cpu_percent,
            mem_used_mb: w.mem_used_mb,
            mem_limit_mb: w.mem_limit_mb,
            disk_used_mb: w.disk_used_mb,
            loadavg_1m: w.loadavg_1m,
            uptime_seconds: w.uptime_seconds,
            last_heartbeat,
        }
    }
}

/// A historical metric record from SQLite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricRecord {
    /// Timestamp of the metric.
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Optional sandbox ID.
    pub sandbox_id: Option<String>,
    /// CPU usage percentage.
    pub cpu_percent: Option<f64>,
    /// Memory used in MB.
    pub mem_used_mb: Option<f64>,
    /// Memory limit in MB.
    pub mem_limit_mb: Option<f64>,
    /// Disk used in MB.
    pub disk_used_mb: Option<f64>,
    /// Commands executed count.
    pub commands_executed: Option<u64>,
    /// Errors total count.
    pub errors_total: Option<u64>,
}

/// Result of a config set operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetConfigResult {
    /// Whether the config was applied.
    pub applied: bool,
    /// Whether a restart is required for the change to take effect.
    pub requires_restart: bool,
    /// Human-readable hint about restart requirement.
    pub restart_hint: Option<String>,
}

/// Keys that cannot be changed at runtime.
const RESTRICTED_CONFIG_KEYS: &[&str] = &[
    "auth.hmac_enabled",
    "auth.jwt_enabled",
    "auth.pre_shared_key_enabled",
    "gateway.port",
];

/// MetricsHub — aggregated observability data store.
///
/// Provides:
/// - Historical metrics storage (SQLite)
/// - Per-sandbox resource usage (from worker heartbeats)
/// - Runtime config management with audit trail
/// - Integration with in-memory GatewayMetrics
pub struct MetricsHub {
    /// In-memory gateway metrics (counters/gauges).
    gateway_metrics: Arc<GatewayMetrics>,
    /// SQLite connection for historical metrics and config history.
    /// Wrapped in Arc<tokio::sync::Mutex<>> so MetricsHub: Send + Sync.
    /// tokio::sync::Mutex is Sync because it only needs T: Send (not T: Sync).
    sqlite: Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    /// Worker heartbeat bridge.
    heartbeat_bridge: Arc<HeartbeatBridge>,
    /// In-memory config history (ring buffer).
    config_history: tokio::sync::RwLock<Vec<ConfigChange>>,
    /// Maximum config history entries.
    max_config_history: usize,
}

impl MetricsHub {
    /// Create a new MetricsHub, initializing SQLite at the given path.
    ///
    /// If `db_path` is None, uses an in-memory SQLite database.
    pub async fn new(
        gateway_metrics: Arc<GatewayMetrics>,
        db_path: Option<PathBuf>,
    ) -> Result<Self, MetricsHubError> {
        let conn = if let Some(path) = db_path {
            rusqlite::Connection::open(&path).map_err(|e| MetricsHubError::Sqlite(e.to_string()))?
        } else {
            rusqlite::Connection::open_in_memory()
                .map_err(|e| MetricsHubError::Sqlite(e.to_string()))?
        };

        let hub = Self {
            gateway_metrics,
            sqlite: Arc::new(tokio::sync::Mutex::new(conn)),
            heartbeat_bridge: Arc::new(HeartbeatBridge::new()),
            config_history: tokio::sync::RwLock::new(Vec::new()),
            max_config_history: 1000,
        };

        hub.init_sqlite().await?;

        Ok(hub)
    }

    /// Create a new MetricsHub with an in-memory database (for testing).
    pub async fn new_in_memory(
        gateway_metrics: Arc<GatewayMetrics>,
    ) -> Result<Self, MetricsHubError> {
        Self::new(gateway_metrics, None).await
    }

    /// Initialize SQLite tables.
    async fn init_sqlite(&self) -> Result<(), MetricsHubError> {
        let conn = self.sqlite.lock().await;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS metrics_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                sandbox_id TEXT,
                cpu_percent REAL,
                mem_used_mb REAL,
                mem_limit_mb REAL,
                disk_used_mb REAL,
                commands_executed INTEGER,
                errors_total INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_metrics_timestamp ON metrics_history(timestamp);
            CREATE INDEX IF NOT EXISTS idx_metrics_sandbox ON metrics_history(sandbox_id);

            CREATE TABLE IF NOT EXISTS config_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                key TEXT NOT NULL,
                old_value TEXT,
                new_value TEXT,
                changed_by TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_config_timestamp ON config_history(timestamp);
            ",
        )
        .map_err(|e| MetricsHubError::Sqlite(e.to_string()))?;

        Ok(())
    }

    /// Record a metric to SQLite.
    pub async fn record_metric(&self, record: &MetricRecord) -> Result<(), MetricsHubError> {
        let conn = self.sqlite.lock().await;
        conn.execute(
            "INSERT INTO metrics_history (timestamp, sandbox_id, cpu_percent, mem_used_mb, mem_limit_mb, disk_used_mb, commands_executed, errors_total)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                record.timestamp.to_rfc3339(),
                record.sandbox_id,
                record.cpu_percent,
                record.mem_used_mb,
                record.mem_limit_mb,
                record.disk_used_mb,
                record.commands_executed,
                record.errors_total,
            ],
        ).map_err(|e| MetricsHubError::Sqlite(e.to_string()))?;

        Ok(())
    }

    /// Get historical metrics from SQLite, filtered by `since` timestamp.
    pub async fn get_metrics_history(
        &self,
        since: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<MetricRecord>, MetricsHubError> {
        let conn = self.sqlite.lock().await;
        let mut stmt = conn.prepare(
            "SELECT timestamp, sandbox_id, cpu_percent, mem_used_mb, mem_limit_mb, disk_used_mb, commands_executed, errors_total
             FROM metrics_history
             WHERE timestamp >= ?
             ORDER BY timestamp ASC"
        ).map_err(|e| MetricsHubError::Sqlite(e.to_string()))?;

        let records = stmt
            .query_map(rusqlite::params![since.to_rfc3339()], |row| {
                Ok(MetricRecord {
                    timestamp: row
                        .get::<_, String>(0)?
                        .parse()
                        .unwrap_or(chrono::Utc::now()),
                    sandbox_id: row.get::<_, Option<String>>(1)?,
                    cpu_percent: row.get::<_, Option<f64>>(2)?,
                    mem_used_mb: row.get::<_, Option<f64>>(3)?,
                    mem_limit_mb: row.get::<_, Option<f64>>(4)?,
                    disk_used_mb: row.get::<_, Option<f64>>(5)?,
                    commands_executed: row.get::<_, Option<u64>>(6)?,
                    errors_total: row.get::<_, Option<u64>>(7)?,
                })
            })
            .map_err(|e| MetricsHubError::Sqlite(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(records)
    }

    /// Get per-sandbox resource usage from the heartbeat bridge.
    pub async fn get_sandbox_resources(
        &self,
        sandbox_id: &str,
    ) -> Result<Option<SandboxResources>, MetricsHubError> {
        Ok(self
            .heartbeat_bridge
            .get_resources(sandbox_id)
            .map(SandboxResources::from))
    }

    /// Set a config value. Returns whether it was applied and if restart is required.
    pub async fn set_config(
        &self,
        key: &str,
        old_value: Option<String>,
        new_value: String,
        changed_by: &str,
    ) -> Result<SetConfigResult, MetricsHubError> {
        // Check if key is restricted
        if RESTRICTED_CONFIG_KEYS.contains(&key) {
            return Ok(SetConfigResult {
                applied: false,
                requires_restart: false,
                restart_hint: Some(format!("Key '{}' cannot be changed at runtime", key)),
            });
        }

        // Check if restart is required
        let requires_restart = key.starts_with("auth.")
            || key.starts_with("gateway.port")
            || key.starts_with("gateway.tls");

        // Persist to SQLite first (before moving values into ConfigChange)
        {
            let conn = self.sqlite.lock().await;
            conn.execute(
                "INSERT INTO config_history (timestamp, key, old_value, new_value, changed_by) VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![
                    chrono::Utc::now().to_rfc3339(),
                    key,
                    old_value,
                    new_value,
                    changed_by,
                ],
            ).map_err(|e| MetricsHubError::Sqlite(e.to_string()))?;
        }

        // Record config change in memory
        let change = ConfigChange::new(key, old_value, new_value, changed_by);
        {
            let mut history = self.config_history.write().await;
            if history.len() >= self.max_config_history {
                history.remove(0);
            }
            history.push(change);
        }

        Ok(SetConfigResult {
            applied: true,
            requires_restart,
            restart_hint: if requires_restart {
                Some("Gateway restart required for this change".to_string())
            } else {
                None
            },
        })
    }

    /// Get the config history from in-memory buffer.
    pub async fn get_config_history(&self) -> Vec<ConfigChange> {
        self.config_history.read().await.clone()
    }

    /// Get a reference to the heartbeat bridge for resource data.
    pub fn heartbeat_bridge(&self) -> Arc<HeartbeatBridge> {
        self.heartbeat_bridge.clone()
    }

    /// Get a reference to gateway metrics.
    pub fn gateway_metrics(&self) -> &GatewayMetrics {
        &self.gateway_metrics
    }

    /// Clean up metrics older than 30 days.
    pub async fn cleanup_old_metrics(&self) -> Result<usize, MetricsHubError> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
        let conn = self.sqlite.lock().await;
        let deleted = conn
            .execute(
                "DELETE FROM metrics_history WHERE timestamp < ?",
                rusqlite::params![cutoff.to_rfc3339()],
            )
            .map_err(|e| MetricsHubError::Sqlite(e.to_string()))?;

        Ok(deleted)
    }
}

/// Errors that can occur in MetricsHub operations.
#[derive(Debug, thiserror::Error)]
pub enum MetricsHubError {
    /// SQLite error.
    #[error("SQLite error: {0}")]
    Sqlite(String),
    /// Configuration error.
    #[error("Config error: {0}")]
    Config(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_metrics_hub_new_in_memory() {
        let metrics = Arc::new(GatewayMetrics::default());
        let hub = MetricsHub::new_in_memory(metrics).await;
        assert!(
            hub.is_ok(),
            "MetricsHub should initialize with in-memory SQLite"
        );
    }

    #[tokio::test]
    async fn test_metrics_hub_record_and_get() {
        let metrics = Arc::new(GatewayMetrics::default());
        let hub = MetricsHub::new_in_memory(metrics).await.unwrap();

        let record = MetricRecord {
            timestamp: chrono::Utc::now(),
            sandbox_id: Some("test-sandbox".to_string()),
            cpu_percent: Some(42.5),
            mem_used_mb: Some(256.0),
            mem_limit_mb: Some(512.0),
            disk_used_mb: Some(100.0),
            commands_executed: Some(10),
            errors_total: Some(0),
        };

        hub.record_metric(&record).await.unwrap();

        let history = hub
            .get_metrics_history(chrono::Utc::now() - chrono::Duration::hours(1))
            .await
            .unwrap();

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].sandbox_id.as_deref(), Some("test-sandbox"));
        assert!((history[0].cpu_percent.unwrap() - 42.5).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_metrics_hub_set_config() {
        let metrics = Arc::new(GatewayMetrics::default());
        let hub = MetricsHub::new_in_memory(metrics).await.unwrap();

        // Normal config change
        let result = hub
            .set_config(
                "pool.max_total",
                Some("10".to_string()),
                "15".to_string(),
                "sandbox_set_config",
            )
            .await
            .unwrap();

        assert!(result.applied);
        assert!(!result.requires_restart);
        assert!(result.restart_hint.is_none());

        // Restricted config key
        let result = hub
            .set_config(
                "auth.hmac_enabled",
                Some("true".to_string()),
                "false".to_string(),
                "sandbox_set_config",
            )
            .await
            .unwrap();

        assert!(!result.applied);
        assert!(result.restart_hint.is_some());
    }

    #[tokio::test]
    async fn test_metrics_hub_set_config_requires_restart() {
        let metrics = Arc::new(GatewayMetrics::default());
        let hub = MetricsHub::new_in_memory(metrics).await.unwrap();

        // Config change that requires restart
        let result = hub
            .set_config(
                "auth.pre_shared_key_enabled",
                None,
                "true".to_string(),
                "sandbox_set_config",
            )
            .await
            .unwrap();

        assert!(!result.applied);
        assert!(result.restart_hint.is_some());
    }

    #[tokio::test]
    async fn test_metrics_hub_config_history() {
        let metrics = Arc::new(GatewayMetrics::default());
        let hub = MetricsHub::new_in_memory(metrics).await.unwrap();

        hub.set_config(
            "pool.max_total",
            Some("10".to_string()),
            "15".to_string(),
            "test",
        )
        .await
        .unwrap();
        hub.set_config(
            "pool.min_idle",
            Some("2".to_string()),
            "3".to_string(),
            "test",
        )
        .await
        .unwrap();

        let history = hub.get_config_history().await;
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].key, "pool.max_total");
        assert_eq!(history[1].key, "pool.min_idle");
    }

    #[tokio::test]
    async fn test_metrics_hub_cleanup() {
        let metrics = Arc::new(GatewayMetrics::default());
        let hub = MetricsHub::new_in_memory(metrics).await.unwrap();

        // Record a metric
        let record = MetricRecord {
            timestamp: chrono::Utc::now() - chrono::Duration::days(31),
            sandbox_id: Some("old-sandbox".to_string()),
            cpu_percent: Some(10.0),
            mem_used_mb: Some(100.0),
            mem_limit_mb: Some(512.0),
            disk_used_mb: Some(50.0),
            commands_executed: Some(5),
            errors_total: Some(0),
        };
        hub.record_metric(&record).await.unwrap();

        // Cleanup should remove old records
        let deleted = hub.cleanup_old_metrics().await.unwrap();
        assert_eq!(deleted, 1);

        // Verify it's gone
        let history = hub
            .get_metrics_history(chrono::Utc::now() - chrono::Duration::days(60))
            .await
            .unwrap();
        assert_eq!(history.len(), 0);
    }
}
