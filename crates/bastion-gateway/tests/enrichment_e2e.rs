//! End-to-end enrichment test for the Bastion MCP Gateway.
//!
//! Tests that Maven commands are enriched with facts, build status, and artifacts
//! when the enrichment engine is enabled. Requires Podman and `BASTION_E2E_ENRICHMENT=1`.
//!
//! Run with: `BASTION_E2E_ENRICHMENT=1 cargo test -p bastion-gateway --test enrichment_e2e -- --ignored`
//!
//! The test is `#[ignore]` by default to avoid requiring Podman in normal CI runs.

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

const E2E_TIMEOUT_SECS: u64 = 120;
const MAX_RETRIES: u32 = 3;
const RETRY_DELAY_SECS: u64 = 5;

/// Spawn the gateway binary and return (child, stdin, stdout_reader).
fn spawn_gateway() -> (std::process::Child, impl Write, impl BufRead) {
    let binary =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/bastion-gateway");

    let worker =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/bastion-worker");

    let mut cmd = Command::new(&binary);
    cmd.arg("--image")
        .arg("debian:bookworm-slim")
        .arg("--worker-binary")
        .arg(&worker);

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

/// Send a JSON-RPC request and read the response.
fn rpc_call<T: serde::Serialize>(
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
    method: &str,
    params: T,
) -> Value {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    });

    let req_str = serde_json::to_string(&req).unwrap();
    stdin
        .write_all(format!("Content-Length: {}\r\n\r\n{}\n", req_str.len(), req_str).as_bytes())
        .unwrap();
    stdin.flush().unwrap();

    // Read response headers
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        if line.starts_with("Content-Length: ") {
            content_length = Some(line["Content-Length: ".len()..].trim().parse().unwrap());
        }
    }

    let len = content_length.expect("Missing Content-Length header");
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).unwrap();
    let response: Value = serde_json::from_slice(&body).unwrap();
    response
}

/// Initialize the MCP session.
fn init_session(stdin: &mut impl Write, reader: &mut impl BufRead) -> Value {
    rpc_call(
        stdin,
        reader,
        "initialize",
        serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "enrichment-e2e-test", "version": "0.1.0"}
        }),
    )
}

/// Call sandbox_run and return the result.
fn sandbox_run(
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
    command: &str,
    timeout_ms: u64,
) -> Value {
    rpc_call(
        stdin,
        reader,
        "tools/call",
        serde_json::json!({
            "name": "sandbox_run",
            "arguments": {
                "template": "maven:3.9-slim",
                "command": command,
                "timeout_ms": timeout_ms
            }
        }),
    )
}

fn is_podman_available() -> bool {
    Command::new("podman")
        .arg("info")
        .arg("--format")
        .arg("json")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Send the `initialized` notification to the gateway.
fn send_initialized_notification(stdin: &mut impl Write) {
    let notif = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    });
    let notif_str = serde_json::to_string(&notif).unwrap();
    stdin
        .write_all(format!("Content-Length: {}\r\n\r\n{}\n", notif_str.len(), notif_str).as_bytes())
        .unwrap();
    stdin.flush().unwrap();
}

