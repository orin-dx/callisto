use std::collections::BTreeSet;

use callisto_model::{ApplyPermit, CommandRunner, Ecosystem, InitDiff, InitReport, SCHEMA_VERSION};

use crate::config::raw::RawConfig;
use crate::error::{ConfigError, GraphError};
use crate::resolver::DependencyResolver;
use crate::Workspace;

/// Options for the `init` command; controls whether detected ecosystem drift is written back to `callisto.toml`.
#[derive(Clone, Debug, Default)]
pub struct InitOptions {
    /// Non-interactive confirmation. On a first run this has no effect
    /// (scaffolding an absent `callisto.toml` is always a direct write; the
    /// CLI's own interactive confirm-or-abort prompt gates *that* decision
    /// before this function is ever called). On a re-run where drift is
    /// detected against the already-recorded workspace state, `yes` is what
    /// gates applying it: `false` reports the diff without touching any
    /// file (dry-preview), `true` writes it (docs/00-design.md §18 Q5.4
    /// mechanism 1 — "re-detects, prints a diff, and applies only with
    /// confirmation").
    ///
    /// Orthogonal to the `ApplyPermit` [`init`] takes: `yes: true` with no
    /// permit means "the user consented to applying the drift, but this is a
    /// dry run" -- the apply outcome is reported and nothing is written.
    pub yes: bool,
}

fn io_err(e: std::io::Error) -> GraphError {
    GraphError::Command(callisto_model::CommandError::Io {
        program: "fs".to_string(),
        message: e.to_string(),
    })
}

/// Ecosystems the discovered workspace currently contains, deduplicated and
/// ordered by `Ecosystem`'s own `Ord` (stable regardless of manifest-walk
/// order).
fn discovered_ecosystems<D: DependencyResolver>(graph: &D) -> BTreeSet<Ecosystem> {
    graph
        .packages()
        .flat_map(|p| p.canonical_manifests())
        .map(|m| m.ecosystem())
        .collect()
}

/// Ecosystems recorded in an existing `callisto.toml`'s `[init]` bookkeeping
/// section (§18 Q5.4 mechanism 1's reconcile baseline). Unrecognized prefix
/// strings are ignored rather than treated as a hard parse error — bookkeeping
/// drift here should never block a command whose entire job is "helpfully
/// tell the user what changed."
fn recorded_ecosystems(content: &str) -> BTreeSet<Ecosystem> {
    toml::from_str::<RawConfig>(content)
        .ok()
        .and_then(|raw| raw.init)
        .and_then(|init| init.ecosystems)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| Ecosystem::from_prefix(s))
        .collect()
}

/// Renders the sorted `[init]` ecosystems array as TOML source, e.g.
/// `["cargo", "npm"]`.
fn render_ecosystems_array(ecosystems: &BTreeSet<Ecosystem>) -> String {
    let items: Vec<String> = ecosystems
        .iter()
        .map(|e| format!("\"{}\"", e.prefix()))
        .collect();
    format!("[{}]", items.join(", "))
}

