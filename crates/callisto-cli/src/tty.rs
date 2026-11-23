//! The one contract for interactive prompts: where they may run, how they end, and how `$EDITOR` is used.

use std::io::IsTerminal as _;

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
        let _editor_owns_ctrl_c = sigint::enter(sigint::Mode::Ignore).map_err(failed)?;
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

/// Runs one `inquire` prompt. A `kill -INT` from outside reaches this process while the prompt holds the terminal
/// raw, so it puts the terminal back and says nothing was written before ending the process as SIGINT does (unix only).
/// Esc and Ctrl-C at the prompt arrive as keys, as [`CliError::Cancelled`].
pub fn prompt<T>(run: impl FnOnce() -> Result<T, inquire::InquireError>) -> Result<T, CliError> {
    // Restoring the terminal on SIGINT is a convenience: a prompt still runs when the handler cannot be installed.
    let _restores = sigint::enter(sigint::Mode::RestoreTerminal).ok();
    run().map_err(CliError::from)
}

#[cfg(unix)]
mod sigint {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    use signal_hook::consts::SIGINT;
    use signal_hook::iterator::Signals;

    /// What a SIGINT does while it is held; without a [`Guard`] it ends the process, as by default.
    #[derive(Clone, Copy)]
    pub enum Mode {
        /// A prompt holds the terminal raw: put it back and say nothing was written, then end the process.
        RestoreTerminal,
        /// Ctrl-C in a line-mode editor reaches this process too: as Git's `launch_editor` does, ignore it while the
        /// editor runs and let its exit status decide.
        Ignore,
    }

    /// One handler thread serves the whole process: once SIGINT has a handler it has no default action, so the thread
    /// takes that action itself whenever no [`Guard`] says otherwise.
    struct State {
        installed: bool,
        mode: Option<Mode>,
    }

    static STATE: Mutex<State> = Mutex::new(State {
        installed: false,
        mode: None,
    });

    fn state() -> MutexGuard<'static, State> {
        STATE.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Dropping this gives SIGINT its default action back.
    pub struct Guard(());

    pub fn enter(mode: Mode) -> std::io::Result<Guard> {
        let mut state = state();
        if !state.installed {
            let mut signals = Signals::new([SIGINT])?;
            std::thread::Builder::new()
                .name("callisto-sigint".to_owned())
                .spawn(move || signals.forever().for_each(|_| react()))?;
            state.installed = true;
        }
        state.mode = Some(mode);
        Ok(Guard(()))
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            state().mode = None;
        }
    }

    fn react() {
        let mode = state().mode;
        match mode {
            Some(Mode::Ignore) => return,
            Some(Mode::RestoreTerminal) => {
                // Best effort: the process is about to end, so there is nowhere to report a failure.
                drop(crossterm::terminal::disable_raw_mode());
                drop(crossterm::execute!(std::io::stderr(), crossterm::cursor::Show));
                eprintln!("\n{}", super::CANCELLED);
            }
            None => {}
        }
        // Fails only when the default action cannot be taken; the process then carries on as if SIGINT were ignored.
        drop(signal_hook::low_level::emulate_default_handler(SIGINT));
    }
}

#[cfg(not(unix))]
mod sigint {
    #[derive(Clone, Copy)]
    pub enum Mode {
        RestoreTerminal,
        Ignore,
    }

    pub struct Guard;

    pub fn enter(_: Mode) -> std::io::Result<Guard> {
        Ok(Guard)
    }
}
