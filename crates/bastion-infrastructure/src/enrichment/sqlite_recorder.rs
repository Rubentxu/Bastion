//! SQLite-backed run recorder for persistence of enrichment telemetry.
//!
//! Implements the `RunRecorder` trait using rusqlite for storage.
//! Uses `tokio::sync::Mutex<Connection>` for async-safe concurrent access.

use async_trait::async_trait;
use rusqlite::params;
use std::path::Path;
use tokio::sync::Mutex;

use bastion_domain::shared::DomainError;
use enrichment_engine::models::EnrichmentRunRecord;
use enrichment_engine::traits::{EnrichmentError, RunRecorder};

/// SQLite-backed implementation of `RunRecorder`.
///
/// Persists `EnrichmentRunRecord`s to an SQLite database with an inline
/// `CREATE TABLE IF NOT EXISTS` schema.
#[derive(Debug)]
pub struct SqliteRunRecorder {
    conn: Mutex<rusqlite::Connection>,
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
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DomainError::Internal(format!("Failed to create DB directory: {}", e)))?;
        }

        let conn = rusqlite::Connection::open(db_path)
            .map_err(|e| DomainError::Internal(format!("Failed to open SQLite DB: {}", e)))?;

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
        })
    }

    /// Create a recorder backed by an in-memory database.
    ///
    /// This is intended for testing only. The schema is created automatically.
    #[cfg(test)]
    pub fn in_memory() -> Result<Self, DomainError> {
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
        })
    }
}

#[async_trait]
impl RunRecorder for SqliteRunRecorder {
    /// Persist an enrichment run record to the database.
    async fn record(&self, run: &EnrichmentRunRecord) -> Result<(), EnrichmentError> {
        let conn = self.conn.lock().await;

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
                run.command,
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
}
