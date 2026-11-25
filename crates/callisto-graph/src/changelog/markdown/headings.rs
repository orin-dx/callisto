//! Heading detection for Markdown an author wrote without knowing where it would land.

#[cfg(test)]
use super::fences::Fences;

/// The ATX heading level of each line, `None` for a line that is not a heading or sits inside a fence.
#[cfg(test)]
pub(super) fn heading_levels(lines: &[&str]) -> Vec<Option<usize>> {
    let mut fences = Fences::default();
    lines
        .iter()
        .map(|line| {
            let body = line.trim_end_matches(['\n', '\r']);
            if fences.is_code(body) {
                return None;
            }
            atx_level(body)
        })
        .collect()
}

/// The level of `line` as an ATX heading: one to six `#` after at most three spaces, then a space, a tab or the end of
/// the line. `#hashtag` is text.
pub(super) fn atx_level(line: &str) -> Option<usize> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent >= 4 {
        return None;
    }
    let trimmed = &line[indent..];
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    let after = &trimmed[hashes..];
    ((1..=6).contains(&hashes) && (after.is_empty() || after.starts_with([' ', '\t']))).then_some(hashes)
}
