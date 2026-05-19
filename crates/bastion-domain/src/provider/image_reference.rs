//! Image reference types for container images.
//!
//! Describes container images and how to pull them.

use serde::{Deserialize, Serialize};

/// Policy for when to pull container images.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImagePullPolicy {
    /// Always pull the image, even if it exists locally.
    Always,
    /// Only pull if the image is not present locally.
    #[default]
    IfNotPresent,
    /// Never pull; only use local images.
    Never,
}

impl std::fmt::Display for ImagePullPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Always => write!(f, "always"),
            Self::IfNotPresent => write!(f, "if_not_present"),
            Self::Never => write!(f, "never"),
        }
    }
}

/// Kind of secret value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretValueKind {
    /// Plain text secret.
    PlainText,
    /// Base64-encoded secret.
    Base64,
    /// Reference to a file containing the secret.
    File,
}

/// A secret value with its kind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretValue {
    /// The kind of secret.
    pub kind: SecretValueKind,
    /// The secret value.
    pub value: String,
}

impl SecretValue {
    /// Create a plain text secret.
    pub fn plain_text(value: impl Into<String>) -> Self {
        Self {
            kind: SecretValueKind::PlainText,
            value: value.into(),
        }
    }

    /// Create a base64-encoded secret.
    pub fn base64(value: impl Into<String>) -> Self {
        Self {
            kind: SecretValueKind::Base64,
            value: value.into(),
        }
    }

    /// Create a file-based secret reference.
    pub fn file(path: impl Into<String>) -> Self {
        Self {
            kind: SecretValueKind::File,
            value: path.into(),
        }
    }
}

/// Platform specification for multi-architecture images.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Platform {
    /// Operating system (e.g., "linux", "windows").
    pub os: String,
    /// Architecture (e.g., "amd64", "arm64").
    pub arch: String,
    /// Optional variant (e.g., "v8" for arm64).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

impl Platform {
    /// Create a new platform specification.
    pub fn new(os: impl Into<String>, arch: impl Into<String>) -> Self {
        Self {
            os: os.into(),
            arch: arch.into(),
            variant: None,
        }
    }

    /// Create a Linux AMD64 platform.
    pub fn linux_amd64() -> Self {
        Self {
            os: "linux".to_string(),
            arch: "amd64".to_string(),
            variant: None,
        }
    }

    /// Create a Linux ARM64 platform.
    pub fn linux_arm64() -> Self {
        Self {
            os: "linux".to_string(),
            arch: "arm64".to_string(),
            variant: None,
        }
    }

    /// Set the variant for this platform.
    pub fn with_variant(mut self, variant: Option<String>) -> Self {
        self.variant = variant;
        self
    }
}

/// Registry credentials for pulling private images.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryCredentials {
    /// Username for authentication.
    pub username: String,
    /// Password or token.
    pub password: SecretValue,
    /// Optional registry URL (defaults to Docker Hub).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
}

impl RegistryCredentials {
    /// Create new registry credentials.
    pub fn new(username: impl Into<String>, password: SecretValue) -> Self {
        Self {
            username: username.into(),
            password,
            registry: None,
        }
    }

    /// Create new registry credentials with a registry URL.
    pub fn with_registry(
        username: impl Into<String>,
        password: SecretValue,
        registry: impl Into<String>,
    ) -> Self {
        Self {
            username: username.into(),
            password,
            registry: Some(registry.into()),
        }
    }
}

/// Reference to a container image.
///
/// Used to specify which image to use for a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageReference {
    /// The image reference string (e.g., "docker.io/library/debian:bookworm-slim").
    pub reference: String,
    /// When to pull the image.
    #[serde(default)]
    pub pull_policy: ImagePullPolicy,
    /// Optional credentials for private registries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<RegistryCredentials>,
    /// Optional platform specification for multi-architecture images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<Platform>,
}

impl ImageReference {
    /// Create a new image reference.
    pub fn new(reference: impl Into<String>) -> Self {
        Self {
            reference: reference.into(),
            pull_policy: ImagePullPolicy::default(),
            credentials: None,
            platform: None,
        }
    }

