//! TOML advice parser and registry.
//!
//! Loads `AdviceDescriptor` from TOML files in `.bastion/catalog/advice/`.
//! Manages advice configuration in `.bastion/advice.toml`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use bastion_domain::catalog::advice::{AdviceDescriptor, AdviceSeverity, AdviceTrigger};
use serde::{Deserialize, Serialize};

// ─── Advice config (domain-aligned) ────────────────────────────────────────────

/// Configuration for advice features, shared between application and infrastructure.
#[derive(Debug, Clone, Default)]
pub struct AdviceConfig {
    /// Global enable/disable flag.
    pub enabled: bool,
    /// List of disabled advice IDs.
    pub disabled: Vec<String>,
}

impl AdviceConfig {
    /// Check if a specific advice ID is disabled.
    pub fn is_disabled(&self, id: &str) -> bool {
        self.disabled.contains(&id.to_string())
    }

    /// Create default config (enabled, no disabled list).
    pub fn default_enabled() -> Self {
        Self {
            enabled: true,
            disabled: Vec::new(),
        }
    }
}

// ─── TOML structures ───────────────────────────────────────────────────────────

/// TOML configuration for an advice file, mirrors the TOML file structure.
#[derive(Debug, Deserialize)]
pub struct TomlAdviceConfig {
    pub advice: TomlAdvice,
}

/// TOML `[advice]` section.
#[derive(Debug, Deserialize)]
pub struct TomlAdvice {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default = "default_severity")]
    pub severity: AdviceSeverity,
    #[serde(default)]
    pub triggers: Vec<TomlTrigger>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub suggested_actions: Vec<String>,
    #[serde(default)]
    pub hint: Option<String>,
}

fn default_category() -> String {
    "general".to_string()
}

fn default_severity() -> AdviceSeverity {
    AdviceSeverity::Warning
}

/// A single trigger parsed from TOML.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TomlTrigger {
    /// Triggered when an assertion fails.
    AssertionFailed {
        /// The assertion ID that must fail.
        assertion_id: String,
    },
    /// Triggered when a doctor fails.
    DoctorFailed {
        /// The doctor ID that must fail.
        doctor_id: String,
    },
    /// Triggered by experience pattern.
    ExperiencePattern {
        /// The tool name to match.
        tool_name: String,
        /// The experience status to match.
        status: String,
        /// Minimum count to trigger.
        threshold: u32,
    },
}

impl TomlTrigger {
    /// Convert to a CEL condition string for the CEL-lite rules engine.
    ///
    /// Returns `None` for triggers that have no direct CEL equivalent.
    pub fn to_cel_condition(&self) -> Option<String> {
        match self {
            TomlTrigger::AssertionFailed { assertion_id } => {
                // fact('assertion:<id>') == "failed"
                Some(format!("fact('assertion:{}') == 'failed'", assertion_id))
            }
            TomlTrigger::DoctorFailed { doctor_id } => {
                // fact('doctor:<id>') == "failed"
                Some(format!("fact('doctor:{}') == 'failed'", doctor_id))
            }
            TomlTrigger::ExperiencePattern {
                tool_name,
                status,
                threshold,
            } => {
                // count_fact('experience:<tool_name>:<status>', '>=', <threshold>)
                Some(format!(
                    "count_fact('experience:{}:{}', '>=', {})",
                    tool_name, status, threshold
                ))
            }
        }
    }

    fn into_trigger(self) -> AdviceTrigger {
        match self {
            TomlTrigger::AssertionFailed { assertion_id } => {
                AdviceTrigger::AssertionFailed { assertion_id }
            }
            TomlTrigger::DoctorFailed { doctor_id } => AdviceTrigger::DoctorFailed { doctor_id },
            TomlTrigger::ExperiencePattern {
                tool_name,
                status,
                threshold,
            } => AdviceTrigger::ExperiencePattern {
                tool_name,
                status,
                threshold,
            },
        }
    }
}

/// Convert TomlTrigger to AdviceTrigger for legacy evaluation.
impl From<TomlTrigger> for AdviceTrigger {
    fn from(toml: TomlTrigger) -> Self {
        toml.into_trigger()
    }
}

