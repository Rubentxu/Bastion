//! SQLite-backed experience store.
//!
//! Provides persistent storage for experience records using rusqlite.

use std::path::Path;

use async_trait::async_trait;
use tokio::sync::Mutex;
use rusqlite::params;

use bastion_domain::catalog::experience::{ExperienceRecord, ExperienceStatus, ExperienceStore};
use bastion_domain::shared::{DomainError, id::SandboxId};

/// SQLite-backed implementation of `ExperienceStore`.
#[derive(Debug)]
pub struct SqliteExperienceStore {
    conn: Mutex<rusqlite::Connection>,
    #[allow(dead_code)]
    db_path: std::path::PathBuf,
}

impl SqliteExperienceStore {
    /// Create a new store, creating the DB schema if it doesn't exist.
    /// Use `:memory:` for an in-memory database (testing).
    pub fn new(db_path: &Path) -> Result<Self, DomainError> {
        let is_memory = db_path.to_str() == Some(":memory:");

        if !is_memory {
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| DomainError::Internal(format!("Failed to create DB directory: {}", e)))?;
            }
        }

        let conn = if is_memory {
            rusqlite::Connection::open(db_path)
        } else {
            rusqlite::Connection::open(db_path)
        }
        .map_err(|e| DomainError::Internal(format!("Failed to open SQLite DB: {}", e)))?;

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS experiences (
                id          TEXT PRIMARY KEY,
                trace_id    TEXT,
                tool        TEXT NOT NULL,
                sandbox_id  TEXT,
                started_at  TEXT NOT NULL,
                ended_at    TEXT,
                exit_code   INTEGER,
                stdout_summary TEXT NOT NULL DEFAULT '',
                stderr_summary TEXT NOT NULL DEFAULT '',
                status      TEXT NOT NULL,
                metadata    TEXT NOT NULL DEFAULT '{}'
            );
            CREATE INDEX IF NOT EXISTS idx_experiences_trace_id ON experiences(trace_id);
            CREATE INDEX IF NOT EXISTS idx_experiences_sandbox_id ON experiences(sandbox_id);
            "#,
        )
        .map_err(|e| DomainError::Internal(format!("Failed to create schema: {}", e)))?;

        Ok(Self {
            conn: Mutex::new(conn),
            db_path: db_path.to_path_buf(),
        })
    }

    async fn insert(&self, record: &ExperienceRecord) -> Result<(), DomainError> {
        let conn = self.conn.lock().await;
        conn.execute(
            r#"INSERT INTO experiences
               (id, trace_id, tool, sandbox_id, started_at, ended_at, exit_code,
                stdout_summary, stderr_summary, status, metadata)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
            params![
                record.id,
                record.trace_id,
                record.tool_name,
                record.sandbox_id.as_ref().map(|s| s.to_string()),
                record.started_at.to_rfc3339(),
                record.finished_at.map(|dt| dt.to_rfc3339()),
                record.exit_code,
                record.stdout_summary,
                record.stderr_summary,
                format!("{:?}", record.status).to_lowercase(),
                serde_json::to_string(&record.metadata)
                    .unwrap_or_else(|_| "{}".to_string()),
            ],
        )
        .map_err(|e| DomainError::Internal(format!("Failed to insert experience: {}", e)))?;
        Ok(())
    }
}

#[async_trait]
impl ExperienceStore for SqliteExperienceStore {
    async fn save(&self, record: &ExperienceRecord) -> Result<(), DomainError> {
        self.insert(record).await
    }

    async fn find_by_id(&self, id: &str) -> Result<Option<ExperienceRecord>, DomainError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, trace_id, tool, sandbox_id, started_at, ended_at, exit_code,
                        stdout_summary, stderr_summary, status, metadata
                 FROM experiences WHERE id = ?1",
            )
            .map_err(|e| DomainError::Internal(format!("Failed to prepare: {}", e)))?;

        let result = stmt.query_row(params![id], |row| {
            Ok(ExperienceRow {
                id: row.get(0)?,
                trace_id: row.get(1)?,
                tool: row.get(2)?,
                sandbox_id: row.get(3)?,
                started_at: row.get(4)?,
                ended_at: row.get(5)?,
                exit_code: row.get(6)?,
                stdout_summary: row.get(7)?,
                stderr_summary: row.get(8)?,
                status: row.get(9)?,
                metadata: row.get(10)?,
            })
        });

        match result {
            Ok(row) => Ok(Some(row_to_experience(row)?)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DomainError::Internal(format!("Query failed: {}", e))),
        }
    }

    async fn find_by_trace_id(&self, trace_id: &str) -> Result<Vec<ExperienceRecord>, DomainError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, trace_id, tool, sandbox_id, started_at, ended_at, exit_code,
                        stdout_summary, stderr_summary, status, metadata
                 FROM experiences
                 WHERE trace_id = ?1
                 ORDER BY started_at DESC",
            )
            .map_err(|e| DomainError::Internal(format!("Failed to prepare: {}", e)))?;

        let rows = stmt
            .query_map(params![trace_id], |row| {
                Ok(ExperienceRow {
                    id: row.get(0)?,
                    trace_id: row.get(1)?,
                    tool: row.get(2)?,
                    sandbox_id: row.get(3)?,
                    started_at: row.get(4)?,
                    ended_at: row.get(5)?,
                    exit_code: row.get(6)?,
                    stdout_summary: row.get(7)?,
                    stderr_summary: row.get(8)?,
                    status: row.get(9)?,
                    metadata: row.get(10)?,
                })
            })
            .map_err(|e| DomainError::Internal(format!("Failed to query: {}", e)))?;

        let mut records = Vec::new();
        for row in rows {
            let row = row.map_err(|e| DomainError::Internal(format!("Failed to read row: {}", e)))?;
            records.push(row_to_experience(row)?);
        }
        Ok(records)
    }

    async fn list_all(&self, limit: usize) -> Result<Vec<ExperienceRecord>, DomainError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, trace_id, tool, sandbox_id, started_at, ended_at, exit_code,
                        stdout_summary, stderr_summary, status, metadata
                 FROM experiences
                 ORDER BY started_at DESC
                 LIMIT ?1",
            )
            .map_err(|e| DomainError::Internal(format!("Failed to prepare: {}", e)))?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(ExperienceRow {
                    id: row.get(0)?,
                    trace_id: row.get(1)?,
                    tool: row.get(2)?,
                    sandbox_id: row.get(3)?,
                    started_at: row.get(4)?,
                    ended_at: row.get(5)?,
                    exit_code: row.get(6)?,
                    stdout_summary: row.get(7)?,
                    stderr_summary: row.get(8)?,
                    status: row.get(9)?,
                    metadata: row.get(10)?,
                })
            })
            .map_err(|e| DomainError::Internal(format!("Failed to query: {}", e)))?;

        let mut records = Vec::new();
        for row in rows {
            let row = row.map_err(|e| DomainError::Internal(format!("Failed to read row: {}", e)))?;
            records.push(row_to_experience(row)?);
        }
        Ok(records)
    }
}

