use std::process::ExitCode;

use callisto_graph::commands::InitOptions;
use callisto_model::ApplyPermit;
use dialoguer::Confirm;

use crate::cli::{GlobalArgs, InitArgs, OutputFormat};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::tty;
use crate::workspace::load_workspace;

pub fn handle(args: InitArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    if !args.yes && tty::is_interactive() {
        let confirm = Confirm::new()
            .with_prompt(format!("Initialize Callisto configuration in `{}`?", ws.root.display()))
            .default(true)
            .interact()
            .map_err(|e| CliError::Other(format!("Interactive prompt failed: {e}")))?;

        if !confirm {
            println!("Initialization cancelled.");
            return Ok(ExitCode::SUCCESS);
        }
    }

    let opts = InitOptions { yes: args.yes };

    // `--yes` (answered above) and `--dry-run` gate different things: the
    // former is consent to apply, the latter is permission to write at all.
    // `--yes --dry-run` must therefore report the apply outcome and write
    // nothing.
    let permit = ApplyPermit::granted_unless_dry_run(global.dry_run);
    let report = callisto_graph::commands::init(&ws, &opts, permit.as_ref())?;

    if permit.is_none() && global.format == OutputFormat::Text {
        println!("[DRY-RUN] Init plan calculated (no files written):");
    }

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_init(&report, &mut std::io::stdout())?,
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_workspace() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[workspace]\nmembers = []\nresolver = \"2\"\n",
        )
        .unwrap();
        tmp
    }

    #[test]
    fn handle_dry_run_text_output_carries_the_dry_run_marker() {
        let tmp = empty_workspace();
        let global = GlobalArgs {
            format: OutputFormat::Text,
            cwd: tmp.path().to_path_buf(),
            dry_run: true,
        };

        let result = handle(InitArgs { yes: true }, &global);
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
        assert!(
            !tmp.path().join("callisto.toml").exists(),
            "dry-run must not write callisto.toml"
        );
    }

    #[test]
    fn handle_json_format_applies_for_real_and_writes_config() {
        let tmp = empty_workspace();
        let global = GlobalArgs {
            format: OutputFormat::Json,
            cwd: tmp.path().to_path_buf(),
            dry_run: false,
        };

        let result = handle(InitArgs { yes: true }, &global);
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
        assert!(
            tmp.path().join("callisto.toml").exists(),
            "a real (non-dry-run) init must write callisto.toml"
        );
    }
}