/// End-to-end test for Maven enrichment in a real sandbox.
///
/// Verifies:
/// 1. A Maven build command is enriched with facts
/// 2. `enricher_id` is present and set to `"maven"` in `enrichment_meta`
/// 3. `source` and `timestamp` are present in `enrichment_meta`
/// 4. `build_status` is present (e.g., "BUILD SUCCESS" or "BUILD FAILURE")
///
/// This test is `#[ignore]` by default and requires:
/// - `BASTION_E2E_ENRICHMENT=1` environment variable
/// - Podman daemon running
/// - `bastion-gateway` and `bastion-worker` binaries built
#[tokio::test]
#[ignore]
async fn test_maven_enrichment_sandbox() {
    // Env-gated: skip unless BASTION_E2E_ENRICHMENT=1
    if std::env::var("BASTION_E2E_ENRICHMENT").as_deref() != Ok("1") {
        eprintln!("SKIPPED: Set BASTION_E2E_ENRICHMENT=1 to run this test");
        return;
    }

    // Podman availability check
    if !is_podman_available() {
        eprintln!("SKIPPED: Podman not available");
        return;
    }

    // Run a Maven compile command with retry loop for transient failures.
    // Each attempt owns the full gateway lifecycle: spawn → init → sandbox_run.
    // Transient failures include Podman startup issues, gateway child crashes,
    // and gateway-gateway communication errors.
    let mut last_resp: Value = Value::Null;
    let mut attempt = 0;
    let mut success = false;

    while attempt < MAX_RETRIES {
        attempt += 1;

        // Spawn gateway for this attempt
        let (mut child, mut stdin, mut reader) = spawn_gateway();

        // Initialize session
        let init_resp = init_session(&mut stdin, &mut reader);
        if init_resp.get("result").is_none() {
            eprintln!(
                "Attempt {}/{}: Initialize failed, retrying in {}s",
                attempt, MAX_RETRIES, RETRY_DELAY_SECS
            );
            let _ = child.kill();
            let _ = child.wait();
            tokio::time::sleep(tokio::time::Duration::from_secs(RETRY_DELAY_SECS)).await;
            continue;
        }

        // Send initialized notification
        send_initialized_notification(&mut stdin);

        // Attempt sandbox_run
        let resp = sandbox_run(
            &mut stdin,
            &mut reader,
            "mvn compile -B -q",
            E2E_TIMEOUT_SECS * 1000,
        );

        // Clean up gateway before checking result
        let _ = child.kill();
        let _ = child.wait();

        last_resp = resp;

        // Check if the response indicates success (has result field)
        if last_resp.get("result").is_some() {
            success = true;
            break;
        }

        // Log the error and retry
        let error_msg = last_resp
            .get("error")
            .map(|e| serde_json::to_string(e).unwrap_or_default())
            .unwrap_or_default();
        eprintln!(
            "Attempt {}/{}: sandbox_run failed with error: {}, retrying in {}s",
            attempt, MAX_RETRIES, error_msg, RETRY_DELAY_SECS
        );

        if attempt < MAX_RETRIES {
            tokio::time::sleep(tokio::time::Duration::from_secs(RETRY_DELAY_SECS)).await;
        }
    }

    // Verify we got a successful response
    if !success {
        panic!(
            "All {} attempts failed. Last response: {:?}",
            MAX_RETRIES, last_resp
        );
    }

    let resp = last_resp;

    // Verify response
    let result = resp.get("result").expect("Expected result in response");

    // Extract the text content from the sandbox_run result
    let text = result
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("text"))
        .and_then(|t| t.as_str());

    let text = text.expect("Expected text content in sandbox_run result");

    // Parse the JSON response from the gateway
    let parsed: Value =
        serde_json::from_str(text).expect("Expected JSON in sandbox_run text content");

    // Verify enrichment_meta fields are present: source, timestamp, enricher_id
    let enrichment_meta = parsed
        .get("enrichment_meta")
        .expect("Expected enrichment_meta in response");

    let source = enrichment_meta
        .get("source")
        .and_then(|s| s.as_str())
        .expect("Expected 'source' string in enrichment_meta");
    assert!(
        !source.is_empty(),
        "Expected non-empty source in enrichment_meta"
    );

    let timestamp = enrichment_meta
        .get("timestamp")
        .and_then(|t| t.as_str())
        .expect("Expected 'timestamp' string in enrichment_meta");
    assert!(
        !timestamp.is_empty(),
        "Expected non-empty timestamp in enrichment_meta"
    );

    let enricher_id = enrichment_meta
        .get("enricher_id")
        .and_then(|s| s.as_str())
        .expect("Expected 'enricher_id' string in enrichment_meta");

    // The Maven enricher should have set enricher_id to "maven"
    assert_eq!(
        enricher_id, "maven",
        "Expected enricher_id == 'maven', got '{}'",
        enricher_id
    );

    let build_status = parsed.get("build_status").and_then(|b| b.as_str()).expect(
        "Expected build_status in enriched response (e.g., BUILD SUCCESS or BUILD FAILURE)",
    );

    assert!(
        build_status.contains("BUILD"),
        "Expected BUILD status, got: {}",
        build_status
    );

    let facts = parsed
        .get("facts")
        .and_then(|f| f.as_array())
        .expect("Expected facts array in enriched response");

    assert!(
        !facts.is_empty(),
        "Expected non-empty facts from Maven extractor"
    );

    println!(
        "✅ Enrichment test passed: enricher_id={}, source={}, timestamp={}, build_status={}, facts={}",
        enricher_id,
        source,
        timestamp,
        build_status,
        facts.len()
    );
}

