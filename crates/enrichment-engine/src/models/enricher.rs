//! Enricher descriptor and extractor configuration models.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Policy for the command extractor.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CommandExtractorPolicy {
    /// Reject unsafe shell syntax (backticks, $(), etc.)
    #[serde(default = "default_true")]
    pub strict: bool,
    /// Emit flag facts (--flag, -flag)
    #[serde(default = "default_true")]
    pub allow_flags: bool,
    /// Emit option facts (-Dk=v, -Pprofile)
    #[serde(default = "default_true")]
    pub allow_options: bool,
    /// Emit target facts (positional non-flag args)
    #[serde(default = "default_true")]
    pub allow_targets: bool,
    /// Max tokens before returning command_too_long diagnostic
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Max input length before returning command_too_long diagnostic
    #[serde(default = "default_max_input_len")]
    pub max_input_len: u32,
    /// Per-enricher goal→intent map override
    #[serde(default)]
    pub goal_map: Option<HashMap<String, String>>,
}

fn default_true() -> bool {
    true
}

fn default_max_tokens() -> u32 {
    64
}

fn default_max_input_len() -> u32 {
    4096
}

impl Default for CommandExtractorPolicy {
    fn default() -> Self {
        Self {
            strict: true,
            allow_flags: true,
            allow_options: true,
            allow_targets: true,
            max_tokens: 64,
            max_input_len: 4096,
            goal_map: None,
        }
    }
}

/// Configuration for an individual extractor within an enricher.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtractorConfig {
    /// Unique extractor identifier within the enricher.
    pub id: String,
    /// Extractor type: "regex", "glob", or "command".
    pub extractor_type: String,
    /// The pattern (regex or glob) to use.
    pub pattern: String,
    /// The fact key to emit.
    pub fact_key: String,
    /// Extraction priority (lower = higher priority).
    #[serde(default)]
    pub priority: i32,
    /// Merge mode: "single" (dedupe by key, max confidence wins) or "multi" (preserve all facts).
    #[serde(default = "default_merge_mode")]
    pub merge_mode: String,
    /// Optional policy for command extractors.
    #[serde(default)]
    pub command_extractor_policy: Option<CommandExtractorPolicy>,
    /// Static facts map (key → value) for static extractors.
    #[serde(default)]
    pub static_value: Option<std::collections::HashMap<String, String>>,
    /// Override the output fact key.
    #[serde(default)]
    pub output_key: Option<String>,
    /// Expected value shape (scalar, list, map).
    #[serde(default)]
    pub shape: Option<String>,
    /// Semantic type (e.g. "version", "count", "status").
    #[serde(default)]
    pub fact_type: Option<String>,
    /// Default confidence for facts from this extractor.
    #[serde(default)]
    pub confidence: Option<f32>,
    /// Provenance label (e.g. "stdout", "filesystem").
    #[serde(default)]
    pub source: Option<String>,
    /// When true, replaces `merge_mode: "single"` semantics.
    #[serde(default)]
    pub single: bool,
}

fn default_merge_mode() -> String {
    "single".to_string()
}

impl Default for ExtractorConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            extractor_type: String::new(),
            pattern: String::new(),
            fact_key: String::new(),
            priority: 0,
            merge_mode: default_merge_mode(),
            command_extractor_policy: None,
            static_value: None,
            output_key: None,
            shape: None,
            fact_type: None,
            confidence: None,
            source: None,
            single: false,
        }
    }
}

/// Descriptor for an enricher — loaded from the catalog.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnricherDescriptor {
    /// Unique enricher identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Semantic version.
    pub version: String,
    /// Regex patterns that activate this enricher.
    pub match_patterns: Vec<String>,
    /// Output template with `{{key}}` interpolation.
    pub template: String,
    /// Whether this enricher is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Extractors to run when this enricher is activated.
    #[serde(default)]
    pub extractors: Vec<ExtractorConfig>,
    /// Catalog schema version for migration support.
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    /// Human-readable description of the enricher.
    #[serde(default)]
    pub description: Option<String>,
    /// Grouping category (e.g. "build", "test", "ci").
    #[serde(default)]
    pub category: Option<String>,
    /// Alias for `match_patterns[0]` for simpler YAML.
    #[serde(default)]
    pub command_pattern: Option<String>,
    /// Scopes where advice applies.
    #[serde(default)]
    pub advice_scope: Vec<String>,
    /// Pre-condition checks before enricher activation.
    #[serde(default)]
    pub pre_checks: Vec<String>,
    /// Assertion ids linked to this enricher.
    #[serde(default)]
    pub assertions: Vec<String>,
}

