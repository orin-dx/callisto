//! The one contract for interactive prompts: where they may run, how Ctrl-C ends them, and how `$EDITOR` is used.

use std::io::{IsTerminal as _, Read as _, Write as _};
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::cli::OutputFormat;
use crate::error::CliError;
use crate::output::prompt_line;

/// Whether stdin is a terminal; `init` still asks only this.
pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal()
}

/// Prompts need a person at the terminal (stdin and stderr, where dialoguer draws) and a text run: `--format json` is
/// for scripts and never prompts.
fn may_prompt(format: OutputFormat) -> bool {
    format == OutputFormat::Text && is_interactive() && std::io::stderr().is_terminal()
}

/// Exit status for Ctrl-C, as shells report SIGINT.
pub const CANCELLED_BY_SIGINT: u8 = 130;

/// How a run of prompts ended.
pub enum Prompted<T> {
    Answered(T),
    /// Ctrl-C: the cursor is restored and the contributor has been told nothing was written.
    Interrupted,
}

/// What owns the terminal when Ctrl-C arrives.
const ELSEWHERE: u8 = 0;
/// dialoguer raises SIGINT for Ctrl-C and then returns `Interrupted`; the handler leaves both to the prompt.
const PROMPT: u8 = 1;
/// `$EDITOR` shares our process group, so its Ctrl-C reaches us too.
const EDITOR: u8 = 2;

static AT: AtomicU8 = AtomicU8::new(ELSEWHERE);

/// Runs `ask` where prompts are allowed, or fails with [`CliError::NotATty`] naming the flags to pass instead.
///
/// Ctrl-C inside `ask` ends as [`Prompted::Interrupted`], decided from the error `ask` returns and not from a signal
/// handler's timing. The handler only keeps SIGINT from killing the process before that error is returned; outside a
/// prompt it still exits 130.
pub fn prompting<T>(format: OutputFormat, ask: impl FnOnce() -> Result<T, CliError>) -> Result<Prompted<T>, CliError> {
    if !may_prompt(format) {
        return Err(CliError::NotATty);
    }
    // Without the handler SIGINT kills the process with the cursor hidden, so a failure to install it is not fatal.
    drop(ctrlc::set_handler(|| match AT.load(Ordering::SeqCst) {
        PROMPT | EDITOR => {}
        _ => std::process::exit(i32::from(CANCELLED_BY_SIGINT)),
    }));
    AT.store(PROMPT, Ordering::SeqCst);
    match ask() {
        Err(CliError::Interactive(dialoguer::Error::IO(error))) if error.kind() == std::io::ErrorKind::Interrupted => {
            // Stay in PROMPT: a handler thread running late must not exit before the caller reports the cancel.
            drop(dialoguer::console::Term::stderr().show_cursor());
            prompt_line(CANCELLED);
            Ok(Prompted::Interrupted)
        }
        result => {
            AT.store(ELSEWHERE, Ordering::SeqCst);
            result.map(Prompted::Answered)
        }
    }
}

/// Printed whenever a prompt run ends without writing.
pub const CANCELLED: &str = "Cancelled, nothing written.";

/// The editor from `$VISUAL`, else `$EDITOR`; an empty or blank setting is unset.
pub fn editor_command() -> Option<String> {
    ["VISUAL", "EDITOR"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|command| !command.trim().is_empty())
}

/// Opens `command` on an empty file and returns what was saved. `None` when the editor exits non-zero (an abort, as
/// `:cq` in Vim) or saves nothing. The editor draws on stderr so stdout stays for the report.
pub fn edit(command: &str) -> Result<Option<String>, CliError> {
    let mut file = tempfile::Builder::new().prefix("callisto-").suffix(".txt").tempfile()?;
    file.flush()?;
    let mut words = shell_words::split(command).unwrap_or_else(|_| vec![command.to_owned()]);
    if words.is_empty() {
        return Ok(None);
    }
    let program = words.remove(0);
    let before = AT.swap(EDITOR, Ordering::SeqCst);
    let status = Command::new(program)
        .args(words)
        .arg(file.path())
        .stdout(std::io::stderr())
        .status();
    AT.store(before, Ordering::SeqCst);
    if !status?.success() {
        return Ok(None);
    }
    let mut written = String::new();
    std::fs::File::open(file.path())?.read_to_string(&mut written)?;
    Ok(Some(written).filter(|text| !text.trim().is_empty()))
}
