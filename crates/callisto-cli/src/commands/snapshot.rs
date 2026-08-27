use std::process::ExitCode;

use callisto_graph::apply::{apply_version_plan, ApplyOptions};
use callisto_model::{ApplyPermit, DiagnosticSeverity};

use crate::cli::{GlobalArgs, OutputFormat, SnapshotArgs};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: SnapshotArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    // Under `--strict`, promote graph diagnostics (including crosscheck
    // failures) to Error severity and abort before touching any files.
    if args.strict {
        let mut diags = ws.graph.diagnostics().to_vec();
        callisto_graph::commands::escalate(&mut diags, true, true);
        let has_errors = diags.iter().any(|d| d.severity == DiagnosticSeverity::Error);
        if has_errors {
            let messages: Vec<String> = diags
                .iter()
                .filter(|d| d.severity == DiagnosticSeverity::Error)
                .map(|d| d.message.clone())
                .collect();
            return Err(CliError::Other(format!(
                "--strict: workspace graph has crosscheck failures:\n{}",
                messages.join("\n")
            )));
        }
    }

    let (plan, report) = callisto_graph::commands::plan_snapshot(&ws, &args.tag)?;

    let apply_opts = ApplyOptions {
        refresh_lockfiles: false,
        transient: true,
    };

    if let Some(permit) = ApplyPermit::granted_unless_dry_run(global.dry_run) {
        apply_version_plan(&ws.root, &plan, &runner, &apply_opts, &permit)?;
    }

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => {
            if global.dry_run {
                println!("[DRY-RUN] Snapshot preview (no files modified):");
            }
            render::render_snapshot(&report, &mut std::io::stdout())?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real (non-dry-run) snapshot must actually apply the version plan to
    /// disk, not just compute and report it.
    #[test]
    fn handle_without_dry_run_applies_the_snapshot_for_real() {
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
        let pkg = root.join("crates/pkg-a");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("Cargo.toml"),
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
                summary: Some("Seed a changeset".to_string()),
            },
            &global,
        )
        .unwrap();

        drop(
            std::process::Command::new("git")
                .args(["add", "."])
                .current_dir(root)
                .output(),
        );
        drop(
            std::process::Command::new("git")
                .args(["commit", "-m", "seed", "--allow-empty"])
                .current_dir(root)
                .output(),
        );

        let result = handle(
            SnapshotArgs {
                tag: "canary".to_string(),
                strict: false,
            },
            &global,
        );
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);

        let cargo_toml = std::fs::read_to_string(pkg.join("Cargo.toml")).unwrap();
        assert!(
            cargo_toml.contains("-canary"),
            "a real snapshot apply must write the snapshot version to disk, got:\n{cargo_toml}"
        );
    }
}
