use std::fs;
use std::io::IsTerminal;
use std::process::ExitCode;

use callisto_format::{Changeset, Entry};
use callisto_graph::DependencyResolver;
use callisto_model::{ApplyPermit, PackageId, Severity, SCHEMA_VERSION};
use dialoguer::{Confirm, Input, MultiSelect};
use serde_json::json;

use crate::cli::{AddArgs, GlobalArgs, OutputFormat};
use crate::error::CliError;
use crate::output::{log_line, write_json};
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: AddArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let mut entries = Vec::new();
    let mut summary = args.summary.clone();

    if !args.packages.is_empty() {
        // Non-interactive mode (flags supplied via CLI or agent)
        for pkg_str in args.packages {
            let (name, sev_str) = pkg_str.rsplit_once(':').ok_or_else(|| {
                CliError::Other(format!(
                    "Invalid package spec `{pkg_str}`. Expected format: `package-name:severity`"
                ))
            })?;

            let severity: Severity = sev_str.parse().map_err(|_err| {
                CliError::Other(format!(
                    "Invalid severity `{sev_str}`. Must be patch, minor, or major."
                ))
            })?;

            let id = PackageId::parse(name)
                .map_err(|e| CliError::Other(format!("Invalid package name `{name}`: {e}")))?;

            entries.push(Entry {
                name: id.name().to_string(),
                severity,
            });
        }
    } else if std::io::stdin().is_terminal() {
        // Interactive 5-step Changesets Wizard (matching @changesets/cli)
        let all_packages: Vec<String> = ws
            .graph
            .packages()
            .map(|p| p.id.name().to_string())
            .collect();

        if all_packages.is_empty() {
            return Err(CliError::Other(
                "No packages found in workspace.".to_string(),
            ));
        }

        // Step 1: Package Selection
        println!("Which packages would you like to include in this changeset?");
        let selected_indices = MultiSelect::new()
            .items(&all_packages)
            .interact()
            .map_err(|e| CliError::Other(format!("Interactive selection failed: {e}")))?;

        if selected_indices.is_empty() {
            return Err(CliError::Other(
                "No packages selected for changeset.".to_string(),
            ));
        }

        let selected_packages: Vec<String> = selected_indices
            .into_iter()
            .map(|i| all_packages[i].clone())
            .collect();

        // Step 2: Major Bump Selection
        println!("\nWhich of these packages should be a MAJOR bump?");
        println!("(Select none if there are no breaking changes)");
        let major_indices = MultiSelect::new()
            .items(&selected_packages)
            .interact()
            .map_err(|e| CliError::Other(format!("Interactive selection failed: {e}")))?;

        let major_set: std::collections::HashSet<usize> = major_indices.into_iter().collect();

        // Step 3: Minor Bump Selection (excluding major packages)
        let minor_candidates: Vec<String> = selected_packages
            .iter()
            .enumerate()
            .filter(|(idx, _)| !major_set.contains(idx))
            .map(|(_, name)| name.clone())
            .collect();

        let minor_indices = if !minor_candidates.is_empty() {
            println!("\nWhich of these packages should be a MINOR bump?");
            println!("(Any remaining packages will default to a PATCH bump)");
            MultiSelect::new()
                .items(&minor_candidates)
                .interact()
                .map_err(|e| CliError::Other(format!("Interactive selection failed: {e}")))?
        } else {
            Vec::new()
        };

        let minor_set: std::collections::HashSet<String> = minor_indices
            .into_iter()
            .map(|idx| minor_candidates[idx].clone())
            .collect();

        // Build entries with assigned severities
        for (idx, pkg_name) in selected_packages.iter().enumerate() {
            let severity = if major_set.contains(&idx) {
                Severity::Major
            } else if minor_set.contains(pkg_name) {
                Severity::Minor
            } else {
                Severity::Patch
            };

            entries.push(Entry {
                name: pkg_name.clone(),
                severity,
            });
        }

        // Step 4: Summary Entry
        if summary.is_none() {
            println!("\nPlease enter a summary for this change:");
            let input_summary: String = Input::new()
                .interact_text()
                .map_err(|e| CliError::Other(format!("Interactive prompt failed: {e}")))?;
            summary = Some(input_summary);
        }

        // Step 5: Confirmation Preview
        let summary_text = summary.as_deref().unwrap_or("Updated package.");
        let temp_changeset = Changeset {
            entries: entries.clone(),
            summary: summary_text.to_string(),
        };

        let preview_text = callisto_format::write_changeset(&temp_changeset)?;
        println!("\n=== Changeset Preview ===\n{preview_text}");

        let confirm = Confirm::new()
            .with_prompt("Is this your desired changeset?")
            .default(true)
            .interact()
            .map_err(|e| CliError::Other(format!("Interactive confirmation failed: {e}")))?;

        if !confirm {
            println!("Changeset creation cancelled.");
            return Ok(ExitCode::SUCCESS);
        }
    } else {
        return Err(CliError::NotATty);
    }

    let summary_text = summary.ok_or_else(|| {
        CliError::Other(
            "--summary is required when specifying packages via CLI flags in non-interactive mode"
                .to_string(),
        )
    })?;
    let changeset = Changeset {
        entries,
        summary: summary_text,
    };
    let text = callisto_format::write_changeset(&changeset)?;

    let changeset_dir = ws.root.join(".changeset");
    let slug = generate_human_slug();
    let filename = format!("{slug}.md");
    let rel_path = format!(".changeset/{filename}");

    let Some(permit) = ApplyPermit::granted_unless_dry_run(global.dry_run) else {
        // Compute what WOULD be written, but never touch disk.
        match global.format {
            OutputFormat::Json => {
                let env = json!({
                    "schemaVersion": SCHEMA_VERSION,
                    "command": "add",
                    "dryRun": true,
                    "path": rel_path,
                    "content": text
                });
                write_json(&mut std::io::stdout(), &env)?;
            }
            OutputFormat::Text => {
                println!("[DRY-RUN] Would add changeset: {rel_path} (no files written)\n\n{text}");
            }
        }
        return Ok(ExitCode::SUCCESS);
    };

    let target_file = changeset_dir.join(&filename);
    fs::create_dir_all(&changeset_dir)?;
    callisto_manifests::atomic::atomic_write(&target_file, &text, &permit)?;

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

fn generate_human_slug() -> String {
    let adjectives = [
        "swift", "clever", "bright", "silent", "brave", "calm", "eager", "gentle", "happy",
        "jolly", "keen", "lively", "mighty", "noble", "proud", "quick", "radiant", "sharp",
        "tough", "vivid",
    ];
    let nouns = [
        "foxes", "hawks", "wolves", "eagles", "bears", "lions", "otters", "pandas", "falcons",
        "tigers", "dolphins", "panthers", "cheetahs", "leopards", "ravens", "badgers", "lynxes",
        "cobras", "owls", "stags",
    ];
    let verbs = [
        "run", "fly", "leap", "soar", "dash", "hunt", "glide", "climb", "swim", "roar", "pounce",
        "sprint", "chase", "race", "bound", "drift", "jump", "strut", "spin", "surge",
    ];

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let a = adjectives[(timestamp as usize) % adjectives.len()];
    let n = nouns[((timestamp >> 4) as usize) % nouns.len()];
    let v = verbs[((timestamp >> 8) as usize) % verbs.len()];

    format!("{a}-{n}-{v}")
}
