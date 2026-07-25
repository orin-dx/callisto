use super::Entry;
use callisto_model::{Severity, SeverityParseError};

/// The name half of one frontmatter line, before severity resolution.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NameToken<'a> {
    Bare(&'a str),
    /// Owned because a quoted name's content is copied out from between the delimiting
    /// quotes, independent of any surrounding text.
    Quoted(String),
}

/// Line-relative failure — the caller (`parse_changeset`) attaches the absolute line number
/// and promotes this into the corresponding `ParseError` variant.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LineError {
    UnclosedQuotedName,
    AmbiguousNameQuoting {
        raw: String,
    },
    MissingSeparator {
        raw: String,
    },
    EmptyName,
    /// Carries the already-resolved name — `parse_entry_line` is the only place that has it
    /// in scope when severity parsing fails, and `ParseError::InvalidSeverity` (§F.5.5) needs
    /// it to construct a useful message.
    InvalidSeverity {
        name: String,
        source: SeverityParseError,
    },
}

/// Splits one frontmatter line into its name token and the unparsed remainder (starting at
/// the separator `:`). This is the function §13 invariant 1 names: "unquotes before
/// splitting on `:`, not after."
///
/// For a **quoted** name, the closing `"` is located by scanning forward for the matching
/// delimiter — never by searching for `:` — before anything about the remainder of the line
/// is inspected. A name that itself contains a `:` (Maven's `groupId:artifactId` form) is
/// never mis-split, because the colon search for the *separator* only ever runs on the
/// remainder *after* the name's own boundary has already been resolved by quote-matching.
///
/// For a **bare** (unquoted) name, there is nothing to unquote, so `rsplit_once(':')` over
/// the whole line is applied directly.
pub(crate) fn split_name_and_rest(line: &str) -> Result<(NameToken<'_>, &str), LineError> {
    let trimmed = line.trim_start();
    if let Some(after_quote) = trimmed.strip_prefix('"') {
        let end = after_quote.find('"').ok_or(LineError::UnclosedQuotedName)?;
        let name = &after_quote[..end];
        let rest = after_quote[end + 1..].trim_start();
        let rest = rest
            .strip_prefix(':')
            .ok_or_else(|| LineError::AmbiguousNameQuoting {
                raw: line.to_string(),
            })?;
        Ok((NameToken::Quoted(name.to_string()), rest))
    } else {
        let (name, rest) = trimmed
            .rsplit_once(':')
            .ok_or_else(|| LineError::MissingSeparator {
                raw: line.to_string(),
            })?;
        Ok((NameToken::Bare(name.trim_end()), rest))
    }
}

/// Parses one frontmatter line (already known non-blank, non-`#`-comment) into an [`Entry`].
pub(crate) fn parse_entry_line(line: &str) -> Result<Entry, LineError> {
    let (token, rest) = split_name_and_rest(line)?;
    let name = match token {
        NameToken::Bare(s) => s.to_string(),
        NameToken::Quoted(s) => s,
    };
    if name.is_empty() {
        return Err(LineError::EmptyName);
    }
    let severity =
        rest.trim()
            .parse::<Severity>()
            .map_err(|source| LineError::InvalidSeverity {
                name: name.clone(),
                source,
            })?;
    Ok(Entry { name, severity })
}