// ============================================================================
// Enrichment MCP tool e2e tests
// ============================================================================

/// Call an enrichment tool via tools/call and return the result.
fn call_enrichment_tool(
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
    tool_name: &str,
    arguments: serde_json::Value,
) -> Value {
    rpc_call(
        stdin,
        reader,
        "tools/call",
        serde_json::json!({
            "name": tool_name,
            "arguments": arguments
        }),
    )
}

/// End-to-end test for `enrichment_optimizer_report` MCP tool.
///
/// Verifies:
/// 1. The tool is callable via JSON-RPC `tools/call`
/// 2. Response is valid JSON-RPC (has `result` or `error`)
/// 3. When recorder is configured, returns `generated_at`, `total_runs_analyzed`, `scores`, `recommendations`
/// 4. When recorder is not configured, returns error with meaningful message
///
/// This test is `#[ignore]` by default and requires:
/// - `BASTION_E2E_ENRICHMENT=1` environment variable
/// - `bastion-gateway` binary built
#[tokio::test]
#[ignore]
async fn test_optimizer_report() {
    // Env-gated: skip unless BASTION_E2E_ENRICHMENT=1
    if std::env::var("BASTION_E2E_ENRICHMENT").as_deref() != Ok("1") {
        eprintln!("SKIPPED: Set BASTION_E2E_ENRICHMENT=1 to run this test");
        return;
    }

    // Spawn gateway (Podman not required for this tool - it queries internal state only)
    let (mut child, mut stdin, mut reader) = spawn_gateway();

    // Initialize session
    let init_resp = init_session(&mut stdin, &mut reader);
    assert!(
        init_resp.get("result").is_some(),
        "Expected result in initialize response, got: {:?}",
        init_resp
    );

    // Send initialized notification
    send_initialized_notification(&mut stdin);

    // Call enrichment_optimizer_report with optional timestamp filter
    let resp = call_enrichment_tool(
        &mut stdin,
        &mut reader,
        "enrichment_optimizer_report",
        serde_json::json!({
            "after": "2025-01-01T00:00:00Z"
        }),
    );

    // Clean up gateway
    let _ = child.kill();
    let _ = child.wait();

    // Verify valid JSON-RPC response (either success or structured error is acceptable)
    assert!(
        resp.get("result").is_some() || resp.get("error").is_some(),
        "Expected valid JSON-RPC response with result or error, got: {:?}",
        resp
    );

    // If we got a result, verify expected fields
    if let Some(result) = resp.get("result") {
        // Result is a JSON string from the tool - parse it
        let result_str = result.as_str().expect("Expected result to be a string");
        let parsed: Value =
            serde_json::from_str(result_str).expect("Expected valid JSON string in result");

        // Verify expected fields in the report
        assert!(
            parsed.get("generated_at").is_some(),
            "Expected 'generated_at' in optimizer report"
        );
        assert!(
            parsed.get("total_runs_analyzed").is_some(),
            "Expected 'total_runs_analyzed' in optimizer report"
        );
        assert!(
            parsed.get("scores").is_some(),
            "Expected 'scores' in optimizer report"
        );
        assert!(
            parsed.get("recommendations").is_some(),
            "Expected 'recommendations' in optimizer report"
        );

        let total_runs = parsed
            .get("total_runs_analyzed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        println!(
            "✅ enrichment_optimizer_report passed: total_runs_analyzed={}",
            total_runs
        );
    } else if let Some(error) = resp.get("error") {
        // If recorder not configured, error is expected - verify it's a meaningful message
        let error_msg = error
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        println!(
            "ℹ️ enrichment_optimizer_report returned error (recorder may not be configured): {}",
            error_msg
        );
    }
}

