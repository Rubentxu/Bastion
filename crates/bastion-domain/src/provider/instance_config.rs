//! Provider instance configuration types.
//!
//! Compositional struct containing provider-specific configuration.

use serde::{Deserialize, Serialize};

use super::image_reference::ImageReference;
use super::mount_ref::{ContainerNetworkMode, MountRef};
use super::socket_ref::SocketRef;
use super::worker_binary_source::WorkerBinarySource;
use crate::shared::DomainError;

// ============================================================================
// Wasm Runtime Types
// ============================================================================

/// Wasm runtime kind.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WasmRuntimeKind {
    /// Wasmtime runtime.
    #[default]
    Wasmtime,
    /// WasmEdge runtime.
    WasmEdge,
    /// WAMR runtime.
    Wamr,
    /// Wasmer runtime.
    Wasmer,
}

/// WASM runtime configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmRuntime {
    kind: WasmRuntimeKind,
    binary_path: Option<String>,
}

impl WasmRuntime {
    /// Create a new WASM runtime with the specified kind.
    pub fn new(kind: WasmRuntimeKind) -> Self {
        Self {
            kind,
            binary_path: None,
        }
    }

    /// Create a new WASM runtime with a custom binary path.
    pub fn with_binary_path(mut self, path: impl Into<String>) -> Self {
        self.binary_path = Some(path.into());
        self
    }

    /// Accessor: runtime kind.
    pub fn kind(&self) -> WasmRuntimeKind {
        self.kind
    }

    /// Accessor: optional path to the runtime binary.
    pub fn binary_path(&self) -> Option<&str> {
        self.binary_path.as_deref()
    }

    /// Check if this runtime has a custom binary path.
    pub fn has_custom_binary(&self) -> bool {
        self.binary_path.is_some()
    }
}

// ============================================================================
// AWS Credentials Types
// ============================================================================

/// AWS credentials kind.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AwsCredentialsKind {
    /// Static access key and secret key.
    Static,
    /// AWS profile from ~/.aws/credentials.
    Profile,
    /// IAM role for pod.
    #[default]
    IamRole,
    /// Web identity token (for ServiceAccount).
    WebIdentity,
}

/// AWS credentials configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsCredentials {
    kind: AwsCredentialsKind,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    profile: Option<String>,
    role_arn: Option<String>,
    session_name: Option<String>,
}

impl AwsCredentials {
    /// Create credentials from an IAM role (for Kubernetes service account).
    pub fn iam_role(role_arn: impl Into<String>) -> Self {
        Self {
            kind: AwsCredentialsKind::IamRole,
            access_key_id: None,
            secret_access_key: None,
            profile: None,
            role_arn: Some(role_arn.into()),
            session_name: None,
        }
    }

    /// Create static credentials.
    ///
    /// Returns `Err(DomainError::Validation)` if either key is empty.
    pub fn static_creds(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let access_key_id = access_key_id.into();
        let secret_access_key = secret_access_key.into();

        if access_key_id.trim().is_empty() {
            return Err(DomainError::Validation(
                "AWS access key ID cannot be empty".into(),
            ));
        }
        if secret_access_key.trim().is_empty() {
            return Err(DomainError::Validation(
                "AWS secret access key cannot be empty".into(),
            ));
        }

        Ok(Self {
            kind: AwsCredentialsKind::Static,
            access_key_id: Some(access_key_id),
            secret_access_key: Some(secret_access_key),
            profile: None,
            role_arn: None,
            session_name: None,
        })
    }

    /// Accessor: credentials kind.
    pub fn kind(&self) -> AwsCredentialsKind {
        self.kind
    }

    /// Accessor: access key ID (only meaningful for Static kind).
    pub fn access_key_id(&self) -> Option<&str> {
        self.access_key_id.as_deref()
    }

    /// Accessor: secret access key (only meaningful for Static kind).
    pub fn secret_access_key(&self) -> Option<&str> {
        self.secret_access_key.as_deref()
    }

