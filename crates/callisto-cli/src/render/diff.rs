use similar::{ChangeTag, TextDiff};

pub fn render_unified_diff(old_text: &str, new_text: &str, header: &str) -> String {
    let mut out = format!("\x1b[1m--- {header} (original)\n+++ {header} (proposed)\x1b[0m\n");
    let diff = TextDiff::from_lines(old_text, new_text);
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete => out.push_str(&format!("\x1b[31m-{change}\x1b[0m")),
            ChangeTag::Insert => out.push_str(&format!("\x1b[32m+{change}\x1b[0m")),
            ChangeTag::Equal => out.push_str(&format!(" {change}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_header_with_original_and_proposed_labels() {
        let rendered = render_unified_diff("a\n", "a\n", "Cargo.toml");
        assert!(rendered.contains("--- Cargo.toml (original)"));
        assert!(rendered.contains("+++ Cargo.toml (proposed)"));
    }

    #[test]
    fn renders_deleted_and_inserted_lines_with_distinct_markers() {
        let rendered = render_unified_diff("old line\n", "new line\n", "h");
        assert!(rendered.contains("-old line"), "got:\n{rendered}");
        assert!(rendered.contains("+new line"), "got:\n{rendered}");
    }

    #[test]
    fn renders_equal_lines_with_space_prefix() {
        let rendered = render_unified_diff("same\n", "same\n", "h");
        assert!(rendered.contains(" same\n"), "got:\n{rendered}");
        assert!(!rendered.contains("-same"));
        assert!(!rendered.contains("+same"));
    }

    #[test]
    fn empty_inputs_produce_only_the_header() {
        let rendered = render_unified_diff("", "", "h");
        let expected = "\x1b[1m--- h (original)\n+++ h (proposed)\x1b[0m\n";
        assert_eq!(rendered, expected);
    }
}
