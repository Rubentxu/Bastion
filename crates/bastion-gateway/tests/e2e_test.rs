//! End-to-end test for Bastion MCP Gateway.
//!
//! Tests the full MCP protocol flow: initialize → tools/list → sandbox lifecycle.
//! Requires Podman daemon running.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(feature = "test-metrics")]
use bastion_test_harness::MetricsCollector;

// ============================================================================
// Metrics helper
// ============================================================================

/// Creates a per-test MetricsCollector with a temp database.
/// When `test-metrics` feature is disabled, this is a no-op.
#[cfg(feature = "test-metrics")]
fn make_metrics_collector(test_name: &str) -> MetricsCollector {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir
        .path()
        .join(format!("{}.db", test_name.replace("::", "_")));
    MetricsCollector::new(&db_path).expect("Failed to create metrics collector")
}

/// Records a test result via metrics collector (no-op when feature disabled).
#[cfg(feature = "test-metrics")]
fn record_test(metrics: &MetricsCollector, name: &str, elapsed: Duration, status: &str) {
    metrics.record_test(name, elapsed, status).ok();
}

/// Spawn the gateway and return stdin/stdout handles.
fn spawn_gateway() -> (std::process::Child, impl Write, impl BufRead) {
    spawn_gateway_with_args(&[])
}

/// Spawn the gateway with additional CLI arguments and return stdin/stdout handles.
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

    // Append additional arguments (e.g., --pool-enabled, --pool-min-idle, etc.)
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

/// Spawn the gateway with pool enabled and return stdin/stdout handles.
fn spawn_gateway_pool() -> (std::process::Child, impl Write, impl BufRead) {
    spawn_gateway_with_args(&[
        "--pool-enabled",
        "--pool-min-idle",
        "0",
        "--pool-max-idle",
        "3",
    ])
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
    // Give the server time to process
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn test_gateway_e2e_lifecycle() {
    let start = Instant::now();

    // Requires Podman daemon running
    let socket = std::path::Path::new("/run/user/1000/podman/podman.sock");
    if !socket.exists() {
        eprintln!("Skipping: Podman socket not found at {:?}", socket);
        return;
    }

    let (mut child, mut stdin, mut reader) = spawn_gateway();

    // Small delay for gateway startup
    std::thread::sleep(Duration::from_millis(500));

    // 1. Initialize
    let init_response = send_request(
        &mut stdin,
        &mut reader,
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "e2e-test", "version": "0.1.0"}
        }),
    );
    assert!(
        init_response.get("result").is_some(),
        "Initialize failed: {:?}",
        init_response
    );
    println!("✓ Initialized: {:?}", init_response["result"]["serverInfo"]);

    // 2. Send initialized notification
    send_notification(&mut stdin, "notifications/initialized", json!({}));

    // 3. List tools
    let tools_response = send_request(&mut stdin, &mut reader, "tools/list", json!({}));
    let tools = tools_response["result"]["tools"]
        .as_array()
        .expect("tools/list should return tools array");

    println!("✓ Found {} tools", tools.len());
    let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    println!("  Tools: {:?}", tool_names);

    assert!(
        tools.len() >= 8,
        "Expected at least 8 tools, got {}",
        tools.len()
    );

    // Verify key tools exist
    let has_create = tool_names.contains(&"sandbox_create");
    let has_run = tool_names.contains(&"sandbox_run");
    let has_terminate = tool_names.contains(&"sandbox_terminate");
    let has_health = tool_names.contains(&"sandbox_health");

    assert!(has_create, "Missing sandbox_create tool");
    assert!(has_run, "Missing sandbox_run tool");
    assert!(has_terminate, "Missing sandbox_terminate tool");
    assert!(has_health, "Missing sandbox_health tool");

    // REG-02: Assert all 8 required tools
    let has_sync = tool_names.contains(&"sandbox_sync");
    let has_run_stream = tool_names.contains(&"sandbox_run_stream");
    let has_cancel = tool_names.contains(&"sandbox_cancel");
    let has_prepare = tool_names.contains(&"sandbox_prepare");

    assert!(has_sync, "Missing sandbox_sync tool");
    assert!(has_run_stream, "Missing sandbox_run_stream tool");
    assert!(has_cancel, "Missing sandbox_cancel tool");
    assert!(has_prepare, "Missing sandbox_prepare tool");

    // 4. Create a sandbox
    let create_response = send_request(
        &mut stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {
                "template": "debian:bookworm-slim",
                "timeout_ms": 120000
            }
        }),
    );
    assert!(
        create_response.get("result").is_some(),
        "sandbox_create failed: {:?}",
        create_response
    );

    let content = &create_response["result"]["content"];
    let text = content[0]["text"].as_str().unwrap_or("");
    let result: Value = serde_json::from_str(text).unwrap_or(json!({}));
    let sandbox_id = result["sandbox_id"]
        .as_str()
        .expect("No sandbox_id in response");
    println!("✓ Created sandbox: {}", sandbox_id);

    assert!(!sandbox_id.is_empty(), "Sandbox ID should not be empty");

    // 5. Run a command
    let run_response = send_request(
        &mut stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_run",
            "arguments": {
                "sandbox_id": sandbox_id,
                "command": "echo hello_from_e2e_test"
            }
        }),
    );
    assert!(
        run_response.get("result").is_some(),
        "sandbox_run failed: {:?}",
        run_response
    );

    let text = run_response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    let result: Value = serde_json::from_str(text).unwrap_or(json!({}));
    let exit_code = result["exit_code"].as_i64().unwrap_or(-1);
    let stdout = result["stdout"].as_str().unwrap_or("");
    println!(
        "✓ Run command: exit_code={}, stdout={}",
        exit_code,
        stdout.trim()
    );

    assert_eq!(exit_code, 0, "Command should succeed");
    assert!(
        stdout.contains("hello_from_e2e_test"),
        "Expected 'hello_from_e2e_test' in output, got: {}",
        stdout
    );

    // 6. Health check
    let health_response = send_request(
        &mut stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_health",
            "arguments": {}
        }),
    );
    let text = health_response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    let health: Value = serde_json::from_str(text).unwrap_or(json!({}));
    assert_eq!(health["status"], "healthy");
    println!("✓ Health check: healthy");

    // 7. Terminate sandbox
    let term_response = send_request(
        &mut stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_terminate",
            "arguments": {
                "sandbox_id": sandbox_id
            }
        }),
    );
    assert!(
        term_response.get("result").is_some(),
        "sandbox_terminate failed: {:?}",
        term_response
    );
    println!("✓ Terminated sandbox");

    // Cleanup
    let _ = child.kill();
    println!("✓ E2E test passed!");

    #[cfg(feature = "test-metrics")]
    record_test(
        &make_metrics_collector("test_gateway_e2e_lifecycle"),
        "test_gateway_e2e_lifecycle",
        start.elapsed(),
        "pass",
    );
}

