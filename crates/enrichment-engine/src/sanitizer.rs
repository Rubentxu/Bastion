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

        // Check for GitHub token patterns (ghp_, gho_, ght_, ghs_)
        if let Some((start, end)) = check_github_token(bytes, i) {
            result.push_str(&input[i..start]);
            result.push_str("[REDACTED]");
            i = end;
            continue;
        }

        // Check for OpenAI key patterns (sk-, sk-proj-, sk-svcacct-, sk-admin-)
        if let Some((start, end)) = check_openai_key(bytes, i) {
            result.push_str(&input[i..start]);
            result.push_str("[REDACTED]");
            i = end;
            continue;
        }

        // Check for CLI secret flag patterns (--secret, --api-key)
        // These need special handling since the flag prefix should be preserved
        if let Some(flag_end) = check_cli_secret_flag(bytes, i) {
            // flag_end is the position after "--secret " or "--api-key "
            result.push_str(&input[i..flag_end]);
            result.push_str("[REDACTED]");
            i = flag_end;
            // Skip past the value (already included in the replacement)
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            continue;
        }

        // Check for JWT prefix pattern (eyJ with x.y.z structure)
        if let Some((start, end)) = check_jwt_prefix(bytes, i) {
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

        // Check for lowercase secret suffix pattern (_key, _token, _secret, _password)
        // This is checked LAST to give explicit patterns and uppercase snake priority
        if let Some((key_len, _, value_end)) = check_lowercase_secret_suffix(bytes, i) {
            result.push_str(&input[i..i + key_len]);
            result.push('=');
            result.push_str("[REDACTED]");
            i = value_end;
            continue;
        }

        // No pattern matched — copy UTF-8 character as-is
        // Determine character byte length to preserve multi-byte sequences
        let char_len = if bytes[i] < 0x80 {
            1 // ASCII
        } else if bytes[i] < 0xE0 {
            2 // 2-byte UTF-8 sequence
        } else if bytes[i] < 0xF0 {
            3 // 3-byte UTF-8 sequence
        } else {
            4 // 4-byte UTF-8 sequence (emoji, etc.)
        };
        result.push_str(&input[i..i + char_len]);
        i += char_len;
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
        ("token=", 5usize),    // "token" = 5 chars
        ("password=", 8usize), // "password" = 8 chars
        ("api_key=", 7usize),  // "api_key" = 7 chars
    ];

    for (pattern_str, key_len) in &patterns {
        let pattern_bytes = pattern_str.as_bytes();
        let pattern_total_len = pattern_bytes.len(); // includes '='
        if i + pattern_total_len <= bytes.len() && &bytes[i..i + pattern_total_len] == pattern_bytes
        {
            // Find the end of the value (whitespace, quote, comma, or end of string)
            let value_start = i + pattern_total_len; // position of '='
            let mut value_end = value_start;
            while value_end < bytes.len() {
                let b = bytes[value_end];
                if b == b' '
                    || b == b'\t'
                    || b == b'\''
                    || b == b'"'
                    || b == b','
                    || b == b'&'
                    || b == b'\n'
                    || b == b'\r'
                {
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
        if b == b' '
            || b == b'\t'
            || b == b'\n'
            || b == b'\r'
            || b == b'\''
            || b == b'"'
            || b == b','
        {
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
            if b == b' '
                || b == b'\t'
                || b == b'\''
                || b == b'"'
                || b == b','
                || b == b'&'
                || b == b'\n'
            {
                break;
            }
            value_end += 1;
        }
        Some((key_bytes.len(), value_start, value_end))
    } else {
        None
    }
}

/// Check if current position starts a GitHub token pattern (ghp_, gho_, ght_, ghs_).
/// Returns (start, end) positions of the token if matched (≥8 hex/alphanumeric chars after prefix), None otherwise.
fn check_github_token(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    // GitHub token prefixes
    let prefixes = [b"ghp_", b"gho_", b"ght_", b"ghs_"];
    for prefix in &prefixes {
        let prefix_len = prefix.len();
        if i + prefix_len <= bytes.len() && &bytes[i..i + prefix_len] == *prefix {
            // Find the end of the token (whitespace or end of string)
            let start = i;
            let mut end = i + prefix_len;
            // Must have at least 8 hex/alphanumeric chars
            let mut count = 0;
            while end < bytes.len() {
                let b = bytes[end];
                if b == b' '
                    || b == b'\t'
                    || b == b'\n'
                    || b == b'\r'
                    || b == b'\''
                    || b == b'"'
                    || b == b','
                    || b == b'&'
                {
                    break;
                }
                // Only hex/alphanumeric allowed in GitHub tokens
                if !(b.is_ascii_digit() || b.is_ascii_lowercase() || b.is_ascii_uppercase()) {
                    break;
                }
                count += 1;
                end += 1;
            }
            if count >= 8 {
                return Some((start, end));
            }
            return None;
        }
    }
    None
}

/// Check if current position starts an OpenAI `sk-*` key pattern.
/// Returns (start, end) positions of the key if matched, None otherwise.
/// Matches: sk- (≥48 chars), sk-proj- (≥16 chars after), sk-svcacct- (≥16 chars after), sk-admin- (≥16 chars after).
fn check_openai_key(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    // sk-proj-, sk-svcacct-, sk-admin- prefixes first (more specific, require 16+ chars after)
    if let Some((start, end)) = check_openai_long_prefix(bytes, i, b"sk-proj-", 16) {
        return Some((start, end));
    }
    if let Some((start, end)) = check_openai_long_prefix(bytes, i, b"sk-svcacct-", 16) {
        return Some((start, end));
    }
    if let Some((start, end)) = check_openai_long_prefix(bytes, i, b"sk-admin-", 16) {
        return Some((start, end));
    }

    // Classic sk- prefix (≥48 chars total after "sk-")
    if i + 3 <= bytes.len() && &bytes[i..i + 3] == b"sk-" {
        let start = i;
        let mut end = i + 3;
        let mut count = 0;
        while end < bytes.len() {
            let b = bytes[end];
            if b == b' '
                || b == b'\t'
                || b == b'\n'
                || b == b'\r'
                || b == b'\''
                || b == b'"'
                || b == b','
                || b == b'&'
            {
                break;
            }
            count += 1;
            end += 1;
        }
        if count >= 48 {
            return Some((start, end));
        }
    }
    None
}

/// Helper for OpenAI long prefix patterns (sk-proj-, sk-svcacct-, sk-admin-).
fn check_openai_long_prefix(
    bytes: &[u8],
    i: usize,
    prefix: &[u8],
    min_chars: usize,
) -> Option<(usize, usize)> {
    let prefix_len = prefix.len();
    if i + prefix_len <= bytes.len() && &bytes[i..i + prefix_len] == prefix {
        let start = i;
        let mut end = i + prefix_len;
        let mut count = 0;
        while end < bytes.len() {
            let b = bytes[end];
            if b == b' '
                || b == b'\t'
                || b == b'\n'
                || b == b'\r'
                || b == b'\''
                || b == b'"'
                || b == b','
                || b == b'&'
            {
                break;
            }
            count += 1;
            end += 1;
        }
        if count >= min_chars {
            return Some((start, end));
        }
    }
    None
}

/// Check if current position starts a CLI `--secret` or `--api-key` flag pattern.
/// Returns the position after the flag prefix ("--secret " or "--api-key ") if matched, None otherwise.
/// Skips any leading whitespace before the flag.
fn check_cli_secret_flag(bytes: &[u8], i: usize) -> Option<usize> {
    // Skip leading whitespace
    let mut pos = i;
    while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
        pos += 1;
    }
    if pos >= bytes.len() {
        return None;
    }

    // Check for --secret
    if pos + 9 <= bytes.len() && &bytes[pos..pos + 9] == b"--secret " {
        return Some(pos + 9); // position after "--secret "
    }

    // Check for --api-key (but NOT --api-key= which is handled by assignment pattern)
    // Note: "--api-key " is 10 chars (2+7+1), not 11
    if pos + 10 <= bytes.len() && &bytes[pos..pos + 10] == b"--api-key " {
        return Some(pos + 10); // position after "--api-key "
    }

    None
}

/// Check if current position starts a JWT header prefix (`eyJ`) with x.y.z tripartite structure.
/// Returns (start, end) positions of the token if matched (50-1200 chars), None otherwise.
fn check_jwt_prefix(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    // JWT header prefix: "eyJ" is base64 of '{"'
    if i + 3 > bytes.len() || &bytes[i..i + 3] != b"eyJ" {
        return None;
    }

    // Find the token end by scanning for whitespace or end
    let start = i;
    let mut end = i + 3;
    let mut has_dot = false;
    let mut dot_count = 0;

    while end < bytes.len() {
        let b = bytes[end];
        if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
            break;
        }
        if b == b'.' {
            has_dot = true;
            dot_count += 1;
        }
        end += 1;
    }

    // Must have x.y.z structure (at least 2 dots)
    if !has_dot || dot_count < 2 {
        return None;
    }

    // Token length must be 50-1200 chars
    let token_len = end - start;
    if !(50..=1200).contains(&token_len) {
        return None;
    }

    Some((start, end))
}