/// TOML config into AdviceDescriptor.
impl From<TomlAdviceConfig> for AdviceDescriptor {
    fn from(config: TomlAdviceConfig) -> Self {
        let TomlAdviceConfig { advice } = config;
        let triggers = advice
            .triggers
            .into_iter()
            .map(|t| t.into_trigger())
            .collect();
        AdviceDescriptor {
            id: advice.id,
            name: advice.name,
            description: advice.description,
            category: advice.category,
            severity: advice.severity,
            triggers,
            message: advice.message,
            suggested_actions: advice.suggested_actions,
            hint: advice.hint,
        }
    }
}

// ─── Advice config file ────────────────────────────────────────────────────────

/// TOML configuration file for advice settings (`.bastion/advice.toml`).
#[derive(Debug, Deserialize, Serialize)]
pub struct TomlAdviceSettings {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub disabled: Vec<String>,
}

fn default_enabled() -> bool {
    true
}

impl Default for TomlAdviceSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            disabled: Vec::new(),
        }
    }
}

// ─── Error types ──────────────────────────────────────────────────────────────

/// Error type for advice parsing operations.
#[derive(Debug, thiserror::Error)]
pub enum AdviceParserError {
    #[error("Failed to read directory: {0}")]
    ReadDir(#[from] std::io::Error),
    #[error("Failed to parse TOML: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("Failed to serialize TOML: {0}")]
    TomlSerialize(String),
    #[error("Advice '{0}' not found")]
    NotFound(String),
}

// ─── Advice registry ──────────────────────────────────────────────────────────

/// Loads and manages advice descriptors from TOML files.
#[derive(Debug)]
pub struct AdviceRegistry {
    advice: Arc<RwLock<HashMap<String, AdviceDescriptor>>>,
    /// TOML source for each advice (for advice_get).
    sources: Arc<RwLock<HashMap<String, String>>>,
}

impl AdviceRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            advice: Arc::new(RwLock::new(HashMap::new())),
            sources: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load all advice TOMLs from a directory.
    ///
    /// Returns the number of advice items successfully loaded.
    /// Skips files that fail to parse with a warning.
    pub fn load_from_dir(&self, dir: &Path) -> Result<usize, AdviceParserError> {
        let mut loaded = 0;

        if !dir.exists() {
            tracing::info!(
                path = %dir.display(),
                "Advice catalog directory does not exist, skipping"
            );
            return Ok(0);
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }

            match self.load_file(&path) {
                Ok((adv, source)) => {
                    let advice_id = adv.id.clone();
                    tracing::info!(
                        id = %advice_id,
                        path = %path.display(),
                        "Loaded advice"
                    );

                    // Check for duplicates — last wins with a warning
                    {
                        let existing = {
                            let advice =
                                self.advice.read().expect("advice registry: lock poisoned");
                            advice.contains_key(&advice_id)
                        };
                        if existing {
                            tracing::warn!(
                                id = %advice_id,
                                path = %path.display(),
                                "Duplicate advice ID — previous descriptor overwritten"
                            );
                        }
                    }

                    {
                        let mut advice =
                            self.advice.write().expect("advice registry: lock poisoned");
                        advice.insert(advice_id.clone(), adv);
                    }
                    {
                        let mut sources = self
                            .sources
                            .write()
                            .expect("advice registry: lock poisoned");
                        sources.insert(advice_id, source);
                    }
                    loaded += 1;
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "Failed to load advice");
                }
            }
        }

        tracing::info!(loaded, "Advice loaded from {}", dir.display());
        Ok(loaded)
    }

    /// Load a single advice TOML file.
    fn load_file(&self, path: &Path) -> Result<(AdviceDescriptor, String), AdviceParserError> {
        let content = fs::read_to_string(path)?;
        let config: TomlAdviceConfig = toml::from_str(&content)?;
        let descriptor: AdviceDescriptor = config.into();
        Ok((descriptor, content))
    }

    /// Get an advice descriptor by ID.
    pub fn get(&self, id: &str) -> Option<AdviceDescriptor> {
        self.advice
            .read()
            .expect("advice registry: lock poisoned")
            .get(id)
            .cloned()
    }

    /// List all loaded advice descriptors.
    pub fn list(&self) -> Vec<AdviceDescriptor> {
        self.advice
            .read()
            .expect("advice registry: lock poisoned")
            .values()
            .cloned()
            .collect()
    }

    /// Get the TOML source for an advice descriptor.
    pub fn get_source(&self, id: &str) -> Option<String> {
        self.sources
            .read()
            .expect("advice registry: lock poisoned")
            .get(id)
            .cloned()
    }

    /// Number of loaded advice items.
    pub fn len(&self) -> usize {
        self.advice
            .read()
            .expect("advice registry: lock poisoned")
            .len()
    }

    /// Check if registry is empty.
    pub fn is_empty(&self) -> bool {
        self.advice
            .read()
            .expect("advice registry: lock poisoned")
            .is_empty()
    }
}

