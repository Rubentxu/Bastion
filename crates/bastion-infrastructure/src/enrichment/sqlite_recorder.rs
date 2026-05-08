//! SQLite-backed run recorder for persistence of enrichment telemetry.
//!
//! Implements the `RunRecorder` trait using a dedicated SQLite worker thread.
//! All database operations are executed synchronously on a single worker thread,
//! avoiding the `!Send` issue with `rusqlite::Connection` in async contexts.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::oneshot;

use bastion_domain::shared::DomainError;
use enrichment_engine::models::EnrichmentRunRecord;
use enrichment_engine::traits::{EnrichmentError, RunRecorder};

use super::config::RetentionConfig;
use super::sqlite_worker::{SqliteWorker, WorkerCommand};

/// SQLite-backed implementation of `RunRecorder`.
///
/// Persists `EnrichmentRunRecord`s to an SQLite database using a dedicated
/// worker thread. This avoids the `!Send` issue with `rusqlite::Connection`
/// in async contexts.
#[derive(Debug)]
pub struct SqliteRunRecorder {
    pub(crate) worker: Arc<SqliteWorker>,
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
    pub fn new(db_path: &std::path::Path) -> Result<Self, DomainError> {
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
    pub fn with_retention(
        db_path: &std::path::Path,
        retention: RetentionConfig,
    ) -> Result<Self, DomainError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                DomainError::Internal(format!("Failed to create DB directory: {}", e))
            })?;
        }

        let conn = rusqlite::Connection::open(db_path)
            .map_err(|e| DomainError::Internal(format!("Failed to open SQLite DB: {}", e)))?;

        // Run schema migrations before any schema creation
        // This ensures backward compatibility with existing databases
        super::schema_migration::SchemaMigration::run(&conn)?;

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

        let worker = SqliteWorker::new(conn)
            .map_err(|e| DomainError::Internal(format!("Failed to start SQLite worker: {}", e)))?;

        Ok(Self {
            worker: Arc::new(worker),
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

        let worker = SqliteWorker::new(conn)
            .map_err(|e| DomainError::Internal(format!("Failed to start SQLite worker: {}", e)))?;

        Ok(Self {
            worker: Arc::new(worker),
            retention,
        })
    }

    /// Internal helper to send a command to the worker and await the response.
    fn send_command<R: Send + 'static>(
        &self,
        make_cmd: impl FnOnce(oneshot::Sender<R>) -> WorkerCommand,
    ) -> oneshot::Receiver<R> {
        let (response_tx, response_rx) = oneshot::channel();
        let cmd = make_cmd(response_tx);

        self.worker.send_cmd(cmd);

        response_rx
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
        let run = run.clone();
        let sanitize = self.retention.sanitize;
        let response_rx = self.send_command(|ch| WorkerCommand::Record {
            run,
            sanitize,
            response_tx: ch,
        });

        response_rx
            .await
            .map_err(|e| EnrichmentError::Recorder(format!("Worker response error: {}", e)))?
    }

    fn retention_config(&self) -> &enrichment_engine::models::RetentionConfig {
        &self.retention
    }

    async fn cleanup(&self) -> Result<u64, EnrichmentError> {
        let response_rx = self.send_command(|ch| WorkerCommand::Cleanup {
            retention: self.retention.clone(),
            response_tx: ch,
        });

        response_rx
            .await
            .map_err(|e| EnrichmentError::Recorder(format!("Worker response error: {}", e)))?
    }

    async fn stats(&self) -> Result<enrichment_engine::models::RunRecorderStats, EnrichmentError> {
        let response_rx = self.send_command(|ch| WorkerCommand::Stats { response_tx: ch });

        response_rx
            .await
            .map_err(|e| EnrichmentError::Recorder(format!("Worker response error: {}", e)))?
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

        // Verify via stats that the record exists
        let stats = recorder.stats().await.unwrap();
        assert_eq!(stats.current_row_count, 1);
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

        // Verify via stats that the record exists
        let stats = recorder.stats().await.unwrap();
        assert_eq!(stats.current_row_count, 1);
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

        let stats = recorder.stats().await.unwrap();
        assert_eq!(stats.current_row_count, 5);
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

        // Should still be 1 (replaced)
        let stats = recorder.stats().await.unwrap();
        assert_eq!(stats.current_row_count, 1);
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
        let stats = recorder.stats().await.unwrap();
        assert_eq!(stats.current_row_count, 1);
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

        let stats = recorder.stats().await.unwrap();
        assert_eq!(stats.current_row_count, 5);
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

        let stats = recorder.stats().await.unwrap();
        assert_eq!(stats.current_row_count, 5);
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
        assert_eq!(
            stats.oldest_record_ts,
            Some("2024-06-01T12:00:00Z".to_string())
        );
        assert_eq!(
            stats.newest_record_ts,
            Some("2024-06-01T12:00:00Z".to_string())
        );
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
        assert_eq!(
            stats.oldest_record_ts,
            Some("2024-05-01T12:00:00Z".to_string())
        );
        assert_eq!(
            stats.newest_record_ts,
            Some("2024-06-01T12:00:00Z".to_string())
        );
    }
}
