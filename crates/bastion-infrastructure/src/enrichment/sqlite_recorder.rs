//! SQLite-backed run recorder for persistence of enrichment telemetry.
//!
//! Implements the `RunRecorder` trait using rusqlite for storage.
//! Uses `tokio::sync::Mutex<Connection>` for async-safe concurrent access.

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::params;
use std::path::Path;
use tokio::sync::Mutex;

use bastion_domain::shared::DomainError;
use enrichment_engine::models::EnrichmentRunRecord;
use enrichment_engine::traits::{EnrichmentError, RunRecorder};

use super::config::RetentionConfig;
use super::schema_migration::SchemaMigration;

/// SQLite-backed implementation of `RunRecorder`.
///
/// Persists `EnrichmentRunRecord`s to an SQLite database with an inline
/// `CREATE TABLE IF NOT EXISTS` schema.
#[derive(Debug)]
pub struct SqliteRunRecorder {
    pub(crate) conn: Mutex<rusqlite::Connection>,
    retention: RetentionConfig,
}

impl SqliteRunRecorder {
    /// Create a new recorder, creating the DB schema if it doesn't exist.
    ///
    /// # Arguments
    ///
    /// * `db_path` - Path to the SQLite database file
    ///
    /// # Errors
    ///
    /// Returns `DomainError` if the database cannot be opened or schema creation fails.
    pub fn new(db_path: &Path) -> Result<Self, DomainError> {
        Self::with_retention(db_path, RetentionConfig::default())
    }

    /// Create a new recorder with custom retention config.
    ///
    /// # Arguments
    ///
    /// * `db_path` - Path to the SQLite database file
    /// * `retention` - Retention policy configuration
    ///
    /// # Errors
    ///
    /// Returns `DomainError` if the database cannot be opened or schema creation fails.
    pub fn with_retention(db_path: &Path, retention: RetentionConfig) -> Result<Self, DomainError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DomainError::Internal(format!("Failed to create DB directory: {}", e)))?;
        }

        let conn = rusqlite::Connection::open(db_path)
            .map_err(|e| DomainError::Internal(format!("Failed to open SQLite DB: {}", e)))?;

        // Run schema migrations before any schema creation
        // This ensures backward compatibility with existing databases
        SchemaMigration::run(&conn)?;

        // Inline schema creation
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS enrichment_runs (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                command TEXT NOT NULL,
                enricher_id TEXT NOT NULL,
                exit_code INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                output_summary_stdout TEXT,
                output_summary_stderr TEXT,
                facts_count INTEGER DEFAULT 0,
                derived_facts_count INTEGER DEFAULT 0,
                rule_hits_count INTEGER DEFAULT 0,
                diagnostics_count INTEGER DEFAULT 0,
                artifact_count INTEGER DEFAULT 0,
                confidence_avg REAL DEFAULT 0.0,
                verdict TEXT,
                recommendation_count INTEGER DEFAULT 0,
                error TEXT
            );
            "#,
        )
        .map_err(|e| DomainError::Internal(format!("Failed to create schema: {}", e)))?;

        Ok(Self {
            conn: Mutex::new(conn),
            retention,
        })
    }

    /// Create a recorder backed by an in-memory database.
    ///
    /// This is intended for testing only. The schema is created automatically.
    #[cfg(test)]
    pub fn in_memory() -> Result<Self, DomainError> {
        Self::with_retention_in_memory(RetentionConfig::default())
    }

    /// Create a recorder backed by an in-memory database with custom retention config.
    ///
    /// This is intended for testing only. The schema is created automatically.
    #[cfg(test)]
    pub fn with_retention_in_memory(retention: RetentionConfig) -> Result<Self, DomainError> {
        let conn = rusqlite::Connection::open_in_memory()
            .map_err(|e| DomainError::Internal(format!("Failed to open in-memory DB: {}", e)))?;

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS enrichment_runs (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                command TEXT NOT NULL,
                enricher_id TEXT NOT NULL,
                exit_code INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                output_summary_stdout TEXT,
                output_summary_stderr TEXT,
                facts_count INTEGER DEFAULT 0,
                derived_facts_count INTEGER DEFAULT 0,
                rule_hits_count INTEGER DEFAULT 0,
                diagnostics_count INTEGER DEFAULT 0,
                artifact_count INTEGER DEFAULT 0,
                confidence_avg REAL DEFAULT 0.0,
                verdict TEXT,
                recommendation_count INTEGER DEFAULT 0,
                error TEXT
            );
            "#,
        )
        .map_err(|e| DomainError::Internal(format!("Failed to create in-memory schema: {}", e)))?;

        Ok(Self {
            conn: Mutex::new(conn),
            retention,
        })
    }

    /// Clean up old records based on retention policy.
    ///
    /// Deletes records older than `max_age_days` and reduces row count to `max_rows`
    /// by deleting the oldest records first.
    ///
    /// This method is idempotent — safe to call multiple times.
    ///
    /// # Returns
    ///
    /// The number of rows deleted.
    pub async fn cleanup(&self) -> Result<u64, EnrichmentError> {
        if !self.retention.enabled {
            return Ok(0);
        }

        let conn = self.conn.lock().await;
        let mut total_deleted: u64 = 0;

        // 1. Age-based deletion
        let cutoff = Utc::now() - chrono::Duration::days(self.retention.max_age_days as i64);
        let cutoff_str = cutoff.to_rfc3339();

        let deleted = conn
            .execute(
                "DELETE FROM enrichment_runs WHERE timestamp < ?1",
                params![cutoff_str],
            )
            .map_err(|e| EnrichmentError::Recorder(format!("cleanup age-based delete failed: {}", e)))?;

        total_deleted += deleted as u64;

        // 2. Row-count cap deletion
        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM enrichment_runs", [], |row| row.get(0))
            .map_err(|e| EnrichmentError::Recorder(format!("count query failed: {}", e)))?;

        if row_count > self.retention.max_rows as i64 {
            let to_delete = row_count - self.retention.max_rows as i64;
            // Delete oldest rows first (by timestamp ASC)
            let deleted = conn
                .execute(
                    &format!(
                        "DELETE FROM enrichment_runs WHERE id IN (SELECT id FROM enrichment_runs ORDER BY timestamp ASC LIMIT {})",
                        to_delete
                    ),
                    [],
                )
                .map_err(|e| EnrichmentError::Recorder(format!("cleanup row-count delete failed: {}", e)))?;

            total_deleted += deleted as u64;
        }

        Ok(total_deleted)
    }

    /// Get the current retention configuration.
    pub fn retention_config(&self) -> &RetentionConfig {
        &self.retention
    }
}

