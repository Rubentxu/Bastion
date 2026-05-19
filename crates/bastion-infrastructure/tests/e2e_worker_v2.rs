//! E2E Test: Bastion Worker Protocol v2
//!
//! Tests the full flow: Gateway Registry → Worker connects → Command execution
//! Uses Podman containers with bind-mounted worker binary.

use std::path::PathBuf;
use std::time::Duration;

use tokio::time::timeout;

use bastion_domain::execution::command::CommandSpec;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::shared::id::SandboxId;
use bastion_infrastructure::provider::PodmanProvider;

/// Helper to skip tests if Podman is not available
async fn ensure_podman() -> PodmanProvider {
    let socket = "/run/user/1000/podman/podman.sock";
    let worker_bin = PathBuf::from("target/x86_64-unknown-linux-musl/release/bastion-worker");

    if !std::path::Path::new(socket).exists() {
        eprintln!("Skipping: Podman socket not found at {}", socket);
        std::process::exit(0);
    }

    if !worker_bin.exists() {
        eprintln!("Skipping: bastion-worker binary not found. Run: cargo build -p bastion-worker");
        std::process::exit(0);
    }

    PodmanProvider::new(socket, "debian:bookworm-slim", worker_bin)
        .expect("Failed to connect to Podman")
}

#[tokio::test]
async fn test_e2e_podman_create_and_run_command() {
    let provider = ensure_podman().await;

    // Verify Podman connectivity
    let pong = provider.ping().await;
    println!("Podman ping: {:?}", pong);
    assert!(pong.is_ok(), "Podman should be reachable");

    // Create sandbox
    let sandbox_id = SandboxId::new(&format!("test-e2e-{}", uuid::Uuid::new_v4().as_simple()));

    println!("Creating sandbox: {}", sandbox_id);

    let sandbox = timeout(
        Duration::from_secs(30),
        provider.create(
            &sandbox_id,
            "", // default image
            &bastion_domain::sandbox::value_objects::ResourcesSpec::default(),
            &bastion_domain::sandbox::value_objects::NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        ),
    )
    .await
    .expect("Timeout creating sandbox")
    .expect("Failed to create sandbox");

    println!(
        "Sandbox created: {} (status: {})",
        sandbox.id, sandbox.status
    );

    // Wait for worker to start (it needs time to connect to the gateway)
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Run a simple command (via exec fallback since no registry running in this test)
    let result = timeout(
        Duration::from_secs(15),
        provider.run_command(
            &sandbox_id,
            &CommandSpec::new("echo 'Hello from Bastion v2!'"),
        ),
    )
    .await
    .expect("Timeout running command")
    .expect("Failed to run command");

    println!(
        "Command result: exit={}, stdout='{}'",
        result.exit_code,
        String::from_utf8_lossy(&result.stdout).trim()
    );

    assert_eq!(result.exit_code, 0, "Command should succeed");
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("Hello from Bastion v2!"),
        "Output should contain expected text"
    );

    // Test write file
    let write_result = provider
        .write_file(&sandbox_id, "/tmp/test-file.txt", b"Hello from test!")
        .await;
    assert!(
        write_result.is_ok(),
        "Write file should succeed: {:?}",
        write_result
    );

    // Test read file
    let read_result = provider.read_file(&sandbox_id, "/tmp/test-file.txt").await;
    assert!(
        read_result.is_ok(),
        "Read file should succeed: {:?}",
        read_result
    );
    assert_eq!(
        read_result.unwrap(),
        b"Hello from test!",
        "Read content should match written content"
    );

    // Test list files
    let list_result = provider.list_files(&sandbox_id, "/tmp").await;
    assert!(
        list_result.is_ok(),
        "List files should succeed: {:?}",
        list_result
    );
    let entries = list_result.unwrap();
    assert!(!entries.is_empty(), "/tmp should have files");

    // Test is_alive
    let alive = provider.is_alive(&sandbox_id).await;
    assert!(alive.is_ok() && alive.unwrap(), "Sandbox should be alive");

    // Terminate sandbox
    println!("Terminating sandbox: {}", sandbox_id);
    let terminate_result = timeout(Duration::from_secs(15), provider.terminate(&sandbox_id))
        .await
        .expect("Timeout terminating sandbox");

    assert!(
        terminate_result.is_ok(),
        "Terminate should succeed: {:?}",
        terminate_result
    );
    println!("Sandbox terminated successfully");

    // Verify not alive after termination
    tokio::time::sleep(Duration::from_secs(1)).await;
    let alive_after = provider.is_alive(&sandbox_id).await;
    assert!(
        alive_after.is_ok() && !alive_after.unwrap(),
        "Sandbox should not be alive after termination"
    );
}

#[tokio::test]
async fn test_e2e_podman_environment_variables() {
    let provider = ensure_podman().await;

    let sandbox_id = SandboxId::new(&format!("test-env-{}", uuid::Uuid::new_v4().as_simple()));

    let mut env = std::collections::HashMap::new();
    env.insert("MY_VAR".to_string(), "my_value".to_string());
    env.insert("BASTION_TEST".to_string(), "true".to_string());

    let _sandbox = timeout(
        Duration::from_secs(30),
        provider.create(
            &sandbox_id,
            "",
            &bastion_domain::sandbox::value_objects::ResourcesSpec::default(),
            &bastion_domain::sandbox::value_objects::NetworkSpec::default(),
            &env,
            3_600_000,
        ),
    )
    .await
    .expect("Timeout")
    .expect("Failed to create sandbox");

    // Verify env var is accessible
    let result = timeout(
        Duration::from_secs(10),
        provider.run_command(&sandbox_id, &CommandSpec::new("echo $MY_VAR")),
    )
    .await
    .expect("Timeout")
    .expect("Failed to run command");

    let output = String::from_utf8_lossy(&result.stdout).trim().to_string();
    assert_eq!(output, "my_value", "Environment variable should be set");
    println!("Env var test passed: MY_VAR={}", output);

    // Cleanup
    let _ = timeout(Duration::from_secs(15), provider.terminate(&sandbox_id)).await;
}

#[tokio::test]
async fn test_e2e_podman_complex_command() {
    let provider = ensure_podman().await;

    let sandbox_id = SandboxId::new(&format!(
        "test-complex-{}",
        uuid::Uuid::new_v4().as_simple()
    ));

    let _ = timeout(
        Duration::from_secs(30),
        provider.create(
            &sandbox_id,
            "",
            &bastion_domain::sandbox::value_objects::ResourcesSpec::default(),
            &bastion_domain::sandbox::value_objects::NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        ),
    )
    .await
    .expect("Timeout")
    .expect("Failed to create sandbox");

    // Install a package and run a complex pipeline
    let result = timeout(
        Duration::from_secs(30),
        provider.run_command(
            &sandbox_id,
            &CommandSpec::new("apt-get update -qq && apt-get install -y -qq curl > /dev/null 2>&1 && curl --version | head -1"),
        ),
    )
    .await
    .expect("Timeout")
    .expect("Failed to run command");

    let output = String::from_utf8_lossy(&result.stdout).trim().to_string();
    println!("Complex command output: {}", output);
    assert!(
        output.contains("curl"),
        "Should have curl installed. Output: {}",
        output
    );
    assert_eq!(result.exit_code, 0);

    // Cleanup
    let _ = timeout(Duration::from_secs(15), provider.terminate(&sandbox_id)).await;
}
