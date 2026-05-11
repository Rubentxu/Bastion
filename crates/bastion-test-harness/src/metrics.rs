//! MetricsCollector implementation.
//!
//! - `test-metrics` enabled: SQLite-backed collector with synchronous writes
//! - `test-metrics` disabled: no-op stub
//!
//! Uses a Mutex to protect the single SQLite connection, which is safe
//! because test code runs single-threaded (no async).

use std::path::Path;
use std::sync::Mutex;

use crate::types::{LatencyStats, MetricsError};

// ─────────────────────────────────────────────────────────────────────────────
// Shared helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Compute the p-th percentile of a sorted vector.
fn percentile(sorted_values: &[u64], p: u64) -> u64 {
    if sorted_values.is_empty() {
        return 0;
    }
    let idx = (p as f64 / 100.0 * (sorted_values.len() as f64 - 1.0)).round() as usize;
    let idx = idx.min(sorted_values.len() - 1);
    sorted_values[idx]
}

// ─────────────────────────────────────────────────────────────────────────────
// Active implementation (test-metrics feature enabled)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "test-metrics")]
mod active {
    use super::*;
    use crate::types::RegressionResult;
    use rusqlite::Connection;

    /// A handle to the metrics collection.
    ///
    /// Stores metrics in a SQLite database. Uses a Mutex to serialize access
    /// since SQLite concurrent reads are OK but writes need locking.
    #[derive(Debug)]
    pub struct MetricsCollector {
        conn: Mutex<Connection>,
        db_path: std::path::PathBuf,
    }

    impl MetricsCollector {
        /// Create a new MetricsCollector with the given SQLite database path.
        pub fn new(path: impl AsRef<Path>) -> Result<Self, MetricsError> {
            // Canonicalize FIRST, then use the canonical path for everything.
            // This ensures self.db_path matches the path used to open the connection.
            let canonical = path
                .as_ref()
                .canonicalize()
                .unwrap_or_else(|_| path.as_ref().to_path_buf());

            let conn = Connection::open(&canonical)
                .map_err(|e| MetricsError::Database(format!("Failed to open database: {}", e)))?;

            crate::schema::init_schema(&conn)
                .map_err(|e| MetricsError::Database(format!("Failed to init schema: {}", e)))?;

            Ok(Self {
                conn: Mutex::new(conn),
                db_path: canonical,
            })
        }

        /// Create a no-op MetricsCollector that discards all data.
        #[allow(dead_code)]
        pub fn noop() -> Self {
            let conn = Connection::open_in_memory()
                .map_err(|e| MetricsError::Database(e.to_string()))
                .unwrap();
            Self {
                conn: Mutex::new(conn),
                db_path: std::path::PathBuf::new(),
            }
        }

        /// Record a test run (fire-and-forget).
        ///
        /// This is synchronous — the write completes before the function returns.
        /// For high-throughput test suites, use `try_record_test` to avoid blocking.
        pub fn record_test(
            &self,
            test_name: &str,
            duration_ms: u64,
            status: &str,
            crate_name: &str,
            file_path: &str,
        ) {
            let build_hash = std::env::var("BUILD_HASH")
                .or_else(|_| std::env::var("CARGO_PKG_VERSION"))
                .unwrap_or_else(|_| "unknown".to_string());

            let timestamp = chrono::Utc::now().to_rfc3339();

            let conn = match self.conn.lock() {
                Ok(c) => c,
                Err(_) => return,
            };

            let _ = crate::schema::record_run(
                &conn,
                test_name,
                crate_name,
                file_path,
                &timestamp,
                duration_ms,
                status,
                &build_hash,
            );
        }

        /// Try to record a test run without blocking.
        ///
        /// Returns `Ok(())` if recorded, `Err(...)` if the lock is poisoned.
        #[allow(dead_code)]
        pub fn try_record_test(
            &self,
            test_name: &str,
            duration_ms: u64,
            status: &str,
            crate_name: &str,
            file_path: &str,
        ) -> Result<(), MetricsError> {
            let build_hash = std::env::var("BUILD_HASH")
                .or_else(|_| std::env::var("CARGO_PKG_VERSION"))
                .unwrap_or_else(|_| "unknown".to_string());

            let timestamp = chrono::Utc::now().to_rfc3339();

            let conn = self
                .conn
                .lock()
                .map_err(|e| MetricsError::Database(format!("Lock poisoned: {}", e)))?;

            crate::schema::record_run(
                &conn,
                test_name,
                crate_name,
                file_path,
                &timestamp,
                duration_ms,
                status,
                &build_hash,
            )
            .map_err(|e| MetricsError::Database(e.to_string()))
        }

        /// Compute latency statistics (P50, P95, P99) for a test.
        pub fn latency_stats(&self, test_name: &str) -> Result<LatencyStats, MetricsError> {
            let conn = self
                .conn
                .lock()
                .map_err(|e| MetricsError::Database(format!("Lock poisoned: {}", e)))?;

            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT duration_ms FROM test_runs
                    WHERE test_name = ?1
                    ORDER BY duration_ms ASC
                    "#,
                )
                .map_err(|e| MetricsError::Database(e.to_string()))?;

            let durations: Vec<u64> = stmt
                .query_map([test_name], |row| row.get::<_, i64>(0))
                .map_err(|e| MetricsError::Database(e.to_string()))?
                .filter_map(|r| r.ok())
                .map(|v| v as u64)
                .collect();

            if durations.len() < 3 {
                return Err(MetricsError::InsufficientSamples {
                    have: durations.len(),
                    need: 3,
                });
            }

