//! Provider instance configuration types.
//!
//! Tagged enum containing provider-specific configuration for each instance type.

use serde::{Deserialize, Serialize};

use super::image_reference::ImageReference;
use super::mount_ref::{ContainerNetworkMode, MountRef};
use super::socket_ref::SocketRef;
use super::worker_binary_source::WorkerBinarySource;
use crate::shared::DomainError;

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

/// Provider instance configuration.
///
/// A tagged enum containing provider-specific configuration for each instance type.
/// Serialized to TOML with `type_id` as the tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type_id", rename_all = "snake_case")]
pub enum ProviderInstanceConfig {
    /// Podman provider configuration.
    Podman {
        /// Socket reference for Podman.
        socket: Option<SocketRef>,
        /// Default image to use.
        image: Option<ImageReference>,
        /// Worker binary source.
        worker_binary: Option<WorkerBinarySource>,
        /// Mounts to create.
        mounts: Vec<MountRef>,
        /// Network mode.
        network_mode: Option<ContainerNetworkMode>,
    },
    /// Docker provider configuration.
    Docker {
        /// Socket reference for Docker.
        socket: Option<SocketRef>,
        /// Default image to use.
        image: Option<ImageReference>,
        /// Worker binary source.
        worker_binary: Option<WorkerBinarySource>,
        /// Mounts to create.
        mounts: Vec<MountRef>,
        /// Network mode.
        network_mode: Option<ContainerNetworkMode>,
    },
    /// gVisor provider configuration.
    Gvisor {
        /// Path to the runsc binary.
        runsc_binary: Option<String>,
        /// Default image to use.
        image: Option<ImageReference>,
        /// Root filesystem directory.
        rootfs_dir: Option<String>,
        /// Worker binary source.
        worker_binary: Option<WorkerBinarySource>,
    },
    /// Firecracker provider configuration.
    Firecracker {
        /// Path to the firecracker binary.
        firecracker_binary: Option<String>,
        /// Path to the kernel image.
        kernel: Option<String>,
        /// Path to the root filesystem image.
        rootfs: Option<String>,
        /// Worker binary source.
        worker_binary: Option<WorkerBinarySource>,
        /// Additional boot arguments.
        boot_args: Option<String>,
    },
    /// WebAssembly provider configuration.
    Wasm {
        /// WASM runtime configuration.
        runtime: WasmRuntime,
        /// Worker binary source.
        worker_binary: Option<WorkerBinarySource>,
    },
    /// Local provider configuration.
    Local {
        /// Workspace directory for local execution.
        workspace_dir: String,
    },
    /// Kubernetes provider configuration.
    Kubernetes {
        /// Kubernetes API server endpoint.
        cluster_endpoint: Option<String>,
        /// Kubernetes namespace.
        namespace: Option<String>,
        /// Credentials for the cluster.
        credentials: Option<KubernetesCredentials>,
        /// Default volume snapshot class.
        default_volume_snapshot_class: Option<String>,
    },
    /// AWS Lambda provider configuration.
    Lambda {
        /// AWS region.
        region: String,
        /// AWS credentials.
        credentials: Option<AwsCredentials>,
    },
}

impl ProviderInstanceConfig {
    /// Create a Podman configuration.
    pub fn podman() -> Self {
        Self::Podman {
            socket: None,
            image: None,
            worker_binary: None,
            mounts: Vec::new(),
            network_mode: None,
        }
    }

    /// Create a Docker configuration.
    pub fn docker() -> Self {
        Self::Docker {
            socket: None,
            image: None,
            worker_binary: None,
            mounts: Vec::new(),
            network_mode: None,
        }
    }

    /// Create a gVisor configuration.
    pub fn gvisor() -> Self {
        Self::Gvisor {
            runsc_binary: None,
            image: None,
            rootfs_dir: None,
            worker_binary: None,
        }
    }

    /// Create a Firecracker configuration.
    pub fn firecracker() -> Self {
        Self::Firecracker {
            firecracker_binary: None,
            kernel: None,
            rootfs: None,
            worker_binary: None,
            boot_args: None,
        }
    }

    /// Create a WASM configuration with default runtime.
    pub fn wasm() -> Self {
        Self::Wasm {
            runtime: WasmRuntime::new(WasmRuntimeKind::default()),
            worker_binary: None,
        }
    }

