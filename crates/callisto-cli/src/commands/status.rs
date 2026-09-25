use std::process::ExitCode;

use callisto_graph::commands::StatusOptions;
use callisto_model::{DiagnosticSeverity, StatusReport};

use crate::cli::{GlobalArgs, OutputFormat, StatusArgs};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

/// Whether a status report has any error-level diagnostic. Pending state
/// (`report.pending`) never factors in: `--check` is a conventional
/// errors-only gate (SPEC-DX-STATUS-ADD AC-04), and a script that wants
/// pending state reads `status --format json`'s `pending` count instead.
pub(crate) fn has_error_diagnostics(report: &StatusReport) -> bool {
    report
        .diagnostics
        .iter()
        .any(|d| d.severity == DiagnosticSeverity::Error)
}

pub fn handle(args: StatusArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let opts = StatusOptions { strict: args.strict };

    let inference = crate::workspace::select_inference();
    let report = callisto_graph::commands::status(&ws, &inference, &opts)?;

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_status(&report, crate::color::enabled(), &mut crate::color::stdout())?,
    }

    // `status()` already applies --strict escalation before
    // returning, so `has_error_diagnostics` alone reflects it.
    if args.check && has_error_diagnostics(&report) {
        Ok(ExitCode::FAILURE)
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

    fn make_report(changesets: Vec<Vec<&str>>, diagnostics: Vec<callisto_model::Diagnostic>) -> StatusReport {
        let has_changesets = changesets.iter().any(|cs| !cs.is_empty());
        let packages: Vec<StatusPackageRecord> = changesets
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
            .collect();
        let pending = packages.iter().filter(|p| p.pending_severity.is_some()).count() as u32;
        StatusReport {
            schema_version: callisto_model::SCHEMA_VERSION,
            has_changesets,
            pending,
            packages,
            diagnostics,
        }
    }

    fn error_diagnostic() -> callisto_model::Diagnostic {
        callisto_model::Diagnostic {
            code: callisto_model::DiagnosticCode::UnknownPackage,
            severity: DiagnosticSeverity::Error,
            message: "unknown package".to_string(),
            package: None,
            path: None,
            governed_by: None,
            escalated_by: None,
        }
    }

    // SPEC-DX-STATUS-ADD AC-04: pending changesets alone (no error-level
    // diagnostics) must not count as an error -- `--check` is an errors-only
    // gate now, unlike the pre-correction 2/3 scheme.
    #[test]
    fn has_error_diagnostics_is_false_for_pending_changesets_with_no_errors() {
        let report = make_report(vec![vec!["cs-001"]], vec![]);
        assert!(
            !has_error_diagnostics(&report),
            "pending changesets alone must not count as an error"
        );
    }

    #[test]
    fn has_error_diagnostics_is_false_when_nothing_pending_and_no_errors() {
        let report = make_report(vec![vec![]], vec![]);
        assert!(!has_error_diagnostics(&report));
    }

    // SPEC-DX-STATUS-ADD AC-04: an error-level diagnostic must be reported
    // regardless of pending state.
    #[test]
    fn has_error_diagnostics_is_true_when_an_error_diagnostic_is_present() {
        let report = make_report(vec![vec![]], vec![error_diagnostic()]);
        assert!(has_error_diagnostics(&report));

        let report_with_pending = make_report(vec![vec!["cs-001"]], vec![error_diagnostic()]);
        assert!(has_error_diagnostics(&report_with_pending));
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
                check: false,
            },
            &global,
        );
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }

    /// SPEC-DX-STATUS-ADD AC-04: `status --check` on a workspace with a
    /// pending changeset but no error-level diagnostics must exit 0 -- the
    /// gate is errors-only, not pending-state.
    #[test]
    fn handle_check_true_with_pending_and_no_errors_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/pkg-a\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("crates/pkg-a")).unwrap();
        std::fs::write(
            root.join("crates/pkg-a/Cargo.toml"),
            "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(root.join(".changeset/bump.md"), "---\npkg-a: patch\n---\n\nfix.\n").unwrap();
        callisto_fixtures::git::init_repo(root);

        let global = GlobalArgs {
            format: OutputFormat::Json,
            cwd: root.to_path_buf(),
            dry_run: false,
        };

        let result = handle(
            StatusArgs {
                strict: false,
                check: true,
            },
            &global,
        );
        assert_eq!(
            result.unwrap(),
            ExitCode::SUCCESS,
            "status --check must succeed when changesets are pending but nothing is an error"
        );
    }

    /// SPEC-DX-STATUS-ADD AC-04: `status --check` must exit 1 when a
    /// changeset carries an error-level diagnostic (e.g. names an unknown
    /// package), regardless of pending state.
    #[test]
    fn handle_check_true_with_error_diagnostic_fails() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/pkg-a\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("crates/pkg-a")).unwrap();
        std::fs::write(
            root.join("crates/pkg-a/Cargo.toml"),
            "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(
            root.join(".changeset/bad.md"),
            "---\ncargo/does-not-exist: patch\n---\n\nbad.\n",
        )
        .unwrap();
        callisto_fixtures::git::init_repo(root);

        let global = GlobalArgs {
            format: OutputFormat::Json,
            cwd: root.to_path_buf(),
            dry_run: false,
        };

        let result = handle(
            StatusArgs {
                strict: false,
                check: true,
            },
            &global,
        );
        assert_eq!(result.unwrap(), ExitCode::FAILURE);
    }
}