impl Default for AdviceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Advice config store ──────────────────────────────────────────────────────

/// Stores and manages advice configuration (`.bastion/advice.toml`).
#[derive(Debug)]
pub struct AdviceConfigStore {
    config_path: PathBuf,
    config: RwLock<AdviceConfig>,
}

impl AdviceConfigStore {
    /// Create a new config store at the given path.
    pub fn new(config_path: PathBuf) -> Self {
        let config = Self::load_config(&config_path).unwrap_or_else(|e| {
            tracing::info!(
                path = %config_path.display(),
                error = %e,
                "No advice config found, using defaults"
            );
            AdviceConfig::default_enabled()
        });
        Self {
            config_path,
            config: RwLock::new(config),
        }
    }

    /// Load config from file, returning defaults if not found.
    fn load_config(path: &Path) -> Result<AdviceConfig, AdviceParserError> {
        if !path.exists() {
            return Ok(AdviceConfig::default_enabled());
        }
        let content = fs::read_to_string(path)?;
        let settings: TomlAdviceSettings = toml::from_str(&content)?;
        Ok(AdviceConfig {
            enabled: settings.enabled,
            disabled: settings.disabled,
        })
    }

    /// Save config to file atomically.
    fn save_config(&self, config: &AdviceConfig) -> Result<(), AdviceParserError> {
        let settings = TomlAdviceSettings {
            enabled: config.enabled,
            disabled: config.disabled.clone(),
        };
        let content = toml::to_string_pretty(&settings)
            .map_err(|e| AdviceParserError::TomlSerialize(e.to_string()))?;

        // Atomic write: write to temp file then rename
        let temp_path = self.config_path.with_extension("toml.tmp");
        fs::write(&temp_path, &content)?;
        fs::rename(&temp_path, &self.config_path)?;

        tracing::info!(
            path = %self.config_path.display(),
            "Saved advice config"
        );
        Ok(())
    }

    /// Get a snapshot of the current config.
    pub fn get_config(&self) -> AdviceConfig {
        self.config
            .read()
            .expect("advice config: lock poisoned")
            .clone()
    }

    /// Set global enabled flag.
    pub fn set_global_enabled(&self, enabled: bool) -> Result<AdviceConfig, AdviceParserError> {
        let mut cfg = self.config.write().expect("advice config: lock poisoned");
        cfg.enabled = enabled;
        self.save_config(&cfg)?;
        Ok(cfg.clone())
    }

    /// Disable a specific advice ID.
    pub fn disable_advice(&self, id: &str) -> Result<AdviceConfig, AdviceParserError> {
        let mut cfg = self.config.write().expect("advice config: lock poisoned");
        if !cfg.disabled.contains(&id.to_string()) {
            cfg.disabled.push(id.to_string());
        }
        self.save_config(&cfg)?;
        Ok(cfg.clone())
    }

    /// Enable a specific advice ID (remove from disabled list).
    pub fn enable_advice(&self, id: &str) -> Result<AdviceConfig, AdviceParserError> {
        let mut cfg = self.config.write().expect("advice config: lock poisoned");
        cfg.disabled.retain(|d| d != id);
        self.save_config(&cfg)?;
        Ok(cfg.clone())
    }

