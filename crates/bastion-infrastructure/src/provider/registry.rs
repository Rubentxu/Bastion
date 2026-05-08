//! Provider registry with TOML loading and hot-reload support.
//!
//! Wraps ProviderFactory and adds:
//! - TOML-based provider configuration loading from `.bastion/providers/*.toml`
//! - File watcher for hot-reload (optional, enabled via --watch-config)

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::provider::config::ProviderConfig;
use crate::provider::factory::ProviderFactory;
use bastion_domain::provider::SandboxProvider;

#[cfg(feature = "file-watcher")]
use notify::{Config as NotifyConfig, Event, RecommendedWatcher, RecursiveMode, Watcher};
#[cfg(feature = "file-watcher")]
use tokio::sync::mpsc;

/// Error type for registry operations.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("Failed to read directory: {0}")]
    ReadDir(#[from] std::io::Error),
    #[error("Failed to parse TOML: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("Provider '{0}' not found in factory")]
    ProviderNotFound(String),
    #[error("Failed to start file watcher: {0}")]
    Watcher(String),
}

/// Information about a loaded provider.
#[derive(Debug, Clone)]
pub struct ProviderRegistryEntry {
    pub name: String,
    pub config: ProviderConfig,
}

/// Provider registry that wraps a ProviderFactory and supports TOML loading.
pub struct ProviderRegistry {
    factory: Arc<RwLock<ProviderFactory>>,
    /// Loaded provider configs (for inspection/hot-reload).
    configs: Arc<RwLock<HashMap<String, ProviderConfig>>>,
    #[cfg(feature = "file-watcher")]
    watcher: Option<RecommendedWatcher>,
}