    /// Create an image reference with pull policy.
    pub fn with_pull_policy(mut self, policy: ImagePullPolicy) -> Self {
        self.pull_policy = policy;
        self
    }

    /// Create an image reference with credentials.
    pub fn with_credentials(mut self, credentials: RegistryCredentials) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Create an image reference with platform.
    pub fn with_platform(mut self, platform: Platform) -> Self {
        self.platform = Some(platform);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_pull_policy_default() {
        assert_eq!(ImagePullPolicy::default(), ImagePullPolicy::IfNotPresent);
    }

    #[test]
    fn test_image_pull_policy_display() {
        assert_eq!(format!("{}", ImagePullPolicy::Always), "always");
        assert_eq!(format!("{}", ImagePullPolicy::IfNotPresent), "if_not_present");
        assert_eq!(format!("{}", ImagePullPolicy::Never), "never");
    }

    #[test]
    fn test_image_pull_policy_serde() {
        let policy = ImagePullPolicy::Always;
        let json = serde_json::to_string(&policy).unwrap();
        assert_eq!(json, "\"always\"");
        let parsed: ImagePullPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, policy);
    }

    #[test]
    fn test_secret_value_plain_text() {
        let secret = SecretValue::plain_text("my-password");
        assert!(matches!(secret.kind, SecretValueKind::PlainText));
        assert_eq!(secret.value, "my-password");
    }

    #[test]
    fn test_secret_value_base64() {
        let secret = SecretValue::base64("bXktcGFzc3dvcmQ=");
        assert!(matches!(secret.kind, SecretValueKind::Base64));
    }

    #[test]
    fn test_secret_value_file() {
        let secret = SecretValue::file("/run/secrets/my-secret");
        assert!(matches!(secret.kind, SecretValueKind::File));
        assert_eq!(secret.value, "/run/secrets/my-secret");
    }

    #[test]
    fn test_platform_linux_amd64() {
        let platform = Platform::linux_amd64();
        assert_eq!(platform.os, "linux");
        assert_eq!(platform.arch, "amd64");
        assert!(platform.variant.is_none());
    }

    #[test]
    fn test_platform_linux_arm64() {
        let platform = Platform::linux_arm64();
        assert_eq!(platform.os, "linux");
        assert_eq!(platform.arch, "arm64");
        assert!(platform.variant.is_none());
    }

    #[test]
    fn test_platform_with_variant() {
        let platform = Platform::new("linux", "arm64").with_variant(Some("v8".to_string()));
        assert_eq!(platform.variant, Some("v8".to_string()));
    }

    #[test]
    fn test_registry_credentials_new() {
        let creds = RegistryCredentials::new(
            "admin",
            SecretValue::plain_text("secret"),
        );
        assert_eq!(creds.username, "admin");
        assert!(creds.registry.is_none());
    }

    #[test]
    fn test_registry_credentials_with_registry() {
        let creds = RegistryCredentials::with_registry(
            "admin",
            SecretValue::plain_text("secret"),
            "ghcr.io",
        );
        assert_eq!(creds.registry, Some("ghcr.io".to_string()));
    }

    #[test]
    fn test_image_reference_new() {
        let image = ImageReference::new("debian:bookworm-slim");
        assert_eq!(image.reference, "debian:bookworm-slim");
        assert_eq!(image.pull_policy, ImagePullPolicy::IfNotPresent);
        assert!(image.credentials.is_none());
        assert!(image.platform.is_none());
    }

    #[test]
    fn test_image_reference_with_pull_policy() {
        let image = ImageReference::new("debian:bookworm-slim")
            .with_pull_policy(ImagePullPolicy::Always);
        assert_eq!(image.pull_policy, ImagePullPolicy::Always);
    }

    #[test]
    fn test_image_reference_serde() {
        let image = ImageReference::new("docker.io/library/nginx:latest");
        let json = serde_json::to_string(&image).unwrap();
        let parsed: ImageReference = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.reference, "docker.io/library/nginx:latest");
    }
}
