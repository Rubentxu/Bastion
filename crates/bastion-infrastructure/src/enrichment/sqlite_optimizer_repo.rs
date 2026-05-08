//! SQLite-backed optimizer repository.
//!
//! Implements the `OptimizerRepository` trait for reading enrichment run records
//! and computing aggregate statistics.

use std::sync::Arc;

use enrichment_engine::models::EnrichmentRunRecord;
use enrichment_engine::optimizer::{AggregateStats, OptimizerRepository};
use enrichment_engine::traits::EnrichmentError;
use tokio::sync::oneshot;

use crate::enrichment::sqlite_recorder::SqliteRunRecorder;
use crate::enrichment::sqlite_worker::WorkerCommand;

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
    async fn read_records(
        &self,
        after: Option<&str>,
    ) -> Result<Vec<EnrichmentRunRecord>, EnrichmentError> {
        let after_opt = after.map(String::from);

        let (response_tx, response_rx) = oneshot::channel();

        let cmd = WorkerCommand::ReadRecords {
            after: after_opt,
            response_tx,
        };

        self.recorder.worker.send_cmd(cmd);

        response_rx
            .await
            .map_err(|e| EnrichmentError::Recorder(format!("Worker response error: {}", e)))?
    }

    /// Read all records for a specific enricher.
    async fn read_records_by_enricher(
        &self,
        enricher_id: &str,
    ) -> Result<Vec<EnrichmentRunRecord>, EnrichmentError> {
        let enricher_id = enricher_id.to_string();

        let (response_tx, response_rx) = oneshot::channel();

        let cmd = WorkerCommand::ReadRecordsByEnricher {
            enricher_id,
            response_tx,
        };

        self.recorder.worker.send_cmd(cmd);

        response_rx
            .await
            .map_err(|e| EnrichmentError::Recorder(format!("Worker response error: {}", e)))?
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrichment::config::RetentionConfig;
    use enrichment_engine::models::EnrichmentRunRecord;
    use enrichment_engine::traits::RunRecorder;

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
        let recorder = Arc::new(SqliteRunRecorder::with_retention_in_memory(retention).unwrap());
        let repo = SqliteOptimizerRepository::new(recorder);

        let records = repo.read_records(None).await.unwrap();
        assert!(records.is_empty());
    }

    #[tokio::test]
    async fn test_read_records_all() {
        let retention = RetentionConfig::default();
        let recorder = Arc::new(SqliteRunRecorder::with_retention_in_memory(retention).unwrap());
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
        let recorder = Arc::new(SqliteRunRecorder::with_retention_in_memory(retention).unwrap());
        let repo = SqliteOptimizerRepository::new(recorder.clone());

        // Insert records for different enrichers
        recorder
            .record(&make_record("rec-1", "maven", "2024-01-01T00:00:00Z"))
            .await
            .unwrap();
        recorder
            .record(&make_record("rec-2", "gradle", "2024-01-01T01:00:00Z"))
            .await
            .unwrap();
        recorder
            .record(&make_record("rec-3", "maven", "2024-01-01T02:00:00Z"))
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
        let recorder = Arc::new(SqliteRunRecorder::with_retention_in_memory(retention).unwrap());
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