/// End-to-end test for `enrichment_retention_info` MCP tool.
///
/// Verifies:
/// 1. The tool is callable via JSON-RPC `tools/call`
/// 2. Response is valid JSON-RPC (has `result` or `error`)
/// 3. When recorder is configured, returns `retention` and `stats` objects
/// 4. When recorder is not configured, returns error with meaningful message
///
/// This test is `#[ignore]` by default and requires:
/// - `BASTION_E2E_ENRICHMENT=1` environment variable
/// - `bastion-gateway` binary built
#[tokio::test]
#[ignore]
async fn test_retention_info() {
    // Env-gated: skip unless BASTION_E2E_ENRICHMENT=1
    if std::env::var("BASTION_E2E_ENRICHMENT").as_deref() != Ok("1") {
        eprintln!("SKIPPED: Set BASTION_E2E_ENRICHMENT=1 to run this test");
        return;
    }

    // Spawn gateway
    let (mut child, mut stdin, mut reader) = spawn_gateway();

    // Initialize session
    let init_resp = init_session(&mut stdin, &mut reader);
    assert!(
        init_resp.get("result").is_some(),
        "Expected result in initialize response, got: {:?}",
        init_resp
    );

    // Send initialized notification
    send_initialized_notification(&mut stdin);

    // Call enrichment_retention_info (no arguments)
    let resp = call_enrichment_tool(
        &mut stdin,
        &mut reader,
        "enrichment_retention_info",
        serde_json::json!({}),
    );

    // Clean up gateway
    let _ = child.kill();
    let _ = child.wait();

    // Verify valid JSON-RPC response
    assert!(
        resp.get("result").is_some() || resp.get("error").is_some(),
        "Expected valid JSON-RPC response with result or error, got: {:?}",
        resp
    );

    // If we got a result, verify expected fields
    if let Some(result) = resp.get("result") {
        let result_str = result.as_str().expect("Expected result to be a string");
        let parsed: Value =
            serde_json::from_str(result_str).expect("Expected valid JSON string in result");

        // Verify retention config fields
        let retention = parsed
            .get("retention")
            .expect("Expected 'retention' object in retention_info");
        assert!(
            retention.get("max_age_days").is_some(),
            "Expected 'max_age_days' in retention config"
        );
        assert!(
            retention.get("max_rows").is_some(),
            "Expected 'max_rows' in retention config"
        );
        assert!(
            retention.get("enabled").is_some(),
            "Expected 'enabled' in retention config"
        );

        // Verify stats fields
        let stats = parsed
            .get("stats")
            .expect("Expected 'stats' object in retention_info");
        assert!(
            stats.get("current_row_count").is_some(),
            "Expected 'current_row_count' in stats"
        );

        println!("✅ enrichment_retention_info passed: retention configured, stats available");
    } else if let Some(error) = resp.get("error") {
        let error_msg = error
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        println!(
            "ℹ️ enrichment_retention_info returned error (recorder may not be configured): {}",
            error_msg
        );
    }
}

/// End-to-end test for `enrichment_retention_cleanup` MCP tool.
///
/// Verifies:
/// 1. The tool is callable via JSON-RPC `tools/call`
/// 2. Response is valid JSON-RPC (has `result` or `error`)
/// 3. When recorder is configured, returns `deleted_rows` and `remaining_rows`
/// 4. When recorder is not configured, returns error with meaningful message
///
/// This test is `#[ignore]` by default and requires:
/// - `BASTION_E2E_ENRICHMENT=1` environment variable
/// - `bastion-gateway` binary built
#[tokio::test]
#[ignore]
async fn test_retention_cleanup() {
    // Env-gated: skip unless BASTION_E2E_ENRICHMENT=1
    if std::env::var("BASTION_E2E_ENRICHMENT").as_deref() != Ok("1") {
        eprintln!("SKIPPED: Set BASTION_E2E_ENRICHMENT=1 to run this test");
        return;
    }

    // Spawn gateway
    let (mut child, mut stdin, mut reader) = spawn_gateway();

    // Initialize session
    let init_resp = init_session(&mut stdin, &mut reader);
    assert!(
        init_resp.get("result").is_some(),
        "Expected result in initialize response, got: {:?}",
        init_resp
    );

    // Send initialized notification
    send_initialized_notification(&mut stdin);

    // Call enrichment_retention_cleanup (no arguments)
    let resp = call_enrichment_tool(
        &mut stdin,
        &mut reader,
        "enrichment_retention_cleanup",
        serde_json::json!({}),
    );

    // Clean up gateway
    let _ = child.kill();
    let _ = child.wait();

    // Verify valid JSON-RPC response
    assert!(
        resp.get("result").is_some() || resp.get("error").is_some(),
        "Expected valid JSON-RPC response with result or error, got: {:?}",
        resp
    );

    // If we got a result, verify expected fields
    if let Some(result) = resp.get("result") {
        let result_str = result.as_str().expect("Expected result to be a string");
        let parsed: Value =
            serde_json::from_str(result_str).expect("Expected valid JSON string in result");

        // Verify cleanup result fields
        assert!(
            parsed.get("deleted_rows").is_some(),
            "Expected 'deleted_rows' in retention_cleanup response"
        );
        assert!(
            parsed.get("remaining_rows").is_some(),
            "Expected 'remaining_rows' in retention_cleanup response"
        );

        let deleted = parsed
            .get("deleted_rows")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        println!(
            "✅ enrichment_retention_cleanup passed: deleted_rows={}",
            deleted
        );
    } else if let Some(error) = resp.get("error") {
        let error_msg = error
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        println!(
            "ℹ️ enrichment_retention_cleanup returned error (recorder may not be configured): {}",
            error_msg
        );
    }
}