    /// Accessor: profile name (only meaningful for Profile kind).
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    /// Accessor: IAM role ARN (meaningful for IamRole and WebIdentity kinds).
    pub fn role_arn(&self) -> Option<&str> {
        self.role_arn.as_deref()
    }

    /// Accessor: session name for STS assume role.
    pub fn session_name(&self) -> Option<&str> {
        self.session_name.as_deref()
    }

    /// Check if this credential configuration is valid.
    ///
    /// Valid configurations:
    /// - Static: requires both access_key_id and secret_access_key
    /// - Profile: requires profile
    /// - IamRole: requires role_arn
    /// - WebIdentity: requires role_arn
    pub fn is_valid(&self) -> bool {
        match self.kind {
            AwsCredentialsKind::Static => {
                self.access_key_id.is_some() && self.secret_access_key.is_some()
            }
            AwsCredentialsKind::Profile => self.profile.is_some(),
            AwsCredentialsKind::IamRole => self.role_arn.is_some(),
            AwsCredentialsKind::WebIdentity => self.role_arn.is_some(),
        }
    }
}

// ============================================================================
// Kubernetes Credentials Types
// ============================================================================

/// Kubernetes credentials kind.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum K8sCredentialsKind {
    /// In-cluster service account.
    #[default]
    InCluster,
    /// Kubeconfig file.
    Kubeconfig,
    /// Static token file.
    TokenFile,
}

/// Kubernetes credentials configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubernetesCredentials {
    kind: K8sCredentialsKind,
    kubeconfig_path: Option<String>,
    token_file: Option<String>,
    namespace: Option<String>,
}

impl KubernetesCredentials {
    /// Create in-cluster credentials (uses service account).
    pub fn in_cluster() -> Self {
        Self {
            kind: K8sCredentialsKind::InCluster,
            kubeconfig_path: None,
            token_file: None,
            namespace: None,
        }
    }

    /// Create kubeconfig-based credentials.
    pub fn kubeconfig(path: impl Into<String>) -> Self {
        Self {
            kind: K8sCredentialsKind::Kubeconfig,
            kubeconfig_path: Some(path.into()),
            token_file: None,
            namespace: None,
        }
    }

    /// Create token file credentials.
    pub fn token_file(path: impl Into<String>) -> Self {
        Self {
            kind: K8sCredentialsKind::TokenFile,
            kubeconfig_path: None,
            token_file: Some(path.into()),
            namespace: None,
        }
    }

    /// Accessor: credentials kind.
    pub fn kind(&self) -> K8sCredentialsKind {
        self.kind
    }

    /// Accessor: path to kubeconfig file (only meaningful for Kubeconfig kind).
    pub fn kubeconfig_path(&self) -> Option<&str> {
        self.kubeconfig_path.as_deref()
    }

    /// Accessor: path to token file (only meaningful for TokenFile kind).
    pub fn token_file_path(&self) -> Option<&str> {
        self.token_file.as_deref()
    }

    /// Accessor: optional namespace override.
    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    /// Check if this credential configuration is valid.
    ///
    /// Valid configurations:
    /// - InCluster: always valid
    /// - Kubeconfig: requires kubeconfig_path
    /// - TokenFile: requires token_file
    pub fn is_valid(&self) -> bool {
        match self.kind {
            K8sCredentialsKind::InCluster => true,
            K8sCredentialsKind::Kubeconfig => self.kubeconfig_path.is_some(),
            K8sCredentialsKind::TokenFile => self.token_file.is_some(),
        }
    }
}

// ============================================================================
// Provider-Specific Config Structs
// ============================================================================

/// Firecracker-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirecrackerConfig {
    /// Path to the firecracker binary.
    pub firecracker_binary: Option<String>,
    /// Path to the kernel image.
    pub kernel: Option<String>,
    /// Path to the root filesystem image.
    pub rootfs: Option<String>,
    /// Worker binary source.
    pub worker_binary: Option<WorkerBinarySource>,
    /// Additional boot arguments.
    pub boot_args: Option<String>,
}

