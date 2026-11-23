//! The one contract for interactive prompts: where they may run, how they end, and how `$EDITOR` is used.

use std::io::IsTerminal as _;
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
    /// as `:cq` in Vim). The editor draws on stderr so stdout stays for the report. Taking `self` means one launch, so
    /// one SIGINT registration, per editor.
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
        let _editor_owns_ctrl_c = EditorOwnsCtrlC::take().map_err(failed)?;
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