fn default_enabled() -> bool {
    true
}

fn default_schema_version() -> String {
    "1.0".to_string()
}

impl Default for EnricherDescriptor {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            version: String::new(),
            match_patterns: Vec::new(),
            template: String::new(),
            enabled: default_enabled(),
            extractors: Vec::new(),
            schema_version: default_schema_version(),
            description: None,
            category: None,
            command_pattern: None,
            advice_scope: Vec::new(),
            pre_checks: Vec::new(),
            assertions: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_extractor_policy_partial_json_applies_defaults() {
        // Partial JSON missing fields should apply defaults
        let json = r#"{"strict":false}"#;
        let policy: CommandExtractorPolicy = serde_json::from_str(json).unwrap();
        assert_eq!(policy.strict, false);
        assert_eq!(policy.allow_flags, true); // default
        assert_eq!(policy.allow_options, true); // default
        assert_eq!(policy.allow_targets, true); // default
        assert_eq!(policy.max_tokens, 64); // default
        assert_eq!(policy.max_input_len, 4096); // default
        assert_eq!(policy.goal_map, None); // default
    }

    #[test]
    fn test_command_extractor_policy_full_json_round_trip() {
        // Full policy round-trips correctly
        let mut goal_map = HashMap::new();
        goal_map.insert("clean".to_string(), "cleanup".to_string());
        let policy = CommandExtractorPolicy {
            strict: false,
            allow_flags: true,
            allow_options: false,
            allow_targets: true,
            max_tokens: 128,
            max_input_len: 8192,
            goal_map: Some(goal_map),
        };
        let json = serde_json::to_string(&policy).unwrap();
        let round_tripped: CommandExtractorPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped.strict, false);
        assert_eq!(round_tripped.allow_flags, true);
        assert_eq!(round_tripped.allow_options, false);
        assert_eq!(round_tripped.allow_targets, true);
        assert_eq!(round_tripped.max_tokens, 128);
        assert_eq!(round_tripped.max_input_len, 8192);
        assert!(round_tripped.goal_map.is_some());
        assert_eq!(
            round_tripped.goal_map.as_ref().unwrap().get("clean"),
            Some(&"cleanup".to_string())
        );
    }

    #[test]
    fn test_extractor_config_backward_compatible_without_policy() {
        // ExtractorConfig without command_extractor_policy is backward compatible
        let json = r#"{
            "id": "test",
            "extractor_type": "regex",
            "pattern": ".*",
            "fact_key": "test",
            "priority": 1,
            "merge_mode": "single"
        }"#;
        let config: ExtractorConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.id, "test");
        assert_eq!(config.extractor_type, "regex");
        assert_eq!(config.command_extractor_policy, None); // default
    }

    #[test]
    fn test_extractor_config_with_command_policy() {
        // ExtractorConfig with command_extractor_policy round-trips correctly
        let json = r#"{
            "id": "cmd",
            "extractor_type": "command",
            "pattern": "",
            "fact_key": "command",
            "priority": 0,
            "merge_mode": "multi",
            "command_extractor_policy": {
                "strict": true,
                "allow_flags": false
            }
        }"#;
        let config: ExtractorConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.extractor_type, "command");
        assert!(config.command_extractor_policy.is_some());
        let policy = config.command_extractor_policy.unwrap();
        assert_eq!(policy.strict, true);
        assert_eq!(policy.allow_flags, false);
        assert_eq!(policy.allow_options, true); // default
        assert_eq!(policy.allow_targets, true); // default
    }

    #[test]
    fn test_command_extractor_policy_default() {
        // Default policy has correct values
        let policy = CommandExtractorPolicy::default();
        assert_eq!(policy.strict, true);
        assert_eq!(policy.allow_flags, true);
        assert_eq!(policy.allow_options, true);
        assert_eq!(policy.allow_targets, true);
        assert_eq!(policy.max_tokens, 64);
        assert_eq!(policy.max_input_len, 4096);
        assert_eq!(policy.goal_map, None);
    }
}