/// Writes (or rewrites) the `[init]` table's `ecosystems` key in an existing
/// `callisto.toml` document via a CST edit, so every other key — including
/// ones the user hand-edited — survives byte-for-byte (§13 invariant 21's
/// "membership changes are only ever written via an explicit, reviewable
/// flow" reasoning applies equally to this bookkeeping key).
fn set_recorded_ecosystems(
    content: &str,
    ecosystems: &BTreeSet<Ecosystem>,
) -> Result<String, GraphError> {
    let mut doc: toml_edit::DocumentMut = content.parse().map_err(|e: toml_edit::TomlError| {
        GraphError::Config(ConfigError::ParseToml {
            path: "callisto.toml".into(),
            message: e.to_string(),
        })
    })?;

    if doc.get("init").and_then(|i| i.as_table()).is_none() {
        doc["init"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let mut array = toml_edit::Array::new();
    for e in ecosystems {
        array.push(e.prefix());
    }
    doc["init"]["ecosystems"] = toml_edit::value(array);

    Ok(doc.to_string())
}

/// Scaffolds `callisto.toml` and `.changeset/`, or reconciles an existing
/// config against the workspace's currently-discovered ecosystems.
///
/// `permit` and [`InitOptions::yes`] are independent gates: `yes` answers
/// "should detected drift be applied?"; `permit` answers "may anything be
/// written at all?". `--yes --dry-run` takes the apply *branch* while
/// writing nothing -- the returned [`InitReport`] describes what would
/// happen, exactly as `version --dry-run` reports its unapplied plan.
///
/// Every computation runs under a dry run too, including the TOML
/// re-render in `set_recorded_ecosystems`, so a config that would fail to
/// parse on a real run fails the dry run instead of reporting a false
/// preview.
///
/// # Errors
///
/// `Err(GraphError::Config(...))` if `callisto.toml` can't be read/parsed,
/// or the ecosystem-list TOML edit fails. `Err` wrapping an I/O error on
/// write failures.
pub fn init<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    opts: &InitOptions,
    permit: Option<&ApplyPermit>,
) -> Result<InitReport, GraphError> {
    let config_path = ws.root.join("callisto.toml");
    let discovered = discovered_ecosystems(&ws.graph);
    let mut initialized = false;
    let mut diff = InitDiff::default();

    if !config_path.exists() {
        // First run: nothing recorded yet to diff against, so this is a
        // direct write, not a reconcile-apply.
        let content = format!(
            "# callisto configuration\n\n[changesets]\ndir = \".changeset\"\n\n\
[cascade]\nmode = \"out-of-range\"\nbump-severity = \"patch\"\npeer-escalation = true\npreserve-npm-ranges = true\n\n\
[init]\necosystems = {}\n",
            render_ecosystems_array(&discovered)
        );
        if let Some(permit) = permit {
            callisto_manifests::atomic::atomic_write(&config_path, &content, permit)
                .map_err(io_err)?;
        }
        initialized = true;
    } else {
        let existing = std::fs::read_to_string(&config_path).map_err(|e| {
            GraphError::Config(ConfigError::Read {
                path: config_path.clone(),
                message: e.to_string(),
            })
        })?;
        let recorded = recorded_ecosystems(&existing);
        let new_ecosystems: Vec<Ecosystem> = discovered.difference(&recorded).copied().collect();

        if !new_ecosystems.is_empty() {
            diff.new_ecosystems = new_ecosystems;

            if opts.yes {
                let reconciled: BTreeSet<Ecosystem> =
                    recorded.union(&discovered).copied().collect();
                let updated = set_recorded_ecosystems(&existing, &reconciled)?;
                if let Some(permit) = permit {
                    callisto_manifests::atomic::atomic_write(&config_path, &updated, permit)
                        .map_err(io_err)?;
                }
                diff.applied = true;
            }
            // else: the diff is reported above without applying it. Note this
            // is a *separate* no-write path from `permit: None`; `init`
            // without `--yes` reports drift on a real run too.
        }
    }

    let cs_dir = ws.root.join(".changeset");
    if let Some(permit) = permit {
        if !cs_dir.exists() {
            std::fs::create_dir_all(&cs_dir).map_err(io_err)?;
        }
        let readme_path = cs_dir.join("README.md");
        if !readme_path.exists() {
            let readme_content = r#"# Changesets

This directory contains markdown changeset files generated by Callisto CLI (`callisto add`).
Each changeset describes a package version bump and associated release summary.
"#;
            callisto_manifests::atomic::atomic_write(&readme_path, readme_content, permit)
                .map_err(io_err)?;
        }
    }

    Ok(InitReport {
        schema_version: SCHEMA_VERSION,
        initialized,
        config_path,
        diff,
        diagnostics: Vec::new(),
    })
}
