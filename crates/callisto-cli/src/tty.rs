//! The one contract for interactive prompts: where they may run, how they end, and how `$EDITOR` is used.

use std::io::IsTerminal as _;

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

/// Printed whenever a prompt run ends without writing.
pub const CANCELLED: &str = "Cancelled, nothing written.";

/// Exit status for a prompt cancelled with Esc or Ctrl-C: 130, as shells report SIGINT.
pub const CANCELLED_EXIT: u8 = 130;

/// Asks which of `names` to pick, as `git add -i` does: the list is printed once, numbered, with `marked` ones starred,
/// then one typed line answers. Nothing is redrawn, so no terminal size or resize can misdraw it. Enter keeps the marked
/// ones; an answer that names nothing listed is asked again with the reason.
pub(crate) fn pick_many(heading: &str, names: &[String], marked: &[bool]) -> Result<Vec<usize>, CliError> {
    print_list(heading, names, marked);
    let enter = if marked.contains(&true) {
        "Enter keeps the * ones"
    } else {
        "Enter picks none"
    };
    eprintln!("{enter}, or type numbers, ranges (1-3) or names; Esc cancels.");
    loop {
        let answer = read_answer()?;
        if answer.trim().is_empty() {
            return Ok((0..names.len())
                .filter(|&index| marked.get(index) == Some(&true))
                .collect());
        }
        match choices(&answer, names) {
            Ok(picked) => return Ok(picked),
            Err(reason) => eprintln!("{reason}"),
        }
    }
}

/// Asks for exactly one of `names`, the way [`pick_many`] asks for several.
pub(crate) fn pick_one(heading: &str, names: &[String]) -> Result<usize, CliError> {
    print_list(heading, names, &[]);
    eprintln!("Type a number or a name; Esc cancels.");
    loop {
        match choices(&read_answer()?, names).as_deref() {
            Ok(&[one]) => return Ok(one),
            Ok(_) => eprintln!("pick exactly one"),
            Err(reason) => eprintln!("{reason}"),
        }
    }
}

fn print_list(heading: &str, names: &[String], marked: &[bool]) {
    let width = names.len().to_string().len();
    let mut text = format!("{heading}\n");
    for (index, name) in names.iter().enumerate() {
        let mark = if marked.get(index) == Some(&true) { '*' } else { ' ' };
        // A control character in a manifest's name must not drive the terminal.
        text.push_str(&format!("  {:>width$} {mark} {}\n", index + 1, name.escape_debug()));
    }
    eprint!("{text}");
}

/// One typed line; Esc, Ctrl-C or end of input cancels. A key that sends an escape sequence (an arrow) asks again.
fn read_answer() -> Result<String, CliError> {
    loop {
        eprint!("> ");
        match line::read()? {
            Typed::Line(answer) => return Ok(answer),
            Typed::Cancel => {
                eprintln!();
                return Err(CliError::Cancelled);
            }
            #[cfg(unix)]
            Typed::Key => eprintln!("\nonly typed text is read here; type the answer again"),
        }
    }
}

/// The listed indices an answer names, in list order. A word is a number or range counted from 1 (`3`, `1-3`) or a
/// listed name; words are separated by spaces or commas.
fn choices(answer: &str, names: &[String]) -> Result<Vec<usize>, String> {
    let mut picked = std::collections::BTreeSet::new();
    for word in answer
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|word| !word.is_empty())
    {
        let shown = word.escape_debug();
        let by_name = names.iter().position(|name| name == word);
        let indices = match (by_name, numbered(word, names.len())) {
            (Some(index), None) => index..=index,
            (None, Some(range)) => range?,
            (Some(index), Some(Ok(range))) if range == (index..=index) => range,
            (Some(_), Some(_)) => return Err(format!("`{shown}` is both a listed name and another entry's number")),
            (None, None) => return Err(format!("`{shown}` is not a number, a range or a listed name")),
        };
        picked.extend(indices);
    }
    Ok(picked.into_iter().collect())
}

/// `n` or `a-b`, counted from 1, as indices; `None` when the word is shaped like neither.
fn numbered(word: &str, len: usize) -> Option<Result<std::ops::RangeInclusive<usize>, String>> {
    let (first, last) = word.split_once('-').unwrap_or((word, word));
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(first) || !digits(last) {
        return None;
    }
    let listed = |text: &str| text.parse::<usize>().ok().filter(|number| (1..=len).contains(number));
    Some(match (listed(first), listed(last)) {
        (Some(low), Some(high)) if low <= high => Ok(low - 1..=high - 1),
        (Some(_), Some(_)) => Err(format!("`{word}` runs backwards; put the lower number first")),
        _ => Err(format!("`{word}` is not in the list (1-{len})")),
    })
}

enum Typed {
    Line(String),
    Cancel,
    /// A key that sends an escape sequence, such as an arrow.
    #[cfg(unix)]
    Key,
}

#[cfg(unix)]
mod line {
    use std::os::fd::{AsFd as _, BorrowedFd};

    use rustix::termios::{self, LocalModes, OptionalActions, QueueSelector, SpecialCodeIndex, Termios};

