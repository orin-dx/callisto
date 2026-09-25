use std::collections::BTreeMap;

use callisto_model::{
    CommandRunner, Diagnostic, DiagnosticCode, DiagnosticSeverity, Package, PackageId, Severity, StatusReport,
    SCHEMA_VERSION,
};

use crate::aggregate::{resolve_target_package, LoadedChangeset};
use crate::changed::changed_since_last_tag;
use crate::commands::escalate;
use crate::commands::version::{plan_version, VersionOptions};
use crate::error::GraphError;
use crate::infer::SeverityInference;
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct StatusOptions {
    pub strict: bool,
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

pub fn status<R: CommandRunner, D: DependencyResolver, I: SeverityInference>(
    ws: &Workspace<'_, R, D>,
    inference: &I,
    opts: &StatusOptions,
) -> Result<StatusReport, GraphError> {
    let mut packages = Vec::new();
    let base_versions = ws.base_versions()?;
    let loaded_changesets = crate::load_changesets(&ws.root, &ws.config)?;
    let tags = ws.tags()?;

    let all_packages: Vec<&Package> = ws.graph.packages().collect();
    let pending = resolve_pending_changesets(all_packages.iter().copied(), &loaded_changesets)?;

    // AC-01 (SPEC-DX-STATUS-ADD): pending severity is `plan_version`'s own
    // `PlannedBump.severity` per package -- the same cascade/fixed/linked-group
    // computation `callisto version` uses. status computes no cascade of its
    // own; it only reads the plan `plan_version` already derives.
    let version_opts = VersionOptions {
        strict: opts.strict,
        allow_empty_changesets: true,
    };
    let plan = plan_version(ws, inference, &version_opts)?;
    let planned_severity: BTreeMap<PackageId, Severity> =
        plan.bumps.iter().map(|b| (b.package.clone(), b.severity)).collect();

    let changed = changed_since_last_tag(&all_packages, tags, ws.git_access())?;

    for pkg in all_packages.iter().copied() {
        let current_version = base_versions.get(&pkg.id).cloned().ok_or_else(|| {
            GraphError::Manifest(callisto_model::ManifestError::MissingField {
                path: pkg.manifests.first().map(|m| m.path.clone()).unwrap_or_default(),
                field: "version",
            })
        })?;
        let last = tags.last_tag(&pkg.id);
        let last_tag = last.map(|t| t.name.clone());
        let last_released_version = last.map(|t| t.version.clone());

        let (pkg_changesets, _) = pending.get(&pkg.id).cloned().unwrap_or_default();
        let pending_severity = planned_severity.get(&pkg.id).copied();

        packages.push(callisto_model::StatusPackageRecord {
            package: pkg.id.clone(),
            current_version,
            last_tag,
            last_released_version,
            pending_severity,
            changed_since_last_tag: changed[&pkg.id],
            release_trigger: pkg.release_trigger,
            pending_changesets: pkg_changesets,
        });
    }

    let mut diagnostics = ws.graph.diagnostics().to_vec();
    // AC-03: fold in every well-formedness diagnostic `validate` used to
    // report (EmptyChangeset, EmptySummary, UnknownPackage,
    // AmbiguousPackageName, InvalidPackageName) now that `validate` is gone.
    diagnostics.extend(changeset_wellformedness_diagnostics(
        all_packages.iter().copied(),
        &loaded_changesets,
    ));
    escalate(&mut diagnostics, opts.strict);

    let has_changesets = packages.iter().any(|p| !p.pending_changesets.is_empty());
    // AC-04: count of packages with a planned bump, post-cascade/fixed/linked-group --
    // the field a script now reads to detect pending changesets, since `--check`'s
    // exit code no longer signals it.
    let pending = packages.iter().filter(|p| p.pending_severity.is_some()).count() as u32;

    Ok(StatusReport {
        schema_version: SCHEMA_VERSION,
        has_changesets,
        pending,
        packages,
        diagnostics,
    })
}

/// The per-changeset well-formedness diagnostics `validate` used to emit
/// (crates/callisto-graph/src/commands/validate.rs, now removed -- SPEC-DX-STATUS-ADD
/// AC-03), ported verbatim: entries/summary shape and package-name resolution,
/// all at Error severity. Uses `resolve_unique` (soft: returns candidates on
/// ambiguity) rather than `resolve_target_package` (hard-errors via `?`) --
/// unlike `resolve_pending_changesets` above, this must never abort the scan.
fn changeset_wellformedness_diagnostics<'a>(
    packages: impl Iterator<Item = &'a Package> + Clone,
    loaded: &[LoadedChangeset],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for cs in loaded {
        if cs.changeset.entries.is_empty() {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::EmptyChangeset,
                severity: DiagnosticSeverity::Error,
                message: format!("Changeset `{}` is empty", cs.path.display()),
                package: None,
                path: Some(cs.path.clone()),
                governed_by: None,
                escalated_by: None,
            });
        }

        if !cs.changeset.entries.is_empty() && cs.changeset.summary.trim().is_empty() {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::EmptySummary,
                severity: DiagnosticSeverity::Error,
                message: format!("Changeset `{}` has entries but an empty summary", cs.path.display()),
                package: None,
                path: Some(cs.path.clone()),
                governed_by: None,
                escalated_by: None,
            });
        }

        for entry in &cs.changeset.entries {
            match PackageId::parse(&entry.name) {
                Ok(id) => match id.resolve_unique(packages.clone(), |p| &p.id) {
                    Ok(None) => {
                        diagnostics.push(Diagnostic {
                            code: DiagnosticCode::UnknownPackage,
                            severity: DiagnosticSeverity::Error,
                            message: format!(
                                "Changeset `{}` references unknown package `{}`",
                                cs.path.display(),
                                entry.name
                            ),
                            package: Some(id),
                            path: Some(cs.path.clone()),
                            governed_by: None,
                            escalated_by: None,
                        });
                    }
                    Ok(Some(_)) => {}
                    Err(candidates) => {
                        let names: Vec<String> = candidates.iter().map(|p| p.id.display_name().to_string()).collect();
                        diagnostics.push(Diagnostic {
                            code: DiagnosticCode::AmbiguousPackageName,
                            severity: DiagnosticSeverity::Error,
                            message: format!(
                                "Changeset `{}` references ambiguous package `{}` (matches: {})",
                                cs.path.display(),
                                entry.name,
                                names.join(", ")
                            ),
                            package: Some(id),
                            path: Some(cs.path.clone()),
                            governed_by: None,
                            escalated_by: None,
                        });
                    }
                },
                Err(_) => {
                    diagnostics.push(Diagnostic {
                        code: DiagnosticCode::InvalidPackageName,
                        severity: DiagnosticSeverity::Error,
                        message: format!(
                            "Changeset `{}` contains invalid package name `{}`",
                            cs.path.display(),
                            entry.name
                        ),
                        package: None,
                        path: Some(cs.path.clone()),
                        governed_by: None,
                        escalated_by: None,
                    });
                }
            }
        }
    }
    diagnostics
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
            last_released_version: None,
            pending_severity: Some(callisto_model::Severity::Minor),
            changed_since_last_tag: false,
            release_trigger: ReleaseTrigger::Changeset,
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

        let (names, sev) = pending.get(&cargo_foo.id).expect("package should be present");
        assert_eq!(names, &vec!["foo-changeset".to_string()]);
        assert_eq!(sev, &Some(Severity::Major));
    }
}
