//! The one contract for interactive prompts: where they may run, how they end, and how `$EDITOR` is used.

use std::io::{IsTerminal as _, Read as _, Write as _};
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::sync::Arc;

use crate::cli::OutputFormat;
use crate::error::CliError;

/// Proof that this run may prompt; only [`Prompts::allowed`] makes one.
pub struct Prompts(());

impl Prompts {
    /// Prompts need a person at the terminal (stdin, and stderr where they draw) and a text run: `--format json` is for
    /// scripts and never prompts. Fails with [`CliError::NotATty`], naming the flags to pass instead.
    pub fn allowed(format: OutputFormat) -> Result<Self, CliError> {
        if format == OutputFormat::Text && std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
            Ok(Self(()))
        } else {
            Err(CliError::NotATty)
        }
    }
}

/// Printed whenever a prompt run ends without writing.
pub const CANCELLED: &str = "Cancelled, nothing written.";

/// Exit status for Ctrl-C, as shells report SIGINT.
pub const CANCELLED_BY_CTRL_C: u8 = 130;

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
    let _editor_owns_ctrl_c = EditorOwnsCtrlC::take()?;
    let status = crate::runner::child_command(&program, &[], &[])
        .args(words)
        .arg(file.path())
        .stdout(std::io::stderr())
        .status()?;
    if !status.success() {
        return Ok(None);
    }
    let mut written = String::new();
    std::fs::File::open(file.path())?.read_to_string(&mut written)?;
    Ok(Some(written))
}

/// Ctrl-C in a line-mode editor reaches this process too: as Git's `launch_editor` does, SIGINT is ignored while the
/// editor runs and its exit status decides. Dropping this gives SIGINT its default action back.
#[cfg(unix)]
struct EditorOwnsCtrlC(Arc<AtomicBool>);

#[cfg(unix)]
impl EditorOwnsCtrlC {
    fn take() -> std::io::Result<Self> {
        let default_action = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register_conditional_default(signal_hook::consts::SIGINT, Arc::clone(&default_action))?;
        Ok(Self(default_action))
    }
}

#[cfg(unix)]
impl Drop for EditorOwnsCtrlC {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[cfg(not(unix))]
struct EditorOwnsCtrlC;

#[cfg(not(unix))]
impl EditorOwnsCtrlC {
    fn take() -> std::io::Result<Self> {
        Ok(Self)
    }
}