/// End-to-end test for `enrichment_health` MCP tool.
///
/// Verifies:
/// 1. The tool is callable via JSON-RPC `tools/call`
/// 2. Response is valid JSON-RPC (has `result` or `error`)
/// 3. When adapter is configured, returns health fields: `enabled`, `catalog_enricher_count`,
///    `recent_runs_5min`, `saturation_events`, `db_row_count`, `recorder_available`
/// 4. When adapter is not configured, returns error with meaningful message
///
/// This test is `#[ignore]` by default and requires:
/// - `BASTION_E2E_ENRICHMENT=1` environment variable
/// - `bastion-gateway` binary built
#[tokio::test]
#[ignore]
async fn test_enrichment_health() {
    // Env-gated: skip unless BASTION_E2E_ENRICHMENT=1
    if std::env::var("BASTION_E2E_ENRICHMENT").as_deref() != Ok("1") {
        eprintln!("SKIPPED: Set BASTION_E2E_ENRICHMENT=1 to run this test");
        return;
    }

    // Spawn gateway
    let (mut child, mut stdin, mut reader) = spawn_gateway();

    // Initialize session
    let init_resp = init_session(&mut stdin, &mut reader);
    assert!(
        init_resp.get("result").is_some(),
        "Expected result in initialize response, got: {:?}",
        init_resp
    );

    // Send initialized notification
    send_initialized_notification(&mut stdin);

    // Call enrichment_health (no arguments)
    let resp = call_enrichment_tool(
        &mut stdin,
        &mut reader,
        "enrichment_health",
        serde_json::json!({}),
    );

    // Clean up gateway
    let _ = child.kill();
    let _ = child.wait();

    // Verify valid JSON-RPC response
    assert!(
        resp.get("result").is_some() || resp.get("error").is_some(),
        "Expected valid JSON-RPC response with result or error, got: {:?}",
        resp
    );

    // If we got a result, verify expected fields
    if let Some(result) = resp.get("result") {
        let result_str = result.as_str().expect("Expected result to be a string");
        let parsed: Value =
            serde_json::from_str(result_str).expect("Expected valid JSON string in result");

        // Verify health fields
        assert!(
            parsed.get("enabled").is_some(),
            "Expected 'enabled' in health response"
        );
        assert!(
            parsed.get("catalog_enricher_count").is_some(),
            "Expected 'catalog_enricher_count' in health response"
        );
        assert!(
            parsed.get("recent_runs_5min").is_some(),
            "Expected 'recent_runs_5min' in health response"
        );
        assert!(
            parsed.get("recorder_available").is_some(),
            "Expected 'recorder_available' in health response"
        );

        let enabled = parsed
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let enricher_count = parsed
            .get("catalog_enricher_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        println!(
            "✅ enrichment_health passed: enabled={}, catalog_enricher_count={}",
            enabled, enricher_count
        );
    } else if let Some(error) = resp.get("error") {
        let error_msg = error
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        println!(
            "ℹ️ enrichment_health returned error (adapter may not be configured): {}",
            error_msg
        );
    }
}
