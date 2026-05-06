//! SQLite-backed optimizer repository.
//!
//! Implements the `OptimizerRepository` trait for reading enrichment run records
//! and computing aggregate statistics.

use std::sync::Arc;

use enrichment_engine::models::EnrichmentRunRecord;
use enrichment_engine::optimizer::{AggregateStats, OptimizerRepository};
use enrichment_engine::traits::EnrichmentError;

use crate::enrichment::sqlite_recorder::SqliteRunRecorder;

/// SQLite-backed implementation of `OptimizerRepository`.
///
/// Reads from the same `enrichment_runs.db` used by `SqliteRunRecorder`.
#[derive(Debug, Clone)]
pub struct SqliteOptimizerRepository {
    recorder: Arc<SqliteRunRecorder>,
}

impl SqliteOptimizerRepository {
    /// Create a new optimizer repository backed by the same recorder.
    pub fn new(recorder: Arc<SqliteRunRecorder>) -> Self {
        Self { recorder }
    }
}

#[async_trait::async_trait]
impl OptimizerRepository for SqliteOptimizerRepository {
    /// Read all run records, optionally filtered to those after a given timestamp.
    async fn read_records(&self, after: Option<&str>) -> Result<Vec<EnrichmentRunRecord>, EnrichmentError> {
        let conn = self.recorder.conn.lock().await;

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

        while let Some(row) = rows.next().map_err(|e| EnrichmentError::Recorder(format!("Failed to fetch row: {}", e)))? {
            if let Ok(record) = row_to_record(row) {
                records.push(record);
            }
        }

        Ok(records)
    }

    /// Read all records for a specific enricher.
    async fn read_records_by_enricher(
        &self,
        enricher_id: &str,
    ) -> Result<Vec<EnrichmentRunRecord>, EnrichmentError> {
        let conn = self.recorder.conn.lock().await;

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
            .query_map([enricher_id], row_to_record)
            .map_err(|e| EnrichmentError::Recorder(format!("Failed to query records: {}", e)))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(records)
    }

    /// Compute aggregate statistics per enricher.
    async fn compute_statistics(&self) -> Result<Vec<AggregateStats>, EnrichmentError> {
        let records = self.read_records(None).await?;

        use std::collections::HashMap;
        let mut by_enricher: HashMap<String, Vec<EnrichmentRunRecord>> = HashMap::new();
        for record in records {
            by_enricher
                .entry(record.enricher_id.clone())
                .or_default()
                .push(record);
        }

        let stats: Vec<AggregateStats> = by_enricher
            .into_iter()
            .map(|(enricher_id, recs)| {
                enrichment_engine::optimizer::compute_aggregate_stats(&enricher_id, &recs)
            })
            .collect();

        Ok(stats)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use enrichment_engine::models::EnrichmentRunRecord;
    use enrichment_engine::traits::RunRecorder;
    use crate::enrichment::config::RetentionConfig;

    fn make_record(id: &str, enricher_id: &str, timestamp: &str) -> EnrichmentRunRecord {
        EnrichmentRunRecord::new(
            id.to_string(),
            timestamp.to_string(),
            "test command".to_string(),
            enricher_id.to_string(),
            0,
            1000,
            None,
            None,
            5,
            2,
            3,
            1,
            2,
            0.85,
            None,
            0,
            None,
        )
    }

    #[tokio::test]
    async fn test_read_records_empty() {
        let retention = RetentionConfig::default();
        let recorder = Arc::new(
            SqliteRunRecorder::with_retention_in_memory(retention).unwrap(),
        );
        let repo = SqliteOptimizerRepository::new(recorder);

        let records = repo.read_records(None).await.unwrap();
        assert!(records.is_empty());
    }

    #[tokio::test]
    async fn test_read_records_all() {
        let retention = RetentionConfig::default();
        let recorder = Arc::new(
            SqliteRunRecorder::with_retention_in_memory(retention).unwrap(),
        );
        let repo = SqliteOptimizerRepository::new(recorder.clone());

        // Insert some records
        let record = make_record("rec-1", "maven", "2024-01-01T00:00:00Z");
        recorder.record(&record).await.unwrap();

        let records = repo.read_records(None).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "rec-1");
    }

    #[tokio::test]
    async fn test_read_records_by_enricher() {
        let retention = RetentionConfig::default();
        let recorder = Arc::new(
            SqliteRunRecorder::with_retention_in_memory(retention).unwrap(),
        );
        let repo = SqliteOptimizerRepository::new(recorder.clone());

        // Insert records for different enrichers
        recorder.record(&make_record("rec-1", "maven", "2024-01-01T00:00:00Z"))
            .await
            .unwrap();
        recorder.record(&make_record("rec-2", "gradle", "2024-01-01T01:00:00Z"))
            .await
            .unwrap();
        recorder.record(&make_record("rec-3", "maven", "2024-01-01T02:00:00Z"))
            .await
            .unwrap();

        let maven_records = repo.read_records_by_enricher("maven").await.unwrap();
        assert_eq!(maven_records.len(), 2);

        let gradle_records = repo.read_records_by_enricher("gradle").await.unwrap();
        assert_eq!(gradle_records.len(), 1);
    }

    #[tokio::test]
    async fn test_compute_statistics() {
        let retention = RetentionConfig::default();
        let recorder = Arc::new(
            SqliteRunRecorder::with_retention_in_memory(retention).unwrap(),
        );
        let repo = SqliteOptimizerRepository::new(recorder.clone());

        // Insert multiple records for same enricher
        for i in 0..5 {
            let record = make_record(
                format!("rec-{}", i).as_str(),
                "maven",
                "2024-01-01T00:00:00Z",
            );
            recorder.record(&record).await.unwrap();
        }

        let stats = repo.compute_statistics().await.unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].enricher_id, "maven");
        assert_eq!(stats[0].total_runs, 5);
    }
}