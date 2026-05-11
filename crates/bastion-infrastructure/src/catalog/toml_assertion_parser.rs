//! TOML assertion parser.
//!
//! Loads `AssertionDescriptor` from TOML files in `.bastion/catalog/assertions/`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, RwLock};

use bastion_domain::catalog::assertion::{AssertionCheck, AssertionDescriptor};
use serde::Deserialize;

/// TOML configuration for an assertion, mirrors the TOML file structure.
#[derive(Debug, Deserialize)]
pub struct TomlAssertionConfig {
    pub assertion: TomlAssertion,
}

/// TOML `[assertion]` section.
/// Supports both legacy `[assertion.check]` (single) and `[[assertion.checks]]` (multi).
#[derive(Debug, Deserialize)]
pub struct TomlAssertion {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default = "default_category")]
    pub category: String,
    /// Legacy single check via `[assertion.check]`.
    #[serde(default)]
    pub check: Option<TomlCheck>,
    /// Multi-check via `[[assertion.checks]]`.
    #[serde(default)]
    pub checks: Vec<TomlCheck>,
}

/// A single check parsed from TOML.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TomlCheck {
    ExitCode { expected: i32 },
    StdoutContains { substring: String },
    StderrContains { substring: String },
    StdoutMatches { regex: String },
    SandboxAlive,
    CommandDuration { max_ms: u64 },
}

fn default_category() -> String {
    "general".to_string()
}

impl TomlCheck {
    /// Convert to a CEL condition string for the CEL-lite rules engine.
    ///
    /// Returns `None` for checks that have no CEL equivalent (e.g., SandboxAlive).
    pub fn to_cel_condition(&self) -> Option<String> {
        match self {
            TomlCheck::ExitCode { expected } => Some(format!("exit_code == {}", expected)),
            TomlCheck::StdoutContains { substring } => Some(format!(
                "stdout_contains('{}')",
                Self::escape_cel_string(substring)
            )),
            TomlCheck::StderrContains { substring } => Some(format!(
                "stderr_contains('{}')",
                Self::escape_cel_string(substring)
            )),
            TomlCheck::StdoutMatches { regex } => Some(format!(
                "stdout_matches('{}')",
                Self::escape_cel_string(regex)
            )),
            TomlCheck::SandboxAlive => None, // Deferred — no CEL equivalent
            TomlCheck::CommandDuration { max_ms } => Some(format!("duration_lt({})", max_ms)),
        }
    }

    /// Escape special characters in a CEL string literal.
    fn escape_cel_string(s: &str) -> String {
        s.replace('\\', "\\\\").replace('\'', "\\'")
    }

    fn into_assertion_check(self) -> AssertionCheck {
        match self {
            TomlCheck::ExitCode { expected } => AssertionCheck::ExitCode { expected },
            TomlCheck::StdoutContains { substring } => AssertionCheck::StdoutContains { substring },
            TomlCheck::StderrContains { substring } => AssertionCheck::StderrContains { substring },
            TomlCheck::StdoutMatches { regex } => AssertionCheck::StdoutMatches { regex },
            TomlCheck::SandboxAlive => AssertionCheck::SandboxAlive,
            TomlCheck::CommandDuration { max_ms } => AssertionCheck::CommandDuration { max_ms },
        }
    }
}

/// Convert TomlCheck to AssertionCheck for legacy evaluation.
impl From<TomlCheck> for AssertionCheck {
    fn from(toml: TomlCheck) -> Self {
        toml.into_assertion_check()
    }
}

/// Convert AssertionCheck to TomlCheck for shim testing.
impl From<AssertionCheck> for TomlCheck {
    fn from(assertion: AssertionCheck) -> Self {
        match assertion {
            AssertionCheck::ExitCode { expected } => TomlCheck::ExitCode { expected },
            AssertionCheck::StdoutContains { substring } => TomlCheck::StdoutContains { substring },
            AssertionCheck::StderrContains { substring } => TomlCheck::StderrContains { substring },
            AssertionCheck::StdoutMatches { regex } => TomlCheck::StdoutMatches { regex },
            AssertionCheck::SandboxAlive => TomlCheck::SandboxAlive,
            AssertionCheck::CommandDuration { max_ms } => TomlCheck::CommandDuration { max_ms },
        }
    }
}

