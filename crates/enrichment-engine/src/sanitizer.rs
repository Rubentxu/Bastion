//! Command sanitizer for deterministic secret redaction.
//!
//! Redacts known secret patterns before commands are persisted to the
//! enrichment runs database. Uses a single O(n) pass with no regex.
//!
//! # Privacy
//!
//! This module ensures that commands stored in `enrichment_runs.db` do not
//! contain raw tokens, passwords, API keys, or other sensitive values.

/// Redact known secret patterns from a command string.
///
/// Applies deterministic redaction in a single forward pass over the input.
/// No regex is used — pattern matching is done via direct character comparison
/// for O(n) time complexity.
///
/// # Redacted Patterns
///
/// - `token=<value>` → `token=[REDACTED]`
/// - `password=<value>` → `password=[REDACTED]`
/// - `api_key=<value>` → `api_key=[REDACTED]`
/// - `Bearer <token>` → `Bearer [REDACTED]`
/// - `AKIA*` (AWS key prefix) → full key redacted until whitespace
/// - `KEY=<value>` where KEY is uppercase-snakecase → `KEY=[REDACTED]`
///
/// # Arguments
///
/// * `input` - The raw command string to sanitize
///
/// # Returns
///
/// A new string with secret values replaced by `[REDACTED]`. The original
/// string is not modified.
///
/// # Examples
///
/// ```
/// use enrichment_engine::sanitize_command;
///
/// // Token redaction
/// assert_eq!(
///     sanitize_command("curl -H 'Authorization: Bearer abc123'"),
///     "curl -H 'Authorization: Bearer [REDACTED]'"
/// );
///
/// // API key redaction
/// assert_eq!(
///     sanitize_command("api_key=sk-12345&model=gpt-4"),
///     "api_key=[REDACTED]&model=gpt-4"
/// );
///
/// // No secrets — identity transform
/// assert_eq!(sanitize_command("ls -la /tmp"), "ls -la /tmp");
/// ```
pub fn sanitize_command(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut result = String::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        // Check for assignment patterns: token=, password=, api_key=
        if let Some((pattern_len, _, value_end)) = check_assignment_pattern(bytes, i) {
            // Output the key part (without '='), then '=', then replacement
            result.push_str(&input[i..i + pattern_len]);
            result.push('=');
            result.push_str("[REDACTED]");
            i = value_end; // Skip to end of original value
            continue;
        }

        // Check for Bearer token pattern
        if let Some(bearer_end) = check_bearer_pattern(bytes, i) {
            // Output "Bearer " prefix and replacement
            result.push_str("Bearer ");
            result.push_str("[REDACTED]");
            // Skip past the token value (everything until whitespace, quote, or end)
            i = bearer_end;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'\'' {
                // Skip the token bytes without outputting
                i += 1;
            }
            // NOTE: Do NOT skip the closing quote here - for Bearer tokens inside quotes,
            // the quote is part of the string structure and should be output normally.
            // Only skip the whitespace that follows the Bearer token.
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            continue;
        }

        // Check for AWS AKIA key pattern
        if let Some((start, end)) = check_aws_key_pattern(bytes, i) {
            result.push_str(&input[i..start]);
            result.push_str("[REDACTED]");
            i = end;
            continue;
        }

        // Check for uppercase_snakecase=VALUE pattern
        if let Some((key_len, _, value_end)) = check_snake_key_pattern(bytes, i) {
            // Output the key part, then '=', then replacement
            result.push_str(&input[i..i + key_len]);
            result.push('=');
            result.push_str("[REDACTED]");
            i = value_end;
            continue;
        }

        // No pattern matched — copy byte as-is
        result.push(bytes[i] as char);
        i += 1;
    }

    result
}

/// Check if current position starts an assignment pattern (token=, password=, api_key=).
/// Returns (pattern_len, value_start, value_end) if found, None otherwise.
/// pattern_len is the length of the key part (e.g., 5 for "token").
/// value_start is the position of '=' (pattern_len).
/// value_end is the position after the value ends (past closing quote if value is quoted).
fn check_assignment_pattern(bytes: &[u8], i: usize) -> Option<(usize, usize, usize)> {
    // Check each pattern individually
    let patterns = [
        ("token=", 5usize),   // "token" = 5 chars
        ("password=", 8usize), // "password" = 8 chars
        ("api_key=", 7usize),  // "api_key" = 7 chars
    ];

    for (pattern_str, key_len) in &patterns {
        let pattern_bytes = pattern_str.as_bytes();
        let pattern_total_len = pattern_bytes.len(); // includes '='
        if i + pattern_total_len <= bytes.len() && &bytes[i..i + pattern_total_len] == pattern_bytes {
            // Find the end of the value (whitespace, quote, comma, or end of string)
            let value_start = i + pattern_total_len; // position of '='
            let mut value_end = value_start;
            while value_end < bytes.len() {
                let b = bytes[value_end];
                if b == b' ' || b == b'\t' || b == b'\'' || b == b'"' || b == b',' || b == b'&' || b == b'\n' || b == b'\r' {
                    break;
                }
                value_end += 1;
            }
            // If value starts with a quote, skip to after the closing quote
            if value_end < bytes.len() && (bytes[value_end] == b'\'' || bytes[value_end] == b'"') {
                let quote = bytes[value_end];
                value_end += 1; // skip opening quote
                while value_end < bytes.len() && bytes[value_end] != quote {
                    value_end += 1;
                }
                if value_end < bytes.len() {
                    value_end += 1; // skip closing quote
                }
            }
            return Some((*key_len, value_start, value_end));
        }
    }
    None
}

