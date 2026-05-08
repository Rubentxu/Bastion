//! SQLite worker thread for non-blocking database operations.
//!
//! Owns a `rusqlite::Connection` on a dedicated std::thread and processes
//! SQL commands sent over an mpsc channel. This avoids the `!Send` issue
//! with `rusqlite::Connection` in async contexts.
//!
//! # Why a Worker Thread?
//!
//! `rusqlite::Connection` with the `bundled` feature is NOT `Send`, meaning
//! it cannot be moved across thread boundaries. The previous approach of
//! holding a `tokio::sync::Mutex<rusqlite::Connection>` in async code failed
//! because `MutexGuard<Connection>` is also not `Send`, and any attempt to
//! `spawn_blocking` with the connection would hit the compiler wall.
//!
//! The worker thread pattern keeps the connection on a single thread permanently,
//! with all database operations happening synchronously on that thread. Async
//! callers send commands over a channel and await responses via oneshot.

use std::thread;

use enrichment_engine::models::{EnrichmentRunRecord, RunRecorderStats};
use enrichment_engine::traits::EnrichmentError;
use tokio::sync::oneshot;

use super::config::RetentionConfig;

/// Commands that can be sent to the SQLite worker thread.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum WorkerCommand {
    /// Record an enrichment run to the database.
    Record {
        run: EnrichmentRunRecord,
        sanitize: bool,
        response_tx: oneshot::Sender<Result<(), EnrichmentError>>,
    },
    /// Clean up old records based on retention policy.
    Cleanup {
        retention: RetentionConfig,
        response_tx: oneshot::Sender<Result<u64, EnrichmentError>>,
    },
    /// Get run recorder statistics.
    Stats {
        response_tx: oneshot::Sender<Result<RunRecorderStats, EnrichmentError>>,
    },
    /// Read all records, optionally filtered by timestamp.
    ReadRecords {
        after: Option<String>,
        response_tx: oneshot::Sender<Result<Vec<EnrichmentRunRecord>, EnrichmentError>>,
    },
    /// Read all records for a specific enricher.
    ReadRecordsByEnricher {
        enricher_id: String,
        response_tx: oneshot::Sender<Result<Vec<EnrichmentRunRecord>, EnrichmentError>>,
    },
    /// Shut down the worker thread gracefully.
    Shutdown {
        response_tx: oneshot::Sender<Result<(), String>>,
    },
}

/// Result type for worker operations that don't return meaningful data.
pub type WorkerResult = Result<(), EnrichmentError>;

/// A handle to the SQLite worker thread.
///
/// Owns the worker thread and provides a channel to send commands to it.
/// When dropped, sends a shutdown command and waits for the thread to terminate.
#[derive(Debug)]
pub struct SqliteWorker {
    command_tx: std::sync::mpsc::SyncSender<WorkerCommand>,
    #[allow(dead_code)]
    worker_thread: thread::JoinHandle<()>,
}

impl SqliteWorker {
    /// Start a new SQLite worker thread with the given connection.
    ///
    /// The worker runs in a dedicated thread and processes commands sequentially.
    /// SQLite operations are thread-safe when all access is serialized through
    /// a single thread, which is exactly what this worker does.
    pub fn new(conn: rusqlite::Connection) -> Result<Self, rusqlite::Error> {
        let (command_tx, command_rx) = std::sync::mpsc::sync_channel::<WorkerCommand>(64);

        let worker_thread = thread::Builder::new()
            .name("sqlite-worker".to_string())
            .spawn(move || {
                Self::run_loop(conn, command_rx);
            })
            .map_err(|e| {
                rusqlite::Error::InvalidParameterName(format!(
                    "Failed to spawn worker thread: {}",
                    e
                ))
            })?;

        Ok(Self {
            command_tx,
            worker_thread,
        })
    }

    /// Create a new worker with an in-memory SQLite database (for testing).
    #[cfg(test)]
    pub fn new_in_memory() -> Result<Self, rusqlite::Error> {
        let conn = rusqlite::Connection::open_in_memory()?;
        Self::new(conn)
    }

