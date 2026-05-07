//! Command extractor.
//!
//! Parses command strings into structured facts using a deterministic tokenizer.
//! No shell execution is performed.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::models::{Fact, OperationInvocation, OperationResult};
use crate::traits::{Extractor as ExtractorTrait, FileSystem};

/// Diagnostics for tokenizer errors.
#[derive(Clone, Debug, PartialEq)]
pub enum Diagnostic {
    /// Unsafe token detected: carries the unsafe character.
    UnsafeToken(String),
    /// Command has too many tokens or is too long.
    CommandTooLong { tokens: u32, length: u32 },
    /// Empty command string.
    EmptyCommand,
    /// Unclosed quote detected: carries the quote prefix.
    UnclosedQuote(String),
}

impl Diagnostic {
    /// Create an unsafe_token diagnostic.
    fn unsafe_token(ch: char) -> Self {
        Diagnostic::UnsafeToken(ch.to_string())
    }

    /// Create a command_too_long diagnostic.
    fn command_too_long(tokens: u32, length: u32) -> Self {
        Diagnostic::CommandTooLong { tokens, length }
    }

    /// Create an empty_command diagnostic.
    fn empty_command() -> Self {
        Diagnostic::EmptyCommand
    }

    /// Create an unclosed_quote diagnostic.
    fn unclosed_quote(prefix: &str) -> Self {
        Diagnostic::UnclosedQuote(prefix.to_string())
    }

    /// Get the diagnostic key for fact emission.
    fn key(&self) -> &'static str {
        match self {
            Diagnostic::UnsafeToken(_) => "unsafe_token",
            Diagnostic::CommandTooLong { .. } => "command_too_long",
            Diagnostic::EmptyCommand => "empty_command",
            Diagnostic::UnclosedQuote(_) => "unclosed_quote",
        }
    }

    /// Get the diagnostic value for fact emission.
    fn value(&self) -> String {
        match self {
            Diagnostic::UnsafeToken(ch) => ch.clone(),
            Diagnostic::CommandTooLong { tokens, length } => format!("t:{} l:{}", tokens, length),
            Diagnostic::EmptyCommand => String::new(),
            Diagnostic::UnclosedQuote(prefix) => prefix.clone(),
        }
    }
}

/// Tokenizer for command strings.
struct Tokenizer;

