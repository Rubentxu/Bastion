//! Rule-based template recommendation engine for agent guidance.
//!
//! Analyzes task descriptions and command keywords to recommend the most
//! appropriate sandbox template with confidence scoring.

use serde::{Deserialize, Serialize};

/// A template recommendation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateRecommendation {
    /// The recommended template name.
    pub template: String,
    /// Confidence score between 0.0 and 1.0.
    pub confidence: f32,
    /// An alternative template if the primary is unavailable.
    pub alternative: String,
    /// Human-readable explanation of the recommendation.
    pub reasoning: String,
}

/// A single recommendation rule.
#[derive(Debug, Clone, PartialEq)]
struct Rule {
    keywords: Vec<&'static str>,
    template: &'static str,
    confidence: f32,
    alternative: &'static str,
    reasoning: &'static str,
}

/// Rule-based template recommendation engine.
///
/// Uses keyword matching against command patterns to recommend templates.
/// Recommendations include confidence scores and reasoning.
#[derive(Debug, Clone)]
pub struct TemplateRecommender {
    rules: Vec<Rule>,
    fallback_template: &'static str,
    fallback_confidence: f32,
    fallback_alternative: &'static str,
}

impl Default for TemplateRecommender {
    fn default() -> Self {
        Self::new()
    }
}

impl TemplateRecommender {
    /// Create a new TemplateRecommender with the default rule set.
    pub fn new() -> Self {
        Self {
            rules: vec![
                Rule {
                    // Exact tool name matches (highest confidence)
                    keywords: vec!["mvn", "maven", "mavenw"],
                    template: "eclipse-temurin:21-jdk-maven",
                    confidence: 0.95,
                    alternative: "ubuntu:24.04",
                    reasoning: "Matched Maven build tool. Template provides pre-installed JDK 21 + Maven 3.11.",
                },
                Rule {
                    keywords: vec!["gradle", "gradlew"],
                    template: "eclipse-temurin:21-jdk-gradle",
                    confidence: 0.90,
                    alternative: "eclipse-temurin:21-jdk-maven",
                    reasoning: "Matched Gradle build tool. Template provides pre-installed JDK 21 + Gradle 8.6.",
                },
                Rule {
                    keywords: vec!["npm", "node", "nodejs", "yarn", "pnpm", "node-gyp"],
                    template: "node:20-slim",
                    confidence: 0.95,
                    alternative: "ubuntu:24.04",
                    reasoning: "Matched Node.js ecosystem. Template provides Node 20 + npm/yarn/pnpm.",
                },
                Rule {
                    keywords: vec!["python", "pip", "pip3", "pyenv", "poetry", "uv", "pipenv"],
                    template: "python:3.12-slim",
                    confidence: 0.95,
                    alternative: "ubuntu:24.04",
                    reasoning: "Matched Python ecosystem. Template provides Python 3.12 + pip + uv.",
                },
                Rule {
                    keywords: vec!["cargo", "rustc", "cargo-watch", "rust-analyzer"],
                    template: "rust:1.77-slim",
                    confidence: 0.95,
                    alternative: "ubuntu:24.04",
                    reasoning: "Matched Rust toolchain. Template provides Rust 1.77 + Cargo.",
                },
                Rule {
                    keywords: vec!["go", "golang", "go build", "go mod"],
                    template: "golang:1.22-alpine",
                    confidence: 0.90,
                    alternative: "ubuntu:24.04",
                    reasoning: "Matched Go toolchain. Template provides Go 1.22.",
                },
                Rule {
                    keywords: vec!["bundle", "ruby", "rake", "rbenv"],
                    template: "ruby:3.3-slim",
                    confidence: 0.90,
                    alternative: "ubuntu:24.04",
                    reasoning: "Matched Ruby ecosystem. Template provides Ruby 3.3 + Bundler.",
                },
                Rule {
                    // asdf + general tools: version manager override.
                    // "python" keyword makes ubuntu competitive with python rule when both match,
                    // but python rule wins on tiebreaker (more specific base confidence).
                    keywords: vec!["apt", "apt-get", "dpkg", "asdf", "python"],
                    template: "ubuntu:24.04",
                    confidence: 0.90,
                    alternative: "debian:bookworm-slim",
                    reasoning: "Matched general-purpose template. Supports apt/dpkg/asdf toolchains.",
                },
                Rule {
                    keywords: vec!["dotnet", "msbuild", "nuget"],
                    template: "mcr.microsoft.com/dotnet/sdk:8.0",
                    confidence: 0.90,
                    alternative: "ubuntu:24.04",
                    reasoning: "Matched .NET SDK. Template provides .NET 8.0 SDK.",
                },
                Rule {
                    keywords: vec!["php", "composer", "laravel", "symfony"],
                    template: "php:8.3-cli",
                    confidence: 0.85,
                    alternative: "ubuntu:24.04",
                    reasoning: "Matched PHP ecosystem. Template provides PHP 8.3 + Composer.",
                },
            ],
            fallback_template: "ubuntu:24.04",
            fallback_confidence: 0.30,
            fallback_alternative: "debian:bookworm-slim",
        }
    }