#[async_trait]
impl RunRecorder for SqliteRunRecorder {
    /// Persist an enrichment run record to the database.
    ///
    /// If `retention.sanitize` is true, the command string is sanitized
    /// before being stored to redact known secret patterns.
    async fn record(&self, run: &EnrichmentRunRecord) -> Result<(), EnrichmentError> {
        let conn = self.conn.lock().await;

        // Sanitize command if configured
        let command = if self.retention.sanitize {
            enrichment_engine::sanitize_command(&run.command)
        } else {
            run.command.clone()
        };

        conn.execute(
            r#"
            INSERT OR REPLACE INTO enrichment_runs (
                id, timestamp, command, enricher_id, exit_code, duration_ms,
                output_summary_stdout, output_summary_stderr,
                facts_count, derived_facts_count, rule_hits_count,
                diagnostics_count, artifact_count, confidence_avg,
                verdict, recommendation_count, error
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17
            )
            "#,
            params![
                run.id,
                run.timestamp,
                command,
                run.enricher_id,
                run.exit_code,
                run.duration_ms as i64,
                run.output_summary_stdout,
                run.output_summary_stderr,
                run.facts_count as i32,
                run.derived_facts_count as i32,
                run.rule_hits_count as i32,
                run.diagnostics_count as i32,
                run.artifact_count as i32,
                run.confidence_avg,
                run.verdict,
                run.recommendation_count as i32,
                run.error,
            ],
        )
        .map_err(|e| EnrichmentError::Recorder(format!("Failed to insert record: {}", e)))?;

        Ok(())
    }

    fn retention_config(&self) -> &enrichment_engine::models::RetentionConfig {
        &self.retention
    }