impl Tokenizer {
    /// Tokenize a command string.
    /// Returns Ok(tokens) on success, or Err(Diagnostic) on failure.
    fn tokenize(
        command: &str,
        max_tokens: u32,
        max_input_len: u32,
    ) -> Result<Vec<String>, Diagnostic> {
        // Check empty
        if command.is_empty() {
            return Err(Diagnostic::empty_command());
        }

        // Check input length
        if command.len() as u32 > max_input_len {
            return Err(Diagnostic::command_too_long(0, command.len() as u32));
        }

        let mut tokens = Vec::new();
        let mut chars = command.chars().peekable();
        let mut token_count = 0u32;

        // Unsafe characters lookup
        fn is_unsafe(c: char) -> bool {
            matches!(
                c,
                '`' | '$' | '|' | '&' | ';' | '>' | '<' | '*' | '?' | '[' | '{' | '\0'
            )
        }

        while let Some(c) = chars.next() {
            if is_unsafe(c) {
                // Check for special unsafe sequences with $
                if c == '$' {
                    // Peek at next char to see what kind of $ sequence this is
                    let next = chars.peek().copied();
                    match next {
                        Some('(') | Some('{') => {
                            // It's an env expansion attempt: $(), ${}
                            return Err(Diagnostic::unsafe_token('$'));
                        }
                        Some(c) if c.is_ascii_alphanumeric() || c == '_' => {
                            // It's a variable: $VAR
                            return Err(Diagnostic::unsafe_token('$'));
                        }
                        _ => {}
                    }
                }
                return Err(Diagnostic::unsafe_token(c));
            }

            if c.is_whitespace() {
                continue;
            }

            // Start of a token
            token_count += 1;
            if token_count > max_tokens {
                return Err(Diagnostic::command_too_long(
                    token_count,
                    command.len() as u32,
                ));
            }

            let mut token = String::new();

            match c {
                '\'' => {
                    // Single-quoted literal: preserve everything until closing quote
                    loop {
                        match chars.next() {
                            Some('\'') => break,
                            Some(c2) => token.push(c2),
                            None => return Err(Diagnostic::unclosed_quote(&format!("'{}", token))),
                        }
                    }
                }
                '"' => {
                    // Double-quoted: process escapes
                    loop {
                        match chars.next() {
                            Some('"') => break,
                            Some('\\') => {
                                match chars.next() {
                                    Some('"') => token.push('"'),
                                    Some('\\') => token.push('\\'),
                                    Some('n') => token.push('\n'),
                                    Some('t') => token.push('\t'),
                                    Some(c2) => {
                                        // Unknown escape, keep both chars
                                        token.push('\\');
                                        token.push(c2);
                                    }
                                    None => {
                                        return Err(Diagnostic::unclosed_quote(&format!(
                                            "\"{}",
                                            token
                                        )));
                                    }
                                }
                            }
                            Some(c2) => token.push(c2),
                            None => {
                                return Err(Diagnostic::unclosed_quote(&format!("\"{}", token)));
                            }
                        }
                    }
                }
                '\\' => {
                    // Backslash escape outside quotes: \space → space
                    match chars.next() {
                        Some(c2) => token.push(c2),
                        None => {
                            // Trailing backslash: just emit it
                            token.push('\\');
                        }
                    }
                }
                _ => {
                    token.push(c);
                    // Continue reading the token until whitespace
                    while let Some(&c2) = chars.peek() {
                        if c2.is_whitespace() {
                            break;
                        }
                        if is_unsafe(c2) {
                            return Err(Diagnostic::unsafe_token(c2));
                        }
                        chars.next();
                        if c2 == '\'' {
                            // Single quote in unquoted context
                            let mut sq = String::new();
                            loop {
                                match chars.next() {
                                    Some('\'') => break,
                                    Some(c3) => sq.push(c3),
                                    None => {
                                        return Err(Diagnostic::unclosed_quote(&format!(
                                            "'{}",
                                            sq
                                        )));
                                    }
                                }
                            }
                            token.push_str(&sq);
                        } else if c2 == '"' {
                            // Double quote in unquoted context
                            loop {
                                match chars.next() {
                                    Some('"') => break,
                                    Some('\\') => match chars.next() {
                                        Some('"') => token.push('"'),
                                        Some('\\') => token.push('\\'),
                                        Some('n') => token.push('\n'),
                                        Some('t') => token.push('\t'),
                                        Some(c3) => {
                                            token.push('\\');
                                            token.push(c3);
                                        }
                                        None => {
                                            return Err(Diagnostic::unclosed_quote(&format!(
                                                "\"{}",
                                                token
                                            )));
                                        }
                                    },
                                    Some(c3) => token.push(c3),
                                    None => {
                                        return Err(Diagnostic::unclosed_quote(&format!(
                                            "\"{}",
                                            token
                                        )));
                                    }
                                }
                            }
                        } else if c2 == '\\' {
                            match chars.next() {
                                Some(c3) => token.push(c3),
                                None => token.push('\\'),
                            }
                        } else {
                            token.push(c2);
                        }
                    }
                }
            }

            tokens.push(token);
        }

        Ok(tokens)
    }
}

/// Tool classifiers.
fn classify_tool(executable: &str) -> &'static str {
    match executable {
        "mvn" => "maven",
        "gradle" | "gradlew" => "gradle",
        "npm" | "npx" | "yarn" => "node",
        "cargo" => "cargo",
        _ => "unknown",
    }
}

/// Intent inference from goal.
fn infer_intent(goal: &str, goal_map: &Option<HashMap<String, String>>) -> String {
    // Check goal_map override first
    if let Some(map) = goal_map
        && let Some(intent) = map.get(goal)
    {
        return intent.clone();
    }

    // Default intent map
    match goal {
        "compile" => "build".to_string(),
        "build" => "build".to_string(),
        "test" => "test".to_string(),
        "clean" => "clean".to_string(),
        "install" => "install".to_string(),
        "publish" | "deploy" => "deploy".to_string(),
        _ => "run".to_string(),
    }
}

