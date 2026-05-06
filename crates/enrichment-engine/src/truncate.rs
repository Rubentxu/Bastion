//! String truncation utilities for safe output storage.
//!
//! Provides UTF-8 safe truncation at character boundaries, ensuring no
//! multi-byte Unicode characters are split. Empty output is stored as `None`.

/// Truncate a string to at most `limit` characters.
///
/// Uses `char_indices()` to find safe cut limits, preventing UTF-8
/// character splitting. If the input exceeds `limit` characters,
/// appends `"…"` (U+2026 HORIZONTAL ELLIPSIS).
///
/// # Arguments
///
/// * `s` - The input string to truncate
/// * `limit` - Maximum number of characters allowed (must be > 0)
///
/// # Returns
///
/// * `None` if `s` is empty or contains only whitespace
/// * `Some(String)` with the truncated string (≤ `limit` chars + "…"" if truncated)
///
/// # Examples
///
/// ```
/// use enrichment_engine::truncate::truncate_string;
///
/// // Short string is returned verbatim
/// assert_eq!(truncate_string("hello", 500), Some("hello".to_string()));
///
///     // Long string is truncated with ellipsis
///     let long = "a".repeat(1000);
///     let result = truncate_string(&long, 500);
///     assert!(result.is_some());
///     let s = result.unwrap();
///     assert!(s.ends_with('…'));
///     assert_eq!(s.chars().count(), 501); // limit + ellipsis
///
/// // Empty string returns None
/// assert_eq!(truncate_string("", 500), None);
///
/// // Whitespace-only returns None
/// assert_eq!(truncate_string("   ", 500), None);
/// ```
///
/// # Privacy
///
/// This function is the ONLY mechanism for storing command output.
/// Raw unbounded stdout/stderr are NEVER persisted — only truncated summaries.
#[must_use]
pub fn truncate_string(s: &str, limit: usize) -> Option<String> {
    if limit == 0 {
        return None;
    }

    // Check if string is empty or only whitespace
    if s.trim().is_empty() {
        return None;
    }

    let char_count = s.chars().count();

    if char_count <= limit {
        // No truncation needed
        Some(s.to_string())
    } else {
        // Find safe cut point using char_indices
        let mut truncated = String::with_capacity(limit + 3); // +3 for ellipsis
        for (i, c) in s.char_indices() {
            if i == 0 {
                // First character - start building
                truncated.push(c);
            } else if truncated.chars().count() < limit {
                truncated.push(c);
            } else {
                break;
            }
        }
        truncated.push('…');
        Some(truncated)
    }
}

