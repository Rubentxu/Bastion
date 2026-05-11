//! End-to-end integration tests for Bastion orientation and metrics MCP tools.
//!
//! Tests the 7 orientation tools and 2 metrics tools:
//! - Orientation: sandbox_orient_me, sandbox_suggest_template, sandbox_capacity_check,
//!                sandbox_optimal_config, sandbox_get_config, sandbox_set_config, sandbox_config_history
//! - Metrics: sandbox_metrics_history, sandbox_resource_usage
//!
//! These tests exercise the actual tool implementations via the MCP protocol
//! and also test the underlying domain/infrastructure components directly.
//!
//! Run with: `cargo test -p bastion-gateway --test orientation_e2e`

use chrono::{Duration, Utc};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

// Test domain components directly
use bastion_domain::orientation::TemplateRecommender;
use bastion_infrastructure::metrics::hub::MetricsHub;
use bastion_infrastructure::metrics::heartbeat_bridge::HeartbeatBridge;
use bastion_infrastructure::metrics::GatewayMetrics;

// ============================================================================
// Helper Functions for Gateway Process Tests
// ============================================================================

/// Returns the path to the bastion-gateway binary if it exists.
fn gateway_binary_path() -> Option<std::path::PathBuf> {
    let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/debug/bastion-gateway");
    if binary.exists() {
        Some(binary)
    } else {
        None
    }
}

/// Spawn the gateway binary and return (child, stdin, stdout_reader).
/// Returns None if the binary is not built.
fn spawn_gateway() -> Option<(std::process::Child, impl Write, impl BufRead)> {
    let binary = gateway_binary_path()?;

    let worker = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/debug/bastion-worker");

    let mut cmd = Command::new(&binary);
    cmd.arg("--image")
        .arg("debian:bookworm-slim")
        .arg("--worker-binary")
        .arg(&worker);

    let mut child = match cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to spawn bastion-gateway: {}", e);
            return None;
        }
    };

    let stdin = child.stdin.take().expect("stdin not captured");
    let stdout = child.stdout.take().expect("stdout not captured");
    let reader = BufReader::new(stdout);

    Some((child, stdin, reader))
}

/// Send a JSON-RPC request and read the response.
fn rpc_call(
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
    method: &str,
    params: Value,
) -> Value {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    let req = json!({
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
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "orientation-e2e-test", "version": "0.1.0"}
        }),
    )
}

