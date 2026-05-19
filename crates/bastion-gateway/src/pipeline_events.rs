//! Pipeline event store and SSE endpoint for pipeline lifecycle events.
//!
//! Provides an in-memory store for pipeline events and SSE streaming to dashboard clients.

use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::sandbox::v2::PipelineEvent;

/// Stores pipeline events per pipeline_run_id and broadcasts to SSE subscribers.
#[derive(Clone)]
pub struct PipelineEventStore {
    /// Events indexed by pipeline_run_id
    events: Arc<DashMap<String, Vec<PipelineEvent>>>,
    /// Broadcast sender for SSE subscribers
    tx: broadcast::Sender<PipelineEvent>,
}

impl PipelineEventStore {
    /// Create a new PipelineEventStore
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            events: Arc::new(DashMap::new()),
            tx,
        }
    }

    /// Store an event and broadcast to subscribers
    pub fn push(&self, event: PipelineEvent) {
        let run_id = event.pipeline_run_id.clone();
        self.events.entry(run_id).or_default().push(event.clone());
        let _ = self.tx.send(event);
    }

    /// Subscribe to all pipeline events
    pub fn subscribe(&self) -> broadcast::Receiver<PipelineEvent> {
        self.tx.subscribe()
    }

    /// Get all events for a specific pipeline run
    #[allow(dead_code)]
    pub fn get_events(&self, pipeline_run_id: &str) -> Vec<PipelineEvent> {
        self.events
            .get(pipeline_run_id)
            .map(|e| e.clone())
            .unwrap_or_default()
    }
}

impl Default for PipelineEventStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a PipelineEvent to SSE-formatted string
pub fn event_to_sse(event: PipelineEvent) -> String {
    let payload_str = String::from_utf8_lossy(&event.payload).to_string();

    #[derive(serde::Serialize)]
    struct PipelineEventSse {
        event_type: String,
        pipeline_run_id: String,
        stage_name: String,
        step_name: String,
        step_index: u32,
        payload: String,
        timestamp_ms: u64,
        labels: std::collections::HashMap<String, String>,
    }

    let sse_event = PipelineEventSse {
        event_type: event.event_type,
        pipeline_run_id: event.pipeline_run_id,
        stage_name: event.stage_name,
        step_name: event.step_name,
        step_index: event.step_index,
        payload: payload_str,
        timestamp_ms: event.timestamp_ms,
        labels: event.labels,
    };

    match serde_json::to_string(&sse_event) {
        Ok(json) => format!("data: {}\n\n", json),
        Err(_) => String::new(),
    }
}
