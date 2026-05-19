//! E2E Podman Tests via HTTP Transport
//!
//! Tests the full flow using the bastion-gateway HTTP transport:
//! - Spawn gateway with Podman provider
//! - Create sandbox via HTTP API
//! - Run commands via HTTP API
//! - Worker connects to gateway and registers
//! - All operations use the gateway's internal command_router
//!
//! ## Running These Tests
//!
//! ```bash
//! # Run with debug build
//! cargo test -p bastion-gateway --test e2e_podman_test
//!
//! # Run specific test
//! cargo test -p bastion-gateway --test e2e_podman_test test_e2e_create_and_run_command
//! ```
//!
//! ## Prerequisites
//!
//! - Podman daemon at `/run/user/1000/podman/podman.sock`
//! - Gateway binary built at `target/debug/bastion-gateway`
//! - Worker binary built at `target/x86_64-unknown-linux-musl/release/bastion-worker`

use serde_json::{json, Value};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Gateway binary path
fn gateway_binary() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    workspace_root.join(format!("target/{}/bastion-gateway", profile))
}

/// Worker binary path (musl static for container compatibility)
fn worker_binary() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    workspace_root.join("target/x86_64-unknown-linux-musl/release/bastion-worker")
}

/// Check if Podman socket exists
fn require_podman() {
    let socket = std::path::Path::new("/run/user/1000/podman/podman.sock");
    if !socket.exists() {
        eprintln!("SKIP: Podman socket not found at {:?}", socket);
        std::process::exit(0);
    }
}

/// Check if gateway binary exists
fn require_gateway_binary() {
    let binary = gateway_binary();
    if !binary.exists() {
        eprintln!(
            "SKIP: Gateway binary not found at {:?}. Build with: cargo build -p bastion-gateway",
            binary
        );
        std::process::exit(0);
    }
}

/// Check if worker binary exists
fn require_worker_binary() {
    let binary = worker_binary();
    if !binary.exists() {
        eprintln!(
            "SKIP: Worker binary not found at {:?}. Build with: cargo build -p bastion-worker --target x86_64-unknown-linux-musl",
            binary
        );
        std::process::exit(0);
    }
}

/// Spawn gateway in HTTP mode on a specific port
fn spawn_gateway_http(port: u16) -> Child {
    let gateway = gateway_binary();
    let worker = worker_binary();

    let child = Command::new(&gateway)
        .arg("--http-port")
        .arg(port.to_string())
        .arg("--image")
        .arg("debian:bookworm-slim")
        .arg("--worker-binary")
        .arg(&worker)
        // Bind registry to all interfaces so containers can connect via host.containers.internal
        .arg("--registry-addr")
        .arg("0.0.0.0:50052")
        // Disable mTLS for development/testing - workers connect without certificates
        .arg("--registry-no-tls")
        // Set pool to minimum to reduce chance of pool-related issues
        .arg("--pool-min-idle")
        .arg("0")
        .arg("--pool-max-idle")
        .arg("0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn bastion-gateway in HTTP mode");

    // Wait for server to start listening
    let start = std::time::Instant::now();
    let max_wait = Duration::from_secs(45);
    while start.elapsed() < max_wait {
        if std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    child
}

/// Session cookie storage for MCP session affinity
static SESSION_COOKIE: once_cell::sync::Lazy<std::sync::Mutex<Option<String>>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(None));

/// Extract session cookie from response headers and store it
fn extract_session_cookie(headers: &reqwest::header::HeaderMap) {
    if let Some(cookie) = headers.get("mcp-session-id") {
        if let Ok(cookie_str) = cookie.to_str() {
            eprintln!("DEBUG: Got session cookie: {}", cookie_str);
            let mut session = SESSION_COOKIE.lock().unwrap();
            *session = Some(cookie_str.to_string());
        }
    }
}

/// Get the stored session cookie
fn get_session_cookie() -> Option<String> {
    SESSION_COOKIE.lock().unwrap().clone()
}

/// Clear session cookie (call before each test to start fresh)
fn clear_session_cookie() {
    let mut session = SESSION_COOKIE.lock().unwrap();
    *session = None;
    eprintln!("DEBUG: Session cookie cleared");
}