/// Check if current position starts a "Bearer " token pattern.
/// Returns the position after "Bearer " if found, None otherwise.
fn check_bearer_pattern(bytes: &[u8], i: usize) -> Option<usize> {
    let bearer = b"Bearer ";
    let bearer_len = bearer.len();
    if i + bearer_len <= bytes.len() && &bytes[i..i + bearer_len] == bearer {
        // Return position after "Bearer "
        Some(i + bearer_len)
    } else {
        None
    }
}

/// Check if current position starts an AWS AKIA key pattern.
/// Returns (start, end) positions of the key if found, None otherwise.
fn check_aws_key_pattern(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    // AWS access key IDs start with "AKIA" followed by 16 characters
    if i + 4 > bytes.len() {
        return None;
    }
    if &bytes[i..i + 4] != b"AKIA" {
        return None;
    }

    // Find the end of the key (whitespace or end of string)
    let start = i;
    let mut end = i + 4; // Start after AKIA prefix
    while end < bytes.len() {
        let b = bytes[end];
        if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' || b == b'\'' || b == b'"' || b == b',' {
            break;
        }
        end += 1;
    }

    // Only match if key has reasonable length (AKIA + at least 4 chars)
    if end - start >= 8 {
        Some((start, end))
    } else {
        None
    }
}

