use std::process::ExitCode;

use callisto_graph::commands::MatrixOptions;

use crate::cli::{GlobalArgs, MatrixArgs, OutputFormat};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: MatrixArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let opts = MatrixOptions {
        package: args.package.clone(),
    };
    let report = callisto_graph::commands::matrix(&ws, &opts)?;

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_matrix(&report, &mut std::io::stdout())?,
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::MatrixArgs;

    /// AC-003 (handler slice): an empty workspace produces exit code 0.
    /// AC-003b: bare invocation (global.format default = Text) does not error.
    #[test]
    fn handle_empty_workspace_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = []\nresolver = \"2\"\n",
        )
        .unwrap();
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
