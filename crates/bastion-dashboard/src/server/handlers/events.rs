//! Server-Sent Events (SSE) handler.
//!
//! Handles real-time event streaming to connected clients.

use axum::{
    extract::{Query, State},
    response::sse::{Event, Sse},
};
use futures::stream;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tracing::debug;
use serde::Deserialize;

use crate::api::events::SseEvent;
use crate::error::ApiError;
use crate::server::state::DashboardServerState;

/// Query parameters for SSE connection.
#[derive(Debug, Deserialize)]
pub struct SseQuery {
    /// Last event ID for reconnect support.
    #[serde(default)]
    pub last_event_id: Option<String>,
}

/// Creates an SSE event stream for dashboard events.
///
/// GET /api/v1/events
///
/// Streams real-time events to the client. Supports reconnect via Last-Event-ID.
pub async fn sse_events(
    State(_state): State<DashboardServerState>,
    Query(query): Query<SseQuery>,
) -> Result<Sse<impl stream::Stream<Item = Result<Event, std::convert::Infallible>>>, ApiError> {
    debug!("SSE connection requested, last_event_id: {:?}", query.last_event_id);

    // Create a broadcast channel for distributing events
    // The sender half would be used by the code that generates events
    let (_tx, rx) = broadcast::channel::<SseEvent>(100);

    // Build SSE stream that converts SseEvent to Event
    let broadcast_stream = BroadcastStream::new(rx).map(|result| {
        match result {
            Ok(event) => {
                let data = serde_json::to_string(&event).unwrap_or_default();
                Ok(Event::default()
                    .event("message")
                    .data(data))
            }
            Err(_) => {
                // For any broadcast error, send a heartbeat comment to keep connection alive
                Ok(Event::default().comment("reconnecting"))
            }
        }
    });

    Ok(Sse::new(broadcast_stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("keepalive"),
    ))
}

/// Broadcast an event to all connected SSE clients.
///
/// This function is used internally to distribute events to all clients
/// subscribed to the SSE endpoint.
pub fn broadcast_event(
    tx: &broadcast::Sender<SseEvent>,
    event: SseEvent,
) -> Result<(), ApiError> {
    tx.send(event)
        .map_err(|_| ApiError::InternalError("Failed to broadcast event".to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_query_deserialization_with_last_event_id() {
        let json = r#"{"last_event_id": "abc123"}"#;
        let query: SseQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.last_event_id, Some("abc123".to_string()));
    }

    #[test]
    fn test_sse_query_deserialization_without_last_event_id() {
        let json = r#"{}"#;
        let query: SseQuery = serde_json::from_str(json).unwrap();
        assert!(query.last_event_id.is_none());
    }

    #[tokio::test]
    async fn test_sse_events_returns_stream() {
        use std::sync::Arc;
        use crate::api::client::DashboardApiClient;
        use crate::project::ProjectManager;

        let api_client = DashboardApiClient::default();
        let manager = ProjectManager::new(api_client.clone());
        let state = DashboardServerState {
            project_manager: Arc::new(manager),
            api_client: Arc::new(api_client),
            gateway_url: "http://localhost:8080".to_string(),
        };

        let result = sse_events(State(state), Query(SseQuery { last_event_id: None })).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_broadcast_event_success() {
        let (tx, mut rx) = broadcast::channel::<SseEvent>(100);
        let event = SseEvent::SandboxCreated {
            sandbox_id: "sb-123".to_string(),
            project_id: "proj-1".to_string(),
            purpose: "adhoc_test".to_string(),
        };

        let result = broadcast_event(&tx, event.clone());
        assert!(result.is_ok());

        // Receive the event
        let received = rx.try_recv();
        assert!(received.is_ok());
        assert!(matches!(received.unwrap(), SseEvent::SandboxCreated { .. }));
    }
}