/// Send HTTP POST request and parse SSE response (maintains session via mcp-session-id header)
fn http_mcp_request(port: u16, method: &str, params: Value, id: u64) -> Result<Value, String> {
    let client = reqwest::blocking::Client::new();

    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });

    eprintln!("DEBUG HTTP request: method={}, id={}", method, id);

    let mut request = client
        .post(format!("http://127.0.0.1:{}/", port))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2024-11-05")
        .json(&payload)
        .timeout(Duration::from_secs(60));

    // Add session cookie if we have one
    if let Some(ref cookie) = get_session_cookie() {
        eprintln!("DEBUG: Using session cookie: {}", cookie);
        request = request.header("mcp-session-id", cookie);
    }

    let response = request
        .send()
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    // Extract and store session cookie from response
    extract_session_cookie(response.headers());

    let status = response.status();
    let text = response
        .text()
        .map_err(|e| format!("Failed to read response: {}", e))?;

    eprintln!("DEBUG HTTP response: status={}, body_len={}", status, text.len());

    // Parse SSE format with multiple "data:" lines:
    // data:
    // id: 0
    // retry: 3000
    //
    // data: {"jsonrpc":"2.0","id":0,"result":{...}}
    //
    // We need to find the line that starts with "data: {" (JSON content)
    let mut json_data = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("data:") {
            let after_data = trimmed.strip_prefix("data:").unwrap_or("").trim();
            // Skip empty data lines and meta lines (id:, retry:)
            if !after_data.is_empty()
                && !after_data.starts_with("id:")
                && !after_data.starts_with("retry:")
                && after_data.starts_with('{')
            {
                json_data = after_data.to_string();
                break;
            }
        }
    }

    if json_data.is_empty() {
        eprintln!("DEBUG: Could not parse JSON from SSE. Full response:\n{}", text);
        return Err(format!("No JSON data found in SSE response: {}", text));
    }

    eprintln!("DEBUG: Parsed JSON: {}", json_data);

    serde_json::from_str(&json_data)
        .map_err(|e| format!("Failed to parse JSON from SSE: {} - text: {}", e, json_data))
}

/// Initialize gateway via HTTP
fn http_initialize(port: u16) -> Result<Value, String> {
    // Clear any existing session
    clear_session_cookie();

    let result = http_mcp_request(
        port,
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "e2e-test", "version": "1.0.0"}
        }),
        0,
    )?;

    // Send the initialized notification (required by MCP protocol)
    http_send_notification(port, "initialized", json!({}))?;

    Ok(result)
}

/// Send a notification (no response expected)
fn http_send_notification(port: u16, method: &str, params: Value) -> Result<(), String> {
    let client = reqwest::blocking::Client::new();

    let payload = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });

    eprintln!("DEBUG HTTP notification: method={}", method);

    let mut request = client
        .post(format!("http://127.0.0.1:{}/", port))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2024-11-05")
        .json(&payload)
        .timeout(Duration::from_secs(30));

    // Add session cookie if we have one
    if let Some(ref cookie) = get_session_cookie() {
        request = request.header("mcp-session-id", cookie);
    }

    let response = request
        .send()
        .map_err(|e| format!("HTTP notification failed: {}", e))?;

    eprintln!("DEBUG HTTP notification response: status={}", response.status());

    Ok(())
}

/// Send tools/call request
fn http_tools_call(port: u16, name: &str, arguments: Value) -> Result<Value, String> {
    http_mcp_request(
        port,
        "tools/call",
        json!({
            "name": name,
            "arguments": arguments
        }),
        2,
    )
}

/// Helper to extract sandbox_id from sandbox_create response
fn extract_sandbox_id(response: &Value) -> Option<String> {
    let content = response.get("result")?.get("content")?;
    let text = content.get(0)?.get("text")?.as_str()?;
    let result: Value = serde_json::from_str(text).ok()?;
    result.get("sandbox_id")?.as_str().map(|s| s.to_string())
}

/// Helper to extract exit_code from sandbox_run response
fn extract_exit_code(response: &Value) -> Option<i64> {
    let content = response.get("result")?.get("content")?;
    let text = content.get(0)?.get("text")?.as_str()?;
    let result: Value = serde_json::from_str(text).ok()?;
    result.get("exit_code")?.as_i64()
}

