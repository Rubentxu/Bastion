//! End-to-end enrichment test for the Bastion MCP Gateway.
//!
//! Tests that Maven commands are enriched with facts, build status, and artifacts
//! when the enrichment engine is enabled. Requires Podman and `BASTION_E2E_ENRICHMENT=1`.
//!
//! Run with: `BASTION_E2E_ENRICHMENT=1 cargo test --test enrichment_sandbox_test -- --ignored`
//!
//! The test is `#[ignore]` by default to avoid requiring Podman in normal CI runs.

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

const E2E_TIMEOUT_SECS: u64 = 120;
const MAX_RETRIES: u32 = 3;
const RETRY_DELAY_SECS: u64 = 5;

/// Spawn the gateway binary and return (child, stdin, stdout_reader).
fn spawn_gateway() -> (std::process::Child, impl Write, impl BufRead) {
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

/// End-to-end test for Maven enrichment in a real sandbox.
///
/// Verifies:
/// 1. A Maven build command is enriched with facts
/// 2. `source_extractor` is present and set to `"maven"`
/// 3. `build_status` is present (e.g., "BUILD SUCCESS" or "BUILD FAILURE")
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

    let (mut child, mut stdin, mut reader) = spawn_gateway();

    // Initialize session
    let resp = init_session(&mut stdin, &mut reader);
    assert!(
        resp.get("result").is_some(),
        "Initialize failed: {:?}",
        resp
    );

    // Send initialized notification
    let notif = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    });
    let notif_str = serde_json::to_string(&notif).unwrap();
    stdin
        .write_all(
            format!(
                "Content-Length: {}\r\n\r\n{}\n",
                notif_str.len(),
                notif_str
            )
            .as_bytes(),
        )
        .unwrap();
    stdin.flush().unwrap();

    // Run a Maven compile command and verify enrichment
    let resp = sandbox_run(
        &mut stdin,
        &mut reader,
        "mvn compile -B -q",
        E2E_TIMEOUT_SECS * 1000,
    );

    // Clean up the gateway
    let _ = child.kill();
    let _ = child.wait();

    // Verify response
    let result = resp
        .get("result")
        .expect("Expected result in response");

    // Extract the text content from the sandbox_run result
    let text = result
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("text"))
        .and_then(|t| t.as_str());

    let text = text.expect("Expected text content in sandbox_run result");

    // Parse the JSON response from the gateway
    let parsed: Value = serde_json::from_str(text)
        .expect("Expected JSON in sandbox_run text content");

    // Verify enrichment fields are present
    let enrichment_meta = parsed
        .get("enrichment_meta")
        .expect("Expected enrichment_meta in response");

    let source_extractor = enrichment_meta
        .get("source_extractor")
        .and_then(|s| s.as_str())
        .expect("Expected source_extractor string in enrichment_meta");

    // The Maven extractor should have set source_extractor to "maven"
    assert_eq!(
        source_extractor, "maven",
        "Expected source_extractor == 'maven', got '{}'",
        source_extractor
    );

    let build_status = parsed
        .get("build_status")
        .and_then(|b| b.as_str())
        .expect("Expected build_status in enriched response (e.g., BUILD SUCCESS or BUILD FAILURE)");

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

    println!("✅ Enrichment test passed: source_extractor={}, build_status={}, facts={}",
        source_extractor, build_status, facts.len());
}