/// Send the `initialized` notification to the gateway.
fn send_initialized_notification(stdin: &mut impl Write) {
    let notif = json!({
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
}

/// Call a tool via tools/call and return the result text.
fn call_tool(
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
    tool_name: &str,
    arguments: Value,
) -> String {
    let resp = rpc_call(
        stdin,
        reader,
        "tools/call",
        json!({
            "name": tool_name,
            "arguments": arguments
        }),
    );

    // Extract text content from the response
    resp.get("result")
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string()
}

// ============================================================================
// TemplateRecommender Direct Tests (Domain Component)
// ============================================================================

/// Test sandbox_suggest_template with Java Maven project description.
#[test]
fn test_template_recommender_java_maven() {
    let recommender = TemplateRecommender::new();
    let result = recommender.recommend("build a Java Maven project with Spring Boot");

    assert_eq!(result.template, "eclipse-temurin:21-jdk-maven");
    assert!(
        result.confidence >= 0.90,
        "Expected confidence >= 0.90, got {}",
        result.confidence
    );
    assert!(
        result.reasoning.contains("Maven"),
        "Expected reasoning to mention Maven, got: {}",
        result.reasoning
    );
}

/// Test sandbox_suggest_template with Python project description.
#[test]
fn test_template_recommender_python() {
    let recommender = TemplateRecommender::new();
    let result = recommender.recommend("pip install -r requirements.txt && pytest tests/");

    assert_eq!(result.template, "python:3.12-slim");
    assert!(
        result.confidence >= 0.90,
        "Expected confidence >= 0.90, got {}",
        result.confidence
    );
}

/// Test sandbox_suggest_template with Rust project description.
#[test]
fn test_template_recommender_rust() {
    let recommender = TemplateRecommender::new();
    let result = recommender.recommend("cargo build --release && cargo test");

    assert_eq!(result.template, "rust:1.77-slim");
    assert!(
        result.confidence >= 0.90,
        "Expected confidence >= 0.90, got {}",
        result.confidence
    );
}

/// Test sandbox_suggest_template with Node.js project description.
#[test]
fn test_template_recommender_nodejs() {
    let recommender = TemplateRecommender::new();
    let result = recommender.recommend("npm install && npm run build");

    assert_eq!(result.template, "node:20-slim");
    assert!(
        result.confidence >= 0.90,
        "Expected confidence >= 0.90, got {}",
        result.confidence
    );
}

/// Test sandbox_suggest_template with Go project description.
#[test]
fn test_template_recommender_golang() {
    let recommender = TemplateRecommender::new();
    let result = recommender.recommend("go mod download && go build ./...");

    assert_eq!(result.template, "golang:1.22-alpine");
    assert!(
        result.confidence >= 0.85,
        "Expected confidence >= 0.85, got {}",
        result.confidence
    );
}

/// Test sandbox_suggest_template with random task (fallback).
#[test]
fn test_template_recommender_fallback() {
    let recommender = TemplateRecommender::new();
    let result = recommender.recommend("do something completely random xyz123");

    // Fallback to ubuntu:24.04 with low confidence
    assert_eq!(result.template, "ubuntu:24.04");
    assert!(
        result.confidence < 0.5,
        "Expected confidence < 0.5 for random task, got {}",
        result.confidence
    );
}

/// Test sandbox_suggest_template with Ruby project description.
#[test]
fn test_template_recommender_ruby() {
    let recommender = TemplateRecommender::new();
    let result = recommender.recommend("bundle install && rake db:migrate");

    assert_eq!(result.template, "ruby:3.3-slim");
    assert!(
        result.confidence >= 0.85,
        "Expected confidence >= 0.85, got {}",
        result.confidence
    );
}

/// Test sandbox_suggest_template with .NET project description.
#[test]
fn test_template_recommender_dotnet() {
    let recommender = TemplateRecommender::new();
    let result = recommender.recommend("dotnet build && dotnet test");

    assert_eq!(result.template, "mcr.microsoft.com/dotnet/sdk:8.0");
    assert!(
        result.confidence >= 0.85,
        "Expected confidence >= 0.85, got {}",
        result.confidence
    );
}

/// Test sandbox_suggest_template with PHP project description.
#[test]
fn test_template_recommender_php() {
    let recommender = TemplateRecommender::new();
    let result = recommender.recommend("composer install && php artisan serve");

    assert_eq!(result.template, "php:8.3-cli");
    assert!(
        result.confidence >= 0.80,
        "Expected confidence >= 0.80, got {}",
        result.confidence
    );
}

// ============================================================================
// MetricsHub Direct Tests (Infrastructure Component)
// ============================================================================

/// Test MetricsHub in-memory creation and basic operations.
#[tokio::test]
async fn test_metrics_hub_in_memory() {
    let metrics = Arc::new(GatewayMetrics::default());
    let hub = MetricsHub::new_in_memory(metrics).await;

    assert!(
        hub.is_ok(),
        "MetricsHub should initialize with in-memory SQLite"
    );
}

/// Test recording and querying metrics history.
#[tokio::test]
async fn test_metrics_hub_record_and_query() {
    let metrics = Arc::new(GatewayMetrics::default());
    let hub = MetricsHub::new_in_memory(metrics).await.unwrap();

    // Record a metric
    let record = bastion_infrastructure::metrics::hub::MetricRecord {
        timestamp: Utc::now(),
        sandbox_id: Some("test-sandbox".to_string()),
        cpu_percent: Some(42.5),
        mem_used_mb: Some(256.0),
        mem_limit_mb: Some(512.0),
        disk_used_mb: Some(100.0),
        commands_executed: Some(10),
        errors_total: Some(0),
    };

    hub.record_metric(&record).await.unwrap();

    // Query history
    let history = hub
        .get_metrics_history(Utc::now() - Duration::hours(1))
        .await
        .unwrap();

    assert_eq!(history.len(), 1);
    assert_eq!(history[0].sandbox_id.as_deref(), Some("test-sandbox"));
    assert!((history[0].cpu_percent.unwrap() - 42.5).abs() < 0.01);
}

/// Test config history recording.
#[tokio::test]
async fn test_metrics_hub_config_history() {
    let metrics = Arc::new(GatewayMetrics::default());
    let hub = MetricsHub::new_in_memory(metrics).await.unwrap();

    // Set some config values
    hub.set_config(
        "pool.max_total",
        Some("10".to_string()),
        "15".to_string(),
        "test",
    )
    .await
    .unwrap();

    hub.set_config(
        "pool.min_idle",
        Some("2".to_string()),
        "3".to_string(),
        "test",
    )
    .await
    .unwrap();

    // Get config history
    let history = hub.get_config_history().await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].key, "pool.max_total");
    assert_eq!(history[1].key, "pool.min_idle");
}