impl FirecrackerConfig {
    /// Create a new FirecrackerConfig with defaults.
    pub fn new() -> Self {
        Self {
            firecracker_binary: None,
            kernel: None,
            rootfs: None,
            worker_binary: None,
            boot_args: None,
        }
    }
}

impl Default for FirecrackerConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Kubernetes-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubernetesConfig {
    /// Kubernetes API server endpoint.
    pub cluster_endpoint: Option<String>,
    /// Kubernetes namespace.
    pub namespace: Option<String>,
    /// Credentials for the cluster.
    pub credentials: Option<KubernetesCredentials>,
    /// Default volume snapshot class.
    pub default_volume_snapshot_class: Option<String>,
}

impl KubernetesConfig {
    /// Create a new KubernetesConfig with defaults.
    pub fn new() -> Self {
        Self {
            cluster_endpoint: None,
            namespace: None,
            credentials: None,
            default_volume_snapshot_class: None,
        }
    }
}

impl Default for KubernetesConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Lambda-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LambdaConfig {
    /// AWS region.
    pub region: String,
    /// AWS credentials.
    pub credentials: Option<AwsCredentials>,
}

impl LambdaConfig {
    /// Create a new LambdaConfig.
    ///
    /// Returns `Err(DomainError::Validation)` if region is empty.
    pub fn new(region: impl Into<String>) -> Result<Self, DomainError> {
        let region = region.into();
        if region.trim().is_empty() {
            return Err(DomainError::Validation(
                "Lambda region cannot be empty".into(),
            ));
        }
        Ok(Self {
            region,
            credentials: None,
        })
    }
}

/// Local provider-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalConfig {
    /// Workspace directory for local execution.
    pub workspace_dir: String,
}

impl LocalConfig {
    /// Create a new LocalConfig.
    ///
    /// Returns `Err(DomainError::Validation)` if workspace_dir is empty.
    pub fn new(workspace_dir: impl Into<String>) -> Result<Self, DomainError> {
        let workspace_dir = workspace_dir.into();
        if workspace_dir.trim().is_empty() {
            return Err(DomainError::Validation(
                "Local workspace_dir cannot be empty".into(),
            ));
        }
        Ok(Self { workspace_dir })
    }
}

// ============================================================================
// Provider Instance Config (Compositional)
// ============================================================================

/// Provider instance configuration.
///
/// A compositional struct containing provider-specific configuration fields.
/// Serialized with `provider_type` field as the type discriminator.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProviderInstanceConfig {
    /// Provider type identifier (e.g., "podman", "docker", "firecracker").
    #[serde(rename = "type_id")]
    pub provider_type: String,

    // ── Shared fields (used by multiple container-based providers) ───────────

    /// Socket reference (used by Podman, Docker).
    pub socket: Option<SocketRef>,
    /// Default image to use.
    pub image: Option<ImageReference>,
    /// Worker binary source.
    pub worker_binary: Option<WorkerBinarySource>,
    /// Mounts to create.
    pub mounts: Vec<MountRef>,
    /// Network mode.
    pub network_mode: Option<ContainerNetworkMode>,

    // ── gVisor-specific fields ──────────────────────────────────────────────

    /// Path to the runsc binary.
    pub runsc_binary: Option<String>,
    /// Root filesystem directory.
    pub rootfs_dir: Option<String>,

    // ── Firecracker-specific config ─────────────────────────────────────────

    /// Firecracker-specific configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firecracker_config: Option<FirecrackerConfig>,

    // ── WASM-specific fields ────────────────────────────────────────────────

    /// WASM runtime configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm_runtime: Option<WasmRuntime>,

    // ── Kubernetes-specific config ──────────────────────────────────────────

    /// Kubernetes-specific configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kubernetes_config: Option<KubernetesConfig>,

    // ── Lambda-specific config ─────────────────────────────────────────────

    /// Lambda-specific configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lambda_config: Option<LambdaConfig>,

    // ── Local-specific config ───────────────────────────────────────────────

    /// Local-specific configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_config: Option<LocalConfig>,
}

