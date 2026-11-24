//! The one contract for interactive prompts: where they may run, how they end, and how `$EDITOR` is used.

use std::io::IsTerminal as _;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::cli::OutputFormat;
use crate::error::CliError;

/// Proof that this run may prompt; only [`Prompts::detect`] makes one.
pub struct Prompts(());

impl Prompts {
    /// Prompts need a person at the terminal (stdin, and stderr where they draw) and a text run: `--format json` is for
    /// scripts and never prompts.
    pub fn detect(format: OutputFormat) -> Option<Self> {
        (format == OutputFormat::Text && std::io::stdin().is_terminal() && std::io::stderr().is_terminal())
            .then_some(Self(()))
    }

    #[cfg(test)]
    pub fn for_tests() -> Self {
        Self(())
    }
}

/// The two list prompts: what each draws beside its options.
#[derive(Clone, Copy)]
pub enum ListKind {
    /// Pick one: `> ` before each option.
    Select,
    /// Tick any: `> [x] ` before each option.
    MultiSelect,
}

impl ListKind {
    /// Columns drawn before each option's label.
    const fn prefix(self) -> usize {
        match self {
            Self::Select => 2,
            Self::MultiSelect => 6,
        }
    }

    /// The keys, in ASCII so a terminal that cannot draw arrows still reads it.
    pub const fn help(self) -> &'static str {
        match self {
            Self::Select => "Up/Down move, Enter selects, type to filter",
            Self::MultiSelect => "Up/Down move, Space toggles, Right all, Left none, type to filter",
        }
    }
}

/// How a list prompt is drawn so that its frame fits the terminal.
pub struct ListLayout {
    pub page_size: usize,
    /// One terminal row each; the same order and count as the labels given.
    pub labels: Vec<String>,
}

/// Fits a list to the terminal. Inquire redraws relative to the previous frame, so a frame taller than the terminal
/// puts the highlight and ticks on the wrong rows. When the size is unknown the page is `wanted` and labels are kept.
pub fn fit_list(kind: ListKind, prompt: &str, wanted: usize, labels: &[String]) -> ListLayout {
    fit_list_to(crossterm::terminal::size().ok(), kind, prompt, wanted, labels)
}

/// [`fit_list`] on a terminal of `size` (columns, rows).
fn fit_list_to(size: Option<(u16, u16)>, kind: ListKind, prompt: &str, wanted: usize, labels: &[String]) -> ListLayout {
    let Some((columns, rows)) = size.map(|(columns, rows)| (usize::from(columns).max(1), usize::from(rows))) else {
        return ListLayout {
            page_size: wanted,
            labels: labels.to_vec(),
        };
    };
    let wrapped = |text: &str| UnicodeWidthStr::width(text).div_ceil(columns).max(1);
    // `? <prompt> ` then `[<help>]`.
    let chrome = wrapped(&format!("? {prompt} ")) + wrapped(&format!("[{}]", kind.help()));
    // One row stays free: a frame as tall as the terminal scrolls it.
    let room = rows.saturating_sub(chrome + 1).max(1);
    let label_width = columns.saturating_sub(kind.prefix()).max(1);
    ListLayout {
        page_size: wanted.min(room).max(1),
        labels: labels.iter().map(|label| truncate(label, label_width)).collect(),
    }
}

/// `text` cut to `width` columns, ending in `...` when something was cut.
fn truncate(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    let ellipsis = width > 3;
    let keep = if ellipsis { width - 3 } else { width };
    let mut cut = String::new();
    let mut used = 0;
    for character in text.chars() {
        let next = used + UnicodeWidthChar::width(character).unwrap_or(0);
        if next > keep {
            break;
        }
        used = next;
        cut.push(character);
    }
    if ellipsis {
        cut.push_str("...");
    }
    cut
}

/// Printed whenever a prompt run ends without writing.
pub const CANCELLED: &str = "Cancelled, nothing written.";

/// Exit status for Ctrl-C, as shells report SIGINT.
pub const CANCELLED_BY_CTRL_C: u8 = 130;

/// The editor named by `$VISUAL`, else `$EDITOR`.
pub struct Editor {
    variable: &'static str,
    command: String,
}

/// Why the editor could not be used; its message is the warning to print.
#[derive(Debug, thiserror::Error)]
#[error("cannot use ${variable} (`{command}`): {reason}; asking for the summary here instead")]
pub struct EditorUnusable {
    variable: &'static str,
    command: String,
    reason: String,
}

impl Editor {
    /// An empty or blank setting is unset.
    pub fn from_env() -> Option<Self> {
        ["VISUAL", "EDITOR"].into_iter().find_map(|variable| {
            let command = std::env::var(variable).ok()?;
            (!command.trim().is_empty()).then_some(Self { variable, command })
        })
    }