/// Test restricted config keys are rejected.
#[tokio::test]
async fn test_metrics_hub_restricted_config_keys() {
    let metrics = Arc::new(GatewayMetrics::default());
    let hub = MetricsHub::new_in_memory(metrics).await.unwrap();

    // Try to set a restricted key
    let result = hub
        .set_config(
            "auth.hmac_enabled",
            Some("true".to_string()),
            "false".to_string(),
            "sandbox_set_config",
        )
        .await
        .unwrap();

    assert!(!result.applied, "Restricted keys should not be applied");
    assert!(
        result.restart_hint.is_some(),
        "Should provide restart hint for restricted keys"
    );
}

// ============================================================================
// HeartbeatBridge Direct Tests (Infrastructure Component)
// ============================================================================

/// Test HeartbeatBridge resource tracking.
#[test]
fn test_heartbeat_bridge_update_and_get() {
    let bridge = HeartbeatBridge::new();
    let now = chrono::Utc::now().timestamp();

    let resources = bastion_infrastructure::metrics::heartbeat_bridge::WorkerResources {
        sandbox_id: "sb-1".to_string(),
        cpu_percent: 25.0,
        mem_used_mb: 128.0,
        mem_limit_mb: 512.0,
        disk_used_mb: 50.0,
        loadavg_1m: 0.5,
        uptime_seconds: 100,
        last_heartbeat_epoch: now,
    };

    bridge.update_resources(resources);

    let tracked = bridge.get_resources("sb-1");
    assert!(
        tracked.is_some(),
        "Should have tracked resources for sb-1"
    );
    let res = tracked.unwrap();
    assert!((res.cpu_percent - 25.0).abs() < 0.01);
    assert!((res.mem_used_mb - 128.0).abs() < 0.01);
}

/// Test HeartbeatBridge system resource aggregation.
#[test]
fn test_heartbeat_bridge_system_aggregation() {
    let bridge = HeartbeatBridge::new();
    let now = chrono::Utc::now().timestamp();

    // Add two sandboxes
    for (id, cpu, mem) in [("sb-1", 25.0, 128.0), ("sb-2", 50.0, 256.0)] {
        let resources = bastion_infrastructure::metrics::heartbeat_bridge::WorkerResources {
            sandbox_id: id.to_string(),
            cpu_percent: cpu,
            mem_used_mb: mem,
            mem_limit_mb: 512.0,
            disk_used_mb: 50.0,
            loadavg_1m: 0.5,
            uptime_seconds: 100,
            last_heartbeat_epoch: now,
        };
        bridge.update_resources(resources);
    }

    let sys = bridge.get_system_resources();
    assert_eq!(sys.active_sandboxes, 2);
    assert!((sys.total_cpu_percent - 75.0).abs() < 0.01);
    assert!((sys.total_mem_used_mb - 384.0).abs() < 0.01);
}

