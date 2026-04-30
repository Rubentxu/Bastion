//! End-to-end test for Bastion MCP Gateway.
//!
//! Tests the full MCP protocol flow: initialize → tools/list → sandbox lifecycle.
//! Requires Podman daemon running.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Spawn the gateway and return stdin/stdout handles.
fn spawn_gateway() -> (std::process::Child, impl Write, impl BufRead) {
    // Path relative to workspace root
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

    let mut child = Command::new(&binary)
        .arg("--image")
        .arg("debian:bookworm-slim")
        .arg("--worker-binary")
        .arg(&worker)
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
    // Give the server time to process
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn test_gateway_e2e_lifecycle() {
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
    let tools_response = send_request(
        &mut stdin,
        &mut reader,
        "tools/list",
        json!({}),
    );
    let tools = tools_response["result"]["tools"]
        .as_array()
        .expect("tools/list should return tools array");

    println!("✓ Found {} tools", tools.len());
    let tool_names: Vec<&str> = tools.iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
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
    let sandbox_id = result["sandbox_id"].as_str().expect("No sandbox_id in response");
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

    let text = run_response["result"]["content"][0]["text"].as_str().unwrap_or("");
    let result: Value = serde_json::from_str(text).unwrap_or(json!({}));
    let exit_code = result["exit_code"].as_i64().unwrap_or(-1);
    let stdout = result["stdout"].as_str().unwrap_or("");
    println!("✓ Run command: exit_code={}, stdout={}", exit_code, stdout.trim());

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
    let text = health_response["result"]["content"][0]["text"].as_str().unwrap_or("");
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
}

#[test]
fn test_gateway_health_only() {
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

    let text = health_response["result"]["content"][0]["text"].as_str().unwrap_or("{}");
    let health: Value = serde_json::from_str(text).unwrap_or(json!({"status": "unknown"}));
    println!("Health: {:?}", health);
    assert_eq!(health["status"], "healthy", "Gateway should be healthy");

    let _ = child.kill();
    println!("✓ Health test passed!");
}