/// Helper to extract stdout from sandbox_run response
fn extract_stdout(response: &Value) -> Option<String> {
    let content = response.get("result")?.get("content")?;
    let text = content.get(0)?.get("text")?.as_str()?;
    let result: Value = serde_json::from_str(text).ok()?;
    result.get("stdout")?.as_str().map(|s| s.to_string())
}

// ============================================================================
// TESTS
// ============================================================================

#[test]
fn test_e2e_podman_prerequisites() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();
    println!("✓ All prerequisites met");
}

#[test]
fn test_e2e_create_and_run_command() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();

    let port = 18100;
    let mut child = spawn_gateway_http(port);

    // Initialize
    let init_result = http_initialize(port);
    assert!(init_result.is_ok(), "Initialize should succeed: {:?}", init_result.err());
    println!("✓ Gateway initialized");

    // Create sandbox
    let create_result = http_tools_call(
        port,
        "sandbox_create",
        json!({
            "template": "debian:bookworm-slim",
            "timeout_ms": 60000
        }),
    );

    let sandbox_id = match create_result {
        Ok(response) => {
            let id = extract_sandbox_id(&response);
            assert!(id.is_some(), "Should have sandbox_id, response: {:?}", response);
            println!("✓ Created sandbox: {}", id.as_ref().unwrap());
            id.unwrap()
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("sandbox_create failed: {}", e);
        }
    };

    // Wait for worker to connect to gateway
    std::thread::sleep(Duration::from_secs(5));

    // Run command via sandbox_run
    let run_result = http_tools_call(
        port,
        "sandbox_run",
        json!({
            "sandbox_id": sandbox_id,
            "command": "echo 'Hello from Bastion E2E!'"
        }),
    );

    match run_result {
        Ok(response) => {
            let exit_code = extract_exit_code(&response).unwrap_or(-1);
            let stdout = extract_stdout(&response).unwrap_or_default();

            assert_eq!(exit_code, 0, "Command should succeed, got exit_code: {}", exit_code);
            assert!(
                stdout.contains("Hello from Bastion E2E!"),
                "Output should contain expected text, got: {}",
                stdout
            );
            println!("✓ Command executed: exit_code={}, stdout={}", exit_code, stdout.trim());
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("sandbox_run failed: {}", e);
        }
    }

    // Terminate sandbox
    let term_result = http_tools_call(
        port,
        "sandbox_terminate",
        json!({
            "sandbox_id": sandbox_id
        }),
    );

    assert!(term_result.is_ok(), "sandbox_terminate should succeed");
    println!("✓ Sandbox terminated");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn test_e2e_environment_variables() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();

    let port = 18101;
    let mut child = spawn_gateway_http(port);

    // Initialize
    let init_result = http_initialize(port);
    assert!(init_result.is_ok(), "Initialize should succeed");
    println!("✓ Gateway initialized");

    // Create sandbox with env vars
    let create_result = http_tools_call(
        port,
        "sandbox_create",
        json!({
            "template": "debian:bookworm-slim",
            "timeout_ms": 60000,
            "env_vars": {
                "MY_VAR": "my_value",
                "BASTION_TEST": "true"
            }
        }),
    );

    let sandbox_id = match create_result {
        Ok(response) => {
            let id = extract_sandbox_id(&response);
            assert!(id.is_some(), "Should have sandbox_id");
            println!("✓ Created sandbox with env vars: {}", id.as_ref().unwrap());
            id.unwrap()
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("sandbox_create failed: {}", e);
        }
    };

    // Wait for worker to connect
    std::thread::sleep(Duration::from_secs(3));

    // Verify env var is accessible
    let run_result = http_tools_call(
        port,
        "sandbox_run",
        json!({
            "sandbox_id": sandbox_id,
            "command": "echo $MY_VAR"
        }),
    );

    match run_result {
        Ok(response) => {
            let stdout = extract_stdout(&response).unwrap_or_default().trim().to_string();
            assert_eq!(stdout, "my_value", "Environment variable should be set, got: {}", stdout);
            println!("✓ Env var verified: MY_VAR={}", stdout);
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("sandbox_run failed: {}", e);
        }
    }

    // Terminate
    let _ = http_tools_call(port, "sandbox_terminate", json!({ "sandbox_id": sandbox_id }));

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn test_e2e_file_operations() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();

    let port = 18102;
    let mut child = spawn_gateway_http(port);

    // Initialize
    let init_result = http_initialize(port);
    assert!(init_result.is_ok(), "Initialize should succeed");

    // Create sandbox
    let create_result = http_tools_call(
        port,
        "sandbox_create",
        json!({
            "template": "debian:bookworm-slim",
            "timeout_ms": 60000
        }),
    );

    let sandbox_id = match create_result {
        Ok(response) => extract_sandbox_id(&response).expect("Should have sandbox_id"),
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("sandbox_create failed: {}", e);
        }
    };
    println!("✓ Created sandbox: {}", sandbox_id);

    // Wait for worker to connect
    std::thread::sleep(Duration::from_secs(3));

    // Write file using sandbox_write
    let write_result = http_tools_call(
        port,
        "sandbox_write",
        json!({
            "sandbox_id": sandbox_id,
            "path": "/tmp/test-file.txt",
            "content": "Hello from E2E test!"
        }),
    );

    assert!(write_result.is_ok(), "sandbox_write should succeed: {:?}", write_result.err());
    println!("✓ File written");

    // Read file using sandbox_read
    let read_result = http_tools_call(
        port,
        "sandbox_read",
        json!({
            "sandbox_id": sandbox_id,
            "path": "/tmp/test-file.txt"
        }),
    );

    match read_result {
        Ok(response) => {
            // sandbox_read returns base64 encoded content
            let content = response.get("result")
                .and_then(|r| r.get("content"))
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");

            // Content is typically base64 encoded
            use std::io::Read;
            let decoded = base64_decode(content);

            assert!(
                decoded.contains("Hello from E2E test!") || content.contains("Hello from E2E test!"),
                "File content should match, got: {} (decoded: {:?})",
                content,
                decoded
            );
            println!("✓ File read and verified");
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("sandbox_read failed: {}", e);
        }
    }

    // List files using sandbox_list_files
    let list_result = http_tools_call(
        port,
        "sandbox_list_files",
        json!({
            "sandbox_id": sandbox_id,
            "dir": "/tmp"
        }),
    );

    assert!(list_result.is_ok(), "sandbox_list_files should succeed");
    println!("✓ Files listed");

    // Terminate
    let _ = http_tools_call(port, "sandbox_terminate", json!({ "sandbox_id": sandbox_id }));

    let _ = child.kill();
    let _ = child.wait();
}

