use std::fs;
use std::process::ExitCode;

use callisto_format::{Changeset, Entry};
use callisto_model::{PackageId, Severity, SCHEMA_VERSION};
use serde_json::json;

use crate::cli::{AddArgs, GlobalArgs, OutputFormat};
use crate::error::CliError;
use crate::output::{log_line, write_json};
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: AddArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    if args.packages.is_empty() {
        return Err(CliError::NotATty);
    }

    let summary = args
        .summary
        .unwrap_or_else(|| "Updated package.".to_string());
    let mut entries = Vec::new();

    for pkg_str in args.packages {
        let (name, sev_str) = pkg_str
            .split_once(':')
            .ok_or_else(|| CliError::Other(format!("Invalid package spec `{pkg_str}`")))?;

        let severity: Severity = sev_str
            .parse()
            .map_err(|_| CliError::Other(format!("Invalid severity `{sev_str}`")))?;

        let id = PackageId::parse(name)
            .map_err(|e| CliError::Other(format!("Invalid package name `{name}`: {e}")))?;

        entries.push(Entry {
            name: id.name().to_string(),
            severity,
        });
    }

    let changeset = Changeset { entries, summary };
    let text = callisto_format::write_changeset(&changeset)?;

    let changeset_dir = ws.root.join(".changeset");
    fs::create_dir_all(&changeset_dir)?;

    let filename = "changeset-add.md";
    let rel_path = format!(".changeset/{filename}");
    fs::write(changeset_dir.join(filename), text)?;

    match global.format {
        OutputFormat::Json => {
            let env = json!({
                "schemaVersion": SCHEMA_VERSION,
                "command": "add",
                "path": rel_path
            });
            write_json(&mut std::io::stdout(), &env)?;
        }
        OutputFormat::Text => {
            log_line(global.format, &format!("Added changeset: {rel_path}"));
        }
    }

    Ok(ExitCode::SUCCESS)
}
