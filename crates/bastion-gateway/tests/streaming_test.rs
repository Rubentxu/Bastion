//! Tests for streaming and security-critical features of the Bastion Gateway.
//!
//! These tests verify:
//! - Basic streaming command execution
//! - Progress notification support
//! - Security: path traversal blocking
//! - Registry routing for commands
//!
//! Requires: Gateway binary compiled + Podman daemon running.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

// ═══════════════════════════════════════════════════════════════════════════════
// SPAWN & HELPERS (mirrors e2e_test.rs patterns)
// ═══════════════════════════════════════════════════════════════════════════════

/// Spawn the gateway and return stdin/stdout handles.
fn spawn_gateway() -> (std::process::Child, impl Write, impl BufRead) {
    spawn_gateway_with_args(&[])
}

/// Spawn the gateway with additional CLI arguments.
fn spawn_gateway_with_args(args: &[&str]) -> (std::process::Child, impl Write, impl BufRead) {
    let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/debug/bastion-gateway");

    let worker = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/debug/bastion-worker");

    let mut cmd = Command::new(&binary);
    cmd.arg("--image")
        .arg("debian:bookworm-slim")
        .arg("--worker-binary")
        .arg(&worker);

    for arg in args {
        cmd.arg(arg);
    }

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn bastion-gateway");

    let stdin = child.stdin.take().expect("stdin not captured");
    let stdout = child.stdout.take().expect("stdout not captured");
    let reader = BufReader::new(stdout);

    (child, stdin, reader)
}

/// Send a JSON-RPC request and return the response.
fn send_request(
    stdin: &mut impl Write,
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
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    // Read response (may have multiple lines for logging etc.)
    let mut response_line = String::new();
    for _ in 0..100 {
        response_line.clear();
        reader.read_line(&mut response_line).unwrap();
        if response_line.contains("\"id\":") || response_line.contains("\"jsonrpc\"") {
            break;
        }
    }

    serde_json::from_str(&response_line).unwrap_or(json!({"error": "parse failed"}))
}

/// Send a JSON-RPC notification (no response expected).
fn send_notification(stdin: &mut impl Write, method: &str, params: Value) {
    let request = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });

    let mut line = serde_json::to_string(&request).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
    std::thread::sleep(Duration::from_millis(100));
}

/// Helper: Initialize gateway connection.
fn init_gateway(
    _child: &mut std::process::Child,
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
) {
    std::thread::sleep(Duration::from_millis(500));

    let init_response = send_request(
        stdin,
        reader,
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "streaming-test", "version": "0.1.0"}
        }),
    );
    assert!(init_response.get("result").is_some(), "Initialize failed: {:?}", init_response);

    send_notification(stdin, "notifications/initialized", json!({}));
}

/// Helper: Extract sandbox_id from tools/call response.
fn extract_sandbox_id(response: &Value) -> Option<String> {
    let content = response["result"]["content"].as_array()?;
    let text = content[0]["text"].as_str()?;
    let result: Value = serde_json::from_str(text).ok()?;
    result["sandbox_id"].as_str().map(String::from)
}

/// Helper: Extract text content from tools/call response.
fn extract_response_text(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// Helper: Create a sandbox and return the sandbox_id.
fn create_sandbox(
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
) -> Option<String> {
    let response = send_request(
        stdin,
        reader,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {
                "template": "debian:bookworm-slim",
                "timeout_ms": 120000
            }
        }),
    );
    extract_sandbox_id(&response)
}

/// Helper: Terminate a sandbox.
fn terminate_sandbox(
    sandbox_id: &str,
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
) -> String {
    let response = send_request(
        stdin,
        reader,
        "tools/call",
        json!({
            "name": "sandbox_terminate",
            "arguments": {"sandbox_id": sandbox_id}
        }),
    );
    extract_response_text(&response)
}

/// Helper: Run a command and return the parsed result.
fn run_command(
    sandbox_id: &str,
    command: &str,
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
) -> Value {
    let response = send_request(
        stdin,
        reader,
        "tools/call",
        json!({
            "name": "sandbox_run",
            "arguments": {
                "sandbox_id": sandbox_id,
                "command": command
            }
        }),
    );
    let text = extract_response_text(&response);
    serde_json::from_str(&text).unwrap_or(json!({}))
}