/// Helper struct for reading an experience row from SQLite.
#[derive(Debug)]
struct ExperienceRow {
    id: String,
    trace_id: Option<String>,
    tool: String,
    sandbox_id: Option<String>,
    started_at: String,
    ended_at: Option<String>,
    exit_code: Option<i32>,
    stdout_summary: String,
    stderr_summary: String,
    status: String,
    metadata: String,
}

fn row_to_experience(row: ExperienceRow) -> Result<ExperienceRecord, DomainError> {
    let status = serde_json::from_str::<ExperienceStatus>(&format!("\"{}\"", row.status))
        .map_err(|e| DomainError::Internal(format!("Failed to parse status '{}': {}", row.status, e)))?;

    let metadata: serde_json::Value = serde_json::from_str(&row.metadata)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    let started_at = chrono::DateTime::parse_from_rfc3339(&row.started_at)
        .map_err(|e| DomainError::Internal(format!("Failed to parse started_at: {}", e)))?
        .with_timezone(&chrono::Utc);

    let ended_at = row.ended_at.map(|s| {
        chrono::DateTime::parse_from_rfc3339(&s)
            .map(|dt| dt.with_timezone(&chrono::Utc))
    }).transpose()
    .map_err(|e| DomainError::Internal(format!("Failed to parse ended_at: {}", e)))?;

    let sandbox_id = row.sandbox_id.map(|s| SandboxId::new(s));

    Ok(ExperienceRecord {
        id: row.id,
        trace_id: row.trace_id,
        tool_name: row.tool,
        sandbox_id,
        started_at,
        finished_at: ended_at,
        exit_code: row.exit_code,
        stdout_summary: row.stdout_summary,
        stderr_summary: row.stderr_summary,
        status,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_store() -> SqliteExperienceStore {
        SqliteExperienceStore::new(std::path::Path::new(":memory:")).unwrap()
    }

    #[tokio::test]
    async fn test_save_and_find_by_id() {
        let store = create_test_store();
        let record = ExperienceRecord::new("sandbox_run")
            .with_trace_id("trace-1")
            .completed(0)
            .with_stdout(b"hello")
            .with_stderr(b"");

        store.save(&record).await.unwrap();

        let found = store.find_by_id(&record.id).await.unwrap();
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.trace_id, Some("trace-1".to_string()));
        assert_eq!(found.exit_code, Some(0));
    }

    #[tokio::test]
    async fn test_find_by_trace_id() {
        let store = create_test_store();

        let r1 = ExperienceRecord::new("sandbox_run").with_trace_id("t1").completed(0).with_stdout(b"a");
        let r2 = ExperienceRecord::new("sandbox_run").with_trace_id("t1").completed(1).with_stdout(b"b");
        let r3 = ExperienceRecord::new("sandbox_run").with_trace_id("t2").completed(0).with_stdout(b"c");

        store.save(&r1).await.unwrap();
        store.save(&r2).await.unwrap();
        store.save(&r3).await.unwrap();

        let results = store.find_by_trace_id("t1").await.unwrap();
        assert_eq!(results.len(), 2);

        let results_t2 = store.find_by_trace_id("t2").await.unwrap();
        assert_eq!(results_t2.len(), 1);
    }

    #[tokio::test]
    async fn test_list_all() {
        let store = create_test_store();
        for i in 0..5 {
            let record = ExperienceRecord::new("sandbox_run")
                .with_trace_id(format!("trace-{}", i))
                .completed(0)
                .with_stdout(b"test");
            store.save(&record).await.unwrap();
        }

        let all = store.list_all(3).await.unwrap();
        assert_eq!(all.len(), 3);
    }
}