    use super::Typed;

    const ESC: u8 = 0x1b;
    const CTRL_C: u8 = 0x03;

    /// Gives the terminal back the settings it had before the line was read.
    struct Restore<'a>(BorrowedFd<'a>, Termios);

    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            // A terminal that refuses its own earlier settings leaves nothing better to do.
            termios::tcsetattr(self.0, OptionalActions::Now, &self.1).ok();
        }
    }

    /// Reads one line in the terminal's line mode, so the terminal does the editing and echo. Esc and Ctrl-C end the
    /// line instead of being typed, and Ctrl-C raises no signal, so either can cancel cleanly.
    pub fn read() -> std::io::Result<Typed> {
        let stdin = std::io::stdin();
        let fd = stdin.as_fd();
        let saved = termios::tcgetattr(fd)?;
        let _restore = Restore(fd, saved.clone());
        let mut mode = saved;
        mode.local_modes -= LocalModes::ISIG;
        mode.special_codes[SpecialCodeIndex::VEOL] = ESC;
        mode.special_codes[SpecialCodeIndex::VEOL2] = CTRL_C;
        termios::tcsetattr(fd, OptionalActions::Now, &mode)?;
        let mut line = Vec::new();
        loop {
            let mut buf = [0; 256];
            let read = read_some(fd, &mut buf)?;
            if read == 0 {
                return Ok(Typed::Cancel);
            }
            line.extend_from_slice(&buf[..read]);
            match line.last() {
                Some(b'\n') => {
                    line.pop();
                    return Ok(Typed::Line(String::from_utf8_lossy(&line).into_owned()));
                }
                Some(&CTRL_C) => return Ok(Typed::Cancel),
                Some(&ESC) => {
                    return Ok(if key_follows(fd, mode)? {
                        Typed::Key
                    } else {
                        Typed::Cancel
                    })
                }
                _ => {}
            }
        }
    }

    /// Whether more bytes follow an Esc within 100 ms, as the rest of an arrow's escape sequence does; they are
    /// discarded.
    fn key_follows(fd: BorrowedFd<'_>, mut mode: Termios) -> std::io::Result<bool> {
        mode.local_modes -= LocalModes::ICANON | LocalModes::ECHO;
        mode.special_codes[SpecialCodeIndex::VMIN] = 0;
        mode.special_codes[SpecialCodeIndex::VTIME] = 1;
        termios::tcsetattr(fd, OptionalActions::Now, &mode)?;
        let mut buf = [0; 64];
        let followed = read_some(fd, &mut buf)? > 0;
        termios::tcflush(fd, QueueSelector::IFlush)?;
        Ok(followed)
    }

    fn read_some(fd: BorrowedFd<'_>, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            match rustix::io::read(fd, &mut *buf) {
                Err(rustix::io::Errno::INTR) => continue,
                result => return Ok(result?),
            }
        }
    }
}

#[cfg(not(unix))]
mod line {
    use super::Typed;

    /// Reads one line as the console gives it; the console keeps its own Esc and Ctrl-C behaviour.
    pub fn read() -> std::io::Result<Typed> {
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            return Ok(Typed::Cancel);
        }
        Ok(Typed::Line(line.trim_end_matches(['\r', '\n']).to_owned()))
    }
}

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
    use super::choices;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn an_answer_picks_by_number_range_and_name_in_list_order() {
        let listed = names(&["a", "b", "c", "d", "cargo/e"]);
        assert_eq!(choices("3, 1", &listed), Ok(vec![0, 2]));
        assert_eq!(choices("2-4", &listed), Ok(vec![1, 2, 3]));
        assert_eq!(choices("cargo/e a 1", &listed), Ok(vec![0, 4]));
        assert_eq!(choices("1-1,,  b", &listed), Ok(vec![0, 1]));
    }

    #[test]
    fn an_answer_naming_nothing_listed_says_why() {
        let listed = names(&["a", "b", "c"]);
        for (answer, reason) in [
            ("0", "`0` is not in the list (1-3)"),
            ("4", "`4` is not in the list (1-3)"),
            ("2-9", "`2-9` is not in the list (1-3)"),
            ("99999999999999999999999", "is not in the list (1-3)"),
            ("3-1", "`3-1` runs backwards"),
            ("x", "`x` is not a number, a range or a listed name"),
            ("1-", "`1-` is not a number"),
            ("-1", "`-1` is not a number"),
            ("A", "`A` is not a number"),
        ] {
            let error = choices(answer, &listed).expect_err(answer);
            assert!(error.contains(reason), "{answer}: {error}");
        }
    }

    #[test]
    fn a_name_that_reads_as_another_entrys_number_is_refused() {
        let ambiguous = "is both a listed name and another entry's number";
        assert!(choices("2", &names(&["2", "b"])).unwrap_err().contains(ambiguous));
        assert!(choices("1-2", &names(&["a", "b", "1-2"]))
            .unwrap_err()
            .contains(ambiguous));
        // A name that is its own number is not ambiguous.
        assert_eq!(choices("1", &names(&["1", "b"])), Ok(vec![0]));
    }
}
