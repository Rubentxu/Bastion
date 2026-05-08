//! Tests for streaming and security-critical features of the Bastion Gateway.
//!
//! These tests verify:
//! - Basic streaming command execution
//! - Progress notification support
//! - Security: path traversal blocking
//! - Registry routing for commands
//!
//! ## Running These Tests
//!
//! These tests REQUIRE infrastructure to be available:
//! - Gateway binary compiled at `target/debug/bastion-gateway`
//! - Podman daemon with `/run/user/1000/podman/podman.sock`
//!
//! If infrastructure is not available, tests will FAIL rather than skip silently.
//! This is intentional - we want to know when tests can't run, not hide it.
//!
//! To run only when infrastructure is available:
//! ```bash
//! cargo test --test streaming_test -- --ignored
//! ```

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

// ═══════════════════════════════════════════════════════════════════════════════
// INFRASTRUCTURE CHECKS (fail fast if not available)
// ═══════════════════════════════════════════════════════════════════════════════

fn get_gateway_binary_path() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR for test = crates/bastion-gateway
    // Need to go up 3 levels to workspace root
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap() // crates
        .parent()
        .unwrap() // workspace root
        .join("target/debug/bastion-gateway")
}

fn get_worker_binary_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/debug/bastion-worker")
}

fn check_infrastructure() -> Result<(), String> {
    let gateway = get_gateway_binary_path();
    if !gateway.exists() {
        return Err(format!(
            "Gateway binary not found at {:?}. Build with: cargo build --package bastion-gateway",
            gateway
        ));
    }

    let worker = get_worker_binary_path();
    if !worker.exists() {
        return Err(format!(
            "Worker binary not found at {:?}. Build with: cargo build --package bastion-worker",
            worker
        ));
    }

    let socket = std::path::Path::new("/run/user/1000/podman/podman.sock");
    if !socket.exists() {
        return Err(format!(
            "Podman socket not found at {:?}. Start podman daemon or use --docker flag",
            socket
        ));
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// SPAWN & HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

fn spawn_gateway() -> Result<(std::process::Child, impl Write, impl BufRead), String> {
    let gateway = get_gateway_binary_path();
    let worker = get_worker_binary_path();

    let mut cmd = Command::new(&gateway);
    cmd.arg("--image")
        .arg("debian:bookworm-slim")
        .arg("--worker-binary")
        .arg(&worker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().map_err(|e| {
        format!(
            "Failed to spawn bastion-gateway: {}. Is the binary executable?",
            e
        )
    })?;

    let stdin = child.stdin.take().ok_or("Failed to capture stdin")?;
    let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;

    Ok((child, stdin, BufReader::new(stdout)))
}

fn send_request(
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let id = rand::random::<u64>() % 1_000_000;
    let request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });

    let mut line = serde_json::to_string(&request)
        .map_err(|e| format!("Failed to serialize request: {}", e))?;
    line.push('\n');

    stdin
        .write_all(line.as_bytes())
        .map_err(|e| format!("Failed to write request: {}", e))?;
    stdin
        .flush()
        .map_err(|e| format!("Failed to flush stdin: {}", e))?;

    // Read response
    let mut response_line = String::new();
    for _ in 0..100 {
        response_line.clear();
        let bytes = reader
            .read_line(&mut response_line)
            .map_err(|e| format!("Failed to read response: {}", e))?;
        if bytes == 0 {
            return Err("Unexpected EOF from gateway".to_string());
        }
        if response_line.contains("\"id\":") || response_line.contains("\"jsonrpc\"") {
            break;
        }
    }

    serde_json::from_str(&response_line)
        .map_err(|e| format!("Failed to parse response '{}': {}", response_line, e))
}

fn send_notification(stdin: &mut impl Write, method: &str, params: Value) -> Result<(), String> {
    let request = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });

    let mut line = serde_json::to_string(&request)
        .map_err(|e| format!("Failed to serialize notification: {}", e))?;
    line.push('\n');

    stdin
        .write_all(line.as_bytes())
        .map_err(|e| format!("Failed to write notification: {}", e))?;
    stdin
        .flush()
        .map_err(|e| format!("Failed to flush: {}", e))?;

    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}

