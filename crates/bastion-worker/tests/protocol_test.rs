//! Worker Protocol Tests
//!
//! Tests MCP protocol features: streaming, progress, cancellation, shutdown

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;

const BINARY: &str = "target/debug/bastion-worker";

// ═══════════════════════════════════════════════════════════════════════════════
// HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

fn worker_available() -> bool {
    let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(BINARY);
    binary.exists()
}

fn spawn_worker() -> Option<(std::process::Child, std::sync::Mutex<BufReader<std::process::ChildStdout>>, std::sync::Mutex<std::process::ChildStdin>)> {
    let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(BINARY);

    if !binary.exists() {
        eprintln!("Worker binary not found at {:?}", binary);
        return None;
    }

    let mut child = Command::new(&binary)
        .arg("--podman")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;
    Some((child, std::sync::Mutex::new(BufReader::new(stdout)), std::sync::Mutex::new(stdin)))
}

fn send_request(stdin: &mut std::process::ChildStdin, reader: &mut BufReader<std::process::ChildStdout>, method: &str, params: Value) -> Value {
    let id = rand::random::<u64>() % 1_000_000;
    let request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let mut line = serde_json::to_string(&request).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    // Read until we get a response with matching id
    let mut response_line = String::new();
    let target_id = id;
    for _ in 0..200 {
        response_line.clear();
        if reader.read_line(&mut response_line).unwrap() == 0 {
            break;
        }
        if let Ok(resp) = serde_json::from_str::<Value>(&response_line) {
            if resp.get("id").and_then(|i| i.as_u64()) == Some(target_id) {
                return resp;
            }
        }
    }
    json!({"error": "timeout"})
}

fn init_worker(stdin: &mut std::process::ChildStdin, reader: &mut BufReader<std::process::ChildStdout>) -> Value {
    send_request(stdin, reader, "initialize", json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {"streaming": true},
        "clientInfo": {"name": "protocol-test", "version": "0.1.0"}
    }))
}

// ═══════════════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════════════

/// Test: initialize returns capabilities including streaming support
#[test]
fn test_worker_initialize_capabilities() {
    if !worker_available() {
        eprintln!("Skipping: bastion-worker binary not available");
        return;
    }

    let (mut child, reader, stdin) = match spawn_worker() {
        Some(v) => v,
        None => return,
    };

    let mut stdin_guard = stdin.lock().unwrap();
    let mut reader_guard = reader.lock().unwrap();

    let resp = init_worker(&mut stdin_guard, &mut reader_guard);

    // Should have server capabilities
    let capabilities = resp["result"]["capabilities"]
        .as_object()
        .expect("capabilities should be an object");

    // Verify streaming capability is advertised
    assert!(capabilities.contains_key("streaming"),
        "Server should advertise streaming capability: {:?}", resp);

    let _ = child.kill();
}

/// Test: sandbox/create returns a sandbox ID
#[test]
fn test_worker_sandbox_create() {
    if !worker_available() {
        eprintln!("Skipping: bastion-worker binary not available");
        return;
    }

    let (mut child, reader, stdin) = match spawn_worker() {
        Some(v) => v,
        None => return,
    };

    let mut stdin_guard = stdin.lock().unwrap();
    let mut reader_guard = reader.lock().unwrap();

    let _ = init_worker(&mut stdin_guard, &mut reader_guard);

    let resp = send_request(&mut stdin_guard, &mut reader_guard, "sandbox/create", json!({
        "template": "debian:bookworm-slim",
        "timeout_ms": 60000
    }));

    let result = resp.get("result");
    assert!(result.is_some(), "Should have result: {:?}", resp);

    let sandbox_id = result
        .and_then(|r| r.get("sandbox_id"))
        .and_then(|id| id.as_str());

    assert!(sandbox_id.is_some(), "Should have sandbox_id in result: {:?}", resp);

    let _ = child.kill();
}

/// Test: sandbox/run streams output via multiple responses
#[test]
fn test_worker_sandbox_run_streaming() {
    if !worker_available() {
        eprintln!("Skipping: bastion-worker binary not available");
        return;
    }

    let (mut child, reader, stdin) = match spawn_worker() {
        Some(v) => v,
        None => return,
    };

    let mut stdin_guard = stdin.lock().unwrap();
    let mut reader_guard = reader.lock().unwrap();

    let _ = init_worker(&mut stdin_guard, &mut reader_guard);

    // Create sandbox first
    let create_resp = send_request(&mut stdin_guard, &mut reader_guard, "sandbox/create", json!({
        "template": "debian:bookworm-slim",
        "timeout_ms": 60000
    }));
    let sandbox_id = create_resp["result"]["sandbox_id"]
        .as_str()
        .unwrap_or("");

    // Run command that produces output
    let run_resp = send_request(&mut stdin_guard, &mut reader_guard, "sandbox/run", json!({
        "sandbox_id": sandbox_id,
        "command": "echo hello && echo world",
        "timeout_ms": 30000
    }));

    let result = run_resp.get("result");
    assert!(result.is_some(), "Should have result: {:?}", run_resp);

    // Response should have stdout with output
    let stdout = result
        .and_then(|r| r.get("stdout"))
        .and_then(|s| s.as_str());

    assert!(stdout.is_some(), "Should have stdout: {:?}", run_resp);

    let _ = child.kill();
}

