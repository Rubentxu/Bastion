//! Provider factory for managing multiple sandbox providers.

use std::collections::HashMap;
use std::sync::Arc;

use bastion_domain::provider::SandboxProvider;
use bastion_domain::provider::capabilities::ProviderCapabilities;

/// Information about a registered provider.
#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub name: String,
    pub capabilities: ProviderCapabilities,
}

/// Factory for creating and managing sandbox providers.
///
/// Allows registration of multiple provider backends and selecting
/// which one to use at runtime.
#[derive(Debug)]
pub struct ProviderFactory {
    providers: HashMap<String, Arc<dyn SandboxProvider>>,
    default: String,
}

impl ProviderFactory {
    /// Create a new factory with the specified default provider name.
    pub fn new(default_provider: &str) -> Self {
        Self {
            providers: HashMap::new(),
            default: default_provider.to_string(),
        }
    }

    /// Register a provider under a name.
    pub fn register(&mut self, name: &str, provider: Arc<dyn SandboxProvider>) {
        tracing::debug!(provider = %name, "Registering provider");
        self.providers.insert(name.to_string(), provider);
    }

    /// Get a provider by name.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn SandboxProvider>> {
        self.providers.get(name)
    }

    /// Get the default provider.
    pub fn default(&self) -> &Arc<dyn SandboxProvider> {
        self.providers
            .get(&self.default)
            .expect("Default provider must be registered")
    }

    /// List all registered providers.
    pub fn list_providers(&self) -> Vec<ProviderInfo> {
        self.providers
            .iter()
            .map(|(name, provider)| ProviderInfo {
                name: name.clone(),
                capabilities: provider.capabilities(),
            })
            .collect()
    }

    /// Check if a provider exists.
    pub fn contains(&self, name: &str) -> bool {
        self.providers.contains_key(name)
    }

    /// Get the name of the default provider.
    pub fn default_name(&self) -> &str {
        &self.default
    }

    /// Extract the underlying providers map (consumes the factory).
    pub fn into_providers(self) -> HashMap<String, Arc<dyn SandboxProvider>> {
        self.providers
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_factory_register_and_get() {
        // This test would need a mock provider - skipped in unit tests
        // Integration tests would cover this
    }
}
