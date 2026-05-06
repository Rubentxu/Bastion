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
}

fn default_merge_mode() -> String {
    "single".to_string()
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
}

fn default_enabled() -> bool {
    true
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
        assert_eq!(round_tripped.goal_map.as_ref().unwrap().get("clean"), Some(&"cleanup".to_string()));
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
