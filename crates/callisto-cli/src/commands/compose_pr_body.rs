use std::io::Read;
use std::process::ExitCode;

use callisto_graph::commands::PrBodyOptions;

use crate::cli::{ComposePrBodyArgs, GlobalArgs, OutputFormat};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::{load_workspace, select_inference};

pub fn handle(args: ComposePrBodyArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let existing_body = match args.existing_body {
        Some(ref s) if s == "-" => {
            let mut stdin_buf = String::new();
            std::io::stdin().read_to_string(&mut stdin_buf)?;
            let clean_stdin = stdin_buf.strip_prefix('\u{FEFF}').unwrap_or(&stdin_buf).to_string();
            Some(clean_stdin)
        }
        other => other,
    };

    let inference = select_inference();
    let opts = PrBodyOptions {
        existing_body,
        labels: args.labels,
        branch: args.branch,
    };

    let report = callisto_graph::commands::compose_pr_body(&ws, &inference, &opts)?;

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_compose_pr_body(&report, &mut std::io::stdout())?,
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_json_format_succeeds_with_a_pending_changeset() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        for (program, args) in [
            ("git", vec!["init", "-q"]),
            ("git", vec!["config", "user.name", "Test"]),
            ("git", vec!["config", "user.email", "test@test.dev"]),
        ] {
            drop(
                std::process::Command::new(program)
                    .args(args)
                    .current_dir(root)
                    .output(),
            );
        }

        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/pkg-a\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("crates/pkg-a")).unwrap();
        std::fs::write(
            root.join("crates/pkg-a/Cargo.toml"),
            "[package]\nname = \"pkg-a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        let global = GlobalArgs {
            format: OutputFormat::Json,
            cwd: root.to_path_buf(),
            dry_run: false,
        };

        crate::commands::init::handle(crate::cli::InitArgs { yes: true }, &global).unwrap();
        crate::commands::add::handle(
            crate::cli::AddArgs {
                packages: vec!["pkg-a:patch".to_string()],
                summary: Some("Fix a bug".to_string()),
            },
            &global,
        )
        .unwrap();

        let result = handle(
            ComposePrBodyArgs {
                existing_body: None,
                labels: vec![],
                branch: None,
            },
            &global,
        );
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }
}