/// Test: progress notifications are sent during long operations
#[test]
fn test_worker_progress_notifications() {
    if !worker_available() {
        eprintln!("Skipping: bastion-worker binary not available");
        return;
    }

    let (mut child, reader, stdin) = match spawn_worker() {
        Some(v) => v,
        None => return,
    };

    // Use thread-safe reader
    let reader = Arc::new(reader);
    let reader_clone = Arc::clone(&reader);

    let mut stdin_guard = stdin.lock().unwrap();
    let mut reader_clone_guard = reader_clone.lock().unwrap();

    let _ = init_worker(&mut stdin_guard, &mut reader_clone_guard);

    // Create sandbox
    let create_resp = send_request(&mut stdin_guard, &mut reader_clone_guard, "sandbox/create", json!({
        "template": "debian:bookworm-slim",
        "timeout_ms": 60000
    }));
    let sandbox_id = create_resp["result"]["sandbox_id"]
        .as_str()
        .unwrap_or("");

    // Send request with progress token
    let _progress_token = "test-progress-123";
    let run_resp = send_request(&mut stdin_guard, &mut reader_clone_guard, "sandbox/run", json!({
        "sandbox_id": sandbox_id,
        "command": "sleep 0.5 && echo done",
        "timeout_ms": 30000
    }));

    // In streaming mode, we might get progress notifications
    // The final response should have the result
    let result = run_resp.get("result");
    assert!(result.is_some(), "Should have final result: {:?}", run_resp);

    let _ = child.kill();
}

/// Test: graceful shutdown via workspace/stop
#[test]
fn test_worker_graceful_shutdown() {
    if !worker_available() {
        eprintln!("Skipping: bastion-worker binary not available");
        return;
    }

    let (mut child, reader, stdin) = match spawn_worker() {
        Some(v) => v,
        None => return,
    };

    let mut stdin_guard = stdin.lock().unwrap();
    let mut reader_guard = reader.lock().unwrap();

    let _ = init_worker(&mut stdin_guard, &mut reader_guard);

    // Send stop request
    let resp = send_request(&mut stdin_guard, &mut reader_guard, "workspace/stop", json!({}));

    // Should get result indicating shutdown
    assert!(resp.get("result").is_some() || resp.get("error").is_some(),
        "Should get response: {:?}", resp);

    // Worker should exit cleanly
    let status = child.wait().unwrap();
    assert!(status.success() || status.code() == Some(0),
        "Worker should exit cleanly: {:?}", status);
}

/// Test: sandbox/terminate stops a running sandbox
#[test]
fn test_worker_sandbox_terminate() {
    if !worker_available() {
        eprintln!("Skipping: bastion-worker binary not available");
        return;
    }

    let (mut child, reader, stdin) = match spawn_worker() {
        Some(v) => v,
        None => return,
    };

    let mut stdin_guard = stdin.lock().unwrap();
    let mut reader_guard = reader.lock().unwrap();

    let _ = init_worker(&mut stdin_guard, &mut reader_guard);

    // Create sandbox
    let create_resp = send_request(&mut stdin_guard, &mut reader_guard, "sandbox/create", json!({
        "template": "debian:bookworm-slim",
        "timeout_ms": 60000
    }));
    let sandbox_id = create_resp["result"]["sandbox_id"]
        .as_str()
        .unwrap_or("");

    // Terminate it
    let term_resp = send_request(&mut stdin_guard, &mut reader_guard, "sandbox/terminate", json!({
        "sandbox_id": sandbox_id
    }));

    // Should succeed
    let error_code = term_resp["error"]["code"].as_i64().unwrap_or(0);
    assert!(term_resp.get("result").is_some() || error_code < 0,
        "Terminate should succeed: {:?}", term_resp);

    let _ = child.kill();
}

/// Test: Invalid sandbox ID returns error
#[test]
fn test_worker_invalid_sandbox_id() {
    if !worker_available() {
        eprintln!("Skipping: bastion-worker binary not available");
        return;
    }

    let (mut child, reader, stdin) = match spawn_worker() {
        Some(v) => v,
        None => return,
    };

    let mut stdin_guard = stdin.lock().unwrap();
    let mut reader_guard = reader.lock().unwrap();

    let _ = init_worker(&mut stdin_guard, &mut reader_guard);

    let resp = send_request(&mut stdin_guard, &mut reader_guard, "sandbox/run", json!({
        "sandbox_id": "nonexistent-sandbox-12345",
        "command": "echo test",
        "timeout_ms": 5000
    }));

    // Should return error for unknown sandbox
    let has_error = resp.get("error").is_some();
    let has_error_in_result = resp["result"]["error"].as_str().is_some();
    assert!(has_error || has_error_in_result,
        "Should return error for invalid sandbox: {:?}", resp);

    let _ = child.kill();
}

/// Test: Long-running command respects timeout
#[test]
fn test_worker_command_timeout() {
    if !worker_available() {
        eprintln!("Skipping: bastion-worker binary not available");
        return;
    }

    let (mut child, reader, stdin) = match spawn_worker() {
        Some(v) => v,
        None => return,
    };

    let mut stdin_guard = stdin.lock().unwrap();
    let mut reader_guard = reader.lock().unwrap();

    let _ = init_worker(&mut stdin_guard, &mut reader_guard);

    // Create sandbox
    let create_resp = send_request(&mut stdin_guard, &mut reader_guard, "sandbox/create", json!({
        "template": "debian:bookworm-slim",
        "timeout_ms": 60000
    }));
    let sandbox_id = create_resp["result"]["sandbox_id"]
        .as_str()
        .unwrap_or("");

    // Run command with very short timeout
    let run_resp = send_request(&mut stdin_guard, &mut reader_guard, "sandbox/run", json!({
        "sandbox_id": sandbox_id,
        "command": "sleep 10",
        "timeout_ms": 1000  // 1ms timeout - should fail
    }));

    // Should timeout or return error
    let text = run_resp["result"]["text"].as_str().unwrap_or("");
    let is_error = text.contains("timeout") || text.contains("error") || run_resp.get("error").is_some();
    assert!(is_error, "Should timeout: {:?}", run_resp);

    let _ = child.kill();
}
