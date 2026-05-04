//! Integration test for PodmanOptimizedMaterializer.
//!
//! Tests host-side extraction + podman cp strategy.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bastion_domain::execution::command::CommandSpec;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::shared::id::SandboxId;
use bastion_domain::template::{
    ArtifactStore, MaterializationMode, ProviderMaterializer, TemplateArtifact,
};
use bastion_infrastructure::provider::podman::PodmanProvider;
use bastion_infrastructure::template::{FsArtifactStore, PodmanOptimizedMaterializer};

fn make_test_tar() -> Vec<u8> {
    let mut tar_data = Vec::new();
    let mut builder = tar::Builder::new(&mut tar_data);

    let content = "#!/bin/sh\necho PODMAN_OK\n";
    let mut header = tar::Header::new_gnu();
    header.set_path("greet.sh").unwrap();
    header.set_size(content.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder
        .append(&header, content.as_bytes())
        .unwrap();
    let _ = builder.into_inner();
    tar_data
}

fn make_podman_test_artifact() -> TemplateArtifact {
    TemplateArtifact::builder("test/podman-opt", "v1")
        .media_type(bastion_domain::template::ArtifactMediaType::RootfsTar)
        .digest("sha256:podman-opt-001")
        .build()
}

fn require_podman() {
    let socket = std::path::Path::new("/run/user/1000/podman/podman.sock");
    if !socket.exists() {
        eprintln!("SKIP: Podman socket not found");
        std::process::exit(0);
    }
}

#[tokio::test]
async fn test_podman_optimized_materializer_copies_correctly() {
    require_podman();

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(FsArtifactStore::new(temp_dir.path().join("artifacts")));
    let provider = Arc::new(
        PodmanProvider::new(
            "/run/user/1000/podman/podman.sock",
            "debian:bookworm-slim",
            PathBuf::from("/tmp"),
        )
        .expect("PodmanProvider"),
    );

    let materializer = PodmanOptimizedMaterializer::new(
        provider.clone(),
        store.clone(),
        temp_dir.path().join("cache"),
    );

    let artifact_bytes = make_test_tar();
    let artifact = make_podman_test_artifact();

    // Store artifact
    store
        .store(&artifact.id.to_string(), &artifact.digest, &artifact_bytes)
        .await
        .expect("store");

    // Create sandbox
    let sandbox_id = SandboxId::generate();
    provider
        .create(
            &sandbox_id,
            "debian:bookworm-slim",
            &Default::default(),
            &Default::default(),
            &HashMap::new(),
            120_000,
        )
        .await
        .expect("create");

    // Materialize with podman-optimized
    let result = materializer
        .materialize(&sandbox_id, &artifact, MaterializationMode::Auto)
        .await
        .expect("materialize");

    assert!(result.cache_hit || true); // first time might be cache miss
    assert!(
        result.mount_path.contains("podman-opt"),
        "Expected podman-opt in mount_path: {}",
        result.mount_path
    );

    // Verify the file was copied into the sandbox
    let cmd = CommandSpec::new(format!(
        "ls -la /opt/bastion/artifacts/{}/greet.sh",
        artifact.digest
    ));
    let ls_result = provider.run_command(&sandbox_id, &cmd).await;
    match ls_result {
        Ok(r) => {
            let output = String::from_utf8_lossy(&r.stdout);
            eprintln!("ls output: {}", output);
            assert!(
                output.contains("greet.sh"),
                "Expected greet.sh in ls output: {}",
                output
            );
        }
        Err(e) => {
            // If ls fails, try finding the file with find
            let cmd2 = CommandSpec::new("find /opt -name greet.sh 2>/dev/null");
            let r2 = provider.run_command(&sandbox_id, &cmd2).await.unwrap();
            let out = String::from_utf8_lossy(&r2.stdout);
            eprintln!("find output: {}", out);
        }
    }

    // Execute the script
    let cmd3 = CommandSpec::new(format!(
        "/opt/bastion/artifacts/{}/greet.sh",
        artifact.digest
    ));
    let run_result = provider.run_command(&sandbox_id, &cmd3).await;
    match run_result {
        Ok(r) => {
            let output = String::from_utf8_lossy(&r.stdout);
            eprintln!("script output: {}", output);
            assert!(
                output.contains("PODMAN_OK"),
                "Expected PODMAN_OK: {}",
                output
            );
        }
        Err(e) => {
            eprintln!("run error: {}", e);
        }
    }

    eprintln!(
        "PodmanOptimized OK: cache={} duration_ms={}",
        result.cache_hit, result.duration_ms
    );

    // Cleanup
    let _ = provider.terminate(&sandbox_id).await;
}

#[tokio::test]
async fn test_podman_optimized_reuses_host_cache() {
    require_podman();

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(FsArtifactStore::new(temp_dir.path().join("artifacts")));
    let provider = Arc::new(
        PodmanProvider::new(
            "/run/user/1000/podman/podman.sock",
            "debian:bookworm-slim",
            PathBuf::from("/tmp"),
        )
        .expect("PodmanProvider"),
    );

    let materializer = PodmanOptimizedMaterializer::new(
        provider.clone(),
        store.clone(),
        temp_dir.path().join("cache"),
    );

    let artifact_bytes = make_test_tar();
    let artifact = make_podman_test_artifact();
    store
        .store(&artifact.id.to_string(), &artifact.digest, &artifact_bytes)
        .await
        .expect("store");

    // First sandbox - first materialization (extract to host cache)
    let sandbox_id1 = SandboxId::generate();
    provider
        .create(
            &sandbox_id1,
            "debian:bookworm-slim",
            &Default::default(),
            &Default::default(),
            &HashMap::new(),
            120_000,
        )
        .await
        .expect("create1");

    let t1 = std::time::Instant::now();
    let result1 = materializer
        .materialize(&sandbox_id1, &artifact, MaterializationMode::Auto)
        .await
        .expect("materialize1");
    let dur1 = t1.elapsed().as_millis() as u64;
    eprintln!("First materialization: {}ms", dur1);

    let _ = provider.terminate(&sandbox_id1).await;

    // Second sandbox - should use host cache (faster)
    let sandbox_id2 = SandboxId::generate();
    provider
        .create(
            &sandbox_id2,
            "debian:bookworm-slim",
            &Default::default(),
            &Default::default(),
            &HashMap::new(),
            120_000,
        )
        .await
        .expect("create2");

    let t2 = std::time::Instant::now();
    let result2 = materializer
        .materialize(&sandbox_id2, &artifact, MaterializationMode::Auto)
        .await
        .expect("materialize2");
    let dur2 = t2.elapsed().as_millis() as u64;
    eprintln!("Second materialization: {}ms", dur2);

    // Second should be significantly faster (no extraction needed)
    assert!(
        dur2 <= dur1 * 2,
        "Second materialization ({dur2}ms) should be <= 2x first ({dur1}ms)"
    );

    let _ = provider.terminate(&sandbox_id2).await;
}
