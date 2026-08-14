use std::path::Path;
use std::time::Duration;

pub const REQUIRED_GIT: &str = ">=2.20";

/// Trait for executing subprocess commands.
pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError>;

    /// Run a command with a hard wall-clock timeout. If the process does not
    /// exit before `timeout` elapses it is killed and
    /// [`CommandError::TimedOut`] is returned.
    ///
    /// The default implementation ignores `timeout` and delegates to [`Self::run`].
    /// Implementors that control a real subprocess should override this with an
    /// actual deadline check.
    fn run_with_timeout(
        &self,
        program: &str,
        args: &[&str],
        cwd: &Path,
        _timeout: Duration,
    ) -> Result<CommandOutput, CommandError> {
        self.run(program, args, cwd)
    }

    /// Like [`Self::run_with_timeout`], but for an internal existence/probe
    /// check whose stderr is expected to look like a failure on the common
    /// path (e.g. `npm view` against an unpublished package prints a
    /// 404-shaped error to stderr as its normal "not found" signal) --
    /// unlike a real, user-facing mutating command, this must not stream
    /// that noise live to the terminal, only capture it into the returned
    /// [`CommandOutput`] for the caller to classify.
    ///
    /// The default implementation just delegates to [`Self::run_with_timeout`]
    /// (streams live, same as any other command); implementors backed by a
    /// real subprocess and doing their own live-streaming should override
    /// this to suppress it for probe calls specifically.
    fn run_quiet(
        &self,
        program: &str,
        args: &[&str],
        cwd: &Path,
        timeout: Duration,
    ) -> Result<CommandOutput, CommandError> {
        self.run_with_timeout(program, args, cwd, timeout)
    }
}

/// Output from executing a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }

    pub fn stdout_trimmed(&self) -> &str {
        self.stdout.trim()
    }

    pub fn stdout_lines(&self) -> impl Iterator<Item = &str> {
        self.stdout
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
    }
}

/// Errors occurring during command execution.
#[derive(Clone, Debug, thiserror::Error, miette::Diagnostic, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandError {
    #[error("`{program}` was not found; callisto requires it to be available")]
    #[diagnostic(
        code(E020),
        help("Ensure program is installed and available on system PATH.")
    )]
    NotFound { program: String },

    #[error("`{program}` reports version `{found}`, but callisto requires {required}")]
    #[diagnostic(code(E021), help("Upgrade program to meet version requirement."))]
    IncompatibleVersion {
        program: String,
        found: String,
        required: String,
    },

    #[error("executing `{program}` is not supported on this surface: {reason}")]
    #[diagnostic(code(E022))]
    Unsupported { program: String, reason: String },

    #[error("`{program}` failed with exit code {exit_code:?}: {stderr}")]
    #[diagnostic(code(E023))]
    Failed {
        program: String,
        exit_code: Option<i32>,
        stderr: String,
    },

    #[error("failed to run `{program}`: {message}")]
    #[diagnostic(code(E024))]
    Io { program: String, message: String },

    #[error("`{program}` timed out after {seconds}s")]
    #[diagnostic(code(E025), help("The process did not exit within the allowed time. This usually indicates a network stall or a registry that is unreachable."))]
    TimedOut { program: String, seconds: u64 },
}

/// Validates git version against REQUIRED_GIT floor.
pub fn check_git_version(reported: &str) -> Result<(), CommandError> {
    let version_str = reported.trim();
    let digits = version_str
        .split_whitespace()
        .find(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or(version_str);

    let parts: Vec<&str> = digits.split('.').collect();
    if parts.len() < 2 {
        return Err(CommandError::IncompatibleVersion {
            program: "git".to_string(),
            found: reported.to_string(),
            required: REQUIRED_GIT.to_string(),
        });
    }

    let major: u64 = parts[0].parse().unwrap_or(0);
    let minor: u64 = parts[1].parse().unwrap_or(0);

    if major < 2 || (major == 2 && minor < 20) {
        return Err(CommandError::IncompatibleVersion {
            program: "git".to_string(),
            found: reported.to_string(),
            required: REQUIRED_GIT.to_string(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_git_version() {
        assert!(check_git_version("git version 2.39.5").is_ok());
        assert!(check_git_version("git version 2.20.0").is_ok());
        assert!(check_git_version("git version 1.8.5").is_err());
    }
}