fn init_gateway(
    child: &mut std::process::Child,
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
) -> Result<(), String> {
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
    )?;

    if init_response.get("result").is_none() {
        return Err(format!("Initialize failed: {:?}", init_response));
    }

    send_notification(stdin, "notifications/initialized", json!({}))?;
    Ok(())
}

fn extract_sandbox_id(response: &Value) -> Result<String, String> {
    let content = response["result"]["content"]
        .as_array()
        .ok_or("Missing result.content array")?;
    let text = content[0]["text"]
        .as_str()
        .ok_or("Missing text in content")?;
    let result: Value =
        serde_json::from_str(text).map_err(|e| format!("Failed to parse result JSON: {}", e))?;
    result["sandbox_id"]
        .as_str()
        .map(String::from)
        .ok_or("Missing sandbox_id in result".to_string())
}

fn extract_response_text(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn create_sandbox(stdin: &mut impl Write, reader: &mut impl BufRead) -> Result<String, String> {
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
    )?;
    extract_sandbox_id(&response)
}

fn terminate_sandbox(
    sandbox_id: &str,
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
) -> Result<String, String> {
    let response = send_request(
        stdin,
        reader,
        "tools/call",
        json!({
            "name": "sandbox_terminate",
            "arguments": {"sandbox_id": sandbox_id}
        }),
    )?;
    Ok(extract_response_text(&response))
}

fn run_command(
    sandbox_id: &str,
    command: &str,
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
) -> Result<Value, String> {
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
    )?;
    let text = extract_response_text(&response);
    serde_json::from_str(&text).map_err(|e| format!("Failed to parse run result: {}", e))
}

fn run_stream_command(
    sandbox_id: &str,
    command: &str,
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
) -> Result<Value, String> {
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
    )?;
    let text = extract_response_text(&response);
    serde_json::from_str(&text).map_err(|e| format!("Failed to parse stream result: {}", e))
}

fn run_stream_command_with_progress(
    sandbox_id: &str,
    command: &str,
    progress_token: &str,
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
) -> Result<Value, String> {
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
    )?;
    let text = extract_response_text(&response);
    serde_json::from_str(&text).map_err(|e| format!("Failed to parse stream result: {}", e))
}

fn sandbox_read(
    sandbox_id: &str,
    path: &str,
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
) -> Result<Value, String> {
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
    )?;
    let text = extract_response_text(&response);
    serde_json::from_str(&text).map_err(|e| format!("Failed to parse read result: {}", e))
}

// ═══════════════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════════════

/// Test streaming basic: create sandbox → run_stream → verify stdout/stderr/exit_code
///
/// REQUIRES: Gateway binary + Podman daemon running
#[test]
fn test_sandbox_run_stream_basic() {
    // Fail fast if infrastructure is missing
    check_infrastructure().expect("Infrastructure check failed");

    let (mut child, mut stdin, mut reader) = spawn_gateway().expect("Failed to spawn gateway");

    init_gateway(&mut child, &mut stdin, &mut reader).expect("Failed to initialize gateway");

    // Create sandbox
    let sandbox_id = create_sandbox(&mut stdin, &mut reader).expect("Failed to create sandbox");
    println!("Created sandbox: {}", sandbox_id);

    // Run streaming command
    let result = run_stream_command(&sandbox_id, "echo hello_world", &mut stdin, &mut reader)
        .expect("Failed to run stream command");

    // Verify structure
    assert!(
        result.get("exit_code").is_some(),
        "Should have exit_code field"
    );
    assert!(result.get("stdout").is_some(), "Should have stdout field");
    assert!(result.get("stderr").is_some(), "Should have stderr field");

    let exit_code = result["exit_code"].as_i64().unwrap_or(-1);
    let stdout = result["stdout"].as_str().unwrap_or("");

    assert_eq!(exit_code, 0, "Command should succeed with exit_code 0");
    assert!(
        stdout.contains("hello_world"),
        "stdout should contain 'hello_world', got: {}",
        stdout
    );

    // Cleanup
    terminate_sandbox(&sandbox_id, &mut stdin, &mut reader).ok();
    child.kill().ok();
}

