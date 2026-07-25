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
    let start = global.cwd.canonicalize().map_err(CliError::Io)?;
    let root = find_workspace_root(&start)?;
    let locator = IgnoreWalkLocator::new(&root);
    Ok(Workspace::load(root, &locator, runner)?)
}
