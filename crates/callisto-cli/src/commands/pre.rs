use std::fs;
use std::path::Path;
use std::process::ExitCode;

use callisto_format::{parse_pre_json, write_pre_json, write_pre_json_preserving, PreMode, PreState};
use callisto_model::{ApplyPermit, CommandRunner, SCHEMA_VERSION};
use serde_json::json;

use crate::cli::{GlobalArgs, OutputFormat, PreArgs};
use crate::error::CliError;
use crate::output::{log_line, write_json};
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

/// Extracted from [`handle`] so a fake [`CommandRunner`] can exercise the `git add` without shelling out for real.
fn stage_pre_json(runner: &dyn CommandRunner, root: &Path, rel_path: &Path) -> Result<(), CliError> {
    let rel_str = rel_path.to_string_lossy();
    let output = runner.run("git", &["add", rel_str.as_ref()], root)?;
    if !output.success() {
        return Err(callisto_model::CommandError::Failed {
            program: "git".to_string(),
            exit_code: output.exit_code,
            stderr: output.redacted_stderr(),
        }
        .into());
    }
    Ok(())
}

/// Mirrors `add`'s dry-run preview so both commands report the not-yet-written content the same way.
fn preview(global: &GlobalArgs, mode: &str, tag: &str, rel_path: &Path, content: &str) -> Result<(), CliError> {
    let rel_str = rel_path.to_string_lossy();
    match global.format {
        OutputFormat::Json => {
            let env = json!({
                "schemaVersion": SCHEMA_VERSION,
                "command": "pre",
                "dryRun": true,
                "mode": mode,
                "tag": tag,
                "path": rel_str,
                "content": content
            });
            write_json(&mut std::io::stdout(), &env)?;
        }
        OutputFormat::Text => {
            println!("[DRY-RUN] Would write {rel_str} (no files written)\n\n{content}");
        }
    }
    Ok(())
}