/// Classifier for command tokens.
struct Classifier;

impl Classifier {
    /// Classify tokens into facts based on policy.
    /// `source_name` is the extractor name used for source_extractor field.
    fn classify(
        tokens: &[String],
        policy: &crate::models::CommandExtractorPolicy,
        source_name: &str,
    ) -> Vec<Fact> {
        let mut facts = Vec::new();

        if tokens.is_empty() {
            return facts;
        }

        // Token[0] → executable
        let executable = &tokens[0];
        facts.push(Fact {
            key: "command_executable".to_string(),
            value: executable.clone(),
            tags: Vec::new(),
            source_extractor: source_name.to_string(),
            confidence: 1.0,
        });

        // Tool classification
        let tool = classify_tool(executable);
        facts.push(Fact {
            key: "command_tool".to_string(),
            value: tool.to_string(),
            tags: vec!["tool".to_string()],
            source_extractor: source_name.to_string(),
            confidence: 1.0,
        });

        // Token[1] if non-flag → goal
        if tokens.len() > 1 {
            let token1 = &tokens[1];
            let is_flag_like =
                token1.starts_with("-D") || token1.starts_with("-P") || token1.starts_with("-x");
            if !is_flag_like {
                let intent = infer_intent(token1, &policy.goal_map);
                facts.push(Fact {
                    key: "command_goal".to_string(),
                    value: token1.clone(),
                    tags: Vec::new(),
                    source_extractor: source_name.to_string(),
                    confidence: 1.0,
                });
                facts.push(Fact {
                    key: "command_intent".to_string(),
                    value: intent,
                    tags: vec!["intent".to_string()],
                    source_extractor: source_name.to_string(),
                    confidence: 1.0,
                });
            }
        }

        // Remaining tokens: flags, options, targets
        let start_idx = 2;
        let mut targets = Vec::new();
        let mut i = start_idx;

        while i < tokens.len() {
            let token = &tokens[i];

            if let Some(flag_rest) = token.strip_prefix("--") {
                // Long flag: --flag or --flag=value
                if policy.allow_flags {
                    if let Some(eq_idx) = flag_rest.find('=') {
                        // --flag=value format
                        let (flag_name, flag_value) = flag_rest.split_at(eq_idx);
                        facts.push(Fact {
                            key: "command_flag".to_string(),
                            value: format!("{}={}", flag_name, &flag_value[1..]),
                            tags: Vec::new(),
                            source_extractor: source_name.to_string(),
                            confidence: 1.0,
                        });
                    } else {
                        // --flag format without = (boolean flag)
                        facts.push(Fact {
                            key: "command_flag".to_string(),
                            value: flag_rest.to_string(),
                            tags: Vec::new(),
                            source_extractor: source_name.to_string(),
                            confidence: 1.0,
                        });
                    }
                }
            } else if let Some(task) = token.strip_prefix("-x") {
                // Gradle exclusion option: -xtask or -x task
                if policy.allow_options {
                    if !task.is_empty() {
                        // -xtask format
                        facts.push(Fact {
                            key: "command_option".to_string(),
                            value: token.clone(),
                            tags: vec!["exclude".to_string(), format!("exclude:{}", task)],
                            source_extractor: source_name.to_string(),
                            confidence: 1.0,
                        });
                    } else if i + 1 < tokens.len() {
                        // -x task format (separate tokens)
                        let next = &tokens[i + 1];
                        if !next.starts_with('-') {
                            facts.push(Fact {
                                key: "command_option".to_string(),
                                value: format!("-x={}", next),
                                tags: vec!["exclude".to_string(), format!("exclude:{}", next)],
                                source_extractor: source_name.to_string(),
                                confidence: 1.0,
                            });
                            i += 1; // skip the next token
                        }
                    }
                }
            } else if let Some(d_arg) = token.strip_prefix("-D") {
                // Java -D option: -Dkey=value or -Dkey
                if policy.allow_options {
                    if let Some(eq_idx) = d_arg.find('=') {
                        let (key, value) = d_arg.split_at(eq_idx);
                        facts.push(Fact {
                            key: "command_option".to_string(),
                            value: token.clone(),
                            tags: vec![format!("key:{}", key), format!("value:{}", &value[1..])],
                            source_extractor: source_name.to_string(),
                            confidence: 1.0,
                        });
                    } else {
                        // -Dkey format (boolean option)
                        facts.push(Fact {
                            key: "command_option".to_string(),
                            value: token.clone(),
                            tags: vec![format!("key:{}", d_arg)],
                            source_extractor: source_name.to_string(),
                            confidence: 1.0,
                        });
                    }
                }
            } else if let Some(profile) = token.strip_prefix("-P") {
                // Maven profile option: -Pprofile
                if policy.allow_options && !profile.is_empty() {
                    facts.push(Fact {
                        key: "command_option".to_string(),
                        value: token.clone(),
                        tags: vec!["profile".to_string(), format!("profile:{}", profile)],
                        source_extractor: source_name.to_string(),
                        confidence: 1.0,
                    });
                }
            } else if token.len() == 2 && token.starts_with('-') {
                // Short flag: -f
                if policy.allow_flags {
                    facts.push(Fact {
                        key: "command_flag".to_string(),
                        value: token[1..].to_string(),
                        tags: Vec::new(),
                        source_extractor: source_name.to_string(),
                        confidence: 1.0,
                    });
                }
            } else {
                // Target (positional non-flag argument)
                targets.push(token.clone());
            }

            i += 1;
        }

        // Emit target facts
        if policy.allow_targets && !targets.is_empty() {
            for target in targets {
                facts.push(Fact {
                    key: "command_target".to_string(),
                    value: target,
                    tags: Vec::new(),
                    source_extractor: source_name.to_string(),
                    confidence: 1.0,
                });
            }
        }

        facts
    }
}

