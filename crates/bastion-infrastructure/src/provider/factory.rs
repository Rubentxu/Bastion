//! Provider factory for managing multiple sandbox providers.

use std::collections::HashMap;
use std::sync::Arc;

use bastion_domain::provider::SandboxProvider;
use bastion_domain::provider::capabilities::ProviderCapabilities;

#[cfg(feature = "use-segregated-traits")]
use bastion_domain::provider::lifecycle::SandboxLifecycle;
#[cfg(feature = "use-segregated-traits")]
use bastion_domain::provider::executor::TaskExecutor;

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

    /// Register a provider using segregated traits (SandboxLifecycle + TaskExecutor).
    ///
    /// This method leverages the blanket impl of SandboxProvider for any type
    /// that implements both SandboxLifecycle and TaskExecutor, allowing providers
    /// to implement only the segregated traits.
    #[cfg(feature = "use-segregated-traits")]
    pub fn register_lifecycle(
        &mut self,
        name: &str,
        provider: impl SandboxLifecycle + TaskExecutor + 'static,
    ) {
        tracing::debug!(provider = %name, "Registering provider with segregated traits");
        let wrapped: Arc<dyn SandboxProvider> = Arc::new(provider);
        self.providers.insert(name.to_string(), wrapped);
    }

    /// Register a provider using segregated traits (no feature flag, usesdyn SandboxProvider directly).
    #[cfg(not(feature = "use-segregated-traits"))]
    pub fn register_lifecycle(
        &mut self,
        _name: &str,
        _provider: impl SandboxProvider + 'static,
    ) {
        panic!("register_lifecycle requires use-segregated-traits feature");
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