    /// `VISUAL` or `EDITOR`, whichever named it.
    pub fn variable(&self) -> &'static str {
        self.variable
    }

    /// Opens the editor on an empty file and returns what was saved. `None` when the editor exits non-zero (an abort,
    /// as `:cq` in Vim). The editor draws on stderr so stdout stays for the report.
    pub fn edit(self) -> Result<Option<String>, EditorUnusable> {
        self.try_edit().map_err(|reason| EditorUnusable {
            variable: self.variable,
            command: self.command,
            reason,
        })
    }

    fn try_edit(&self) -> Result<Option<String>, String> {
        let words =
            shell_words::split(&self.command).map_err(|error| format!("it does not split into words: {error}"))?;
        let Some((program, args)) = words.split_first() else {
            return Err("it names no program".to_owned());
        };
        let failed = |error: std::io::Error| error.to_string();
        let file = tempfile::Builder::new()
            .prefix("callisto-")
            .suffix(".txt")
            .tempfile()
            .map_err(failed)?;
        let _editor_owns_ctrl_c = editor_owns_ctrl_c::enter().map_err(failed)?;
        let status = crate::runner::child_command(program, &[], &[])
            .args(args)
            .arg(file.path())
            .stdout(std::io::stderr())
            .status()
            .map_err(|error| format!("`{program}` did not start: {error}"))?;
        if !status.success() {
            return Ok(None);
        }
        let saved = std::fs::read(file.path()).map_err(failed)?;
        String::from_utf8(saved)
            .map(Some)
            .map_err(|error| format!("what it saved is not UTF-8 text: {}", error.utf8_error()))
    }
}

/// Runs one `inquire` prompt. Esc and Ctrl-C arrive as keys, as [`CliError::Cancelled`].
pub fn prompt<T>(run: impl FnOnce() -> Result<T, inquire::InquireError>) -> Result<T, CliError> {
    run().map_err(CliError::from)
}

#[cfg(unix)]
mod editor_owns_ctrl_c {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use signal_hook::consts::SIGINT;

    /// Ctrl-C in a line-mode editor reaches this process too: as Git's `launch_editor` does, ignore SIGINT while the
    /// editor runs and let its exit status decide. Dropping this gives SIGINT its default action back.
    pub struct Guard(Arc<AtomicBool>);

    pub fn enter() -> std::io::Result<Guard> {
        let default_action = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register_conditional_default(SIGINT, Arc::clone(&default_action))?;
        Ok(Guard(default_action))
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
}

#[cfg(not(unix))]
mod editor_owns_ctrl_c {
    pub struct Guard;

    pub fn enter() -> std::io::Result<Guard> {
        Ok(Guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(count: usize) -> Vec<String> {
        (0..count).map(|n| format!("package-{n}")).collect()
    }

    #[test]
    fn the_page_leaves_room_for_the_prompt_help_and_one_spare_row() {
        // 10 rows: prompt, help and the spare row leave 7.
        let layout = fit_list_to(Some((120, 10)), ListKind::MultiSelect, "Pick", 15, &labels(20));
        assert_eq!(layout.page_size, 7);
        // A tall terminal keeps the wanted page.
        let layout = fit_list_to(Some((120, 60)), ListKind::MultiSelect, "Pick", 15, &labels(20));
        assert_eq!(layout.page_size, 15);
    }

    #[test]
    fn a_prompt_and_help_that_wrap_take_more_rows() {
        let prompt = "p".repeat(100);
        let wide = fit_list_to(Some((200, 12)), ListKind::Select, &prompt, 15, &labels(20));
        let narrow = fit_list_to(Some((50, 12)), ListKind::Select, &prompt, 15, &labels(20));
        assert!(
            narrow.page_size < wide.page_size,
            "{} vs {}",
            narrow.page_size,
            wide.page_size
        );
    }

    #[test]
    fn the_page_is_never_below_one() {
        for rows in [0, 1, 2, 3] {
            let layout = fit_list_to(Some((80, rows)), ListKind::Select, "Pick", 7, &labels(3));
            assert_eq!(layout.page_size, 1, "{rows} rows");
        }
    }

    #[test]
    fn labels_are_cut_to_one_row_after_the_prefix() {
        let long = vec![
            "x".repeat(100),
            "short".to_owned(),
            "wide-\u{4e2d}\u{6587}\u{4e2d}\u{6587}".to_owned(),
        ];
        let layout = fit_list_to(Some((20, 30)), ListKind::MultiSelect, "Pick", 15, &long);
        // 20 columns less the 6-column prefix.
        assert_eq!(layout.labels[0], format!("{}...", "x".repeat(11)));
        assert_eq!(layout.labels[1], "short");
        assert!(
            UnicodeWidthStr::width(layout.labels[2].as_str()) <= 14,
            "{:?}",
            layout.labels[2]
        );
        let layout = fit_list_to(Some((20, 30)), ListKind::Select, "Pick", 15, &long);
        assert_eq!(layout.labels[0], format!("{}...", "x".repeat(15)));
    }

    #[test]
    fn an_unknown_size_keeps_the_page_and_the_labels() {
        let long = vec!["x".repeat(500)];
        let layout = fit_list_to(None, ListKind::MultiSelect, "Pick", 15, &long);
        assert_eq!((layout.page_size, layout.labels), (15, long));
    }

    #[test]
    fn a_terminal_too_narrow_for_an_ellipsis_still_gets_one_row_labels() {
        let layout = fit_list_to(Some((8, 30)), ListKind::MultiSelect, "Pick", 15, &labels(1));
        assert_eq!(layout.labels, ["pa"]);
    }
}
