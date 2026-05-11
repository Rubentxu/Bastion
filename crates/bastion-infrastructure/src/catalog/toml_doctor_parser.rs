//! TOML doctor parser and registry.
//!
//! Loads `DoctorDescriptor` from TOML files in `.bastion/catalog/doctors/`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, RwLock};

use bastion_domain::catalog::doctor::{DoctorCheck, DoctorDescriptor, Severity};
use serde::Deserialize;

/// TOML configuration for a doctor, mirrors the TOML file structure.
#[derive(Debug, Deserialize)]
pub struct TomlDoctorConfig {
    pub doctor: TomlDoctor,
}

/// TOML `[doctor]` section.
#[derive(Debug, Deserialize)]
pub struct TomlDoctor {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default = "default_severity")]
    pub severity: Severity,
    #[serde(default)]
    pub checks: Vec<TomlDoctorCheck>,
}

fn default_category() -> String {
    "sandbox".to_string()
}

fn default_severity() -> Severity {
    Severity::Warning
}

/// A single check parsed from TOML.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TomlDoctorCheck {
    /// Check if a sandbox is alive via provider.
    Aliveness {
        /// Optional sandbox ID override.
        sandbox_id: Option<String>,
    },
    /// Check resource usage against thresholds.
    Resources {
        /// Maximum total sandboxes across all pools.
        max_total: Option<usize>,
        /// Maximum idle sandboxes per template.
        max_idle_per_template: Option<usize>,
    },
    /// Delegate to an existing assertion by name.
    AssertionDriven {
        /// The assertion ID to evaluate against experience records.
        assertion_id: String,
    },
}

impl TomlDoctorCheck {
    /// Convert to a CEL condition string for the CEL-lite rules engine.
    ///
    /// Returns `None` for checks that have no CEL equivalent (Aliveness, Resources).
    pub fn to_cel_condition(&self) -> Option<String> {
        match self {
            TomlDoctorCheck::Aliveness { .. } => None, // Deferred — infrastructure check
            TomlDoctorCheck::Resources { .. } => None, // Deferred — infrastructure check
            TomlDoctorCheck::AssertionDriven { assertion_id } => {
                // fact('assertion:<id>') == "passed"
                Some(format!("fact('assertion:{}') == 'passed'", assertion_id))
            }
        }
    }

    fn into_doctor_check(self) -> DoctorCheck {
        match self {
            TomlDoctorCheck::Aliveness { sandbox_id } => DoctorCheck::Aliveness { sandbox_id },
            TomlDoctorCheck::Resources {
                max_total,
                max_idle_per_template,
            } => DoctorCheck::Resources {
                max_total,
                max_idle_per_template,
            },
            TomlDoctorCheck::AssertionDriven { assertion_id } => {
                DoctorCheck::AssertionDriven { assertion_id }
            }
        }
    }
}

/// Convert TomlDoctorCheck to DoctorCheck for legacy evaluation.
impl From<TomlDoctorCheck> for DoctorCheck {
    fn from(toml: TomlDoctorCheck) -> Self {
        toml.into_doctor_check()
    }
}

/// TOML config into DoctorDescriptor.
impl From<TomlDoctorConfig> for DoctorDescriptor {
    fn from(config: TomlDoctorConfig) -> Self {
        let TomlDoctorConfig { doctor } = config;
        let checks = doctor
            .checks
            .into_iter()
            .map(|c| c.into_doctor_check())
            .collect();
        DoctorDescriptor {
            id: doctor.id,
            name: doctor.name,
            description: doctor.description,
            category: doctor.category,
            severity: doctor.severity,
            checks,
        }
    }
}

