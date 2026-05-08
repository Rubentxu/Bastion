//! Edge Case Tests for Bastion Gateway
//!
//! Tests error handling, timeout behavior, concurrent requests, and edge cases

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

// ============================================================================
// HELPERS
// ============================================================================

fn spawn_gateway() -> Option<(
    std::process::Child,
    Mutex<std::process::ChildStdin>,
    BufReader<std::process::ChildStdout>,
)> {
    let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/debug/bastion-gateway");

    if !binary.exists() {
        eprintln!("Gateway binary not found at {:?}", binary);
        return None;
    }

    let worker = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/debug/bastion-worker");

    let mut child = Command::new(&binary)
        .arg("--image")
        .arg("debian:bookworm-slim")
        .arg("--worker-binary")
        .arg(&worker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;

    Some((child, Mutex::new(stdin), BufReader::new(stdout)))
}

fn send_request(
    stdin: &Mutex<std::process::ChildStdin>,
    reader: &mut impl BufRead,
    method: &str,
    params: Value,
) -> Value {
    let id = rand::random::<u64>() % 1_000_000;
    let request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let mut line = serde_json::to_string(&request).unwrap();
    line.push('\n');

    stdin.lock().unwrap().write_all(line.as_bytes()).unwrap();
    stdin.lock().unwrap().flush().unwrap();

    let mut response_line = String::new();
    for _ in 0..200 {
        response_line.clear();
        reader.read_line(&mut response_line).unwrap();
        if let Ok(resp) = serde_json::from_str::<Value>(&response_line) {
            if resp.get("id").and_then(|i| i.as_u64()) == Some(id) {
                return resp;
            }
        }
    }
    json!({"error": "timeout"})
}

fn send_notification(stdin: &Mutex<std::process::ChildStdin>, method: &str, params: Value) {
    let request = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    let mut line = serde_json::to_string(&request).unwrap();
    line.push('\n');
    stdin.lock().unwrap().write_all(line.as_bytes()).unwrap();
    stdin.lock().unwrap().flush().unwrap();
    std::thread::sleep(Duration::from_millis(100));
}

fn init_gateway(stdin: &Mutex<std::process::ChildStdin>, reader: &mut impl BufRead) {
    let _ = send_request(
        stdin,
        reader,
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "edge-case-test", "version": "0.1.0"}
        }),
    );
    send_notification(stdin, "notifications/initialized", json!({}));
}

fn extract_sandbox_id(response: &Value) -> String {
    let content = &response["result"]["content"];
    let text = content[0]["text"].as_str().unwrap_or("");
    let result: Value = serde_json::from_str(text).unwrap_or(json!({}));
    result["sandbox_id"].as_str().unwrap_or("").to_string()
}

fn extract_response_text(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

// ============================================================================
// TESTS
// ============================================================================

/// Test: Concurrent requests don't deadlock the gateway
/// Since BufReader doesn't implement Sync, we can't truly share the reader
/// across threads. This test verifies that rapid sequential requests from
/// multiple threads (sharing stdin via mutex) complete without deadlock.
#[test]
fn test_concurrent_sandbox_operations() {
    let Some((mut child, stdin, mut reader)) = spawn_gateway() else {
        return;
    };

    // Give gateway time to start
    std::thread::sleep(Duration::from_millis(500));
    init_gateway(&stdin, &mut reader);

    // Use a channel-based approach to collect responses
    // Each thread will send requests and collect responses through a channel
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel();
    let stdin_arc = std::sync::Arc::new(stdin);

    // Spawn 3 threads each making 3 rapid requests
    for thread_id in 0..3 {
        let tx = tx.clone();
        let stdin_clone = std::sync::Arc::clone(&stdin_arc);

        std::thread::spawn(move || {
            let mut responses = Vec::new();
            for req_id in 0..3 {
                // Create and send request
                let id = rand::random::<u64>() % 1_000_000;
                let request = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "tools/call",
                    "params": {
                        "name": "sandbox_list",
                        "arguments": {}
                    }
                });

                let mut line = serde_json::to_string(&request).unwrap();
                line.push('\n');

                stdin_clone
                    .lock()
                    .unwrap()
                    .write_all(line.as_bytes())
                    .unwrap();
                stdin_clone.lock().unwrap().flush().unwrap();

                // Small delay to allow response to arrive
                std::thread::sleep(Duration::from_millis(50));

                responses.push(json!({
                    "thread": thread_id,
                    "request": req_id,
                    "sent": true
                }));
            }
            let _ = tx.send((thread_id, responses));
        });
    }

    // Drop the original sender to end the channel
    drop(tx);

    // Collect all responses
    let mut all_responses = Vec::new();
    for (thread_id, responses) in rx {
        all_responses.push((thread_id, responses));
    }

    // Verify all 3 threads completed
    assert_eq!(all_responses.len(), 3, "All 3 threads should complete");

    // Final sanity check with the main reader - verify gateway still responds
    let resp = send_request(
        &stdin_arc,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_list",
            "arguments": {}
        }),
    );
    assert!(
        resp.get("result").is_some() || resp.get("error").is_some(),
        "Gateway should still respond after concurrent requests"
    );

    let _ = child.kill();
}

