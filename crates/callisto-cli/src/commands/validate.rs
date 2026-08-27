use std::process::ExitCode;

use callisto_graph::commands::ValidateOptions;

use crate::cli::{GlobalArgs, OutputFormat, ValidateArgs};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: ValidateArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let opts = ValidateOptions {
        staged: args.staged,
        since: args.since,
        strict: args.strict,
        strict_graph: args.strict_graph,
    };

    let report = callisto_graph::commands::validate(&ws, &opts)?;

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_validate(&report, &mut std::io::stdout())?,
    }

    if report.ok {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A changeset that parses cleanly (non-empty entries, non-empty summary)
    /// but names a package absent from the workspace -- a validation-level
    /// `Error` diagnostic (`UnknownPackage`), not a parse-time failure.
    fn seed_workspace_with_unknown_package_changeset() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();
        let changeset_dir = root.join(".changeset");
        std::fs::create_dir_all(&changeset_dir).unwrap();
        std::fs::write(
            changeset_dir.join("bad.md"),
            "---\nnot-a-real-package: patch\n---\n\nSome change.\n",
        )
        .unwrap();
        tmp
    }

    fn opts() -> ValidateArgs {
        ValidateArgs {
            staged: false,
            since: None,
            strict: false,
            strict_graph: false,
        }
    }

    #[test]
    fn handle_text_format_reports_clean_workspace_as_success() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();

        let global = GlobalArgs {
            format: OutputFormat::Text,
            cwd: root.to_path_buf(),
            dry_run: false,
        };

        let result = handle(opts(), &global);
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }

    #[test]
    fn handle_returns_failure_exit_code_when_report_is_not_ok() {
        let tmp = seed_workspace_with_unknown_package_changeset();
        let global = GlobalArgs {
            format: OutputFormat::Json,
            cwd: tmp.path().to_path_buf(),
            dry_run: false,
        };

        let result = handle(opts(), &global).expect("validate should not error on an invalid-but-parseable changeset");
        assert_eq!(
            result,
            ExitCode::FAILURE,
            "a workspace with a changeset naming an unknown package must report ok=false"
        );
    }
}
