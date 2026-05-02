# Test Coverage Planning: Bastion Self-Testing Harness

**Date**: 2026-05-02
**System**: Bastion - Sandbox Gateway & Infrastructure
**Testing Approach**: Podman-backed self-testing with source code mounted at `/workspace/code`

---

## 1. Feature Inventory

### 1.1 Gateway MCP Tools (server.rs)

| Tool | Function | Priority |
|------|----------|----------|
| `sandbox_create` | Creates sandbox via pool checkout or direct creation | CRITICAL |
| `sandbox_run` | Execute command, return exit_code+stdout+stderr | CRITICAL |
| `sandbox_run_stream` | Streaming command with progress notifications | HIGH |
| `sandbox_write` | Write file to sandbox | HIGH |
| `sandbox_read` | Read file from sandbox | HIGH |
| `sandbox_list_files` | List directory contents | HIGH |
| `sandbox_terminate` | Terminate sandbox (pool checkin or direct terminate) | CRITICAL |
| `sandbox_info` | Get sandbox metadata/status | MEDIUM |
| `sandbox_list` | List all active sandboxes | MEDIUM |
| `sandbox_pool_stats` | Pool statistics (enabled, idle, active, templates) | HIGH |
| `sandbox_health` | Health check (provider + pool status) | HIGH |
| `sandbox_metrics` | Prometheus-format metrics export | LOW |

### 1.2 Providers

| Provider | Create | Terminate | Run Cmd | Stream | File Ops | Snapshots |
|----------|--------|-----------|---------|--------|----------|-----------|
| **Podman** | ✅ | ✅ | ✅ | ✅ (exec) | ✅ (exec) | ❌ |
| **Firecracker** | ✅ | ✅ | ✅ (serial) | ❌ (requires registry) | ✅ (serial) | ✅ |
| **gVisor** | ✅ | ✅ | ✅ (runsc exec) | ✅ (fallback) | ✅ (runsc exec) | ❌ |

**Provider Capabilities**:
- Podman: `requires_kvm=false`, `avg_startup_ms=1500`
- Firecracker: `requires_kvm=true`, `supports_snapshots=true`, `avg_startup_ms=300`
- gVisor: `requires_kvm=false`, `avg_startup_ms=2000`

### 1.3 Pool Manager (pool/manager.rs)

| Feature | Description |
|---------|-------------|
| Checkout | Get sandbox from pool (or create) |
| Checkin | Return sandbox to pool |
| Recovery | Reintegrate orphaned sandboxes on restart |
| Eviction | Remove idle sandboxes after timeout |
| Refill | Create new sandboxes to maintain min_idle |
| Stats | Per-template and aggregate statistics |
| Template registration | Register templates for pool management |

### 1.4 Registry Worker (bastion-worker)

| Feature | Description |
|---------|-------------|
| Registration | Connect + challenge-response auth |
| Command execution | Run command via shell |
| File operations | Read/write/list files |
| Streaming | Chunked output streaming |
| Cancellation | Cancel running command |
| Health reporting | Memory, CPU, disk, uptime |
| Graceful shutdown | Drain pending commands |
| Reconnection | Exponential backoff retry |

---

## 2. Existing Test Coverage

### 2.1 Self-Testing Harness (self_test.rs)

| Test | Description | CI |
|------|-------------|-----|
| `self_test_cargo_check` | `cargo check -p bastion-domain` in Podman | ✅ |
| `self_test_cargo_test_unit` | `cargo test --lib` in Podman | ✅ |

### 2.2 E2E Gateway Tests (e2e_test.rs)

| Test | Coverage | CI |
|------|----------|-----|
| `test_gateway_e2e_lifecycle` | Initialize → Create → Run → Health → Terminate | ✅ |
| `test_gateway_health_only` | Health endpoint | ✅ |
| `test_gateway_pool_lifecycle` | Pool checkout/checkin cycle | ✅ |
| `test_gateway_list_and_info` | sandbox_list + sandbox_info | ✅ |
| `test_gateway_pool_recovery` | Gateway restart + recovery | ✅ |
| `test_gateway_error_handling` | Error cases (non-existent sandbox) | ✅ |