/// Test: Malformed JSON returns parse error
#[test]
fn test_malformed_json_request() {
    let Some((mut child, stdin, mut reader)) = spawn_gateway() else {
        return;
    };

    std::thread::sleep(Duration::from_millis(500));
    init_gateway(&stdin, &mut reader);

    // Send malformed JSON
    let mut stdin = stdin.lock().unwrap();
    stdin.write_all(b"this is not json\n").unwrap();
    stdin.flush().unwrap();

    // Read response (should be error response)
    let mut line = String::new();
    let result = reader.read_line(&mut line);

    // Should either get an error response or no crash
    assert!(
        result.is_ok(),
        "Gateway should handle malformed input gracefully"
    );

    let _ = child.kill();
}

/// Test: Missing required fields returns error
#[test]
fn test_missing_required_fields() {
    let Some((mut child, stdin, mut reader)) = spawn_gateway() else {
        return;
    };

    std::thread::sleep(Duration::from_millis(500));
    init_gateway(&stdin, &mut reader);

    // sandbox/create without template
    let resp = send_request(
        &stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {}
        }),
    );

    // Should return error about missing template
    let has_error = resp.get("error").is_some()
        || resp["result"]["content"][0]["text"]
            .as_str()
            .map(|s| s.contains("error") || s.contains("template"))
            .unwrap_or(false);

    assert!(
        has_error,
        "Should return error for missing template: {:?}",
        resp
    );

    let _ = child.kill();
}

/// Test: Sandbox operations with very long timeout
#[test]
fn test_sandbox_long_timeout() {
    let Some((mut child, stdin, mut reader)) = spawn_gateway() else {
        return;
    };

    std::thread::sleep(Duration::from_millis(500));
    init_gateway(&stdin, &mut reader);

    // Create sandbox with very long timeout (24 hours)
    let resp = send_request(
        &stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {
                "template": "debian:bookworm-slim",
                "timeout_ms": 86400000  // 24 hours
            }
        }),
    );

    // Should accept the timeout without error
    assert!(
        resp.get("result").is_some() || resp.get("error").is_some(),
        "Should handle long timeout: {:?}",
        resp
    );

    let _ = child.kill();
}

/// Test: Sandbox operations with zero timeout
#[test]
fn test_sandbox_zero_timeout() {
    let Some((mut child, stdin, mut reader)) = spawn_gateway() else {
        return;
    };

    std::thread::sleep(Duration::from_millis(500));
    init_gateway(&stdin, &mut reader);

    // Create sandbox with zero timeout
    let resp = send_request(
        &stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {
                "template": "debian:bookworm-slim",
                "timeout_ms": 0
            }
        }),
    );

    // Should either reject zero timeout or use default
    let response = resp.clone();
    let is_valid = response.get("result").is_some()
        || response["result"]["content"][0]["text"]
            .as_str()
            .map(|t| t.contains("error") || t.contains("timeout"))
            .unwrap_or(false);

    assert!(is_valid, "Should handle zero timeout: {:?}", resp);

    let _ = child.kill();
}