/// Handles the `pre enter` and `pre exit` subcommands.
///
/// `pre enter` writes `.changeset/pre.json` with the supplied tag and a snapshot
/// of current package versions. `pre exit` updates an existing `pre.json`'s mode
/// to `exit`. Under `--dry-run`, neither subcommand writes any file; instead the
/// would-be content is rendered to stdout.
///
/// # Errors
///
/// Returns [`CliError::PreAlreadyActive`] if `enter` runs while `pre.json`'s mode
/// is already `pre`, and [`CliError::PreNotActive`] if `exit` runs with no
/// `pre.json` on disk. Otherwise returns [`CliError::Io`] if reading an existing
/// `pre.json` fails, or if any write fails (`enter` subcommand on a real run).
pub fn handle(args: PreArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let permit = ApplyPermit::granted_unless_dry_run(global.dry_run);

    match args {
        PreArgs::Enter { tag } => {
            // Bug 3: reject empty tags before any workspace I/O.
            if tag.trim().is_empty() {
                return Err(CliError::PreTagEmpty);
            }

            let ws = load_workspace(global, &runner)?;

            // Reject re-entering only while a cycle is actually active (mode `pre`); a `pre.json` left
            // behind in `exit` mode is a finished cycle, so entering starts a new one from where it
            // left off instead of erroring on a file that merely still exists on disk.
            let rel_pre_path = ws.config.pre_json_path();
            let pre_path = ws.root.join(&rel_pre_path);
            let pre_dir = pre_path.parent().unwrap_or(&ws.root).to_path_buf();
            let pre_state = if pre_path.exists() {
                let existing_text = fs::read_to_string(&pre_path).map_err(|source| CliError::Io {
                    source,
                    path: Some(pre_path.clone()),
                })?;
                let existing = parse_pre_json(&existing_text)?;
                if existing.mode == PreMode::Pre {
                    return Err(CliError::PreAlreadyActive);
                }
                // Re-entering after `exit` carries the same initial-versions baseline and recorded
                // changesets forward under the new tag, matching a resumed rather than a fresh cycle.
                PreState {
                    mode: PreMode::Pre,
                    tag: tag.clone(),
                    ..existing
                }
            } else {
                let initial = ws.initial_versions()?;
                let mut snapshot = indexmap::IndexMap::new();
                for (k, v) in initial {
                    snapshot.insert(k, v);
                }
                PreState::entering(tag.clone(), snapshot)
            };
            let text = write_pre_json(&pre_state);

            let Some(permit) = permit else {
                preview(global, "pre", &tag, &rel_pre_path, &text)?;
                return Ok(ExitCode::SUCCESS);
            };

            fs::create_dir_all(&pre_dir)?;
            callisto_manifests::atomic::atomic_write(&pre_path, &text, &permit)?;

            // Bug 4: stage pre.json so it is included in the next commit.
            stage_pre_json(&runner, &ws.root, &rel_pre_path)?;

            match global.format {
                OutputFormat::Json => {
                    let env = json!({
                        "schemaVersion": SCHEMA_VERSION,
                        "command": "pre",
                        "mode": "pre",
                        "tag": tag
                    });
                    write_json(&mut std::io::stdout(), &env)?;
                }
                OutputFormat::Text => {
                    log_line(global.format, &format!("Entered pre mode with tag `{tag}`"));
                }
            }
        }
        PreArgs::Exit => {
            let start = dunce::canonicalize(&global.cwd).map_err(|source| CliError::Io {
                source,
                path: Some(global.cwd.clone()),
            })?;
            let root = callisto_graph::locate::find_workspace_root(&start, &runner)?;
            let config = callisto_graph::load_config(&root)?;
            let rel_pre_path = config.pre_json_path();
            let pre_path = root.join(&rel_pre_path);

            if !pre_path.exists() {
                return Err(CliError::PreNotActive);
            }
            let text = fs::read_to_string(&pre_path).map_err(|source| CliError::Io {
                source,
                path: Some(pre_path.clone()),
            })?;
            let mut pre_state = parse_pre_json(&text)?;

            // Reject double-exit.
            if pre_state.mode == PreMode::Exit {
                return Err(CliError::PreAlreadyExited);
            }

            pre_state.mode = PreMode::Exit;

            let updated = write_pre_json_preserving(&pre_state, &text);

            let Some(permit) = permit else {
                preview(global, "exit", &pre_state.tag, &rel_pre_path, &updated)?;
                return Ok(ExitCode::SUCCESS);
            };

            callisto_manifests::atomic::atomic_write(&pre_path, &updated, &permit)?;

            // Mutates the same tracked file `pre enter` created, so it must be staged the same way.
            stage_pre_json(&runner, &root, &rel_pre_path)?;

            match global.format {
                OutputFormat::Json => {
                    let env = json!({
                        "schemaVersion": SCHEMA_VERSION,
                        "command": "pre",
                        "mode": "exit",
                        "tag": pre_state.tag
                    });
                    write_json(&mut std::io::stdout(), &env)?;
                }
                OutputFormat::Text => {
                    log_line(global.format, "Exiting pre mode");
                }
            }
        }
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use callisto_model::{CommandError, CommandOutput, CommandRunner};

    use super::{handle, stage_pre_json};
    use crate::cli::{GlobalArgs, OutputFormat, PreArgs};

    /// `pre enter --dry-run --format text` must not write `.changeset/pre.json`
    /// and must still succeed, rendering the preview via the Text branch
    /// (the Json branch is already covered by other callers of `preview`).
    #[test]
    fn handle_enter_dry_run_text_format_previews_without_writing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        callisto_fixtures::git::init_repo(root);
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();

        let global = GlobalArgs {
            format: OutputFormat::Text,
            cwd: root.to_path_buf(),
            dry_run: true,
        };

        let result = handle(
            PreArgs::Enter {
                tag: "beta".to_string(),
            },
            &global,
        );
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(result.unwrap(), std::process::ExitCode::SUCCESS);
        assert!(
            !root.join(".changeset/pre.json").exists(),
            "dry-run must not write pre.json"
        );
    }

    /// After `pre enter` -> `pre exit`, a second `pre enter` with a new tag must succeed (not the old
    /// "already in pre-release mode" error) and must carry the recorded changesets forward under the
    /// new tag rather than resetting to an empty list.
    #[test]
    fn handle_enter_after_exit_succeeds_and_keeps_changesets() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        callisto_fixtures::git::init_repo(root);
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(
            root.join(".changeset/pre.json"),
            r#"{
  "mode": "exit",
  "tag": "beta",
  "initialVersions": {
    "a": "1.0.0"
  },
  "changesets": [
    "cool-dragons-fly"
  ]
}
"#,
        )
        .unwrap();

        let global = GlobalArgs {
            format: OutputFormat::Text,
            cwd: root.to_path_buf(),
            dry_run: false,
        };

        let result = handle(PreArgs::Enter { tag: "rc".to_string() }, &global);
        assert!(result.is_ok(), "expected Ok, got: {result:?}");

        let written = std::fs::read_to_string(root.join(".changeset/pre.json")).unwrap();
        let state = callisto_format::parse_pre_json(&written).unwrap();
        assert_eq!(state.mode, callisto_format::PreMode::Pre);
        assert_eq!(state.tag, "rc");
        assert_eq!(state.changesets, vec!["cool-dragons-fly".to_string()]);
        assert_eq!(state.initial_versions["a"].raw(), "1.0.0");
    }

    /// `pre enter` while `pre.json`'s mode is already `pre` must fail with the typed
    /// `PreAlreadyActive` diagnostic, not a raw string error.
    #[test]
    fn handle_enter_while_mode_pre_returns_typed_pre_already_active() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        callisto_fixtures::git::init_repo(root);
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(
            root.join(".changeset/pre.json"),
            r#"{"mode": "pre", "tag": "beta", "initialVersions": {}, "changesets": []}"#,
        )
        .unwrap();

        let global = GlobalArgs {
            format: OutputFormat::Text,
            cwd: root.to_path_buf(),
            dry_run: false,
        };

        let result = handle(PreArgs::Enter { tag: "rc".to_string() }, &global);
        assert!(
            matches!(result, Err(crate::error::CliError::PreAlreadyActive)),
            "expected PreAlreadyActive, got: {result:?}"
        );
    }

    /// `pre exit` with no `.changeset/pre.json` on disk must fail with the typed
    /// `PreNotActive` diagnostic, not a raw I/O error.
    #[test]
    fn handle_exit_with_no_pre_json_returns_typed_pre_not_active() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        callisto_fixtures::git::init_repo(root);
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();

        let global = GlobalArgs {
            format: OutputFormat::Text,
            cwd: root.to_path_buf(),
            dry_run: false,
        };

        let result = handle(PreArgs::Exit, &global);
        assert!(
            matches!(result, Err(crate::error::CliError::PreNotActive)),
            "expected PreNotActive, got: {result:?}"
        );
    }

    /// `pre exit` must surface a `CliError::Io` (not panic) when the workspace
    /// root cannot even be canonicalized, mirroring `workspace::load_workspace`'s
    /// nonexistent-cwd contract.
    #[test]
    fn handle_exit_reports_io_error_for_a_nonexistent_cwd() {
        let global = GlobalArgs {
            format: OutputFormat::Text,
            cwd: std::path::PathBuf::from("/nonexistent/definitely-not-a-real-path-abc123"),
            dry_run: false,
        };

        match handle(PreArgs::Exit, &global) {
            Err(crate::error::CliError::Io { path, .. }) => {
                assert_eq!(path, Some(global.cwd.clone()));
            }
            other => panic!("expected CliError::Io, got: {other:?}"),
        }
    }

    /// A `git add` failure echoing an authenticated GitHub remote URL (the
    /// realistic CI shape: `https://x-access-token:TOKEN@github.com/...`)
    /// must not leak the credential into the resulting `CliError`.
    #[test]
    fn stage_pre_json_failure_redacts_credential_from_error() {
        struct LeakyGitRunner;
        impl CommandRunner for LeakyGitRunner {
            fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
                Ok(CommandOutput {
                    exit_code: Some(128),
                    stdout: String::new(),
                    stderr: "fatal: unable to access 'https://x-access-token:ghs_leaked_secret@github.com/org/repo.git/': The requested URL returned error: 403".to_string(),
                })
            }
        }

        let err = stage_pre_json(&LeakyGitRunner, Path::new("."), Path::new(".changeset/pre.json"))
            .expect_err("git add failure must surface as an Err");
        let rendered = format!("{err}");
        assert!(
            !rendered.contains("ghs_leaked_secret"),
            "credential must not survive redaction, got: {rendered}"
        );
        assert!(rendered.contains("[REDACTED]"), "got: {rendered}");
    }
}