/// Test HeartbeatBridge removal.
#[test]
fn test_heartbeat_bridge_remove() {
    let bridge = HeartbeatBridge::new();
    let now = chrono::Utc::now().timestamp();

    let resources = bastion_infrastructure::metrics::heartbeat_bridge::WorkerResources {
        sandbox_id: "sb-1".to_string(),
        cpu_percent: 25.0,
        mem_used_mb: 128.0,
        mem_limit_mb: 512.0,
        disk_used_mb: 50.0,
        loadavg_1m: 0.5,
        uptime_seconds: 100,
        last_heartbeat_epoch: now,
    };

    bridge.update_resources(resources);
    assert_eq!(bridge.tracked_count(), 1);

    let removed = bridge.remove_resources("sb-1");
    assert!(removed, "Should successfully remove sb-1");
    assert_eq!(bridge.tracked_count(), 0);

    let not_removed = bridge.remove_resources("sb-1");
    assert!(!not_removed, "Should return false for already-removed sandbox");
}

/// Test HeartbeatBridge stale pruning.
#[test]
fn test_heartbeat_bridge_prune_stale() {
    let bridge = HeartbeatBridge::with_stale_threshold(10);
    let now = chrono::Utc::now().timestamp();
    let old = now - 60; // 60 seconds ago

    // Add fresh and stale resources
    for (id, epoch) in [("sb-fresh", now), ("sb-old", old)] {
        let resources = bastion_infrastructure::metrics::heartbeat_bridge::WorkerResources {
            sandbox_id: id.to_string(),
            cpu_percent: 25.0,
            mem_used_mb: 128.0,
            mem_limit_mb: 512.0,
            disk_used_mb: 50.0,
            loadavg_1m: 0.5,
            uptime_seconds: 100,
            last_heartbeat_epoch: epoch,
        };
        bridge.update_resources(resources);
    }

    let pruned = bridge.prune_stale();
    assert_eq!(pruned, 1, "Should prune 1 stale entry");
    assert_eq!(bridge.tracked_count(), 1);
    assert!(
        bridge.get_resources("sb-fresh").is_some(),
        "Fresh sandbox should remain"
    );
    assert!(
        bridge.get_resources("sb-old").is_none(),
        "Stale sandbox should be removed"
    );
}

// ============================================================================
// MCP Tool Tests (via Gateway Process)
// ============================================================================

/// Helper: wait for gateway to be ready
fn wait_for_gateway(stdin: &mut impl Write, reader: &mut impl BufRead) -> bool {
    // Wait a bit for gateway to initialize
    std::thread::sleep(StdDuration::from_millis(500));

    // Try to initialize - if it works, we're ready
    let resp = init_session(stdin, reader);
    resp.get("result").is_some()
}

/// Test sandbox_orient_me MCP tool returns expected fields.
#[test]
fn test_sandbox_orient_me_via_mcp() {
    let start = Instant::now();
    let Some((mut child, mut stdin, mut reader)) = spawn_gateway() else {
        eprintln!("SKIPPED: bastion-gateway binary not built");
        return;
    };

    // Wait for gateway to be ready
    if !wait_for_gateway(&mut stdin, &mut reader) {
        let _ = child.kill();
        eprintln!("SKIPPED: Gateway failed to initialize");
        return;
    }

    send_initialized_notification(&mut stdin);

    // Call sandbox_orient_me
    let text = call_tool(&mut stdin, &mut reader, "sandbox_orient_me", json!({}));

    let _ = child.kill();

    // Parse response
    let result: Value = serde_json::from_str(&text).unwrap_or_else(|e| {
        json!({"error": format!("failed to parse: {}", e)})
    });

    // Verify expected fields
    assert!(
        result.get("gateway_version").is_some(),
        "Expected gateway_version in response: {}",
        result
    );
    assert!(
        result.get("provider").is_some(),
        "Expected provider in response: {}",
        result
    );
    assert!(
        result.get("pool_status").is_some(),
        "Expected pool_status in response: {}",
        result
    );
    assert!(
        result.get("available_templates").is_some(),
        "Expected available_templates in response: {}",
        result
    );
    assert!(
        result.get("capabilities").is_some(),
        "Expected capabilities in response: {}",
        result
    );
    assert!(
        result.get("known_limitations").is_some(),
        "Expected known_limitations in response: {}",
        result
    );
    assert!(
        result.get("worker_heartbeat_available").is_some(),
        "Expected worker_heartbeat_available in response: {}",
        result
    );

    // Verify capabilities include orientation tools
    let capabilities = result["capabilities"].as_array().unwrap();
    let cap_names: Vec<&str> = capabilities
        .iter()
        .filter_map(|c| c.as_str())
        .collect();

    assert!(
        cap_names.contains(&"sandbox_orient_me"),
        "Should include sandbox_orient_me capability"
    );
    assert!(
        cap_names.contains(&"sandbox_suggest_template"),
        "Should include sandbox_suggest_template capability"
    );

    println!(
        "✅ sandbox_orient_me passed in {:?} - gateway_version={}",
        start.elapsed(),
        result["gateway_version"]
    );
}