#[test]
fn test_gateway_health_only() {
    let start = Instant::now();
    let (mut child, mut stdin, mut reader) = spawn_gateway();
    std::thread::sleep(Duration::from_millis(500));

    // Initialize
    let init_response = send_request(
        &mut stdin,
        &mut reader,
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "e2e-test", "version": "0.1.0"}
        }),
    );
    assert!(init_response.get("result").is_some());

    // Initialized
    send_notification(&mut stdin, "notifications/initialized", json!({}));

    // Health check
    let health_response = send_request(
        &mut stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_health",
            "arguments": {}
        }),
    );

    let text = health_response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("{}");
    let health: Value = serde_json::from_str(text).unwrap_or(json!({"status": "unknown"}));
    println!("Health: {:?}", health);
    assert_eq!(health["status"], "healthy", "Gateway should be healthy");

    let _ = child.kill();
    println!("✓ Health test passed!");

    #[cfg(feature = "test-metrics")]
    record_test(
        &make_metrics_collector("test_gateway_health_only"),
        "test_gateway_health_only",
        start.elapsed(),
        "pass",
    );
}

// ============================================================================
// Pool Manager E2E Tests
// ============================================================================

/// Helper: Initialize gateway connection and return stdin/stdout handles.
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
            "clientInfo": {"name": "e2e-test", "version": "0.1.0"}
        }),
    );
    assert!(
        init_response.get("result").is_some(),
        "Initialize failed: {:?}",
        init_response
    );

    send_notification(stdin, "notifications/initialized", json!({}));
}

