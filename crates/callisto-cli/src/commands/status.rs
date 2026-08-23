use std::process::ExitCode;

use callisto_graph::commands::StatusOptions;
use callisto_model::{DiagnosticSeverity, StatusReport};

use crate::cli::{GlobalArgs, OutputFormat, StatusArgs};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

/// Compute the --check exit code as a raw `u8` from a status report.
///
/// Returns:
/// - `1` when there are packages with pending changesets (maps to `ExitCode::FAILURE`).
/// - `2` when no changesets are pending (sentinel for CI scripts).
pub(crate) fn check_exit_code_raw(report: &StatusReport) -> u8 {
    let has_pending = report.packages.iter().any(|p| !p.pending_changesets.is_empty());
    if has_pending {
        1
    } else {
        2
    }
}

pub fn handle(args: StatusArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let opts = StatusOptions {
        strict: args.strict,
        strict_graph: args.strict_graph,
    };

    let report = callisto_graph::commands::status(&ws, &opts)?;

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_status(&report, &mut std::io::stdout())?,
    }

    let has_errors = report
        .diagnostics
        .iter()
        .any(|d| d.severity == DiagnosticSeverity::Error || (args.strict && d.severity == DiagnosticSeverity::Warning));

    if has_errors {
        Ok(ExitCode::FAILURE)
    } else if args.check {
        Ok(ExitCode::from(check_exit_code_raw(&report)))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::{
        Ecosystem, PackageId, ReleaseTrigger, Severity, StatusPackageRecord, Version, VersionGrammar,
    };

    fn v1() -> Version {
        Version::parse("1.0.0", VersionGrammar::SemVer).unwrap()
    }

    fn pkg(name: &str) -> PackageId {
        PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: name.to_string(),
        }
    }

    fn make_report(changesets: Vec<Vec<&str>>) -> StatusReport {
        let has_changesets = changesets.iter().any(|cs| !cs.is_empty());
        StatusReport {
            schema_version: callisto_model::SCHEMA_VERSION,
            has_changesets,
            packages: changesets
                .into_iter()
                .enumerate()
                .map(|(i, cs)| StatusPackageRecord {
                    package: pkg(&format!("crate-{i}")),
                    current_version: v1(),
                    last_tag: None,
                    last_released_version: None,
                    pending_severity: if cs.is_empty() { None } else { Some(Severity::Patch) },
                    changed_since_last_tag: !cs.is_empty(),
                    release_trigger: ReleaseTrigger::Changeset,
                    pending_changesets: cs.into_iter().map(|s| s.to_string()).collect(),
                })
                .collect(),
            diagnostics: vec![],
        }
    }

    // QW-3: check=true with pending changesets must return 1 (FAILURE).
    #[test]
    fn check_exit_code_returns_failure_when_changesets_pending() {
        let report = make_report(vec![vec!["cs-001"]]);
        assert_eq!(
            check_exit_code_raw(&report),
            1,
            "check_exit_code must return 1 (FAILURE) when changesets are pending"
        );
    }

    // QW-3: check=true with no pending changesets must return exit code 2.
    #[test]
    fn check_exit_code_returns_2_when_no_changesets_pending() {
        let report = make_report(vec![vec![]]);
        assert_eq!(
            check_exit_code_raw(&report),
            2,
            "check_exit_code must return 2 when no changesets are pending"
        );
    }
}