/// TOML config into AssertionDescriptor.
impl From<TomlAssertionConfig> for AssertionDescriptor {
    fn from(config: TomlAssertionConfig) -> Self {
        let TomlAssertionConfig { assertion } = config;
        // Collect checks from both legacy `check` and new `checks` fields.
        let mut all_checks: Vec<AssertionCheck> = assertion
            .check
            .map(|c| c.into_assertion_check())
            .into_iter()
            .chain(
                assertion
                    .checks
                    .into_iter()
                    .map(|c| c.into_assertion_check()),
            )
            .collect();
        // Ensure at least one check exists (normalize empty to first check if needed).
        if all_checks.is_empty() {
            tracing::warn!(id = %assertion.id, "Assertion has no checks, defaulting to exit_code=0");
            all_checks.push(AssertionCheck::ExitCode { expected: 0 });
        }
        AssertionDescriptor {
            id: assertion.id,
            name: assertion.name,
            description: assertion.description,
            category: assertion.category,
            checks: all_checks,
        }
    }
}

/// Error type for assertion parsing operations.
#[derive(Debug, thiserror::Error)]
pub enum AssertionParserError {
    #[error("Failed to read directory: {0}")]
    ReadDir(#[from] std::io::Error),
    #[error("Failed to parse TOML: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("Assertion '{0}' not found")]
    NotFound(String),
}

/// Loads and manages assertion descriptors from TOML files.
#[derive(Debug)]
pub struct AssertionRegistry {
    assertions: Arc<RwLock<HashMap<String, AssertionDescriptor>>>,
}

impl AssertionRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            assertions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load all assertion TOMLs from a directory.
    ///
    /// Returns the number of assertions successfully loaded.
    /// Skips files that fail to parse with a warning.
    pub fn load_from_dir(&self, dir: &Path) -> Result<usize, AssertionParserError> {
        let mut loaded = 0;

        if !dir.exists() {
            tracing::info!(path = %dir.display(), "Assertion config directory does not exist, skipping");
            return Ok(0);
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }

            match self.load_file(&path) {
                Ok(assertion) => {
                    tracing::info!(
                        id = %assertion.id,
                        path = %path.display(),
                        "Loaded assertion"
                    );
                    self.assertions
                        .write()
                        .expect("assertion registry: lock poisoned")
                        .insert(assertion.id.clone(), assertion);
                    loaded += 1;
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "Failed to load assertion");
                }
            }
        }

        tracing::info!(loaded, "Assertions loaded from {}", dir.display());
        Ok(loaded)
    }

    /// Load a single assertion TOML file.
    fn load_file(&self, path: &Path) -> Result<AssertionDescriptor, AssertionParserError> {
        let content = fs::read_to_string(path)?;
        let config: TomlAssertionConfig = toml::from_str(&content)?;
        Ok(config.into())
    }

    /// Get an assertion by ID.
    pub fn get(&self, id: &str) -> Option<AssertionDescriptor> {
        self.assertions
            .read()
            .expect("assertion registry: lock poisoned")
            .get(id)
            .cloned()
    }

    /// List all loaded assertion IDs.
    pub fn list(&self) -> Vec<AssertionDescriptor> {
        self.assertions
            .read()
            .expect("assertion registry: lock poisoned")
            .values()
            .cloned()
            .collect()
    }

    /// Number of loaded assertions.
    pub fn len(&self) -> usize {
        self.assertions
            .read()
            .expect("assertion registry: lock poisoned")
            .len()
    }

    /// Check if registry is empty.
    pub fn is_empty(&self) -> bool {
        self.assertions
            .read()
            .expect("assertion registry: lock poisoned")
            .is_empty()
    }
}