    /// Create a Local configuration.
    ///
    /// Returns `Err(DomainError::Validation)` if workspace_dir is empty.
    pub fn local(workspace_dir: impl Into<String>) -> Result<Self, DomainError> {
        let workspace_dir = workspace_dir.into();
        if workspace_dir.trim().is_empty() {
            return Err(DomainError::Validation(
                "Local workspace_dir cannot be empty".into(),
            ));
        }
        Ok(Self::Local { workspace_dir })
    }

    /// Create a Kubernetes configuration.
    pub fn kubernetes() -> Self {
        Self::Kubernetes {
            cluster_endpoint: None,
            namespace: None,
            credentials: None,
            default_volume_snapshot_class: None,
        }
    }

    /// Create a Lambda configuration.
    ///
    /// Returns `Err(DomainError::Validation)` if region is empty.
    pub fn lambda(region: impl Into<String>) -> Result<Self, DomainError> {
        let region = region.into();
        if region.trim().is_empty() {
            return Err(DomainError::Validation(
                "Lambda region cannot be empty".into(),
            ));
        }
        Ok(Self::Lambda {
            region,
            credentials: None,
        })
    }

    // ==================== Podman Accessors ====================

    /// Accessor: Podman socket reference.
    pub fn podman_socket(&self) -> Option<&SocketRef> {
        match self {
            Self::Podman { socket, .. } => socket.as_ref(),
            _ => None,
        }
    }

    /// Accessor: Podman default image.
    pub fn podman_image(&self) -> Option<&ImageReference> {
        match self {
            Self::Podman { image, .. } => image.as_ref(),
            _ => None,
        }
    }

    /// Accessor: Podman worker binary source.
    pub fn podman_worker_binary(&self) -> Option<&WorkerBinarySource> {
        match self {
            Self::Podman { worker_binary, .. } => worker_binary.as_ref(),
            _ => None,
        }
    }

    /// Accessor: Podman mounts.
    pub fn podman_mounts(&self) -> &[MountRef] {
        match self {
            Self::Podman { mounts, .. } => mounts,
            _ => &[],
        }
    }

    /// Accessor: Podman network mode.
    pub fn podman_network_mode(&self) -> Option<&ContainerNetworkMode> {
        match self {
            Self::Podman { network_mode, .. } => network_mode.as_ref(),
            _ => None,
        }
    }

    // ==================== Docker Accessors ====================

    /// Accessor: Docker socket reference.
    pub fn docker_socket(&self) -> Option<&SocketRef> {
        match self {
            Self::Docker { socket, .. } => socket.as_ref(),
            _ => None,
        }
    }

    /// Accessor: Docker default image.
    pub fn docker_image(&self) -> Option<&ImageReference> {
        match self {
            Self::Docker { image, .. } => image.as_ref(),
            _ => None,
        }
    }

    /// Accessor: Docker worker binary source.
    pub fn docker_worker_binary(&self) -> Option<&WorkerBinarySource> {
        match self {
            Self::Docker { worker_binary, .. } => worker_binary.as_ref(),
            _ => None,
        }
    }

    /// Accessor: Docker mounts.
    pub fn docker_mounts(&self) -> &[MountRef] {
        match self {
            Self::Docker { mounts, .. } => mounts,
            _ => &[],
        }
    }

    /// Accessor: Docker network mode.
    pub fn docker_network_mode(&self) -> Option<&ContainerNetworkMode> {
        match self {
            Self::Docker { network_mode, .. } => network_mode.as_ref(),
            _ => None,
        }
    }

    // ==================== gVisor Accessors ====================

    /// Accessor: gVisor runsc binary path.
    pub fn gvisor_runsc_binary(&self) -> Option<&str> {
        match self {
            Self::Gvisor { runsc_binary, .. } => runsc_binary.as_deref(),
            _ => None,
        }
    }

    /// Accessor: gVisor default image.
    pub fn gvisor_image(&self) -> Option<&ImageReference> {
        match self {
            Self::Gvisor { image, .. } => image.as_ref(),
            _ => None,
        }
    }

    /// Accessor: gVisor root filesystem directory.
    pub fn gvisor_rootfs_dir(&self) -> Option<&str> {
        match self {
            Self::Gvisor { rootfs_dir, .. } => rootfs_dir.as_deref(),
            _ => None,
        }
    }