/// Test progress notifications: create sandbox → run_stream with progress_token
///
/// REQUIRES: Gateway binary + Podman daemon running
#[test]
fn test_sandbox_run_stream_progress() {
    check_infrastructure().expect("Infrastructure check failed");

    let (mut child, mut stdin, mut reader) = spawn_gateway().expect("Failed to spawn gateway");

    init_gateway(&mut child, &mut stdin, &mut reader).expect("Failed to initialize gateway");

    let sandbox_id = create_sandbox(&mut stdin, &mut reader).expect("Failed to create sandbox");

    // Run with progress token
    let result = run_stream_command_with_progress(
        &sandbox_id,
        "echo progress_test",
        "test-token-123",
        &mut stdin,
        &mut reader,
    )
    .expect("Failed to run stream command with progress");

    // Verify no error
    assert!(
        result.get("error").is_none(),
        "Should not return error, got: {:?}",
        result
    );
    assert!(
        result.get("exit_code").is_some(),
        "Should have exit_code field"
    );

    let exit_code = result["exit_code"].as_i64().unwrap_or(-1);
    assert_eq!(exit_code, 0, "Command should succeed");

    terminate_sandbox(&sandbox_id, &mut stdin, &mut reader).ok();
    child.kill().ok();
}

/// Test security: attempt to read /etc/passwd → must be blocked
///
/// SECURITY-CRITICAL: Verifies path traversal attacks are blocked.
/// Workers must NOT be able to read files outside their designated workspace.
///
/// REQUIRES: Gateway binary + Podman daemon running
#[test]
fn test_worker_path_traversal_blocked() {
    check_infrastructure().expect("Infrastructure check failed");

    let (mut child, mut stdin, mut reader) = spawn_gateway().expect("Failed to spawn gateway");

    init_gateway(&mut child, &mut stdin, &mut reader).expect("Failed to initialize gateway");

    let sandbox_id = create_sandbox(&mut stdin, &mut reader).expect("Failed to create sandbox");

    // Attempt to read /etc/passwd (path traversal attack)
    let result = sandbox_read(&sandbox_id, "/etc/passwd", &mut stdin, &mut reader)
        .expect("Failed to attempt sandbox_read");

    let text = result["text"].as_str().unwrap_or("");

    // Check if the content looks like /etc/passwd (lines with colons - user entries)
    let looks_like_etc_passwd = text.contains(":") && text.lines().any(|l| l.starts_with("root:"));

    // SECURITY VIOLATION - actual /etc/passwd contents leaked!
    assert!(
        !looks_like_etc_passwd,
        "SECURITY VIOLATION: /etc/passwd contents leaked: {}",
        text
    );

    // Either error response or empty content is acceptable
    let is_error = result.get("error").is_some()
        || text.to_lowercase().contains("error")
        || text.to_lowercase().contains("denied")
        || text.to_lowercase().contains("permission")
        || text.to_lowercase().contains("not found")
        || text.to_lowercase().contains("blocked")
        || text.is_empty();

    assert!(is_error, "Path traversal should be blocked, got: {}", text);

    terminate_sandbox(&sandbox_id, &mut stdin, &mut reader).ok();
    child.kill().ok();
}