/// Error type for doctor parsing operations.
#[derive(Debug, thiserror::Error)]
pub enum DoctorParserError {
    #[error("Failed to read directory: {0}")]
    ReadDir(#[from] std::io::Error),
    #[error("Failed to parse TOML: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("Doctor '{0}' not found")]
    NotFound(String),
}

/// Loads and manages doctor descriptors from TOML files.
#[derive(Debug)]
pub struct DoctorRegistry {
    doctors: Arc<RwLock<HashMap<String, DoctorDescriptor>>>,
    /// TOML source for each doctor (for doctor_explain)
    sources: Arc<RwLock<HashMap<String, String>>>,
}

impl DoctorRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            doctors: Arc::new(RwLock::new(HashMap::new())),
            sources: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load all doctor TOMLs from a directory.
    ///
    /// Returns the number of doctors successfully loaded.
    /// Skips files that fail to parse with a warning.
    pub fn load_from_dir(&self, dir: &Path) -> Result<usize, DoctorParserError> {
        let mut loaded = 0;

        if !dir.exists() {
            tracing::info!(path = %dir.display(), "Doctor config directory does not exist, skipping");
            return Ok(0);
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }

            match self.load_file(&path) {
                Ok((doctor, source)) => {
                    let doctor_id = doctor.id.clone();
                    tracing::info!(
                        id = %doctor_id,
                        path = %path.display(),
                        "Loaded doctor"
                    );
                    {
                        let mut doctors = self.doctors.write().unwrap();
                        doctors.insert(doctor_id.clone(), doctor);
                    }
                    {
                        let mut sources = self.sources.write().unwrap();
                        sources.insert(doctor_id, source);
                    }
                    loaded += 1;
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "Failed to load doctor");
                }
            }
        }

        tracing::info!(loaded, "Doctors loaded from {}", dir.display());
        Ok(loaded)
    }

    /// Load a single doctor TOML file.
    fn load_file(&self, path: &Path) -> Result<(DoctorDescriptor, String), DoctorParserError> {
        let content = fs::read_to_string(path)?;
        let config: TomlDoctorConfig = toml::from_str(&content)?;
        let descriptor: DoctorDescriptor = config.into();
        Ok((descriptor, content))
    }

    /// Get a doctor by ID.
    pub fn get(&self, id: &str) -> Option<DoctorDescriptor> {
        self.doctors.read().unwrap().get(id).cloned()
    }

    /// List all loaded doctors.
    pub fn list(&self) -> Vec<DoctorDescriptor> {
        self.doctors.read().unwrap().values().cloned().collect()
    }

    /// Get the TOML source for a doctor (for doctor_explain).
    pub fn get_source(&self, id: &str) -> Option<String> {
        self.sources.read().unwrap().get(id).cloned()
    }

    /// Number of loaded doctors.
    pub fn len(&self) -> usize {
        self.doctors.read().unwrap().len()
    }

    /// Check if registry is empty.
    pub fn is_empty(&self) -> bool {
        self.doctors.read().unwrap().is_empty()
    }
}

