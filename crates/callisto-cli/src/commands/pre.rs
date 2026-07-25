use std::fs;
use std::process::ExitCode;

use callisto_format::{parse_pre_json, write_pre_json, PreMode, PreState};
use callisto_model::SCHEMA_VERSION;
use serde_json::json;

use crate::cli::{GlobalArgs, OutputFormat, PreArgs};
use crate::error::CliError;
use crate::output::{log_line, write_json};
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: PreArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;

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

            let pre_dir = ws.root.join(".changeset");
            fs::create_dir_all(&pre_dir)?;
            fs::write(pre_dir.join("pre.json"), text)?;

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
            let start = global.cwd.canonicalize().map_err(CliError::Io)?;
            let root = callisto_graph::locate::find_workspace_root(&start)?;
            let pre_path = root.join(".changeset/pre.json");

            let text = fs::read_to_string(&pre_path)?;
            let mut pre_state = parse_pre_json(&text)?;
            pre_state.mode = PreMode::Exit;

            let updated = write_pre_json(&pre_state);
            fs::write(&pre_path, updated)?;

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