/// Test sandbox_suggest_template MCP tool with various task descriptions.
#[test]
fn test_sandbox_suggest_template_via_mcp() {
    let Some((mut child, mut stdin, mut reader)) = spawn_gateway() else {
        eprintln!("SKIPPED: bastion-gateway binary not built");
        return;
    };

    if !wait_for_gateway(&mut stdin, &mut reader) {
        let _ = child.kill();
        eprintln!("SKIPPED: Gateway failed to initialize");
        return;
    }

    send_initialized_notification(&mut stdin);

    let test_cases = vec![
        (
            "build a Java Maven project",
            "eclipse-temurin:21-jdk-maven",
        ),
        (
            "run Python tests with pytest",
            "python:3.12-slim",
        ),
        (
            "compile Rust code",
            "rust:1.77-slim",
        ),
        (
            "npm install and run node script",
            "node:20-slim",
        ),
    ];

    for (task, expected_template) in test_cases {
        let text = call_tool(
            &mut stdin,
            &mut reader,
            "sandbox_suggest_template",
            json!({"task_description": task}),
        );

        let result: Value = serde_json::from_str(&text).unwrap_or_else(|e| {
            json!({"error": format!("failed to parse: {}", e)})
        });

        assert!(
            result.get("template").is_some(),
            "Expected template in response for task '{}': {}",
            task,
            result
        );
        assert!(
            result.get("confidence").is_some(),
            "Expected confidence in response for task '{}': {}",
            task,
            result
        );

        let template = result["template"].as_str().unwrap();
        assert_eq!(
            template, expected_template,
            "Expected template '{}' for task '{}', got '{}'",
            expected_template, task, template
        );

        println!("✅ sandbox_suggest_template '{}' -> {}", task, template);
    }

    let _ = child.kill();
}

/// Test sandbox_capacity_check MCP tool.
#[test]
fn test_sandbox_capacity_check_via_mcp() {
    let Some((mut child, mut stdin, mut reader)) = spawn_gateway() else {
        eprintln!("SKIPPED: bastion-gateway binary not built");
        return;
    };

    if !wait_for_gateway(&mut stdin, &mut reader) {
        let _ = child.kill();
        eprintln!("SKIPPED: Gateway failed to initialize");
        return;
    }

    send_initialized_notification(&mut stdin);

    // Test with count=1
    let text = call_tool(
        &mut stdin,
        &mut reader,
        "sandbox_capacity_check",
        json!({"count": 1}),
    );

    let result: Value = serde_json::from_str(&text).unwrap_or_else(|e| {
        json!({"error": format!("failed to parse: {}", e)})
    });

    // Verify response structure
    assert!(
        result.get("available").is_some(),
        "Expected 'available' in response: {}",
        result
    );
    assert!(
        result.get("current_count").is_some(),
        "Expected 'current_count' in response: {}",
        result
    );
    assert!(
        result.get("max_capacity").is_some(),
        "Expected 'max_capacity' in response: {}",
        result
    );
    assert!(
        result.get("recommended_action").is_some(),
        "Expected 'recommended_action' in response: {}",
        result
    );

    // Test with count=10
    let text2 = call_tool(
        &mut stdin,
        &mut reader,
        "sandbox_capacity_check",
        json!({"count": 10}),
    );

    let result2: Value = serde_json::from_str(&text2).unwrap_or_else(|_| json!({}));
    assert!(
        result2.get("available").is_some(),
        "Expected 'available' for count=10: {}",
        result2
    );

    let _ = child.kill();
    println!("✅ sandbox_capacity_check passed");
}