### 2.3 Provider Lifecycle Tests

**Podman (podman_lifecycle.rs)**:
| Test | CI |
|------|-----|
| `test_podman_ping` | ✅ |
| `test_sandbox_create_and_terminate` | ✅ |
| `test_sandbox_run_command` | ✅ |
| `test_sandbox_write_and_read_file` | ✅ |
| `test_sandbox_list_files` | ✅ |

**Firecracker (firecracker_lifecycle.rs)**:
| Test | CI | Notes |
|------|-----|-------|
| `test_firecracker_create_and_terminate` | ❌ | Requires KVM |
| `test_firecracker_run_command` | ❌ | Requires KVM |
| `test_firecracker_write_and_read_file` | ❌ | Requires KVM |
| `test_firecracker_list_files` | ❌ | Requires KVM |
| `test_firecracker_multiple_vms` | ❌ | Requires KVM |

**gVisor (gvisor_lifecycle.rs)**:
| Test | CI | Notes |
|------|-----|-------|
| `test_gvisor_create_and_terminate` | ❌ | Requires runsc |
| `test_gvisor_run_command` | ❌ | Requires runsc |
| `test_gvisor_write_and_read_file` | ❌ | Requires runsc |
| `test_gvisor_list_files` | ❌ | Requires runsc |
| `test_gvisor_command_with_timeout` | ❌ | Requires runsc |
| `test_gvisor_multiple_vms` | ❌ | Requires runsc |
| `test_gvisor_streaming_command` | ❌ | Requires runsc |

**E2E Worker v2 (e2e_worker_v2.rs)**:
| Test | CI |
|------|-----|
| `test_e2e_podman_create_and_run_command` | ✅ |
| `test_e2e_podman_environment_variables` | ✅ |
| `test_e2e_podman_complex_command` | ✅ |

### 2.4 Unit Tests

| Component | Module | Test Count |
|-----------|--------|------------|
| Metrics | `metrics/mod.rs` | 5 tests |
| Pool Manager | `pool/manager.rs` | 4 tests |
| Firecracker path validation | `provider/firecracker.rs` | 1 test |
| gVisor path validation | `provider/gvisor.rs` | 2 tests |

---

## 3. Coverage Gap Analysis

### 3.1 CRITICAL Gaps (No Coverage)

| Feature | Risk | Reason |
|---------|------|--------|
| `sandbox_run_stream` streaming progress | HIGH | Never tested with actual streaming |
| Podman registry routing fallback | HIGH | No test for `CommandRouter` path |
| Firecracker `run_command_stream` | HIGH | Returns `UnsupportedOperation` without registry |
| Worker `read_file` multi-chunk | MEDIUM | Single chunk only in e2e |
| Worker `write_file` multi-chunk | MEDIUM | Never tested |
| Worker `handle_list` | MEDIUM | Not tested |
| Worker `Cancel` command | MEDIUM | Not tested |
| Worker graceful shutdown | MEDIUM | Not tested |
| Pool recovery skips unregistered templates | LOW | Has unit test but no integration |
| Pool `evict_idle` | LOW | Never tested |
| Path traversal prevention | HIGH | Security-critical, not tested |
| Rate limiting | MEDIUM | TokenBucket untested |
| Circuit breaker | MEDIUM | Untested |

### 3.2 MEDIUM Gaps (Partial Coverage)

| Feature | Existing | Missing |
|---------|----------|---------|
| `sandbox_run` exit codes | ✅ Basic | Non-zero exit codes, signals |
| `sandbox_write` encoding | ✅ Basic | Binary content, large files |
| `sandbox_read` large files | ✅ Basic | Chunked reading |
| `sandbox_list_files` parsing | ✅ Basic | Empty dirs, special chars in names |
| Pool `register_template` | Unit | No integration test |
| Pool `stats` aggregation | Unit | No E2E verification |
| Provider `capabilities()` | None | Not tested |

### 3.3 Firecracker/gVisor Specific Gaps

| Feature | Gap |
|---------|-----|
| Firecracker TAP device creation/cleanup | Not tested |
| Firecracker rootfs mount for worker injection | Not tested |
| Firecracker snapshot create/restore | Partially tested (snapshots not tested) |
| gVisor OCI bundle creation | Not tested |
| gVisor `container_is_running` polling | Not tested |

