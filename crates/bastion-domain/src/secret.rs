//! Secret resolution domain types.
//!
//! Provides the `SecretResolver` port and `SecretSource` value object.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::shared::DomainError;

/// Resolved secret value with metadata about its source.
#[derive(Debug, Clone)]
pub struct SecretValue {
    /// The actual secret value.
    pub value: String,
    /// Where the secret came from (e.g., "env:GITHUB_TOKEN", "vault", "noop").
    pub source: String,
}

impl SecretValue {
    /// Create a new resolved secret.
    pub fn new(value: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            source: source.into(),
        }
    }
}

/// Port for secret resolution — resolves references to actual values.
#[async_trait]
pub trait SecretResolver: Send + Sync {
    /// Resolve a secret key to its value.
    async fn resolve(&self, key: &str) -> Result<SecretValue, DomainError>;
}

/// Source of a secret — either a reference to be resolved, or an inline value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SecretSource {
    /// Reference like `"${{secret:GITHUB_TOKEN}}"` — resolved at gateway level.
    Ref(String),
    /// Direct value — ALREADY resolved (e.g., from env injection).
    Inline(String),
}

impl SecretSource {
    /// Returns the key if this is a `Ref`, otherwise None.
    pub fn as_ref_key(&self) -> Option<&str> {
        match self {
            SecretSource::Ref(k) => Some(k),
            SecretSource::Inline(_) => None,
        }
    }

    /// Returns true if this is an inline secret.
    pub fn is_inline(&self) -> bool {
        matches!(self, SecretSource::Inline(_))
    }
}

/// Pattern for secret references: `${{secret:KEY}}`
pub const SECRET_REF_PREFIX: &str = "${{secret:";
pub const SECRET_REF_SUFFIX: &str = "}}";

/// Parse a secret reference string. Returns `Some(key)` if it matches the pattern.
pub fn parse_secret_ref(s: &str) -> Option<&str> {
    if s.starts_with(SECRET_REF_PREFIX) && s.ends_with(SECRET_REF_SUFFIX) {
        let inner = &s[SECRET_REF_PREFIX.len()..s.len() - SECRET_REF_SUFFIX.len()];
        Some(inner)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_secret_ref_valid() {
        assert_eq!(
            parse_secret_ref("${{secret:GITHUB_TOKEN}}"),
            Some("GITHUB_TOKEN")
        );
        assert_eq!(parse_secret_ref("${{secret:FOO}}"), Some("FOO"));
    }

    #[test]
    fn test_parse_secret_ref_invalid() {
        assert_eq!(parse_secret_ref("GITHUB_TOKEN"), None);
        assert_eq!(parse_secret_ref("${{secret:}}"), Some(""));
        assert_eq!(parse_secret_ref("${{secret:GITHUB_TOKEN"), None);
        assert_eq!(parse_secret_ref("{{secret:GITHUB_TOKEN}}"), None);
    }

    #[test]
    fn test_secret_source_is_inline() {
        assert!(SecretSource::Inline("value".to_string()).is_inline());
        assert!(!SecretSource::Ref("KEY".to_string()).is_inline());
    }

    #[test]
    fn test_secret_source_as_ref_key() {
        assert_eq!(
            SecretSource::Ref("KEY".to_string()).as_ref_key(),
            Some("KEY")
        );
        assert_eq!(SecretSource::Inline("val".to_string()).as_ref_key(), None);
    }
}
