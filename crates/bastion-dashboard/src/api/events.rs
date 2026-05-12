//! Server-Sent Events (SSE) handling.
//!
//! Provides types and parsing for SSE events from the dashboard API.

use serde::{Deserialize, Serialize};

use crate::error::SseError;

/// SSE event types supported by the dashboard API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SseEvent {
    /// A new sandbox was created.
    SandboxCreated {
        sandbox_id: String,
        project_id: String,
        purpose: String,
    },

    /// A sandbox was terminated.
    SandboxTerminated {
        sandbox_id: String,
        reason: Option<String>,
    },

    /// Sandbox status changed.
    SandboxStatusChanged {
        sandbox_id: String,
        old_status: String,
        new_status: String,
    },

    /// Metrics update for a sandbox.
    MetricsUpdate {
        sandbox_id: String,
        cpu_percent: f64,
        memory_mb: u64,
    },

    /// Pool-related event.
    PoolEvent {
        pool_id: String,
        action: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<String>,
    },

    /// Unknown or unsupported event type.
    Unknown {
        event_type: String,
        raw_data: String,
    },
}

impl SseEvent {
    /// Parse an SSE event from raw SSE text (event type + data lines).
    ///
    /// SSE format:
    /// event: <type>\n
    /// data: <json>\n
    /// \n (blank line terminates event)
    pub fn parse(lines: &[String]) -> Result<Option<Self>, SseError> {
        let mut event_type: Option<String> = None;
        let mut data_lines: Vec<String> = Vec::new();

        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                // Empty line marks end of event
                if event_type.is_some() && !data_lines.is_empty() {
                    let data = data_lines.join("\n");
                    let parsed = Self::parse_event(event_type.as_deref(), &data)?;
                    return Ok(Some(parsed));
                }
                event_type = None;
                data_lines.clear();
                continue;
            }

            if let Some(prefix) = line.strip_prefix("event:") {
                event_type = Some(prefix.trim().to_string());
            } else if let Some(prefix) = line.strip_prefix("data:") {
                data_lines.push(prefix.trim().to_string());
            }
        }

        Ok(None)
    }

    /// Parse a single event given its type and data.
    fn parse_event(event_type: Option<&str>, data: &str) -> Result<Self, SseError> {
        let event_type = event_type.unwrap_or("unknown");

        match event_type {
            "sandbox_created" => {
                #[derive(Deserialize)]
                struct CreatedData {
                    sandbox_id: String,
                    project_id: String,
                    purpose: String,
                }
                let parsed: CreatedData = serde_json::from_str(data)
                    .map_err(|e| SseError::ParseError(format!("Invalid JSON: {}", e)))?;
                Ok(Self::SandboxCreated {
                    sandbox_id: parsed.sandbox_id,
                    project_id: parsed.project_id,
                    purpose: parsed.purpose,
                })
            }
            "sandbox_terminated" => {
                #[derive(Deserialize)]
                struct TerminatedData {
                    sandbox_id: String,
                    reason: Option<String>,
                }
                let parsed: TerminatedData = serde_json::from_str(data)
                    .map_err(|e| SseError::ParseError(format!("Invalid JSON: {}", e)))?;
                Ok(Self::SandboxTerminated {
                    sandbox_id: parsed.sandbox_id,
                    reason: parsed.reason,
                })
            }
            "sandbox_status_changed" => {
                #[derive(Deserialize)]
                struct StatusData {
                    sandbox_id: String,
                    old_status: String,
                    new_status: String,
                }
                let parsed: StatusData = serde_json::from_str(data)
                    .map_err(|e| SseError::ParseError(format!("Invalid JSON: {}", e)))?;
                Ok(Self::SandboxStatusChanged {
                    sandbox_id: parsed.sandbox_id,
                    old_status: parsed.old_status,
                    new_status: parsed.new_status,
                })
            }
            "metrics_update" => {
                #[derive(Deserialize)]
                struct MetricsData {
                    sandbox_id: String,
                    cpu_percent: f64,
                    memory_mb: u64,
                }
                let parsed: MetricsData = serde_json::from_str(data)
                    .map_err(|e| SseError::ParseError(format!("Invalid JSON: {}", e)))?;
                Ok(Self::MetricsUpdate {
                    sandbox_id: parsed.sandbox_id,
                    cpu_percent: parsed.cpu_percent,
                    memory_mb: parsed.memory_mb,
                })
            }
            "pool_event" => {
                #[derive(Deserialize)]
                struct PoolData {
                    pool_id: String,
                    action: String,
                    details: Option<String>,
                }
                let parsed: PoolData = serde_json::from_str(data)
                    .map_err(|e| SseError::ParseError(format!("Invalid JSON: {}", e)))?;
                Ok(Self::PoolEvent {
                    pool_id: parsed.pool_id,
                    action: parsed.action,
                    details: parsed.details,
                })
            }
            _ => Ok(Self::Unknown {
                event_type: event_type.to_string(),
                raw_data: data.to_string(),
            }),
        }
    }
}

