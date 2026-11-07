//! The one place that decides which lines of a Markdown text are fenced code, so the changelog code that walks lines
//! (finding a section, spotting a duplicate heading, rebasing headings) cannot disagree about it.

/// Tracks fenced code line by line. A fence may open on a list item's first line (`- ```sh`) and close indented, as a
/// changeset's example under a bullet does.
#[derive(Default)]
pub(crate) struct Fences {
    open: Option<(char, usize)>,
}

impl Fences {
    /// Feeds the next line and returns whether it is fenced code, counting the opening and closing fence lines.
    pub(crate) fn is_code(&mut self, line: &str) -> bool {
        let content = line.trim_start();
        match self.open {
            None => match fence_opening(strip_list_marker(content)) {
                Some(open) => {
                    self.open = Some(open);
                    true
                }
                None => false,
            },
            Some((marker, length)) => {
                if closes_fence(content, marker, length) {
                    self.open = None;
                }
                true
            }
        }
    }
}

/// `text` without a leading list marker, so a fence opened on a bullet's first line is seen. A marker is `-`, `*` or
/// `+`, or up to nine digits and a `.` or `)`, followed by a space, a tab or the end of the line. `text` itself comes back
/// when it has none, so `1.5x`, `**bold**` and `---` are not markers.
pub(super) fn strip_list_marker(text: &str) -> &str {
    let after_bullet = text.strip_prefix(['-', '*', '+']);
    let rest = after_bullet.or_else(|| {
        let digits = text.chars().take_while(char::is_ascii_digit).count();
        let numbered = (1..=9).contains(&digits) && text[digits..].starts_with(['.', ')']);
        numbered.then(|| &text[digits + 1..])
    });
    match rest {
        Some(rest) if rest.is_empty() || rest.starts_with([' ', '\t']) => rest.trim_start(),
        _ => text,
    }
}

/// The marker character and length of a fence that `text` opens (three or more backticks or tildes). A backtick fence's
/// info string cannot hold a backtick, which tells it from inline code such as ```` ```a``` ````.
pub(super) fn fence_opening(text: &str) -> Option<(char, usize)> {
    let marker = text.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let length = text.chars().take_while(|c| *c == marker).count();
    (length >= 3 && !(marker == '`' && text[length..].contains('`'))).then_some((marker, length))
}

/// Whether `text` closes a fence opened with `length` of `marker`: the same character, at least as many, nothing else.
fn closes_fence(text: &str, marker: char, length: usize) -> bool {
    let run = text.chars().take_while(|c| *c == marker).count();
    run >= length && text[run..].trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::Fences;

    fn classify(text: &str) -> Vec<bool> {
        let mut fences = Fences::default();
        text.lines().map(|line| fences.is_code(line)).collect()
    }

    #[test]
    fn a_fence_and_its_contents_are_code() {
        assert_eq!(classify("a\n```sh\n# x\n```\nb"), [false, true, true, true, false]);
        assert_eq!(classify("~~~\n# x\n~~~"), [true, true, true]);
    }

    #[test]
    fn a_fence_opened_on_a_bullet_closes_indented() {
        assert_eq!(classify("- ```md\n  # Title\n  ```\n# Real"), [true, true, true, false]);
        assert_eq!(classify("1. ```\n   # Title\n   ```"), [true, true, true]);
    }

    #[test]
    fn a_shorter_or_different_fence_does_not_close() {
        assert_eq!(classify("````\n```\n# x\n````\n# y"), [true, true, true, true, false]);
        assert_eq!(classify("```\n~~~\n# x\n```"), [true, true, true, true]);
    }

    #[test]
    fn inline_code_on_one_line_is_not_a_fence() {
        assert_eq!(classify("```a``` text\n# Real"), [false, false]);
    }

    #[test]
    fn a_list_marker_is_followed_by_a_space_a_tab_or_the_end_of_the_line() {
        use super::strip_list_marker;
        for (text, rest) in [
            ("- item", "item"),
            ("* item", "item"),
            ("+ item", "item"),
            ("1. item", "item"),
            ("12) item", "item"),
            ("1.\titem", "item"),
            ("-\titem", "item"),
            ("1.", ""),
            ("-", ""),
        ] {
            assert_eq!(strip_list_marker(text), rest, "{text:?}");
        }
        for text in [
            "1.5x faster",
            "**bold**",
            "---",
            "-item",
            "1234567890. too many digits",
            "1 . spaced",
            "word",
        ] {
            assert_eq!(strip_list_marker(text), text, "{text:?}");
        }
    }

    #[test]
    fn a_fence_opened_after_a_tab_separated_marker_is_a_fence() {
        assert_eq!(classify("1.\t```sh\n   # x\n   ```\n# Real"), [true, true, true, false]);
    }
}
