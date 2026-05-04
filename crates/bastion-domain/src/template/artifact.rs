//! Template artifact domain types.
//!
//! These are pure domain objects with no infrastructure dependencies.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Unique identifier for a template artifact.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtifactId(pub String);

impl ArtifactId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for ArtifactId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Represents how the artifact is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactMediaType {
    /// OCI image (for container providers).
    OciImage,
    /// OCI filesystem layer (content-addressed, reusable).
    OciLayer,
    /// Generic OCI artifact (non-image content).
    OciArtifact,
    /// Plain tarball of root filesystem.
    RootfsTar,
    /// Standard disk image (qcow2, raw).
    VmDisk,
    /// Firecracker microVM snapshot (memory + state).
    MicroVmSnapshot,
    /// AWS Lambda layer (.zip).
    LambdaLayerZip,
    /// WASM module.
    WasmModule,
    /// Catch-all for custom provider formats.
    Custom(String),
}

/// A tool provided as part of a capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub version: String,
}

/// A verification step to run after materializing a capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationStep {
    /// Human-readable label (e.g. "java -version").
    pub label: String,
    /// The command to run for verification.
    pub command: String,
    /// Expected exit code (0 for success).
    pub expected_exit_code: i32,
    /// Substring that must appear in stdout/stderr.
    pub expected_output_contains: Option<String>,
}

/// A capability provided by a template artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    /// Stable capability name (e.g. "jvm-build", "node-build").
    pub name: String,
    /// Tools included in this capability.
    pub tools: Vec<ToolDescriptor>,
    /// Verification steps to confirm capability is functional.
    pub verification: Vec<VerificationStep>,
}

/// Prepared environment specification.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PreparedEnvironmentSpec {
    /// Environment variables to set.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Path prefix to prepend to PATH.
    #[serde(default)]
    pub path_prefix: Vec<String>,
}

/// Security metadata for template artifacts.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArtifactSecurityMetadata {
    /// Content digest (sha256:...).
    pub digest: Option<String>,
    /// Cryptographic signature (optional).
    pub signature: Option<String>,
    /// Reference to Software Bill of Materials.
    pub sbom_ref: Option<String>,
    /// Reference to build provenance.
    pub provenance_ref: Option<String>,
    /// Whether the artifact should be mounted readonly.
    pub readonly: bool,
    /// Allowed network hosts when using this artifact.
    #[serde(default)]
    pub allowed_network: Vec<String>,
    /// Allowed write paths when using this artifact.
    #[serde(default)]
    pub allowed_writes: Vec<String>,
    /// Whether this artifact contains secrets (must NOT be shared).
    pub contains_secrets: bool,
}

/// A template artifact: versioned, verifiable, capability-providing artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateArtifact {
    /// Unique identifier.
    pub id: ArtifactId,
    /// Human-readable name (e.g. "bastion/jvm-build").
    pub name: String,
    /// Version string.
    pub version: String,
    /// Content digest for verification.
    pub digest: String,
    /// How this artifact is stored.
    pub media_type: ArtifactMediaType,
    /// Capabilities provided by this artifact.
    #[serde(default)]
    pub capabilities: Vec<CapabilityDescriptor>,
    /// Prepared environment specification.
    #[serde(default)]
    pub env: PreparedEnvironmentSpec,
    /// Security metadata.
    #[serde(default)]
    pub security: ArtifactSecurityMetadata,
    /// Optional provider hints for materialization.
    #[serde(default)]
    pub provider_hints: HashMap<String, String>,
}

impl TemplateArtifact {
    /// Create a new template artifact builder.
    pub fn builder(name: impl Into<String>, version: impl Into<String>) -> TemplateArtifactBuilder {
        TemplateArtifactBuilder::new(name, version)
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

pub struct TemplateArtifactBuilder {
    id: Option<ArtifactId>,
    name: String,
    version: String,
    digest: String,
    media_type: ArtifactMediaType,
    capabilities: Vec<CapabilityDescriptor>,
    env: PreparedEnvironmentSpec,
    security: ArtifactSecurityMetadata,
    provider_hints: HashMap<String, String>,
}

impl TemplateArtifactBuilder {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        let name = name.into();
        let version = version.into();
        let digest = format!("sha256:{}", &version); // placeholder, real impl uses content hash

        Self {
            id: None,
            name,
            version: version.clone(),
            digest,
            media_type: ArtifactMediaType::RootfsTar,
            capabilities: Vec::new(),
            env: PreparedEnvironmentSpec::default(),
            security: ArtifactSecurityMetadata::default(),
            provider_hints: HashMap::new(),
        }
    }

    pub fn id(mut self, id: impl Into<ArtifactId>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn digest(mut self, digest: impl Into<String>) -> Self {
        self.digest = digest.into();
        self
    }

    pub fn media_type(mut self, mt: ArtifactMediaType) -> Self {
        self.media_type = mt;
        self
    }

    pub fn add_capability(mut self, cap: CapabilityDescriptor) -> Self {
        self.capabilities.push(cap);
        self
    }

    pub fn env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.env.insert(key.into(), value.into());
        self
    }

    pub fn path_prefix(mut self, path: impl Into<String>) -> Self {
        self.env.path_prefix.push(path.into());
        self
    }

    pub fn readonly(mut self, readonly: bool) -> Self {
        self.security.readonly = readonly;
        self
    }

    pub fn build(self) -> TemplateArtifact {
        TemplateArtifact {
            id: self.id.unwrap_or_else(|| {
                ArtifactId(format!("{}-{}", self.name.replace('/', "-"), self.version))
            }),
            name: self.name,
            version: self.version,
            digest: self.digest,
            media_type: self.media_type,
            capabilities: self.capabilities,
            env: self.env,
            security: self.security,
            provider_hints: self.provider_hints,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_jvm_template() {
        let artifact = TemplateArtifact::builder("bastion/jvm-build", "java17-maven3.9-v1")
            .media_type(ArtifactMediaType::OciArtifact)
            .digest("sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
            .env_var("JAVA_HOME", "/opt/bastion/toolchains/jvm/java17")
            .path_prefix("/opt/bastion/toolchains/jvm/java17/bin")
            .path_prefix("/opt/bastion/toolchains/maven/bin")
            .readonly(true)
            .add_capability(CapabilityDescriptor {
                name: "jvm-build".into(),
                tools: vec![
                    ToolDescriptor { name: "java".into(), version: "17".into() },
                    ToolDescriptor { name: "maven".into(), version: "3.9".into() },
                    ToolDescriptor { name: "git".into(), version: "any".into() },
                ],
                verification: vec![
                    VerificationStep {
                        label: "java -version".into(),
                        command: "java -version".into(),
                        expected_exit_code: 0,
                        expected_output_contains: Some("openjdk".into()),
                    },
                    VerificationStep {
                        label: "mvn -version".into(),
                        command: "mvn -version".into(),
                        expected_exit_code: 0,
                        expected_output_contains: Some("Apache Maven".into()),
                    },
                ],
            })
            .build();

        assert_eq!(artifact.name, "bastion/jvm-build");
        assert_eq!(artifact.version, "java17-maven3.9-v1");
        assert_eq!(artifact.capabilities.len(), 1);
        assert_eq!(artifact.capabilities[0].name, "jvm-build");
        assert!(artifact.security.readonly);
    }
}