impl ProviderInstanceConfig {
    /// Create a Podman configuration.
    pub fn podman() -> Self {
        Self {
            provider_type: "podman".to_string(),
            socket: None,
            image: None,
            worker_binary: None,
            mounts: Vec::new(),
            network_mode: None,
            runsc_binary: None,
            rootfs_dir: None,
            firecracker_config: None,
            wasm_runtime: None,
            kubernetes_config: None,
            lambda_config: None,
            local_config: None,
        }
    }

    /// Create a Docker configuration.
    pub fn docker() -> Self {
        Self {
            provider_type: "docker".to_string(),
            socket: None,
            image: None,
            worker_binary: None,
            mounts: Vec::new(),
            network_mode: None,
            runsc_binary: None,
            rootfs_dir: None,
            firecracker_config: None,
            wasm_runtime: None,
            kubernetes_config: None,
            lambda_config: None,
            local_config: None,
        }
    }

    /// Create a gVisor configuration.
    pub fn gvisor() -> Self {
        Self {
            provider_type: "gvisor".to_string(),
            socket: None,
            image: None,
            worker_binary: None,
            mounts: Vec::new(),
            network_mode: None,
            runsc_binary: None,
            rootfs_dir: None,
            firecracker_config: None,
            wasm_runtime: None,
            kubernetes_config: None,
            lambda_config: None,
            local_config: None,
        }
    }

    /// Create a Firecracker configuration.
    pub fn firecracker() -> Self {
        Self {
            provider_type: "firecracker".to_string(),
            socket: None,
            image: None,
            worker_binary: None,
            mounts: Vec::new(),
            network_mode: None,
            runsc_binary: None,
            rootfs_dir: None,
            firecracker_config: Some(FirecrackerConfig::new()),
            wasm_runtime: None,
            kubernetes_config: None,
            lambda_config: None,
            local_config: None,
        }
    }

    /// Create a WASM configuration with default runtime.
    pub fn wasm() -> Self {
        Self {
            provider_type: "wasm".to_string(),
            socket: None,
            image: None,
            worker_binary: None,
            mounts: Vec::new(),
            network_mode: None,
            runsc_binary: None,
            rootfs_dir: None,
            firecracker_config: None,
            wasm_runtime: Some(WasmRuntime::new(WasmRuntimeKind::default())),
            kubernetes_config: None,
            lambda_config: None,
            local_config: None,
        }
    }

    /// Create a Local configuration.
    ///
    /// Returns `Err(DomainError::Validation)` if workspace_dir is empty.
    pub fn local(workspace_dir: impl Into<String>) -> Result<Self, DomainError> {
        let local_config = LocalConfig::new(workspace_dir)?;
        Ok(Self {
            provider_type: "local".to_string(),
            socket: None,
            image: None,
            worker_binary: None,
            mounts: Vec::new(),
            network_mode: None,
            runsc_binary: None,
            rootfs_dir: None,
            firecracker_config: None,
            wasm_runtime: None,
            kubernetes_config: None,
            lambda_config: None,
            local_config: Some(local_config),
        })
    }

    /// Create a Kubernetes configuration.
    pub fn kubernetes() -> Self {
        Self {
            provider_type: "kubernetes".to_string(),
            socket: None,
            image: None,
            worker_binary: None,
            mounts: Vec::new(),
            network_mode: None,
            runsc_binary: None,
            rootfs_dir: None,
            firecracker_config: None,
            wasm_runtime: None,
            kubernetes_config: Some(KubernetesConfig::new()),
            lambda_config: None,
            local_config: None,
        }
    }