---

## 4. Eventualities to Handle

### 4.1 Provider Failures

| Scenario | Expected Behavior | Test |
|----------|-------------------|------|
| Podman socket not available | Skip tests gracefully | ✅ |
| Container creation timeout | Return error | ❌ |
| Container already exists | Return `AlreadyExists` | ❌ |
| Worker binary missing in container | Create fails | ❌ |
| Image pull failure | Return error | ❌ |

### 4.2 Sandbox Failures

| Scenario | Expected Behavior | Test |
|----------|-------------------|------|
| Sandbox doesn't exist | Return `NotFound` | ✅ Partial |
| Sandbox not running | Return error on run | ❌ |
| Command timeout | Return `timed_out=true` | ❌ |
| Command killed (signal) | Return non-zero exit | ❌ |
| Read non-existent file | Return error | ❌ |
| Write to read-only path | Return error | ❌ |
| Path traversal attempt | Block and return error | ❌ |

### 4.3 Pool Manager Failures

| Scenario | Expected Behavior | Test |
|----------|-------------------|------|
| Pool at max_idle | Reject checkin, terminate | ❌ |
| Pool checkout timeout | Fall back to direct create | ❌ |
| Recovery with orphaned sandboxes | Mark terminated | ✅ Unit |
| Recovery with unregistered templates | Skip | ✅ Unit |

### 4.4 Worker/Registry Failures

| Scenario | Expected Behavior | Test |
|----------|-------------------|------|
| Worker disconnects mid-command | Timeout, circuit breaker | ❌ |
| Worker registration rejected | Return error | ❌ |
| Rate limit exceeded | Return `RateLimited` | ❌ |
| Command cancelled | Process killed | ❌ |
| Large file read (>10MB) | Chunked transfer | ❌ |

---

## 5. Test Implementation Order

### Phase 1: CRITICAL - Gateway Tool Coverage (CI)

```
tests/gateway_test.rs
├── test_sandbox_run_stream_basic           # Basic streaming
├── test_sandbox_run_stream_progress       # Progress notifications
├── test_sandbox_run_exit_codes           # Non-zero exits
├── test_sandbox_write_binary             # Binary content
├── test_sandbox_read_nonexistent         # Error case
├── test_sandbox_list_files_empty         # Empty directory
├── test_sandbox_list_files_special_chars # Filenames with spaces/special chars
└── test_sandbox_metrics_format           # Prometheus format validation
```

### Phase 2: HIGH - Provider Coverage (CI)

```
tests/provider_test.rs  
├── test_provider_capabilities             # Verify capabilities()
├── test_podman_registry_routing          # CommandRouter fallback path
├── test_podman_file_binary_content       # Binary file write/read
└── test_podman_large_file                # Large file handling
```

### Phase 3: HIGH - Pool Manager Integration (CI)

```
tests/pool_integration_test.rs
├── test_pool_register_template           # Template registration
├── test_pool_evict_idle                  # Idle eviction
├── test_pool_checkout_timeout_fallback  # Direct create fallback
└── test_pool_stats_accuracy             # Verify stats match reality
```

### Phase 4: MEDIUM - Worker Protocol (CI with Podman)

```
tests/worker_protocol_test.rs
├── test_worker_read_file_chunked        # Multi-chunk reads
├── test_worker_write_file_chunked       # Multi-chunk writes
├── test_worker_list_directory           # Directory listing
├── test_worker_cancel_command           # Cancel running process
├── test_worker_graceful_shutdown        # Drain pending commands
├── test_worker_path_traversal_blocked   # Security: path traversal
└── test_worker_rate_limiting            # Rate limit enforcement
```

### Phase 5: LOCAL ONLY - Firecracker/gVisor (Manual)

```
tests/local_firecracker_test.rs          # Requires KVM
tests/local_gvisor_test.rs               # Requires runsc
```

---

## 6. Code Examples for Critical Tests

### 6.1 Test: sandbox_run_stream with Progress

