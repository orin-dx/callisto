use callisto_model::{CommandRunner, ValidateReport, SCHEMA_VERSION};

use crate::commands::escalate;
use crate::error::GraphError;
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct ValidateOptions {
    pub staged: bool,
    pub since: Option<String>,
    pub strict: bool,
    pub strict_graph: bool,
}

pub fn validate<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    opts: &ValidateOptions,
) -> Result<ValidateReport, GraphError> {
    let mut diagnostics = Vec::new();
    let _loaded = crate::load_changesets(&ws.root, &ws.config)?;

    escalate(&mut diagnostics, opts.strict, opts.strict_graph);

    let is_valid = diagnostics
        .iter()
        .all(|d| d.severity != callisto_model::DiagnosticSeverity::Error);
    Ok(ValidateReport {
        schema_version: SCHEMA_VERSION,
        valid: is_valid,
        diagnostics,
    })
}
