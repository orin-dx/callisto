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
