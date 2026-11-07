use std::io::Read;
use std::process::ExitCode;

use callisto_graph::commands::PrBodyOptions;

use crate::cli::{ComposePrBodyArgs, GlobalArgs, OutputFormat};
use crate::error::CliError;
use crate::output::emit_report;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::{load_workspace, select_inference, workspace_root};

pub fn handle(args: ComposePrBodyArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let existing_body = match args.existing_body {
        Some(ref s) if s == "-" => {
            let mut stdin_buf = String::new();
            std::io::stdin().read_to_string(&mut stdin_buf)?;
            let clean_stdin = stdin_buf.strip_prefix('\u{FEFF}').unwrap_or(&stdin_buf).to_string();
            Some(clean_stdin)
        }
        other => other,
    };

    let opts = PrBodyOptions {
        existing_body,
        branch: args.branch,
    };

    let report = match args.preview {
        Some(path) => {
            // The workspace root, as `version --emit-preview` resolves it, not the process directory.
            let path = workspace_root(global, &CliCommandRunner)?.join(path);
            let text = std::fs::read_to_string(&path).map_err(|source| CliError::Io {
                source,
                path: Some(path.clone()),
            })?;
            let mut preview = callisto_graph::release_preview::ReleasePreview::from_json(&text).map_err(|detail| {
                CliError::ReleasePrArgJsonInvalid {
                    flag: "preview",
                    detail,
                }
            })?;
            if args.publish_plan {
                let runner = CliCommandRunner;
                let ws = load_workspace(global, &runner)?;
                let planned = callisto_graph::commands::plan_workspace_release(
                    &ws,
                    &[],
                    callisto_graph::commands::LocalReleaseSource::Preview,
                );
                preview = preview.with_publish(callisto_graph::release_preview::publish_outlook(planned));
            }
            callisto_graph::commands::render_pr_body_from_preview(&preview, &opts)
        }
        None => {
            let runner = CliCommandRunner;
            let ws = load_workspace(global, &runner)?;
            callisto_graph::commands::compose_pr_body(&ws, &select_inference(), &opts)?
        }
    };

    match global.format {
        OutputFormat::Json => emit_report(&mut std::io::stdout(), &report, global.dry_run)?,
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

        callisto_fixtures::git::init_repo(root);

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

        callisto_fixtures::scaffold_callisto(&global.cwd);
        crate::commands::add::handle(
            crate::cli::AddArgs {
                packages: vec!["cargo/pkg-a:patch".to_string()],
                summary: Some("Fix a bug".to_string()),
            },
            &global,
        )
        .unwrap();

        let result = handle(
            ComposePrBodyArgs {
                existing_body: None,
                branch: None,
                preview: None,
                publish_plan: false,
            },
            &global,
        );
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }
}