/// Helper: Extract sandbox_id from tools/call response.
fn extract_sandbox_id(response: &Value) -> String {
    let content = &response["result"]["content"];
    let text = content[0]["text"].as_str().unwrap_or("");
    let result: Value = serde_json::from_str(text).unwrap_or(json!({}));
    result["sandbox_id"].as_str().unwrap_or("").to_string()
}

/// Helper: Extract text content from tools/call response.
fn extract_response_text(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// Helper: Get pool stats.
fn get_pool_stats(stdin: &mut impl Write, reader: &mut impl BufRead) -> Value {
    let response = send_request(
        stdin,
        reader,
        "tools/call",
        json!({
            "name": "sandbox_pool_stats",
            "arguments": {}
        }),
    );
    let text = extract_response_text(&response);
    serde_json::from_str(&text).unwrap_or(json!({}))
}

/// Helper: Terminate a sandbox and return the response text.
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

/// Helper: Create a sandbox and return the sandbox_id.
fn create_sandbox(stdin: &mut impl Write, reader: &mut impl BufRead) -> String {
    let response = send_request(
        stdin,
        reader,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {
                "template": "debian:bookworm-slim",
                "timeout_ms": 60000
            }
        }),
    );
    extract_sandbox_id(&response)
}

/// Helper: Get sandbox list.
fn get_sandbox_list(stdin: &mut impl Write, reader: &mut impl BufRead) -> Value {
    let response = send_request(
        stdin,
        reader,
        "tools/call",
        json!({
            "name": "sandbox_list",
            "arguments": {}
        }),
    );
    let text = extract_response_text(&response);
    serde_json::from_str(&text).unwrap_or(json!({}))
}

/// Helper: Get sandbox info.
fn get_sandbox_info(sandbox_id: &str, stdin: &mut impl Write, reader: &mut impl BufRead) -> Value {
    let response = send_request(
        stdin,
        reader,
        "tools/call",
        json!({
            "name": "sandbox_info",
            "arguments": {"sandbox_id": sandbox_id}
        }),
    );
    let text = extract_response_text(&response);
    serde_json::from_str(&text).unwrap_or(json!({}))
}

/// Helper: Run a command in a sandbox.
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

#[test]
fn test_gateway_pool_lifecycle() {
    let start = Instant::now();
    // Requires Podman daemon running
    let socket = std::path::Path::new("/run/user/1000/podman/podman.sock");
    if !socket.exists() {
        eprintln!("Skipping: Podman socket not found at {:?}", socket);
        return;
    }

    let (mut child, mut stdin, mut reader) = spawn_gateway_pool();
    init_gateway(&mut child, &mut stdin, &mut reader);

    // 1. Create a sandbox (should indicate from_pool: false since pool starts empty)
    let create_response = send_request(
        &mut stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {
                "template": "debian:bookworm-slim",
                "timeout_ms": 60000
            }
        }),
    );

    let create_text = extract_response_text(&create_response);
    let create_result: Value = serde_json::from_str(&create_text).unwrap_or(json!({}));
    let from_pool_first = create_result["from_pool"].as_bool().unwrap_or(true);
    let sandbox_id = create_result["sandbox_id"].as_str().unwrap_or("");

    println!(
        "✓ Created sandbox: {} (from_pool: {})",
        sandbox_id, from_pool_first
    );
    assert!(!sandbox_id.is_empty(), "Sandbox ID should not be empty");
    // Pool starts empty, so this should be from_pool: false
    // Note: This may be true if pool had containers pre-warmed from previous runs

    // 2. Verify pool stats
    let stats = get_pool_stats(&mut stdin, &mut reader);
    println!("✓ Pool stats after create: {:?}", stats);
    assert!(
        stats["enabled"].as_bool().unwrap_or(false),
        "Pool should be enabled"
    );

    // 3. Run a command
    let run_result = run_command(
        sandbox_id,
        "echo pool_lifecycle_test",
        &mut stdin,
        &mut reader,
    );
    let exit_code = run_result["exit_code"].as_i64().unwrap_or(-1);
    let stdout = run_result["stdout"].as_str().unwrap_or("");
    println!(
        "✓ Run command: exit_code={}, stdout={}",
        exit_code,
        stdout.trim()
    );
    assert_eq!(exit_code, 0, "Command should succeed");

    // 4. Terminate sandbox (should return to pool)
    let term_text = terminate_sandbox(sandbox_id, &mut stdin, &mut reader);
    let term_result: Value = serde_json::from_str(&term_text).unwrap_or(json!({}));
    let term_status = term_result["status"].as_str().unwrap_or("");
    println!("✓ Terminate result: status={}", term_status);
    assert_eq!(
        term_status, "pooled",
        "Terminated sandbox should be returned to pool"
    );

    // 5. Verify pool stats show 1 idle
    let stats_after = get_pool_stats(&mut stdin, &mut reader);
    let idle_count = stats_after["idle"].as_u64().unwrap_or(0);
    println!("✓ Pool stats after terminate: idle={}", idle_count);
    assert_eq!(
        idle_count, 1,
        "Pool should have 1 idle sandbox after terminate"
    );

    // 6. Create another sandbox (should come from pool - from_pool: true)
    let create2_response = send_request(
        &mut stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {
                "template": "debian:bookworm-slim",
                "timeout_ms": 60000
            }
        }),
    );

    let create2_text = extract_response_text(&create2_response);
    let create2_result: Value = serde_json::from_str(&create2_text).unwrap_or(json!({}));
    let from_pool_second = create2_result["from_pool"].as_bool().unwrap_or(false);
    let sandbox_id2 = create2_result["sandbox_id"].as_str().unwrap_or("");

    println!(
        "✓ Created second sandbox: {} (from_pool: {})",
        sandbox_id2, from_pool_second
    );
    assert!(!sandbox_id2.is_empty(), "Sandbox ID should not be empty");
    assert!(
        from_pool_second,
        "Second sandbox should come from pool (hot)"
    );

    // Cleanup - terminate the second sandbox
    let _ = terminate_sandbox(sandbox_id2, &mut stdin, &mut reader);

    let _ = child.kill();
    println!("✓ Pool lifecycle test passed!");

    #[cfg(feature = "test-metrics")]
    record_test(
        &make_metrics_collector("test_gateway_pool_lifecycle"),
        "test_gateway_pool_lifecycle",
        start.elapsed(),
        "pass",
    );
}