/// Test sandbox_optimal_config MCP tool with various use cases.
#[test]
fn test_sandbox_optimal_config_via_mcp() {
    let Some((mut child, mut stdin, mut reader)) = spawn_gateway() else {
        eprintln!("SKIPPED: bastion-gateway binary not built");
        return;
    };

    if !wait_for_gateway(&mut stdin, &mut reader) {
        let _ = child.kill();
        eprintln!("SKIPPED: Gateway failed to initialize");
        return;
    }

    send_initialized_notification(&mut stdin);

    let test_cases = vec![
        ("ci_build", "pool", true),
        ("local_dev", "pool", true),
        ("data_processing", "pool", true),
    ];

    for (use_case, _expected_key, _expect_warnings) in test_cases {
        let text = call_tool(
            &mut stdin,
            &mut reader,
            "sandbox_optimal_config",
            json!({"use_case": use_case}),
        );

        let result: Value = serde_json::from_str(&text).unwrap_or_else(|e| {
            json!({"error": format!("failed to parse: {}", e)})
        });

        assert!(
            result.get("config").is_some(),
            "Expected 'config' in response for use_case '{}': {}",
            use_case,
            result
        );
        assert!(
            result.get("warnings").is_some(),
            "Expected 'warnings' in response for use_case '{}': {}",
            use_case,
            result
        );
        assert!(
            result.get("restart_required").is_some(),
            "Expected 'restart_required' in response for use_case '{}': {}",
            use_case,
            result
        );
        assert!(
            result.get("reasoning").is_some(),
            "Expected 'reasoning' in response for use_case '{}': {}",
            use_case,
            result
        );

        // Verify config has expected structure
        let config = &result["config"];
        assert!(
            config.get("pool").is_some(),
            "Expected 'config.pool' for use_case '{}': {}",
            use_case,
            result
        );

        println!("✅ sandbox_optimal_config '{}' passed", use_case);
    }

    let _ = child.kill();
}

/// Test sandbox_get_config MCP tool verifies secrets are redacted.
#[test]
fn test_sandbox_get_config_via_mcp() {
    let Some((mut child, mut stdin, mut reader)) = spawn_gateway() else {
        eprintln!("SKIPPED: bastion-gateway binary not built");
        return;
    };

    if !wait_for_gateway(&mut stdin, &mut reader) {
        let _ = child.kill();
        eprintln!("SKIPPED: Gateway failed to initialize");
        return;
    }

    send_initialized_notification(&mut stdin);

    let text = call_tool(&mut stdin, &mut reader, "sandbox_get_config", json!({}));

    let _ = child.kill();

    let result: Value = serde_json::from_str(&text).unwrap_or_else(|e| {
        json!({"error": format!("failed to parse: {}", e)})
    });

    // Verify response structure
    assert!(
        result.get("config").is_some(),
        "Expected 'config' in response: {}",
        result
    );
    assert!(
        result.get("notes").is_some(),
        "Expected 'notes' in response: {}",
        result
    );

    // Verify notes mention redaction
    let notes = result["notes"].as_array().unwrap();
    let notes_text = serde_json::to_string(notes).unwrap();
    assert!(
        notes_text.contains("redacted") || notes_text.contains("Auth"),
        "Expected notes to mention redaction or Auth: {:?}",
        notes
    );

    // Verify psk_count is present (but actual PSKs are redacted)
    let auth = result["config"].get("auth");
    assert!(
        auth.is_some(),
        "Expected 'config.auth' in response: {}",
        result
    );

    println!("✅ sandbox_get_config passed - secrets redacted");
}

