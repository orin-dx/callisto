use callisto_graph::infer::SeverityInference;
use callisto_graph::locate::{find_workspace_root, IgnoreWalkLocator};
use callisto_graph::resolver::ManifestWalkResolver;
use callisto_graph::Workspace;

use crate::cli::GlobalArgs;
use crate::error::CliError;
use crate::runner::CliCommandRunner;

pub fn load_workspace<'a>(
    global: &GlobalArgs,
    runner: &'a CliCommandRunner,
) -> Result<Workspace<'a, CliCommandRunner, ManifestWalkResolver>, CliError> {
    let start = dunce::canonicalize(&global.cwd).map_err(|source| CliError::Io {
        source,
        path: Some(global.cwd.clone()),
    })?;
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
}