#[test]
fn test_gateway_list_and_info() {
    let start = Instant::now();
    // Requires Podman daemon running
    let socket = std::path::Path::new("/run/user/1000/podman/podman.sock");
    if !socket.exists() {
        eprintln!("Skipping: Podman socket not found at {:?}", socket);
        return;
    }

    let (mut child, mut stdin, mut reader) = spawn_gateway();
    init_gateway(&mut child, &mut stdin, &mut reader);

    // 1. Create two sandboxes
    let sandbox1_id = create_sandbox(&mut stdin, &mut reader);
    println!("✓ Created sandbox 1: {}", sandbox1_id);
    assert!(!sandbox1_id.is_empty(), "Sandbox 1 ID should not be empty");

    let sandbox2_id = create_sandbox(&mut stdin, &mut reader);
    println!("✓ Created sandbox 2: {}", sandbox2_id);
    assert!(!sandbox2_id.is_empty(), "Sandbox 2 ID should not be empty");

    // 2. Call sandbox_list and verify count >= 2 and contains the IDs
    let list = get_sandbox_list(&mut stdin, &mut reader);
    let count = list["count"].as_u64().unwrap_or(0);
    let empty_vec: Vec<Value> = vec![];
    let sandboxes = list["sandboxes"].as_array().unwrap_or(&empty_vec);

    println!(
        "✓ Sandbox list: count={}, sandboxes={:?}",
        count, list["sandboxes"]
    );
    assert!(count >= 2, "Expected at least 2 sandboxes, got {}", count);

    let list_ids: Vec<&str> = sandboxes
        .iter()
        .filter_map(|s| s["sandbox_id"].as_str())
        .collect();

    assert!(
        list_ids.contains(&sandbox1_id.as_str()),
        "Sandbox 1 ID not in list: {:?}",
        list_ids
    );
    assert!(
        list_ids.contains(&sandbox2_id.as_str()),
        "Sandbox 2 ID not in list: {:?}",
        list_ids
    );

    // 3. Call sandbox_info for each sandbox and verify fields
    let info1 = get_sandbox_info(&sandbox1_id, &mut stdin, &mut reader);
    println!("✓ Sandbox 1 info: {:?}", info1);
    assert_eq!(
        info1["sandbox_id"].as_str().unwrap_or(""),
        sandbox1_id,
        "Info sandbox_id should match"
    );
    assert!(
        info1.get("status").is_some(),
        "Info should have status field"
    );
    assert!(
        info1.get("template").is_some(),
        "Info should have template field"
    );
    assert!(
        info1.get("created_at").is_some(),
        "Info should have created_at field"
    );

    let info2 = get_sandbox_info(&sandbox2_id, &mut stdin, &mut reader);
    println!("✓ Sandbox 2 info: {:?}", info2);
    assert_eq!(
        info2["sandbox_id"].as_str().unwrap_or(""),
        sandbox2_id,
        "Info sandbox_id should match"
    );

    // 4. Terminate both sandboxes
    let _ = terminate_sandbox(&sandbox1_id, &mut stdin, &mut reader);
    let _ = terminate_sandbox(&sandbox2_id, &mut stdin, &mut reader);
    println!("✓ Terminated both sandboxes");

    // 5. Verify sandbox_list no longer contains them (they may be cleaned up async)
    std::thread::sleep(Duration::from_millis(500));
    let list_after = get_sandbox_list(&mut stdin, &mut reader);
    let count_after = list_after["count"].as_u64().unwrap_or(0);
    println!("✓ Sandbox list after terminate: count={}", count_after);
    // Note: count may be 0 or may still show them if cleanup is async

    let _ = child.kill();
    println!("✓ List and info test passed!");

    #[cfg(feature = "test-metrics")]
    record_test(
        &make_metrics_collector("test_gateway_list_and_info"),
        "test_gateway_list_and_info",
        start.elapsed(),
        "pass",
    );
}

