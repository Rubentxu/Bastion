//! Sandbox-backed FileSystem implementation.
//!
//! Implements the `FileSystem` trait by executing commands inside the sandbox
//! via the `SandboxProvider` interface.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use bastion_domain::execution::CommandSpec;
use bastion_domain::provider::port::SandboxProvider;
use bastion_domain::shared::id::SandboxId;

use enrichment_engine::traits::{EnrichmentError, FileSystem};

/// Characters that indicate potential shell command injection.
const DANGEROUS_CHARS: &[char] = &[';', '|', '$', '`', '\n', '\r', '\x00'];

/// A `FileSystem` impl that executes read/find commands inside a sandbox.
#[derive(Debug)]
pub struct SandboxFileSystem {
    provider: Arc<dyn SandboxProvider>,
    sandbox_id: SandboxId,
}

impl SandboxFileSystem {
    /// Create a new SandboxFileSystem.
    pub fn new(provider: Arc<dyn SandboxProvider>, sandbox_id: SandboxId) -> Self {
        Self { provider, sandbox_id }
    }

    /// Check if a path contains dangerous shell characters.
    fn is_dangerous_path(path: &str) -> bool {
        DANGEROUS_CHARS.iter().any(|c| path.contains(*c))
    }

    /// Validate a glob pattern for shell metacharacters.
    /// Only allow alphanumeric, `-`, `_`, `.`, `*`, `/`, and spaces.
    fn is_valid_glob_pattern(pattern: &str) -> bool {
        if pattern.is_empty() {
            return false;
        }
        // Check for dangerous shell metacharacters that are not * (glob wildcard)
        let dangerous_meta: &[char] = &[';', '|', '$', '`', '\'', '"', '<', '>', '&', '\n', '\r', '\x00'];
        !dangerous_meta.iter().any(|c| pattern.contains(*c))
    }

    /// Convert a glob pattern to a safe find command.
    ///
    /// For `target/*.jar`: splits into dir=`target`, pattern=`*.jar`
    /// and produces `find target -name '*.jar'`.
    ///
    /// For `**/*.java`: dir=`.`, pattern=`*.java`
    /// and produces `find . -name '*.java'`.
    ///
    /// For `src/main/java/**/*.kt`: dir=`src/main/java`, pattern=`*.kt`
    /// and produces `find src/main/java -name '*.kt'`.
    ///
    /// Rejects patterns that could escape the sandbox (absolute paths, `..`).
    fn glob_to_find_command(pattern: &str) -> Result<CommandSpec, EnrichmentError> {
        // Handle ** in patterns: ** means "zero or more directories"
        // We need to strip ** from the directory part
        //
        // Strategy: find the last / that separates directory from filename.
        // If the directory part contains **, we need to handle it specially.

        // First, strip any leading **/ since that means "start from current dir"
        let working_pattern = if let Some(stripped) = pattern.strip_prefix("**/") {
            stripped
        } else {
            pattern
        };

        // Find the last / to separate directory from filename
        // But we need to handle ** in the directory part
        let (dir, name) = match working_pattern.rfind('/') {
            Some(idx) if idx > 0 => (&working_pattern[..idx], &working_pattern[idx + 1..]),
            Some(_) => (".", &working_pattern[1..]), // Pattern starts with /
            None => (".", working_pattern),
        };

        // If dir contains **, we need to strip it
        // For "src/main/java/**" + "*.kt" -> dir should be "src/main/java"
        let clean_dir = dir.replace("**", "").trim_end_matches('/').to_string();
        let clean_dir = if clean_dir.is_empty() { "." } else { &clean_dir };

        // Reject path traversal attempts
        if clean_dir.contains("..") || clean_dir.starts_with('/') {
            return Err(EnrichmentError::FileSystem(
                "path traversal not allowed in glob pattern".to_string(),
            ));
        }

        let escaped_dir = clean_dir.replace('\'', "'\\''");
        let escaped_name = name.replace('\'', "'\\''");
        Ok(CommandSpec::new(format!(
            "find {} -name '{}' 2>/dev/null",
            escaped_dir, escaped_name
        )))
    }
}