impl Default for AssertionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_load_from_dir() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("command.exit_code.zero.toml");
        std::fs::write(
            &path,
            r#"
[assertion]
id = "command.exit_code.zero"
name = "Exit Code Zero"
description = "Fails if command exit code is non-zero"
category = "command"

[assertion.check]
type = "exit_code"
expected = 0
"#,
        )
        .unwrap();

        let registry = AssertionRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);

        let assertion = registry.get("command.exit_code.zero").unwrap();
        assert_eq!(assertion.id, "command.exit_code.zero");
        assert!(matches!(
            assertion.checks[0],
            AssertionCheck::ExitCode { expected: 0 }
        ));
    }

    #[test]
    fn test_load_multi_check_assertion() {
        // Test multi-check via [[assertion.checks]]
        let dir = tempdir().unwrap();
        let path = dir.path().join("maven.build.success.toml");
        std::fs::write(
            &path,
            r#"
[assertion]
id = "maven.build.success"
name = "Maven Build Success"
description = "Maven build must exit with code 0 and stdout must contain BUILD SUCCESS"
category = "maven"

[[assertion.checks]]
type = "exit_code"
expected = 0

[[assertion.checks]]
type = "stdout_contains"
substring = "BUILD SUCCESS"
"#,
        )
        .unwrap();

        let registry = AssertionRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);

        let assertion = registry.get("maven.build.success").unwrap();
        assert_eq!(assertion.checks.len(), 2);
        assert!(matches!(
            assertion.checks[0],
            AssertionCheck::ExitCode { expected: 0 }
        ));
        assert!(matches!(
            &assertion.checks[1],
            AssertionCheck::StdoutContains { substring } if substring == "BUILD SUCCESS"
        ));
    }

    #[test]
    fn test_load_legacy_single_check() {
        // Test backwards compatibility with legacy [assertion.check]
        let dir = tempdir().unwrap();
        let path = dir.path().join("command.exit_code.zero.toml");
        std::fs::write(
            &path,
            r#"
[assertion]
id = "command.exit_code.zero"
name = "Exit Code Zero"
description = "Fails if command exit code is non-zero"
category = "command"

[assertion.check]
type = "exit_code"
expected = 0
"#,
        )
        .unwrap();

        let registry = AssertionRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);

        let assertion = registry.get("command.exit_code.zero").unwrap();
        assert_eq!(assertion.checks.len(), 1);
        assert!(matches!(
            assertion.checks[0],
            AssertionCheck::ExitCode { expected: 0 }
        ));
    }

    #[test]
    fn test_load_combined_checks() {
        // Test that both legacy check and new checks can coexist (check first, then checks)
        let dir = tempdir().unwrap();
        let path = dir.path().join("combined.checks.toml");
        std::fs::write(
            &path,
            r#"
[assertion]
id = "combined.checks"
name = "Combined Checks"
description = "Has both legacy check and additional checks"
category = "test"

[assertion.check]
type = "exit_code"
expected = 0

[[assertion.checks]]
type = "stdout_contains"
substring = "done"
"#,
        )
        .unwrap();

        let registry = AssertionRegistry::new();
        let count = registry.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 1);

        let assertion = registry.get("combined.checks").unwrap();
        // Legacy check + 1 new check = 2 total
        assert_eq!(assertion.checks.len(), 2);
        assert!(matches!(
            assertion.checks[0],
            AssertionCheck::ExitCode { expected: 0 }
        ));
        assert!(matches!(
            &assertion.checks[1],
            AssertionCheck::StdoutContains { substring } if substring == "done"
        ));
    }

    #[test]
    fn test_not_found() {
        let registry = AssertionRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    // ─── to_cel_condition tests ────────────────────────────────────────────────

    #[test]
    fn test_toml_check_to_cel_exit_code() {
        let check = TomlCheck::ExitCode { expected: 0 };
        assert_eq!(check.to_cel_condition(), Some("exit_code == 0".to_string()));
    }

    #[test]
    fn test_toml_check_to_cel_exit_code_nonzero() {
        let check = TomlCheck::ExitCode { expected: 127 };
        assert_eq!(
            check.to_cel_condition(),
            Some("exit_code == 127".to_string())
        );
    }

    #[test]
    fn test_toml_check_to_cel_stdout_contains() {
        let check = TomlCheck::StdoutContains {
            substring: "BUILD SUCCESS".to_string(),
        };
        assert_eq!(
            check.to_cel_condition(),
            Some("stdout_contains('BUILD SUCCESS')".to_string())
        );
    }

    #[test]
    fn test_toml_check_to_cel_stderr_contains() {
        let check = TomlCheck::StderrContains {
            substring: "ERROR".to_string(),
        };
        assert_eq!(
            check.to_cel_condition(),
            Some("stderr_contains('ERROR')".to_string())
        );
    }

    #[test]
    fn test_toml_check_to_cel_stdout_matches() {
        // The regex string "line \d+" (single backslash) gets escaped to "line \\d+"
        let check = TomlCheck::StdoutMatches {
            regex: "line \\d+".to_string(),
        };
        assert_eq!(
            check.to_cel_condition(),
            Some("stdout_matches('line \\\\d+')".to_string())
        );
    }

    #[test]
    fn test_toml_check_to_cel_command_duration() {
        let check = TomlCheck::CommandDuration { max_ms: 5000 };
        assert_eq!(
            check.to_cel_condition(),
            Some("duration_lt(5000)".to_string())
        );
    }

    #[test]
    fn test_toml_check_to_cel_sandbox_alive_skipped() {
        // SandboxAlive has no CEL equivalent — returns None
        let check = TomlCheck::SandboxAlive;
        assert_eq!(check.to_cel_condition(), None);
    }

    #[test]
    fn test_toml_check_to_cel_escapes_single_quotes() {
        let check = TomlCheck::StdoutContains {
            substring: "it's broken".to_string(),
        };
        // Single quotes should be escaped
        assert_eq!(
            check.to_cel_condition(),
            Some(r"stdout_contains('it\'s broken')".to_string())
        );
    }

    #[test]
    fn test_toml_check_to_cel_escapes_backslashes() {
        let check = TomlCheck::StdoutContains {
            substring: r"C:\path".to_string(),
        };
        // Backslashes should be escaped
        assert_eq!(
            check.to_cel_condition(),
            Some(r"stdout_contains('C:\\path')".to_string())
        );
    }
}