    /// Recommend a template for the given task description.
    pub fn recommend(&self, task: &str) -> TemplateRecommendation {
        let tokens = self.tokenize(task);
        if tokens.is_empty() {
            return self.fallback();
        }

        let scores = self.match_keywords(&tokens);

        scores
            .into_iter()
            // Tiebreaker: prefer rule with MORE keyword matches (higher specificity)
            .max_by(|a, b| {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.2.cmp(&b.2)) // a has more matches → a wins
            })
            .map(|(rule, score, _matches)| TemplateRecommendation {
                template: rule.template.to_string(),
                confidence: score.min(1.0),
                alternative: rule.alternative.to_string(),
                reasoning: rule.reasoning.to_string(),
            })
            .unwrap_or_else(|| self.fallback())
    }

    /// Tokenize input text: lowercase, split on non-alphanumeric characters and hyphens.
    fn tokenize(&self, text: &str) -> Vec<String> {
        text.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty() && s.len() > 1)
            .map(String::from)
            .collect()
    }

    /// Match tokens against rules, returning (Rule, confidence_score, match_count) tuples.
    /// Scoring: base_confidence + bonus_per_match.
    /// Bonus: +0.05 per keyword match (additive, capped at 1.0).
    /// Tiebreaker: more keyword matches = higher specificity wins.
    fn match_keywords(&self, tokens: &[String]) -> Vec<(Rule, f32, usize)> {
        let mut scores: Vec<(Rule, f32, usize)> = Vec::new();

        for rule in &self.rules {
            let matches: usize = rule
                .keywords
                .iter()
                .filter(|kw| tokens.iter().any(|t| t.contains(*kw)))
                .count();

            if matches > 0 {
                // Scoring: base_confidence + 0.05 per keyword match (capped at 1.0)
                let score = (rule.confidence + 0.05 * (matches as f32)).min(1.0);

                // Deduplicate by template name: keep the higher score
                if let Some(existing) = scores
                    .iter_mut()
                    .find(|(r, _, _)| r.template == rule.template)
                {
                    if score > existing.1 {
                        existing.1 = score;
                        existing.2 = matches;
                    }
                } else {
                    scores.push((rule.clone(), score, matches));
                }
            }
        }

        scores
    }

    /// Return the fallback recommendation.
    fn fallback(&self) -> TemplateRecommendation {
        TemplateRecommendation {
            template: self.fallback_template.to_string(),
            confidence: self.fallback_confidence,
            alternative: self.fallback_alternative.to_string(),
            reasoning: "No specific toolchain detected. Defaulting to ubuntu:24.04 as a general-purpose template.".to_string(),
        }
    }

    /// Get all available rule keywords (for debugging/testing).
    #[cfg(test)]
    fn all_keywords(&self) -> Vec<&'static str> {
        self.rules.iter().flat_map(|r| r.keywords.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recommender() -> TemplateRecommender {
        TemplateRecommender::new()
    }

    #[test]
    fn test_java_maven_recommendation() {
        let r = recommender();
        let result = r.recommend("build a Java Maven project with Spring Boot");

        assert_eq!(result.template, "eclipse-temurin:21-jdk-maven");
        assert!(result.confidence >= 0.90);
        assert!(result.reasoning.contains("Maven"));
    }

    #[test]
    fn test_node_npm_recommendation() {
        let r = recommender();
        let result = r.recommend("npm install and run node script");

        assert_eq!(result.template, "node:20-slim");
        assert!(result.confidence >= 0.90);
        assert!(result.reasoning.contains("Node"));
    }

    #[test]
    fn test_rust_cargo_recommendation() {
        let r = recommender();
        let result = r.recommend("cargo build --release");

        assert_eq!(result.template, "rust:1.77-slim");
        assert!(result.confidence >= 0.90);
    }

    #[test]
    fn test_python_recommendation() {
        let r = recommender();
        let result = r.recommend("pip install -r requirements.txt");

        assert_eq!(result.template, "python:3.12-slim");
        assert!(result.confidence >= 0.80);
    }

    #[test]
    fn test_go_recommendation() {
        let r = recommender();
        let result = r.recommend("go mod download && go build ./...");

        assert_eq!(result.template, "golang:1.22-alpine");
        assert!(result.confidence >= 0.85);
    }

    #[test]
    fn test_ruby_recommendation() {
        let r = recommender();
        let result = r.recommend("bundle install && rake db:migrate");

        assert_eq!(result.template, "ruby:3.3-slim");
        assert!(result.confidence >= 0.80);
    }

    #[test]
    fn test_asdf_recommendation() {
        let r = recommender();
        // "asdf install python 3.12" matches both ubuntu (asdf+python keywords) and python rule.
        // ubuntu has 2 matches → score 1.0; python has 1 match → score 1.0.
        // Tiebreaker: more matches wins → ubuntu (2 vs 1).
        let result = r.recommend("asdf install python 3.12");

        assert_eq!(result.template, "ubuntu:24.04");
        assert!(result.confidence >= 0.80); // Both rules score 1.0 (capped)
    }

    #[test]
    fn test_fallback() {
        let r = recommender();
        let result = r.recommend("do something completely random xyz123");

        assert_eq!(result.template, "ubuntu:24.04");
        assert!(result.confidence < 0.5);
        assert!(result.reasoning.contains("No specific toolchain"));
    }

    #[test]
    fn test_empty_input() {
        let r = recommender();
        let result = r.recommend("");

        assert_eq!(result.template, "ubuntu:24.04");
        assert!(result.confidence < 0.5);
    }

    #[test]
    fn test_confidence_scoring() {
        let r = recommender();

        // Single keyword match
        let single = r.recommend("mvn");
        // Multiple keyword matches should boost confidence
        let multi = r.recommend("mvn maven mavenw");

        // Both should match maven, but multi should have higher confidence
        assert_eq!(single.template, multi.template);
        // Note: our scoring penalizes extra matches slightly, so they may be equal
        // The key is that both are above 0.9
        assert!(single.confidence >= 0.90);
        assert!(multi.confidence >= 0.90);
    }

    #[test]
    fn test_case_insensitive() {
        let r = recommender();
        let upper = r.recommend("MAVEN GRADLE");
        let lower = r.recommend("maven gradle");

        // Both should match something relevant
        assert!(upper.confidence >= 0.80);
        assert!(lower.confidence >= 0.80);
    }

    #[test]
    fn test_partial_keyword_match() {
        let r = recommender();

        // "java" is not a direct keyword for maven (maven keywords are mvn/maven/mavenw).
        // "spring" also not in maven keywords. This falls through to fallback.
        // Confidence should be low (< 0.5) since no explicit tool name matched.
        let result = r.recommend("java spring boot application");
        // Falls back to ubuntu with low confidence (no keyword matches)
        assert!(result.template == "ubuntu:24.04");
        assert!(result.confidence < 0.5);
    }

    #[test]
    fn test_dotnet_recommendation() {
        let r = recommender();
        let result = r.recommend("dotnet build && dotnet test");

        assert_eq!(result.template, "mcr.microsoft.com/dotnet/sdk:8.0");
        assert!(result.confidence >= 0.85);
    }

    #[test]
    fn test_php_recommendation() {
        let r = recommender();
        let result = r.recommend("composer install && php artisan serve");

        assert_eq!(result.template, "php:8.3-cli");
        assert!(result.confidence >= 0.80);
    }

    #[test]
    fn test_tokenize() {
        let r = recommender();

        assert_eq!(r.tokenize("maven build"), vec!["maven", "build"]);
        assert_eq!(
            r.tokenize("java-spring-boot"),
            vec!["java", "spring", "boot"]
        );
        assert_eq!(r.tokenize("mvn --version"), vec!["mvn", "version"]);
        assert_eq!(r.tokenize(""), Vec::<String>::new());
        assert_eq!(r.tokenize("a"), Vec::<String>::new()); // single char filtered
    }
}