```rust
#[tokio::test]
async fn test_sandbox_run_stream_progress() {
    let (mut child, mut stdin, mut reader) = spawn_gateway();
    init_gateway(&mut child, &mut stdin, &mut reader);

    // Create sandbox
    let sandbox_id = create_sandbox(&mut stdin, &mut reader);

    // Call sandbox_run_stream with progress token
    let progress_token = "test-token-123";
    let response = send_request(
        &mut stdin,
        &mut reader,
        "tools/call",
        json!({
            "name": "sandbox_run_stream",
            "arguments": {
                "sandbox_id": sandbox_id,
                "command": "for i in 1 2 3 4 5; do echo $i; sleep 0.5; done"
            },
            "progressToken": progress_token
        }),
    );

    // Verify response structure
    let text = extract_response_text(&response);
    let result: Value = serde_json::from_str(&text).unwrap_or(json!({}));
    
    assert!(result.get("exit_code").is_some());
    assert!(result.get("stdout").is_some());
    
    // Cleanup
    let _ = terminate_sandbox(&sandbox_id, &mut stdin, &mut reader);
    let _ = child.kill();
}
```

### 6.2 Test: Path Traversal Prevention (Security-Critical)

```rust
#[tokio::test]
async fn test_worker_path_traversal_blocked() {
    let provider = create_provider();
    let sandbox_id = SandboxId::generate();

    provider.create(
        &sandbox_id, "debian:bookworm-slim",
        &ResourcesSpec::default(), &NetworkSpec::default(),
        &HashMap::new(), 60_000
    ).await.expect("Failed to create");

    // Set router for registry-based routing
    provider.set_command_router(registry_client.clone());

    // Attempt path traversal - should be blocked by validate_path()
    let result = provider.read_file(
        &sandbox_id,
        "/etc/passwd"  // Should be blocked - outside /workspace, /tmp, etc.
    ).await;

    assert!(result.is_err(), "Path traversal should be blocked");
    let err = result.unwrap_err();
    assert!(err.to_string().contains("outside allowed") || 
            err.to_string().contains("permission"));

    provider.terminate(&sandbox_id).await.expect("Cleanup failed");
}
```

### 6.3 Test: Pool Eviction

```rust
#[tokio::test]
async fn test_pool_evict_idle() {
    // Create pool manager with very short idle timeout
    let config = PoolConfig {
        min_idle: 0,
        max_idle: 2,
        idle_timeout_ms: 100,  // 100ms for testing
        ..Default::default()
    };
    
    let manager = SandboxPoolManager::new(
        provider.clone(), repository.clone(), config
    );
    manager.register_template("debian:bookworm-slim");
    manager.start().await.expect("Failed to start");
    
    // Checkout sandboxes and return them
    for _ in 0..3 {
        let sandbox = manager.checkout("debian:bookworm-slim", 60_000).await
            .expect("Checkout failed");
        manager.checkin(&sandbox.id).await.expect("Checkin failed");
    }
    
    // Wait for idle timeout
    tokio::time::sleep(Duration::from_millis(200)).await;
    
    // Trigger eviction (via stats or manual call)
    let stats_before = manager.stats().await;
    
    // Wait for refill loop to run
    tokio::time::sleep(Duration::from_millis(6000)).await;
    
    let stats_after = manager.stats().await;
    
    // Verify idle count didn't exceed max_idle
    assert!(stats_after.idle <= config.max_idle);
    
    manager.stop().await.expect("Failed to stop");
}
```

### 6.4 Test: Command Cancellation

```rust
#[tokio::test]
async fn test_worker_cancel_command() {
    // Requires registry routing
    let registry = Arc::new(RegistryService::new());
    let mut provider = create_provider();
    provider.set_command_router(registry.clone());

    let sandbox_id = SandboxId::generate();
    let sandbox = provider.create(&sandbox_id, "", &ResourcesSpec::default(), 
        &NetworkSpec::default(), &HashMap::new(), 60_000).await
        .expect("Create failed");

    // Wait for worker to connect
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Start a long-running command
    let cmd = CommandSpec::new("sleep 30");
    let stream = provider.run_command_stream(&sandbox_id, &cmd).await
        .expect("Stream failed");

    // Issue cancel
    registry.cancel_command(&sandbox_id, command_id).await;

    // Verify command was cancelled (exit code or error)
    // ... verify stream ends with cancellation

    provider.terminate(&sandbox_id).await.expect("Cleanup failed");
}
```

