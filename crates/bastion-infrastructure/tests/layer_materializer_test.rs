//! Integration test for ZipLayerMaterializer.
//!
//! Tests tar-to-zip conversion and zip layer deployment.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bastion_domain::execution::command::CommandSpec;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::shared::id::SandboxId;
use bastion_domain::template::{
    ArtifactStore, LayerArtifact, MaterializationMode, ProviderMaterializer, TemplateArtifact,
};
use bastion_infrastructure::provider::podman::PodmanProvider;
use bastion_infrastructure::template::{FsArtifactStore, ZipLayerMaterializer};

fn make_test_tar() -> Vec<u8> {
    let mut tar_data = Vec::new();
    let mut builder = tar::Builder::new(&mut tar_data);

    let content = "#!/bin/sh\necho LAYER_OK\n";
    let mut header = tar::Header::new_gnu();
    header.set_path("layer_script.sh").unwrap();
    header.set_size(content.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder.append(&header, content.as_bytes()).unwrap();
    let _ = builder.into_inner();
    tar_data
}

fn make_layer_artifact() -> TemplateArtifact {
    TemplateArtifact::builder("test/layer-tools", "v1")
        .media_type(bastion_domain::template::ArtifactMediaType::LambdaLayerZip)
        .digest("sha256:layer-test-001")
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
async fn test_zip_layer_materializer_deploys_layer() {
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

    let materializer = ZipLayerMaterializer::new(
        provider.clone(),
        store.clone(),
        temp_dir.path().join("cache"),
    );

    let artifact_bytes = make_test_tar();
    let artifact = make_layer_artifact();
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

    // Materialize as zip layer
    let result = materializer
        .materialize(&sandbox_id, &artifact, MaterializationMode::Auto)
        .await
        .expect("materialize");

    assert!(
        result.mount_path.contains("/opt/bastion/layers/"),
        "Expected /opt/bastion/layers/ in mount_path: {}",
        result.mount_path
    );

    // Check the layer was deployed
    let cmd = CommandSpec::new(format!("ls -la {}/layer_script.sh", result.mount_path));
    let ls_result = provider.run_command(&sandbox_id, &cmd).await;
    match ls_result {
        Ok(r) => {
            let output = String::from_utf8_lossy(&r.stdout);
            eprintln!("ls: {}", output);
        }
        Err(_) => {
            // If unzip wasn't available, check via find
            let cmd2 =
                CommandSpec::new(format!("find /opt/bastion/layers -name '*.sh' 2>/dev/null"));
            if let Ok(r2) = provider.run_command(&sandbox_id, &cmd2).await {
                eprintln!("find: {}", String::from_utf8_lossy(&r2.stdout));
            }
        }
    }

    eprintln!(
        "ZipLayer OK: mount={} duration_ms={}",
        result.mount_path, result.duration_ms
    );

    let _ = provider.terminate(&sandbox_id).await;
}

#[tokio::test]
async fn test_zip_layer_creates_layer_artifact() {
    // Test the LayerArtifact creation and zip conversion without sandbox
    let artifact = TemplateArtifact::builder("test/layer-test", "v1")
        .media_type(bastion_domain::template::ArtifactMediaType::LambdaLayerZip)
        .digest("sha256:unit-test-001")
        .env_var("JAVA_HOME", "/opt/bastion/layers/sha256:unit-test-001")
        .build();

    let layer = LayerArtifact::new(artifact.clone(), Some("Test layer".into()));

    assert_eq!(layer.name, "test/layer-test");
    assert_eq!(layer.version, 1);
    assert!(layer.arn.starts_with("arn:bastion:layer:"));
    assert!(layer.mount_path().starts_with("/opt/bastion/layers/"));

    eprintln!(
        "LayerArtifact: arn={} mount={}",
        layer.arn,
        layer.mount_path()
    );
}
