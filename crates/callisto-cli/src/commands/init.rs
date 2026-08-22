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