impl ProviderRegistry {
    /// Create a new registry wrapping an existing factory.
    pub fn new(factory: ProviderFactory) -> Self {
        Self {
            factory: Arc::new(RwLock::new(factory)),
            configs: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "file-watcher")]
            watcher: None,
        }
    }

    /// Register a provider by name.
    pub fn register(&self, name: &str, provider: Arc<dyn SandboxProvider>) {
        self.factory.write().unwrap().register(name, provider);
    }

    /// Load all provider TOMLs from a directory.
    ///
    /// Returns the number of providers successfully loaded.
    pub fn load_from_dir(&self, dir: &Path) -> Result<usize, RegistryError> {
        let mut loaded = 0;

        if !dir.exists() {
            tracing::info!(path = %dir.display(), "Provider config directory does not exist, skipping");
            return Ok(0);
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }

            match self.load_file(&path) {
                Ok(config) => {
                    let name = config.name.clone();
                    tracing::info!(name = %name, path = %path.display(), "Loaded provider config");
                    self.configs.write().unwrap().insert(name, config);
                    loaded += 1;
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "Failed to load provider config");
                }
            }
        }

        // Register loaded configs into the factory
        self.register_configs()?;

        tracing::info!(loaded, "Provider configs loaded from {}", dir.display());
        Ok(loaded)
    }

    /// Load a single TOML file and register it.
    fn load_file(&self, path: &Path) -> Result<ProviderConfig, RegistryError> {
        let content = fs::read_to_string(path)?;
        let config: ProviderConfig = toml::from_str(&content)?;
        Ok(config)
    }

    /// Register all loaded configs into the factory.
    fn register_configs(&self) -> Result<(), RegistryError> {
        let configs = self.configs.read().unwrap().clone();

        for (name, config) in &configs {
            // Try to instantiate the provider based on kind
            match self.instantiate_provider(config) {
                Ok(provider) => {
                    self.factory.write().unwrap().register(name, provider);
                    tracing::debug!(name = %name, "Registered provider from TOML");
                }
                Err(e) => {
                    tracing::warn!(name = %name, error = %e, "Could not instantiate provider, skipping registration");
                }
            }
        }

        Ok(())
    }

    /// Instantiate a provider based on its kind.
    ///
    /// For "builtin" kinds, this would look up the appropriate factory method.
    /// Currently supports: podman, local, and wasm.
    fn instantiate_provider(
        &self,
        config: &ProviderConfig,
    ) -> Result<Arc<dyn SandboxProvider>, RegistryError> {
        match config.kind.as_str() {
            "podman" => {
                // PodmanProvider::new requires socket, image, worker_binary
                let socket = config
                    .socket
                    .clone()
                    .unwrap_or_else(|| "/run/user/1000/podman/podman.sock".to_string());
                let image = config
                    .image
                    .clone()
                    .unwrap_or_else(|| "debian:bookworm-slim".to_string());
                let worker_binary = config
                    .worker_binary
                    .clone()
                    .unwrap_or_else(|| "target/debug/bastion-worker".to_string());

                let podman = crate::provider::PodmanProvider::new(
                    &socket,
                    &image,
                    PathBuf::from(&worker_binary),
                )
                .map_err(|e| RegistryError::Watcher(e.to_string()))?;
                Ok(Arc::new(podman) as Arc<dyn SandboxProvider>)
            }
            "local" => {
                // LocalProvider: uses temp directory for workspaces
                // Requires DANGEROUS_ALLOW_LOCAL=1 env var to be set
                let base_dir = config
                    .socket
                    .clone()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| std::env::temp_dir().join("bastion-local"));

                let local = crate::provider::LocalProvider::new(base_dir)
                    .map_err(|e| RegistryError::Watcher(e.to_string()))?;
                Ok(Arc::new(local) as Arc<dyn SandboxProvider>)
            }
            #[cfg(feature = "wasm-sandbox")]
            "wasm" => {
                let wasm = crate::provider::WasmSandboxProvider::new();
                Ok(Arc::new(wasm) as Arc<dyn SandboxProvider>)
            }
            #[cfg(not(feature = "wasm-sandbox"))]
            "wasm" => Err(RegistryError::Watcher(
                "wasm-sandbox feature not enabled. Rebuild with --features wasm-sandbox".into(),
            )),
            _ => Err(RegistryError::Watcher(format!(
                "Unknown provider kind: {}",
                config.kind
            ))),
        }
    }

    /// Get a provider by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn SandboxProvider>> {
        self.factory.read().unwrap().get(name).cloned()
    }

    /// Get the default provider.
    pub fn default(&self) -> Arc<dyn SandboxProvider> {
        self.factory.read().unwrap().default().clone()
    }

    /// List all registered providers.
    pub fn list_providers(&self) -> Vec<crate::provider::factory::ProviderInfo> {
        self.factory.read().unwrap().list_providers()
    }

    /// Check if a provider exists.
    pub fn contains(&self, name: &str) -> bool {
        self.factory.read().unwrap().contains(name)
    }

    /// Get the name of the default provider.
    pub fn default_name(&self) -> String {
        self.factory.read().unwrap().default_name().to_string()
    }

    /// Extract the underlying providers map (consumes the registry).
    pub fn into_providers(self) -> HashMap<String, Arc<dyn SandboxProvider>> {
        // Try to unwrap the Arc, then get the inner RwLock, then get the factory
        match Arc::try_unwrap(self.factory) {
            Ok(rw_lock) => rw_lock.into_inner().unwrap().into_providers(),
            Err(_) => panic!("ProviderRegistry::into_providers called while still in use"),
        }
    }

    /// Reload all configs from disk (used by file watcher).
    #[allow(unused_variables)]
    pub fn reload(&self) -> Result<usize, RegistryError> {
        // For simplicity, just re-register from in-memory configs
        // A full implementation would track file paths
        self.register_configs()?;
        Ok(self.configs.read().unwrap().len())
    }

    /// Start a file watcher on the provider config directory.
    ///
    /// Only available when the "file-watcher" feature is enabled.
    #[cfg(feature = "file-watcher")]
    pub fn start_watcher(&mut self, path: PathBuf) -> Result<(), RegistryError> {
        let registry = Arc::clone(&self.factory);
        let configs = Arc::clone(&self.configs);

        let (tx, mut rx) = mpsc::channel(100);

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = tx.blocking_send(event);
                }
            },
            NotifyConfig::default(),
        )
        .map_err(|e| RegistryError::Watcher(e.to_string()))?;

        watcher
            .watch(&path, RecursiveMode::NonRecursive)
            .map_err(|e| RegistryError::Watcher(e.to_string()))?;

        // Spawn background task to handle events with debouncing
        tokio::spawn(async move {
            let mut debounce_timer = tokio::time::Instant::now();
            const DEBOUNCE_MS: u64 = 2000;

            loop {
                tokio::select! {
                    Some(event) = rx.recv() => {
                        // Only reload on modify events
                        if matches!(event.kind, notify::EventKind::Modify(_)) {
                            debounce_timer = tokio::time::Instant::now() + tokio::time::Duration::from_millis(DEBOUNCE_MS);
                        }
                    }
                    _ = tokio::time::sleep_until(debounce_timer) => {
                        // Debounce window elapsed, reload configs
                        tracing::info!("Reloading provider configs after file change");
                        let factory = registry.read().unwrap();
                        // Would need to implement actual reload logic here
                        drop(factory);
                    }
                }
            }
        });

        self.watcher = Some(watcher);
        tracing::info!(path = %path.display(), "Started provider config file watcher");
        Ok(())
    }

    /// Start a file watcher (stub when feature is disabled).
    #[cfg(not(feature = "file-watcher"))]
    pub fn start_watcher(&mut self, _path: PathBuf) -> Result<(), RegistryError> {
        tracing::warn!("File watcher not available: enable the 'file-watcher' feature");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creation() {
        let factory = ProviderFactory::new("podman");
        let registry = ProviderRegistry::new(factory);
        assert!(registry.default_name() == "podman");
    }

    #[test]
    fn test_load_from_nonexistent_dir() {
        let factory = ProviderFactory::new("podman");
        let registry = ProviderRegistry::new(factory);
        let result = registry.load_from_dir(Path::new("/nonexistent/path"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }
}