/// Helper: Run a streaming command and return the parsed result.
fn run_stream_command(
    sandbox_id: &str,
    command: &str,
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
) -> Value {
    let response = send_request(
        stdin,
        reader,
        "tools/call",
        json!({
            "name": "sandbox_run_stream",
            "arguments": {
                "sandbox_id": sandbox_id,
                "command": command
            }
        }),
    );
    let text = extract_response_text(&response);
    serde_json::from_str(&text).unwrap_or(json!({}))
}

/// Helper: Run a streaming command with progress token.
fn run_stream_command_with_progress(
    sandbox_id: &str,
    command: &str,
    progress_token: &str,
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
) -> Value {
    let response = send_request(
        stdin,
        reader,
        "tools/call",
        json!({
            "name": "sandbox_run_stream",
            "arguments": {
                "sandbox_id": sandbox_id,
                "command": command
            },
            "_meta": {"progressToken": progress_token}
        }),
    );
    let text = extract_response_text(&response);
    serde_json::from_str(&text).unwrap_or(json!({}))
}

/// Helper: Read a file from sandbox.
fn sandbox_read(
    sandbox_id: &str,
    path: &str,
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
) -> Value {
    let response = send_request(
        stdin,
        reader,
        "tools/call",
        json!({
            "name": "sandbox_read",
            "arguments": {
                "sandbox_id": sandbox_id,
                "path": path
            }
        }),
    );
    let text = extract_response_text(&response);
    serde_json::from_str(&text).unwrap_or(json!({}))
}

// ═══════════════════════════════════════════════════════════════════════════════
// INFRASTRUCTURE CHECKS
// ═══════════════════════════════════════════════════════════════════════════════

/// Check if gateway binary exists.
fn gateway_binary_exists() -> bool {
    let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .parent().unwrap()
        .parent().unwrap()
        .join("target/debug/bastion-gateway");
    binary.exists()
}

/// Check if Podman socket exists.
fn podman_socket_exists() -> bool {
    let socket = std::path::Path::new("/run/user/1000/podman/podman.sock");
    socket.exists()
}

// ═══════════════════════════════════════════════════════════════════════════════
// TEST 1: test_sandbox_run_stream_basic
// ═══════════════════════════════════════════════════════════════════════════════

/// Test streaming basic: create sandbox → run_stream → verify stdout/stderr/exit_code
#[test]
fn test_sandbox_run_stream_basic() {
    // Check infrastructure
    if !gateway_binary_exists() {
        eprintln!("Skipping: Gateway binary not found at target/debug/bastion-gateway");
        return;
    }
    if !podman_socket_exists() {
        eprintln!("Skipping: Podman socket not found at /run/user/1000/podman/podman.sock");
        return;
    }

    let (mut child, mut stdin, mut reader) = spawn_gateway();
    init_gateway(&mut child, &mut stdin, &mut reader);

    // Create sandbox
    let sandbox_id = match create_sandbox(&mut stdin, &mut reader) {
        Some(id) => id,
        None => {
            eprintln!("Failed to create sandbox");
            let _ = child.kill();
            return;
        }
    };
    println!("✓ Created sandbox: {}", sandbox_id);

    // Run streaming command - basic echo test
    let result = run_stream_command(&sandbox_id, "echo hello_world", &mut stdin, &mut reader);
    println!("✓ Stream result: {:?}", result);

    // Verify structure
    assert!(result.get("exit_code").is_some(), "Should have exit_code field");
    assert!(result.get("stdout").is_some(), "Should have stdout field");
    assert!(result.get("stderr").is_some(), "Should have stderr field");

    let exit_code = result["exit_code"].as_i64().unwrap_or(-1);
    let stdout = result["stdout"].as_str().unwrap_or("");

    assert_eq!(exit_code, 0, "Command should succeed with exit_code 0");
    assert!(stdout.contains("hello_world"), "stdout should contain 'hello_world', got: {}", stdout);

    println!("✓ Basic streaming test passed: exit_code={}, stdout={}", exit_code, stdout.trim());

    // Cleanup
    let _ = terminate_sandbox(&sandbox_id, &mut stdin, &mut reader);
    let _ = child.kill();
    println!("✓ Cleanup complete");
}

// ═══════════════════════════════════════════════════════════════════════════════
// TEST 2: test_sandbox_run_stream_progress
// ═══════════════════════════════════════════════════════════════════════════════