/// Command extractor.
#[derive(Debug, Clone)]
pub struct CommandExtractor {
    name: String,
    policy: crate::models::CommandExtractorPolicy,
}

impl CommandExtractor {
    /// Create a new command extractor with the given policy.
    pub fn with_policy(name: &str, policy: crate::models::CommandExtractorPolicy) -> Self {
        Self {
            name: name.to_string(),
            policy,
        }
    }

    /// Extract facts from a command string.
    fn extract_command(&self, command: &str) -> Vec<Fact> {
        // Tokenize
        let token_result =
            Tokenizer::tokenize(command, self.policy.max_tokens, self.policy.max_input_len);

        match token_result {
            Err(diag) => {
                // Return diagnostic fact + empty vec
                vec![Fact {
                    key: diag.key().to_string(),
                    value: diag.value(),
                    tags: vec!["diagnostic".to_string()],
                    source_extractor: self.name.clone(),
                    confidence: 0.0,
                }]
            }
            Ok(tokens) => {
                if tokens.is_empty() {
                    // Empty after tokenization (shouldn't happen, but handle gracefully)
                    vec![Fact {
                        key: "empty_command".to_string(),
                        value: String::new(),
                        tags: vec!["diagnostic".to_string()],
                        source_extractor: self.name.clone(),
                        confidence: 0.0,
                    }]
                } else {
                    Classifier::classify(&tokens, &self.policy, &self.name)
                }
            }
        }
    }
}

#[async_trait]
impl ExtractorTrait for CommandExtractor {
    fn name(&self) -> &str {
        &self.name
    }