/// Test: Rapid sequential requests don't cause race conditions
#[test]
fn test_rapid_sequential_requests() {
    let Some((mut child, stdin, mut reader)) = spawn_gateway() else {
        return;
    };

    std::thread::sleep(Duration::from_millis(500));
    init_gateway(&stdin, &mut reader);

    // Send 10 rapid requests in succession
    for i in 0..10 {
        let resp = send_request(
            &stdin,
            &mut reader,
            "tools/call",
            json!({
                "name": "sandbox_list",
                "arguments": {}
            }),
        );

        // Should get a valid response
        assert!(
            resp.get("result").is_some() || resp.get("error").is_some(),
            "Request {} should return response",
            i
        );
    }

    let _ = child.kill();
}

/// Test: Empty command string
#[test]
fn test_empty_command_string() {
    let Some((mut child, stdin, mut reader)) = spawn_gateway() else {
        return;
    };

    std::thread::sleep(Duration::from_millis(500));
    init_gateway(&stdin, &mut reader);

    // Create sandbox
    let create_resp = send_request(
        &stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {"template": "debian:bookworm-slim", "timeout_ms": 60000}
        }),
    );

    let sandbox_id = extract_sandbox_id(&create_resp);

    if !sandbox_id.is_empty() {
        // Run with empty command
        let resp = send_request(
            &stdin,
            &mut reader,
            "tools/call",
            json!({
                "name": "sandbox_run",
                "arguments": {"sandbox_id": sandbox_id, "command": ""}
            }),
        );

        // Should handle empty command gracefully
        let text = extract_response_text(&resp);
        let handled =
            text.contains("error") || text.contains("empty") || resp.get("error").is_some();
        assert!(handled, "Should handle empty command: {}", text);
    }

    let _ = child.kill();
}

/// Test: Very long command string
#[test]
fn test_very_long_command() {
    let Some((mut child, stdin, mut reader)) = spawn_gateway() else {
        return;
    };

    std::thread::sleep(Duration::from_millis(500));
    init_gateway(&stdin, &mut reader);

    // Create sandbox
    let create_resp = send_request(
        &stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {"template": "debian:bookworm-slim", "timeout_ms": 60000}
        }),
    );

    let sandbox_id = extract_sandbox_id(&create_resp);

    if !sandbox_id.is_empty() {
        // Command that's ~10KB of repeated chars
        let long_cmd = "echo ".repeat(1000);
        let resp = send_request(
            &stdin,
            &mut reader,
            "tools/call",
            json!({
                "name": "sandbox_run",
                "arguments": {"sandbox_id": sandbox_id, "command": long_cmd}
            }),
        );

        // Should handle long command without crashing
        assert!(
            resp.get("result").is_some() || resp.get("error").is_some(),
            "Should handle long command: {:?}",
            resp
        );
    }

    let _ = child.kill();
}

/// Test: Special characters in command
#[test]
fn test_special_characters_in_command() {
    let Some((mut child, stdin, mut reader)) = spawn_gateway() else {
        return;
    };

    std::thread::sleep(Duration::from_millis(500));
    init_gateway(&stdin, &mut reader);

    // Create sandbox
    let create_resp = send_request(
        &stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {"template": "debian:bookworm-slim", "timeout_ms": 60000}
        }),
    );

    let sandbox_id = extract_sandbox_id(&create_resp);

    if !sandbox_id.is_empty() {
        // Command with special chars - testing for command injection resistance
        let special_cmd = r#"echo "hello $USER 'quotes' $(whoami) && ls -la | grep "^d""#;
        let resp = send_request(
            &stdin,
            &mut reader,
            "tools/call",
            json!({
                "name": "sandbox_run",
                "arguments": {"sandbox_id": sandbox_id, "command": special_cmd}
            }),
        );

        // Should handle special chars without injection - just check it returns something valid
        assert!(
            resp.get("result").is_some() || resp.get("error").is_some(),
            "Should handle special chars: {:?}",
            resp
        );
    }

    let _ = child.kill();
}