---

## 7. CI Considerations

### 7.1 Tests that CAN run in CI

| Category | Tests | Why |
|----------|-------|-----|
| Gateway E2E | All in `e2e_test.rs` | Podman available in CI |
| Podman Lifecycle | All in `podman_lifecycle.rs` | Podman available |
| E2E Worker v2 | All in `e2e_worker_v2.rs` | Podman available |
| Self-Testing | `self_test_cargo_check`, `self_test_cargo_test_unit` | Podman + source mount |
| Unit Tests | Metrics, Pool Manager | No runtime dependencies |

### 7.2 Tests that CANNOT run in CI

| Category | Tests | Why |
|----------|-------|-----|
| Firecracker | All in `firecracker_lifecycle.rs` | Requires KVM (`/dev/kvm`) |
| gVisor | All in `gvisor_lifecycle.rs` | Requires `runsc` binary |
| KVM-dependent | Any test requiring hardware virt | CI containers lack KVM |

### 7.3 Conditional Test Execution

```rust
// Example: Conditional test based on environment
#[tokio::test]
async fn test_firecracker_only() {
    // Check if KVM is available
    if !Path::new("/dev/kvm").exists() {
        eprintln!("Skipping: KVM not available");
        return;
    }
    // ... test implementation
}

#[tokio::test] 
async fn test_gvisor_only() {
    // Check if runsc is available
    if which("runsc").is_none() {
        eprintln!("Skipping: runsc not found");
        return;
    }
    // ... test implementation
}
```

---

## 8. Summary: Tests to Implement

### CRITICAL (Do First)
1. `test_sandbox_run_stream_basic` - Streaming command execution
2. `test_sandbox_run_stream_progress` - Progress notifications
3. `test_worker_path_traversal_blocked` - Security critical
4. `test_podman_registry_routing` - CommandRouter fallback path

### HIGH Priority
5. `test_sandbox_run_exit_codes` - Non-zero exit codes
6. `test_sandbox_write_binary` - Binary content
7. `test_provider_capabilities` - Verify capabilities match reality
8. `test_pool_evict_idle` - Idle eviction
9. `test_pool_stats_accuracy` - Verify stats
10. `test_worker_read_file_chunked` - Multi-chunk reads
11. `test_worker_write_file_chunked` - Multi-chunk writes
12. `test_worker_cancel_command` - Cancellation

### MEDIUM Priority
13. `test_sandbox_read_nonexistent` - Error handling
14. `test_sandbox_list_files_empty` - Empty directory
15. `test_sandbox_list_files_special_chars` - Special chars in names
16. `test_worker_list_directory` - Directory listing
17. `test_worker_graceful_shutdown` - Drain commands
18. `test_worker_rate_limiting` - Rate limit enforcement
19. `test_pool_register_template` - Template registration
20. `test_pool_checkout_timeout_fallback` - Fallback on timeout

### LOCAL ONLY (Manual/Slow)
21. Firecracker lifecycle tests (requires KVM)
22. gVisor lifecycle tests (requires runsc)
23. Snapshot create/restore (slow, requires special setup)

---

## 9. Test Naming Convention

```
test_{component}_{feature}_{scenario}

Examples:
- test_gateway_run_stream_basic
- test_podman_write_binary_content
- test_worker_cancel_command_while_running
- test_pool_evict_idle_after_timeout
- test_firecracker_snapshot_create
```

---

## 10. Test Documentation Requirements

Each test MUST include:
1. **Description**: What it tests
2. **Setup**: What prerequisites are needed
3. **Execution**: Steps to run the test
4. **Assertions**: What is verified
5. **Cleanup**: How resources are freed
6. **Skip conditions**: When to skip (e.g., Podman unavailable)
7. **CI eligibility**: Whether it can run in CI

---

*Document generated: 2026-05-02*
*Next action: Begin implementing Phase 1 tests*