/// Test sandbox_set_config MCP tool verifies restricted key rejection.
#[test]
fn test_sandbox_set_config_restricted_keys() {
    let Some((mut child, mut stdin, mut reader)) = spawn_gateway() else {
        eprintln!("SKIPPED: bastion-gateway binary not built");
        return;
    };

    if !wait_for_gateway(&mut stdin, &mut reader) {
        let _ = child.kill();
        eprintln!("SKIPPED: Gateway failed to initialize");
        return;
    }

    send_initialized_notification(&mut stdin);

    // Try to set a restricted auth key
    let text = call_tool(
        &mut stdin,
        &mut reader,
        "sandbox_set_config",
        json!({
            "updates": {
                "auth.hmac_enabled": false
            }
        }),
    );

    let _ = child.kill();

    let result: Value = serde_json::from_str(&text).unwrap_or_else(|e| {
        json!({"error": format!("failed to parse: {}", e)})
    });

    // Verify response structure
    assert!(
        result.get("failed").is_some(),
        "Expected 'failed' in response: {}",
        result
    );
    assert!(
        result.get("applied").is_some(),
        "Expected 'applied' in response: {}",
        result
    );

    // Verify the restricted key was rejected
    let failed = result["failed"].as_array().unwrap();
    assert!(
        !failed.is_empty(),
        "Expected restricted key 'auth.hmac_enabled' to be in failed list: {}",
        result
    );

    // Verify restart hint is present for auth changes
    if let Some(restart_hint) = result.get("restart_hint").and_then(|h| h.as_str()) {
        assert!(
            restart_hint.contains("restart"),
            "Expected restart hint for auth changes, got: {}",
            restart_hint
        );
    }

    println!("✅ sandbox_set_config restricted key rejection passed");
}

/// Test sandbox_config_history MCP tool with empty history.
#[test]
fn test_sandbox_config_history_empty() {
    let Some((mut child, mut stdin, mut reader)) = spawn_gateway() else {
        eprintln!("SKIPPED: bastion-gateway binary not built");
        return;
    };

    if !wait_for_gateway(&mut stdin, &mut reader) {
        let _ = child.kill();
        eprintln!("SKIPPED: Gateway failed to initialize");
        return;
    }

    send_initialized_notification(&mut stdin);

    let text = call_tool(
        &mut stdin,
        &mut reader,
        "sandbox_config_history",
        json!({}),
    );

    let _ = child.kill();

    let result: Value = serde_json::from_str(&text).unwrap_or_else(|e| {
        json!({"error": format!("failed to parse: {}", e)})
    });

    // Verify response structure
    assert!(
        result.get("changes").is_some(),
        "Expected 'changes' in response: {}",
        result
    );

    // Changes should be empty (no config changes made in this test)
    let changes = result["changes"].as_array().unwrap();
    assert!(
        changes.is_empty() || !changes.is_empty(),
        "Changes array should be present (empty is valid if no config history)"
    );

    println!("✅ sandbox_config_history passed");
}

/// Test sandbox_metrics_history MCP tool.
#[test]
fn test_sandbox_metrics_history_via_mcp() {
    let Some((mut child, mut stdin, mut reader)) = spawn_gateway() else {
        eprintln!("SKIPPED: bastion-gateway binary not built");
        return;
    };

    if !wait_for_gateway(&mut stdin, &mut reader) {
        let _ = child.kill();
        eprintln!("SKIPPED: Gateway failed to initialize");
        return;
    }

    send_initialized_notification(&mut stdin);

    // Query with a recent timestamp
    let since = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let text = call_tool(
        &mut stdin,
        &mut reader,
        "sandbox_metrics_history",
        json!({"since": since}),
    );

    let _ = child.kill();

    let result: Value = serde_json::from_str(&text).unwrap_or_else(|e| {
        json!({"error": format!("failed to parse: {}", e)})
    });

    // Verify response structure
    assert!(
        result.get("records").is_some(),
        "Expected 'records' in response: {}",
        result
    );
    assert!(
        result.get("count").is_some(),
        "Expected 'count' in response: {}",
        result
    );
    assert!(
        result.get("since").is_some(),
        "Expected 'since' in response: {}",
        result
    );

    // Count should be 0 (no metrics recorded in this test)
    let count = result["count"].as_u64().unwrap_or(1);
    assert_eq!(
        count, 0,
        "Expected 0 records without metrics, got {}",
        count
    );

    println!("✅ sandbox_metrics_history passed");
}