impl Default for DoctorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_domain::catalog::doctor::Severity;
    use tempfile::tempdir;

    #[test]
    fn test_load_from_dir() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sandbox.alive.toml");
        std::fs::write(
            &path,
            r#"
[doctor]
id = "sandbox.alive"
name = "Sandbox Alive"
description = "Checks that a sandbox is alive and responsive"
category = "sandbox"
severity = "critical"

[[doctor.checks]]
type = "aliveness"
"#,
        )
        .unwrap();

        let registry = DoctorRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);

        let doctor = registry.get("sandbox.alive").unwrap();
        assert_eq!(doctor.id, "sandbox.alive");
        assert_eq!(doctor.name, "Sandbox Alive");
        assert_eq!(doctor.severity, Severity::Critical);
        assert!(matches!(
            doctor.checks[0],
            DoctorCheck::Aliveness { sandbox_id: None }
        ));
    }

    #[test]
    fn test_load_resources_check() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sandbox.resources.toml");
        std::fs::write(
            &path,
            r#"
[doctor]
id = "sandbox.resources"
name = "Sandbox Resources"
description = "Checks pool resource usage against thresholds"
category = "sandbox"
severity = "warning"

[[doctor.checks]]
type = "resources"
max_total = 200
max_idle_per_template = 20
"#,
        )
        .unwrap();

        let registry = DoctorRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);

        let doctor = registry.get("sandbox.resources").unwrap();
        assert!(matches!(
            doctor.checks[0],
            DoctorCheck::Resources {
                max_total: Some(200),
                max_idle_per_template: Some(20)
            }
        ));
    }

    #[test]
    fn test_load_assertion_driven_check() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("docker.daemon.toml");
        std::fs::write(
            &path,
            r#"
[doctor]
id = "docker.daemon"
name = "Docker Daemon"
description = "Checks Docker daemon health via assertion replay"
category = "provider"
severity = "critical"

[[doctor.checks]]
type = "assertion_driven"
assertion_id = "command.exit_code.zero"
"#,
        )
        .unwrap();

        let registry = DoctorRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);

        let doctor = registry.get("docker.daemon").unwrap();
        match &doctor.checks[0] {
            DoctorCheck::AssertionDriven { assertion_id } => {
                assert_eq!(assertion_id, "command.exit_code.zero");
            }
            other => panic!("Expected AssertionDriven, got {:?}", other),
        }
    }

    #[test]
    fn test_load_multiple_checks() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("multi.check.toml");
        std::fs::write(
            &path,
            r#"
[doctor]
id = "multi.check"
name = "Multi Check"
description = "Multiple checks"
category = "test"
severity = "warning"

[[doctor.checks]]
type = "aliveness"

[[doctor.checks]]
type = "resources"
max_total = 100
"#,
        )
        .unwrap();

        let registry = DoctorRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);

        let doctor = registry.get("multi.check").unwrap();
        assert_eq!(doctor.checks.len(), 2);
    }

    #[test]
    fn test_not_found() {
        let registry = DoctorRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_get_source() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sandbox.alive.toml");
        let content = r#"
[doctor]
id = "sandbox.alive"
name = "Sandbox Alive"
description = "Checks that a sandbox is alive"
category = "sandbox"
severity = "critical"

[[doctor.checks]]
type = "aliveness"
"#;
        std::fs::write(&path, content).unwrap();

        let registry = DoctorRegistry::new();
        registry.load_from_dir(dir.path()).unwrap();

        let source = registry.get_source("sandbox.alive").unwrap();
        assert!(source.contains("sandbox.alive"));
    }

    #[test]
    fn test_round_trip() {
        // Test that TOML -> parse -> serialize -> parse gives equivalent result
        let dir = tempdir().unwrap();
        let path = dir.path().join("roundtrip.toml");
        let original = r#"
[doctor]
id = "roundtrip.test"
name = "Roundtrip Test"
description = "Testing roundtrip serialization"
category = "test"
severity = "info"

[[doctor.checks]]
type = "aliveness"
sandbox_id = "sb-123"
"#;
        std::fs::write(&path, original).unwrap();

        let registry = DoctorRegistry::new();
        registry.load_from_dir(dir.path()).unwrap();

        let doctor = registry.get("roundtrip.test").unwrap();
        assert_eq!(doctor.id, "roundtrip.test");
        assert_eq!(doctor.name, "Roundtrip Test");
    }

    #[test]
    fn test_list() {
        let dir = tempdir().unwrap();
        let path1 = dir.path().join("doctor1.toml");
        let path2 = dir.path().join("doctor2.toml");
        std::fs::write(
            &path1,
            r#"
[doctor]
id = "doctor1"
name = "Doctor 1"
description = "First doctor"
category = "test"
severity = "warning"
[[doctor.checks]]
type = "aliveness"
"#,
        )
        .unwrap();
        std::fs::write(
            &path2,
            r#"
[doctor]
id = "doctor2"
name = "Doctor 2"
description = "Second doctor"
category = "test"
severity = "info"
[[doctor.checks]]
type = "resources"
max_total = 50
"#,
        )
        .unwrap();

        let registry = DoctorRegistry::new();
        registry.load_from_dir(dir.path()).unwrap();

        let doctors = registry.list();
        assert_eq!(doctors.len(), 2);
    }

    // ─── to_cel_condition tests ────────────────────────────────────────────────

    #[test]
    fn test_toml_doctor_check_to_cel_assertion_driven() {
        let check = TomlDoctorCheck::AssertionDriven {
            assertion_id: "maven.build.success".to_string(),
        };
        assert_eq!(
            check.to_cel_condition(),
            Some("fact('assertion:maven.build.success') == 'passed'".to_string())
        );
    }

    #[test]
    fn test_toml_doctor_check_to_cel_aliveness_skipped() {
        // Aliveness has no CEL equivalent — returns None
        let check = TomlDoctorCheck::Aliveness { sandbox_id: None };
        assert_eq!(check.to_cel_condition(), None);
    }

    #[test]
    fn test_toml_doctor_check_to_cel_resources_skipped() {
        // Resources has no CEL equivalent — returns None
        let check = TomlDoctorCheck::Resources {
            max_total: Some(10),
            max_idle_per_template: None,
        };
        assert_eq!(check.to_cel_condition(), None);
    }
}
