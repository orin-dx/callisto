use std::fs;
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
                return Err(CliError::Other(
                    "Pre-release tag cannot be empty".to_string(),
                ));
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
            let output = runner
                .run("git", &["add", PRE_JSON_REL], &ws.root)
                .map_err(|e| CliError::Other(format!("git add failed: {e}")))?;
            if !output.success() {
                return Err(CliError::Other(format!(
                    "git add .changeset/pre.json failed (exit {:?}): {}",
                    output.exit_code, output.stderr
                )));
            }

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
