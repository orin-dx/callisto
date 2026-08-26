use std::fs;
use std::path::Path;
use std::process::ExitCode;

use callisto_format::{parse_pre_json, write_pre_json, PreMode, PreState};
use callisto_model::{ApplyPermit, CommandRunner, SCHEMA_VERSION};
use serde_json::json;

use crate::cli::{GlobalArgs, OutputFormat, PreArgs};
use crate::error::CliError;
use crate::output::{log_line, write_json};
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

/// Relative path reported in dry-run previews, so the output names the file
/// the user would find rather than an absolute path.
const PRE_JSON_REL: &str = ".changeset/pre.json";

/// Redacts known registry/VCS credential env-var values and any URL
/// userinfo component from raw `git` subprocess stderr before it is
/// embedded in a [`CliError`] -- a failing `git` invocation can surface an
/// authenticated remote URL (e.g. GitHub Actions'
/// `https://x-access-token:TOKEN@github.com/...`) verbatim in its own
/// error output, and that text flows into `--format json` output downstream.
fn redact_git_stderr(text: &str) -> String {
    callisto_model::redact_known_secrets(text, &callisto_model::known_credential_env_values(std::env::vars()))
}

/// Stages `.changeset/pre.json` via `git add`, called by `pre enter` on a
/// real (non-dry-run) write so the new file is included in the next commit.
/// Extracted from [`handle`] so it's directly testable with a fake
/// [`CommandRunner`] -- `handle` itself always constructs a real
/// [`crate::runner::CliCommandRunner`], which shells out for real.
fn stage_pre_json(runner: &dyn CommandRunner, root: &Path) -> Result<(), CliError> {
    let output = runner
        .run("git", &["add", PRE_JSON_REL], root)
        .map_err(|e| CliError::Other(format!("git add failed: {e}")))?;
    if !output.success() {
        return Err(CliError::Other(format!(
            "git add .changeset/pre.json failed (exit {:?}): {}",
            output.exit_code,
            redact_git_stderr(&output.stderr)
        )));
    }
    Ok(())
}

/// Reports the `.changeset/pre.json` content a real run would have written,
/// mirroring `add`'s dry-run preview in both output formats.
fn preview(global: &GlobalArgs, mode: &str, tag: &str, content: &str) -> Result<(), CliError> {
    match global.format {
        OutputFormat::Json => {
            let env = json!({
                "schemaVersion": SCHEMA_VERSION,
                "command": "pre",
                "dryRun": true,
                "mode": mode,
                "tag": tag,
                "path": PRE_JSON_REL,
                "content": content
            });
            write_json(&mut std::io::stdout(), &env)?;
        }
        OutputFormat::Text => {
            println!("[DRY-RUN] Would write {PRE_JSON_REL} (no files written)\n\n{content}");
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
/// Returns [`CliError::Io`] if reading the existing `pre.json` fails (`exit`
/// subcommand), or if any write fails (`enter` subcommand on a real run).
pub fn handle(args: PreArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let permit = ApplyPermit::granted_unless_dry_run(global.dry_run);

    match args {
        PreArgs::Enter { tag } => {
            // Bug 3: reject empty tags before any workspace I/O.
            if tag.trim().is_empty() {
                return Err(CliError::Other("Pre-release tag cannot be empty".to_string()));
            }

            let ws = load_workspace(global, &runner)?;

            // Bug 1: reject re-entering pre mode when already active.
            // The check runs for both real and dry-run paths so a dry-run
            // never silently simulates overwriting live pre-release state.
            let pre_dir = ws.root.join(".changeset");
            let pre_path = pre_dir.join("pre.json");
            if pre_path.exists() {
                return Err(CliError::Other(
                    "Workspace is already in pre-release mode. Run `callisto pre exit` first, \
                     or delete .changeset/pre.json manually to reset."
                        .to_string(),
                ));
            }

            let initial = ws.initial_versions()?;
            let mut snapshot = indexmap::IndexMap::new();
            for (k, v) in initial {
                snapshot.insert(k, v);
            }

            let pre_state = PreState::entering(tag.clone(), snapshot);
            let text = write_pre_json(&pre_state);

            let Some(permit) = permit else {
                preview(global, "pre", &tag, &text)?;
                return Ok(ExitCode::SUCCESS);
            };

            fs::create_dir_all(&pre_dir)?;
            callisto_manifests::atomic::atomic_write(&pre_path, &text, &permit)?;

            // Bug 4: stage pre.json so it is included in the next commit.
            stage_pre_json(&runner, &ws.root)?;

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
            let root = callisto_graph::locate::find_workspace_root(&start)?;
            let pre_path = root.join(PRE_JSON_REL);

            let text = fs::read_to_string(&pre_path).map_err(|source| CliError::Io {
                source,
                path: Some(pre_path.clone()),
            })?;
            let mut pre_state = parse_pre_json(&text)?;

            // Bug 2: reject double-exit.
            if pre_state.mode == PreMode::Exit {
                return Err(CliError::Other(
                    "Workspace is not in pre-release mode (already exited). \
                     Run `callisto version` to finalize the release."
                        .to_string(),
                ));
            }

            pre_state.mode = PreMode::Exit;

            let updated = write_pre_json(&pre_state);

            let Some(permit) = permit else {
                preview(global, "exit", &pre_state.tag, &updated)?;
                return Ok(ExitCode::SUCCESS);
            };

            callisto_manifests::atomic::atomic_write(&pre_path, &updated, &permit)?;

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

        let err = stage_pre_json(&LeakyGitRunner, Path::new(".")).expect_err("git add failure must surface as an Err");
        let rendered = format!("{err}");
        assert!(
            !rendered.contains("ghs_leaked_secret"),
            "credential must not survive redaction, got: {rendered}"
        );
        assert!(rendered.contains("[REDACTED]"), "got: {rendered}");
    }
}
