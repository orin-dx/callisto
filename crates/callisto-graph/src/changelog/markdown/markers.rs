//! `Breaking:` and `Action:` markers in a changeset summary.

use super::fences::{strip_list_marker, Fences};
use super::headings::atx_level;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkerKind {
    Breaking,
    Action,
}

/// A marker paragraph: its kind and its text, marker word removed and lines joined by single spaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Marker {
    pub(crate) kind: MarkerKind,
    pub(crate) text: String,
}

const PREFIXES: [(&str, MarkerKind); 4] = [
    ("**Breaking:**", MarkerKind::Breaking),
    ("**Action:**", MarkerKind::Action),
    ("Breaking:", MarkerKind::Breaking),
    ("Action:", MarkerKind::Action),
];

/// The markers of `summary` in order. A marker is a paragraph whose first line starts at column 0 with `Breaking:` or
/// `Action:`, optionally bold. Fenced code, list items and indented code never hold one, and a marker with no text is
/// not a marker.
pub(crate) fn extract_markers(summary: &str) -> Vec<Marker> {
    let mut markers = Vec::new();
    let mut fences = Fences::default();
    let mut paragraph: Vec<&str> = Vec::new();
    for line in summary.lines() {
        let skipped = fences.is_code(line) || line.trim().is_empty() || atx_level(line).is_some();
        if skipped || ends_paragraph(&paragraph, line) {
            markers.extend(marker_of(&paragraph));
            paragraph.clear();
        }
        if !skipped {
            paragraph.push(line);
        }
    }
    markers.extend(marker_of(&paragraph));
    markers
}

/// Whether `line` cannot continue `paragraph`: it opens a heading or list item, or ends an indented code block.
fn ends_paragraph(paragraph: &[&str], line: &str) -> bool {
    let Some(first) = paragraph.first() else {
        return false;
    };
    let trimmed = line.trim_start();
    atx_level(line).is_some()
        || strip_list_marker(trimmed) != trimmed
        || (first.starts_with("    ") && !line.starts_with("    "))
}

fn marker_of(paragraph: &[&str]) -> Option<Marker> {
    let first = paragraph.first()?;
    let (prefix, kind) = PREFIXES.iter().find(|(prefix, _)| first.starts_with(prefix))?;
    let text = std::iter::once(&first[prefix.len()..])
        .chain(paragraph[1..].iter().copied())
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then_some(Marker { kind: *kind, text })
}

#[cfg(test)]
mod tests {
    use super::MarkerKind::{Action, Breaking};
    use super::*;

    fn found(summary: &str) -> Vec<(MarkerKind, String)> {
        extract_markers(summary).into_iter().map(|m| (m.kind, m.text)).collect()
    }

    fn one(kind: MarkerKind, text: &str) -> Vec<(MarkerKind, String)> {
        vec![(kind, text.to_owned())]
    }

    #[test]
    fn a_marker_right_under_a_heading_is_found() {
        assert_eq!(
            found("## Migration\nBreaking: removes the old API."),
            one(Breaking, "removes the old API.")
        );
        assert_eq!(found("# Notes\nAction: re-run init.\n"), one(Action, "re-run init."));
        assert_eq!(
            found("Breaking: a\n## Heading\nAction: b"),
            [(Breaking, "a".to_owned()), (Action, "b".to_owned())]
        );
        assert_eq!(found("## Migration\nJust text."), []);
    }

    #[test]
    fn each_kind_is_found() {
        assert_eq!(found("Breaking: drops the flag."), one(Breaking, "drops the flag."));
        assert_eq!(found("Action: run the migration."), one(Action, "run the migration."));
    }

    #[test]
    fn the_headline_paragraph_may_be_a_marker() {
        assert_eq!(found("Breaking: x\n\nMore."), one(Breaking, "x"));
    }

    #[test]
    fn bold_forms_are_found() {
        assert_eq!(found("**Breaking:** drops it."), one(Breaking, "drops it."));
        assert_eq!(found("**Action:**  run it."), one(Action, "run it."));
    }

