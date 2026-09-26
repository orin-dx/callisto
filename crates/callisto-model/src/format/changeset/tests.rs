use super::*;

fn cs(entries: Vec<(&str, Severity)>, summary: &str) -> Changeset {
    Changeset {
        entries: entries
            .into_iter()
            .map(|(name, severity)| Entry {
                name: name.to_string(),
                severity,
            })
            .collect(),
        summary: summary.to_string(),
    }
}

#[test]
fn parses_a_single_entry_changeset() {
    let source = "---\n\"@myorg/foo\": minor\n---\n\nBumps the widget API.\n";
    let changeset = parse_changeset(source).unwrap();
    assert_eq!(
        changeset,
        cs(vec![("@myorg/foo", Severity::Minor)], "Bumps the widget API.")
    );
}

#[test]
fn parses_multiple_entries_mixed_quoted_and_bare() {
    let source = "---\ncargo/foo: patch\n\"@myorg/bar\": major\n---\n\nSummary text.\n";
    let changeset = parse_changeset(source).unwrap();
    assert_eq!(
        changeset,
        cs(
            vec![("cargo/foo", Severity::Patch), ("@myorg/bar", Severity::Major)],
            "Summary text."
        )
    );
}

#[test]
fn skips_comment_and_blank_lines_in_frontmatter() {
    let source = "---\n# a comment\n\ncargo/foo: patch\n---\n\nSummary.\n";
    let changeset = parse_changeset(source).unwrap();
    assert_eq!(changeset, cs(vec![("cargo/foo", Severity::Patch)], "Summary."));
}

#[test]
fn empty_frontmatter_with_nonempty_summary_is_valid() {
    let source = "---\n---\n\nDocs-only change, no version bump.\n";
    let changeset = parse_changeset(source).unwrap();
    assert_eq!(changeset, cs(vec![], "Docs-only change, no version bump."));
}

#[test]
fn empty_frontmatter_and_empty_summary_is_invalid() {
    let source = "---\n---\n\n";
    let err = parse_changeset(source).unwrap_err();
    assert_eq!(err, ParseError::EmptyChangeset);
}

#[test]
fn missing_opening_delimiter_is_an_error() {
    let err = parse_changeset("cargo/foo: patch\n\nSummary.\n").unwrap_err();
    assert_eq!(err, ParseError::MissingFrontmatterStart);
}

#[test]
fn unclosed_frontmatter_is_an_error() {
    let err = parse_changeset("---\ncargo/foo: patch\n\nSummary.\n").unwrap_err();
    assert_eq!(err, ParseError::UnclosedFrontmatter);
}

#[test]
fn duplicate_raw_name_is_an_error() {
    let source = "---\ncargo/foo: patch\ncargo/foo: minor\n---\n\nSummary.\n";
    let err = parse_changeset(source).unwrap_err();
    assert_eq!(
        err,
        ParseError::DuplicateEntry {
            line: 3,
            first_line: 2,
            name: "cargo/foo".to_string()
        }
    );
}

#[test]
fn line_numbers_in_errors_are_absolute_not_frontmatter_relative() {
    let source = "---\ncargo/foo: patch\ncargo/foo: minor\n---\n\nSummary.\n";
    // Line 1 is `---`, line 2 is the first entry, line 3 is the duplicate.
    let err = parse_changeset(source).unwrap_err();
    match err {
        ParseError::DuplicateEntry { line, first_line, .. } => {
            assert_eq!(first_line, 2);
            assert_eq!(line, 3);
        }
        other => panic!("expected DuplicateEntry, got {other:?}"),
    }
}

#[test]
fn crlf_input_parses_the_same_as_its_lf_twin() {
    let lf = "---\ncargo/foo: patch\n---\n\nSummary.\n";
    let crlf = "---\r\ncargo/foo: patch\r\n---\r\n\r\nSummary.\r\n";
    assert_eq!(parse_changeset(lf).unwrap(), parse_changeset(crlf).unwrap());
}