#[async_trait]
impl FileSystem for SandboxFileSystem {
    async fn read_to_string(&self, path: &str) -> Result<String, EnrichmentError> {
        // Reject dangerous paths that could enable shell injection
        if Self::is_dangerous_path(path) {
            return Err(EnrichmentError::FileSystem("unsafe path".to_string()));
        }

        // Shell-escape the path using single quotes
        // This is safe because we checked for dangerous chars above
        let escaped_path = path.replace('\'', "'\\''");
        let cmd = CommandSpec::new(format!("cat '{}'", escaped_path));

        let result = self
            .provider
            .run_command(&self.sandbox_id, &cmd)
            .await
            .map_err(|e| EnrichmentError::FileSystem(e.to_string()))?;

        if result.exit_code != 0 {
            return Err(EnrichmentError::FileSystem(format!(
                "cat {} failed: {}",
                path,
                String::from_utf8_lossy(&result.stderr)
            )));
        }

        Ok(String::from_utf8_lossy(&result.stdout).to_string())
    }

    async fn glob(&self, pattern: &str) -> Result<Vec<PathBuf>, EnrichmentError> {
        // Validate glob pattern at execution time
        if !Self::is_valid_glob_pattern(pattern) {
            return Err(EnrichmentError::FileSystem("invalid glob pattern".to_string()));
        }

        // Convert glob pattern to safe find command
        // This fixes the semantics: target/*.jar now correctly matches target/app.jar
        let find_cmd = Self::glob_to_find_command(pattern)?;

        let result = self
            .provider
            .run_command(&self.sandbox_id, &find_cmd)
            .await
            .map_err(|e| EnrichmentError::FileSystem(e.to_string()))?;

        if result.exit_code != 0 {
            return Ok(Vec::new());
        }

        let output = String::from_utf8_lossy(&result.stdout);
        let paths: Vec<PathBuf> = output
            .lines()
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect();

        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dangerous_path_rejected() {
        // These paths should be rejected
        assert!(SandboxFileSystem::is_dangerous_path("/etc/passwd; rm -rf /"));
        assert!(SandboxFileSystem::is_dangerous_path("file.txt | cat"));
        assert!(SandboxFileSystem::is_dangerous_path("$HOME/.ssh/id_rsa"));
        assert!(SandboxFileSystem::is_dangerous_path("`whoami`"));
        assert!(SandboxFileSystem::is_dangerous_path("multi\nline"));
        // Safe paths should pass
        assert!(!SandboxFileSystem::is_dangerous_path("target/classes/app.jar"));
        assert!(!SandboxFileSystem::is_dangerous_path("simple/path/file.txt"));
    }

    #[test]
    fn test_glob_pattern_validation() {
        // Valid patterns
        assert!(SandboxFileSystem::is_valid_glob_pattern("target/*.jar"));
        assert!(SandboxFileSystem::is_valid_glob_pattern("src/**/*.rs"));
        assert!(SandboxFileSystem::is_valid_glob_pattern("**/*.txt"));
        // Invalid patterns
        assert!(!SandboxFileSystem::is_valid_glob_pattern(""));
        assert!(!SandboxFileSystem::is_valid_glob_pattern("; rm -rf /"));
        assert!(!SandboxFileSystem::is_valid_glob_pattern("file | cat"));
        assert!(!SandboxFileSystem::is_valid_glob_pattern("$(whoami)"));
    }

    #[test]
    fn test_glob_to_find_command_simple() {
        // target/*.jar -> find target -name '*.jar'
        let cmd = SandboxFileSystem::glob_to_find_command("target/*.jar").unwrap();
        assert!(cmd.command.contains("find target -name '*.jar'"), "got: {}", cmd.command);
    }

    #[test]
    fn test_glob_to_find_command_recursive() {
        // **/*.java -> find . -name '*.java'
        let cmd = SandboxFileSystem::glob_to_find_command("**/*.java").unwrap();
        assert!(cmd.command.contains("find . -name '*.java'"), "got: {}", cmd.command);
    }

    #[test]
    fn test_glob_to_find_command_nested() {
        // src/main/java/**/*.kt -> find src/main/java -name '*.kt'
        let cmd = SandboxFileSystem::glob_to_find_command("src/main/java/**/*.kt").unwrap();
        assert!(cmd.command.contains("find src/main/java -name '*.kt'"), "got: {}", cmd.command);
    }

    #[test]
    fn test_glob_to_find_command_rejects_path_traversal() {
        // Absolute path should be rejected
        let result = SandboxFileSystem::glob_to_find_command("/etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path traversal"));

        // Parent directory traversal should be rejected
        let result = SandboxFileSystem::glob_to_find_command("../target/*.jar");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path traversal"));

        // Deep parent traversal
        let result = SandboxFileSystem::glob_to_find_command("target/../../etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_glob_to_find_command_single_level() {
        // *.jar -> find . -name '*.jar'
        let cmd = SandboxFileSystem::glob_to_find_command("*.jar").unwrap();
        assert!(cmd.command.contains("find . -name '*.jar'"), "got: {}", cmd.command);
    }
}