/// Test that commands route through registry when configured
///
/// REQUIRES: Gateway binary + Podman daemon running
#[test]
fn test_podman_registry_routing() {
    check_infrastructure().expect("Infrastructure check failed");

    let gateway = get_gateway_binary_path();
    let worker = get_worker_binary_path();

    // Start gateway with registry enabled
    let mut cmd = Command::new(&gateway);
    cmd.arg("--image")
        .arg("debian:bookworm-slim")
        .arg("--worker-binary")
        .arg(&worker)
        .arg("--registry-addr")
        .arg("127.0.0.1:15001")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().expect("Failed to spawn gateway with registry");

    let mut stdin = child.stdin.take().expect("stdin not captured");
    let stdout = child.stdout.take().expect("stdout not captured");
    let mut reader = BufReader::new(stdout);

    init_gateway(&mut child, &mut stdin, &mut reader).expect("Failed to initialize gateway");

    let sandbox_id = create_sandbox(&mut stdin, &mut reader).expect("Failed to create sandbox");

    // Run command
    let result = run_command(
        &sandbox_id,
        "echo registry_routing_test",
        &mut stdin,
        &mut reader,
    )
    .expect("Failed to run command");

    assert!(
        result.get("exit_code").is_some(),
        "Should have exit_code field"
    );
    let exit_code = result["exit_code"].as_i64().unwrap_or(-1);
    assert_eq!(exit_code, 0, "Command should succeed via registry routing");

    let stdout = result["stdout"].as_str().unwrap_or("");
    assert!(
        stdout.contains("registry_routing_test"),
        "stdout should contain test marker"
    );

    terminate_sandbox(&sandbox_id, &mut stdin, &mut reader).ok();
    child.kill().ok();
}

/// Test exit codes are properly propagated
#[test]
fn test_sandbox_run_exit_codes() {
    check_infrastructure().expect("Infrastructure check failed");

    let (mut child, mut stdin, mut reader) = spawn_gateway().expect("Failed to spawn gateway");

    init_gateway(&mut child, &mut stdin, &mut reader).expect("Failed to initialize gateway");

    let sandbox_id = create_sandbox(&mut stdin, &mut reader).expect("Failed to create sandbox");

    // Test successful command
    let result = run_command(&sandbox_id, "true", &mut stdin, &mut reader)
        .expect("Failed to run 'true' command");
    assert_eq!(
        result["exit_code"].as_i64().unwrap_or(-1),
        0,
        "'true' should exit with 0"
    );

    // Test failing command
    let result2 = run_command(&sandbox_id, "exit 42", &mut stdin, &mut reader)
        .expect("Failed to run 'exit 42' command");
    assert_eq!(
        result2["exit_code"].as_i64().unwrap_or(-1),
        42,
        "'exit 42' should return 42"
    );

    terminate_sandbox(&sandbox_id, &mut stdin, &mut reader).ok();
    child.kill().ok();
}

/// Test reading non-existent file returns error
#[test]
fn test_sandbox_read_nonexistent() {
    check_infrastructure().expect("Infrastructure check failed");

    let (mut child, mut stdin, mut reader) = spawn_gateway().expect("Failed to spawn gateway");

    init_gateway(&mut child, &mut stdin, &mut reader).expect("Failed to initialize gateway");

    let sandbox_id = create_sandbox(&mut stdin, &mut reader).expect("Failed to create sandbox");

    // Read non-existent file
    let result = sandbox_read(
        &sandbox_id,
        "/nonexistent/file.txt",
        &mut stdin,
        &mut reader,
    )
    .expect("Failed to attempt read");

    // Should return error
    let text = result["text"].as_str().unwrap_or("");
    let has_error = text.to_lowercase().contains("error")
        || text.to_lowercase().contains("not found")
        || text.to_lowercase().contains("no such");

    assert!(
        has_error || text.is_empty(),
        "Should return error for non-existent file, got: {}",
        text
    );

    terminate_sandbox(&sandbox_id, &mut stdin, &mut reader).ok();
    child.kill().ok();
}
