//! SQLite schema for test metrics collection.
//!
//! Table: `test_runs`
//! - id: INTEGER PRIMARY KEY
//! - test_name: TEXT NOT NULL
//! - crate_name: TEXT NOT NULL
//! - file_path: TEXT NOT NULL
//! - timestamp: TEXT NOT NULL (ISO8601)
//! - duration_ms: INTEGER NOT NULL
//! - status: TEXT NOT NULL ("pass" | "fail")
//! - build_hash: TEXT NOT NULL

use rusqlite::{Connection, Result as SqliteResult};

/// Initialize the database schema — creates `test_runs` table and indexes.
pub fn init_schema(conn: &Connection) -> SqliteResult<()> {
    conn.execute(
        r#"
        CREATE TABLE IF NOT EXISTS test_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            test_name TEXT NOT NULL,
            crate_name TEXT NOT NULL,
            file_path TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            duration_ms INTEGER NOT NULL,
            status TEXT NOT NULL,
            build_hash TEXT NOT NULL
        )
        "#,
        [],
    )?;

    // Unique index on (test_name, crate_name, file_path, build_hash, timestamp)
    // to avoid duplicate entries for the same test run
    conn.execute(
        r#"
        CREATE UNIQUE INDEX IF NOT EXISTS idx_test_runs_unique
        ON test_runs(test_name, crate_name, file_path, build_hash, timestamp)
        "#,
        [],
    )?;

    // Index for latency queries — order by timestamp for time-series analysis
    conn.execute(
        r#"
        CREATE INDEX IF NOT EXISTS idx_test_runs_latency
        ON test_runs(test_name, crate_name, duration_ms)
        "#,
        [],
    )?;

    Ok(())
}

/// Record a single test run to the database.
#[allow(clippy::too_many_arguments)]
pub fn record_run(
    conn: &Connection,
    test_name: &str,
    crate_name: &str,
    file_path: &str,
    timestamp: &str,
    duration_ms: u64,
    status: &str,
    build_hash: &str,
) -> SqliteResult<()> {
    use rusqlite::params;

    conn.execute(
        r#"
        INSERT INTO test_runs
            (test_name, crate_name, file_path, timestamp, duration_ms, status, build_hash)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        "#,
        params![
            test_name,
            crate_name,
            file_path,
            timestamp,
            duration_ms as i64,
            status,
            build_hash,
        ],
    )?;

    Ok(())
}