    /// Accessor: gVisor worker binary source.
    pub fn gvisor_worker_binary(&self) -> Option<&WorkerBinarySource> {
        match self {
            Self::Gvisor { worker_binary, .. } => worker_binary.as_ref(),
            _ => None,
        }
    }

    // ==================== Firecracker Accessors ====================

    /// Accessor: Firecracker binary path.
    pub fn firecracker_binary(&self) -> Option<&str> {
        match self {
            Self::Firecracker {
                firecracker_binary,
                ..
            } => firecracker_binary.as_deref(),
            _ => None,
        }
    }

    /// Accessor: Firecracker kernel image path.
    pub fn firecracker_kernel(&self) -> Option<&str> {
        match self {
            Self::Firecracker { kernel, .. } => kernel.as_deref(),
            _ => None,
        }
    }

    /// Accessor: Firecracker rootfs image path.
    pub fn firecracker_rootfs(&self) -> Option<&str> {
        match self {
            Self::Firecracker { rootfs, .. } => rootfs.as_deref(),
            _ => None,
        }
    }

    /// Accessor: Firecracker worker binary source.
    pub fn firecracker_worker_binary(&self) -> Option<&WorkerBinarySource> {
        match self {
            Self::Firecracker {
                worker_binary, ..
            } => worker_binary.as_ref(),
            _ => None,
        }
    }

    /// Accessor: Firecracker boot arguments.
    pub fn firecracker_boot_args(&self) -> Option<&str> {
        match self {
            Self::Firecracker { boot_args, .. } => boot_args.as_deref(),
            _ => None,
        }
    }

    // ==================== Wasm Accessors ====================

    /// Accessor: WASM runtime configuration.
    pub fn wasm_runtime(&self) -> Option<&WasmRuntime> {
        match self {
            Self::Wasm { runtime, .. } => Some(runtime),
            _ => None,
        }
    }

    /// Accessor: WASM worker binary source.
    pub fn wasm_worker_binary(&self) -> Option<&WorkerBinarySource> {
        match self {
            Self::Wasm { worker_binary, .. } => worker_binary.as_ref(),
            _ => None,
        }
    }

    // ==================== Local Accessors ====================

    /// Accessor: Local workspace directory.
    pub fn local_workspace_dir(&self) -> Option<&str> {
        match self {
            Self::Local { workspace_dir } => Some(workspace_dir),
            _ => None,
        }
    }

    // ==================== Kubernetes Accessors ====================

    /// Accessor: Kubernetes API server endpoint.
    pub fn kubernetes_cluster_endpoint(&self) -> Option<&str> {
        match self {
            Self::Kubernetes {
                cluster_endpoint, ..
            } => cluster_endpoint.as_deref(),
            _ => None,
        }
    }

    /// Accessor: Kubernetes namespace.
    pub fn kubernetes_namespace(&self) -> Option<&str> {
        match self {
            Self::Kubernetes { namespace, .. } => namespace.as_deref(),
            _ => None,
        }
    }

    /// Accessor: Kubernetes credentials.
    pub fn kubernetes_credentials(&self) -> Option<&KubernetesCredentials> {
        match self {
            Self::Kubernetes { credentials, .. } => credentials.as_ref(),
            _ => None,
        }
    }

    /// Accessor: Kubernetes default volume snapshot class.
    pub fn kubernetes_default_volume_snapshot_class(&self) -> Option<&str> {
        match self {
            Self::Kubernetes {
                default_volume_snapshot_class,
                ..
            } => default_volume_snapshot_class.as_deref(),
            _ => None,
        }
    }

    // ==================== Lambda Accessors ====================

    /// Accessor: Lambda region.
    pub fn lambda_region(&self) -> Option<&str> {
        match self {
            Self::Lambda { region, .. } => Some(region),
            _ => None,
        }
    }

    /// Accessor: Lambda credentials.
    pub fn lambda_credentials(&self) -> Option<&AwsCredentials> {
        match self {
            Self::Lambda { credentials, .. } => credentials.as_ref(),
            _ => None,
        }
    }

    // ==================== Validation Helpers ====================

    /// Check if Local config has a non-empty workspace directory.
    pub fn is_valid_local(&self) -> bool {
        match self {
            Self::Local { workspace_dir } => !workspace_dir.trim().is_empty(),
            _ => false,
        }
    }