/// Check if current position starts an uppercase_snakecase= pattern.
/// e.g., "API_KEY=value", "MY_SECRET=123"
/// Returns (key_len, value_start, value_end) if found, None otherwise.
fn check_snake_key_pattern(bytes: &[u8], i: usize) -> Option<(usize, usize, usize)> {
    // Must have at least one char before '='
    if i >= bytes.len() || bytes[i] == b'=' {
        return None;
    }

    // Find the '=' position
    let mut eq_pos = i;
    while eq_pos < bytes.len() && bytes[eq_pos] != b'=' {
        eq_pos += 1;
    }

    if eq_pos >= bytes.len() || bytes[eq_pos] != b'=' {
        return None;
    }

    // Check if the key part is uppercase snakecase (e.g., API_KEY, MY_SECRET, TOKEN)
    let key_bytes = &bytes[i..eq_pos];
    if key_bytes.is_empty() || key_bytes.len() > 64 {
        return None;
    }

    // Uppercase snakecase: starts with uppercase, contains only A-Z, 0-9, underscore
    let mut has_upper = false;

    for (idx, &b) in key_bytes.iter().enumerate() {
        match b {
            b'A'..=b'Z' => {
                has_upper = true;
            }
            b'_' => {}
            b'0'..=b'9' => {
                // Digits allowed but not at start
                if idx == 0 {
                    return None;
                }
            }
            _ => {
                return None;
            }
        }
    }

    // Must have at least one uppercase and be a reasonable key
    if has_upper && key_bytes.len() >= 3 {
        // Find the end of the value (similar to assignment pattern)
        let value_start = eq_pos + 1;
        let mut value_end = value_start;
        while value_end < bytes.len() {
            let b = bytes[value_end];
            if b == b' ' || b == b'\t' || b == b'\'' || b == b'"' || b == b',' || b == b'&' || b == b'\n' {
                break;
            }
            value_end += 1;
        }
        Some((key_bytes.len(), value_start, value_end))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── token= pattern ─────────────────────────────────────────────────────────

    #[test]
    fn sanitize_token_eq_value() {
        assert_eq!(
            sanitize_command("token=secret123"),
            "token=[REDACTED]"
        );
    }

    #[test]
    fn sanitize_token_eq_quoted_value() {
        assert_eq!(
            sanitize_command("token='Bearer abc123'"),
            "token=[REDACTED]"
        );
    }

    #[test]
    fn sanitize_token_with_suffix() {
        assert_eq!(
            sanitize_command("mytoken=sk-12345"),
            "mytoken=[REDACTED]"
        );
    }

    // ─── password= pattern ───────────────────────────────────────────────────────

    #[test]
    fn sanitize_password_eq_value() {
        assert_eq!(
            sanitize_command("password=P@ssw0rd!"),
            "password=[REDACTED]"
        );
    }

    #[test]
    fn sanitize_password_with_env() {
        assert_eq!(
            sanitize_command("DB_PASSWORD=secret123"),
            "DB_PASSWORD=[REDACTED]"
        );
    }

    // ─── api_key= pattern ───────────────────────────────────────────────────────

    #[test]
    fn sanitize_api_key_eq_value() {
        assert_eq!(
            sanitize_command("api_key=sk-12345&model=gpt-4"),
            "api_key=[REDACTED]&model=gpt-4"
        );
    }

    #[test]
    fn sanitize_api_key_with_prefix() {
        assert_eq!(
            sanitize_command("OPENAI_API_KEY=sk-12345"),
            "OPENAI_API_KEY=[REDACTED]"
        );
    }

    // ─── Bearer token pattern ───────────────────────────────────────────────────

    #[test]
    fn sanitize_bearer_token() {
        assert_eq!(
            sanitize_command("curl -H 'Authorization: Bearer abc123'"),
            "curl -H 'Authorization: Bearer [REDACTED]'"
        );
    }

    #[test]
    fn sanitize_bearer_alone() {
        assert_eq!(
            sanitize_command("Bearer abc123"),
            "Bearer [REDACTED]"
        );
    }

    // ─── AWS AKIA key pattern ──────────────────────────────────────────────────

    #[test]
    fn sanitize_aws_access_key() {
        // AKIA followed by 16 characters
        assert_eq!(
            sanitize_command("AKIAIOSFODNN7EXAMPLE"),
            "[REDACTED]"
        );
    }

    #[test]
    fn sanitize_aws_key_in_context() {
        assert_eq!(
            sanitize_command("aws_access_key_id=AKIAIOSFODNN7EXAMPLE"),
            "aws_access_key_id=[REDACTED]"
        );
    }

    // ─── Uppercase snakecase= pattern ──────────────────────────────────────────

    #[test]
    fn sanitize_uppercase_snake_key() {
        assert_eq!(
            sanitize_command("API_KEY=secret"),
            "API_KEY=[REDACTED]"
        );
    }

    #[test]
    fn sanitize_multiple_snake_keys() {
        assert_eq!(
            sanitize_command("TOKEN=value&PASSWORD=pass"),
            "TOKEN=[REDACTED]&PASSWORD=[REDACTED]"
        );
    }

    #[test]
    fn sanitize_mixed_case_not_redacted() {
        // Mixed case should NOT be redacted (not uppercase snakecase)
        assert_eq!(
            sanitize_command("myToken=secret"),
            "myToken=secret"
        );
    }

    // Note: api_key=secret is redacted per spec (api_key= is a secret pattern)
    // regardless of the value. Only AKIA* has case-sensitive prefix matching.

    // ─── No secrets — identity transform ───────────────────────────────────────

    #[test]
    fn sanitize_no_secrets() {
        assert_eq!(sanitize_command("ls -la /tmp"), "ls -la /tmp");
    }

    #[test]
    fn sanitize_empty_string() {
        assert_eq!(sanitize_command(""), "");
    }

    #[test]
    fn sanitize_command_only() {
        assert_eq!(
            sanitize_command("mvn package -DskipTests"),
            "mvn package -DskipTests"
        );
    }

    // ─── Determinism ────────────────────────────────────────────────────────────

    #[test]
    fn sanitize_deterministic_same_input() {
        let input = "token=mypassword api_key=sk-12345";
        assert_eq!(sanitize_command(input), sanitize_command(input));
    }

    #[test]
    fn sanitize_deterministic_multiple_runs() {
        let input = "Bearer abc123";
        for _ in 0..100 {
            assert_eq!(sanitize_command(input), "Bearer [REDACTED]");
        }
    }

    // ─── Complex cases ───────────────────────────────────────────────────────────

    #[test]
    fn sanitize_multiple_patterns_same_line() {
        assert_eq!(
            sanitize_command("token=secret api_key=sk-12345 Bearer abc123"),
            "token=[REDACTED] api_key=[REDACTED] Bearer [REDACTED]"
        );
    }

    #[test]
    fn sanitize_preserves_non_secret_content() {
        assert_eq!(
            sanitize_command("curl https://api.example.com?token=secret&limit=10"),
            "curl https://api.example.com?token=[REDACTED]&limit=10"
        );
    }

    #[test]
    fn sanitize_key_with_underscore_middle() {
        // FOO_BAR=baz should be redacted
        assert_eq!(
            sanitize_command("FOO_BAR=baz"),
            "FOO_BAR=[REDACTED]"
        );
    }

    #[test]
    fn sanitize_nested_equals() {
        // url=https://foo=bar should redact value until whitespace
        assert_eq!(
            sanitize_command("url=https://example.com?token=secret"),
            "url=https://example.com?token=[REDACTED]"
        );
    }
}