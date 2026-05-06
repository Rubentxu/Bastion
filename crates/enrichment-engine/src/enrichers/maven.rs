//! Maven enricher descriptor.
//!
//! The Maven enricher activates on `mvn` commands and extracts facts
//! about build status, test results, artifacts, and error diagnostics.

use crate::models::{EnricherDescriptor, ExtractorConfig};

/// Built-in Maven enricher descriptor.
pub fn maven_enricher() -> EnricherDescriptor {
    EnricherDescriptor {
        id: "maven".to_string(),
        name: "Maven".to_string(),
        version: "1.0".to_string(),
        match_patterns: vec![r"^mvn\s+(package|install|verify|test|compile|clean|deploy)".to_string()],
        template: "Build {{status}}. Artifacts: {{artifacts_count}} JAR(s). Tests: {{tests_run}} run, {{tests_failed}} failed. Coordinates: {{group_id}}:{{artifact_id}}:{{version}}".to_string(),
        enabled: true,
        extractors: vec![
            ExtractorConfig {
                id: "build_status".to_string(),
                extractor_type: "regex".to_string(),
                pattern: r"(?P<status>BUILD\s+(SUCCESS|FAILURE))".to_string(),
                fact_key: "build_status".to_string(),
                priority: 1,
                merge_mode: "single".to_string(),
            },
            ExtractorConfig {
                id: "maven_coords".to_string(),
                extractor_type: "regex".to_string(),
                pattern: r"Building\s+(?P<group_id>[\w.]+):(?P<artifact_id>[\w-]+):(?P<version>[\w.]+)".to_string(),
                fact_key: "maven_coords".to_string(),
                priority: 2,
                merge_mode: "single".to_string(),
            },
            ExtractorConfig {
                id: "test_results".to_string(),
                extractor_type: "regex".to_string(),
                pattern: r"Tests run:\s*(?P<tests_run>\d+),\s*Failures:\s*(?P<tests_failed>\d+),\s*Errors:\s*(?P<tests_errors>\d+),\s*Skipped:\s*(?P<tests_skipped>\d+)".to_string(),
                fact_key: "test_results".to_string(),
                priority: 3,
                merge_mode: "single".to_string(),
            },
            ExtractorConfig {
                id: "jar_artifacts".to_string(),
                extractor_type: "glob".to_string(),
                pattern: "target/*.jar".to_string(),
                fact_key: "jar_artifact".to_string(),
                priority: 4,
                merge_mode: "multi".to_string(),
            },
            ExtractorConfig {
                id: "war_artifacts".to_string(),
                extractor_type: "glob".to_string(),
                pattern: "target/*.war".to_string(),
                fact_key: "war_artifact".to_string(),
                priority: 5,
                merge_mode: "multi".to_string(),
            },
            ExtractorConfig {
                id: "error_diagnostics".to_string(),
                extractor_type: "regex".to_string(),
                pattern: r"(?P<error_msg>(error|ERROR|compilation failure|COMPILATION ERROR):.*)".to_string(),
                fact_key: "error_msg".to_string(),
                priority: 6,
                merge_mode: "single".to_string(),
            },
        ],
    }
}

/// All built-in enricher descriptors.
pub fn all_enrichers() -> Vec<EnricherDescriptor> {
    vec![maven_enricher()]
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_maven_success_regex() {
        let pattern = r"(?P<status>BUILD\s+(SUCCESS|FAILURE))";
        let re = regex::Regex::new(pattern).unwrap();
        let text = "BUILD SUCCESS";
        let caps = re.captures(text).unwrap();
        assert_eq!(caps.name("status").unwrap().as_str(), "BUILD SUCCESS");
    }

    #[test]
    fn test_maven_failure_regex() {
        let pattern = r"(?P<status>BUILD\s+(SUCCESS|FAILURE))";
        let re = regex::Regex::new(pattern).unwrap();
        let text = "BUILD FAILURE";
        let caps = re.captures(text).unwrap();
        assert_eq!(caps.name("status").unwrap().as_str(), "BUILD FAILURE");
    }

    #[test]
    fn test_test_count_parse() {
        let pattern = r"Tests run:\s*(?P<tests_run>\d+),\s*Failures:\s*(?P<tests_failed>\d+),\s*Errors:\s*(?P<tests_errors>\d+),\s*Skipped:\s*(?P<tests_skipped>\d+)";
        let re = regex::Regex::new(pattern).unwrap();
        let text = "Tests run: 10, Failures: 2, Errors: 0, Skipped: 1";
        let caps = re.captures(text).unwrap();
        assert_eq!(caps.name("tests_run").unwrap().as_str(), "10");
        assert_eq!(caps.name("tests_failed").unwrap().as_str(), "2");
    }

    #[test]
    fn test_non_maven_skip() {
        // Maven enricher should NOT match cargo commands
        let pattern = r"^mvn\s+(package|install|verify|test|compile|clean|deploy)";
        let re = regex::Regex::new(pattern).unwrap();
        assert!(!re.is_match("cargo build"));
        assert!(!re.is_match("gradle build"));
        assert!(!re.is_match("echo hello"));
    }
}
