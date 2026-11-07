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

/// `text` without a leading list marker (`-`, `*`, `+`, `1.` or `1)`), so a fence opened on a bullet's first line is seen.
fn strip_list_marker(text: &str) -> &str {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = text.strip_prefix(marker) {
            return rest.trim_start();
        }
    }
    let after_digits = text.trim_start_matches(|c: char| c.is_ascii_digit());
    if after_digits.len() < text.len() && (after_digits.starts_with(". ") || after_digits.starts_with(") ")) {
        return after_digits[2..].trim_start();
    }
    text
}

/// The marker character and length of a fence that `text` opens (three or more backticks or tildes). A backtick fence's
/// info string cannot hold a backtick, which tells it from inline code such as ```` ```a``` ````.
fn fence_opening(text: &str) -> Option<(char, usize)> {
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
}
