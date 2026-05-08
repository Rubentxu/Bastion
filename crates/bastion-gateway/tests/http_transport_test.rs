//! HTTP Transport tests for Bastion MCP Gateway.
//!
//! Tests the Streamable HTTP transport mode which enables:
//! - Daemon mode (no stdin/stdout dependency)
//! - Remote worker connections
//! - HTTP-based MCP protocol with SSE responses
//!
//! ## Running These Tests
//!
//! ```bash
//! # Run with debug build
//! cargo test -p bastion-gateway --test http_transport_test
//!
//! # Run with release build
//! cargo test -p bastion-gateway --test http_transport_test -- --release
//! ```
//!
//! ## Prerequisites
//!
//! - Podman daemon at `/run/user/1000/podman/podman.sock`
//! - Gateway binary built at `target/debug/bastion-gateway` (or release)
//! - Worker binary built at `target/debug/bastion-worker`

use serde_json::{Value, json};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Gateway binary path (debug or release depending on test profile)
fn gateway_binary() -> std::path::PathBuf {
    // Determine if we're in release mode by checking CARGO_MANIFEST_DIR
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    workspace_root.join(format!("target/{}/bastion-gateway", profile))
}

/// Worker binary path
fn worker_binary() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    workspace_root.join(format!("target/{}/bastion-worker", profile))
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
            "SKIP: Worker binary not found at {:?}. Build with: cargo build -p bastion-worker",
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
        .arg("--transport")
        .arg("http")
        .arg("--http-port")
        .arg(port.to_string())
        .arg("--image")
        .arg("debian:bookworm-slim")
        .arg("--worker-binary")
        .arg(&worker)
        .arg("--pool-enabled")
        .arg("--pool-min-idle")
        .arg("0")
        .arg("--pool-max-idle")
        .arg("2")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn bastion-gateway in HTTP mode");

    // Wait for server to start listening
    let start = std::time::Instant::now();
    let max_wait = Duration::from_secs(10);
    while start.elapsed() < max_wait {
        if std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    child
}

/// Send HTTP POST request and parse SSE response
fn http_mcp_request(port: u16, method: &str, params: Value, id: u64) -> Result<Value, String> {
    let client = reqwest::blocking::Client::new();

    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });

    let response = client
        .post(format!("http://127.0.0.1:{}/", port))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2024-11-05")
        .json(&payload)
        .timeout(Duration::from_secs(30))
        .send()
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    let status = response.status();
    let text = response
        .text()
        .map_err(|e| format!("Failed to read response: {}", e))?;

    eprintln!("DEBUG HTTP response status: {}, body: {:?}", status, text);

    // Parse SSE format: "data: {json}\n\n" or multiple data lines
    // The first "data: " may be empty, followed by "id:" and "retry:" lines
    // Then another "data: " with the actual JSON response
    let mut json_data = String::new();
    for line in text.lines() {
        if line.starts_with("data: ") {
            let after_data = &line[6..];
            // Skip empty data lines
            if !after_data.is_empty() && after_data != "" {
                json_data = after_data.to_string();
                break;
            }
        }
    }

    if json_data.is_empty() {
        return Err(format!("No JSON data found in SSE response: {}", text));
    }

    serde_json::from_str(&json_data)
        .map_err(|e| format!("Failed to parse JSON from SSE: {} - text: {}", e, json_data))
}

/// Initialize gateway via HTTP
fn http_initialize(port: u16) -> Result<Value, String> {
    http_mcp_request(
        port,
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "http-test", "version": "1.0.0"}
        }),
        0,
    )
}