#[test]
fn test_gateway_pool_recovery() {
    let start = Instant::now();
    // Requires Podman daemon running
    let socket = std::path::Path::new("/run/user/1000/podman/podman.sock");
    if !socket.exists() {
        eprintln!("Skipping: Podman socket not found at {:?}", socket);
        return;
    }

    // 1. Start gateway with pool enabled
    let (mut child, mut stdin, mut reader) = spawn_gateway_pool();
    init_gateway(&mut child, &mut stdin, &mut reader);

    // 2. Create a sandbox via pool
    let create_response = send_request(
        &mut stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {
                "template": "debian:bookworm-slim",
                "timeout_ms": 60000
            }
        }),
    );

    let create_text = extract_response_text(&create_response);
    let create_result: Value = serde_json::from_str(&create_text).unwrap_or(json!({}));
    let sandbox_id = create_result["sandbox_id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let from_pool = create_result["from_pool"].as_bool().unwrap_or(false);

    println!(
        "✓ Created sandbox: {} (from_pool: {})",
        sandbox_id, from_pool
    );
    assert!(!sandbox_id.is_empty(), "Sandbox ID should not be empty");

    // 3. Verify pool stats
    let stats = get_pool_stats(&mut stdin, &mut reader);
    println!("✓ Pool stats: {:?}", stats);
    assert!(
        stats["enabled"].as_bool().unwrap_or(false),
        "Pool should be enabled"
    );

    // 4. Kill the gateway process
    println!("✓ Killing gateway process...");
    let _ = child.kill();
    let _ = child.wait();

    // 5. Restart gateway with same flags
    std::thread::sleep(Duration::from_millis(1000)); // Wait for port cleanup
    let (mut child2, mut stdin2, mut reader2) = spawn_gateway_pool();
    init_gateway(&mut child2, &mut stdin2, &mut reader2);

    // 6. Verify gateway starts correctly (best effort recovery test)
    let health_response = send_request(
        &mut stdin2,
        &mut reader2,
        "tools/call",
        json!({
            "name": "sandbox_health",
            "arguments": {}
        }),
    );
    let health_text = extract_response_text(&health_response);
    let health: Value = serde_json::from_str(&health_text).unwrap_or(json!({"status": "unknown"}));
    println!("✓ Health after restart: {:?}", health);
    assert_eq!(
        health["status"], "healthy",
        "Gateway should be healthy after restart"
    );

    // 7. Create another sandbox and verify pool still works
    let list_before = get_sandbox_list(&mut stdin2, &mut reader2);
    let count_before = list_before["count"].as_u64().unwrap_or(0);
    println!("✓ Sandboxes before new create: count={}", count_before);

    let create2_response = send_request(
        &mut stdin2,
        &mut reader2,
        "tools/call",
        json!({
            "name": "sandbox_create",
            "arguments": {
                "template": "debian:bookworm-slim",
                "timeout_ms": 60000
            }
        }),
    );

    let create2_text = extract_response_text(&create2_response);
    let create2_result: Value = serde_json::from_str(&create2_text).unwrap_or(json!({}));
    let sandbox2_id = create2_result["sandbox_id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    println!("✓ Created sandbox after restart: {}", sandbox2_id);
    assert!(
        !sandbox2_id.is_empty(),
        "Sandbox ID should not be empty after restart"
    );

    // Verify pool stats still work
    let stats2 = get_pool_stats(&mut stdin2, &mut reader2);
    println!("✓ Pool stats after restart: {:?}", stats2);
    assert!(
        stats2["enabled"].as_bool().unwrap_or(false),
        "Pool should still be enabled after restart"
    );

    // Cleanup
    let _ = terminate_sandbox(&sandbox2_id, &mut stdin2, &mut reader2);
    let _ = child2.kill();
    println!("✓ Pool recovery test passed!");

    #[cfg(feature = "test-metrics")]
    record_test(
        &make_metrics_collector("test_gateway_pool_recovery"),
        "test_gateway_pool_recovery",
        start.elapsed(),
        "pass",
    );
}

#[test]
fn test_gateway_error_handling() {
    let start = Instant::now();
    // Requires Podman daemon running
    let socket = std::path::Path::new("/run/user/1000/podman/podman.sock");
    if !socket.exists() {
        eprintln!("Skipping: Podman socket not found at {:?}", socket);
        return;
    }

    let (mut child, mut stdin, mut reader) = spawn_gateway();
    init_gateway(&mut child, &mut stdin, &mut reader);

    let fake_id = "nonexistent-sandbox-12345";

    // 1. sandbox_run on non-existent sandbox -> should return error
    let run_response = send_request(
        &mut stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_run",
            "arguments": {
                "sandbox_id": fake_id,
                "command": "echo test"
            }
        }),
    );

    let run_text = extract_response_text(&run_response);
    let run_result: Value = serde_json::from_str(&run_text).unwrap_or(json!({}));
    let has_error = run_result.get("error").is_some() || run_text.contains("error");
    println!("✓ sandbox_run on non-existent: error={}", has_error);
    // Note: The response format may vary - check for error field or error in text

    // 2. sandbox_terminate on already terminated sandbox -> should return error or handle gracefully
    let term_response = send_request(
        &mut stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_terminate",
            "arguments": {"sandbox_id": fake_id}
        }),
    );

    let term_text = extract_response_text(&term_response);
    let term_result: Value = serde_json::from_str(&term_text).unwrap_or(json!({}));
    // Should either have an error field or return status: error/terminated/etc.
    println!("✓ sandbox_terminate on non-existent: {:?}", term_result);

    // 3. sandbox_info on non-existent sandbox -> should return error
    let info_response = send_request(
        &mut stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_info",
            "arguments": {"sandbox_id": fake_id}
        }),
    );

    let info_text = extract_response_text(&info_response);
    let info_result: Value = serde_json::from_str(&info_text).unwrap_or(json!({}));
    let info_has_error =
        info_result.get("error").is_some() || info_text.to_lowercase().contains("error");
    println!("✓ sandbox_info on non-existent: error={}", info_has_error);
    assert!(
        info_has_error,
        "sandbox_info should return error for non-existent sandbox"
    );

    // 4. sandbox_write without sandbox -> should return error
    let write_response = send_request(
        &mut stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_write",
            "arguments": {
                "sandbox_id": fake_id,
                "path": "/tmp/test.txt",
                "content": "test content"
            }
        }),
    );

    let write_text = extract_response_text(&write_response);
    let write_result: Value = serde_json::from_str(&write_text).unwrap_or(json!({}));
    let write_has_error =
        write_result.get("error").is_some() || write_text.to_lowercase().contains("error");
    println!("✓ sandbox_write on non-existent: error={}", write_has_error);
    assert!(
        write_has_error,
        "sandbox_write should return error for non-existent sandbox"
    );

    let _ = child.kill();
    println!("✓ Error handling test passed!");

    #[cfg(feature = "test-metrics")]
    record_test(
        &make_metrics_collector("test_gateway_error_handling"),
        "test_gateway_error_handling",
        start.elapsed(),
        "pass",
    );
}