    /// Create a Lambda configuration.
    ///
    /// Returns `Err(DomainError::Validation)` if region is empty.
    pub fn lambda(region: impl Into<String>) -> Result<Self, DomainError> {
        let lambda_config = LambdaConfig::new(region)?;
        Ok(Self {
            provider_type: "lambda".to_string(),
            socket: None,
            image: None,
            worker_binary: None,
            mounts: Vec::new(),
            network_mode: None,
            runsc_binary: None,
            rootfs_dir: None,
            firecracker_config: None,
            wasm_runtime: None,
            kubernetes_config: None,
            lambda_config: Some(lambda_config),
            local_config: None,
        })
    }

    // ==================== Type Query Methods ================================

    /// Check if this is a Podman configuration.
    pub fn is_podman(&self) -> bool {
        self.provider_type == "podman"
    }

    /// Check if this is a Docker configuration.
    pub fn is_docker(&self) -> bool {
        self.provider_type == "docker"
    }

    /// Check if this is a gVisor configuration.
    pub fn is_gvisor(&self) -> bool {
        self.provider_type == "gvisor"
    }

    /// Check if this is a Firecracker configuration.
    pub fn is_firecracker(&self) -> bool {
        self.provider_type == "firecracker"
    }

    /// Check if this is a WASM configuration.
    pub fn is_wasm(&self) -> bool {
        self.provider_type == "wasm"
    }

    /// Check if this is a Local configuration.
    pub fn is_local(&self) -> bool {
        self.provider_type == "local"
    }

    /// Check if this is a Kubernetes configuration.
    pub fn is_kubernetes(&self) -> bool {
        self.provider_type == "kubernetes"
    }

    /// Check if this is a Lambda configuration.
    pub fn is_lambda(&self) -> bool {
        self.provider_type == "lambda"
    }

    // ==================== Shared Field Accessors ===========================

    /// Accessor: socket reference (Podman, Docker).
    pub fn socket(&self) -> Option<&SocketRef> {
        self.socket.as_ref()
    }

    /// Accessor: image reference.
    pub fn image(&self) -> Option<&ImageReference> {
        self.image.as_ref()
    }

    /// Accessor: worker binary source.
    pub fn worker_binary(&self) -> Option<&WorkerBinarySource> {
        self.worker_binary.as_ref()
    }

    /// Accessor: mounts.
    pub fn mounts(&self) -> &[MountRef] {
        &self.mounts
    }

    /// Accessor: network mode.
    pub fn network_mode(&self) -> Option<&ContainerNetworkMode> {
        self.network_mode.as_ref()
    }

    // ==================== gVisor Accessors ================================

    /// Accessor: gVisor runsc binary path.
    pub fn runsc_binary(&self) -> Option<&str> {
        self.runsc_binary.as_deref()
    }

    /// Accessor: gVisor root filesystem directory.
    pub fn rootfs_dir(&self) -> Option<&str> {
        self.rootfs_dir.as_deref()
    }

    // ==================== Firecracker Accessors =============================

    /// Accessor: Firecracker binary path.
    pub fn firecracker_binary(&self) -> Option<&str> {
        self.firecracker_config
            .as_ref()
            .and_then(|c| c.firecracker_binary.as_deref())
    }

    /// Accessor: Firecracker kernel image path.
    pub fn firecracker_kernel(&self) -> Option<&str> {
        self.firecracker_config
            .as_ref()
            .and_then(|c| c.kernel.as_deref())
    }

    /// Accessor: Firecracker rootfs image path.
    pub fn firecracker_rootfs(&self) -> Option<&str> {
        self.firecracker_config
            .as_ref()
            .and_then(|c| c.rootfs.as_deref())
    }

    /// Accessor: Firecracker worker binary source.
    pub fn firecracker_worker_binary(&self) -> Option<&WorkerBinarySource> {
        self.firecracker_config
            .as_ref()
            .and_then(|c| c.worker_binary.as_ref())
    }

    /// Accessor: Firecracker boot arguments.
    pub fn firecracker_boot_args(&self) -> Option<&str> {
        self.firecracker_config
            .as_ref()
            .and_then(|c| c.boot_args.as_deref())
    }

