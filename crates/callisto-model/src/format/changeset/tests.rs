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

// A leading byte-order mark and CRLF or lone-CR line endings are normalized before parsing.
#[test]
fn a_byte_order_mark_and_every_line_ending_style_parse_the_same() {
    let expected = cs(vec![("cargo/foo", Severity::Minor)], "Bumps the widget API.");
    for source in [
        "---\ncargo/foo: minor\n---\n\nBumps the widget API.\n",
        "\u{feff}---\ncargo/foo: minor\n---\n\nBumps the widget API.\n",
        "---\r\ncargo/foo: minor\r\n---\r\n\r\nBumps the widget API.\r\n",
        "---\rcargo/foo: minor\r---\r\rBumps the widget API.\r",
        "\u{feff}---\r\ncargo/foo: minor\r\n---\r\n\r\nBumps the widget API.\r\n",
    ] {
        assert_eq!(parse_changeset(source).unwrap(), expected, "source: {source:?}");
    }
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

/// `@changesets/cli add --empty` writes exactly this shape; callisto must accept it, not
/// just the packages it happens to already know about.
#[test]
fn empty_frontmatter_and_empty_summary_is_valid() {
    let source = "---\n---\n\n";
    let changeset = parse_changeset(source).unwrap();
    assert_eq!(changeset, cs(vec![], ""));
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

/// Writing an empty changeset (no entries, no summary) must succeed, byte-for-byte matching
/// what `@changesets/cli add --empty` writes -- the two must round-trip through each other.
#[test]
fn write_accepts_empty_changeset() {
    let changeset = cs(vec![], "");
    let written = write_changeset(&changeset).unwrap();
    assert_eq!(written, "---\n---\n\n");
    assert_eq!(parse_changeset(&written).unwrap(), changeset);
}

/// A name the line grammar cannot hold is refused, never written in a form that reads back as another name.
#[test]
fn write_refuses_a_name_with_a_quote_carriage_return_or_line_feed() {
    for name in ["weird\"name", "bad\nname", "bad\rname", "bad\r\nname", "trailing\n"] {
        let changeset = cs(vec![("ok", Severity::Minor), (name, Severity::Patch)], "Summary.");
        assert_eq!(
            write_changeset(&changeset).unwrap_err(),
            WriteError::NameNotRepresentable {
                index: 1,
                name: name.to_string()
            },
            "{name:?}"
        );
    }
}

/// The reader rejects a name given twice, so the writer refuses to write one.
#[test]
fn write_refuses_a_name_given_twice() {
    let changeset = cs(vec![("a", Severity::Patch), ("a", Severity::Major)], "Summary.");
    assert_eq!(
        write_changeset(&changeset).unwrap_err(),
        WriteError::NameNotRepresentable {
            index: 1,
            name: "a".to_string()
        }
    );
}

/// Every whitespace character the reader trims from a bare name is kept by quoting it, as are delimiters.
#[test]
fn names_with_boundary_whitespace_or_delimiters_read_back_unchanged() {
    for name in [
        " lead",
        "trail ",
        "\tlead",
        "trail\t",
        "\u{a0}lead",
        "trail\u{a0}",
        "\u{3000}wide",
        "line\u{2028}",
        "\u{85}next",
        "\u{b}vt",
        "in\tside",
        "in\u{a0}side",
        "a:b",
        "maven/org.example:core",
        "a#b",
        "#lead",
        "@scope/pkg",
        "cargo/foo",
        "plain",
        "-dash",
        "---",
    ] {
        let changeset = cs(vec![(name, Severity::Minor)], "Summary.");
        let written = write_changeset(&changeset).unwrap();
        assert_eq!(
            parse_changeset(&written).unwrap(),
            changeset,
            "{name:?} wrote {written:?}"
        );
    }
}

/// The written file uses LF only, so a summary's CR or CRLF line ends read back as the LF the reader yields.
#[test]
fn a_summary_with_carriage_returns_is_written_with_lf_line_ends() {
    let written = write_changeset(&cs(vec![("a", Severity::Patch)], "First\r\nSecond\rThird")).unwrap();
    assert!(!written.contains('\r'), "{written:?}");
    assert_eq!(parse_changeset(&written).unwrap().summary, "First\nSecond\nThird");
}

/// write_changeset must reject a changeset that has entries but an empty summary.
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

/// Names drawn from letters plus whitespace, delimiter, quote and line-break characters.
fn any_name() -> impl Strategy<Value = String> {
    let special = prop::sample::select(vec![
        ' ', '\t', '\u{a0}', '\u{3000}', '\u{2028}', '\u{85}', '\u{b}', '\u{c}', ':', '#', '@', '/', '-', '"', '\'',
        '\r', '\n',
    ]);
    proptest::collection::vec(prop_oneof![3 => proptest::char::range('a', 'c'), 1 => special], 1..6)
        .prop_map(|chars| chars.into_iter().collect())
}

fn any_severity() -> impl Strategy<Value = Severity> {
    prop::sample::select(vec![Severity::None, Severity::Patch, Severity::Minor, Severity::Major])
}

proptest! {
    /// A changeset is either written so it reads back unchanged, or refused at the first name it cannot represent.
    #[test]
    fn written_changesets_read_back_unchanged(
        entries in proptest::collection::vec((any_name(), any_severity()), 1..4),
        summary in "[a-z][a-zA-Z0-9_ .,!\n]{0,40}",
    ) {
        let changeset = Changeset {
            entries: entries.into_iter().map(|(name, severity)| Entry { name, severity }).collect(),
            summary: summary.trim().to_string(),
        };
        let refused_at = changeset.entries.iter().enumerate().position(|(index, entry)| {
            entry.name.contains(['"', '\r', '\n']) || changeset.entries[..index].iter().any(|e| e.name == entry.name)
        });
        match (write_changeset(&changeset), refused_at) {
            (Ok(written), None) => prop_assert_eq!(parse_changeset(&written).unwrap(), changeset),
            (Err(WriteError::NameNotRepresentable { index, .. }), Some(expected)) => prop_assert_eq!(index, expected),
            (result, expected) => prop_assert!(false, "{:?} for {:?}, expected refusal at {:?}", result, changeset, expected),
        }
    }
}