    /// Clear all disabled advice IDs.
    pub fn clear_disabled(&self) -> Result<AdviceConfig, AdviceParserError> {
        let mut cfg = self.config.write().expect("advice config: lock poisoned");
        cfg.disabled.clear();
        self.save_config(&cfg)?;
        Ok(cfg.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_advice_file(dir: &Path, id: &str, content: &str) {
        let path = dir.join(format!("{}.toml", id));
        std::fs::write(&path, content).unwrap();
    }

    #[test]
    fn test_load_single_advice() {
        let dir = tempdir().unwrap();
        write_advice_file(
            dir.path(),
            "maven.build.failure",
            r#"
[advice]
id = "maven.build.failure"
name = "Maven Build Failure"
description = "Triggered when a Maven build assertion fails"
category = "maven"
severity = "warning"

[[advice.triggers]]
type = "assertion_failed"
assertion_id = "maven.build.success"

message = "Build failed. Check the output for compilation errors."
"#,
        );

        let registry = AdviceRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);

        let advice = registry.get("maven.build.failure").unwrap();
        assert_eq!(advice.id, "maven.build.failure");
        assert_eq!(advice.name, "Maven Build Failure");
        assert_eq!(advice.severity, AdviceSeverity::Warning);
        assert_eq!(advice.triggers.len(), 1);
        // suggested_actions uses #[serde(default)] which returns empty Vec when field is absent
        assert_eq!(advice.suggested_actions.len(), 0);
    }

    #[test]
    fn test_load_multiple_advice() {
        let dir = tempdir().unwrap();
        write_advice_file(
            dir.path(),
            "advice1",
            r#"
[advice]
id = "advice1"
name = "Advice 1"
description = "First advice"
category = "test"
severity = "critical"

[[advice.triggers]]
type = "assertion_failed"
assertion_id = "a1"

message = "Message 1"
"#,
        );
        write_advice_file(
            dir.path(),
            "advice2",
            r#"
[advice]
id = "advice2"
name = "Advice 2"
description = "Second advice"
category = "test"
severity = "hint"

[[advice.triggers]]
type = "doctor_failed"
doctor_id = "d1"

message = "Message 2"
"#,
        );

        let registry = AdviceRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 2);

        let list = registry.list();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_experience_pattern_trigger() {
        let dir = tempdir().unwrap();
        write_advice_file(
            dir.path(),
            "exp.pattern",
            r#"
[advice]
id = "exp.pattern"
name = "Experience Pattern"
description = "Experience pattern advice"
category = "test"
severity = "warning"

[[advice.triggers]]
type = "experience_pattern"
tool_name = "sandbox_run"
status = "failure"
threshold = 3

message = "Multiple failures detected"
"#,
        );

        let registry = AdviceRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);

        let advice = registry.get("exp.pattern").unwrap();
        assert_eq!(advice.triggers.len(), 1);
        match &advice.triggers[0] {
            AdviceTrigger::ExperiencePattern {
                tool_name,
                status,
                threshold,
            } => {
                assert_eq!(tool_name, "sandbox_run");
                assert_eq!(status, "failure");
                assert_eq!(*threshold, 3);
            }
            other => panic!("Expected ExperiencePattern, got {:?}", other),
        }
    }

    #[test]
    fn test_duplicate_id_last_wins() {
        let dir = tempdir().unwrap();
        // Two files with the same ID
        write_advice_file(
            dir.path(),
            "dup",
            r#"
[advice]
id = "dup.id"
name = "First"
description = "First one"
category = "test"
severity = "hint"

[[advice.triggers]]
type = "assertion_failed"
assertion_id = "a"

message = "First message"
"#,
        );
        write_advice_file(
            dir.path(),
            "dup2",
            r#"
[advice]
id = "dup.id"
name = "Second"
description = "Second one"
category = "test"
severity = "critical"

[[advice.triggers]]
type = "doctor_failed"
doctor_id = "d"

message = "Second message"
"#,
        );

        let registry = AdviceRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 2); // Both loaded, second overwrites first

