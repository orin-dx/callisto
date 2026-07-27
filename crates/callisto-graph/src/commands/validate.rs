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
    let loaded = crate::load_changesets(&ws.root, &ws.config)?;

    let target_changesets: Vec<_> = if opts.staged {
        if let Ok(out) = ws
            .runner
            .run("git", &["diff", "--cached", "--name-only"], &ws.root)
        {
            let files: Vec<&str> = out.stdout_trimmed().lines().collect();
            loaded
                .into_iter()
                .filter(|cs| files.iter().any(|f| cs.path.ends_with(f)))
                .collect()
        } else {
            loaded
        }
    } else if let Some(ref since) = opts.since {
        let range = format!("{since}..HEAD");
        if let Ok(out) = ws
            .runner
            .run("git", &["diff", "--name-only", &range], &ws.root)
        {
            let files: Vec<&str> = out.stdout_trimmed().lines().collect();
            loaded
                .into_iter()
                .filter(|cs| files.iter().any(|f| cs.path.ends_with(f)))
                .collect()
        } else {
            loaded
        }
    } else {
        loaded
    };

    for cs in &target_changesets {
        if cs.changeset.entries.is_empty() {
            diagnostics.push(callisto_model::Diagnostic {
                code: callisto_model::DiagnosticCode::EmptyChangeset,
                severity: callisto_model::DiagnosticSeverity::Error,
                message: format!("Changeset `{}` is empty", cs.path.display()),
                package: None,
                path: Some(cs.path.clone()),
                governed_by: None,
                escalated_by: None,
            });
        }

        for entry in &cs.changeset.entries {
            if let Ok(id) = callisto_model::PackageId::parse(&entry.name) {
                if !ws.graph.packages().any(|p| p.id == id) {
                    diagnostics.push(callisto_model::Diagnostic {
                        code: callisto_model::DiagnosticCode::UnknownPackage,
                        severity: callisto_model::DiagnosticSeverity::Error,
                        message: format!(
                            "Changeset `{}` references unknown package `{}`",
                            cs.path.display(),
                            entry.name
                        ),
                        package: Some(id),
                        path: Some(cs.path.clone()),
                        governed_by: None,
                        escalated_by: None,
                    });
                }
            }
        }
    }

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