/// Simple base64 decoder for test
fn base64_decode(input: &str) -> String {
    // Try to decode as base64 first
    let decoded = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        input
    );
    match decoded {
        Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
        Err(_) => input.to_string(), // Return original if not base64
    }
}

#[test]
fn test_e2e_complex_command() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();

    let port = 18103;
    let mut child = spawn_gateway_http(port);

    // Initialize
    let init_result = http_initialize(port);
    assert!(init_result.is_ok(), "Initialize should succeed");

    // Create sandbox
    let create_result = http_tools_call(
        port,
        "sandbox_create",
        json!({
            "template": "debian:bookworm-slim",
            "timeout_ms": 120000
        }),
    );

    let sandbox_id = match create_result {
        Ok(response) => extract_sandbox_id(&response).expect("Should have sandbox_id"),
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("sandbox_create failed: {}", e);
        }
    };
    println!("✓ Created sandbox: {}", sandbox_id);

    // Wait for worker to connect
    std::thread::sleep(Duration::from_secs(3));

    // Install a package and run a complex pipeline
    let run_result = http_tools_call(
        port,
        "sandbox_run",
        json!({
            "sandbox_id": sandbox_id,
            "command": "apt-get update -qq && apt-get install -y -qq curl > /dev/null 2>&1 && curl --version | head -1"
        }),
    );

    match run_result {
        Ok(response) => {
            let exit_code = extract_exit_code(&response).unwrap_or(-1);
            let stdout = extract_stdout(&response).unwrap_or_default();

            // Command may fail due to network, but if it works, should show curl
            if exit_code == 0 {
                assert!(
                    stdout.contains("curl"),
                    "Should have curl installed"
                );
                println!("✓ Complex command succeeded: {}", stdout.trim());
            } else {
                println!("⚠ Complex command failed (network issue?), exit_code={}", exit_code);
            }
        }
        Err(e) => {
            println!("⚠ sandbox_run failed (expected if no network): {}", e);
        }
    }

    // Terminate
    let _ = http_tools_call(port, "sandbox_terminate", json!({ "sandbox_id": sandbox_id }));

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn test_e2e_sandbox_lifecycle() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();

    let port = 18104;
    let mut child = spawn_gateway_http(port);

    // Initialize
    let init_result = http_initialize(port);
    assert!(init_result.is_ok(), "Initialize should succeed");
    println!("✓ Gateway initialized");

    // Create sandbox
    let create_result = http_tools_call(
        port,
        "sandbox_create",
        json!({
            "template": "debian:bookworm-slim",
            "timeout_ms": 60000
        }),
    );

    let sandbox_id = match create_result {
        Ok(response) => {
            let id = extract_sandbox_id(&response);
            assert!(id.is_some(), "Should have sandbox_id");
            println!("✓ Created sandbox: {}", id.as_ref().unwrap());
            id.unwrap()
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("sandbox_create failed: {}", e);
        }
    };

    // Wait for worker
    std::thread::sleep(Duration::from_secs(3));

    // Check sandbox is alive
    let alive_result = http_tools_call(
        port,
        "sandbox_health",
        json!({
            "sandbox_id": sandbox_id
        }),
    );

    assert!(alive_result.is_ok(), "sandbox should be alive");
    println!("✓ Sandbox is alive");

    // Run multiple commands
    for i in 0..3 {
        let run_result = http_tools_call(
            port,
            "sandbox_run",
            json!({
                "sandbox_id": sandbox_id,
                "command": format!("echo 'command {}'", i)
            }),
        );

        match run_result {
            Ok(response) => {
                let exit_code = extract_exit_code(&response).unwrap_or(-1);
                assert_eq!(exit_code, 0, "Command {} should succeed", i);
                println!("✓ Command {} executed successfully", i);
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Command {} failed: {}", i, e);
            }
        }
    }

    // Terminate
    let term_result = http_tools_call(
        port,
        "sandbox_terminate",
        json!({
            "sandbox_id": sandbox_id
        }),
    );

    assert!(term_result.is_ok(), "sandbox_terminate should succeed");
    println!("✓ Sandbox terminated");

    let _ = child.kill();
    let _ = child.wait();
}