    // ==================== Wasm Accessors =================================

    /// Accessor: WASM runtime configuration.
    pub fn wasm_runtime(&self) -> Option<&WasmRuntime> {
        self.wasm_runtime.as_ref()
    }

    // ==================== Kubernetes Accessors =============================

    /// Accessor: Kubernetes API server endpoint.
    pub fn kubernetes_cluster_endpoint(&self) -> Option<&str> {
        self.kubernetes_config
            .as_ref()
            .and_then(|c| c.cluster_endpoint.as_deref())
    }

    /// Accessor: Kubernetes namespace.
    pub fn kubernetes_namespace(&self) -> Option<&str> {
        self.kubernetes_config
            .as_ref()
            .and_then(|c| c.namespace.as_deref())
    }

    /// Accessor: Kubernetes credentials.
    pub fn kubernetes_credentials(&self) -> Option<&KubernetesCredentials> {
        self.kubernetes_config
            .as_ref()
            .and_then(|c| c.credentials.as_ref())
    }

    /// Accessor: Kubernetes default volume snapshot class.
    pub fn kubernetes_default_volume_snapshot_class(&self) -> Option<&str> {
        self.kubernetes_config
            .as_ref()
            .and_then(|c| c.default_volume_snapshot_class.as_deref())
    }

    // ==================== Lambda Accessors =================================

    /// Accessor: Lambda region.
    pub fn lambda_region(&self) -> Option<&str> {
        self.lambda_config.as_ref().map(|c| c.region.as_str())
    }

    /// Accessor: Lambda credentials.
    pub fn lambda_credentials(&self) -> Option<&AwsCredentials> {
        self.lambda_config
            .as_ref()
            .and_then(|c| c.credentials.as_ref())
    }

    // ==================== Local Accessors ==================================

    /// Accessor: Local workspace directory.
    pub fn local_workspace_dir(&self) -> Option<&str> {
        self.local_config
            .as_ref()
            .map(|c| c.workspace_dir.as_str())
    }

    // ==================== Validation Helpers ==============================

    /// Check if Local config has a non-empty workspace directory.
    pub fn is_valid_local(&self) -> bool {
        self.local_config
            .as_ref()
            .map_or(false, |c| !c.workspace_dir.trim().is_empty())
    }