        let advice = registry.get("dup.id").unwrap();
        assert_eq!(advice.name, "Second"); // Last wins
        assert_eq!(advice.severity, AdviceSeverity::Critical);
    }

    #[test]
    fn test_not_found() {
        let registry = AdviceRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_get_source() {
        let dir = tempdir().unwrap();
        let content = r#"
[advice]
id = "source.test"
name = "Source Test"
description = "Testing source retention"
category = "test"
severity = "warning"

[[advice.triggers]]
type = "assertion_failed"
assertion_id = "a"

message = "Message"
"#;
        let path = dir.path().join("source.test.toml");
        std::fs::write(&path, content).unwrap();

        let registry = AdviceRegistry::new();
        registry.load_from_dir(dir.path()).unwrap();

        let source = registry.get_source("source.test").unwrap();
        assert!(source.contains("source.test"));
        assert!(source.contains("Source Test"));
    }

    #[test]
    fn test_list() {
        let dir = tempdir().unwrap();
        for i in 0..3 {
            write_advice_file(
                dir.path(),
                &format!("advice{}", i),
                &format!(
                    r#"
[advice]
id = "advice{}"
name = "Advice {}"
description = "Advice number {}"
category = "test"
severity = "warning"

[[advice.triggers]]
type = "assertion_failed"
assertion_id = "a{}"

message = "Message {}"
"#,
                    i, i, i, i, i
                ),
            );
        }

        let registry = AdviceRegistry::new();
        registry.load_from_dir(dir.path()).unwrap();

        let list = registry.list();
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn test_empty_dir() {
        let dir = tempdir().unwrap();
        let registry = AdviceRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_config_store_default() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("advice.toml");
        let store = AdviceConfigStore::new(config_path.clone());

        let cfg = store.get_config();
        assert!(cfg.enabled);
        assert!(cfg.disabled.is_empty());
    }

    #[test]
    fn test_config_store_save_and_load() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("advice.toml");

        // Create store and modify config
        let store = AdviceConfigStore::new(config_path.clone());
        store.disable_advice("advice1").unwrap();
        store.disable_advice("advice2").unwrap();

        // Create new store from same path and verify
        let store2 = AdviceConfigStore::new(config_path);
        let cfg = store2.get_config();
        assert!(cfg.enabled);
        assert_eq!(cfg.disabled, vec!["advice1", "advice2"]);
    }

    #[test]
    fn test_config_clear_disabled() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("advice.toml");

        let store = AdviceConfigStore::new(config_path.clone());
        store.disable_advice("a").unwrap();
        store.disable_advice("b").unwrap();
        store.clear_disabled().unwrap();

        let store2 = AdviceConfigStore::new(config_path);
        let cfg = store2.get_config();
        assert!(cfg.disabled.is_empty());
    }

    // ─── to_cel_condition tests ────────────────────────────────────────────────

    #[test]
    fn test_toml_trigger_to_cel_assertion_failed() {
        let trigger = TomlTrigger::AssertionFailed {
            assertion_id: "maven.build.success".to_string(),
        };
        assert_eq!(
            trigger.to_cel_condition(),
            Some("fact('assertion:maven.build.success') == 'failed'".to_string())
        );
    }

    #[test]
    fn test_toml_trigger_to_cel_doctor_failed() {
        let trigger = TomlTrigger::DoctorFailed {
            doctor_id: "sandbox.alive".to_string(),
        };
        assert_eq!(
            trigger.to_cel_condition(),
            Some("fact('doctor:sandbox.alive') == 'failed'".to_string())
        );
    }

    #[test]
    fn test_toml_trigger_to_cel_experience_pattern() {
        let trigger = TomlTrigger::ExperiencePattern {
            tool_name: "cargo".to_string(),
            status: "FAIL".to_string(),
            threshold: 3,
        };
        assert_eq!(
            trigger.to_cel_condition(),
            Some("count_fact('experience:cargo:FAIL', '>=', 3)".to_string())
        );
    }
}