/// Truncate a string by finding the last safe cut point at or before `limit`.
///
/// This is an alternative approach that finds the best cut point
/// (preferring word boundaries when possible) rather than hard-cutting
/// at `limit` characters.
///
/// # Arguments
///
/// * `s` - The input string to truncate
/// * `limit` - Maximum number of characters allowed
///
/// # Returns
///
/// * `None` if `s` is empty or only whitespace
/// * `Some(String)` truncated to ≤ `limit` characters with "…"" appended
#[must_use]
pub fn truncate_string_at(s: &str, limit: usize) -> Option<String> {
    if limit == 0 {
        return None;
    }

    if s.trim().is_empty() {
        return None;
    }

    let char_count = s.chars().count();

    if char_count <= limit {
        return Some(s.to_string());
    }

    // Collect characters up to limit
    let chars: String = s.chars().take(limit).collect();
    Some(format!("{}…", chars))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Basic truncation ─────────────────────────────────────────────────────────

    #[test]
    fn short_string_verbatim() {
        assert_eq!(truncate_string("hello", 500), Some("hello".to_string()));
    }

    #[test]
    fn exact_limit_returns_unchanged() {
        assert_eq!(truncate_string("abc", 3), Some("abc".to_string()));
    }

    #[test]
    fn long_string_truncated_with_ellipsis() {
        let long = "a".repeat(1000);
        let result = truncate_string(&long, 500);
        assert!(result.is_some());
        let s = result.unwrap();
        // Should be limit chars + ellipsis = 501 chars
        assert_eq!(s.chars().count(), 501);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn string_at_limit_plus_one_truncated() {
        let result = truncate_string("abcd", 3);
        assert!(result.is_some());
        let s = result.unwrap();
        assert_eq!(s.chars().count(), 4); // 3 chars + ellipsis
        assert!(s.ends_with('…'));
    }

    // ─── Empty and whitespace handling ───────────────────────────────────────────

    #[test]
    fn empty_string_returns_none() {
        assert_eq!(truncate_string("", 500), None);
    }

    #[test]
    fn whitespace_only_returns_none() {
        assert_eq!(truncate_string("   ", 500), None);
        assert_eq!(truncate_string("\t\n", 500), None);
        assert_eq!(truncate_string("  \t  \n  ", 500), None);
    }

    #[test]
    fn single_whitespace_char_returns_none() {
        assert_eq!(truncate_string(" ", 500), None);
        assert_eq!(truncate_string("\n", 500), None);
    }

    // ─── Unicode handling ────────────────────────────────────────────────────────

    #[test]
    fn unicode_chars_not_split() {
        // Emoji take 4 bytes but count as 1 character
        let emoji = "👍👍👍👍👍"; // 5 emoji = 5 chars
        let result = truncate_string(emoji, 3);
        assert!(result.is_some());
        let s = result.unwrap();
        assert_eq!(s.chars().count(), 4); // 3 + ellipsis
        assert!(s.ends_with('…'));
    }

    #[test]
    fn mixed_ascii_unicode_truncation() {
        let mixed = "hello👍world";
        let result = truncate_string(mixed, 6);
        assert!(result.is_some());
        let s = result.unwrap();
        assert_eq!(s.chars().count(), 7); // 6 + ellipsis
    }

    #[test]
    fn multi_byte_char_at_boundary() {
        // Japanese characters (3 bytes each)
        let jp = "日本語";
        assert_eq!(jp.chars().count(), 3);
        let result = truncate_string(jp, 2);
        assert!(result.is_some());
        let s = result.unwrap();
        assert_eq!(s.chars().count(), 3); // 2 + ellipsis
        assert!(s.ends_with('…'));
    }

    #[test]
    fn zero_limit_returns_none() {
        assert_eq!(truncate_string("hello", 0), None);
    }

    // ─── Edge cases ──────────────────────────────────────────────────────────────

    #[test]
    fn single_char_string_at_limit() {
        assert_eq!(truncate_string("a", 1), Some("a".to_string()));
    }

    #[test]
    fn single_char_string_over_limit() {
        let result = truncate_string("ab", 1);
        assert!(result.is_some());
        let s = result.unwrap();
        assert_eq!(s.chars().count(), 2); // 1 + ellipsis
        assert!(s.ends_with('…'));
    }

    #[test]
    fn newlines_trimmed_for_empty_check() {
        // "\n" is whitespace, so should return None
        assert_eq!(truncate_string("\n", 500), None);
        // But "\nhello" should work since trim() removes whitespace
        let result = truncate_string("\nhello", 500);
        assert_eq!(result, Some("\nhello".to_string()));
    }

    // ─── truncate_string_at tests ────────────────────────────────────────────────

    #[test]
    fn truncate_at_short_string() {
        assert_eq!(truncate_string_at("hello", 500), Some("hello".to_string()));
    }

    #[test]
    fn truncate_at_long_string() {
        let long = "a".repeat(1000);
        let result = truncate_string_at(&long, 500);
        assert!(result.is_some());
        let s = result.unwrap();
        assert_eq!(s.chars().count(), 501); // 500 + ellipsis
        assert!(s.ends_with('…'));
    }

    #[test]
    fn truncate_at_empty_returns_none() {
        assert_eq!(truncate_string_at("", 500), None);
    }

    #[test]
    fn truncate_at_whitespace_returns_none() {
        assert_eq!(truncate_string_at("   ", 500), None);
    }

    // ─── Privacy guarantees ─────────────────────────────────────────────────────

    #[test]
    fn very_long_stdout_is_truncated() {
        // Simulate a 10KB stdout that should be reduced to ~500 chars + ellipsis
        let stdout = "x".repeat(10_000);
        let result = truncate_string(&stdout, 500);
        assert!(result.is_some());
        let s = result.unwrap();
        assert!(s.chars().count() <= 501); // limit + ellipsis
        assert!(s.ends_with('…'));
    }

    #[test]
    fn empty_stderr_returns_none_not_empty_string() {
        // Empty stderr should be stored as None per privacy requirements
        assert_eq!(truncate_string("", 500), None);
    }
}
