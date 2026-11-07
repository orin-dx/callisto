//! Every Markdown pass over the same documents. A pass that disagrees with the others about what is code or what is a
//! heading fails here, so a new pass joins the table instead of growing its own scanner.

use callisto_model::Version;

use super::fences::Fences;
use super::headings::{heading_levels, rebase_headings};
use crate::changelog::write::{find_section, has_heading};

struct Case {
    name: &'static str,
    text: &'static str,
    /// Zero-based lines that are fenced code, fence lines included.
    code: &'static [usize],
    /// `(line, level)` of each heading outside code.
    headings: &'static [(usize, usize)],
}

const CASES: &[Case] = &[
    Case {
        name: "plain",
        text: "Intro line.\n\n## One\n\ntext\n\n### Two\n",
        code: &[],
        headings: &[(2, 2), (6, 3)],
    },
    Case {
        name: "backtick fence",
        text: "## A\n```sh\n# c\n## d\n```\n## B",
        code: &[1, 2, 3, 4],
        headings: &[(0, 2), (5, 2)],
    },
    Case {
        name: "tilde fence",
        text: "~~~\n# t\n~~~\n## B",
        code: &[0, 1, 2],
        headings: &[(3, 2)],
    },
    Case {
        name: "longer closing fence",
        text: "````\n```\n# in\n````\n## After",
        code: &[0, 1, 2, 3],
        headings: &[(4, 2)],
    },
    Case {
        name: "fence opened on a bullet and closed indented",
        text: "- ```md\n  # Title\n  ```\n## After",
        code: &[0, 1, 2],
        headings: &[(3, 2)],
    },
    Case {
        name: "fence opened on a numbered item",
        text: "1. ```sh\n   ## x\n   ```\n## After",
        code: &[0, 1, 2],
        headings: &[(3, 2)],
    },
    Case {
        name: "inline triple backticks are not a fence",
        text: "```a``` text\n## Real",
        code: &[],
        headings: &[(1, 2)],
    },
    Case {
        name: "indented code is not a heading",
        text: "    ## not\n## Real",
        code: &[],
        headings: &[(1, 2)],
    },
    Case {
        name: "a hashtag is not a heading",
        text: "#hashtag\n## Real",
        code: &[],
        headings: &[(1, 2)],
    },
    Case {
        name: "a title ending in a hash",
        text: "### C#\n## Real",
        code: &[],
        headings: &[(0, 3), (1, 2)],
    },
    Case {
        name: "an unclosed fence runs to the end",
        text: "## A\n```\n## never closed\n",
        code: &[1, 2],
        headings: &[(0, 2)],
    },
    Case {
        name: "CRLF line endings",
        text: "```\r\n# x\r\n```\r\n## Real",
        code: &[0, 1, 2],
        headings: &[(3, 2)],
    },
];

#[test]
fn the_fence_tracker_marks_the_expected_lines() {
    for case in CASES {
        let mut fences = Fences::default();
        let code: Vec<usize> = case
            .text
            .lines()
            .enumerate()
            .filter_map(|(index, line)| fences.is_code(line).then_some(index))
            .collect();
        assert_eq!(code, case.code, "{}", case.name);
    }
}

#[test]
fn the_heading_pass_finds_the_expected_headings_and_none_in_code() {
    for case in CASES {
        let lines: Vec<&str> = case.text.split_inclusive('\n').collect();
        let found: Vec<(usize, usize)> = heading_levels(&lines)
            .into_iter()
            .enumerate()
            .filter_map(|(index, level)| level.map(|level| (index, level)))
            .collect();
        assert_eq!(found, case.headings, "{}", case.name);
        assert!(
            found.iter().all(|(index, _)| !case.code.contains(index)),
            "{}: a heading inside code",
            case.name
        );
    }
}

#[test]
fn the_duplicate_heading_check_sees_only_headings_outside_code() {
    for case in CASES {
        for (index, line) in case
            .text
            .lines()
            .enumerate()
            .filter(|(_, line)| line.starts_with("## "))
        {
            let outside_code = !case.code.contains(&index);
            assert_eq!(has_heading(case.text, line), outside_code, "{}: {line:?}", case.name);
        }
    }
}

#[test]
fn a_section_ends_at_the_first_top_level_heading_outside_code() {
    let version = Version::semver(1, 0, 0);
    for case in CASES {
        let document = format!("## 1.0.0\n\n{}\n## 0.9.0\n\nolder entry\n", case.text);
        let end = case
            .headings
            .iter()
            .find(|(_, level)| *level <= 2)
            .map_or(case.text.lines().count(), |(index, _)| *index);
        let expected: String = case.text.lines().take(end).collect::<Vec<_>>().join("\n");

        let found = find_section(&document, &version).map(|section| section.replace("\r\n", "\n"));
        assert_eq!(found.as_deref(), Some(expected.trim()), "{}", case.name);
    }
}

#[test]
fn rebasing_changes_only_heading_lines_and_is_idempotent() {
    for case in CASES {
        let rebased = rebase_headings(case.text, 4);
        assert_eq!(rebase_headings(&rebased, 4), rebased, "{}: not idempotent", case.name);
        for (index, (before, after)) in case.text.lines().zip(rebased.lines()).enumerate() {
            let is_heading = case.headings.iter().any(|(line, _)| *line == index);
            if !is_heading {
                assert_eq!(before, after, "{}: line {index} changed", case.name);
            }
        }
    }
}