            let p50 = percentile(&durations, 50);
            let p95 = percentile(&durations, 95);
            let p99 = percentile(&durations, 99);

            Ok(LatencyStats {
                p50_ms: p50,
                p95_ms: p95,
                p99_ms: p99,
                sample_count: durations.len(),
            })
        }

        /// Compute flakiness score: `failed / total` runs.
        pub fn flakiness_score(&self, test_name: &str) -> Result<f32, MetricsError> {
            let conn = self
                .conn
                .lock()
                .map_err(|e| MetricsError::Database(format!("Lock poisoned: {}", e)))?;

            #[derive(Debug)]
            struct CountRow {
                total: i64,
                failed: i64,
            }

            let row: CountRow = conn
                .query_row(
                    r#"
                    SELECT
                        COUNT(*) as total,
                        COALESCE(SUM(CASE WHEN status = 'fail' THEN 1 ELSE 0 END), 0) as failed
                    FROM test_runs
                    WHERE test_name = ?1
                    "#,
                    [test_name],
                    |row| {
                        Ok(CountRow {
                            total: row.get(0)?,
                            failed: row.get(1)?,
                        })
                    },
                )
                .map_err(|e| MetricsError::Database(e.to_string()))?;

            if row.total == 0 {
                return Ok(0.0);
            }

            Ok(row.failed as f32 / row.total as f32)
        }

        /// Detect regressions between two metric databases (P95 delta > 20%).
        #[allow(dead_code)]
        pub fn regression_detect(
            baseline_db: impl AsRef<Path>,
            current_db: impl AsRef<Path>,
        ) -> Result<Vec<RegressionResult>, MetricsError> {
            let baseline_conn = rusqlite::Connection::open(baseline_db.as_ref()).map_err(|e| {
                MetricsError::RegressionDetect(format!("Cannot open baseline: {}", e))
            })?;
            let current_conn = rusqlite::Connection::open(current_db.as_ref()).map_err(|e| {
                MetricsError::RegressionDetect(format!("Cannot open current: {}", e))
            })?;

            let test_names: Vec<String> = {
                let mut stmt = baseline_conn
                    .prepare("SELECT DISTINCT test_name FROM test_runs")
                    .map_err(|e| MetricsError::RegressionDetect(e.to_string()))?;
                stmt.query_map([], |row| row.get(0))
                    .map_err(|e| MetricsError::RegressionDetect(e.to_string()))?
                    .filter_map(|r| r.ok())
                    .collect()
            };

            let mut regressions = Vec::new();

            for test_name in test_names {
                let baseline_p95 = p95_for_test(&baseline_conn, &test_name);
                let current_p95 = p95_for_test(&current_conn, &test_name);

                if let (Some(bp95), Some(cp95)) = (baseline_p95, current_p95) {
                    if bp95 > 0 {
                        let delta_pct = ((cp95 as f64 - bp95 as f64) / bp95 as f64) * 100.0;
                        if delta_pct > 20.0 {
                            regressions.push(RegressionResult {
                                test_name,
                                baseline_p95: bp95,
                                current_p95: cp95,
                                delta_pct,
                            });
                        }
                    }
                }
            }

            Ok(regressions)
        }
    }

    /// Get P95 latency for a test from a connection.
    fn p95_for_test(conn: &rusqlite::Connection, test_name: &str) -> Option<u64> {
        let mut stmt = match conn.prepare(
            r#"
            SELECT duration_ms FROM test_runs
            WHERE test_name = ?1
            ORDER BY duration_ms ASC
            "#,
        ) {
            Ok(s) => s,
            Err(_) => return None,
        };

        let durations: Vec<u64> = stmt
            .query_map([test_name], |row| row.get::<_, i64>(0))
            .ok()?
            .filter_map(|r| r.ok())
            .map(|v| v as u64)
            .collect();

        if durations.len() < 3 {
            return None;
        }

        Some(percentile(&durations, 95))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Noop implementation (test-metrics feature disabled)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(feature = "test-metrics"))]
mod noop {
    use super::*;
    use std::path::Path;

    /// A no-op MetricsCollector that discards all data.
    ///
    /// Used when the `test-metrics` feature is disabled.
    #[derive(Debug, Clone, Default)]
    pub struct MetricsCollector;

    impl MetricsCollector {
        /// Create a no-op collector.
        #[allow(dead_code)]
        pub fn new(_path: impl AsRef<Path>) -> Result<Self, MetricsError> {
            Ok(Self)
        }

        /// Create a no-op collector (alias for `new`).
        #[allow(dead_code)]
        pub fn noop() -> Self {
            Self
        }

        /// No-op: does nothing.
        #[allow(dead_code)]
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
        #[allow(dead_code)]
        pub fn latency_stats(&self, _test_name: &str) -> Result<LatencyStats, MetricsError> {
            Err(MetricsError::Database(
                "MetricsCollector is disabled".to_string(),
            ))
        }

        /// Returns 0.0 for noop collector.
        #[allow(dead_code)]
        pub fn flakiness_score(&self, _test_name: &str) -> Result<f32, MetricsError> {
            Ok(0.0)
        }

        /// Returns empty vec for noop collector.
        #[allow(dead_code)]
        pub fn regression_detect(
            _baseline: impl AsRef<Path>,
            _current: impl AsRef<Path>,
        ) -> Result<Vec<RegressionResult>, MetricsError> {
            Ok(Vec::new())
        }
    }
}

// Re-export the appropriate implementation
#[cfg(feature = "test-metrics")]
pub use active::MetricsCollector;

#[cfg(not(feature = "test-metrics"))]
pub use noop::MetricsCollector;