/// Parse a stream of SSE data into events.
///
/// Each event in the stream is separated by a blank line (double newline).
/// Each line within an event ends with a single newline.
pub fn parse_sse_stream(stream_data: &[u8]) -> Result<Vec<SseEvent>, SseError> {
    let text = String::from_utf8_lossy(stream_data);
    let mut events = Vec::new();

    // Process each event block (separated by blank lines)
    for block in text.split("\n\n") {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }

        // Split into individual lines
        let mut lines: Vec<String> = block.lines().map(|s| s.to_string()).collect();

        // Add empty line at end to signal event completion to parser
        lines.push(String::new());

        if let Some(event) = SseEvent::parse(&lines)? {
            events.push(event);
        }
    }

    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sandbox_created_event() {
        let lines = vec![
            "event: sandbox_created".to_string(),
            "data: {\"sandbox_id\": \"sb-123\", \"project_id\": \"proj-1\", \"purpose\": \"adhoc_test\"}".to_string(),
            "".to_string(),
        ];

        let event = SseEvent::parse(&lines).unwrap().unwrap();
        match event {
            SseEvent::SandboxCreated { sandbox_id, project_id, purpose } => {
                assert_eq!(sandbox_id, "sb-123");
                assert_eq!(project_id, "proj-1");
                assert_eq!(purpose, "adhoc_test");
            }
            other => panic!("Expected SandboxCreated, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_metrics_update_event() {
        let lines = vec![
            "event: metrics_update".to_string(),
            "data: {\"sandbox_id\": \"sb-123\", \"cpu_percent\": 45.5, \"memory_mb\": 256}".to_string(),
            "".to_string(),
        ];

        let event = SseEvent::parse(&lines).unwrap().unwrap();
        match event {
            SseEvent::MetricsUpdate { sandbox_id, cpu_percent, memory_mb } => {
                assert_eq!(sandbox_id, "sb-123");
                assert!((cpu_percent - 45.5).abs() < 0.01);
                assert_eq!(memory_mb, 256);
            }
            other => panic!("Expected MetricsUpdate, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_unknown_event() {
        let lines = vec![
            "event: custom_event".to_string(),
            "data: some raw data".to_string(),
            "".to_string(),
        ];

        let event = SseEvent::parse(&lines).unwrap().unwrap();
        match event {
            SseEvent::Unknown { event_type, raw_data } => {
                assert_eq!(event_type, "custom_event");
                assert_eq!(raw_data, "some raw data");
            }
            other => panic!("Expected Unknown, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_empty_lines() {
        let lines: Vec<String> = vec![];
        let result = SseEvent::parse(&lines).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_sse_stream() {
        // Two events separated by blank line
        let data = "event: sandbox_created\ndata: {\"sandbox_id\": \"sb-1\", \"project_id\": \"p-1\", \"purpose\": \"adhoc\"}\n\nevent: metrics_update\ndata: {\"sandbox_id\": \"sb-1\", \"cpu_percent\": 10.0, \"memory_mb\": 128}\n\n";

        let events = parse_sse_stream(data.as_bytes()).unwrap();
        assert_eq!(events.len(), 2);

        match &events[0] {
            SseEvent::SandboxCreated { sandbox_id, .. } => {
                assert_eq!(sandbox_id, "sb-1");
            }
            _ => panic!("Expected SandboxCreated"),
        }

        match &events[1] {
            SseEvent::MetricsUpdate { sandbox_id, .. } => {
                assert_eq!(sandbox_id, "sb-1");
            }
            _ => panic!("Expected MetricsUpdate"),
        }
    }
}