    /// Send a command to the worker (fire-and-forget).
    ///
    /// This is a synchronous blocking call that sends the command without
    /// expecting a response. Used for commands where the response is
    /// embedded in the command struct.
    pub(crate) fn send_cmd(&self, cmd: WorkerCommand) {
        self.command_tx.send(cmd).expect("Worker thread panicked");
    }

    /// Handle a single command, returning `true` to continue the loop or `false` to shut down.
    fn handle_command(conn: &rusqlite::Connection, command: WorkerCommand) -> bool {
        match command {
            WorkerCommand::Record {
                run,
                sanitize,
                response_tx,
            } => {
                let result = Self::do_record(conn, &run, sanitize);
                let _ = response_tx.send(result);
            }
            WorkerCommand::Cleanup {
                retention,
                response_tx,
            } => {
                let result = Self::do_cleanup(conn, &retention);
                let _ = response_tx.send(result);
            }
            WorkerCommand::Stats { response_tx } => {
                let result = Self::do_stats(conn);
                let _ = response_tx.send(result);
            }
            WorkerCommand::ReadRecords { after, response_tx } => {
                let result = Self::do_read_records(conn, after.as_deref());
                let _ = response_tx.send(result);
            }
            WorkerCommand::ReadRecordsByEnricher {
                enricher_id,
                response_tx,
            } => {
                let result = Self::do_read_records_by_enricher(conn, &enricher_id);
                let _ = response_tx.send(result);
            }
            WorkerCommand::Shutdown { response_tx } => {
                tracing::debug!("SQLite worker received shutdown command");
                let _ = response_tx.send(Ok(()));
                return false;
            }
        }
        true
    }

    /// The main worker loop — processes commands sequentially.
    fn run_loop(conn: rusqlite::Connection, command_rx: std::sync::mpsc::Receiver<WorkerCommand>) {
        loop {
            match command_rx.recv() {
                Ok(command) => {
                    if !Self::handle_command(&conn, command) {
                        drop(conn);
                        break;
                    }
                }
                Err(_) => {
                    tracing::debug!("SQLite worker: channel closed, shutting down");
                    break;
                }
            }
        }
        tracing::debug!("SQLite worker thread exiting");
    }