    async fn cleanup(&self) -> Result<u64, EnrichmentError> {
        if !self.retention.enabled {
            return Ok(0);
        }

        let conn = self.conn.lock().await;
        let mut total_deleted: u64 = 0;

        // 1. Age-based deletion
        let cutoff = Utc::now() - chrono::Duration::days(self.retention.max_age_days as i64);
        let cutoff_str = cutoff.to_rfc3339();

        let deleted = conn
            .execute(
                "DELETE FROM enrichment_runs WHERE timestamp < ?1",
                params![cutoff_str],
            )
            .map_err(|e| EnrichmentError::Recorder(format!("cleanup age-based delete failed: {}", e)))?;

        total_deleted += deleted as u64;

        // 2. Row-count cap deletion
        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM enrichment_runs", [], |row| row.get(0))
            .map_err(|e| EnrichmentError::Recorder(format!("count query failed: {}", e)))?;

        if row_count > self.retention.max_rows as i64 {
            let to_delete = row_count - self.retention.max_rows as i64;
            let deleted = conn
                .execute(
                    &format!(
                        "DELETE FROM enrichment_runs WHERE id IN (SELECT id FROM enrichment_runs ORDER BY timestamp ASC LIMIT {})",
                        to_delete
                    ),
                    [],
                )
                .map_err(|e| EnrichmentError::Recorder(format!("cleanup row-count delete failed: {}", e)))?;

            total_deleted += deleted as u64;
        }

        Ok(total_deleted)
    }

