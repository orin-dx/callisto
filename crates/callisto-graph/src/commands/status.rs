use std::collections::BTreeMap;

use callisto_model::{CommandRunner, Package, PackageId, Severity, StatusReport, SCHEMA_VERSION};
use callisto_vcs::GitAccess;

use crate::aggregate::{resolve_target_package, LoadedChangeset};
use crate::changed::changed_since_last_tag;
use crate::commands::escalate;
use crate::error::GraphError;
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct StatusOptions {
    pub strict: bool,
    pub strict_graph: bool,
}

/// Per-package accumulator: pending changeset names and their max severity.
type PendingChangesets = BTreeMap<PackageId, (Vec<String>, Option<Severity>)>;

/// Resolves each changeset entry to at most one package and accumulates the
/// pending changeset names and max severity per package.
///
/// Uses [`resolve_target_package`] (the same ambiguity-checking resolution
/// `aggregate()` relies on) rather than a bare `matches()` loop, so a
/// changeset entry naming a bare package that exists in two or more
/// ecosystems (e.g. `cargo/foo` and `npm/foo`) errors instead of silently
/// attaching the changeset to every matching package.
fn resolve_pending_changesets<'a>(
    packages: impl Iterator<Item = &'a Package> + Clone,
    loaded_changesets: &[LoadedChangeset],
) -> Result<PendingChangesets, GraphError> {
    let mut pending: PendingChangesets = BTreeMap::new();
    for lc in loaded_changesets {
        for entry in &lc.changeset.entries {
            let Ok(entry_id) = PackageId::parse(&entry.name) else {
                continue;
            };
            let Some(resolved) = resolve_target_package(packages.clone(), &entry_id)? else {
                continue;
            };
            let name = lc
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let state = pending.entry(resolved.id.clone()).or_default();
            state.0.push(name);
            state.1 = match (state.1, entry.severity) {
                (None, s) => Some(s),
                (Some(cur), s) => Some(cur.max(s)),
            };
        }
    }
    Ok(pending)
}

pub fn status<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    opts: &StatusOptions,
) -> Result<StatusReport, GraphError> {
    let mut packages = Vec::new();
    let base_versions = ws.base_versions()?;
    let loaded_changesets = crate::load_changesets(&ws.root, &ws.config)?;
    let tags = ws.tags()?;

    let all_packages: Vec<&Package> = ws.graph.packages().collect();
    let pending = resolve_pending_changesets(all_packages.iter().copied(), &loaded_changesets)?;

    // Built once and shared across every package below -- discovering the
    // repository fresh per package (as `changed_since_last_tag` used to do
    // internally) is N redundant discoveries of the exact same repository
    // for an N-package workspace.
    let git = GitAccess::discover(&ws.root, ws.runner);

    for pkg in all_packages.iter().copied() {
        let current_version = base_versions.get(&pkg.id).cloned().ok_or_else(|| {
            GraphError::Manifest(callisto_model::ManifestError::MissingField {
                path: pkg
                    .manifests
                    .first()
                    .map(|m| m.path.clone())
                    .unwrap_or_default(),
                field: "version",
            })
        })?;
        let last_tag = tags.last_tag(&pkg.id).map(|t| t.name.clone());
        let changed = changed_since_last_tag(ws.runner, &ws.root, pkg, tags, &git)?;

        let (pkg_changesets, max_sev) = pending.get(&pkg.id).cloned().unwrap_or_default();

        packages.push(callisto_model::StatusPackageRecord {
            package: pkg.id.clone(),
            current_version,
            last_tag,
            pending_severity: max_sev,
            changed_since_last_tag: changed,
            pending_changesets: pkg_changesets,
        });
    }

    let mut diagnostics = ws.graph.diagnostics().to_vec();
    escalate(&mut diagnostics, opts.strict, opts.strict_graph);

    Ok(StatusReport {
        schema_version: SCHEMA_VERSION,
        packages,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use callisto_format::{Changeset, Entry};
    use callisto_model::{Ecosystem, ReleaseTrigger, Severity};

    use super::*;

    #[test]
    fn test_status_package_record_structure() {
        let rec = callisto_model::StatusPackageRecord {
            package: callisto_model::PackageId::parse("test-pkg").unwrap(),
            current_version: callisto_model::Version::semver(1, 0, 0),
            last_tag: None,
            pending_severity: Some(callisto_model::Severity::Minor),
            changed_since_last_tag: false,
            pending_changesets: vec!["my-changeset".to_string()],
        };
        assert_eq!(rec.pending_changesets.len(), 1);
        assert_eq!(rec.pending_severity, Some(callisto_model::Severity::Minor));
    }

    fn make_package(ecosystem: Ecosystem, name: &str) -> Package {
        Package {
            id: PackageId::Prefixed {
                ecosystem,
                name: name.to_string(),
            },
            manifests: Vec::new(),
            changelog: None,
            release_trigger: ReleaseTrigger::Changeset,
            publish_to: Vec::new(),
            tag_template: None,
        }
    }

    fn make_loaded_changeset(name: &str, severity: Severity) -> LoadedChangeset {
        LoadedChangeset {
            path: PathBuf::from(format!("{name}-changeset.md")),
            id: format!("{name}-changeset"),
            changeset: Changeset {
                entries: vec![Entry {
                    name: name.to_string(),
                    severity,
                }],
                summary: "test summary".to_string(),
            },
        }
    }

    /// Spec: a bare changeset entry name that exists in two or more
    /// ecosystems (e.g. `cargo/foo` and `npm/foo`) must error instead of
    /// silently attaching the changeset to every matching package. Before
    /// the fix, `status()` used `pkg.id.matches(&entry_id)` in a per-package
    /// loop, which attached the changeset to *both* packages with no
    /// indication the reference was ambiguous.
    #[test]
    fn test_resolve_pending_changesets_ambiguous_bare_name_errors() {
        let cargo_foo = make_package(Ecosystem::Cargo, "foo");
        let npm_foo = make_package(Ecosystem::Npm, "foo");
        let packages = vec![&cargo_foo, &npm_foo];

        let loaded = vec![make_loaded_changeset("foo", Severity::Minor)];

        let result = resolve_pending_changesets(packages.into_iter(), &loaded);

        match result {
            Err(GraphError::AmbiguousName { name, candidates }) => {
                assert_eq!(name, "foo");
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected GraphError::AmbiguousName, got {other:?}"),
        }
    }

    /// An unambiguous bare-name changeset entry still resolves to the single
    /// matching package.
    #[test]
    fn test_resolve_pending_changesets_unambiguous_bare_name_resolves() {
        let cargo_foo = make_package(Ecosystem::Cargo, "foo");
        let packages = vec![&cargo_foo];

        let loaded = vec![make_loaded_changeset("foo", Severity::Major)];

        let pending = resolve_pending_changesets(packages.into_iter(), &loaded).unwrap();

        let (names, sev) = pending
            .get(&cargo_foo.id)
            .expect("package should be present");
        assert_eq!(names, &vec!["foo-changeset".to_string()]);
        assert_eq!(sev, &Some(Severity::Major));
    }
}
