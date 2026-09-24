use std::process::ExitCode;

use callisto_graph::commands::StatusOptions;
use callisto_model::{DiagnosticSeverity, StatusReport};

use crate::cli::{GlobalArgs, OutputFormat, StatusArgs};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

/// Compute the --check exit code as a raw `u8` from a status report that has
/// no error-level diagnostics (the caller checks that first).
///
/// Returns:
/// - `2` when there are packages with pending changesets.
/// - `3` when no changesets are pending (sentinel for CI scripts).
pub(crate) fn check_exit_code_raw(report: &StatusReport) -> u8 {
    if report.has_changesets {
        2
    } else {
        3
    }
}

pub fn handle(args: StatusArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let opts = StatusOptions {
        strict: args.strict,
        strict_graph: args.strict_graph,
    };

    let inference = crate::workspace::select_inference();
    let report = callisto_graph::commands::status(&ws, &inference, &opts)?;

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_status(&report, crate::color::enabled(), &mut crate::color::stdout())?,
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

    // SPEC-DX-STATUS-ADD AC-04: check=true with pending changesets (and no
    // errors) must return 2.
    #[test]
    fn check_exit_code_returns_2_when_changesets_pending() {
        let report = make_report(vec![vec!["cs-001"]]);
        assert_eq!(
            check_exit_code_raw(&report),
            2,
            "check_exit_code must return 2 when changesets are pending and there are no errors"
        );
    }

    // SPEC-DX-STATUS-ADD AC-04: check=true with no pending changesets (and no
    // errors) must return 3.
    #[test]
    fn check_exit_code_returns_3_when_no_changesets_pending() {
        let report = make_report(vec![vec![]]);
        assert_eq!(
            check_exit_code_raw(&report),
            3,
            "check_exit_code must return 3 when no changesets are pending and there are no errors"
        );
    }

    #[test]
    fn handle_text_format_succeeds_on_empty_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();

        let global = GlobalArgs {
            format: OutputFormat::Text,
            cwd: root.to_path_buf(),
            dry_run: false,
        };

        let result = handle(
            StatusArgs {
                strict: false,
                strict_graph: false,
                check: false,
            },
            &global,
        );
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }
}