    async fn stats(&self) -> Result<enrichment_engine::models::RunRecorderStats, EnrichmentError> {
        let conn = self.conn.lock().await;

        let row_count: u64 = conn
            .query_row("SELECT COUNT(*) FROM enrichment_runs", [], |row| row.get(0))
            .map_err(|e| EnrichmentError::Recorder(format!("stats count query failed: {}", e)))?;

        if row_count == 0 {
            return Ok(enrichment_engine::models::RunRecorderStats::empty());
        }

        let oldest: Option<String> = conn
            .query_row(
                "SELECT MIN(timestamp) FROM enrichment_runs",
                [],
                |row| row.get(0),
            )
            .map_err(|e| EnrichmentError::Recorder(format!("stats oldest query failed: {}", e)))?;

        let newest: Option<String> = conn
            .query_row(
                "SELECT MAX(timestamp) FROM enrichment_runs",
                [],
                |row| row.get(0),
            )
            .map_err(|e| EnrichmentError::Recorder(format!("stats newest query failed: {}", e)))?;

        Ok(enrichment_engine::models::RunRecorderStats::new(
            row_count,
            oldest,
            newest,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enrichment_engine::models::EnrichmentRunRecord;

    #[tokio::test]
    async fn test_record_persists_and_can_be_queried() {
        let recorder = SqliteRunRecorder::in_memory().unwrap();

        let record = EnrichmentRunRecord::new(
            "550e8400-e29b-41d4-a716-446655440000".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "mvn package".to_string(),
            "maven".to_string(),
            0,
            5000,
            Some("BUILD SUCCESS".to_string()),
            None,
            5,
            2,
            3,
            1,
            2,
            0.85,
            Some("PASSED".to_string()),
            1,
            None,
        );

        recorder.record(&record).await.unwrap();

        // Verify the record was persisted by checking row count
        let conn = recorder.conn.lock().await;
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM enrichment_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Query the record and verify fields
        let stored: (String, String, i32, i32, f64, Option<String>) = conn
            .query_row(
                "SELECT id, command, exit_code, facts_count, confidence_avg, verdict FROM enrichment_runs WHERE id = ?1",
                params!["550e8400-e29b-41d4-a716-446655440000"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )
            .unwrap();

        assert_eq!(stored.0, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(stored.1, "mvn package");
        assert_eq!(stored.2, 0);
        assert_eq!(stored.3, 5);
        assert!((stored.4 - 0.85).abs() < f64::EPSILON);
        assert_eq!(stored.5, Some("PASSED".to_string()));
    }

    #[tokio::test]
    async fn test_record_with_error() {
        let recorder = SqliteRunRecorder::in_memory().unwrap();

        let record = EnrichmentRunRecord::new(
            "550e8400-e29b-41d4-a716-446655440001".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "mvn package".to_string(),
            "maven".to_string(),
            1,
            1000,
            None,
            Some("COMPILATION ERROR".to_string()),
            0,
            0,
            0,
            0,
            0,
            0.0,
            None,
            0,
            Some("extraction failed: pattern not found".to_string()),
        );

        recorder.record(&record).await.unwrap();

        let conn = recorder.conn.lock().await;
        let error: Option<String> = conn
            .query_row(
                "SELECT error FROM enrichment_runs WHERE id = ?1",
                params!["550e8400-e29b-41d4-a716-446655440001"],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(error, Some("extraction failed: pattern not found".to_string()));
    }

    #[tokio::test]
    async fn test_multiple_records() {
        let recorder = SqliteRunRecorder::in_memory().unwrap();

        for i in 0..5 {
            let record = EnrichmentRunRecord::new(
                format!("550e8400-e29b-41d4-a716-44665544000{}", i),
                "2024-01-01T00:00:00Z".to_string(),
                format!("mvn package {}", i),
                "maven".to_string(),
                0,
                5000 + (i as u64 * 100),
                Some("BUILD SUCCESS".to_string()),
                None,
                5 + i,
                2,
                3,
                1,
                2,
                0.85,
                Some("PASSED".to_string()),
                1,
                None,
            );
            recorder.record(&record).await.unwrap();
        }

        let conn = recorder.conn.lock().await;
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM enrichment_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 5);

        // Verify ordering by checking facts_count varies
        // Use a prepared statement and iterate while holding the lock
        let mut stmt = conn
            .prepare("SELECT facts_count FROM enrichment_runs ORDER BY command")
            .unwrap();
        let facts: Vec<i32> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(facts, vec![5, 6, 7, 8, 9]);
    }

    #[tokio::test]
    async fn test_replace_existing_record() {
        let recorder = SqliteRunRecorder::in_memory().unwrap();

        let record1 = EnrichmentRunRecord::new(
            "550e8400-e29b-41d4-a716-446655440010".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "mvn package".to_string(),
            "maven".to_string(),
            0,
            5000,
            Some("BUILD SUCCESS".to_string()),
            None,
            5,
            2,
            3,
            1,
            2,
            0.85,
            Some("PASSED".to_string()),
            1,
            None,
        );

        recorder.record(&record1).await.unwrap();

        // Record same ID with different facts_count
        let record2 = EnrichmentRunRecord::new(
            "550e8400-e29b-41d4-a716-446655440010".to_string(),
            "2024-01-01T00:00:01Z".to_string(),
            "mvn package".to_string(),
            "maven".to_string(),
            0,
            6000,
            Some("BUILD SUCCESS".to_string()),
            None,
            10, // Different facts_count
            2,
            3,
            1,
            2,
            0.90,
            Some("PASSED".to_string()),
            1,
            None,
        );

        recorder.record(&record2).await.unwrap();

        let conn = recorder.conn.lock().await;
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM enrichment_runs", [], |row| row.get(0))
            .unwrap();
        // Should still be 1 (replaced)
        assert_eq!(count, 1);

        // Verify the updated values
        let facts: i32 = conn
            .query_row(
                "SELECT facts_count FROM enrichment_runs WHERE id = ?1",
                params!["550e8400-e29b-41d4-a716-446655440010"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(facts, 10);
    }

    #[tokio::test]
    async fn test_auto_create_table_on_fresh_db() {
        // SqliteRunRecorder::in_memory() creates the table automatically
        let recorder = SqliteRunRecorder::in_memory().unwrap();

        // Should be able to record immediately without error
        let record = EnrichmentRunRecord::new(
            "test-id".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "echo hello".to_string(),
            "".to_string(),
            0,
            100,
            Some("hello".to_string()),
            None,
            0,
            0,
            0,
            0,
            0,
            0.0,
            None,
            0,
            None,
        );

        recorder.record(&record).await.unwrap();
    }

    // ─── Sanitization tests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_record_sanitizes_token_by_default() {
        let recorder = SqliteRunRecorder::in_memory().unwrap();

        let record = EnrichmentRunRecord::new(
            "sanitize-test-1".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "curl -H 'Authorization: Bearer secret123'".to_string(),
            "http".to_string(),
            0,
            100,
            None,
            None,
            0,
            0,
            0,
            0,
            0,
            0.0,
            None,
            0,
            None,
        );

        recorder.record(&record).await.unwrap();

        let conn = recorder.conn.lock().await;
        let stored_cmd: String = conn
            .query_row(
                "SELECT command FROM enrichment_runs WHERE id = 'sanitize-test-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(stored_cmd, "curl -H 'Authorization: Bearer [REDACTED]'");
    }

    #[tokio::test]
    async fn test_record_sanitizes_api_key() {
        let recorder = SqliteRunRecorder::in_memory().unwrap();

        let record = EnrichmentRunRecord::new(
            "sanitize-test-2".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "api_key=sk-12345&model=gpt-4".to_string(),
            "openai".to_string(),
            0,
            100,
            None,
            None,
            0,
            0,
            0,
            0,
            0,
            0.0,
            None,
            0,
            None,
        );

        recorder.record(&record).await.unwrap();

        let conn = recorder.conn.lock().await;
        let stored_cmd: String = conn
            .query_row(
                "SELECT command FROM enrichment_runs WHERE id = 'sanitize-test-2'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(stored_cmd, "api_key=[REDACTED]&model=gpt-4");
    }

    #[tokio::test]
    async fn test_record_no_sanitize_when_disabled() {
        let mut retention = RetentionConfig::default();
        retention.sanitize = false;
        let recorder = SqliteRunRecorder::with_retention_in_memory(retention).unwrap();

        let record = EnrichmentRunRecord::new(
            "sanitize-test-3".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "token=my-secret-token".to_string(),
            "test".to_string(),
            0,
            100,
            None,
            None,
            0,
            0,
            0,
            0,
            0,
            0.0,
            None,
            0,
            None,
        );

        recorder.record(&record).await.unwrap();

        let conn = recorder.conn.lock().await;
        let stored_cmd: String = conn
            .query_row(
                "SELECT command FROM enrichment_runs WHERE id = 'sanitize-test-3'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        // Should be unchanged since sanitization is disabled
        assert_eq!(stored_cmd, "token=my-secret-token");
    }

    // ─── Cleanup tests ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_cleanup_disabled_returns_zero() {
        let mut retention = RetentionConfig::default();
        retention.enabled = false;
        let recorder = SqliteRunRecorder::with_retention_in_memory(retention).unwrap();

        let deleted = recorder.cleanup().await.unwrap();
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn test_cleanup_age_based() {
        let mut retention = RetentionConfig::default();
        retention.max_age_days = 30; // 30 days
        retention.enabled = true;
        let recorder = SqliteRunRecorder::with_retention_in_memory(retention).unwrap();

        // Record from 60 days ago (should be deleted)
        let old_record = EnrichmentRunRecord::new(
            "old-record".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "old command".to_string(),
            "test".to_string(),
            0,
            100,
            None,
            None,
            0,
            0,
            0,
            0,
            0,
            0.0,
            None,
            0,
            None,
        );

        // Record from today (should be kept)
        let new_record = EnrichmentRunRecord::new(
            "new-record".to_string(),
            chrono::Utc::now().to_rfc3339(),
            "new command".to_string(),
            "test".to_string(),
            0,
            100,
            None,
            None,
            0,
            0,
            0,
            0,
            0,
            0.0,
            None,
            0,
            None,
        );

        recorder.record(&old_record).await.unwrap();
        recorder.record(&new_record).await.unwrap();

        let deleted = recorder.cleanup().await.unwrap();
        assert_eq!(deleted, 1);

        // Verify old record is gone, new record remains
        let conn = recorder.conn.lock().await;
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM enrichment_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let remaining_id: String = conn
            .query_row("SELECT id FROM enrichment_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining_id, "new-record");
    }

    #[tokio::test]
    async fn test_cleanup_row_count_cap() {
        let mut retention = RetentionConfig::default();
        retention.max_rows = 5;
        retention.enabled = true;
        let recorder = SqliteRunRecorder::with_retention_in_memory(retention).unwrap();

        // Insert 10 records
        for i in 0..10 {
            let record = EnrichmentRunRecord::new(
                format!("row-cap-{}", i),
                chrono::Utc::now().to_rfc3339(),
                format!("command {}", i),
                "test".to_string(),
                0,
                100,
                None,
                None,
                0,
                0,
                0,
                0,
                0,
                0.0,
                None,
                0,
                None,
            );
            recorder.record(&record).await.unwrap();
        }

        let deleted = recorder.cleanup().await.unwrap();
        assert_eq!(deleted, 5); // 10 - 5 = 5 deleted

        let conn = recorder.conn.lock().await;
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM enrichment_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 5);
    }

    #[tokio::test]
    async fn test_cleanup_under_cap_no_op() {
        let mut retention = RetentionConfig::default();
        retention.max_rows = 100;
        retention.enabled = true;
        let recorder = SqliteRunRecorder::with_retention_in_memory(retention).unwrap();

        // Insert 5 records (under cap of 100)
        for i in 0..5 {
            let record = EnrichmentRunRecord::new(
                format!("under-cap-{}", i),
                chrono::Utc::now().to_rfc3339(),
                format!("command {}", i),
                "test".to_string(),
                0,
                100,
                None,
                None,
                0,
                0,
                0,
                0,
                0,
                0.0,
                None,
                0,
                None,
            );
            recorder.record(&record).await.unwrap();
        }

        let deleted = recorder.cleanup().await.unwrap();
        assert_eq!(deleted, 0); // No deletion needed

        let conn = recorder.conn.lock().await;
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM enrichment_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 5);
    }

    #[tokio::test]
    async fn test_cleanup_within_window_kept() {
        let mut retention = RetentionConfig::default();
        retention.max_age_days = 90; // 90 days
        retention.enabled = true;
        let recorder = SqliteRunRecorder::with_retention_in_memory(retention).unwrap();

        // Record from 30 days ago (within window)
        let within_window = EnrichmentRunRecord::new(
            "within-window".to_string(),
            (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339(),
            "recent command".to_string(),
            "test".to_string(),
            0,
            100,
            None,
            None,
            0,
            0,
            0,
            0,
            0,
            0.0,
            None,
            0,
            None,
        );

        recorder.record(&within_window).await.unwrap();

        let deleted = recorder.cleanup().await.unwrap();
        assert_eq!(deleted, 0); // No deletion

        let conn = recorder.conn.lock().await;
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM enrichment_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    // ─── Stats tests ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_stats_empty_database() {
        let recorder = SqliteRunRecorder::in_memory().unwrap();

        let stats = recorder.stats().await.unwrap();
        assert_eq!(stats.current_row_count, 0);
        assert!(stats.oldest_record_ts.is_none());
        assert!(stats.newest_record_ts.is_none());
    }

    #[tokio::test]
    async fn test_stats_single_record() {
        let recorder = SqliteRunRecorder::in_memory().unwrap();

        let record = EnrichmentRunRecord::new(
            "stats-test-1".to_string(),
            "2024-06-01T12:00:00Z".to_string(),
            "mvn test".to_string(),
            "maven".to_string(),
            0,
            5000,
            Some("BUILD SUCCESS".to_string()),
            None,
            5,
            2,
            3,
            1,
            2,
            0.85,
            Some("PASSED".to_string()),
            1,
            None,
        );

        recorder.record(&record).await.unwrap();

        let stats = recorder.stats().await.unwrap();
        assert_eq!(stats.current_row_count, 1);
        assert_eq!(stats.oldest_record_ts, Some("2024-06-01T12:00:00Z".to_string()));
        assert_eq!(stats.newest_record_ts, Some("2024-06-01T12:00:00Z".to_string()));
    }

    #[tokio::test]
    async fn test_stats_multiple_records() {
        let recorder = SqliteRunRecorder::in_memory().unwrap();

        // Record from 30 days ago
        let old_record = EnrichmentRunRecord::new(
            "stats-old".to_string(),
            "2024-05-01T12:00:00Z".to_string(),
            "mvn compile".to_string(),
            "maven".to_string(),
            0,
            3000,
            Some("BUILD SUCCESS".to_string()),
            None,
            3,
            1,
            2,
            0,
            1,
            0.80,
            Some("PASSED".to_string()),
            0,
            None,
        );

        // Record from today
        let new_record = EnrichmentRunRecord::new(
            "stats-new".to_string(),
            "2024-06-01T12:00:00Z".to_string(),
            "mvn test".to_string(),
            "maven".to_string(),
            0,
            5000,
            Some("BUILD SUCCESS".to_string()),
            None,
            5,
            2,
            3,
            1,
            2,
            0.85,
            Some("PASSED".to_string()),
            1,
            None,
        );

        recorder.record(&old_record).await.unwrap();
        recorder.record(&new_record).await.unwrap();

        let stats = recorder.stats().await.unwrap();
        assert_eq!(stats.current_row_count, 2);
        assert_eq!(stats.oldest_record_ts, Some("2024-05-01T12:00:00Z".to_string()));
        assert_eq!(stats.newest_record_ts, Some("2024-06-01T12:00:00Z".to_string()));
    }
}