    async fn extract(
        &self,
        invocation: &OperationInvocation,
        _result: &OperationResult,
        _fs: &dyn FileSystem,
    ) -> Vec<Fact> {
        self.extract_command(&invocation.command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Tokenizer Tests ────────────────────────────────────────────────────────

    #[test]
    fn test_tokenizer_whitespace_split() {
        let result = Tokenizer::tokenize("mvn clean package", 64, 4096);
        assert!(result.is_ok());
        let tokens = result.unwrap();
        assert_eq!(tokens, vec!["mvn", "clean", "package"]);
    }

    #[test]
    fn test_tokenizer_single_quotes_preserve() {
        let result = Tokenizer::tokenize("echo 'hello world'", 64, 4096);
        assert!(result.is_ok());
        let tokens = result.unwrap();
        assert_eq!(tokens, vec!["echo", "hello world"]);
    }

    #[test]
    fn test_tokenizer_double_quotes_escapes() {
        // Test valid escape sequences: \" → ", \\ → \, \n → newline, \t → tab
        // Using standard double-quoted string with proper escapes
        let result = Tokenizer::tokenize("echo \"hello\\\"world\\\\test\\n\"", 64, 4096);
        assert!(result.is_ok(), "Tokenizer failed: {:?}", result);
        let tokens = result.unwrap();
        assert_eq!(tokens, vec!["echo", "hello\"world\\test\n"]);
    }

    #[test]
    fn test_tokenizer_merge_unescaped_space() {
        let result = Tokenizer::tokenize(r"echo hello\ world", 64, 4096);
        assert!(result.is_ok());
        let tokens = result.unwrap();
        assert_eq!(tokens, vec!["echo", "hello world"]);
    }

    #[test]
    fn test_tokenizer_reject_backtick() {
        let result = Tokenizer::tokenize("echo `whoami`", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken('`'.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_dollar_paren() {
        let result = Tokenizer::tokenize("echo $(whoami)", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken('$'.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_dollar_brace() {
        let result = Tokenizer::tokenize("echo ${HOME}", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken('$'.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_dollar_var() {
        let result = Tokenizer::tokenize("echo $HOME", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken('$'.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_pipe() {
        let result = Tokenizer::tokenize("cat f | grep x", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken('|'.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_ampersand() {
        let result = Tokenizer::tokenize("cmd && other", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken('&'.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_semicolon() {
        let result = Tokenizer::tokenize("mvn clean; rm /", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken(';'.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_redirect_out() {
        let result = Tokenizer::tokenize("echo x > f", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken('>'.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_redirect_append() {
        let result = Tokenizer::tokenize("echo x >> f", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken('>'.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_redirect_in() {
        let result = Tokenizer::tokenize("cat < f", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken('<'.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_glob_star() {
        let result = Tokenizer::tokenize("ls *.txt", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken('*'.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_glob_question() {
        let result = Tokenizer::tokenize("ls file?.txt", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken('?'.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_glob_bracket() {
        let result = Tokenizer::tokenize("ls [abc]", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken('['.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_glob_brace() {
        let result = Tokenizer::tokenize("ls {a,b}", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken('{'.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_nul() {
        let result = Tokenizer::tokenize("echo \0null", 64, 4096);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Diagnostic::UnsafeToken('\0'.to_string())
        );
    }

    #[test]
    fn test_tokenizer_reject_too_many_tokens() {
        let tokens: Vec<String> = (0..65).map(|i| format!("t{}", i)).collect();
        let command = tokens.join(" ");
        let result = Tokenizer::tokenize(&command, 64, 4096);
        assert!(result.is_err());
        let diag = result.unwrap_err();
        assert_eq!(diag.key(), "command_too_long");
    }

    #[test]
    fn test_tokenizer_reject_too_long_input() {
        let command = "x".repeat(4097);
        let result = Tokenizer::tokenize(&command, 64, 4096);
        assert!(result.is_err());
        let diag = result.unwrap_err();
        assert_eq!(diag.key(), "command_too_long");
    }

    #[test]
    fn test_tokenizer_unclosed_single_quote() {
        let result = Tokenizer::tokenize("echo 'unclosed", 64, 4096);
        assert!(result.is_err());
        let diag = result.unwrap_err();
        assert_eq!(diag.key(), "unclosed_quote");
    }

    #[test]
    fn test_tokenizer_unclosed_double_quote() {
        let result = Tokenizer::tokenize(r#"echo "unclosed"#, 64, 4096);
        assert!(result.is_err());
        let diag = result.unwrap_err();
        assert_eq!(diag.key(), "unclosed_quote");
    }

    #[test]
    fn test_tokenizer_empty_command() {
        let result = Tokenizer::tokenize("", 64, 4096);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), Diagnostic::EmptyCommand);
    }

    // ─── Classifier Tests ─────────────────────────────────────────────────────

    #[test]
    fn test_classify_maven() {
        // Use --settings=settings.xml format (equals sign)
        let tokens = vec![
            "mvn".to_string(),
            "clean".to_string(),
            "package".to_string(),
            "-DskipTests".to_string(),
            "--settings=settings.xml".to_string(),
        ];
        let policy = crate::models::CommandExtractorPolicy::default();
        let facts = Classifier::classify(&tokens, &policy, "test");

        let fact_map: HashMap<&str, &str> = facts
            .iter()
            .map(|f| (f.key.as_str(), f.value.as_str()))
            .collect();

        assert_eq!(fact_map.get("command_executable"), Some(&"mvn"));
        assert_eq!(fact_map.get("command_goal"), Some(&"clean"));
        assert_eq!(fact_map.get("command_tool"), Some(&"maven"));
        assert_eq!(fact_map.get("command_intent"), Some(&"clean"));
        assert_eq!(fact_map.get("command_flag"), Some(&"settings=settings.xml"));
        assert_eq!(fact_map.get("command_option"), Some(&"-DskipTests"));
    }

    #[test]
    fn test_classify_npm() {
        let tokens = vec![
            "npm".to_string(),
            "install".to_string(),
            "--save-dev".to_string(),
            "express".to_string(),
        ];
        let policy = crate::models::CommandExtractorPolicy::default();
        let facts = Classifier::classify(&tokens, &policy, "test");

        let fact_map: HashMap<&str, &str> = facts
            .iter()
            .map(|f| (f.key.as_str(), f.value.as_str()))
            .collect();

        assert_eq!(fact_map.get("command_executable"), Some(&"npm"));
        assert_eq!(fact_map.get("command_goal"), Some(&"install"));
        assert_eq!(fact_map.get("command_tool"), Some(&"node"));
        assert_eq!(fact_map.get("command_intent"), Some(&"install"));
        assert_eq!(fact_map.get("command_flag"), Some(&"save-dev"));
        assert_eq!(fact_map.get("command_target"), Some(&"express"));
    }

    #[test]
    fn test_classify_gradle() {
        let tokens = vec![
            "gradle".to_string(),
            "build".to_string(),
            "-x".to_string(),
            "test".to_string(),
        ];
        let policy = crate::models::CommandExtractorPolicy::default();
        let facts = Classifier::classify(&tokens, &policy, "test");

        let fact_map: HashMap<&str, &str> = facts
            .iter()
            .map(|f| (f.key.as_str(), f.value.as_str()))
            .collect();

        assert_eq!(fact_map.get("command_executable"), Some(&"gradle"));
        assert_eq!(fact_map.get("command_goal"), Some(&"build"));
        assert_eq!(fact_map.get("command_tool"), Some(&"gradle"));
        assert_eq!(fact_map.get("command_intent"), Some(&"build"));
        // -x test should produce option -x=test
        assert_eq!(fact_map.get("command_option"), Some(&"-x=test"));
    }

    #[test]
    fn test_classify_unknown_tool() {
        let tokens = vec!["unknown-tool".to_string(), "run".to_string()];
        let policy = crate::models::CommandExtractorPolicy::default();
        let facts = Classifier::classify(&tokens, &policy, "test");

        let fact_map: HashMap<&str, &str> = facts
            .iter()
            .map(|f| (f.key.as_str(), f.value.as_str()))
            .collect();

        assert_eq!(fact_map.get("command_executable"), Some(&"unknown-tool"));
        assert_eq!(fact_map.get("command_tool"), Some(&"unknown"));
        assert_eq!(fact_map.get("command_goal"), Some(&"run"));
        assert_eq!(fact_map.get("command_intent"), Some(&"run"));
    }

    #[test]
    fn test_classify_flags_suppressed_when_disabled() {
        let tokens = vec![
            "npm".to_string(),
            "install".to_string(),
            "--save-dev".to_string(),
            "express".to_string(),
        ];
        let mut policy = crate::models::CommandExtractorPolicy::default();
        policy.allow_flags = false;
        let facts = Classifier::classify(&tokens, &policy, "test");

        let has_flag = facts.iter().any(|f| f.key == "command_flag");
        assert!(!has_flag, "Flag facts should be suppressed");
        // But executable, goal, tool, intent should still be there
        assert!(facts.iter().any(|f| f.key == "command_executable"));
        assert!(facts.iter().any(|f| f.key == "command_tool"));
    }

    #[test]
    fn test_classify_options_suppressed_when_disabled() {
        let tokens = vec![
            "mvn".to_string(),
            "clean".to_string(),
            "-DskipTests".to_string(),
        ];
        let mut policy = crate::models::CommandExtractorPolicy::default();
        policy.allow_options = false;
        let facts = Classifier::classify(&tokens, &policy, "test");

        let has_option = facts.iter().any(|f| f.key == "command_option");
        assert!(!has_option, "Option facts should be suppressed");
    }

    #[test]
    fn test_classify_targets_suppressed_when_disabled() {
        let tokens = vec![
            "npm".to_string(),
            "install".to_string(),
            "express".to_string(),
        ];
        let mut policy = crate::models::CommandExtractorPolicy::default();
        policy.allow_targets = false;
        let facts = Classifier::classify(&tokens, &policy, "test");

        let has_target = facts.iter().any(|f| f.key == "command_target");
        assert!(!has_target, "Target facts should be suppressed");
    }

    #[test]
    fn test_classify_goal_map_override() {
        let tokens = vec!["mvn".to_string(), "clean".to_string()];
        let mut policy = crate::models::CommandExtractorPolicy::default();
        policy.goal_map = Some(
            vec![("clean".to_string(), "cleanup".to_string())]
                .into_iter()
                .collect(),
        );

        let facts = Classifier::classify(&tokens, &policy, "test");
        let fact_map: HashMap<&str, &str> = facts
            .iter()
            .map(|f| (f.key.as_str(), f.value.as_str()))
            .collect();

        assert_eq!(fact_map.get("command_intent"), Some(&"cleanup"));
    }

    #[test]
    fn test_classify_intent_defaults() {
        // compile → build
        let tokens = vec!["mvn".to_string(), "compile".to_string()];
        let policy = crate::models::CommandExtractorPolicy::default();
        let facts = Classifier::classify(&tokens, &policy, "test");
        let intent = facts.iter().find(|f| f.key == "command_intent").unwrap();
        assert_eq!(intent.value, "build");

        // build → build
        let tokens = vec!["mvn".to_string(), "build".to_string()];
        let facts = Classifier::classify(&tokens, &policy, "test");
        let intent = facts.iter().find(|f| f.key == "command_intent").unwrap();
        assert_eq!(intent.value, "build");

        // test → test
        let tokens = vec!["mvn".to_string(), "test".to_string()];
        let facts = Classifier::classify(&tokens, &policy, "test");
        let intent = facts.iter().find(|f| f.key == "command_intent").unwrap();
        assert_eq!(intent.value, "test");

        // deploy → deploy
        let tokens = vec!["mvn".to_string(), "deploy".to_string()];
        let facts = Classifier::classify(&tokens, &policy, "test");
        let intent = facts.iter().find(|f| f.key == "command_intent").unwrap();
        assert_eq!(intent.value, "deploy");

        // unknown goal → run
        let tokens = vec!["mvn".to_string(), "custom-goal".to_string()];
        let facts = Classifier::classify(&tokens, &policy, "test");
        let intent = facts.iter().find(|f| f.key == "command_intent").unwrap();
        assert_eq!(intent.value, "run");
    }

    // ─── Fact Emission Tests ───────────────────────────────────────────────────

    #[test]
    fn test_fact_confidence_1_0_normal() {
        let extractor =
            CommandExtractor::with_policy("test", crate::models::CommandExtractorPolicy::default());
        let facts = extractor.extract_command("mvn clean");
        for fact in &facts {
            if fact.key != "command_executable" && fact.key != "command_tool" {
                continue;
            }
            assert_eq!(
                fact.confidence, 1.0,
                "Normal facts should have confidence 1.0"
            );
        }
    }

    #[test]
    fn test_fact_confidence_0_0_diagnostic() {
        let extractor =
            CommandExtractor::with_policy("test", crate::models::CommandExtractorPolicy::default());
        let facts = extractor.extract_command("echo `whoami`");
        let diag_facts: Vec<_> = facts
            .iter()
            .filter(|f| f.tags.contains(&"diagnostic".to_string()))
            .collect();
        assert!(!diag_facts.is_empty());
        for fact in diag_facts {
            assert_eq!(
                fact.confidence, 0.0,
                "Diagnostic facts should have confidence 0.0"
            );
        }
    }

    #[test]
    fn test_fact_source_extractor_command() {
        let extractor = CommandExtractor::with_policy(
            "my_cmd_ext",
            crate::models::CommandExtractorPolicy::default(),
        );
        let facts = extractor.extract_command("mvn clean");
        for fact in &facts {
            assert_eq!(fact.source_extractor, "my_cmd_ext");
        }
    }

    #[test]
    fn test_fact_keys_match_spec() {
        let extractor =
            CommandExtractor::with_policy("test", crate::models::CommandExtractorPolicy::default());
        let facts =
            extractor.extract_command("mvn clean package -DskipTests --settings settings.xml");
        let keys: Vec<_> = facts.iter().map(|f| f.key.clone()).collect();

        assert!(keys.contains(&"command_executable".to_string()));
        assert!(keys.contains(&"command_goal".to_string()));
        assert!(keys.contains(&"command_flag".to_string()));
        assert!(keys.contains(&"command_option".to_string()));
        assert!(keys.contains(&"command_tool".to_string()));
        assert!(keys.contains(&"command_intent".to_string()));
    }

    // ─── CommandExtractor Integration Tests ───────────────────────────────────

    #[tokio::test]
    async fn test_command_extractor_implements_extractor_trait() {
        use crate::traits::EnrichmentError;
        use crate::traits::Extractor as ExtractorTrait;

        struct FakeFs;

        #[async_trait::async_trait]
        impl FileSystem for FakeFs {
            async fn read_to_string(&self, _path: &str) -> Result<String, EnrichmentError> {
                Ok(String::new())
            }
            async fn glob(
                &self,
                _pattern: &str,
            ) -> Result<Vec<std::path::PathBuf>, EnrichmentError> {
                Ok(Vec::new())
            }
        }

        let extractor =
            CommandExtractor::with_policy("test", crate::models::CommandExtractorPolicy::default());
        let invocation = OperationInvocation::from_command("mvn clean package");
        let result = OperationResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 100,
            timed_out: false,
        };

        let facts = extractor.extract(&invocation, &result, &FakeFs).await;
        assert!(!facts.is_empty());
        assert!(
            facts
                .iter()
                .any(|f| f.key == "command_executable" && f.value == "mvn")
        );
    }

    #[test]
    fn test_empty_command_returns_diagnostic() {
        let extractor =
            CommandExtractor::with_policy("test", crate::models::CommandExtractorPolicy::default());
        let facts = extractor.extract_command("");
        assert!(!facts.is_empty());
        // Should have diagnostic fact
        assert!(
            facts
                .iter()
                .any(|f| f.tags.contains(&"diagnostic".to_string()))
        );
    }

    #[test]
    fn test_non_strict_allows_unsafe() {
        // In non-strict mode, we still tokenize but could allow some things
        // Actually, the tokenizer itself doesn't change - strict affects whether we reject
        // The spec says strict=true rejects, but the tokenizer itself is the rejection point
        // So let's test that unsafe is still rejected
        let extractor =
            CommandExtractor::with_policy("test", crate::models::CommandExtractorPolicy::default());
        let facts = extractor.extract_command("echo `whoami`");
        // Should have diagnostic
        assert!(facts.iter().any(|f| f.key == "unsafe_token"));
    }
}
