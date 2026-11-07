//! A changeset summary's headline.

use super::fences::{fence_opening, strip_list_marker};
use super::headings::atx_level;

const HEADLINE_MAX_CHARS: usize = 120;

/// A changeset summary split into a one-line headline and the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SplitSummary {
    /// The first paragraph, on one line. `None` when the summary does not open with a plain paragraph.
    pub(crate) headline: Option<String>,
    /// What the headline does not already say. Empty for a one-paragraph summary.
    pub(crate) detail: String,
}

/// The headline is the summary's first paragraph. A summary opening with a heading, list, quote, table, fence, image or
/// HTML has none, and its whole text is the detail. A paragraph past 120 characters is cut for the headline and
/// kept whole in the detail, so nothing the author wrote is lost.
pub(crate) fn split_summary(summary: &str) -> SplitSummary {
    let text = summary.trim();
    let paragraph: Vec<&str> = text.lines().take_while(|line| !line.trim().is_empty()).collect();
    let opens_a_block = paragraph.first().is_none_or(|line| opens_a_block(line));
    if opens_a_block {
        return SplitSummary {
            headline: None,
            detail: text.to_owned(),
        };
    }
    let headline = paragraph.iter().map(|line| line.trim()).collect::<Vec<_>>().join(" ");
    if headline.chars().count() > HEADLINE_MAX_CHARS {
        let cut: String = headline.chars().take(HEADLINE_MAX_CHARS - 1).collect();
        return SplitSummary {
            headline: Some(format!("{}…", cut.trim_end())),
            detail: text.to_owned(),
        };
    }
    let rest = text
        .lines()
        .skip(paragraph.len())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    SplitSummary {
        headline: Some(headline),
        detail: rest,
    }
}

/// Whether `line` opens something other than a plain paragraph. It asks the same questions the other passes do, so a
/// heading, list item or fence means here what it means there.
fn opens_a_block(line: &str) -> bool {
    let line = line.trim_start();
    atx_level(line).is_some()
        || strip_list_marker(line) != line
        || fence_opening(line).is_some()
        || ["---", "***", "___"].iter().any(|rule| line.starts_with(rule))
        || line.starts_with(['>', '|', '<'])
        || line.starts_with("![")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_paragraph_summary_is_all_headline() {
        let split = split_summary("Uploads now retry.\n");
        assert_eq!(split.headline.as_deref(), Some("Uploads now retry."));
        assert_eq!(split.detail, "");
    }

    #[test]
    fn later_paragraphs_become_the_detail() {
        let split = split_summary("Uploads now retry\nwith backoff.\n\n## Migration\n\nSet `retries = 3`.");
        assert_eq!(split.headline.as_deref(), Some("Uploads now retry with backoff."));
        assert_eq!(split.detail, "## Migration\n\nSet `retries = 3`.");
    }

    #[test]
    fn a_long_headline_is_cut_and_the_detail_keeps_the_whole_text() {
        let long = "word ".repeat(40);
        let split = split_summary(&long);
        let headline = split.headline.unwrap();
        assert!(headline.ends_with('…') && headline.chars().count() <= HEADLINE_MAX_CHARS);
        assert_eq!(split.detail, long.trim());
    }

    #[test]
    fn a_summary_opening_with_a_block_has_no_headline() {
        for opening in [
            "## Migration\n\nText.",
            "- one\n- two",
            "1. first",
            "> quoted",
            "| a | b |\n| - | - |",
            "```sh\nx\n```",
            "~~~\nx\n~~~",
            "![alt](img.png)",
            "<details>",
            "---",
        ] {
            let split = split_summary(opening);
            assert_eq!(split.headline, None, "{opening}");
            assert_eq!(split.detail, opening.trim());
        }
    }

    #[test]
    fn text_that_only_looks_like_a_block_is_a_headline() {
        for (text, headline) in [
            ("`retries` now back off.", "`retries` now back off."),
            ("#hashtag support lands.", "#hashtag support lands."),
            ("**Breaking:** drops the old API.", "**Breaking:** drops the old API."),
            ("*Faster* uploads.", "*Faster* uploads."),
            ("!important flag added.", "!important flag added."),
            ("~Approximately~ right.", "~Approximately~ right."),
        ] {
            let split = split_summary(text);
            assert_eq!(split.headline.as_deref(), Some(headline), "{text}");
            assert_eq!(split.detail, "");
        }
    }

    #[test]
    fn an_empty_summary_has_no_headline() {
        assert_eq!(
            split_summary("  \n"),
            SplitSummary {
                headline: None,
                detail: String::new()
            }
        );
    }

    #[test]
    fn a_tab_separated_or_bare_list_marker_opens_a_block() {
        for opening in [
            "1.\tStop the service\n2.\tRun the migration",
            "-\tone\n-\ttwo",
            "1.",
            "-",
        ] {
            assert_eq!(split_summary(opening).headline, None, "{opening:?}");
        }
        assert_eq!(
            split_summary("1.5x faster uploads.").headline.as_deref(),
            Some("1.5x faster uploads.")
        );
    }

    /// The classifier this module replaced. The new one may call more lines paragraphs, but only the kinds below.
    fn old_starts_a_block(line: &str) -> bool {
        let digits = line.chars().take_while(char::is_ascii_digit).count();
        let numbered = digits > 0 && matches!(line[digits..].chars().next(), Some('.' | ')'));
        numbered
            || line.starts_with(['#', '>', '|', '!', '<', '`', '~'])
            || ["- ", "* ", "+ ", "---", "***"]
                .iter()
                .any(|marker| line.starts_with(marker))
    }

    /// What the new classifier deliberately reads as a paragraph where the old one read a block.
    fn deliberately_a_paragraph(line: &str) -> bool {
        let digits = line.chars().take_while(char::is_ascii_digit).count();
        let numbered_text = digits > 0
            && matches!(line[digits..].chars().next(), Some('.' | ')'))
            && !line[digits + 1..].is_empty()
            && !line[digits + 1..].starts_with([' ', '\t']);
        numbered_text || line.starts_with(['#', '!', '`', '~'])
    }

    proptest::proptest! {
        #[test]
        fn the_new_classifier_only_frees_text_the_old_one_wrongly_blocked(
            marker in proptest::sample::select(vec!["-", "*", "+", "#", "##", ">", "|", "!", "<", "`", "``", "```", "~", "---", "1.", "12)", "1.5", ""]),
            separator in proptest::sample::select(vec!["", " ", "\t", "  ", "x"]),
            tail in "[a-z`# ]{0,4}",
        ) {
            let line = format!("{marker}{separator}{tail}");
            if old_starts_a_block(&line) && !opens_a_block(&line) {
                proptest::prop_assert!(deliberately_a_paragraph(&line), "{line:?} lost its block status");
            }
        }
    }
}
