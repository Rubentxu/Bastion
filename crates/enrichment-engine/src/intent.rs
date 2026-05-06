//! Intent detector.
//!
//! Pattern-matches a command string against enricher match_patterns
//! to determine which enricher(s) should be activated.

use crate::models::EnricherDescriptor;

/// Detects which enricher(s) match a given command string.
pub struct IntentDetector;

impl IntentDetector {
    /// Return enricher descriptors that match the given command.
    ///
    /// Matches by checking if any of the enricher's `match_patterns` regexes
    /// match the command string.
    pub fn detect<'a>(command: &str, enrichers: &'a [EnricherDescriptor]) -> Vec<&'a EnricherDescriptor> {
        enrichers
            .iter()
            .filter(|e| {
                e.enabled && e.match_patterns.iter().any(|p| {
                    regex::Regex::new(p)
                        .map(|re| re.is_match(command))
                        .unwrap_or(false)
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enricher(id: &str, patterns: Vec<&str>) -> EnricherDescriptor {
        EnricherDescriptor {
            id: id.to_string(),
            name: id.to_string(),
            version: "1.0".to_string(),
            match_patterns: patterns.into_iter().map(String::from).collect(),
            template: String::new(),
            enabled: true,
            extractors: Vec::new(),
        }
    }

    #[test]
    fn test_detect_maven_commands() {
        let enrichers = vec![
            enricher("maven", vec![r"^mvn\s+(package|install|verify|test|compile|clean|deploy)"]),
            enricher("gradle", vec![r"^gradle\s+\w+"]),
        ];

        assert_eq!(IntentDetector::detect("mvn package", &enrichers)[0].id, "maven");
        assert_eq!(IntentDetector::detect("gradle build", &enrichers)[0].id, "gradle");
        assert!(IntentDetector::detect("echo hello", &enrichers).is_empty());
    }

    #[test]
    fn test_disabled_enricher_not_matched() {
        let mut enricher = enricher("maven", vec![r"^mvn\s+"]);
        enricher.enabled = false;
        let enrichers = vec![enricher];
        assert!(IntentDetector::detect("mvn package", &enrichers).is_empty());
    }
}