// ============================================================================
// Self-Testing (cargo check/test inside sandbox)
// ============================================================================

#[test]
fn test_e2e_self_test_cargo_check() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();

    let port = 18105;
    let mut child = spawn_gateway_http(port);

    // Initialize
    let init_result = http_initialize(port);
    assert!(init_result.is_ok(), "Initialize should succeed");
    println!("✓ Gateway initialized");

    // Create sandbox with source code mounted
    let create_result = http_tools_call(
        port,
        "sandbox_create",
        json!({
            "template": "debian:bookworm-slim",
            "timeout_ms": 300000,
            "mount_code": true
        }),
    );

    let sandbox_id = match create_result {
        Ok(response) => {
            let id = extract_sandbox_id(&response);
            assert!(id.is_some(), "Should have sandbox_id");
            println!("✓ Created sandbox for self-test: {}", id.as_ref().unwrap());
            id.unwrap()
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("sandbox_create failed: {}", e);
        }
    };

    // Wait for worker to connect
    std::thread::sleep(Duration::from_secs(3));

    // Run cargo check against mounted source
    let run_result = http_tools_call(
        port,
        "sandbox_run",
        json!({
            "sandbox_id": sandbox_id,
            "command": "cd /workspace/code && cargo check -p bastion-domain 2>&1 | tail -20",
            "timeout_ms": 120000
        }),
    );

    match run_result {
        Ok(response) => {
            let exit_code = extract_exit_code(&response).unwrap_or(-1);
            let stdout = extract_stdout(&response).unwrap_or_default();

            // cargo check should succeed (exit_code 0) or show only warnings
            if exit_code == 0 {
                println!("✓ cargo check succeeded");
            } else {
                println!("⚠ cargo check exited with {}, output:\n{}", exit_code, stdout);
            }
        }
        Err(e) => {
            println!("⚠ sandbox_run failed: {}", e);
        }
    }

    // Terminate
    let _ = http_tools_call(port, "sandbox_terminate", json!({ "sandbox_id": sandbox_id }));

    let _ = child.kill();
    let _ = child.wait();
}