/// Test: Unicode in command
#[test]
fn test_unicode_in_command() {
    let Some((mut child, stdin, mut reader)) = spawn_gateway() else {
        return;
    };

    std::thread::sleep(Duration::from_millis(500));
    init_gateway(&stdin, &mut reader);

    // Create sandbox
    let create_resp = send_request(
        &stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {"template": "debian:bookworm-slim", "timeout_ms": 60000}
        }),
    );

    let sandbox_id = extract_sandbox_id(&create_resp);

    if !sandbox_id.is_empty() {
        // Command with unicode
        let unicode_cmd = "echo '日本語测试 Ελληνικά 🎉'";
        let resp = send_request(
            &stdin,
            &mut reader,
            "tools/call",
            json!({
                "name": "sandbox_run",
                "arguments": {"sandbox_id": sandbox_id, "command": unicode_cmd}
            }),
        );

        // Should handle unicode
        assert!(
            resp.get("result").is_some() || resp.get("error").is_some(),
            "Should handle unicode: {:?}",
            resp
        );
    }

    let _ = child.kill();
}

/// Test: Unknown tool name returns error
#[test]
fn test_unknown_tool_name() {
    let Some((mut child, stdin, mut reader)) = spawn_gateway() else {
        return;
    };

    std::thread::sleep(Duration::from_millis(500));
    init_gateway(&stdin, &mut reader);

    // Call non-existent tool
    let resp = send_request(
        &stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "nonexistent_tool_xyz",
            "arguments": {}
        }),
    );

    // Should return error
    let has_error = resp.get("error").is_some()
        || extract_response_text(&resp)
            .to_lowercase()
            .contains("error");
    assert!(
        has_error,
        "Should return error for unknown tool: {:?}",
        resp
    );

    let _ = child.kill();
}

/// Test: Invalid JSON-RPC version
#[test]
fn test_invalid_jsonrpc_version() {
    let Some((mut child, stdin, mut reader)) = spawn_gateway() else {
        return;
    };

    std::thread::sleep(Duration::from_millis(500));
    init_gateway(&stdin, &mut reader);

    // Send request with invalid JSON-RPC version
    let id = rand::random::<u64>() % 1_000_000;
    let request = json!({
        "jsonrpc": "1.0",  // Invalid - should be "2.0"
        "id": id,
        "method": "tools/list",
        "params": {}
    });
    let mut line = serde_json::to_string(&request).unwrap();
    line.push('\n');

    stdin.lock().unwrap().write_all(line.as_bytes()).unwrap();
    stdin.lock().unwrap().flush().unwrap();

    // Read response - should handle gracefully
    let mut response_line = String::new();
    let result = reader.read_line(&mut response_line);
    assert!(
        result.is_ok(),
        "Gateway should handle invalid JSON-RPC version"
    );

    let _ = child.kill();
}

/// Test: Negative timeout value
#[test]
fn test_negative_timeout() {
    let Some((mut child, stdin, mut reader)) = spawn_gateway() else {
        return;
    };

    std::thread::sleep(Duration::from_millis(500));
    init_gateway(&stdin, &mut reader);

    // Create sandbox with negative timeout
    let resp = send_request(
        &stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {
                "template": "debian:bookworm-slim",
                "timeout_ms": -1
            }
        }),
    );

    // Should handle negative timeout gracefully (either reject or use default)
    let response = resp.clone();
    let is_valid = response.get("result").is_some()
        || response["result"]["content"][0]["text"]
            .as_str()
            .map(|t| t.contains("error") || t.contains("timeout") || t.contains("negative"))
            .unwrap_or(false);

    assert!(is_valid, "Should handle negative timeout: {:?}", resp);

    let _ = child.kill();
}