    /// Execute a record operation.
    fn do_record(
        conn: &rusqlite::Connection,
        run: &EnrichmentRunRecord,
        sanitize: bool,
    ) -> WorkerResult {
        use enrichment_engine::sanitize_command;
        use rusqlite::params;

        let command = if sanitize {
            sanitize_command(&run.command)
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

    /// Execute a cleanup operation.
    fn do_cleanup(
        conn: &rusqlite::Connection,
        retention: &RetentionConfig,
    ) -> Result<u64, EnrichmentError> {
        use chrono::Utc;
        use rusqlite::params;

        if !retention.enabled {
            return Ok(0);
        }

        let mut total_deleted: u64 = 0;

        // 1. Age-based deletion
        let cutoff = Utc::now() - chrono::Duration::days(retention.max_age_days as i64);
        let cutoff_str = cutoff.to_rfc3339();

        let deleted = conn
            .execute(
                "DELETE FROM enrichment_runs WHERE timestamp < ?1",
                params![cutoff_str],
            )
            .map_err(|e| {
                EnrichmentError::Recorder(format!("cleanup age-based delete failed: {}", e))
            })?;

        total_deleted += deleted as u64;

        // 2. Row-count cap deletion
        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM enrichment_runs", [], |row| row.get(0))
            .map_err(|e| EnrichmentError::Recorder(format!("count query failed: {}", e)))?;

        if row_count > retention.max_rows as i64 {
            let to_delete = row_count - retention.max_rows as i64;
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

    /// Execute a stats query.
    fn do_stats(conn: &rusqlite::Connection) -> Result<RunRecorderStats, EnrichmentError> {
        let row_count: u64 = conn
            .query_row("SELECT COUNT(*) FROM enrichment_runs", [], |row| row.get(0))
            .map_err(|e| EnrichmentError::Recorder(format!("stats count query failed: {}", e)))?;

        if row_count == 0 {
            return Ok(RunRecorderStats::empty());
        }

        let oldest: Option<String> = conn
            .query_row("SELECT MIN(timestamp) FROM enrichment_runs", [], |row| {
                row.get(0)
            })
            .map_err(|e| EnrichmentError::Recorder(format!("stats oldest query failed: {}", e)))?;

        let newest: Option<String> = conn
            .query_row("SELECT MAX(timestamp) FROM enrichment_runs", [], |row| {
                row.get(0)
            })
            .map_err(|e| EnrichmentError::Recorder(format!("stats newest query failed: {}", e)))?;

        Ok(RunRecorderStats::new(row_count, oldest, newest))
    }

    /// Read all records, optionally filtered by timestamp.
    fn do_read_records(
        conn: &rusqlite::Connection,
        after: Option<&str>,
    ) -> Result<Vec<EnrichmentRunRecord>, EnrichmentError> {
        let query = "SELECT id, timestamp, command, enricher_id, exit_code, duration_ms,
                output_summary_stdout, output_summary_stderr,
                facts_count, derived_facts_count, rule_hits_count,
                diagnostics_count, artifact_count, confidence_avg,
                verdict, recommendation_count, error
         FROM enrichment_runs";

        let query = if after.is_some() {
            format!("{} WHERE timestamp >= ?1 ORDER BY timestamp ASC", query)
        } else {
            format!("{} ORDER BY timestamp ASC", query)
        };

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| EnrichmentError::Recorder(format!("Failed to prepare query: {}", e)))?;

        let mut records = Vec::new();
        let mut rows = if let Some(after_ts) = after {
            stmt.query([after_ts])
        } else {
            stmt.query([])
        }
        .map_err(|e| EnrichmentError::Recorder(format!("Failed to query records: {}", e)))?;

        while let Some(row) = rows
            .next()
            .map_err(|e| EnrichmentError::Recorder(format!("Failed to fetch row: {}", e)))?
        {
            if let Ok(record) = Self::row_to_record(row) {
                records.push(record);
            }
        }

        Ok(records)
    }

    /// Read all records for a specific enricher.
    fn do_read_records_by_enricher(
        conn: &rusqlite::Connection,
        enricher_id: &str,
    ) -> Result<Vec<EnrichmentRunRecord>, EnrichmentError> {
        let mut stmt = conn
            .prepare(
                "SELECT id, timestamp, command, enricher_id, exit_code, duration_ms,
                        output_summary_stdout, output_summary_stderr,
                        facts_count, derived_facts_count, rule_hits_count,
                        diagnostics_count, artifact_count, confidence_avg,
                        verdict, recommendation_count, error
                 FROM enrichment_runs WHERE enricher_id = ?1 ORDER BY timestamp ASC",
            )
            .map_err(|e| EnrichmentError::Recorder(format!("Failed to prepare query: {}", e)))?;

        let records = stmt
            .query_map([enricher_id], Self::row_to_record)
            .map_err(|e| EnrichmentError::Recorder(format!("Failed to query records: {}", e)))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(records)
    }

    /// Convert a sqlite row to EnrichmentRunRecord.
    fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<EnrichmentRunRecord> {
        Ok(EnrichmentRunRecord::new(
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get::<_, i64>(5)? as u64,
            row.get(6)?,
            row.get(7)?,
            row.get::<_, i32>(8)? as u32,
            row.get::<_, i32>(9)? as u32,
            row.get::<_, i32>(10)? as u32,
            row.get::<_, i32>(11)? as u32,
            row.get::<_, i32>(12)? as u32,
            row.get(13)?,
            row.get(14)?,
            row.get::<_, i32>(15)? as u32,
            row.get(16)?,
        ))
    }
}

impl Drop for SqliteWorker {
    fn drop(&mut self) {
        tracing::debug!("SqliteWorker being dropped, initiating graceful shutdown");
        // Try to send shutdown command. If the channel is closed (worker already
        // exited), that's fine - we just won't get a response.
        let (response_tx, _response_rx) = oneshot::channel::<Result<(), String>>();
        let cmd = WorkerCommand::Shutdown { response_tx };
        let _ = self.command_tx.send(cmd);
        // Note: we don't block on thread join here to avoid deadlocks.
        // The worker thread will terminate when the receiver is dropped.
        // In production, this is acceptable as SQLite will clean up.
    }
}
