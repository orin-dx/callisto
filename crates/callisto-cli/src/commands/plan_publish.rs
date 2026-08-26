use std::process::ExitCode;

use callisto_graph::commands::PublishOptions;

use crate::cli::{GlobalArgs, OutputFormat, PlanPublishArgs};
use crate::error::CliError;
use crate::output::write_report_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: PlanPublishArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let opts = PublishOptions { only: args.only };
    let report = callisto_graph::commands::plan_publish(&ws, &opts)?;

    match global.format {
        OutputFormat::Json => write_report_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_publish(&report, &mut std::io::stdout())?,
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_json_format_succeeds_on_empty_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();

        let global = GlobalArgs {
            format: OutputFormat::Json,
            cwd: root.to_path_buf(),
            dry_run: false,
        };

        let result = handle(PlanPublishArgs::default(), &global);
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }
}