/// §6.1: "quoted-when-necessary on write." Conservative by construction — over-quoting is
/// lossless, under-quoting corrupts output, so every character class that could make a bare
/// scalar ambiguous or YAML-invalid triggers quoting.
pub(crate) fn needs_quoting(name: &str) -> bool {
    let Some(first) = name.chars().next() else {
        return true; // empty name — write_changeset rejects this separately
    };
    matches!(
        first,
        '@' | '`'
            | '"'
            | '\''
            | '&'
            | '*'
            | '!'
            | '|'
            | '>'
            | '%'
            | '#'
            | '-'
            | '?'
            | ':'
            | ','
            | '['
            | ']'
            | '{'
            | '}'
            | ' '
    ) || name.contains(':')
        || name.contains('#')
        || name.ends_with(' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_bare_name_from_severity() {
        let (token, rest) = split_name_and_rest("cargo/foo: patch").unwrap();
        assert_eq!(token, NameToken::Bare("cargo/foo"));
        assert_eq!(rest, " patch");
    }

    #[test]
    fn splits_quoted_name_from_severity() {
        let (token, rest) = split_name_and_rest("\"@myorg/foo\": minor").unwrap();
        assert_eq!(token, NameToken::Quoted("@myorg/foo".to_string()));
        assert_eq!(rest, " minor");
    }

    #[test]
    fn quoted_name_containing_colon_is_not_mis_split() {
        // The reference `knope-dev/changesets` crate colon-splits before unquoting, which
        // mis-parses exactly this Maven-style identity (§13 invariant 1).
        let (token, rest) = split_name_and_rest("\"maven/org.example:foo-core\": major").unwrap();
        assert_eq!(
            token,
            NameToken::Quoted("maven/org.example:foo-core".to_string())
        );
        assert_eq!(rest, " major");
    }

    #[test]
    fn bare_name_uses_rsplit_once_on_colon() {
        // rsplit_once means only the LAST colon is the separator for a bare line.
        let (token, rest) = split_name_and_rest("weird:but:bare: patch").unwrap();
        assert_eq!(token, NameToken::Bare("weird:but:bare"));
        assert_eq!(rest, " patch");
    }

    #[test]
    fn unclosed_quote_is_an_error() {
        let err = split_name_and_rest("\"@myorg/foo: minor").unwrap_err();
        assert_eq!(err, LineError::UnclosedQuotedName);
    }

    #[test]
    fn content_between_closing_quote_and_colon_is_ambiguous() {
        let err = split_name_and_rest("\"@myorg/foo\" extra: minor").unwrap_err();
        assert_eq!(
            err,
            LineError::AmbiguousNameQuoting {
                raw: "\"@myorg/foo\" extra: minor".to_string()
            }
        );
    }

    #[test]
    fn bare_line_with_no_colon_is_missing_separator() {
        let err = split_name_and_rest("no colon here").unwrap_err();
        assert_eq!(
            err,
            LineError::MissingSeparator {
                raw: "no colon here".to_string()
            }
        );
    }

    #[test]
    fn parses_full_entry_line() {
        let entry = parse_entry_line("cargo/foo: patch").unwrap();
        assert_eq!(entry.name, "cargo/foo");
        assert_eq!(entry.severity, Severity::Patch);
    }

    #[test]
    fn parse_entry_line_rejects_empty_name() {
        let err = parse_entry_line("\"\": minor").unwrap_err();
        assert_eq!(err, LineError::EmptyName);
    }

    #[test]
    fn parse_entry_line_rejects_invalid_severity() {
        let err = parse_entry_line("cargo/foo: critical").unwrap_err();
        assert!(matches!(err, LineError::InvalidSeverity { .. }));
    }

    #[test]
    fn parse_entry_line_severity_is_case_insensitive() {
        let entry = parse_entry_line("cargo/foo: MAJOR").unwrap();
        assert_eq!(entry.severity, Severity::Major);
    }

    #[test]
    fn needs_quoting_flags_npm_scoped_names() {
        assert!(needs_quoting("@myorg/foo"));
    }

    #[test]
    fn needs_quoting_allows_bare_cargo_style_names() {
        assert!(!needs_quoting("cargo/foo"));
    }

    #[test]
    fn needs_quoting_flags_names_containing_colon() {
        assert!(needs_quoting("maven/org.example:foo-core"));
    }

    #[test]
    fn needs_quoting_flags_empty_name() {
        assert!(needs_quoting(""));
    }

    #[test]
    fn needs_quoting_flags_trailing_space() {
        assert!(needs_quoting("foo "));
    }
}
