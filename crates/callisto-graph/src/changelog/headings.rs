//! Heading levels for Markdown an author wrote without knowing where it would land.

use crate::changelog::fences::Fences;

/// Shifts the headings in `text` so its shallowest one sits at `first_level`, keeping their relative depth. Only the
/// `#` run changes; every other byte passes through. Fenced code and indented code are left alone. A heading pushed
/// past level 6 becomes a bold line, so no text is lost.
pub fn rebase_headings(text: &str, first_level: usize) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let headings = heading_levels(&lines);
    let Some(shallowest) = headings.iter().flatten().copied().min() else {
        return text.to_owned();
    };

    let mut out = String::with_capacity(text.len());
    for (line, level) in lines.iter().zip(&headings) {
        let Some(level) = level else {
            out.push_str(line);
            continue;
        };
        let target = level + first_level - shallowest;
        out.push_str(&retarget(line, *level, target));
    }
    out
}

/// The ATX heading level of each line, `None` for a line that is not a heading or sits inside a fence.
fn heading_levels(lines: &[&str]) -> Vec<Option<usize>> {
    let mut fences = Fences::default();
    lines
        .iter()
        .map(|line| {
            let body = line.trim_end_matches(['\n', '\r']);
            if fences.is_code(body) {
                return None;
            }
            let indent = body.len() - body.trim_start_matches(' ').len();
            if indent >= 4 {
                return None;
            }
            let trimmed = &body[indent..];
            let hashes = trimmed.chars().take_while(|c| *c == '#').count();
            let after = &trimmed[hashes..];
            ((1..=6).contains(&hashes) && (after.is_empty() || after.starts_with([' ', '\t']))).then_some(hashes)
        })
        .collect()
}

fn retarget(line: &str, level: usize, target: usize) -> String {
    let body = line.trim_end_matches(['\n', '\r']);
    let ending = &line[body.len()..];
    let indent = body.len() - body.trim_start_matches(' ').len();
    let after = &body[indent + level..];
    if target <= 6 {
        return format!("{}{}{after}{ending}", &body[..indent], "#".repeat(target));
    }
    let title = without_closing_hashes(after.trim());
    if title.is_empty() {
        return ending.to_owned();
    }
    format!("{}**{title}**{ending}", &body[..indent])
}

/// `title` without an ATX closing sequence: trailing `#`s that follow a space. `C#` keeps its hash.
fn without_closing_hashes(title: &str) -> &str {
    let stripped = title.trim_end_matches('#');
    if stripped.len() == title.len() {
        return title;
    }
    if stripped.is_empty() || stripped.ends_with([' ', '\t']) {
        stripped.trim_end()
    } else {
        title
    }
}

#[cfg(test)]
mod tests {
    use super::rebase_headings;

    #[test]
    fn the_shallowest_heading_lands_on_the_first_level_and_depth_is_kept() {
        assert_eq!(
            rebase_headings("## A\n\ntext\n\n### B\n", 5),
            "##### A\n\ntext\n\n###### B\n"
        );
        assert_eq!(rebase_headings("# A\n## B\n", 3), "### A\n#### B\n");
    }

    #[test]
    fn text_without_headings_is_returned_unchanged() {
        let text = "just text\n\n- a list\n";
        assert_eq!(rebase_headings(text, 4), text);
        assert_eq!(rebase_headings("", 4), "");
    }

    #[test]
    fn hashes_inside_fences_and_indented_code_are_not_headings() {
        let text = "## Real\n\n```sh\n# comment\n## also a comment\n```\n\n    ## indented code\n\n~~~\n# tilde\n~~~\n";
        let expected =
            "#### Real\n\n```sh\n# comment\n## also a comment\n```\n\n    ## indented code\n\n~~~\n# tilde\n~~~\n";
        assert_eq!(rebase_headings(text, 4), expected);
    }

    #[test]
    fn a_longer_closing_fence_ends_the_block_and_a_shorter_one_does_not() {
        let text = "## A\n````\n```\n# inside\n````\n## B\n";
        assert_eq!(rebase_headings(text, 3), "### A\n````\n```\n# inside\n````\n### B\n");
    }

    #[test]
    fn a_hash_without_a_space_is_not_a_heading() {
        let text = "#hashtag\n\n## Real\n";
        assert_eq!(rebase_headings(text, 3), "#hashtag\n\n### Real\n");
    }

    #[test]
    fn a_heading_past_level_six_becomes_bold_and_closing_hashes_go() {
        assert_eq!(rebase_headings("# A\n### B ###\n", 5), "##### A\n**B**\n");
    }

    #[test]
    fn line_endings_and_trailing_text_are_kept() {
        assert_eq!(rebase_headings("## A\r\ntext\r\n", 4), "#### A\r\ntext\r\n");
        assert_eq!(rebase_headings("## A", 3), "### A");
        assert_eq!(rebase_headings("  ## Indented\n", 3), "  ### Indented\n");
    }

    #[test]
    fn a_fence_opened_on_a_bullet_hides_its_indented_headings() {
        let text = "## Real\n\n- ```md\n  # Title\n  ```\n\n## Later\n";
        assert_eq!(
            rebase_headings(text, 4),
            "#### Real\n\n- ```md\n  # Title\n  ```\n\n#### Later\n"
        );
    }

    #[test]
    fn a_literal_hash_ending_a_title_is_not_a_closing_sequence() {
        assert_eq!(rebase_headings("# Migration\n### C#\n", 5), "##### Migration\n**C#**\n");
        assert_eq!(rebase_headings("# A\n### C# ###\n", 5), "##### A\n**C#**\n");
        assert_eq!(rebase_headings("# A\n### ###\n", 5), "##### A\n\n");
    }
}
