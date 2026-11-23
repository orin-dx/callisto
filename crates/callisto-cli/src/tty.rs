//! The one contract for interactive prompts: where they may run, how Ctrl-C ends them, and how `$EDITOR` is used.

use std::io::{ErrorKind, IsTerminal as _, Read as _, Write as _};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Once;

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
const CANCELLED_BY_SIGINT: i32 = 130;

/// What owns the terminal when Ctrl-C arrives.
const ELSEWHERE: u8 = 0;
/// A prompt: Ctrl-C cancels the run, and nothing has been written yet.
const PROMPT: u8 = 1;
/// `$EDITOR` shares our process group, so its Ctrl-C reaches us too.
const EDITOR: u8 = 2;

static AT: AtomicU8 = AtomicU8::new(ELSEWHERE);

/// Runs `ask` where prompts are allowed, or fails with [`CliError::NotATty`] naming the flags to pass instead.
///
/// Ctrl-C inside `ask`, as a key or a signal, ends the process through [`cancel`]; outside a prompt it exits 130.
pub fn prompting<T>(format: OutputFormat, ask: impl FnOnce() -> Result<T, CliError>) -> Result<T, CliError> {
    if !may_prompt(format) {
        return Err(CliError::NotATty);
    }
    terminal::save();
    // Without the handler SIGINT kills the process with the cursor hidden, so a failure to install it is not fatal.
    drop(ctrlc::set_handler(|| match AT.load(Ordering::SeqCst) {
        PROMPT => cancel(),
        EDITOR => {}
        _ => std::process::exit(CANCELLED_BY_SIGINT),
    }));
    AT.store(PROMPT, Ordering::SeqCst);
    let result = ask();
    if matches!(&result, Err(CliError::Interactive(dialoguer::Error::IO(error))) if error.kind() == ErrorKind::Interrupted)
    {
        cancel();
    }
    AT.store(ELSEWHERE, Ordering::SeqCst);
    result
}

static CLEANUP: Once = Once::new();

/// Restores the terminal and cursor, says nothing was written and exits 130. The signal handler and the prompt may
/// both get here; the cleanup runs once and the later caller waits for it to finish before exiting.
fn cancel() -> ! {
    CLEANUP.call_once(|| {
        terminal::restore();
        drop(dialoguer::console::Term::stderr().show_cursor());
        prompt_line(CANCELLED);
    });
    std::process::exit(CANCELLED_BY_SIGINT)
}

/// The terminal settings from before the first prompt; a prompt killed mid-read leaves the terminal raw.
#[cfg(unix)]
mod terminal {
    use std::sync::OnceLock;

    use rustix::termios::{tcgetattr, tcsetattr, OptionalActions, Termios};

    static SAVED: OnceLock<Termios> = OnceLock::new();

    pub fn save() {
        if let Ok(settings) = tcgetattr(std::io::stdin()) {
            drop(SAVED.set(settings));
        }
    }

    pub fn restore() {
        if let Some(settings) = SAVED.get() {
            // Best effort: the process is exiting and has nowhere to report a failure.
            tcsetattr(std::io::stdin(), OptionalActions::Now, settings).ok();
        }
    }
}

#[cfg(not(unix))]
mod terminal {
    pub fn save() {}
    pub fn restore() {}
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
/// `:cq` in Vim). The editor draws on stderr so stdout stays for the report.
pub fn edit(command: &str) -> Result<Option<String>, CliError> {
    let mut file = tempfile::Builder::new().prefix("callisto-").suffix(".txt").tempfile()?;
    file.flush()?;
    let mut words = shell_words::split(command).unwrap_or_else(|_| vec![command.to_owned()]);
    if words.is_empty() {
        return Ok(None);
    }
    let program = words.remove(0);
    let before = AT.swap(EDITOR, Ordering::SeqCst);
    let status = crate::runner::child_command(&program, &[], &[])
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
    Ok(Some(written))
}
