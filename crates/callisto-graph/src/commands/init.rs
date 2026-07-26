use callisto_model::{CommandRunner, InitReport, SCHEMA_VERSION};

use crate::error::GraphError;
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct InitOptions {
    pub yes: bool,
}

pub fn init<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    _opts: &InitOptions,
) -> Result<InitReport, GraphError> {
    let config_path = ws.root.join("callisto.toml");
    let written = if !config_path.exists() {
        let content = r#"# callisto configuration

[changesets]
dir = ".changeset"

[cascade]
mode = "out-of-range"
bump-severity = "patch"
peer-escalation = true
preserve-npm-ranges = true
"#;
        callisto_manifests::atomic::atomic_write(&config_path, content).is_ok()
    } else {
        false
    };

    Ok(InitReport {
        schema_version: SCHEMA_VERSION,
        initialized: written,
        config_path,
        diagnostics: Vec::new(),
    })
}