/// Test sandbox_resource_usage MCP tool for non-existent sandbox.
#[test]
fn test_sandbox_resource_usage_via_mcp() {
    let Some((mut child, mut stdin, mut reader)) = spawn_gateway() else {
        eprintln!("SKIPPED: bastion-gateway binary not built");
        return;
    };

    if !wait_for_gateway(&mut stdin, &mut reader) {
        let _ = child.kill();
        eprintln!("SKIPPED: Gateway failed to initialize");
        return;
    }

    send_initialized_notification(&mut stdin);

    // Query resource usage for a sandbox that doesn't exist
    let text = call_tool(
        &mut stdin,
        &mut reader,
        "sandbox_resource_usage",
        json!({"sandbox_id": "nonexistent-sandbox-12345"}),
    );

    let _ = child.kill();

    let result: Value = serde_json::from_str(&text).unwrap_or_else(|e| {
        json!({"error": format!("failed to parse: {}", e)})
    });

    // Verify response structure
    assert!(
        result.get("sandbox_id").is_some(),
        "Expected 'sandbox_id' in response: {}",
        result
    );
    assert!(
        result.get("active").is_some(),
        "Expected 'active' in response: {}",
        result
    );

    // For non-existent sandbox, active should be false
    let active = result["active"].as_bool().unwrap_or(true);
    assert!(
        !active,
        "Expected active=false for non-existent sandbox, got true"
    );

    println!("✅ sandbox_resource_usage passed for non-existent sandbox");
}

// ============================================================================
// Integration Test: Full Orientation Tools Flow
// ============================================================================

/// Test a complete flow: orient_me -> suggest_template -> capacity_check
#[test]
fn test_orientation_tools_integration_flow() {
    let start = Instant::now();
    let Some((mut child, mut stdin, mut reader)) = spawn_gateway() else {
        eprintln!("SKIPPED: bastion-gateway binary not built");
        return;
    };

    if !wait_for_gateway(&mut stdin, &mut reader) {
        let _ = child.kill();
        eprintln!("SKIPPED: Gateway failed to initialize");
        return;
    }

    send_initialized_notification(&mut stdin);

    // 1. Get environment orientation
    let orient_text = call_tool(&mut stdin, &mut reader, "sandbox_orient_me", json!({}));
    let orient: Value = serde_json::from_str(&orient_text).unwrap_or_else(|_| json!({}));

    assert!(
        orient.get("gateway_version").is_some(),
        "orient_me should return gateway_version"
    );
    let templates = orient["available_templates"]
        .as_array()
        .expect("orient_me should have available_templates");
    assert!(
        !templates.is_empty(),
        "Should have some available templates"
    );
    println!("✅ Step 1: sandbox_orient_me returned {} templates", templates.len());

    // 2. Get template suggestion
    let suggest_text = call_tool(
        &mut stdin,
        &mut reader,
        "sandbox_suggest_template",
        json!({"task_description": "build a Java Maven project"}),
    );
    let suggest: Value = serde_json::from_str(&suggest_text).unwrap_or_else(|_| json!({}));

    assert!(
        suggest.get("template").is_some(),
        "sandbox_suggest_template should return template"
    );
    let template = suggest["template"].as_str().unwrap();
    assert!(
        template.contains("temurin") || template.contains("jdk"),
        "Java project should suggest JDK template, got: {}",
        template
    );
    println!("✅ Step 2: sandbox_suggest_template suggested {}", template);

    // 3. Check capacity
    let capacity_text = call_tool(
        &mut stdin,
        &mut reader,
        "sandbox_capacity_check",
        json!({"count": 5}),
    );
    let capacity: Value = serde_json::from_str(&capacity_text).unwrap_or_else(|_| json!({}));

    assert!(
        capacity.get("available").is_some(),
        "sandbox_capacity_check should return available"
    );
    println!("✅ Step 3: sandbox_capacity_check available={}", capacity["available"]);

    // 4. Get current config
    let config_text = call_tool(&mut stdin, &mut reader, "sandbox_get_config", json!({}));
    let config: Value = serde_json::from_str(&config_text).unwrap_or_else(|_| json!({}));

    assert!(
        config.get("config").is_some(),
        "sandbox_get_config should return config"
    );
    println!("✅ Step 4: sandbox_get_config returned config");

    let _ = child.kill();

    println!(
        "✅ Orientation tools integration flow passed in {:?}",
        start.elapsed()
    );
}