    /// Check if Lambda config has a non-empty region.
    pub fn is_valid_lambda(&self) -> bool {
        match self {
            Self::Lambda { region, .. } => !region.trim().is_empty(),
            _ => false,
        }
    }
}

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
        assert_eq!(creds.role_arn(), Some("arn:aws:iam::123456789:role/my-role"));
        assert!(creds.is_valid());
    }

    #[test]
    fn test_aws_credentials_static() {
        let creds = AwsCredentials::static_creds("AKIAIOSFODNN7EXAMPLE", "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY")
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
        assert!(matches!(config, ProviderInstanceConfig::Podman { .. }));
        assert!(config.podman_socket().is_none());
        assert!(config.podman_image().is_none());
        assert!(config.podman_mounts().is_empty());
    }

    #[test]
    fn test_provider_instance_config_docker() {
        let config = ProviderInstanceConfig::docker();
        assert!(matches!(config, ProviderInstanceConfig::Docker { .. }));
        assert!(config.docker_socket().is_none());
    }

    #[test]
    fn test_provider_instance_config_wasm() {
        let config = ProviderInstanceConfig::wasm();
        assert!(matches!(config, ProviderInstanceConfig::Wasm { .. }));
        assert!(config.wasm_runtime().is_some());
        assert!(config.wasm_worker_binary().is_none());
    }

    #[test]
    fn test_provider_instance_config_local_valid() {
        let config = ProviderInstanceConfig::local("/tmp/workspace")
            .expect("valid workspace dir");
        assert!(matches!(config, ProviderInstanceConfig::Local { .. }));
        assert_eq!(config.local_workspace_dir(), Some("/tmp/workspace"));
        assert!(config.is_valid_local());
    }

    #[test]
    fn test_provider_instance_config_local_empty() {
        let err = ProviderInstanceConfig::local("")
            .expect_err("empty workspace should fail");
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn test_provider_instance_config_lambda_valid() {
        let config = ProviderInstanceConfig::lambda("us-east-1")
            .expect("valid region");
        assert!(matches!(config, ProviderInstanceConfig::Lambda { .. }));
        assert_eq!(config.lambda_region(), Some("us-east-1"));
        assert!(config.is_valid_lambda());
    }

    #[test]
    fn test_provider_instance_config_lambda_empty() {
        let err = ProviderInstanceConfig::lambda("")
            .expect_err("empty region should fail");
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn test_provider_instance_config_gvisor() {
        let config = ProviderInstanceConfig::gvisor();
        assert!(matches!(config, ProviderInstanceConfig::Gvisor { .. }));
        assert!(config.gvisor_runsc_binary().is_none());
    }

    #[test]
    fn test_provider_instance_config_firecracker() {
        let config = ProviderInstanceConfig::firecracker();
        assert!(matches!(config, ProviderInstanceConfig::Firecracker { .. }));
        assert!(config.firecracker_binary().is_none());
    }

    #[test]
    fn test_provider_instance_config_kubernetes() {
        let config = ProviderInstanceConfig::kubernetes();
        assert!(matches!(config, ProviderInstanceConfig::Kubernetes { .. }));
        assert!(config.kubernetes_credentials().is_none());
    }

    #[test]
    fn test_provider_instance_config_serde_podman() {
        let config = ProviderInstanceConfig::podman();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"type_id\":\"podman\""));
        let parsed: ProviderInstanceConfig = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ProviderInstanceConfig::Podman { .. }));
    }

    #[test]
    fn test_provider_instance_config_serde_wasm() {
        let config = ProviderInstanceConfig::wasm();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"type_id\":\"wasm\""));
        let parsed: ProviderInstanceConfig = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ProviderInstanceConfig::Wasm { .. }));
    }

    #[test]
    fn test_provider_instance_config_serde_local() {
        let config = ProviderInstanceConfig::local("/workspace")
            .expect("valid workspace");
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"type_id\":\"local\""));
        let parsed: ProviderInstanceConfig = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ProviderInstanceConfig::Local { .. }));
    }

    #[test]
    fn test_provider_instance_config_wrong_accessors_return_none() {
        let config = ProviderInstanceConfig::podman();
        // Accessors for other variants should return None
        assert!(config.docker_socket().is_none());
        assert!(config.local_workspace_dir().is_none());
        assert!(config.wasm_runtime().is_none());
        assert!(config.lambda_region().is_none());
    }
}
