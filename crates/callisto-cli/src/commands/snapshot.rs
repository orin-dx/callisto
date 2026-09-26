use std::process::ExitCode;

use callisto_graph::apply::{apply_version_plan, ApplyOptions};
use callisto_model::ApplyPermit;

use crate::cli::{GlobalArgs, OutputFormat, SnapshotArgs};
use crate::commands::abort_on_graph_errors;
use crate::error::CliError;
use crate::output::{emit_line, emit_report};
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: SnapshotArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    // Promote graph diagnostics per
    // `--strict` and abort before touching any files.
    abort_on_graph_errors(ws.graph.diagnostics(), args.strict)?;

    let (plan, report) = callisto_graph::commands::plan_snapshot(&ws, &args.tag)?;

    // Every package converges on the same synthetic version, so each ecosystem's lockfile must
    // refresh too -- otherwise a stale lockfile fails `--locked` resolution right after the snapshot.
    let apply_opts = ApplyOptions {
        refresh_lockfiles: true,
        transient: true,
    };

    if let Some(permit) = ApplyPermit::granted_unless_dry_run(global.dry_run) {
        apply_version_plan(&ws.root, &plan, &runner, &apply_opts, &permit)?;
    }

    match global.format {
        OutputFormat::Json => emit_report(&mut std::io::stdout(), &report, global.dry_run)?,
        OutputFormat::Text => {
            if global.dry_run {
                emit_line("[DRY-RUN] Snapshot preview (no files modified):")?;
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

        callisto_fixtures::git::init_repo(root);

        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/pkg-a\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        let pkg = root.join("crates/pkg-a");
        std::fs::create_dir_all(pkg.join("src")).unwrap();
        std::fs::write(
            pkg.join("Cargo.toml"),
            "[package]\nname = \"pkg-a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        // A real target so `cargo update --workspace` (now run in snapshot's transient apply too)
        // can load the manifest.
        std::fs::write(pkg.join("src/lib.rs"), "").unwrap();

        let global = GlobalArgs {
            format: OutputFormat::Json,
            cwd: root.to_path_buf(),
            dry_run: false,
        };

        callisto_fixtures::scaffold_callisto(&global.cwd);
        crate::commands::add::handle(
            crate::cli::AddArgs {
                packages: vec!["pkg-a:patch".to_string()],
                summary: Some("Seed a changeset".to_string()),
            },
            &global,
        )
        .unwrap();

        assert!(std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .args(["-c", "commit.gpgSign=false", "commit", "-m", "seed", "--allow-empty"])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .args(["rev-parse", "--verify", "HEAD"])
            .current_dir(root)
            .status()
            .unwrap()
            .success());

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