    /// Check if Lambda config has a non-empty region.
    pub fn is_valid_lambda(&self) -> bool {
        self.lambda_config
            .as_ref()
            .map_or(false, |c| !c.region.trim().is_empty())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_runtime_default_kind() {
        let runtime = WasmRuntime::new(WasmRuntimeKind::default());
        assert!(matches!(runtime.kind(), WasmRuntimeKind::Wasmtime));
        assert!(runtime.binary_path().is_none());
    }

    #[test]
    fn test_wasm_runtime_with_binary_path() {
        let runtime = WasmRuntime::new(WasmRuntimeKind::WasmEdge)
            .with_binary_path("/usr/bin/wasmedge");
        assert!(matches!(runtime.kind(), WasmRuntimeKind::WasmEdge));
        assert_eq!(runtime.binary_path(), Some("/usr/bin/wasmedge"));
    }

    #[test]
    fn test_wasm_runtime_accessors() {
        let runtime = WasmRuntime::new(WasmRuntimeKind::Wasmtime);
        assert!(!runtime.has_custom_binary());

        let runtime = runtime.with_binary_path("/custom/path");
        assert!(runtime.has_custom_binary());
    }

    #[test]
    fn test_aws_credentials_iam_role() {
        let creds = AwsCredentials::iam_role("arn:aws:iam::123456789:role/my-role");
        assert!(matches!(creds.kind(), AwsCredentialsKind::IamRole));
        assert_eq!(
            creds.role_arn(),
            Some("arn:aws:iam::123456789:role/my-role")
        );
        assert!(creds.is_valid());
    }

    #[test]
    fn test_aws_credentials_static() {
        let creds = AwsCredentials::static_creds(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        )
        .expect("valid credentials");
        assert!(matches!(creds.kind(), AwsCredentialsKind::Static));
        assert!(creds.access_key_id().is_some());
        assert!(creds.secret_access_key().is_some());
        assert!(creds.is_valid());
    }

    #[test]
    fn test_aws_credentials_static_empty_access_key() {
        let err = AwsCredentials::static_creds("", "secret")
            .expect_err("empty access key should fail");
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn test_aws_credentials_static_empty_secret() {
        let err = AwsCredentials::static_creds("key", "")
            .expect_err("empty secret should fail");
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn test_aws_credentials_accessors() {
        let creds = AwsCredentials::static_creds("key", "secret").unwrap();
        assert_eq!(creds.access_key_id(), Some("key"));
        assert_eq!(creds.secret_access_key(), Some("secret"));
        assert!(creds.profile().is_none());
        assert!(creds.session_name().is_none());
    }

    #[test]
    fn test_kubernetes_credentials_in_cluster() {
        let creds = KubernetesCredentials::in_cluster();
        assert!(matches!(creds.kind(), K8sCredentialsKind::InCluster));
        assert!(creds.is_valid());
    }

    #[test]
    fn test_kubernetes_credentials_kubeconfig() {
        let creds = KubernetesCredentials::kubeconfig("/path/to/kubeconfig");
        assert!(matches!(creds.kind(), K8sCredentialsKind::Kubeconfig));
        assert_eq!(creds.kubeconfig_path(), Some("/path/to/kubeconfig"));
        assert!(creds.is_valid());
    }

    #[test]
    fn test_kubernetes_credentials_invalid_kubeconfig() {
        let creds = KubernetesCredentials {
            kind: K8sCredentialsKind::Kubeconfig,
            kubeconfig_path: None,
            token_file: None,
            namespace: None,
        };
        assert!(!creds.is_valid());
    }

    #[test]
    fn test_kubernetes_credentials_token_file() {
        let creds = KubernetesCredentials::token_file("/path/to/token");
        assert!(matches!(creds.kind(), K8sCredentialsKind::TokenFile));
        assert_eq!(creds.token_file_path(), Some("/path/to/token"));
        assert!(creds.is_valid());
    }

    #[test]
    fn test_kubernetes_credentials_accessors() {
        let creds = KubernetesCredentials::kubeconfig("/path/to/kubeconfig");
        assert_eq!(creds.namespace(), None);
    }

    #[test]
    fn test_provider_instance_config_podman() {
        let config = ProviderInstanceConfig::podman();
        assert!(config.is_podman());
        assert_eq!(config.provider_type, "podman");
        assert!(config.socket().is_none());
        assert!(config.image().is_none());
        assert!(config.mounts().is_empty());
    }

    #[test]
    fn test_provider_instance_config_docker() {
        let config = ProviderInstanceConfig::docker();
        assert!(config.is_docker());
        assert_eq!(config.provider_type, "docker");
        assert!(config.socket().is_none());
    }

    #[test]
    fn test_provider_instance_config_wasm() {
        let config = ProviderInstanceConfig::wasm();
        assert!(config.is_wasm());
        assert!(config.wasm_runtime().is_some());
        assert!(config.worker_binary().is_none());
    }

    #[test]
    fn test_provider_instance_config_local_valid() {
        let config =
            ProviderInstanceConfig::local("/tmp/workspace").expect("valid workspace dir");
        assert!(config.is_local());
        assert_eq!(config.local_workspace_dir(), Some("/tmp/workspace"));
        assert!(config.is_valid_local());
    }

    #[test]
    fn test_provider_instance_config_local_empty() {
        let err = ProviderInstanceConfig::local("").expect_err("empty workspace should fail");
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn test_provider_instance_config_lambda_valid() {
        let config = ProviderInstanceConfig::lambda("us-east-1").expect("valid region");
        assert!(config.is_lambda());
        assert_eq!(config.lambda_region(), Some("us-east-1"));
        assert!(config.is_valid_lambda());
    }

    #[test]
    fn test_provider_instance_config_lambda_empty() {
        let err = ProviderInstanceConfig::lambda("").expect_err("empty region should fail");
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn test_provider_instance_config_gvisor() {
        let config = ProviderInstanceConfig::gvisor();
        assert!(config.is_gvisor());
        assert!(config.runsc_binary().is_none());
    }

    #[test]
    fn test_provider_instance_config_firecracker() {
        let config = ProviderInstanceConfig::firecracker();
        assert!(config.is_firecracker());
        assert!(config.firecracker_config.is_some());
        assert!(config.firecracker_binary().is_none());
    }

    #[test]
    fn test_provider_instance_config_kubernetes() {
        let config = ProviderInstanceConfig::kubernetes();
        assert!(config.is_kubernetes());
        assert!(config.kubernetes_config.is_some());
        assert!(config.kubernetes_credentials().is_none());
    }

    #[test]
    fn test_provider_instance_config_serde_podman() {
        let config = ProviderInstanceConfig::podman();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"type_id\":\"podman\""));
        let parsed: ProviderInstanceConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_podman());
    }

    #[test]
    fn test_provider_instance_config_serde_wasm() {
        let config = ProviderInstanceConfig::wasm();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"type_id\":\"wasm\""));
        let parsed: ProviderInstanceConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_wasm());
    }

    #[test]
    fn test_provider_instance_config_serde_local() {
        let config =
            ProviderInstanceConfig::local("/workspace").expect("valid workspace");
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"type_id\":\"local\""));
        let parsed: ProviderInstanceConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_local());
        assert_eq!(parsed.local_workspace_dir(), Some("/workspace"));
    }

    #[test]
    fn test_provider_instance_config_firecracker_serde() {
        let config = ProviderInstanceConfig::firecracker();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"type_id\":\"firecracker\""));
        let parsed: ProviderInstanceConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_firecracker());
        assert!(parsed.firecracker_config.is_some());
    }

    #[test]
    fn test_provider_instance_config_kubernetes_serde() {
        let config = ProviderInstanceConfig::kubernetes();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"type_id\":\"kubernetes\""));
        let parsed: ProviderInstanceConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_kubernetes());
        assert!(parsed.kubernetes_config.is_some());
    }

    #[test]
    fn test_provider_instance_config_lambda_serde() {
        let config = ProviderInstanceConfig::lambda("us-west-2").expect("valid region");
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"type_id\":\"lambda\""));
        let parsed: ProviderInstanceConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_lambda());
        assert_eq!(parsed.lambda_region(), Some("us-west-2"));
    }

    #[test]
    fn test_provider_instance_config_type_checks() {
        let config = ProviderInstanceConfig::podman();
        assert!(config.is_podman());
        assert!(!config.is_docker());
        assert!(!config.is_gvisor());
        assert!(!config.is_firecracker());
        assert!(!config.is_wasm());
        assert!(!config.is_local());
        assert!(!config.is_kubernetes());
        assert!(!config.is_lambda());
    }

    #[test]
    fn test_provider_instance_config_firecracker_accessors() {
        let config = ProviderInstanceConfig::firecracker();
        // All firecracker-specific accessors should work when config is present
        assert!(config.firecracker_config.is_some());
        assert!(config.firecracker_binary().is_none());
        assert!(config.firecracker_kernel().is_none());
        assert!(config.firecracker_rootfs().is_none());
        assert!(config.firecracker_boot_args().is_none());
    }

    #[test]
    fn test_provider_instance_config_kubernetes_accessors() {
        let config = ProviderInstanceConfig::kubernetes();
        assert!(config.kubernetes_config.is_some());
        assert!(config.kubernetes_cluster_endpoint().is_none());
        assert!(config.kubernetes_namespace().is_none());
        assert!(config.kubernetes_credentials().is_none());
        assert!(config.kubernetes_default_volume_snapshot_class().is_none());
    }
}