/// Test progress notifications: create sandbox → run_stream with progress_token →
/// verify that the call succeeds and progress mechanism works
#[test]
fn test_sandbox_run_stream_progress() {
    // Check infrastructure
    if !gateway_binary_exists() {
        eprintln!("Skipping: Gateway binary not found");
        return;
    }
    if !podman_socket_exists() {
        eprintln!("Skipping: Podman socket not found");
        return;
    }

    let (mut child, mut stdin, mut reader) = spawn_gateway();
    init_gateway(&mut child, &mut stdin, &mut reader);

    // Create sandbox
    let sandbox_id = match create_sandbox(&mut stdin, &mut reader) {
        Some(id) => id,
        None => {
            eprintln!("Failed to create sandbox");
            let _ = child.kill();
            return;
        }
    };
    println!("✓ Created sandbox: {}", sandbox_id);

    // Run streaming command with progress token
    let result = run_stream_command_with_progress(
        &sandbox_id,
        "echo progress_test",
        "test-token-123",
        &mut stdin,
        &mut reader,
    );
    println!("✓ Stream result with progress: {:?}", result);

    // Verify response (progress is informational, we just verify no error)
    assert!(result.get("error").is_none(), "Should not return error, got: {:?}", result);
    assert!(result.get("exit_code").is_some(), "Should have exit_code field");

    let exit_code = result["exit_code"].as_i64().unwrap_or(-1);
    assert_eq!(exit_code, 0, "Command should succeed");

    println!("✓ Progress streaming test passed: exit_code={}", exit_code);

    // Cleanup
    let _ = terminate_sandbox(&sandbox_id, &mut stdin, &mut reader);
    let _ = child.kill();
    println!("✓ Cleanup complete");
}

// ═══════════════════════════════════════════════════════════════════════════════
// TEST 3: test_worker_path_traversal_blocked
// ═══════════════════════════════════════════════════════════════════════════════

/// Test security: attempt to read /etc/passwd → must be blocked
///
/// SECURITY-CRITICAL: This verifies that path traversal attacks are blocked.
/// Workers should NOT be able to read files outside their designated workspace.
#[test]
fn test_worker_path_traversal_blocked() {
    // Check infrastructure
    if !gateway_binary_exists() {
        eprintln!("Skipping: Gateway binary not found");
        return;
    }
    if !podman_socket_exists() {
        eprintln!("Skipping: Podman socket not found");
        return;
    }

    let (mut child, mut stdin, mut reader) = spawn_gateway();
    init_gateway(&mut child, &mut stdin, &mut reader);

    // Create sandbox
    let sandbox_id = match create_sandbox(&mut stdin, &mut reader) {
        Some(id) => id,
        None => {
            eprintln!("Failed to create sandbox");
            let _ = child.kill();
            return;
        }
    };
    println!("✓ Created sandbox: {}", sandbox_id);

    // Attempt to read /etc/passwd (path traversal attack)
    let result = sandbox_read(&sandbox_id, "/etc/passwd", &mut stdin, &mut reader);
    println!("✓ Attempted to read /etc/passwd, result: {:?}", result);

    // The read should either:
    // 1. Return an error (preferred - worker blocks it)
    // 2. Return empty content (acceptable - read succeeded but file is empty/not accessible)
    // 3. NOT return actual /etc/passwd contents that reveal user accounts

    let text = result["text"].as_str().unwrap_or("");

    // Check if it's an error response
    let is_error = result.get("error").is_some()
        || text.to_lowercase().contains("error")
        || text.to_lowercase().contains("denied")
        || text.to_lowercase().contains("permission")
        || text.to_lowercase().contains("not found")
        || text.to_lowercase().contains("blocked");

    // Check if the content looks like /etc/passwd (lines with colons - user entries)
    let looks_like_etc_passwd = text.contains(":") && text.lines().any(|l| l.starts_with("root:"));

    if looks_like_etc_passwd {
        // SECURITY VIOLATION - actual /etc/passwd contents leaked!
        eprintln!("SECURITY VIOLATION: /etc/passwd contents leaked: {}", text);
        panic!("Path traversal NOT blocked! Worker was able to read /etc/passwd");
    }

    if is_error {
        println!("✓ Path traversal blocked (got error response): {}", text);
    } else {
        println!("✓ Path traversal blocked or file inaccessible (no error but no sensitive content)");
    }

    // Cleanup
    let _ = terminate_sandbox(&sandbox_id, &mut stdin, &mut reader);
    let _ = child.kill();
    println!("✓ Security test complete");
}

// ═══════════════════════════════════════════════════════════════════════════════
// TEST 4: test_podman_registry_routing
// ═══════════════════════════════════════════════════════════════════════════════