#[test]
fn round_trip_write_then_parse_reproduces_the_changeset() {
    let original = cs(
        vec![("cargo/foo", Severity::Patch), ("@myorg/bar", Severity::Major)],
        "Summary text.",
    );
    let written = write_changeset(&original).unwrap();
    let reparsed = parse_changeset(&written).unwrap();
    assert_eq!(reparsed, original);
}

#[test]
fn write_quotes_names_only_when_necessary() {
    let changeset = cs(
        vec![("cargo/foo", Severity::Patch), ("@myorg/bar", Severity::Major)],
        "Summary.",
    );
    let written = write_changeset(&changeset).unwrap();
    assert!(written.contains("cargo/foo: patch"));
    assert!(written.contains("\"@myorg/bar\": major"));
}

#[test]
fn write_always_uses_lf_and_ends_with_trailing_newline() {
    let changeset = cs(vec![("cargo/foo", Severity::Patch)], "Summary.");
    let written = write_changeset(&changeset).unwrap();
    assert!(!written.contains('\r'));
    assert!(written.ends_with('\n'));
    assert!(!written.ends_with("\n\n"));
}

#[test]
fn write_rejects_empty_changeset() {
    let changeset = cs(vec![], "");
    let err = write_changeset(&changeset).unwrap_err();
    assert_eq!(err, WriteError::EmptyChangeset);
}

#[test]
fn write_rejects_name_containing_a_literal_quote() {
    let changeset = cs(vec![("weird\"name", Severity::Patch)], "Summary.");
    let err = write_changeset(&changeset).unwrap_err();
    assert_eq!(
        err,
        WriteError::NameContainsQuote {
            index: 0,
            name: "weird\"name".to_string()
        }
    );
}

/// write_changeset must reject a changeset that has entries but an empty summary.
/// Before the fix, the AND-gate guard allowed this through silently.
#[test]
fn write_rejects_empty_summary_with_entries() {
    let changeset = cs(vec![("cargo/foo", Severity::Patch)], "");
    let err = write_changeset(&changeset).unwrap_err();
    assert_eq!(err, WriteError::EmptySummary);
}

/// write_changeset must reject a whitespace-only summary even when entries are present.
#[test]
fn write_rejects_whitespace_only_summary_with_entries() {
    let changeset = cs(vec![("cargo/foo", Severity::Patch)], "   \n\t  ");
    let err = write_changeset(&changeset).unwrap_err();
    assert_eq!(err, WriteError::EmptySummary);
}

/// parse_changeset must reject a file that has entries but an empty summary body.
#[test]
fn parse_rejects_empty_summary_with_entries() {
    let malformed = "---\ncargo/foo: minor\n---\n\n   \n";
    let err = parse_changeset(malformed).unwrap_err();
    assert_eq!(err, ParseError::EmptySummary);
}

/// parse_changeset must reject a file with entries and only whitespace after the closing
/// delimiter.
#[test]
fn parse_rejects_whitespace_only_summary_with_entries() {
    let malformed = "---\ncargo/foo: minor\n---\n\n  \t  \n\n";
    let err = parse_changeset(malformed).unwrap_err();
    assert_eq!(err, ParseError::EmptySummary);
}

use proptest::prelude::*;

proptest! {
    #[test]
    fn proptest_changeset_roundtrip(
        pkg_name in "[a-z0-9_\\-/]{1,20}",
        summary in "[a-zA-Z0-9_\\.\\,\\!]{1,100}"
    ) {
        let changeset = Changeset {
            entries: vec![Entry {
                name: pkg_name,
                severity: Severity::Minor,
            }],
            summary: summary.to_string(),
        };
        if let Ok(written) = write_changeset(&changeset) {
            let reparsed = parse_changeset(&written).unwrap();
            prop_assert_eq!(reparsed, changeset);
        }
    }
}