/// Check if current position starts a lowercase secret suffix pattern.
/// Matches keys of 3+ bytes containing ONLY a-z, 0-9, underscore
/// that END with exactly one of: _key, _token, _secret, _password.
/// Follows same delimiter semantics as check_assignment_pattern.
///
/// Returns (key_len, value_start, value_end) if found, None otherwise.
fn check_lowercase_secret_suffix(bytes: &[u8], i: usize) -> Option<(usize, usize, usize)> {
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

    // Key must be 3+ bytes
    let key_len = eq_pos - i;
    if key_len < 3 {
        return None;
    }

    // Key must contain ONLY a-z, 0-9, underscore
    let key_bytes = &bytes[i..eq_pos];
    for &b in key_bytes {
        match b {
            b'a'..=b'z' | b'0'..=b'9' | b'_' => {}
            _ => return None,
        }
    }

    // Key must END with exactly one of: _key, _token, _secret, _password
    let suffixes: [&[u8]; 4] = [b"_key", b"_token", b"_secret", b"_password"];
    let mut matched_suffix_len = 0;

    for suffix in &suffixes {
        let suffix_len = suffix.len();
        if key_len >= suffix_len {
            let suffix_start = eq_pos - suffix_len;
            if &bytes[suffix_start..eq_pos] == *suffix {
                matched_suffix_len = suffix_len;
                break;
            }
        }
    }

    if matched_suffix_len == 0 {
        return None;
    }

    // Find the end of the value (same delimiter set as check_assignment_pattern)
    let value_start = eq_pos + 1;
    let mut value_end = value_start;

    // Handle quoted values
    if value_end < bytes.len() && (bytes[value_end] == b'\'' || bytes[value_end] == b'"') {
        let quote = bytes[value_end];
        value_end += 1; // skip opening quote
        while value_end < bytes.len() && bytes[value_end] != quote {
            value_end += 1;
        }
        if value_end < bytes.len() {
            value_end += 1; // skip closing quote
        }
    } else {
        // Scan until delimiter or end
        while value_end < bytes.len() {
            let b = bytes[value_end];
            if b == b' '
                || b == b'\t'
                || b == b'\''
                || b == b'"'
                || b == b','
                || b == b'&'
                || b == b'\n'
                || b == b'\r'
            {
                break;
            }
            value_end += 1;
        }
    }

    Some((key_len, value_start, value_end))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── token= pattern ─────────────────────────────────────────────────────────

    #[test]
    fn sanitize_token_eq_value() {
        assert_eq!(sanitize_command("token=secret123"), "token=[REDACTED]");
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
        assert_eq!(sanitize_command("mytoken=sk-12345"), "mytoken=[REDACTED]");
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
        assert_eq!(sanitize_command("Bearer abc123"), "Bearer [REDACTED]");
    }

    // ─── AWS AKIA key pattern ──────────────────────────────────────────────────

    #[test]
    fn sanitize_aws_access_key() {
        // AKIA followed by 16 characters
        assert_eq!(sanitize_command("AKIAIOSFODNN7EXAMPLE"), "[REDACTED]");
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
        assert_eq!(sanitize_command("API_KEY=secret"), "API_KEY=[REDACTED]");
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
        assert_eq!(sanitize_command("myToken=secret"), "myToken=secret");
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
        assert_eq!(sanitize_command("FOO_BAR=baz"), "FOO_BAR=[REDACTED]");
    }

    #[test]
    fn sanitize_nested_equals() {
        // url=https://foo=bar should redact value until whitespace
        assert_eq!(
            sanitize_command("url=https://example.com?token=secret"),
            "url=https://example.com?token=[REDACTED]"
        );
    }

    // ─── GitHub Token Patterns (ghp_, gho_, ght_, ghs_) ─────────────────────────

    #[test]
    fn sanitize_github_token_ghp_classic_pat() {
        // Classic PAT with 8+ chars after prefix
        assert_eq!(
            sanitize_command("token=ghp_abc123def45678"),
            "token=[REDACTED]"
        );
    }

    #[test]
    fn sanitize_github_token_gho_oauth() {
        // OAuth access token
        assert_eq!(
            sanitize_command("Bearer gho_xxxxxxxxxxxx"),
            "Bearer [REDACTED]"
        );
    }

    #[test]
    fn sanitize_github_token_short_false_positive() {
        // Short prefix <8 chars should NOT be redacted
        assert_eq!(sanitize_command("ghp_test"), "ghp_test");
    }

    #[test]
    fn sanitize_github_token_inline_ghs() {
        // Inline ghs_ token with 8+ chars
        assert_eq!(sanitize_command("ghs_12345678ab"), "[REDACTED]");
    }

    #[test]
    fn sanitize_github_token_ght() {
        // ght_ token
        assert_eq!(sanitize_command("ght_abcdef123456"), "[REDACTED]");
    }

    // ─── OpenAI Key Patterns (sk-, sk-proj-, sk-svcacct-, sk-admin-) ──────────

    #[test]
    fn sanitize_openai_key_sk_proj() {
        // sk-proj- with 16+ chars after prefix
        assert_eq!(
            sanitize_command("api_key=sk-proj-abc123def456ghijkl"),
            "api_key=[REDACTED]"
        );
    }

    #[test]
    fn sanitize_openai_key_sk_classic() {
        // Classic sk- with 48+ chars
        let key = "sk-".to_string() + &"x".repeat(50);
        assert_eq!(
            sanitize_command(&format!("token={}", key)),
            "token=[REDACTED]"
        );
    }

    #[test]
    fn sanitize_openai_key_short_false_positive() {
        // sk-sandbox < 48 chars should NOT be redacted
        assert_eq!(sanitize_command("sk-sandbox"), "sk-sandbox");
    }

    #[test]
    fn sanitize_openai_key_sk_svcacct() {
        // sk-svcacct- with 16+ chars
        assert_eq!(
            sanitize_command("api_key=sk-svcacct-abcdefgh12345678"),
            "api_key=[REDACTED]"
        );
    }

    #[test]
    fn sanitize_openai_key_sk_admin() {
        // sk-admin- with 16+ chars
        assert_eq!(
            sanitize_command("api_key=sk-admin-abcdefgh12345678"),
            "api_key=[REDACTED]"
        );
    }

    // ─── CLI Secret Flag Patterns (--secret, --api-key) ────────────────────────

    #[test]
    fn sanitize_cli_secret_flag_dash_secret() {
        assert_eq!(
            sanitize_command("cmd --secret abc123 other"),
            "cmd --secret [REDACTED] other"
        );
    }

    #[test]
    fn sanitize_cli_secret_flag_dash_api_key() {
        assert_eq!(
            sanitize_command("tool --api-key sk-123 --flag"),
            "tool --api-key [REDACTED] --flag"
        );
    }

    #[test]
    fn sanitize_cli_secret_flag_dash_secret_long() {
        assert_eq!(
            sanitize_command("deploy --secret my_super_secret_token --env prod"),
            "deploy --secret [REDACTED] --env prod"
        );
    }

    // ─── JWT Prefix Pattern (eyJ + x.y.z structure) ───────────────────────────

    #[test]
    fn sanitize_jwt_prefix_valid() {
        // eyJ... with 200 chars and x.y.z structure
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.".to_string() + &"x".repeat(150) + ".z";
        assert_eq!(
            sanitize_command(&format!("Bearer {}", jwt)),
            "Bearer [REDACTED]"
        );
    }

    #[test]
    fn sanitize_jwt_prefix_short_no_dots() {
        // Short base64 without dots should NOT be redacted
        assert_eq!(sanitize_command("eyJpZCI6"), "eyJpZCI6");
    }

    #[test]
    fn sanitize_jwt_prefix_no_tripartite() {
        // eyJabc without dots should NOT be redacted
        assert_eq!(sanitize_command("eyJabc"), "eyJabc");
    }

    #[test]
    fn sanitize_jwt_prefix_no_prefix() {
        // No eyJ prefix, just base64-like string - should NOT be redacted
        let no_prefix = "dGhpcyBpcyBhIHNhbXBsZSJ9".to_string();
        assert_eq!(sanitize_command(&no_prefix), no_prefix);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Phase 1: Lowercase Secret Suffix Detection (_key, _token, _secret, _password)
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_lowercase_secret_key_suffix() {
        // REQ-LCK-01#1: secret_key=fake123 → secret_key=[REDACTED]
        assert_eq!(
            sanitize_command("secret_key=fake123"),
            "secret_key=[REDACTED]"
        );
    }

    #[test]
    fn test_lowercase_access_token_suffix() {
        // REQ-LCK-01#2: access_token=abc123 → access_token=[REDACTED]
        assert_eq!(
            sanitize_command("access_token=abc123"),
            "access_token=[REDACTED]"
        );
    }

    #[test]
    fn test_lowercase_client_secret_suffix() {
        // REQ-LCK-01#3: client_secret=mysecret → client_secret=[REDACTED]
        assert_eq!(
            sanitize_command("client_secret=mysecret"),
            "client_secret=[REDACTED]"
        );
    }

    #[test]
    fn test_lowercase_db_password_suffix() {
        // REQ-LCK-01#4: db_password=letmein → db_password=[REDACTED]
        assert_eq!(
            sanitize_command("db_password=letmein"),
            "db_password=[REDACTED]"
        );
    }

    #[test]
    fn test_lowercase_github_token_suffix() {
        // REQ-LCK-01#5: github_token=ghp_fake12345678 → github_token=[REDACTED]
        assert_eq!(
            sanitize_command("github_token=ghp_fake12345678"),
            "github_token=[REDACTED]"
        );
    }

    #[test]
    fn test_lowercase_quoted_value() {
        // REQ-LCK-01#6: secret_key='f4k3t0k3n' → secret_key=[REDACTED]
        assert_eq!(
            sanitize_command("secret_key='f4k3t0k3n'"),
            "secret_key=[REDACTED]"
        );
    }

    #[test]
    fn test_lowercase_empty_value() {
        // REQ-LCK-01#7: secret_key= → secret_key=[REDACTED]
        assert_eq!(sanitize_command("secret_key="), "secret_key=[REDACTED]");
    }

    #[test]
    fn test_lowercase_ampersand_delimiter() {
        // REQ-LCK-01#8: private_key=abc&next=val → private_key=[REDACTED]&next=val
        assert_eq!(
            sanitize_command("private_key=abc&next=val"),
            "private_key=[REDACTED]&next=val"
        );
    }

    #[test]
    fn test_lowercase_newline_delimiter() {
        // REQ-LCK-01#9: private_key=abc\nnext → private_key=[REDACTED]\nnext
        assert_eq!(
            sanitize_command("private_key=abc\nnext"),
            "private_key=[REDACTED]\nnext"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Phase 2: False-Positive Guards (keys WITHOUT exact suffix match)
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_key_count_not_redacted() {
        // REQ-LCK-03#13: key_count=5 → key_count=5 (no exact suffix match)
        assert_eq!(sanitize_command("key_count=5"), "key_count=5");
    }

    #[test]
    fn test_user_name_not_redacted() {
        // REQ-LCK-03#14: user_name=john → user_name=john
        assert_eq!(sanitize_command("user_name=john"), "user_name=john");
    }

    #[test]
    fn test_model_id_not_redacted() {
        // REQ-LCK-03#16: model_id=gpt-4 → model_id=gpt-4
        assert_eq!(sanitize_command("model_id=gpt-4"), "model_id=gpt-4");
    }

    #[test]
    fn test_profile_path_not_redacted() {
        // REQ-LCK-03#15: profile_path=/tmp → profile_path=/tmp
        assert_eq!(sanitize_command("profile_path=/tmp"), "profile_path=/tmp");
    }

    #[test]
    fn test_env_not_redacted() {
        // REQ-LCK-03#17: env=production → env=production (single word)
        assert_eq!(sanitize_command("env=production"), "env=production");
    }

    #[test]
    fn test_short_key_not_redacted() {
        // REQ-LCK-03#18: k=val → k=val (key < 3 chars)
        assert_eq!(sanitize_command("k=val"), "k=val");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Phase 3: Priority, Unicode & Idempotence
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_uppercase_snake_priority_over_lowercase() {
        // REQ-LCK-04#19: AWS_KEY=AKIA_FAKEEXAMPLE_0001 → uppercase snake wins
        assert_eq!(
            sanitize_command("AWS_KEY=AKIA_FAKEEXAMPLE_0001"),
            "AWS_KEY=[REDACTED]"
        );
    }

    #[test]
    fn test_explicit_api_key_pattern_priority() {
        // REQ-LCK-04#20: api_key=secret_key_value → explicit api_key= wins
        assert_eq!(
            sanitize_command("api_key=secret_key_value"),
            "api_key=[REDACTED]"
        );
    }

    #[test]
    fn test_token_index_not_redacted() {
        // REQ-LCK-03#13 variant: token_index=1 → NOT redacted (no exact suffix)
        assert_eq!(sanitize_command("token_index=1"), "token_index=1");
    }

    #[test]
    fn test_unicode_preserved() {
        // REQ-LCK-05#21: echo café && secret_key=fake → echo café && secret_key=[REDACTED]
        assert_eq!(
            sanitize_command("echo café && secret_key=fake"),
            "echo café && secret_key=[REDACTED]"
        );
    }

    #[test]
    fn test_emoji_preserved() {
        // REQ-LCK-05#22: secret_key=fake 🔑 done → secret_key=[REDACTED] 🔑 done
        assert_eq!(
            sanitize_command("secret_key=fake 🔑 done"),
            "secret_key=[REDACTED] 🔑 done"
        );
    }

    #[test]
    fn test_idempotent_deterministic() {
        // REQ-LCK-06#23: Repeated calls produce identical output
        let input = "secret_key=fake123 access_token=abc";
        let first = sanitize_command(input);
        for _ in 0..100 {
            assert_eq!(sanitize_command(input), first);
        }
    }
}
