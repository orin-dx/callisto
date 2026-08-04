use std::fs;
use std::process::ExitCode;

use callisto_format::{parse_pre_json, write_pre_json, PreMode, PreState};
use callisto_model::{ApplyPermit, SCHEMA_VERSION};
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
            let ws = load_workspace(global, &runner)?;
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

            let pre_dir = ws.root.join(".changeset");
            fs::create_dir_all(&pre_dir)?;
            callisto_manifests::atomic::atomic_write(&pre_dir.join("pre.json"), &text, &permit)?;

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
            let start = global.cwd.canonicalize().map_err(|source| CliError::Io {
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
