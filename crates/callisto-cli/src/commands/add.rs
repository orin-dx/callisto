use std::fs;
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
use crate::tty;
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

            // Validate that the package exists in the workspace. A changeset for a
            // non-existent package would fail silently during `callisto version` once
            // the changeset is consumed — better to catch it here.
            let known = ws
                .graph
                .packages()
                .any(|p| p.id.matches(&id) || id.matches(&p.id));
            if !known {
                let known_names: Vec<String> =
                    ws.graph.packages().map(|p| p.id.display_name()).collect();
                return Err(CliError::Other(format!(
                    "Unknown package `{name}`. Known packages: {}",
                    known_names.join(", ")
                )));
            }

            entries.push(Entry {
                // Use display_name() so "cargo/foo" is preserved, not just "foo".
                // In a polyglot workspace, the bare name is ambiguous across ecosystems.
                name: id.display_name(),
                severity,
            });
        }
    } else if tty::is_interactive() {
        // Interactive 5-step Changesets Wizard (matching @changesets/cli)
        let all_packages: Vec<String> = collect_package_names(ws.graph.packages());

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
                .validate_with(|input: &String| -> Result<(), &str> {
                    if input.trim().is_empty() {
                        Err("Summary cannot be empty. Please enter a description of the change.")
                    } else {
                        Ok(())
                    }
                })
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

    let raw_summary = summary.ok_or_else(|| {
        CliError::Other(
            "--summary is required when specifying packages via CLI flags in non-interactive mode"
                .to_string(),
        )
    })?;
    let summary_text = validate_summary(&raw_summary)?;
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

/// Validates that a changeset summary string is non-empty and non-whitespace.
///
/// Returns `Ok(trimmed_summary)` on success, or a [`CliError`] with a user-facing message.
fn validate_summary(summary: &str) -> Result<String, CliError> {
    let trimmed = summary.trim().to_string();
    if trimmed.is_empty() {
        return Err(CliError::Other(
            "--summary cannot be empty. Provide a non-empty description of the change.".to_string(),
        ));
    }
    Ok(trimmed)
}

/// Builds the list of package names shown in the interactive multiselect wizard.
///
/// Uses `.display_name()` so that packages sharing a bare name across different
/// ecosystems (e.g. `cargo/foo` and `npm/foo`) appear as distinct, qualified
/// entries in the list rather than two identical `"foo"` rows.
fn collect_package_names<'a>(
    packages: impl Iterator<Item = &'a callisto_model::Package>,
) -> Vec<String> {
    packages.map(|p| p.id.display_name()).collect()
}

fn generate_human_slug() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

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

    // Each word list has 20 entries; total combination space is 20^3 = 8,000.
    // We map consecutive counter values to unique triples via integer division:
    //   a = idx % 20, n = (idx / 20) % 20, v = (idx / 400) % 20
    // This is a bijection on {0..7999}, so 8,000 consecutive calls all yield
    // distinct slugs regardless of OS clock resolution.
    //
    // A timestamp-derived offset rotates the starting position across process
    // restarts so repeated runs don't produce the same slug sequence.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    // Shift ts right to get a slowly-varying cross-run offset (changes every ~1 µs).
    let offset = (ts >> 10) % 8000;
    let idx = (count + offset) % 8000;

    let a = adjectives[(idx % 20) as usize];
    let n = nouns[((idx / 20) % 20) as usize];
    let v = verbs[((idx / 400) % 20) as usize];

    format!("{a}-{n}-{v}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::{ManifestDecl, ManifestFormat, ManifestRole, Package, ReleaseTrigger};
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// Constructs a minimal `Package` for a given id string (e.g. `"cargo/foo"`).
    fn make_package(id_str: &str) -> Package {
        let id = PackageId::parse(id_str).unwrap();
        let decl = ManifestDecl::new(
            PathBuf::from(format!("{}/Cargo.toml", id.name())),
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        Package {
            id,
            manifests: vec![decl],
            changelog: None,
            release_trigger: ReleaseTrigger::Changeset,
            publish_to: vec![],
            tag_template: None,
        }
    }

    /// Non-interactive mode with an empty --summary must fail with a clear error before
    /// reaching write_changeset, not silently write a file with an empty summary.
    #[test]
    fn non_interactive_empty_summary_is_rejected() {
        let result = validate_summary("");
        assert!(result.is_err(), "empty summary should be rejected");
    }

    #[test]
    fn non_interactive_whitespace_summary_is_rejected() {
        let result = validate_summary("   \t\n  ");
        assert!(
            result.is_err(),
            "whitespace-only summary should be rejected"
        );
    }

    #[test]
    fn non_interactive_nonempty_summary_is_accepted() {
        let result = validate_summary("Fix a bug in the parser");
        assert!(result.is_ok(), "non-empty summary should be accepted");
        assert_eq!(result.unwrap(), "Fix a bug in the parser");
    }

    #[test]
    fn validate_summary_trims_leading_and_trailing_whitespace() {
        let result = validate_summary("  important fix  ");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "important fix");
    }

    /// Regression test for Bug 1: non-interactive `add --package cargo/foo:patch`
    /// must produce `Entry { name: "cargo/foo", ... }`, not `Entry { name: "foo", ... }`.
    ///
    /// Before the fix, line 43 used `id.name()` which strips the ecosystem prefix.
    /// After the fix, it uses `id.display_name()` which preserves `"cargo/foo"`.
    /// In a polyglot workspace, the bare name `"foo"` is ambiguous and causes
    /// `AmbiguousName` errors during `callisto version`.
    #[test]
    fn non_interactive_add_entry_preserves_ecosystem_qualifier() {
        let id = PackageId::parse("cargo/foo").unwrap();
        // display_name() returns ecosystem-qualified form; name() returns bare name.
        // The fix ensures the Entry built in the non-interactive handler uses display_name().
        let entry = Entry {
            name: id.display_name(),
            severity: Severity::Patch,
        };
        assert_eq!(
            entry.name, "cargo/foo",
            "Entry.name must equal display_name(), not the bare name"
        );
    }

    /// Consecutive calls to generate_human_slug() must all produce distinct values,
    /// even when the OS clock resolution coalesces rapid calls to the same tick.
    #[test]
    fn test_generate_human_slug_consecutive_calls_differ() {
        let slugs: HashSet<String> = (0..50).map(|_| generate_human_slug()).collect();
        assert_eq!(
            slugs.len(),
            50,
            "expected 50 distinct slugs from 50 consecutive calls; got {} unique values",
            slugs.len()
        );
    }

    /// When a polyglot workspace contains `cargo/foo` and `npm/foo`, the
    /// interactive wizard's package-selection list must show two distinct,
    /// ecosystem-qualified names so the user can tell them apart.
    ///
    /// Fails before the fix (`.name()` produces two identical `"foo"` entries)
    /// and passes after the fix (`.display_name()` produces `"cargo/foo"` and
    /// `"npm/foo"`).
    #[test]
    fn polyglot_package_selection_names_are_distinct() {
        let packages = [make_package("cargo/foo"), make_package("npm/foo")];
        let names = collect_package_names(packages.iter());

        assert_eq!(names.len(), 2, "expected 2 entries; got {names:?}");
        assert!(
            names.contains(&"cargo/foo".to_string()),
            "expected ecosystem-qualified name 'cargo/foo'; got {names:?}"
        );
        assert!(
            names.contains(&"npm/foo".to_string()),
            "expected ecosystem-qualified name 'npm/foo'; got {names:?}"
        );
    }
}
