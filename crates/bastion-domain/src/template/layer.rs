//! FaaS Layer artifact types — zip-based layers for FaaS and sandboxes.
//!
//! Layers are immutable, versioned zip packages that provide capabilities.
//! They are the FaaS equivalent of TemplateArtifacts.

use serde::{Deserialize, Serialize};

use super::artifact::{ArtifactSecurityMetadata, PreparedEnvironmentSpec, TemplateArtifact};

/// Maximum number of layers per function (AWS Lambda limit).
pub const MAX_LAYERS_PER_FUNCTION: usize = 5;

/// Standard mount path for layers inside execution environments.
pub const LAYER_MOUNT_PREFIX: &str = "/opt/bastion/layers";

/// A FaaS-compatible layer artifact.
///
/// Layers are immutable zip packages versioned by ARN-like identifiers.
/// They are mounted at `/opt/bastion/layers/<name>/` in the execution environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerArtifact {
    /// Unique layer identifier (e.g., "bastion:jvm-build").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Immutable version number.
    pub version: u32,
    /// ARN-like reference (e.g., "arn:bastion:layer:jvm-build:1").
    pub arn: String,
    /// The underlying template artifact this layer wraps.
    #[serde(flatten)]
    pub template: TemplateArtifact,
    /// Layer description for documentation.
    pub description: Option<String>,
    /// Compatible runtimes for this layer.
    #[serde(default)]
    pub compatible_runtimes: Vec<String>,
    /// License information.
    pub license: Option<String>,
}

impl LayerArtifact {
    /// Create a new layer from a template artifact.
    pub fn new(template: TemplateArtifact, description: Option<String>) -> Self {
        let name = template.name.clone();
        let id = format!("layer:{}", template.name.replace('/', "-"));
        let version = 1;
        let arn = format!("arn:bastion:layer:{}:{}", template.name.replace('/', "-"), version);

        Self {
            id,
            name,
            version,
            arn,
            template,
            description,
            compatible_runtimes: Vec::new(),
            license: None,
        }
    }

    /// Mount path inside the execution environment.
    pub fn mount_path(&self) -> String {
        format!("{}/{}", LAYER_MOUNT_PREFIX, self.template.digest)
    }

    /// Get the environment specification for this layer.
    pub fn env_spec(&self) -> &PreparedEnvironmentSpec {
        &self.template.env
    }

    /// Get the security metadata.
    pub fn security(&self) -> &ArtifactSecurityMetadata {
        &self.template.security
    }
}

/// A set of layers attached to a function/sandbox.
#[derive(Debug, Clone, Default)]
pub struct LayerStack {
    layers: Vec<LayerArtifact>,
}

impl LayerStack {
    pub fn new() -> Self {
        Self { layers: Vec::new() }
    }

    /// Add a layer to the stack.
    pub fn add(&mut self, layer: LayerArtifact) -> Result<(), LayerStackError> {
        if self.layers.len() >= MAX_LAYERS_PER_FUNCTION {
            return Err(LayerStackError::TooManyLayers);
        }
        // Check for duplicate layer names
        if self.layers.iter().any(|l| l.name == layer.name) {
            return Err(LayerStackError::DuplicateLayer(layer.name));
        }
        self.layers.push(layer);
        Ok(())
    }

    /// Remove a layer by name.
    pub fn remove(&mut self, name: &str) -> Option<LayerArtifact> {
        if let Some(pos) = self.layers.iter().position(|l| l.name == name) {
            Some(self.layers.remove(pos))
        } else {
            None
        }
    }

    /// Get all layers.
    pub fn layers(&self) -> &[LayerArtifact] {
        &self.layers
    }

    /// Number of layers.
    pub fn len(&self) -> usize {
        self.layers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// Build the combined PATH from all layers.
    pub fn combined_path_prefix(&self) -> Vec<String> {
        self.layers
            .iter()
            .flat_map(|l| l.template.env.path_prefix.clone())
            .collect()
    }

    /// Build combined env vars from all layers.
    pub fn combined_env(&self) -> std::collections::HashMap<String, String> {
        let mut env = std::collections::HashMap::new();
        for layer in &self.layers {
            for (k, v) in &layer.template.env.env {
                env.insert(k.clone(), v.clone());
            }
        }
        env
    }
}

/// Errors for layer stack operations.
#[derive(Debug, thiserror::Error)]
pub enum LayerStackError {
    #[error("Maximum of {MAX_LAYERS_PER_FUNCTION} layers exceeded")]
    TooManyLayers,

    #[error("Layer '{0}' already exists in the stack")]
    DuplicateLayer(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::artifact::{ArtifactMediaType, CapabilityDescriptor, TemplateArtifact, ToolDescriptor, VerificationStep};

    fn make_jvm_layer() -> LayerArtifact {
        let template = TemplateArtifact::builder("bastion/jvm-build", "v1")
            .media_type(ArtifactMediaType::LambdaLayerZip)
            .digest("sha256:jvm-layer-001")
            .env_var("JAVA_HOME", "/opt/bastion/layers/sha256:jvm-layer-001")
            .path_prefix("/opt/bastion/layers/sha256:jvm-layer-001/bin")
            .add_capability(CapabilityDescriptor {
                name: "jvm-build".into(),
                tools: vec![
                    ToolDescriptor { name: "java".into(), version: "17".into() },
                ],
                verification: vec![],
            })
            .build();
        LayerArtifact::new(template, Some("JVM build tools for Java 17".into()))
    }

    #[test]
    fn test_layer_stack_basic() {
        let mut stack = LayerStack::new();
        let layer = make_jvm_layer();
        stack.add(layer).unwrap();
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn test_layer_stack_max_layers() {
        let mut stack = LayerStack::new();
        for i in 0..MAX_LAYERS_PER_FUNCTION {
            let template = TemplateArtifact::builder(format!("test/layer-{}", i), "v1")
                .digest(format!("sha256:layer-{}", i))
                .build();
            stack.add(LayerArtifact::new(template, None)).unwrap();
        }
        assert_eq!(stack.len(), 5);

        // Adding 6th should fail
        let extra = TemplateArtifact::builder("test/layer-extra", "v1").build();
        assert!(stack.add(LayerArtifact::new(extra, None)).is_err());
    }

    #[test]
    fn test_layer_arn_format() {
        let layer = make_jvm_layer();
        assert!(layer.arn.starts_with("arn:bastion:layer:"));
        assert!(layer.arn.contains("jvm-build"));
    }

    #[test]
    fn test_layer_mount_path() {
        let layer = make_jvm_layer();
        assert!(layer.mount_path().starts_with("/opt/bastion/layers/"));
    }
}
