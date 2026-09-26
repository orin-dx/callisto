pub mod add;
pub mod completions;
pub mod compose_pr_body;
pub mod help;
pub mod init;
pub mod matrix;
pub mod pre;
pub mod release;
pub mod release_pr;
pub mod schema;
pub mod snapshot;
pub mod status;
pub mod version;

/// Reads `arg` as a JSON document: a literal `-` reads from stdin, a value
/// starting with `{` is treated as inline JSON, anything else is read as a
/// file path. Shared by every command that accepts a report/plan as either
/// a file, inline JSON, or piped stdin (`release-pr verify --decision`,
/// `--snapshot`).
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

/// Escalates workspace graph diagnostics per `strict` (via
/// `callisto_graph::commands::escalate`) and errors out naming every
/// diagnostic that is `Error` severity afterward.
///
/// Used by `snapshot`, which must abort *before* touching any
/// files/tags on an error diagnostic -- unlike `status`/`validate`/`version`,
/// which fold escalated diagnostics into their report and gate on exit code
/// instead of an `Err`.
pub(crate) fn abort_on_graph_errors(
    diagnostics: &[callisto_model::Diagnostic],
    strict: bool,
) -> Result<(), crate::error::CliError> {
    let mut diags = diagnostics.to_vec();
    callisto_graph::commands::escalate(&mut diags, strict);

    let messages: Vec<String> = diags
        .iter()
        .filter(|d| d.severity == callisto_model::DiagnosticSeverity::Error)
        .map(|d| d.message.clone())
        .collect();

    if messages.is_empty() {
        Ok(())
    } else {
        Err(crate::error::CliError::StrictDiagnosticsPresent { messages })
    }
}

#[cfg(test)]
mod tests {
    use callisto_model::{Diagnostic, DiagnosticCode, DiagnosticSeverity, StrictFlag};

    use super::abort_on_graph_errors;

    fn strict_diagnostic() -> Diagnostic {
        Diagnostic {
            code: DiagnosticCode::RangeNotRoundTrippable,
            severity: DiagnosticSeverity::Warning,
            message: "graph warning a -> b".to_string(),
            package: None,
            path: None,
            escalated_by: Some(StrictFlag::Strict),
            governed_by: None,
        }
    }

    /// Without `--strict`, a `Strict`-tagged warning stays a warning, so
    /// `snapshot` must proceed rather than abort.
    #[test]
    fn without_strict_warning_diagnostics_stay_unescalated() {
        let diags = vec![strict_diagnostic()];
        assert!(abort_on_graph_errors(&diags, false).is_ok());
    }

    #[test]
    fn strict_escalates_and_aborts_naming_the_diagnostic() {
        let diags = vec![strict_diagnostic()];
        let err = abort_on_graph_errors(&diags, true).unwrap_err();
        assert!(err.to_string().contains("graph warning a -> b"));
    }
}