    #[test]
    fn a_multi_line_paragraph_joins_with_single_spaces() {
        assert_eq!(
            found("Intro.\n\nBreaking: first\n  second   line\nthird\n\nafter"),
            one(Breaking, "first second   line third")
        );
    }

    #[test]
    fn several_markers_keep_their_order() {
        assert_eq!(
            found("Intro.\n\nAction: a\n\nBreaking: b\n\nAction: c"),
            [
                (Action, "a".to_owned()),
                (Breaking, "b".to_owned()),
                (Action, "c".to_owned())
            ]
        );
    }

    #[test]
    fn markers_in_fences_are_not_markers() {
        for text in [
            "```\nBreaking: x\n```",
            "~~~\nBreaking: x\n~~~",
            "- ```md\n  Breaking: x\n  ```",
            "````\n```\nBreaking: x\n````",
            "```\nAction: x\n```",
            "```\n\nBreaking: x\n```",
            "Intro.\n```\nBreaking: x\n```",
        ] {
            assert_eq!(found(text), [], "{text:?}");
        }
        assert_eq!(found("```\nBreaking: x\n```\n\nBreaking: y"), one(Breaking, "y"));
    }

    #[test]
    fn markers_in_list_items_are_not_markers() {
        for text in [
            "- Breaking: x",
            "1. Breaking: x",
            "- item\n  Breaking: x",
            "- item\nBreaking: x",
        ] {
            assert_eq!(found(text), [], "{text:?}");
        }
    }

    #[test]
    fn indented_code_is_not_a_marker() {
        for text in ["    Breaking: x", "Intro.\n\n    Breaking: x", "  Breaking: x"] {
            assert_eq!(found(text), [], "{text:?}");
        }
        assert_eq!(found("Intro.\n\n    code\nBreaking: x"), one(Breaking, "x"));
    }

    #[test]
    fn the_word_is_case_sensitive_and_needs_its_colon() {
        for text in [
            "breaking: x",
            "BREAKING: x",
            "action: x",
            "Breaking x",
            "Breaking",
            "Breakingly: x",
        ] {
            assert_eq!(found(text), [], "{text:?}");
        }
    }

    #[test]
    fn a_marker_without_text_is_not_a_marker() {
        for text in [
            "Breaking:",
            "Breaking:   ",
            "**Action:**",
            "Breaking:\n\nNext paragraph.",
        ] {
            assert_eq!(found(text), [], "{text:?}");
        }
        assert_eq!(
            found("Breaking:\ntext on the next line"),
            one(Breaking, "text on the next line")
        );
    }

    #[test]
    fn crlf_line_endings_are_handled() {
        assert_eq!(
            found("Intro.\r\n\r\nBreaking: a\r\nb\r\n\r\n```\r\nAction: x\r\n```\r\n"),
            one(Breaking, "a b")
        );
    }

    #[test]
    fn a_list_or_heading_ends_the_marker_paragraph() {
        assert_eq!(found("Breaking: a\n- item"), one(Breaking, "a"));
        assert_eq!(found("Action: a\n## Heading"), one(Action, "a"));
    }

    proptest::proptest! {
        #[test]
        fn a_marker_in_a_fence_is_hidden_and_at_top_level_is_found(
            text in "[a-z]{1,8}( [a-z]{1,8}){0,3}",
            fence in proptest::sample::select(vec!["```", "~~~", "````", "~~~~~"]),
            word in proptest::sample::select(vec!["Breaking:", "Action:", "**Breaking:**", "**Action:**"]),
        ) {
            let fenced = format!("Headline.\n\n{fence}\n{word} {text}\n{fence}\n");
            proptest::prop_assert_eq!(extract_markers(&fenced), vec![]);
            let top = extract_markers(&format!("Headline.\n\n{word} {text}\n"));
            proptest::prop_assert_eq!(top.len(), 1);
            proptest::prop_assert_eq!(&top[0].text, &text);
        }
    }
}
