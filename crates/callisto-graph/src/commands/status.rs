use std::collections::BTreeMap;

use callisto_model::{
    BumpReason, CommandRunner, Diagnostic, DiagnosticCode, DiagnosticSeverity, Package, PackageId, Severity,
    StatusReport, SCHEMA_VERSION,
};

use crate::aggregate::{resolve_target_package, LoadedChangeset};
use crate::changed::changed_since_last_tag;
use crate::commands::escalate;
use crate::commands::version::{plan_version, VersionOptions};
use crate::error::GraphError;
use crate::identity::IdentityIndex;
use crate::infer::SeverityInference;
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct StatusOptions {
    pub strict: bool,
}

/// Per-package accumulator: pending changeset names and their max severity.
type PendingChangesets = BTreeMap<PackageId, (Vec<String>, Option<Severity>)>;

/// Resolves each changeset entry to at most one package through the identity index -- the
/// resolver `callisto version` uses -- and accumulates the pending changeset names and max
/// severity per package.
///
/// An unknown or ambiguous entry is skipped here rather than hard-erroring:
/// `changeset_wellformedness_diagnostics` reports both cases as an Error-severity diagnostic.
fn resolve_pending_changesets<'a>(
    packages: impl Iterator<Item = &'a Package> + Clone,
    identity: &IdentityIndex,
    loaded_changesets: &[LoadedChangeset],
) -> Result<PendingChangesets, GraphError> {
    let mut pending: PendingChangesets = BTreeMap::new();
    for lc in loaded_changesets {
        for entry in &lc.changeset.entries {
            let Ok(entry_id) = PackageId::parse(&entry.name) else {
                continue;
            };
            let resolved = match resolve_target_package(packages.clone(), identity, &entry_id) {
                Ok(Some(resolved)) => resolved,
                Ok(None) | Err(GraphError::AmbiguousName { .. }) => continue,
                Err(e) => return Err(e),
            };
            let name = lc
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let state = pending.entry(resolved.id.clone()).or_default();
            if state.0.last() != Some(&name) {
                state.0.push(name);
            }
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
    let (loaded_changesets, changeset_load_failures) = crate::load_changesets_permissive(&ws.root, &ws.config)?;
    let tags = ws.tags()?;

    let all_packages: Vec<&Package> = ws.graph.packages().collect();
    let pending = resolve_pending_changesets(all_packages.iter().copied(), &ws.identity, &loaded_changesets)?;

    // Pending severity is `plan_version`'s own
    // `PlannedBump.severity` per package -- the same cascade/fixed/linked-group
    // computation `callisto version` uses. status computes no cascade of its
    // own; it only reads the plan `plan_version` already derives.
    let version_opts = VersionOptions {
        strict: opts.strict,
        allow_empty_changesets: true,
    };
    // An ambiguous changeset entry, or an unparseable changeset file, makes `plan_version`
    // (via `aggregate`'s own strict `load_changesets`) hard-error -- the right behavior for a
    // real `version`/`release` run, but `status` must survive both and report
    // `AmbiguousPackageName`/the parse failure diagnostic below instead. Severity planning is
    // skipped for this run rather than duplicating `aggregate`'s own resolution to route
    // around its error.
    let plan = match plan_version(ws, inference, &version_opts) {
        Ok(plan) => Some(plan),
        Err(GraphError::AmbiguousName { .. } | GraphError::ParseChangeset { .. }) => None,
        Err(e) => return Err(e),
    };
    let planned_severity: BTreeMap<PackageId, Severity> = plan
        .iter()
        .flat_map(|plan| &plan.bumps)
        .map(|b| (b.package.clone(), b.severity))
        .collect();

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
    diagnostics.extend(tags.diagnostics.iter().cloned());
    // Fold in every well-formedness diagnostic `validate` used to
    // report (EmptyChangeset, UnknownPackage, AmbiguousPackageName,
    // InvalidPackageName) now that `validate` is gone.
    diagnostics.extend(changeset_wellformedness_diagnostics(
        all_packages.iter().copied(),
        &ws.identity,
        &loaded_changesets,
    )?);
    for failure in &changeset_load_failures {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::ChangesetParseFailed,
            severity: DiagnosticSeverity::Error,
            message: format!("{}: {}", failure.path.display(), failure.source),
            package: None,
            path: Some(failure.path.clone()),
            governed_by: None,
            escalated_by: None,
        });
    }
    for bump in plan.iter().flat_map(|plan| &plan.bumps) {
        let Some(BumpReason::Inference { commits, .. }) = &bump.reason else {
            continue;
        };
        let has_changeset = packages
            .iter()
            .any(|record| record.package == bump.package && !record.pending_changesets.is_empty());
        if has_changeset {
            continue;
        }
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::InferredReleaseWithoutChangeset,
            severity: DiagnosticSeverity::Warning,
            message: format!(
                "`{}` has a pending release inferred from {commits} commit(s) and no changeset; the release pull request reads changesets only, so add one to release it",
                bump.package.display_name()
            ),
            package: Some(bump.package.clone()),
            path: None,
            governed_by: None,
            escalated_by: None,
        });
    }
    escalate(&mut diagnostics, opts.strict);

    let has_changesets = packages.iter().any(|p| !p.pending_changesets.is_empty());
    // Count of packages with a planned bump, post-cascade/fixed/linked-group --
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
/// (crates/callisto-graph/src/commands/validate.rs, now removed): entries shape and
/// package-name resolution. Resolves through the same identity index
/// `resolve_pending_changesets` does, but this scan must never abort early,
/// so an unknown or ambiguous entry becomes a diagnostic instead of an
/// early return.
///
/// An entries-empty changeset is advisory only (`Info`): `@changesets/cli add --empty`
/// produces one deliberately, so it is not a well-formedness problem. A changeset with
/// entries but an empty summary never reaches this scan at all -- `load_changesets_permissive`
/// already rejects that shape at parse time, surfacing it as a `ChangesetParseFailed`
/// diagnostic instead.
fn changeset_wellformedness_diagnostics<'a>(
    packages: impl Iterator<Item = &'a Package> + Clone,
    identity: &IdentityIndex,
    loaded: &[LoadedChangeset],
) -> Result<Vec<Diagnostic>, GraphError> {
    let mut diagnostics = Vec::new();
    for cs in loaded {
        if cs.changeset.entries.is_empty() {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::EmptyChangeset,
                severity: DiagnosticSeverity::Info,
                message: format!("Changeset `{}` is empty", cs.path.display()),
                package: None,
                path: Some(cs.path.clone()),
                governed_by: None,
                escalated_by: None,
            });
        }

        for entry in &cs.changeset.entries {
            match PackageId::parse(&entry.name) {
                Ok(id) => match resolve_target_package(packages.clone(), identity, &id) {
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
                    Err(GraphError::AmbiguousName { candidates, .. }) => {
                        let names: Vec<String> = candidates.iter().map(|id| id.display_name().to_string()).collect();
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
                    Err(e) => return Err(e),
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
    Ok(diagnostics)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use callisto_model::format::{Changeset, Entry};
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

    /// Index as `ManifestWalkResolver::build` would register each package's own manifest names.
    fn identity_for(entries: &[(Ecosystem, &str, &Package)]) -> IdentityIndex {
        let mut index = IdentityIndex::default();
        for (eco, name, pkg) in entries {
            index.native.insert((*eco, (*name).to_string()), pkg.id.clone());
        }
        index
    }

    /// A bare entry naming a name shared by two promoted packages attaches to neither;
    /// `changeset_wellformedness_diagnostics` reports it instead.
    #[test]
    fn test_resolve_pending_changesets_ambiguous_bare_name_is_skipped() {
        let cargo_foo = make_package(Ecosystem::Cargo, "foo");
        let npm_foo = make_package(Ecosystem::Npm, "foo");
        let identity = identity_for(&[(Ecosystem::Cargo, "foo", &cargo_foo), (Ecosystem::Npm, "foo", &npm_foo)]);
        let loaded = vec![make_loaded_changeset("foo", Severity::Minor)];

        let pending = resolve_pending_changesets([&cargo_foo, &npm_foo].into_iter(), &identity, &loaded).unwrap();

        assert!(pending.is_empty());
    }

    #[test]
    fn test_resolve_pending_changesets_unambiguous_bare_name_resolves() {
        let cargo_foo = make_package(Ecosystem::Cargo, "foo");
        let identity = identity_for(&[(Ecosystem::Cargo, "foo", &cargo_foo)]);
        let loaded = vec![make_loaded_changeset("foo", Severity::Major)];

        let pending = resolve_pending_changesets([&cargo_foo].into_iter(), &identity, &loaded).unwrap();

        let (names, sev) = pending.get(&cargo_foo.id).expect("package should be present");
        assert_eq!(names, &vec!["foo-changeset".to_string()]);
        assert_eq!(sev, &Some(Severity::Major));
    }

    /// Two selectors for one package fold into one record carrying the higher severity.
    #[test]
    fn test_resolve_pending_changesets_resolves_a_secondary_native_name() {
        let owner = make_package(Ecosystem::Cargo, "x-core");
        let identity = identity_for(&[
            (Ecosystem::Cargo, "x-core", &owner),
            (Ecosystem::Npm, "@x/core", &owner),
        ]);
        let mut one = make_loaded_changeset("@x/core", Severity::Minor);
        one.changeset.entries.push(Entry {
            name: "npm/@x/core".to_string(),
            severity: Severity::Patch,
        });
        let loaded = vec![one];

        let pending = resolve_pending_changesets([&owner].into_iter(), &identity, &loaded).unwrap();

        let (names, sev) = pending.get(&owner.id).expect("owner should be present");
        assert_eq!(names, &vec!["core-changeset".to_string()]);
        assert_eq!(sev, &Some(Severity::Minor));
    }

    #[test]
    fn test_resolve_pending_changesets_qualified_selector_for_a_missing_ecosystem_is_skipped() {
        let owner = make_package(Ecosystem::Cargo, "foo");
        let identity = identity_for(&[(Ecosystem::Cargo, "foo", &owner)]);
        let loaded = vec![make_loaded_changeset("npm/foo", Severity::Minor)];

        let pending = resolve_pending_changesets([&owner].into_iter(), &identity, &loaded).unwrap();

        assert!(pending.is_empty());
    }

    #[test]
    fn test_wellformedness_reports_unknown_for_a_qualified_selector_the_package_lacks() {
        let owner = make_package(Ecosystem::Cargo, "foo");
        let identity = identity_for(&[(Ecosystem::Cargo, "foo", &owner)]);
        let loaded = vec![make_loaded_changeset("npm/foo", Severity::Minor)];

        let diagnostics = changeset_wellformedness_diagnostics([&owner].into_iter(), &identity, &loaded).unwrap();

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, DiagnosticCode::UnknownPackage);
    }

    #[test]
    fn test_wellformedness_accepts_a_secondary_native_name_and_flags_ambiguity() {
        let owner = make_package(Ecosystem::Cargo, "x");
        let other = make_package(Ecosystem::Cargo, "shared");
        let identity = identity_for(&[
            (Ecosystem::Cargo, "x", &owner),
            (Ecosystem::Npm, "shared", &owner),
            (Ecosystem::Cargo, "shared", &other),
        ]);
        let ok = vec![make_loaded_changeset("npm/shared", Severity::Minor)];
        let diagnostics = changeset_wellformedness_diagnostics([&owner, &other].into_iter(), &identity, &ok).unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");

        let ambiguous = vec![make_loaded_changeset("shared", Severity::Minor)];
        let diagnostics =
            changeset_wellformedness_diagnostics([&owner, &other].into_iter(), &identity, &ambiguous).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, DiagnosticCode::AmbiguousPackageName);
    }
}
