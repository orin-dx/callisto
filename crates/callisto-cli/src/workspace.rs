use std::path::Path;

use callisto_graph::infer::SeverityInference;
use callisto_graph::locate::{find_workspace_root, IgnoreWalkLocator};
use callisto_graph::resolver::ManifestWalkResolver;
use callisto_graph::Workspace;
use callisto_model::{check_git_version, CommandRunner};

use crate::cli::GlobalArgs;
use crate::error::CliError;
use crate::runner::CliCommandRunner;

/// Rejects an unsupported `git` version before any git-dependent operation runs.
fn ensure_git_supported(runner: &dyn CommandRunner, cwd: &Path) -> Result<(), CliError> {
    let output = runner.run("git", &["--version"], cwd)?;
    check_git_version(&output.stdout)?;
    Ok(())
}

pub fn load_workspace<'a>(
    global: &GlobalArgs,
    runner: &'a CliCommandRunner,
) -> Result<Workspace<'a, CliCommandRunner, ManifestWalkResolver>, CliError> {
    let start = dunce::canonicalize(&global.cwd).map_err(|source| CliError::Io {
        source,
        path: Some(global.cwd.clone()),
    })?;
    ensure_git_supported(runner, &start)?;
    let root = find_workspace_root(&start)?;
    let locator = IgnoreWalkLocator::new(&root);
    Ok(Workspace::load(root, &locator, runner)?)
}

/// Selects the concrete `SeverityInference` impl at compile time, milestone-gated by the
/// `inference` Cargo feature (§17, §G.14): `NoInference` when the feature is off,
/// `CommitInference` (a real git-backed adapter, §G.6.4) when it's on. `CommitInference`
/// needs no workspace/runner context of its own -- `SeverityInference::infer` receives the
/// caller's `GitAccess` per call -- so this needs no parameters and no lifetime.
#[cfg(not(feature = "inference"))]
pub fn select_inference() -> impl SeverityInference {
    callisto_graph::infer::NoInference
}

#[cfg(feature = "inference")]
pub fn select_inference() -> impl SeverityInference {
    callisto_graph::infer::CommitInference
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use callisto_model::{CommandError, CommandOutput, CommandRunner};

    use super::*;

    #[test]
    fn load_workspace_reports_io_error_for_a_nonexistent_cwd() {
        let global = GlobalArgs {
            format: crate::cli::OutputFormat::Text,
            cwd: std::path::PathBuf::from("/nonexistent/definitely-not-a-real-path-abc123"),
            dry_run: false,
        };
        let runner = CliCommandRunner;

        match load_workspace(&global, &runner) {
            Err(CliError::Io { path, .. }) => {
                assert_eq!(path, Some(global.cwd.clone()));
            }
            Err(other) => panic!("expected CliError::Io, got a different CliError: {other:?}"),
            Ok(_) => panic!("expected an error for a nonexistent cwd, got Ok"),
        }
    }

    struct FixedVersionRunner(&'static str);

    impl CommandRunner for FixedVersionRunner {
        fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: self.0.to_string(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn ensure_git_supported_rejects_a_git_version_below_the_floor() {
        let runner = FixedVersionRunner("git version 1.8.5");
        match ensure_git_supported(&runner, Path::new(".")) {
            Err(CliError::Command(CommandError::IncompatibleVersion { program, .. })) => {
                assert_eq!(program, "git");
            }
            other => panic!("expected CliError::Command(IncompatibleVersion), got: {other:?}"),
        }
    }

    #[test]
    fn ensure_git_supported_accepts_a_git_version_at_the_floor() {
        let runner = FixedVersionRunner("git version 2.20.0");
        assert!(ensure_git_supported(&runner, Path::new(".")).is_ok());
    }
}
