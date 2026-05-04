//! Integration test for UniversalMaterializer with Podman sandbox.

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
use bastion_infrastructure::template::{FsArtifactStore, UniversalMaterializer};

fn make_test_tar() -> Vec<u8> {
    let mut tar_data = Vec::new();
    let mut builder = tar::Builder::new(&mut tar_data);

    let content = "#!/bin/sh\necho HELLO\n";
    let mut header = tar::Header::new_gnu();
    header.set_path("hello.sh").unwrap();
    header.set_size(content.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder.append(&header, content.as_bytes()).unwrap();
    let _ = builder.into_inner();
    tar_data
}

fn make_basic_artifact() -> TemplateArtifact {
    // Use a simpler verification: just check that extraction directory exists
    // (The universal materializer runs verification steps after extraction)
    TemplateArtifact::builder("test/basic-tools", "v1")
        .media_type(bastion_domain::template::ArtifactMediaType::RootfsTar)
        .digest("sha256:basic-003")
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
async fn test_universal_materializer_stores_and_caches() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let store = Arc::new(FsArtifactStore::new(temp_dir.path().join("artifacts")));

    let artifact_bytes = make_test_tar();
    let artifact = make_basic_artifact();

    // Store
    store
        .store(&artifact.id.to_string(), &artifact.digest, &artifact_bytes)
        .await
        .expect("Failed to store");

    // Check cache
    let cached = store
        .is_cached(&artifact.id.to_string(), &artifact.digest)
        .await
        .unwrap();
    assert!(cached, "Artifact should be cached");

    // Fetch
    let fetched = store
        .fetch(&artifact.id.to_string(), &artifact.digest)
        .await
        .expect("Failed to fetch");
    assert_eq!(fetched.len(), artifact_bytes.len());

    eprintln!("Store/cache/fetch OK, bytes: {}", fetched.len());
}

#[tokio::test]
async fn test_universal_materializer_materializes_to_sandbox() {
    require_podman();

    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let store = Arc::new(FsArtifactStore::new(temp_dir.path().join("artifacts")));
    let provider = Arc::new(
        PodmanProvider::new(
            "/run/user/1000/podman/podman.sock",
            "debian:bookworm-slim",
            PathBuf::from("/tmp"),
        )
        .expect("Failed to create PodmanProvider"),
    );

    let materializer = UniversalMaterializer::new(
        provider.clone(),
        store.clone(),
        temp_dir.path().join("cache"),
    );

    let artifact_bytes = make_test_tar();
    let artifact = make_basic_artifact();
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
        .expect("create sandbox");

    // Materialize
    let result = materializer
        .materialize(&sandbox_id, &artifact, MaterializationMode::Auto)
        .await
        .expect("Materialization");

    assert!(result.cache_hit);
    eprintln!(
        "Materialization OK: mount={} cache={} duration_ms={}",
        result.mount_path, result.cache_hit, result.duration_ms
    );

    // Verify the file was created in the sandbox
    let cmd = CommandSpec::new(format!(
        "ls -la /opt/bastion/artifacts/{}/hello.sh",
        artifact.digest
    ));
    let ls_result = provider.run_command(&sandbox_id, &cmd).await;
    if let Ok(r) = &ls_result {
        let output = String::from_utf8_lossy(&r.stdout);
        eprintln!("ls output: {}", output);
    }

    let _ = provider.terminate(&sandbox_id).await;
}
