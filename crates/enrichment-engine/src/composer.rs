//! Agent context composer.
//!
//! Renders a template string with `{{key}}` interpolation from facts.
//! Undefined keys yield empty strings.

use crate::models::Fact;

/// Composes a rendered string from a template and a list of facts.
pub struct AgentContextComposer;

impl AgentContextComposer {
    /// Render a template string, interpolating `{{key}}` placeholders
    /// with values from the provided facts.
    ///
    /// Keys not found in facts resolve to empty strings.
    pub fn compose(template: &str, facts: &[Fact]) -> String {
        let mut result = template.to_string();

        // Build a map from fact key -> value
        let fact_map: std::collections::HashMap<&str, &str> =
            facts.iter().map(|f| (f.key.as_str(), f.value.as_str())).collect();

        // Replace all {{key}} patterns
        let re = regex::Regex::new(r"\{\{(\w+)\}\}").expect("Invalid template regex");
        result = re
            .replace_all(&result, |caps: &regex::Captures| {
                let key = &caps[1];
                fact_map.get(key).copied().unwrap_or("")
            })
            .to_string();

        result
    }

    /// Extract a scalar fact value by key, returning None if not found.
    pub fn get_fact<'a>(facts: &'a [Fact], key: &str) -> Option<&'a str> {
        facts.iter().find(|f| f.key == key).map(|f| f.value.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Fact;

    fn fact(key: &str, value: &str) -> Fact {
        Fact {
            key: key.to_string(),
            value: value.to_string(),
            tags: Vec::new(),
            source_extractor: "test".to_string(),
            confidence: 1.0,
        }
    }

    #[test]
    fn test_valid_keys() {
        let facts = vec![fact("status", "SUCCESS"), fact("count", "42")];
        let template = "Build {{status}}. Count: {{count}}";
        let result = AgentContextComposer::compose(template, &facts);
        assert_eq!(result, "Build SUCCESS. Count: 42");
    }

    #[test]
    fn test_undefined_keys_yield_empty() {
        let facts = vec![fact("status", "SUCCESS")];
        let template = "Status: {{status}}, Missing: {{missing}}";
        let result = AgentContextComposer::compose(template, &facts);
        assert_eq!(result, "Status: SUCCESS, Missing: ");
    }

    #[test]
    fn test_multiple_facts() {
        let facts = vec![
            fact("status", "SUCCESS"),
            fact("artifacts_count", "3"),
            fact("tests_run", "10"),
            fact("tests_failed", "1"),
        ];
        let template = "Build {{status}}. Artifacts: {{artifacts_count}} JAR(s). Tests: {{tests_run}} run, {{tests_failed}} failed.";
        let result = AgentContextComposer::compose(template, &facts);
        assert_eq!(result, "Build SUCCESS. Artifacts: 3 JAR(s). Tests: 10 run, 1 failed.");
    }
}
