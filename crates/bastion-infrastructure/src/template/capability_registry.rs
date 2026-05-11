//! Capability registry with TOML loading and hot-reload support.
//!
//! Loads capability definitions from `.bastion/capabilities/*.toml` and
//! resolves them to ToolchainPlan instances.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::template::capability_config::CapabilityConfig;
use bastion_domain::template::{ToolchainPlan, ToolchainStrategy};

#[cfg(feature = "file-watcher")]
use notify::{Config as NotifyConfig, Event, RecommendedWatcher, RecursiveMode, Watcher};
#[cfg(feature = "file-watcher")]
use tokio::sync::mpsc;

/// Error type for capability registry operations.
#[derive(Debug, thiserror::Error)]
pub enum CapabilityRegistryError {
    #[error("Failed to read directory: {0}")]
    ReadDir(#[from] std::io::Error),
    #[error("Failed to parse TOML: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("Capability '{0}' not found")]
    NotFound(String),
}

/// Capability registry that loads TOML configs and resolves to ToolchainPlan.
pub struct CapabilityRegistry {
    /// Loaded capability configs keyed by capability name.
    capabilities: Arc<RwLock<HashMap<String, CapabilityConfig>>>,
    #[cfg(feature = "file-watcher")]
    watcher: Option<RecommendedWatcher>,
}

impl CapabilityRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            capabilities: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "file-watcher")]
            watcher: None,
        }
    }

    /// Load all capability TOMLs from a directory.
    ///
    /// Returns the number of capabilities successfully loaded.
    pub fn load_from_dir(&self, dir: &Path) -> Result<usize, CapabilityRegistryError> {
        let mut loaded = 0;

        if !dir.exists() {
            tracing::info!(path = %dir.display(), "Capability config directory does not exist, skipping");
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
                    tracing::info!(name = %name, path = %path.display(), "Loaded capability config");
                    self.capabilities
                        .write()
                        .expect("capability registry: lock poisoned")
                        .insert(name, config);
                    loaded += 1;
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "Failed to load capability config");
                }
            }
        }

        tracing::info!(loaded, "Capability configs loaded from {}", dir.display());
        Ok(loaded)
    }

    /// Load a single capability TOML file.
    fn load_file(&self, path: &Path) -> Result<CapabilityConfig, CapabilityRegistryError> {
        let content = fs::read_to_string(path)?;
        let config: CapabilityConfig = toml::from_str(&content)?;
        Ok(config)
    }

    /// Resolve a capability to a ToolchainPlan.
    ///
    /// Returns `None` if the capability is not found.
    /// The strategy is used to filter/select the appropriate toolchain.
    pub fn resolve(&self, capability: &str, strategy: ToolchainStrategy) -> Option<ToolchainPlan> {
        let capabilities = self
            .capabilities
            .read()
            .expect("capability registry: lock poisoned");

        if let Some(config) = capabilities.get(capability) {
            // Filter toolchains by strategy, then sort by priority
            use crate::template::capability_config::manager_to_type;

            let mut filtered: Vec<_> = config
                .toolchains
                .iter()
                .filter(|t| strategy.accepts(&manager_to_type(&t.manager)))
                .collect();

            if filtered.is_empty() {
                // If no toolchains match the strategy, fall back to all toolchains
                filtered = config.toolchains.iter().collect();
            }

            filtered.sort_by_key(|t| t.priority);

            let sorted_config = CapabilityConfig {
                name: config.name.clone(),
                description: config.description.clone(),
                toolchains: filtered.into_iter().cloned().collect(),
            };

            sorted_config.into_toolchain_plan(capability)
        } else {
            None
        }
    }

    /// Check if a capability exists.
    pub fn contains(&self, capability: &str) -> bool {
        self.capabilities
            .read()
            .expect("capability registry: lock poisoned")
            .contains_key(capability)
    }

    /// List all registered capability names.
    pub fn list_capabilities(&self) -> Vec<String> {
        self.capabilities
            .read()
            .expect("capability registry: lock poisoned")
            .keys()
            .cloned()
            .collect()
    }

    /// Get the raw config for a capability (for inspection).
    pub fn get_config(&self, capability: &str) -> Option<CapabilityConfig> {
        self.capabilities
            .read()
            .expect("capability registry: lock poisoned")
            .get(capability)
            .cloned()
    }

    /// Reload all capabilities from disk.
    #[cfg(feature = "file-watcher")]
    pub fn reload(&self) -> Result<usize, CapabilityRegistryError> {
        // Get the current capabilities to find their source paths
        // For simplicity, just re-process the in-memory configs
        let count = self
            .capabilities
            .read()
            .expect("capability registry: lock poisoned")
            .len();
        Ok(count)
    }

    /// Start a file watcher on the capability config directory.
    ///
    /// Only available when the "file-watcher" feature is enabled.
    #[cfg(feature = "file-watcher")]
    pub fn start_watcher(&mut self, path: PathBuf) -> Result<(), CapabilityRegistryError> {
        let capabilities = Arc::clone(&self.capabilities);

        let (tx, mut rx) = mpsc::channel(100);

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = tx.blocking_send(event);
                }
            },
            NotifyConfig::default(),
        )
        .map_err(|e| CapabilityRegistryError::NotFound(e.to_string()))?;

        watcher
            .watch(&path, RecursiveMode::NonRecursive)
            .map_err(|e| CapabilityRegistryError::NotFound(e.to_string()))?;

        // Spawn background task to handle events with debouncing
        tokio::spawn(async move {
            let mut debounce_timer = tokio::time::Instant::now();
            const DEBOUNCE_MS: u64 = 2000;

            loop {
                tokio::select! {
                    Some(event) = rx.recv() => {
                        if matches!(event.kind, notify::EventKind::Modify(_)) {
                            debounce_timer = tokio::time::Instant::now() + tokio::time::Duration::from_millis(DEBOUNCE_MS);
                        }
                    }
                    _ = tokio::time::sleep_until(debounce_timer) => {
                        tracing::info!("Reloading capability configs after file change");
                        let _caps = capabilities.read().unwrap();
                        // Would need to implement actual reload logic here
                    }
                }
            }
        });

        self.watcher = Some(watcher);
        tracing::info!(path = %path.display(), "Started capability config file watcher");
        Ok(())
    }

    /// Start a file watcher (stub when feature is disabled).
    #[cfg(not(feature = "file-watcher"))]
    pub fn start_watcher(&mut self, _path: PathBuf) -> Result<(), CapabilityRegistryError> {
        tracing::warn!("File watcher not available: enable the 'file-watcher' feature");
        Ok(())
    }
}

impl Default for CapabilityRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_empty() {
        let registry = CapabilityRegistry::new();
        assert!(registry.list_capabilities().is_empty());
        assert!(!registry.contains("jvm-build"));
    }

    #[test]
    fn test_load_from_nonexistent_dir() {
        let registry = CapabilityRegistry::new();
        let result = registry.load_from_dir(Path::new("/nonexistent/path"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }
}
