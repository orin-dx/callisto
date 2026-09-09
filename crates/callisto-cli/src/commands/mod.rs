pub mod add;
pub mod completions;
pub mod compose_pr_body;
pub mod filter_plan;
pub mod init;
pub mod matrix;
pub mod plan_publish;
pub mod pre;
pub mod publish;
pub mod release;
pub mod release_pr;
pub mod schema;
pub mod snapshot;
pub mod status;
pub mod tag;
pub mod validate;
pub mod version;

/// Reads `arg` as a JSON document: a literal `-` reads from stdin, a value
/// starting with `{` is treated as inline JSON, anything else is read as a
/// file path. Shared by every command that accepts a report/plan as either
/// a file, inline JSON, or piped stdin (`tag --plan`, `filter-plan --plan`,
/// `filter-plan --report`).
pub(crate) fn read_json_arg(arg: &str) -> Result<String, crate::error::CliError> {
    if arg == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
        Ok(buf)
    } else if arg.trim_start().starts_with('{') {
        Ok(arg.to_string())
    } else {
        std::fs::read_to_string(arg).map_err(|source| crate::error::CliError::Io {
            source,
            path: Some(std::path::PathBuf::from(arg)),
        })
    }
}

/// Escalates workspace graph diagnostics per `strict`/`strict_graph` (mirroring
/// `callisto_graph::commands::escalate`'s own semantics: `strict` escalates
/// both `StrictFlag::Strict` and `StrictFlag::StrictGraph` diagnostics,
/// `strict_graph` alone escalates only the latter) and errors out naming every
/// diagnostic that is `Error` severity afterward.
///
/// Shared by `snapshot` and `tag`, which must abort *before* touching any
/// files/tags on a crosscheck failure -- unlike `status`/`validate`/`version`,
/// which fold escalated diagnostics into their report and gate on exit code
/// instead of an `Err`.
pub(crate) fn abort_on_crosscheck_failures(
    diagnostics: &[callisto_model::Diagnostic],
    strict: bool,
    strict_graph: bool,
) -> Result<(), crate::error::CliError> {
    let mut diags = diagnostics.to_vec();
    callisto_graph::commands::escalate(&mut diags, strict, strict_graph);

    let messages: Vec<String> = diags
        .iter()
        .filter(|d| d.severity == callisto_model::DiagnosticSeverity::Error)
        .map(|d| d.message.clone())
        .collect();

    if messages.is_empty() {
        Ok(())
    } else {
        Err(crate::error::CliError::Other(format!(
            "--strict/--strict-graph: workspace graph has crosscheck failures:\n{}",
            messages.join("\n")
        )))
    }
}

#[cfg(test)]
mod tests {
    use callisto_model::{Diagnostic, DiagnosticCode, DiagnosticSeverity, StrictFlag};

    use super::abort_on_crosscheck_failures;

    fn strict_graph_diagnostic() -> Diagnostic {
        Diagnostic {
            code: DiagnosticCode::GraphEdgeDisagreement,
            severity: DiagnosticSeverity::Warning,
            message: "moon declares a -> b but no manifest declares it".to_string(),
            package: None,
            path: None,
            escalated_by: Some(StrictFlag::StrictGraph),
            governed_by: None,
        }
    }

    /// Neither flag set: a `StrictGraph`-tagged warning stays a warning, so
    /// `snapshot`/`tag` must proceed rather than abort.
    #[test]
    fn neither_flag_leaves_warning_diagnostics_unescalated() {
        let diags = vec![strict_graph_diagnostic()];
        assert!(abort_on_crosscheck_failures(&diags, false, false).is_ok());
    }

    /// This is the bug fix under test: previously `snapshot`/`tag` hardcoded
    /// `escalate(&mut diags, true, true)`, reachable only from behind an
    /// `if args.strict` gate -- `--strict-graph` alone had no field to carry
    /// it and no way to trigger escalation on its own. Now `strict_graph:
    /// true` with `strict: false` must, by itself, escalate a
    /// `StrictFlag::StrictGraph` diagnostic to `Error` and abort.
    #[test]
    fn strict_graph_alone_now_escalates_graph_diagnostics() {
        let diags = vec![strict_graph_diagnostic()];
        let err = abort_on_crosscheck_failures(&diags, false, true).unwrap_err();
        assert!(err.to_string().contains("moon declares a -> b"));
    }

    /// `--strict` alone still escalates `StrictGraph`-tagged diagnostics too
    /// (matching `escalate`'s own `strict || strict_graph` rule for that
    /// flag, and matching `status`/`validate`/`version`'s behavior).
    #[test]
    fn strict_alone_still_escalates_graph_diagnostics() {
        let diags = vec![strict_graph_diagnostic()];
        assert!(abort_on_crosscheck_failures(&diags, true, false).is_err());
    }
}