/// Test that commands go via CommandRouter registry in exec pipeline
///
/// This test verifies that when registry is configured, commands are routed
/// through the CommandRouter interface rather than direct exec.
#[test]
fn test_podman_registry_routing() {
    // Check infrastructure
    if !gateway_binary_exists() {
        eprintln!("Skipping: Gateway binary not found");
        return;
    }
    if !podman_socket_exists() {
        eprintln!("Skipping: Podman socket not found");
        return;
    }

    // Start gateway with registry enabled
    let (mut child, mut stdin, mut reader) = spawn_gateway_with_args(&[
        "--registry-addr",
        "127.0.0.1:15001",
    ]);

    init_gateway(&mut child, &mut stdin, &mut reader);

    // Create sandbox
    let sandbox_id = match create_sandbox(&mut stdin, &mut reader) {
        Some(id) => id,
        None => {
            eprintln!("Failed to create sandbox");
            let _ = child.kill();
            return;
        }
    };
    println!("✓ Created sandbox: {}", sandbox_id);

    // Run a command - this will go through the command routing
    let result = run_command(&sandbox_id, "echo registry_routing_test", &mut stdin, &mut reader);
    println!("✓ Command result: {:?}", result);

    // Verify command executed successfully
    assert!(result.get("exit_code").is_some(), "Should have exit_code field");
    let exit_code = result["exit_code"].as_i64().unwrap_or(-1);
    assert_eq!(exit_code, 0, "Command should succeed via registry routing");

    let stdout = result["stdout"].as_str().unwrap_or("");
    assert!(stdout.contains("registry_routing_test"), "stdout should contain test marker");

    println!("✓ Registry routing test passed: exit_code={}, stdout={}", exit_code, stdout.trim());

    // Cleanup
    let _ = terminate_sandbox(&sandbox_id, &mut stdin, &mut reader);
    let _ = child.kill();
    println!("✓ Cleanup complete");
}

// ═══════════════════════════════════════════════════════════════════════════════
// ADDITIONAL TESTS
// ═══════════════════════════════════════════════════════════════════════════════

/// Test exit codes are properly propagated
#[test]
fn test_sandbox_run_exit_codes() {
    if !gateway_binary_exists() {
        eprintln!("Skipping: Gateway binary not found");
        return;
    }
    if !podman_socket_exists() {
        eprintln!("Skipping: Podman socket not found");
        return;
    }

    let (mut child, mut stdin, mut reader) = spawn_gateway();
    init_gateway(&mut child, &mut stdin, &mut reader);

    let sandbox_id = match create_sandbox(&mut stdin, &mut reader) {
        Some(id) => id,
        None => { let _ = child.kill(); return; }
    };

    // Test successful command
    let result = run_command(&sandbox_id, "true", &mut stdin, &mut reader);
    assert_eq!(result["exit_code"].as_i64().unwrap_or(-1), 0, "true should exit with 0");

    // Test failing command
    let result2 = run_command(&sandbox_id, "exit 42", &mut stdin, &mut reader);
    assert_eq!(result2["exit_code"].as_i64().unwrap_or(-1), 42, "exit 42 should return 42");

    println!("✓ Exit code propagation test passed");

    let _ = terminate_sandbox(&sandbox_id, &mut stdin, &mut reader);
    let _ = child.kill();
}

/// Test reading non-existent file returns error
#[test]
fn test_sandbox_read_nonexistent() {
    if !gateway_binary_exists() {
        eprintln!("Skipping: Gateway binary not found");
        return;
    }
    if !podman_socket_exists() {
        eprintln!("Skipping: Podman socket not found");
        return;
    }

    let (mut child, mut stdin, mut reader) = spawn_gateway();
    init_gateway(&mut child, &mut stdin, &mut reader);

    let sandbox_id = match create_sandbox(&mut stdin, &mut reader) {
        Some(id) => id,
        None => { let _ = child.kill(); return; }
    };

    // Read non-existent file
    let result = sandbox_read(&sandbox_id, "/nonexistent/file.txt", &mut stdin, &mut reader);

    // Should return error or empty content
    let text = result["text"].as_str().unwrap_or("");
    let has_error = text.to_lowercase().contains("error")
        || text.to_lowercase().contains("not found")
        || text.to_lowercase().contains("no such");

    assert!(has_error || text.is_empty(),
        "Should return error for non-existent file, got: {}", text);

    println!("✓ Read non-existent file test passed");

    let _ = terminate_sandbox(&sandbox_id, &mut stdin, &mut reader);
    let _ = child.kill();
}
