use std::process::ExitCode;

use callisto_graph::commands::MatrixOptions;

use crate::cli::{GlobalArgs, MatrixArgs, OutputFormat};
use crate::error::CliError;
use crate::output::emit_report;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: MatrixArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let opts = MatrixOptions {
        package: args.package.clone(),
        napi_crates: true,
    };
    let report = callisto_graph::commands::matrix(&ws, &opts)?;

    match global.format {
        OutputFormat::Json => emit_report(&mut std::io::stdout(), &report, global.dry_run)?,
        OutputFormat::Text => render::render_matrix(&report, &mut std::io::stdout())?,
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::MatrixArgs;

    /// An empty workspace produces exit code 0.
    /// Bare invocation (global.format default = Text) does not error.
    #[test]
    fn handle_empty_workspace_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        callisto_fixtures::git::init_repo(root);
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();

        let global = crate::cli::GlobalArgs {
            format: crate::cli::OutputFormat::Text,
            cwd: root.to_path_buf(),
            dry_run: false,
        };
        let args = MatrixArgs { package: None };

        let result = handle(args, &global);
        assert!(result.is_ok(), "expected Ok(ExitCode), got {result:?}");
        assert_eq!(result.unwrap(), std::process::ExitCode::SUCCESS);
    }
}
