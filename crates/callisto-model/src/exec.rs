use std::path::Path;
use std::time::Duration;

/// The minimum `git` version callisto supports, as a [`semver::VersionReq`] grammar string
/// consumed directly by [`check_git_version`] — the single place this floor is defined, so
/// the requirement used for the actual comparison and the one rendered in
/// [`CommandError::IncompatibleVersion`]'s message can never drift apart.
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
    /// check whose stderr looks like a failure on the common path (e.g.
    /// `npm view` against an unpublished package prints a 404-shaped "not
    /// found" to stderr normally) -- unlike a real, user-facing mutating
    /// command, this must not stream that noise live to the terminal, only
    /// capture it into [`CommandOutput`] for the caller to classify.
    ///
    /// Default implementation just delegates to [`Self::run_with_timeout`]
    /// (streams live); implementors doing their own live-streaming should
    /// override this to suppress it for probe calls.
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
        self.stdout.lines().map(|l| l.trim()).filter(|l| !l.is_empty())
    }
}

/// Errors occurring during command execution.
#[derive(Clone, Debug, thiserror::Error, miette::Diagnostic, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandError {
    #[error("`{program}` was not found; callisto requires it to be available")]
    #[diagnostic(code(E020), help("Ensure program is installed and available on system PATH."))]
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

/// Returns the leading run of ASCII digits in `s`, stopping at the first
/// non-digit character (or the end of the string). Used to strip anything
/// `semver` would otherwise interpret as pre-release/build metadata (a
/// hyphen or plus) or a non-numeric trailing suffix, since a git version's
/// own reported string was never intended to carry semver's meaning for
/// those -- see [`check_git_version`]'s doc comment.
fn leading_digits(s: &str) -> &str {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    &s[..end]
}

/// Validates git version against the [`REQUIRED_GIT`] floor.
///
/// `reported` is raw `git --version` output (e.g. `"git version 2.39.5"`,
/// `"...2.39.5.windows.1"` on Windows, `"...2.39.5-rc1"` for a pre-release).
/// Only the leading `major.minor.patch` triple is parsed, each component
/// truncated at its first non-digit char (`5-rc1` -> `5`) before reaching
/// `semver`: an unstripped hyphenated suffix would make `semver::VersionReq`
/// exclude it from the `>=2.20` floor entirely, since semver reqs never
/// match a pre-release unless the requirement itself carries a matching
/// tag -- irrelevant to a real git build tag. Extra dot-separated
/// components (`.windows.1`) are ignored; a missing minor/patch defaults
/// to `0`.
///
/// Comparison itself is delegated to [`semver::VersionReq`] parsing
/// [`REQUIRED_GIT`], so the floor lives in one place, not a hand-maintained
/// duplicate that could drift from the error message's constant.
pub fn check_git_version(reported: &str) -> Result<(), CommandError> {
    let incompatible = || CommandError::IncompatibleVersion {
        program: "git".to_string(),
        found: reported.to_string(),
        required: REQUIRED_GIT.to_string(),
    };

    let version_str = reported.trim();
    let digits = version_str
        .split_whitespace()
        .find(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or(version_str);

    let mut parts = digits.splitn(4, '.').take(3);
    let major = leading_digits(parts.next().unwrap_or("0"));
    let minor = leading_digits(parts.next().unwrap_or("0"));
    let patch = leading_digits(parts.next().unwrap_or("0"));
    let core = format!("{major}.{minor}.{patch}");

    let version = semver::Version::parse(&core).map_err(|_parse_err| incompatible())?;
    let required = semver::VersionReq::parse(REQUIRED_GIT).map_err(|_parse_err| incompatible())?;

    if required.matches(&version) {
        Ok(())
    } else {
        Err(incompatible())
    }
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

    /// Boundary check: REQUIRED_GIT's floor is exactly 2.20 -- one patch
    /// below and one minor below must both be rejected, matching the
    /// version-requirement string itself rather than a separately
    /// hand-maintained numeric comparison that could silently drift from it.
    #[test]
    fn rejects_versions_just_below_the_required_floor() {
        assert!(check_git_version("git version 2.19.9").is_err());
        assert!(check_git_version("git version 1.99.99").is_err());
    }

    /// Regression: `semver::VersionReq`'s `>=2.20` never matches a
    /// pre-release version unless the requirement itself carries a matching
    /// pre-release tag, which would make a hyphenated git build tag like
    /// `-rc1` falsely reject an otherwise-satisfying version if passed
    /// through to `semver::Version::parse` unstripped. A pre-release build
    /// well above the floor must still be accepted.
    #[test]
    fn accepts_hyphenated_prerelease_suffix_above_the_floor() {
        assert!(check_git_version("git version 2.39.5-rc1").is_ok());
        assert!(check_git_version("git version 2.19.9-rc1").is_err());
    }

    /// Git for Windows appends a non-semver `.windows.N` suffix onto the
    /// real version (e.g. `2.39.5.windows.1`). Only the leading
    /// major.minor.patch triple must be parsed; the suffix must not cause a
    /// spurious parse failure.
    #[test]
    fn ignores_trailing_windows_suffix() {
        assert!(check_git_version("git version 2.39.5.windows.1").is_ok());
        assert!(check_git_version("git version 2.19.9.windows.1").is_err());
    }

    /// A truncated report (major.minor with no patch) must not be rejected
    /// outright -- the missing patch is treated as 0, same as the original
    /// hand-rolled comparison's behavior.
    #[test]
    fn treats_missing_patch_as_zero() {
        assert!(check_git_version("git version 2.20").is_ok());
        assert!(check_git_version("git version 2.19").is_err());
    }

    /// Garbage input must be rejected as incompatible, not silently
    /// defaulted to 0 and evaluated as if it were a real (if very old)
    /// version -- there is a real difference between "we don't know what
    /// version this is" and "this version is definitely too old", even
    /// though both currently produce the same IncompatibleVersion error.
    #[test]
    fn rejects_unparseable_version_string() {
        assert!(check_git_version("git version unknown").is_err());
        assert!(check_git_version("").is_err());
    }
}