/// Send tools/list request
fn http_tools_list(port: u16) -> Result<Value, String> {
    http_mcp_request(port, "tools/list", json!({}), 1)
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

// ============================================================================
// TESTS
// ============================================================================

#[test]
fn test_http_transport_gateway_starts() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();

    let port = 18080;
    let mut child = spawn_gateway_http(port);

    // Give it time to start
    std::thread::sleep(Duration::from_millis(500));

    // Check if process is still running
    match child.try_wait().unwrap() {
        Some(status) => {
            panic!("Gateway exited unexpectedly with status: {:?}", status);
        }
        None => {
            // Still running - good!
            println!("✓ Gateway is running in HTTP mode on port {}", port);
        }
    }

    // Cleanup
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn test_http_transport_tcp_connection() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();

    let port = 18081;
    let _child = spawn_gateway_http(port);

    // Try to connect to the HTTP port
    let addr = format!("127.0.0.1:{}", port);
    match TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(5)) {
        Ok(_stream) => {
            println!("✓ TCP connection to {} successful", addr);
        }
        Err(e) => {
            panic!("Failed to connect to {}: {}", addr, e);
        }
    }
}

#[test]
fn test_http_transport_initialize() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();

    let port = 18082;
    let mut child = spawn_gateway_http(port);

    // Send initialize request
    let result = http_initialize(port);

    match result {
        Ok(response) => {
            assert!(
                response.get("result").is_some(),
                "Initialize should return result, got: {:?}",
                response
            );

            let server_info = &response["result"]["serverInfo"];
            assert_eq!(server_info["name"].as_str(), Some("rmcp"));
            assert_eq!(server_info["version"].as_str(), Some("1.5.0"));

            let capabilities = &response["result"]["capabilities"];
            assert!(
                capabilities.get("tools").is_some(),
                "Should have tools capability"
            );

            println!("✓ Initialize successful: {:?}", server_info);
        }
        Err(e) => {
            panic!("Initialize failed: {}", e);
        }
    }

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn test_http_transport_tools_list() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();

    let port = 18083;
    let mut child = spawn_gateway_http(port);

    // Initialize first
    let init_result = http_initialize(port);
    assert!(init_result.is_ok(), "Initialize should succeed");

    // List tools
    let tools_result = http_tools_list(port);

    match tools_result {
        Ok(response) => {
            assert!(
                response.get("result").is_some(),
                "tools/list should return result, got: {:?}",
                response
            );

            let tools = response["result"]["tools"]
                .as_array()
                .expect("tools/list should return tools array");

            println!("✓ Found {} tools", tools.len());

            let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
            println!("  Tools: {:?}", tool_names);

            // Verify key tools exist
            assert!(
                tool_names.contains(&"sandbox_create"),
                "Missing sandbox_create"
            );
            assert!(tool_names.contains(&"sandbox_run"), "Missing sandbox_run");
            assert!(
                tool_names.contains(&"sandbox_terminate"),
                "Missing sandbox_terminate"
            );
            assert!(
                tool_names.contains(&"sandbox_health"),
                "Missing sandbox_health"
            );
        }
        Err(e) => {
            panic!("tools/list failed: {}", e);
        }
    }

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn test_http_transport_health_check() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();

    let port = 18084;
    let mut child = spawn_gateway_http(port);

    // Initialize
    let init_result = http_initialize(port);
    assert!(init_result.is_ok(), "Initialize should succeed");

    // Health check
    let health_result = http_tools_call(port, "sandbox_health", json!({}));

    match health_result {
        Ok(response) => {
            assert!(
                response.get("result").is_some(),
                "sandbox_health should return result"
            );

            let content = &response["result"]["content"];
            let text = content[0]["text"].as_str().unwrap_or("{}");
            let health: Value =
                serde_json::from_str(text).unwrap_or_else(|_| json!({"status": "unknown"}));

            assert_eq!(
                health["status"].as_str(),
                Some("healthy"),
                "Health status should be healthy, got: {:?}",
                health
            );

            println!("✓ Health check passed: {:?}", health["status"]);
        }
        Err(e) => {
            panic!("sandbox_health failed: {}", e);
        }
    }

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn test_http_transport_pool_stats() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();

    let port = 18085;
    let mut child = spawn_gateway_http(port);

    // Initialize
    let init_result = http_initialize(port);
    assert!(init_result.is_ok(), "Initialize should succeed");

    // Pool stats
    let pool_result = http_tools_call(port, "sandbox_pool_stats", json!({}));

    match pool_result {
        Ok(response) => {
            assert!(
                response.get("result").is_some(),
                "sandbox_pool_stats should return result"
            );

            let content = &response["result"]["content"];
            let text = content[0]["text"].as_str().unwrap_or("{}");
            let pool: Value = serde_json::from_str(text).unwrap_or_else(|_| json!({}));

            assert!(
                pool["enabled"].as_bool().unwrap_or(false),
                "Pool should be enabled"
            );

            println!(
                "✓ Pool stats: enabled={}, active={}, idle={}",
                pool["enabled"].as_bool().unwrap_or(false),
                pool["active"].as_u64().unwrap_or(0),
                pool["idle"].as_u64().unwrap_or(0)
            );
        }
        Err(e) => {
            panic!("sandbox_pool_stats failed: {}", e);
        }
    }

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn test_http_transport_sandbox_lifecycle() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();

    let port = 18086;
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
        Ok(response) => {
            assert!(
                response.get("result").is_some(),
                "sandbox_create should return result"
            );

            let content = &response["result"]["content"];
            let text = content[0]["text"].as_str().unwrap_or("{}");
            let result: Value = serde_json::from_str(text).unwrap_or_else(|_| json!({}));

            let id = result["sandbox_id"].as_str().unwrap_or("").to_string();
            assert!(!id.is_empty(), "sandbox_id should not be empty");
            println!("✓ Created sandbox: {}", id);
            id
        }
        Err(e) => {
            panic!("sandbox_create failed: {}", e);
        }
    };

    // Run command
    let run_result = http_tools_call(
        port,
        "sandbox_run",
        json!({
            "sandbox_id": sandbox_id,
            "command": "echo http_lifecycle_test"
        }),
    );

    match run_result {
        Ok(response) => {
            assert!(
                response.get("result").is_some(),
                "sandbox_run should return result"
            );

            let content = &response["result"]["content"];
            let text = content[0]["text"].as_str().unwrap_or("{}");
            let result: Value = serde_json::from_str(text).unwrap_or_else(|_| json!({}));

            let exit_code = result["exit_code"].as_i64().unwrap_or(-1);
            let stdout = result["stdout"].as_str().unwrap_or("");

            assert_eq!(exit_code, 0, "Command should succeed");
            assert!(
                stdout.contains("http_lifecycle_test"),
                "Output should contain test marker, got: {}",
                stdout
            );

            println!(
                "✓ Command executed: exit_code={}, stdout={}",
                exit_code,
                stdout.trim()
            );
        }
        Err(e) => {
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

    match term_result {
        Ok(response) => {
            assert!(
                response.get("result").is_some(),
                "sandbox_terminate should return result"
            );
            println!("✓ Sandbox terminated");
        }
        Err(e) => {
            panic!("sandbox_terminate failed: {}", e);
        }
    }

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn test_http_transport_invalid_request() {
    require_podman();
    require_gateway_binary();
    require_worker_binary();

    let port = 18087;
    let mut child = spawn_gateway_http(port);

    // Try to send tools/list WITHOUT initialize first
    let client = reqwest::blocking::Client::new();
    let response = client
        .post(format!("http://127.0.0.1:{}/", port))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2024-11-05")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }))
        .timeout(Duration::from_secs(10))
        .send()
        .unwrap();

    let text = response.text().unwrap();

    // Should get an error response (initialize first)
    assert!(
        text.contains("initialize") || text.contains("error"),
        "Should require initialize first, got: {}",
        text
    );

    println!("✓ Invalid request properly rejected: needs initialize first");

    let _ = child.kill();
    let _ = child.wait();
}
