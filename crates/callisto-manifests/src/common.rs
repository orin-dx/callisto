//! Helpers shared between ecosystem-specific manifest editors (`cargo`, `npm`).
//!
//! Only functions verified to be byte-for-byte identical in behavior across
//! ecosystems live here. Functions that merely *look* similar but encode
//! ecosystem-specific semantics (e.g. `render_at_precision`, which differs
//! between Cargo and npm on prerelease/hyphen handling) stay local to their
//! respective modules.

/// Returns true if `s` looks like a bare `major.minor.patch` semver string
/// (as opposed to a range expression like `^1.0.0` or `>=1.0.0`).
///
/// This is a syntactic heuristic: it does not fully validate the string as
/// a semver version, it just checks that it starts with an ASCII digit and
/// splits into exactly three dot-separated parts.
pub(crate) fn is_bare_semver(s: &str) -> bool {
    let chars = s.chars().next();
    if !chars.is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3
}

/// Splits a version requirement clause into its leading comparison operator
/// (if any) and the remaining version-ish text.
///
/// Recognized operators: `^`, `~`, `>=`, `>`, `<=`, `<`, `=`. If none match,
/// the empty-string prefix is returned along with the trimmed input.
pub(crate) fn split_single_operator_prefix(s: &str) -> Option<(&str, &str)> {
    let trimmed = s.trim();
    for op in ["^", "~", ">=", ">", "<=", "<", "="] {
        if let Some(rest) = trimmed.strip_prefix(op) {
            return Some((op, rest.trim()));
        }
    }
    Some(("", trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_bare_semver_accepts_three_part_numeric() {
        assert!(is_bare_semver("1.2.3"));
        assert!(is_bare_semver("0.0.0"));
    }

    #[test]
    fn is_bare_semver_rejects_non_digit_start() {
        assert!(!is_bare_semver("^1.2.3"));
        assert!(!is_bare_semver("a.b.c"));
        assert!(!is_bare_semver(""));
    }

    #[test]
    fn is_bare_semver_rejects_wrong_part_count() {
        assert!(!is_bare_semver("1.2"));
        assert!(!is_bare_semver("1.2.3.4"));
        assert!(!is_bare_semver("1"));
    }

    #[test]
    fn is_bare_semver_rejects_prerelease_suffix() {
        // A prerelease suffix on the patch segment (e.g. "3-beta") plus its
        // own dot-separated identifier pushes the split count to 4, so this
        // is correctly rejected as "not bare".
        assert!(!is_bare_semver("1.2.3-beta.1"));
    }

    #[test]
    fn split_single_operator_prefix_recognizes_all_operators() {
        assert_eq!(split_single_operator_prefix("^1.2.3"), Some(("^", "1.2.3")));
        assert_eq!(split_single_operator_prefix("~1.2.3"), Some(("~", "1.2.3")));
        assert_eq!(
            split_single_operator_prefix(">=1.2.3"),
            Some((">=", "1.2.3"))
        );
        assert_eq!(split_single_operator_prefix(">1.2.3"), Some((">", "1.2.3")));
        assert_eq!(
            split_single_operator_prefix("<=1.2.3"),
            Some(("<=", "1.2.3"))
        );
        assert_eq!(split_single_operator_prefix("<1.2.3"), Some(("<", "1.2.3")));
        assert_eq!(split_single_operator_prefix("=1.2.3"), Some(("=", "1.2.3")));
    }

    #[test]
    fn split_single_operator_prefix_no_operator_returns_empty_prefix() {
        assert_eq!(split_single_operator_prefix("1.2.3"), Some(("", "1.2.3")));
    }

    #[test]
    fn split_single_operator_prefix_trims_whitespace() {
        assert_eq!(
            split_single_operator_prefix("  ^1.2.3  "),
            Some(("^", "1.2.3"))
        );
        assert_eq!(
            split_single_operator_prefix(">=  1.2.3"),
            Some((">=", "1.2.3"))
        );
    }
}
