use std::collections::HashSet;

use callisto_model::{
    CommandRunner, CratePublish, DepKind, Ecosystem, NpmMainPublish, PackageId, PublishPlan, PublishTarget,
    PypiPublish, RegistryKey, ReleaseEntry, SCHEMA_VERSION,
};
use callisto_vcs::GitDataSource;

use crate::error::GraphError;
use crate::resolver::DependencyResolver;
use crate::toposort::toposort_impl;
use crate::Workspace;

/// Edge kinds cascade/version-bump propagation cares about: a `Dev`-only
/// dependency change correctly never forces a consumer's version to bump.
const CASCADE_ORDERING_KINDS: &[DepKind] = &[DepKind::Runtime, DepKind::Build, DepKind::Optional];

/// Edge kinds publish ordering cares about: cascade's kinds, plus `Dev`.
/// `cargo publish` (run without `--no-verify`, see `publish_client.rs`)
/// re-extracts the packaged tarball and does a real local build to verify
/// it, which needs *every* dependency in the crate's `Cargo.toml` —
/// `[dev-dependencies]` included — resolvable from the registry. A
/// dev-dependency on a workspace sibling published in the same batch
/// therefore still needs that sibling to publish first, even though the
/// two crates have no cascade-relevant ordering constraint between them.
const PUBLISH_ORDERING_KINDS: &[DepKind] = &[DepKind::Runtime, DepKind::Build, DepKind::Optional, DepKind::Dev];

/// Computes the order packages must be published in.
///
/// This is a thin, purpose-named wrapper around [`toposort_impl`] (the
/// generic algorithm, reused as-is) rather than a generic
/// `DependencyResolverExt::toposort()` on a shared trait — it exists
/// specifically for `plan_publish` below, its only caller, and its
/// semantics are publish-specific, not "the one true topological sort."
///
/// Tries [`PUBLISH_ORDERING_KINDS`] first (including `Dev`, unlike cascade's
/// own [`CASCADE_ORDERING_KINDS`]) so a dev-dependency on a same-batch
/// sibling publishes in the right order. `Dev` edges are best-effort, not a
/// hard requirement, precisely because mutual dev-only dependencies between
/// two otherwise-unrelated packages are a legitimate pattern (e.g. two
/// crates each dev-depending on the other for cross-integration tests) —
/// unlike `Runtime`/`Build`/`Optional`, which must never cycle, a `Dev`
/// cycle must not hard-fail the whole publish plan. If including `Dev`
/// edges would produce a cycle, this excludes `Dev` edges only between the
/// specific packages that form a Dev-induced cycle — a `Dev` edge anywhere
/// else in `subset` (an unrelated pair with no cycle at all) still counts as
/// an ordering constraint, so one legitimate Dev-only cycle can never
/// silently un-order an unrelated dev-dependency elsewhere in the same
/// batch. A cycle that survives with every `Dev` edge excluded is a genuine
/// `Runtime`/`Build`/`Optional` cycle and still hard-fails the whole plan.
fn publish_order<D: DependencyResolver + ?Sized>(
    resolver: &D,
    subset: &HashSet<PackageId>,
) -> Result<Vec<PackageId>, GraphError> {
    let all_pkg_ids: Vec<PackageId> = resolver.packages().map(|p| p.id.clone()).collect();
    let edges_of = |id: &PackageId| -> Vec<(PackageId, DepKind)> {
        resolver.dependencies_of(id).map(|e| (e.to.clone(), e.kind)).collect()
    };

    match toposort_impl(subset, &all_pkg_ids, PUBLISH_ORDERING_KINDS, edges_of) {
        Ok(order) => Ok(order),
        Err(GraphError::Cycle { .. }) => {
            // Confirm the cycle is Dev-induced: a cycle that survives with no
            // Dev edges at all is a genuine Runtime/Build/Optional cycle,
            // which must still hard-fail the whole plan.
            toposort_impl(subset, &all_pkg_ids, CASCADE_ORDERING_KINDS, edges_of)?;

            let cyclic_components = crate::toposort::cyclic_sccs(subset, edges_of, PUBLISH_ORDERING_KINDS);
            crate::toposort::toposort_with_edge_filter(subset, &all_pkg_ids, edges_of, |from, to, kind| {
                if !PUBLISH_ORDERING_KINDS.contains(&kind) {
                    return false;
                }
                if kind == DepKind::Dev {
                    let in_same_cyclic_component = cyclic_components
                        .iter()
                        .any(|scc| scc.contains(from) && scc.contains(to));
                    return !in_same_cyclic_component;
                }
                true
            })
        }
        Err(e) => Err(e),
    }
}

#[derive(Clone, Debug, Default)]
pub struct PublishOptions {
    /// When non-empty, only packages whose bare name (without ecosystem prefix)
    /// appears in this list are included in the plan. An empty `only` list
    /// means "include all packages" (the default).
    pub only: Vec<String>,
}

/// Validates a `publishConfig.registry` URL from a package's own
/// `package.json` before it's used as an `npm publish --registry`/`npm
/// view --registry` target (both run with `NPM_TOKEN` live in CI).
///
/// `publishConfig.registry` is attacker-controllable (a PR author sets
/// their own `package.json`) and must never be trusted verbatim. Two
/// checks, mirroring the leading-`-` flag-injection guard on package names
/// (see `SubprocessRegistryClient::npm_publish`):
///
/// 1. Must use `https` -- a scheme downgrade is rejected even if the host
///    would otherwise be approved.
/// 2. Must exactly match a `url` on an `npm`-kind entry in
///    `callisto.toml`'s `[registries]` table.
///
/// No configured npm registries means no override is ever approved --
/// `callisto.toml`, not `package.json`, is the source of truth for where
/// credentialed publish requests can go.
fn validate_npm_registry_url(
    url: &str,
    package: &PackageId,
    registries: &std::collections::BTreeMap<RegistryKey, crate::config::RegistryConfig>,
) -> Result<(), GraphError> {
    let is_approved = url.starts_with("https://")
        && registries
            .values()
            .any(|cfg| cfg.kind == Ecosystem::Npm && cfg.url.as_deref() == Some(url));

    if is_approved {
        Ok(())
    } else {
        Err(GraphError::UntrustedNpmRegistry {
            package: package.clone(),
            url: url.to_string(),
        })
    }
}

/// Resolves a package's `changelog_section` for `plan_publish`: reads the file at
/// `ws_root.join(changelog_rel_path)` and extracts the `## {ver}` section via
/// `callisto_changelog::extract_section`. Every non-fatal outcome (file not found, no
/// matching heading, empty matched section, or an unreadable file) leaves the return value
/// `None` and pushes exactly one Warning diagnostic into `diagnostics` rather than aborting
/// the plan -- `ChangelogSectionNotFound` for the first three (AC-10b, AC-11, AC-12),
/// `ChangelogReadError` for a read failure that is not "file does not exist" (AC-12c).
fn resolve_changelog_section(
    ws_root: &std::path::Path,
    changelog_rel_path: &std::path::Path,
    pkg_id: &callisto_model::PackageId,
    ver: &callisto_model::Version,
    diagnostics: &mut Vec<callisto_model::Diagnostic>,
) -> Option<String> {
    let full_path = ws_root.join(changelog_rel_path);
    let content = match std::fs::read_to_string(&full_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            diagnostics.push(callisto_model::Diagnostic {
                code: callisto_model::DiagnosticCode::ChangelogSectionNotFound,
                severity: callisto_model::DiagnosticSeverity::Warning,
                message: format!(
                    "no changelog file found at `{}` for package `{}`",
                    changelog_rel_path.display(),
                    pkg_id.display_name()
                ),
                package: Some(pkg_id.clone()),
                path: Some(changelog_rel_path.to_path_buf()),
                escalated_by: None,
                governed_by: None,
            });
            return None;
        }
        Err(e) => {
            diagnostics.push(callisto_model::Diagnostic {
                code: callisto_model::DiagnosticCode::ChangelogReadError,
                severity: callisto_model::DiagnosticSeverity::Warning,
                message: format!(
                    "could not read changelog at `{}` for package `{}`: {e}",
                    changelog_rel_path.display(),
                    pkg_id.display_name()
                ),
                package: Some(pkg_id.clone()),
                path: Some(changelog_rel_path.to_path_buf()),
                escalated_by: None,
                governed_by: None,
            });
            return None;
        }
    };

    match callisto_changelog::extract_section(&content, ver) {
        Some(section) => Some(section.to_string()),
        None => {
            diagnostics.push(callisto_model::Diagnostic {
                code: callisto_model::DiagnosticCode::ChangelogSectionNotFound,
                severity: callisto_model::DiagnosticSeverity::Warning,
                message: format!(
                    "no `## {}` section found in `{}` for package `{}`",
                    ver.render(),
                    changelog_rel_path.display(),
                    pkg_id.display_name()
                ),
                package: Some(pkg_id.clone()),
                path: Some(changelog_rel_path.to_path_buf()),
                escalated_by: None,
                governed_by: None,
            });
            None
        }
    }
}

pub fn plan_publish<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    opts: &PublishOptions,
) -> Result<PublishPlan, GraphError> {
    let mut rust_crates = Vec::new();
    let mut npm_main_packages = Vec::new();
    let mut npm_platform_packages = Vec::new();
    let mut pypi_packages = Vec::new();
    let mut releases = Vec::new();

    let base_versions = ws.base_versions()?;
    let inference = crate::infer::NoInference;
    let mut diagnostics: Vec<callisto_model::Diagnostic> = Vec::new();
    let version_plan = match crate::commands::version::plan_version(
        ws,
        &inference,
        &crate::commands::version::VersionOptions::default(),
    ) {
        Ok(plan) => Some(plan),
        Err(e) => {
            diagnostics.push(callisto_model::Diagnostic {
                code: callisto_model::DiagnosticCode::ChangesetReadError,
                severity: callisto_model::DiagnosticSeverity::Warning,
                message: format!("Could not read changesets: {e}"),
                package: None,
                path: None,
                escalated_by: None,
                governed_by: None,
            });
            None
        }
    };

    // Build a single lookup map once — eliminates O(N) scans inside the topo loop
    // (PERF-003/004/005). Keys and values are borrowed from the graph for the
    // lifetime of this function, so no extra clones are needed for the lookups.
    let pkg_map: std::collections::HashMap<&callisto_model::PackageId, &callisto_model::Package> =
        ws.graph.packages().map(|p| (&p.id, p)).collect();
    let all_ids: std::collections::HashSet<_> = pkg_map.keys().map(|&id| id.clone()).collect();
    let topo_ids = publish_order(&ws.graph, &all_ids)?;

    // `Workspace::git_access` (native gix, falling back to the
    // `CommandRunner` shell path when unavailable -- always true on
    // wasm32) rather than a fresh `GitAccess::discover`, which has no
    // such fallback: on wasm32, native discovery unconditionally fails
    // (gix is excluded from that target's dependency set), so `head_sha`
    // was always `None` there, silently omitting every release entry
    // from the plan. Sharing the workspace-scoped instance also means
    // this command's tag-index lookup below (via `ws.tags()`) reuses the
    // same discovery instead of paying for a second one.
    let head_sha = match ws.git_access().head_sha() {
        Ok(sha) => Some(sha),
        Err(e) => {
            diagnostics.push(callisto_model::Diagnostic {
                code: callisto_model::DiagnosticCode::GitDiscoveryFailed,
                severity: callisto_model::DiagnosticSeverity::Warning,
                message: format!("Could not resolve HEAD SHA: {e}; release entries will be omitted from the plan"),
                package: None,
                path: None,
                escalated_by: None,
                governed_by: None,
            });
            None
        }
    };

    // Build the tag index once before the loop. If git is unavailable (no
    // .git directory, no git binary, or any other VCS error), emit a soft
    // diagnostic and treat every package as a release candidate for this
    // plan (tag_match = false). Hard-propagating the error here would
    // contradict the soft GitDiscoveryFailed diagnostic already emitted by
    // the head_sha block above.
    let tag_index = match ws.tags() {
        Ok(idx) => Some(idx),
        Err(e) => {
            diagnostics.push(callisto_model::Diagnostic {
                code: callisto_model::DiagnosticCode::GitDiscoveryFailed,
                severity: callisto_model::DiagnosticSeverity::Warning,
                message: format!("Could not read git tags: {e}; all packages treated as release candidates"),
                package: None,
                path: None,
                escalated_by: None,
                governed_by: None,
            });
            None
        }
    };

    // Whether each workspace package is a release candidate this run,
    // tracked unconditionally (not just for packages that end up in a
    // publish list) so the depends_on_platforms cross-check below can tell
    // "this platform sibling isn't in the plan because it's already
    // published" (is_release == false) apart from "it's misconfigured or
    // was filtered out" (is_release == true but never dispatched).
    let mut is_release_by_id: std::collections::HashMap<PackageId, bool> = std::collections::HashMap::new();
    // Packages that actually landed in at least one of the four publish
    // lists below. A package can be `is_release == true` and still never
    // appear here (e.g. `publish_to` is empty, or only names
    // not-yet-implemented targets) — that distinction is exactly what the
    // --package precise-error and depends_on_platforms checks need.
    let mut dispatched_ids: std::collections::HashSet<PackageId> = std::collections::HashSet::new();

    for id in &topo_ids {
        let pkg = match pkg_map.get(id) {
            Some(&p) => p,
            None => continue,
        };

        let bump_info = version_plan
            .as_ref()
            .and_then(|plan| plan.bumps.iter().find(|b| b.package == pkg.id));

        let (is_release, ver) = if let Some(bump) = bump_info {
            (true, bump.to.clone())
        } else {
            let cur_ver = base_versions.get(&pkg.id).cloned().ok_or_else(|| {
                GraphError::Manifest(callisto_model::ManifestError::MissingField {
                    path: pkg.manifests.first().map(|m| m.path.clone()).unwrap_or_default(),
                    field: "version",
                })
            })?;
            let tag_match = tag_index
                .and_then(|idx| idx.last_tag(&pkg.id))
                .map(|t| t.version == cur_ver)
                .unwrap_or(false);
            (!tag_match, cur_ver)
        };
        is_release_by_id.insert(pkg.id.clone(), is_release);

        if is_release {
            // Single exhaustive dispatch match over every configured target —
            // replaces the old ad-hoc `.any(matches!(...))` membership checks,
            // which silently dropped `PublishTarget::NuGet`/`GitHubRelease` on
            // the floor with no diagnostic. `PublishTarget` is `#[non_exhaustive]`
            // (defined in callisto-model), so a wildcard arm is still required
            // by the compiler even though every current variant is named
            // explicitly below; the wildcard exists only to catch a future
            // variant added without a corresponding arm here, not to silently
            // swallow one of today's variants.
            let mut publishes_cargo = false;
            let mut publishes_npm = false;
            let mut publishes_pypi = false;
            let mut npm_registry_url: Option<String> = None;
            let mut npm_access: Option<callisto_model::NpmAccess> = None;
            // True once at least one configured target has a real dispatch
            // implementation. Drives the release-tag/ReleaseEntry gate below —
            // a package configured only with not-yet-implemented targets
            // (NuGet, GitHubRelease) must not get a ReleaseEntry claiming a
            // release happened when nothing was actually publishable.
            let mut has_dispatchable_target = false;

            for target in &pkg.publish_to {
                match target {
                    callisto_model::PublishTarget::CratesIo => {
                        publishes_cargo = true;
                        has_dispatchable_target = true;
                    }
                    callisto_model::PublishTarget::Npm { registry, access } => {
                        publishes_npm = true;
                        has_dispatchable_target = true;
                        // Extract the private registry URL and access
                        // setting from the first Npm target, both read
                        // from `publishConfig` in package.json.
                        if npm_registry_url.is_none() {
                            if let Some(url) = registry {
                                validate_npm_registry_url(url, &pkg.id, &ws.config.registries)?;
                            }
                            npm_registry_url = registry.clone();
                            npm_access = *access;
                        }
                    }
                    callisto_model::PublishTarget::Pypi { .. } => {
                        publishes_pypi = true;
                        has_dispatchable_target = true;
                    }
                    callisto_model::PublishTarget::NuGet { .. } => {
                        diagnostics.push(callisto_model::Diagnostic {
                            code: callisto_model::DiagnosticCode::PublishTargetNotImplemented,
                            severity: callisto_model::DiagnosticSeverity::Warning,
                            message: format!(
                                "package `{}` configures publish-to = [\"nuget\"], but NuGet \
                                 publishing is not yet implemented; this target will not be \
                                 published",
                                pkg.id.display_name()
                            ),
                            package: Some(pkg.id.clone()),
                            path: None,
                            escalated_by: None,
                            governed_by: None,
                        });
                    }
                    callisto_model::PublishTarget::GitHubRelease => {
                        diagnostics.push(callisto_model::Diagnostic {
                            code: callisto_model::DiagnosticCode::PublishTargetNotImplemented,
                            severity: callisto_model::DiagnosticSeverity::Warning,
                            message: format!(
                                "package `{}` configures publish-to = [\"github-release\"], but \
                                 GitHub Release publishing is not yet implemented; this target \
                                 will not be published",
                                pkg.id.display_name()
                            ),
                            package: Some(pkg.id.clone()),
                            path: None,
                            escalated_by: None,
                            governed_by: None,
                        });
                    }
                    callisto_model::PublishTarget::None => {}
                    #[allow(unreachable_patterns)]
                    _ => {
                        diagnostics.push(callisto_model::Diagnostic {
                            code: callisto_model::DiagnosticCode::PublishTargetNotImplemented,
                            severity: callisto_model::DiagnosticSeverity::Warning,
                            message: format!(
                                "package `{}` configures a publish-to target with no \
                                 implemented dispatch; this target will not be published",
                                pkg.id.display_name()
                            ),
                            package: Some(pkg.id.clone()),
                            path: None,
                            escalated_by: None,
                            governed_by: None,
                        });
                    }
                }
            }

            let is_platform_pkg = pkg
                .manifests
                .iter()
                .any(|m| matches!(m.role, callisto_model::ManifestRole::Platform { .. }));

            // Resolve the package directory (relative to workspace root) from
            // the first manifest path. All manifests for a package share the
            // same parent directory, so any first manifest is correct.
            let pkg_dir = pkg
                .manifests
                .first()
                .and_then(|m| m.path.parent())
                .map(|p| p.to_path_buf())
                // SAFETY: unwrap_or_default produces an empty PathBuf only when
                // no manifests exist; in that case package_dir being empty just
                // disables the pre-publish version check, which is acceptable.
                .unwrap_or_default();

            if publishes_cargo {
                dispatched_ids.insert(pkg.id.clone());
                rust_crates.push(CratePublish {
                    name: pkg.id.name().to_string(),
                    version: ver.clone(),
                    publish_to: callisto_model::RegistryKey(callisto_model::RegistryKey::CRATES_IO.to_string()),
                    registry: None,
                    package_dir: if pkg_dir.as_os_str().is_empty() {
                        None
                    } else {
                        Some(pkg_dir.clone())
                    },
                });
            }

            if publishes_npm {
                let tag = if ver.is_prerelease() {
                    Some("next".to_string())
                } else {
                    None
                };

                // Determine npm access level. Honour the operator's explicit
                // `publishConfig.access` from package.json first, whatever it
                // is -- "restricted", or "public" (which a bare bool used to
                // silently drop for unscoped packages, since it collapsed
                // "absent" and "explicit public" to the same value). Only
                // fall back to the `@scope/name`-implies-public heuristic
                // when nothing was explicitly set. npm's `--access` CLI flag
                // takes full precedence over publishConfig.access, so
                // callisto must read and propagate the intent explicitly
                // here.
                let access = npm_access.or_else(|| {
                    if pkg.id.name().starts_with('@') {
                        Some(callisto_model::NpmAccess::Public)
                    } else {
                        None
                    }
                });

                if is_platform_pkg {
                    dispatched_ids.insert(pkg.id.clone());
                    npm_platform_packages.push(callisto_model::NpmPublish {
                        name: pkg.id.name().to_string(),
                        version: ver.clone(),
                        publish_to: callisto_model::RegistryKey(callisto_model::RegistryKey::NPM.to_string()),
                        package_dir: pkg_dir.clone(),
                        registry: npm_registry_url.clone(),
                        tag: tag.clone(),
                        access,
                    });
                } else {
                    let platform_deps: Vec<String> = ws
                        .graph
                        .dependencies_of(&pkg.id)
                        .filter(|edge| {
                            pkg_map
                                .get(&edge.to)
                                .map(|p| {
                                    p.manifests
                                        .iter()
                                        .any(|m| matches!(m.role, callisto_model::ManifestRole::Platform { .. }))
                                })
                                .unwrap_or(false)
                        })
                        .map(|edge| edge.to.name().to_string())
                        .collect();

                    dispatched_ids.insert(pkg.id.clone());
                    npm_main_packages.push(NpmMainPublish {
                        name: pkg.id.name().to_string(),
                        version: ver.clone(),
                        publish_to: callisto_model::RegistryKey(callisto_model::RegistryKey::NPM.to_string()),
                        package_dir: pkg_dir.clone(),
                        registry: npm_registry_url,
                        tag,
                        access,
                        depends_on_platforms: platform_deps,
                    });
                }
            }

            if publishes_pypi {
                // Extract the optional custom index URL from the first Pypi
                // target. Multiple Pypi entries on the same package are not
                // expected, so only the first is consulted.
                let index = pkg
                    .publish_to
                    .iter()
                    .find_map(|t| {
                        if let callisto_model::PublishTarget::Pypi { index } = t {
                            Some(index.clone())
                        } else {
                            None
                        }
                    })
                    .flatten();

                dispatched_ids.insert(pkg.id.clone());
                pypi_packages.push(PypiPublish {
                    name: pkg.id.name().to_string(),
                    version: ver.clone(),
                    publish_to: RegistryKey(RegistryKey::PYPI.to_string()),
                    package_dir: pkg_dir,
                    index,
                });
            }

            if !pkg.publish_to.is_empty()
                && !pkg.publish_to.iter().all(|t| *t == PublishTarget::None)
                && has_dispatchable_target
            {
                // Both head_sha and tag_index must be available: head_sha supplies
                // the commit to tag and tag_index supplies the template to render
                // the tag name. When tag_index is None (ws.tags() failed and was
                // soft-handled above), release entries are omitted — consistent
                // with the GitDiscoveryFailed diagnostic already pushed.
                if let (Some(ref sha), Some(idx)) = (&head_sha, tag_index) {
                    let changelog_section = pkg.changelog.as_ref().and_then(|ch_path| {
                        resolve_changelog_section(&ws.root, ch_path, &pkg.id, &ver, &mut diagnostics)
                    });
                    releases.push(ReleaseEntry {
                        package: pkg.id.clone(),
                        tag_name: idx.template(&pkg.id).render(&ver),
                        sha: sha.clone(),
                        changelog_section,
                        is_prerelease: ver.is_prerelease(),
                    });
                }
            }
        }
    }

    // Apply the `only` filter: when the caller specifies a set of package names,
    // drop everything not in that set from every ecosystem list. An empty `only`
    // means "all packages".
    //
    // Each requested name is resolved to a single, ecosystem-disambiguated
    // `PackageId` via `PackageId::resolve_unique` against the full workspace
    // membership *before* any retaining happens — matching by bare name alone
    // (the old behaviour) would let `--package core` silently sweep up an
    // unrelated Cargo crate and an unrelated npm package that merely happen
    // to share the name `core`. A bare, unqualified request that is genuinely
    // ambiguous (two workspace packages share the name across ecosystems)
    // reuses the existing `AmbiguousName` error and tells the caller to
    // qualify it (`npm:core`).
    if !opts.only.is_empty() {
        let mut resolved: Vec<PackageId> = Vec::with_capacity(opts.only.len());
        for requested in &opts.only {
            let requested_id = PackageId::parse(requested).map_err(|_parse_err| GraphError::UnknownPackage {
                id: PackageId::Bare(requested.clone()),
            })?;
            match requested_id.resolve_unique(all_ids.iter(), |id| id) {
                Ok(Some(id)) => resolved.push(id.clone()),
                Ok(None) => {
                    return Err(GraphError::UnknownPackage {
                        id: PackageId::Bare(requested.clone()),
                    });
                }
                Err(candidates) => {
                    return Err(GraphError::AmbiguousName {
                        name: requested.clone(),
                        candidates: candidates.into_iter().cloned().collect(),
                    });
                }
            }
        }

        let keep = |ecosystem: Ecosystem, name: &str| {
            let entry_id = resolve_entry_id(&pkg_map, ecosystem, name);
            resolved.contains(&entry_id)
        };
        rust_crates.retain(|c| keep(Ecosystem::Cargo, &c.name));
        npm_main_packages.retain(|c| keep(Ecosystem::Npm, &c.name));
        npm_platform_packages.retain(|c| keep(Ecosystem::Npm, &c.name));
        pypi_packages.retain(|c| keep(Ecosystem::Pypi, &c.name));
        releases.retain(|r| resolved.iter().any(|id| id.matches(&r.package)));

        // Every requested package must actually land in the plan. A name that
        // resolved above (it exists in the workspace) but never made it into
        // any list is either not a release candidate right now, or configures
        // no dispatchable publish target — both distinct, more actionable
        // causes than a plain typo, so this reports which one applies instead
        // of the generic "not found in workspace" UnknownPackage message.
        for id in &resolved {
            if dispatched_ids.contains(id) {
                continue;
            }
            let reason = if is_release_by_id.get(id) == Some(&false) {
                crate::error::NotInPlanReason::NotARelease
            } else {
                crate::error::NotInPlanReason::NoDispatchableTarget
            };
            return Err(GraphError::PackageNotInPublishPlan { id: id.clone(), reason });
        }
    }

    // Cross-check every npm main package's declared platform dependencies
    // against what actually ended up in the final plan. `depends_on_platforms`
    // is computed from graph edges alone (above) and knows nothing about
    // whether the named sibling is actually publishable this run — it could
    // be missing because `--only` filtered it out, because it's misconfigured
    // (no npm publish target), or legitimately absent because it's already
    // published (its version already tag-matches, so it was never a release
    // candidate this run). Only the last case is safe to let through silently.
    for main in &npm_main_packages {
        for dep_name in &main.depends_on_platforms {
            if npm_platform_packages.iter().any(|p| &p.name == dep_name) {
                continue;
            }
            let dep_id = resolve_entry_id(&pkg_map, Ecosystem::Npm, dep_name);
            if is_release_by_id.get(&dep_id) == Some(&false) {
                continue;
            }
            let main_id = resolve_entry_id(&pkg_map, Ecosystem::Npm, &main.name);
            return Err(GraphError::MissingPlatformDependency {
                main: main_id,
                depends_on: dep_name.clone(),
            });
        }
    }

    Ok(PublishPlan {
        schema_version: SCHEMA_VERSION,
        rust_crates,
        npm_main_packages,
        npm_platform_packages,
        pypi_packages,
        releases,
        diagnostics,
    })
}

/// Recovers a plan entry's real workspace `PackageId` from its ecosystem and
/// bare name. `CratePublish`/`NpmPublish`/`NpmMainPublish`/`PypiPublish`
/// store only a bare `name: String` (no wire-format change here), so this
/// reconstructs the id the same way `walk.rs`'s identity-promotion leaves
/// it: unpromoted packages keep a `Bare` id; a package only gets a
/// `Prefixed` id when it collided with a same-named package in another
/// ecosystem. Trying `Bare` first and falling back to `Prefixed` mirrors
/// that: a package is never registered under both forms at once.
fn resolve_entry_id(
    pkg_map: &std::collections::HashMap<&PackageId, &callisto_model::Package>,
    ecosystem: Ecosystem,
    name: &str,
) -> PackageId {
    let bare = PackageId::Bare(name.to_string());
    if pkg_map.contains_key(&bare) {
        return bare;
    }
    PackageId::Prefixed {
        ecosystem,
        name: name.to_string(),
    }
}

/// Filters `plan` down to only the entries `report` confirms actually
/// succeeded (`Published` or `AlreadyPublished`), dropping anything that
/// failed. For a CI pipeline that runs `plan-publish` -> `publish` -> `tag`
/// as separate steps, this lets `tag`/`gh release create` operate on what
/// actually shipped instead of the pre-publish plan -- so a single
/// package's failure doesn't cost its already-succeeded siblings a tag or a
/// GitHub Release in the same run.
///
/// Matches `rust_crates`/`npm_platform_packages`/`npm_main_packages`/
/// `pypi_packages` entries against `report.attempts` by the same
/// `PackageId::Prefixed { ecosystem, name }` shape [`PublishOrchestrator::execute`]
/// constructs at publish time (not [`resolve_entry_id`]'s bare-vs-prefixed
/// resolution, since `report` carries no workspace context to resolve
/// against). `releases` entries carry the package's real graph-resolved id
/// instead, which may be `Bare` even when the matching attempt is
/// `Prefixed` -- matched via `PackageId::matches`'s bare-is-wildcard
/// semantics, so a release with two publish targets (e.g. Cargo and npm) is
/// kept only when every target for that package succeeded.
pub fn filter_plan_by_report(plan: &PublishPlan, report: &callisto_model::PublishReport) -> PublishPlan {
    let succeeded: std::collections::HashSet<&PackageId> = report
        .attempts
        .iter()
        .filter(|a| !a.result.is_failure())
        .map(|a| &a.package)
        .collect();
    let failed: Vec<&PackageId> = report
        .attempts
        .iter()
        .filter(|a| a.result.is_failure())
        .map(|a| &a.package)
        .collect();

    let kept = |ecosystem: Ecosystem, name: &str| {
        let id = PackageId::Prefixed {
            ecosystem,
            name: name.to_string(),
        };
        succeeded.contains(&id)
    };

    PublishPlan {
        schema_version: plan.schema_version,
        rust_crates: plan
            .rust_crates
            .iter()
            .filter(|c| kept(Ecosystem::Cargo, &c.name))
            .cloned()
            .collect(),
        npm_platform_packages: plan
            .npm_platform_packages
            .iter()
            .filter(|c| kept(Ecosystem::Npm, &c.name))
            .cloned()
            .collect(),
        npm_main_packages: plan
            .npm_main_packages
            .iter()
            .filter(|c| kept(Ecosystem::Npm, &c.name))
            .cloned()
            .collect(),
        pypi_packages: plan
            .pypi_packages
            .iter()
            .filter(|c| kept(Ecosystem::Pypi, &c.name))
            .cloned()
            .collect(),
        releases: plan
            .releases
            .iter()
            .filter(|r| !failed.iter().any(|f| f.matches(&r.package)))
            .cloned()
            .collect(),
        diagnostics: plan.diagnostics.clone(),
    }
}

use callisto_model::{
    ApplyPermit, PublishAttempt, PublishAttemptResult, PublishOutcome, PublishReport, RateLimitPolicy, RegistryClient,
    RegistryError, TimeProvider, Version,
};
use std::time::Duration;

/// Maximum number of rate-limit retries per package before the orchestrator
/// gives up and records a failure. Prevents an infinite retry loop when a
/// registry consistently returns 429 responses with a short `retry_after`.
pub(crate) const MAX_RATE_LIMIT_RETRIES: usize = 10;

/// Maximum `retry_after` duration (seconds) the orchestrator will honor
/// before treating the rate-limit as a hard failure.
const MAX_RETRY_AFTER_SECS: u64 = 600;

/// Default fallback wait (seconds) when the registry does not supply a
/// `retry_after` value and the client cannot parse one from output.
pub(crate) const DEFAULT_RATE_LIMIT_WAIT_SECS: u64 = 60;

/// Parses a numeric retry-after value (seconds) as reported by a registry
/// tool's output. Shared by [`PublishOrchestrator::parse_http_429_ttl`] and
/// by ecosystem [`RegistryClient`] implementations that need to extract a
/// retry duration from free-form subprocess output.
pub fn parse_retry_after(raw: &str) -> Option<Duration> {
    raw.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Production [`TimeProvider`] backed by the OS clock and a real sleep.
pub struct SystemTimeProvider;

impl TimeProvider for SystemTimeProvider {
    fn now(&self) -> std::time::SystemTime {
        std::time::SystemTime::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Production [`RateLimitPolicy`] that always permits the retry the registry
/// asked for. `PublishOrchestrator` already bounds total wait via its 600s
/// cutoff, so this policy has no additional gating to apply.
pub struct AlwaysRetryPolicy;

impl RateLimitPolicy for AlwaysRetryPolicy {
    fn check_rate_limit(&self, _retry_after: Duration) -> Result<(), RegistryError> {
        Ok(())
    }
}

pub struct PublishOrchestrator<R, P, T> {
    client: R,
    policy: P,
    time: T,
    progress: Option<Box<dyn Fn(String) + Send + Sync>>,
    skip_precheck: bool,
}

impl<R, P, T> PublishOrchestrator<R, P, T>
where
    R: RegistryClient,
    P: RateLimitPolicy,
    T: TimeProvider,
{
    pub fn new(client: R, policy: P, time: T) -> Self {
        Self {
            client,
            policy,
            time,
            progress: None,
            skip_precheck: false,
        }
    }

    /// Attach a progress callback that is invoked before each package publish
    /// attempt. The message includes the package name and version.
    pub fn with_progress<F: Fn(String) + Send + Sync + 'static>(mut self, f: F) -> Self {
        self.progress = Some(Box::new(f));
        self
    }

    /// Skips the `is_published` pre-check before every publish attempt,
    /// relying solely on the registry client's own already-published
    /// classification from the `publish()` call itself. Defaults to `false`
    /// (the pre-check runs) since some registries' `publish()` re-runs local
    /// lifecycle scripts (e.g. npm's `prepublishOnly`/`prepack`) even on an
    /// already-published version, which the pre-check avoids. Opt in when
    /// that cost matters more than the pre-check's extra registry round-trip.
    pub fn with_skip_precheck(mut self, skip: bool) -> Self {
        self.skip_precheck = skip;
        self
    }

    pub fn parse_http_429_ttl(retry_after_header: &str) -> Option<Duration> {
        parse_retry_after(retry_after_header)
    }

    fn emit_progress(&self, name: &str, version: &Version) {
        if let Some(ref cb) = self.progress {
            cb(format!("Publishing {name}@{version}…"));
        }
    }

    /// Attempts to publish every package in `plan` to its ecosystem
    /// registry, recording a per-package outcome (or failure) for each one
    /// rather than aborting the whole batch on the first error — one
    /// package's registry rejection or auth failure must not silently erase
    /// the fact that earlier packages in the same run genuinely published or
    /// were already present.
    pub fn execute(&self, plan: &PublishPlan, permit: &ApplyPermit) -> PublishReport {
        let mut attempts = Vec::new();

        for rust_crate in &plan.rust_crates {
            let pkg_id = PackageId::Prefixed {
                ecosystem: Ecosystem::Cargo,
                name: rust_crate.name.clone(),
            };
            self.emit_progress(&rust_crate.name, &rust_crate.version);
            attempts.push(self.attempt_publish(pkg_id, rust_crate.version.clone(), permit));
        }

        for npm_pkg in &plan.npm_platform_packages {
            let pkg_id = PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name: npm_pkg.name.clone(),
            };
            self.emit_progress(&npm_pkg.name, &npm_pkg.version);
            attempts.push(self.attempt_publish(pkg_id, npm_pkg.version.clone(), permit));
        }

        // Names of npm platform packages that did NOT successfully publish
        // above (Published/AlreadyPublished both count as success). A main
        // package declaring one of these in `depends_on_platforms` must not
        // be published: its `optionalDependencies` would reference a version
        // that was never actually uploaded. Scoped to `Ecosystem::Npm`:
        // `depends_on_platforms` is an npm-only construct, and a bare
        // `.name()` carries no ecosystem information, so an unscoped
        // comparison could mistake a same-named failed Cargo crate for a
        // failed npm platform dependency.
        let failed_platform_names: std::collections::HashSet<String> = attempts
            .iter()
            .filter(|a| a.package.ecosystem() == Some(Ecosystem::Npm) && a.result.is_failure())
            .map(|a| a.package.name().to_string())
            .collect();

        for npm_pkg in &plan.npm_main_packages {
            let pkg_id = PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name: npm_pkg.name.clone(),
            };
            let failed_deps: Vec<&str> = npm_pkg
                .depends_on_platforms
                .iter()
                .map(String::as_str)
                .filter(|dep| failed_platform_names.contains(*dep))
                .collect();
            if !failed_deps.is_empty() {
                attempts.push(PublishAttempt {
                    package: pkg_id,
                    version: npm_pkg.version.clone(),
                    result: PublishAttemptResult::Failed {
                        kind: "dependencyFailed".to_string(),
                        error: format!(
                            "skipped: platform dependenc{} failed to publish: {}",
                            if failed_deps.len() == 1 { "y" } else { "ies" },
                            failed_deps.join(", ")
                        ),
                    },
                });
                continue;
            }
            self.emit_progress(&npm_pkg.name, &npm_pkg.version);
            attempts.push(self.attempt_publish(pkg_id, npm_pkg.version.clone(), permit));
        }

        for pypi_pkg in &plan.pypi_packages {
            let pkg_id = PackageId::Prefixed {
                ecosystem: Ecosystem::Pypi,
                name: pypi_pkg.name.clone(),
            };
            self.emit_progress(&pypi_pkg.name, &pypi_pkg.version);
            attempts.push(self.attempt_publish(pkg_id, pypi_pkg.version.clone(), permit));
        }

        PublishReport {
            schema_version: callisto_model::SCHEMA_VERSION,
            attempts,
            diagnostics: Vec::new(),
        }
    }

    fn attempt_publish(&self, package: PackageId, version: Version, permit: &ApplyPermit) -> PublishAttempt {
        let result = match self.publish_with_retry(&package, &version, permit) {
            Ok(PublishOutcome::Published) => PublishAttemptResult::Published,
            Ok(PublishOutcome::AlreadyPublished) => PublishAttemptResult::AlreadyPublished,
            Err(err) => PublishAttemptResult::Failed {
                kind: err.kind_str().to_string(),
                error: err.to_string(),
            },
        };

        PublishAttempt {
            package,
            version,
            result,
        }
    }

    fn publish_with_retry(
        &self,
        pkg_id: &PackageId,
        version: &Version,
        permit: &ApplyPermit,
    ) -> Result<PublishOutcome, RegistryError> {
        // Treat any is_published error as "unknown — proceed to publish". The
        // pre-check is an optional optimization, not a required gate. Propagating
        // errors here aborts the publish without ever calling publish(), which
        // records a misleading failure (e.g. "rateLimited") that never reached
        // the actual publish step.
        if !self.skip_precheck && self.client.is_published(pkg_id, version).unwrap_or(false) {
            return Ok(PublishOutcome::AlreadyPublished);
        }

        let mut retries = 0usize;
        loop {
            match self.client.publish(pkg_id, version, permit) {
                // Both a fresh publish and a publish-time "already there"
                // classification are done-and-not-an-error: neither should
                // retry, and AlreadyPublished is treated identically to the
                // is_published short-circuit above.
                Ok(outcome @ (PublishOutcome::Published | PublishOutcome::AlreadyPublished)) => return Ok(outcome),
                Err(RegistryError::RateLimited(retry_after)) => {
                    if retry_after > Duration::from_secs(MAX_RETRY_AFTER_SECS) {
                        return Err(RegistryError::RateLimited(retry_after));
                    }
                    retries += 1;
                    if retries >= MAX_RATE_LIMIT_RETRIES {
                        return Err(RegistryError::Other(format!(
                            "rate-limited {MAX_RATE_LIMIT_RETRIES} consecutive times; giving up"
                        )));
                    }
                    self.policy.check_rate_limit(retry_after)?;
                    self.time.sleep(retry_after);
                }
                Err(RegistryError::AuthFailed(err)) => {
                    return Err(RegistryError::AuthFailed(err));
                }
                Err(err) => {
                    return Err(err);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {

    fn permit() -> ApplyPermit {
        ApplyPermit::force_for_tests()
    }
    use super::*;
    use std::sync::Mutex;
    use std::time::SystemTime;

    /// Minimal `DependencyResolver` test double for [`publish_order`]:
    /// packages with no manifests/publish targets, plus a fixed edge list.
    struct TestGraph {
        packages: Vec<callisto_model::Package>,
        edges: Vec<callisto_model::DepEdge>,
    }

    fn test_package(name: &str) -> callisto_model::Package {
        callisto_model::Package {
            id: PackageId::parse(name).unwrap(),
            manifests: vec![],
            changelog: None,
            release_trigger: callisto_model::ReleaseTrigger::Changeset,
            publish_to: vec![],
            tag_template: None,
        }
    }

    fn test_edge(from: &str, to: &str, kind: callisto_model::DepKind) -> callisto_model::DepEdge {
        callisto_model::DepEdge {
            from: PackageId::parse(from).unwrap(),
            to: PackageId::parse(to).unwrap(),
            kind,
            spec: callisto_model::DepSpec::Opaque("*".to_string()),
            from_manifest: std::path::PathBuf::from(format!("{from}/Cargo.toml")),
            inherited: false,
        }
    }

    impl DependencyResolver for TestGraph {
        fn packages(&self) -> impl Iterator<Item = &callisto_model::Package> {
            self.packages.iter()
        }

        fn dependencies_of(&self, id: &PackageId) -> impl Iterator<Item = &callisto_model::DepEdge> {
            self.edges.iter().filter(move |e| &e.from == id)
        }

        fn dependents_of(&self, id: &PackageId) -> impl Iterator<Item = &callisto_model::DepEdge> {
            self.edges.iter().filter(move |e| &e.to == id)
        }
    }

    fn all_ids(graph: &TestGraph) -> HashSet<PackageId> {
        graph.packages.iter().map(|p| p.id.clone()).collect()
    }

    #[test]
    fn publish_order_sequences_a_dev_only_dependency_before_its_dependent() {
        // conventional dev-depends on vcs (test-only), with no Runtime edge
        // between them -- the exact shape of the real bug: publish_order
        // must still put vcs before conventional so cargo publish's own
        // verification build (which needs dev-deps resolvable) succeeds.
        let graph = TestGraph {
            packages: vec![test_package("conventional"), test_package("vcs")],
            edges: vec![test_edge("conventional", "vcs", callisto_model::DepKind::Dev)],
        };

        let order = publish_order(&graph, &all_ids(&graph)).unwrap();
        let vcs_pos = order
            .iter()
            .position(|id| id.name() == "vcs")
            .expect("vcs must be in the order");
        let conventional_pos = order
            .iter()
            .position(|id| id.name() == "conventional")
            .expect("conventional must be in the order");
        assert!(
            vcs_pos < conventional_pos,
            "vcs (dev-dependency) must publish before conventional; got order: {order:?}"
        );
    }

    #[test]
    fn publish_order_tolerates_a_dev_only_cycle_without_hard_failing() {
        // Two packages mutually dev-depending on each other for
        // cross-integration tests -- a legitimate pattern with no Runtime
        // edge between them. Including Dev edges unconditionally would
        // make this a hard Cycle error; publish_order must instead fall
        // back to the cascade-scoped kinds (empty here) and still succeed.
        let graph = TestGraph {
            packages: vec![test_package("pkg-a"), test_package("pkg-b")],
            edges: vec![
                test_edge("pkg-a", "pkg-b", callisto_model::DepKind::Dev),
                test_edge("pkg-b", "pkg-a", callisto_model::DepKind::Dev),
            ],
        };

        let order = publish_order(&graph, &all_ids(&graph));
        assert!(
            order.is_ok(),
            "a dev-only cycle must not hard-fail publish_order; got {order:?}"
        );
        assert_eq!(order.unwrap().len(), 2);
    }

    #[test]
    fn publish_order_still_errors_on_a_real_runtime_cycle() {
        // A genuine Runtime cycle must still be a hard error -- the
        // cascade-scoped fallback is not a general "never fail" escape
        // hatch, only a tolerance for Dev-only cycles.
        let graph = TestGraph {
            packages: vec![test_package("pkg-a"), test_package("pkg-b")],
            edges: vec![
                test_edge("pkg-a", "pkg-b", callisto_model::DepKind::Runtime),
                test_edge("pkg-b", "pkg-a", callisto_model::DepKind::Runtime),
            ],
        };

        let order = publish_order(&graph, &all_ids(&graph));
        assert!(
            matches!(order, Err(GraphError::Cycle { .. })),
            "a real Runtime cycle must still error; got {order:?}"
        );
    }

    #[test]
    fn publish_order_scopes_dev_cycle_exclusion_to_cyclic_pair_only() {
        // pkg-a <-> pkg-b: legitimate Dev-only cycle (cross-integration
        // tests). conventional -Dev-> vcs: a completely unrelated pair, no
        // cycle at all -- exactly the case PUBLISH_ORDERING_KINDS exists to
        // order correctly. Before the SCC-scoped fix, the pkg-a/pkg-b cycle
        // made the global fallback drop Dev edges for the WHOLE subset,
        // silently un-ordering vcs/conventional too.
        let graph = TestGraph {
            packages: vec![
                test_package("pkg-a"),
                test_package("pkg-b"),
                test_package("conventional"),
                test_package("vcs"),
            ],
            edges: vec![
                test_edge("pkg-a", "pkg-b", callisto_model::DepKind::Dev),
                test_edge("pkg-b", "pkg-a", callisto_model::DepKind::Dev),
                test_edge("conventional", "vcs", callisto_model::DepKind::Dev),
            ],
        };

        let order = publish_order(&graph, &all_ids(&graph)).expect("a Dev-only cycle must not hard-fail");
        let vcs_pos = order.iter().position(|id| id.name() == "vcs").unwrap();
        let conventional_pos = order.iter().position(|id| id.name() == "conventional").unwrap();
        assert!(
            vcs_pos < conventional_pos,
            "vcs must still publish before conventional despite the unrelated pkg-a/pkg-b \
             Dev cycle elsewhere; got order: {order:?}"
        );
    }

    #[test]
    fn publish_order_dev_cycle_of_three_packages() {
        // A -Dev-> B -Dev-> C -Dev-> A: a 3-node cyclic component, not just
        // the 2-node pairs the other tests cover. Alongside an unrelated
        // legitimate Dev edge that must still be honoured.
        let graph = TestGraph {
            packages: vec![
                test_package("pkg-a"),
                test_package("pkg-b"),
                test_package("pkg-c"),
                test_package("conventional"),
                test_package("vcs"),
            ],
            edges: vec![
                test_edge("pkg-a", "pkg-b", callisto_model::DepKind::Dev),
                test_edge("pkg-b", "pkg-c", callisto_model::DepKind::Dev),
                test_edge("pkg-c", "pkg-a", callisto_model::DepKind::Dev),
                test_edge("conventional", "vcs", callisto_model::DepKind::Dev),
            ],
        };

        let order = publish_order(&graph, &all_ids(&graph)).expect("a 3-node Dev-only cycle must not hard-fail");
        assert_eq!(order.len(), 5);
        let vcs_pos = order.iter().position(|id| id.name() == "vcs").unwrap();
        let conventional_pos = order.iter().position(|id| id.name() == "conventional").unwrap();
        assert!(
            vcs_pos < conventional_pos,
            "the unrelated Dev edge must still be honoured; got order: {order:?}"
        );
    }

    #[test]
    fn publish_order_mixed_runtime_and_dev_cycle_excludes_only_the_dev_edge() {
        // pkg-a -Runtime-> pkg-b, pkg-b -Dev-> pkg-a: one 2-node cycle built
        // from two DIFFERENT edge kinds. Distinguishes a correct
        // implementation (cyclic_sccs computed over the full
        // PUBLISH_ORDERING_KINDS-inclusive graph) from a subtly wrong one
        // (cyclic_sccs computed over Dev-only edges): under the wrong
        // version, this pair never registers as a cyclic component at all
        // (a lone directed Dev edge isn't a cycle by itself), the Dev edge
        // survives un-excluded, and the final pass still contains both
        // directions -- wrongly erroring even though the cascade-only pass
        // already proved success is achievable.
        let graph = TestGraph {
            packages: vec![test_package("pkg-a"), test_package("pkg-b")],
            edges: vec![
                test_edge("pkg-a", "pkg-b", callisto_model::DepKind::Runtime),
                test_edge("pkg-b", "pkg-a", callisto_model::DepKind::Dev),
            ],
        };

        let order = publish_order(&graph, &all_ids(&graph))
            .expect("a Runtime+Dev mixed cycle must resolve via the surviving Runtime edge");
        let pos_a = order.iter().position(|id| id.name() == "pkg-a").unwrap();
        let pos_b = order.iter().position(|id| id.name() == "pkg-b").unwrap();
        // pkg-a -Runtime-> pkg-b means pkg-a *depends on* pkg-b, so the
        // dependency-first order must publish pkg-b before pkg-a — the
        // surviving Runtime edge, not the excluded Dev edge, determines this.
        assert!(
            pos_b < pos_a,
            "the surviving Runtime edge (pkg-a depends on pkg-b) must determine order, not \
             the excluded Dev edge; got order: {order:?}"
        );
    }

    struct MockRegistryClient {
        published: Mutex<std::collections::HashSet<(PackageId, Version)>>,
        /// Stack of canned responses (popped one per `publish` call). When
        /// exhausted, `publish` defaults to a fresh `Ok(Published)`.
        responses: Mutex<Vec<Result<PublishOutcome, RegistryError>>>,
    }

    impl RegistryClient for MockRegistryClient {
        fn is_published(&self, package: &PackageId, version: &Version) -> Result<bool, RegistryError> {
            let published = self.published.lock().unwrap();
            Ok(published.contains(&(package.clone(), version.clone())))
        }

        fn publish(
            &self,
            package: &PackageId,
            version: &Version,
            _permit: &ApplyPermit,
        ) -> Result<PublishOutcome, RegistryError> {
            let mut responses = self.responses.lock().unwrap();
            let outcome = match responses.pop() {
                Some(res) => res?,
                None => PublishOutcome::Published,
            };

            if matches!(outcome, PublishOutcome::Published) {
                let mut published = self.published.lock().unwrap();
                published.insert((package.clone(), version.clone()));
            }
            Ok(outcome)
        }
    }

    struct MockRateLimitPolicy;
    impl RateLimitPolicy for MockRateLimitPolicy {
        fn check_rate_limit(&self, _retry_after: Duration) -> Result<(), RegistryError> {
            Ok(())
        }
    }

    struct MockTimeProvider {
        time: Mutex<SystemTime>,
    }

    impl TimeProvider for MockTimeProvider {
        fn now(&self) -> SystemTime {
            *self.time.lock().unwrap()
        }

        fn sleep(&self, duration: Duration) {
            let mut time = self.time.lock().unwrap();
            *time += duration;
        }
    }

    fn create_test_plan() -> callisto_model::PublishPlan {
        callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![callisto_model::CratePublish {
                name: "test-crate".to_string(),
                version: Version::parse("1.0.0", callisto_model::VersionGrammar::SemVer).unwrap(),
                publish_to: callisto_model::RegistryKey(callisto_model::RegistryKey::CRATES_IO.to_string()),
                registry: None,
                package_dir: None,
            }],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        }
    }

    fn pypi_publish_entry(name: &str) -> callisto_model::PypiPublish {
        callisto_model::PypiPublish {
            name: name.to_string(),
            version: v100(),
            publish_to: callisto_model::RegistryKey(callisto_model::RegistryKey::PYPI.to_string()),
            package_dir: std::path::PathBuf::new(),
            index: None,
        }
    }

    #[test]
    fn test_publish_success() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let report = orchestrator.execute(&create_test_plan(), &permit());
        assert_eq!(report.attempts.len(), 1);
        assert!(matches!(report.attempts[0].result, PublishAttemptResult::Published));
        assert_eq!(orchestrator.time.now(), SystemTime::UNIX_EPOCH);
    }

    /// A main npm package must not be published if any platform package it
    /// depends on (per `depends_on_platforms`, computed from the real
    /// dependency graph in `plan_publish`) failed to publish in the same
    /// run -- publishing it anyway would ship an `optionalDependencies`
    /// reference to a version that was never actually uploaded. The main
    /// package's own registry client is never even called: the skip must
    /// happen before any publish attempt, not as a post-hoc failure.
    #[test]
    fn npm_main_package_is_skipped_when_its_platform_dependency_fails() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            // Popped LIFO. Only one real `publish` call is expected (the
            // platform package) -- the main package must be skipped before
            // ever reaching the client, so it must not need a second entry.
            responses: Mutex::new(vec![Err(RegistryError::Other("registry rejected upload".to_string()))]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![],
            npm_platform_packages: vec![callisto_model::NpmPublish {
                name: "my-cli-linux-x64".to_string(),
                version: v100(),
                publish_to: callisto_model::RegistryKey(callisto_model::RegistryKey::NPM.to_string()),
                package_dir: std::path::PathBuf::new(),
                registry: None,
                tag: None,
                access: None,
            }],
            npm_main_packages: vec![callisto_model::NpmMainPublish {
                name: "my-cli".to_string(),
                version: v100(),
                publish_to: callisto_model::RegistryKey(callisto_model::RegistryKey::NPM.to_string()),
                package_dir: std::path::PathBuf::new(),
                registry: None,
                tag: None,
                access: None,
                depends_on_platforms: vec!["my-cli-linux-x64".to_string()],
            }],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };

        let report = orchestrator.execute(&plan, &permit());

        assert_eq!(report.attempts.len(), 2, "both packages must appear in the report");

        let platform_attempt = report
            .attempts
            .iter()
            .find(|a| a.package.name() == "my-cli-linux-x64")
            .expect("platform package attempt must be present");
        assert!(
            platform_attempt.result.is_failure(),
            "platform package attempt should reflect the real registry failure, got: {:?}",
            platform_attempt.result
        );

        let main_attempt = report
            .attempts
            .iter()
            .find(|a| a.package.name() == "my-cli")
            .expect("main package attempt must be present");
        assert!(
            main_attempt.result.is_failure(),
            "main package must be recorded as failed (skipped), not silently published, got: {:?}",
            main_attempt.result
        );
        if let PublishAttemptResult::Failed { kind, error } = &main_attempt.result {
            assert_eq!(kind, "dependencyFailed", "got kind: {kind}");
            assert!(
                error.contains("my-cli-linux-x64"),
                "error message must name the failed platform dependency, got: {error}"
            );
        }
        assert!(report.has_failures());
    }

    /// A failed Cargo crate must never be mistaken for a failed npm platform
    /// dependency just because they share a bare name -- `depends_on_platforms`
    /// is an npm-only construct (`optionalDependencies`), and `PackageId`'s
    /// bare `.name()` carries no ecosystem information on its own. A Cargo
    /// crate named identically to an npm platform package that actually
    /// succeeded must not cause the dependent npm main package to be skipped.
    #[test]
    fn cargo_crate_failure_does_not_false_positive_match_an_npm_platform_dependency_of_the_same_name() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            // Popped LIFO: publish() is called for rust_crates first, then
            // npm_platform_packages, then npm_main_packages. Push responses
            // in reverse: main package publish succeeds (default, no entry
            // needed), platform package publish succeeds (default), Cargo
            // crate publish fails.
            responses: Mutex::new(vec![Err(RegistryError::Other("crates.io rejected upload".to_string()))]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![callisto_model::CratePublish {
                // Same bare name as the npm platform package below, deliberately.
                name: "my-cli-linux-x64".to_string(),
                version: v100(),
                publish_to: callisto_model::RegistryKey(callisto_model::RegistryKey::CRATES_IO.to_string()),
                registry: None,
                package_dir: None,
            }],
            npm_platform_packages: vec![callisto_model::NpmPublish {
                name: "my-cli-linux-x64".to_string(),
                version: v100(),
                publish_to: callisto_model::RegistryKey(callisto_model::RegistryKey::NPM.to_string()),
                package_dir: std::path::PathBuf::new(),
                registry: None,
                tag: None,
                access: None,
            }],
            npm_main_packages: vec![callisto_model::NpmMainPublish {
                name: "my-cli".to_string(),
                version: v100(),
                publish_to: callisto_model::RegistryKey(callisto_model::RegistryKey::NPM.to_string()),
                package_dir: std::path::PathBuf::new(),
                registry: None,
                tag: None,
                access: None,
                depends_on_platforms: vec!["my-cli-linux-x64".to_string()],
            }],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };

        let report = orchestrator.execute(&plan, &permit());

        let cargo_attempt = report
            .attempts
            .iter()
            .find(|a| a.package.ecosystem() == Some(callisto_model::Ecosystem::Cargo))
            .expect("cargo crate attempt must be present");
        assert!(
            cargo_attempt.result.is_failure(),
            "the cargo crate publish must genuinely fail"
        );

        let main_attempt = report
            .attempts
            .iter()
            .find(|a| a.package.ecosystem() == Some(callisto_model::Ecosystem::Npm) && a.package.name() == "my-cli")
            .expect("main package attempt must be present");
        assert!(
            !main_attempt.result.is_failure(),
            "the npm main package must publish normally -- its real npm platform dependency succeeded; \
             a same-named Cargo crate failing in a different ecosystem must not skip it, got: {:?}",
            main_attempt.result
        );
    }

    #[test]
    fn test_publish_already_published_is_not_an_error_and_does_not_retry() {
        // publish() itself reporting AlreadyPublished (rather than the
        // is_published pre-check short-circuiting) must be treated the same
        // way: success, no retry loop, no sleep.
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![Ok(PublishOutcome::AlreadyPublished)]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let report = orchestrator.execute(&create_test_plan(), &permit());
        assert_eq!(report.attempts.len(), 1);
        assert!(matches!(
            report.attempts[0].result,
            PublishAttemptResult::AlreadyPublished
        ));
        assert_eq!(orchestrator.time.now(), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn test_publish_rate_limit_retry() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![Err(RegistryError::RateLimited(Duration::from_secs(60)))]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let report = orchestrator.execute(&create_test_plan(), &permit());
        assert_eq!(report.attempts.len(), 1);
        assert!(matches!(report.attempts[0].result, PublishAttemptResult::Published));
        assert_eq!(
            orchestrator.time.now(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(60)
        );
    }

    #[test]
    fn test_publish_rate_limit_exceeds_600s() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![Err(RegistryError::RateLimited(Duration::from_secs(
                MAX_RETRY_AFTER_SECS + 1,
            )))]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let report = orchestrator.execute(&create_test_plan(), &permit());
        assert_eq!(report.attempts.len(), 1);
        match &report.attempts[0].result {
            PublishAttemptResult::Failed { error, .. } => {
                assert!(error.contains("601"));
            }
            other => panic!("expected Failed outcome, got {other:?}"),
        }
    }

    /// After MAX_RATE_LIMIT_RETRIES consecutive 429 responses the orchestrator
    /// must give up and record a failure rather than retrying indefinitely.
    /// Without an iteration cap the loop would spin through all responses and
    /// then succeed (MockRegistryClient returns Published when exhausted),
    /// so this test would incorrectly pass as Published.
    ///
    /// The cap must fire after exactly MAX_RATE_LIMIT_RETRIES responses — not
    /// MAX_RATE_LIMIT_RETRIES + 1. The constant names the limit; the loop must
    /// honour it without an off-by-one.
    #[test]
    fn test_publish_rate_limit_cap_fires_at_exactly_max_retries() {
        // Exactly MAX_RATE_LIMIT_RETRIES rate-limit responses — the cap must
        // fire on the Nth response, not require an (N+1)th attempt first.
        let rate_limits: Vec<_> = (0..MAX_RATE_LIMIT_RETRIES)
            .map(|_| Err(RegistryError::RateLimited(Duration::from_secs(1))))
            .collect();
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(rate_limits),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let report = orchestrator.execute(&create_test_plan(), &permit());
        assert_eq!(report.attempts.len(), 1);
        assert!(
            matches!(report.attempts[0].result, PublishAttemptResult::Failed { .. }),
            "cap must fire after exactly MAX_RATE_LIMIT_RETRIES ({MAX_RATE_LIMIT_RETRIES}) \
             responses; got: {:?}",
            report.attempts[0].result
        );
    }

    #[test]
    fn test_publish_rate_limit_cap_aborts_after_max_retries() {
        // Push more rate-limit responses than the cap allows.
        let many_rate_limits: Vec<_> = (0..=MAX_RATE_LIMIT_RETRIES)
            .map(|_| Err(RegistryError::RateLimited(Duration::from_secs(1))))
            .collect();
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(many_rate_limits),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let report = orchestrator.execute(&create_test_plan(), &permit());
        assert_eq!(report.attempts.len(), 1);
        match &report.attempts[0].result {
            PublishAttemptResult::Failed { error, .. } => {
                assert!(
                    error.to_lowercase().contains("rate") || error.to_lowercase().contains("retry"),
                    "failure message should mention rate-limit or retry; got: {error}"
                );
            }
            other => panic!("expected Failed after retry cap, got {other:?}"),
        }
    }

    #[test]
    fn test_publish_auth_fail_fast() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![Err(RegistryError::AuthFailed("Invalid token".to_string()))]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let report = orchestrator.execute(&create_test_plan(), &permit());
        assert_eq!(report.attempts.len(), 1);
        match &report.attempts[0].result {
            PublishAttemptResult::Failed { error, .. } => {
                assert!(error.contains("Invalid token"));
            }
            other => panic!("expected Failed outcome, got {other:?}"),
        }
    }

    fn v100() -> Version {
        Version::parse("1.0.0", callisto_model::VersionGrammar::SemVer).unwrap()
    }

    fn crate_publish(name: &str) -> callisto_model::CratePublish {
        callisto_model::CratePublish {
            name: name.to_string(),
            version: v100(),
            publish_to: callisto_model::RegistryKey(callisto_model::RegistryKey::CRATES_IO.to_string()),
            registry: None,
            package_dir: None,
        }
    }

    #[test]
    fn test_publish_execute_reports_distinct_per_package_outcomes() {
        // crate-a publishes fresh, crate-b is already on the index, crate-c
        // fails outright. The report returned by `execute` must surface all
        // three distinctly instead of discarding per-package results.
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![
                Err(RegistryError::AuthFailed("bad token".to_string())), // crate-c
                Ok(PublishOutcome::AlreadyPublished),                    // crate-b
                Ok(PublishOutcome::Published),                           // crate-a
            ]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![
                crate_publish("crate-a"),
                crate_publish("crate-b"),
                crate_publish("crate-c"),
            ],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };

        let report = orchestrator.execute(&plan, &permit());

        assert_eq!(report.attempts.len(), 3);
        assert_eq!(report.attempts[0].package.name(), "crate-a");
        assert!(matches!(
            report.attempts[0].result,
            callisto_model::PublishAttemptResult::Published
        ));
        assert_eq!(report.attempts[1].package.name(), "crate-b");
        assert!(matches!(
            report.attempts[1].result,
            callisto_model::PublishAttemptResult::AlreadyPublished
        ));
        assert_eq!(report.attempts[2].package.name(), "crate-c");
        match &report.attempts[2].result {
            callisto_model::PublishAttemptResult::Failed { error, .. } => {
                assert!(error.contains("bad token"));
            }
            other => panic!("expected Failed outcome for crate-c, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_ttl() {
        assert_eq!(
            PublishOrchestrator::<MockRegistryClient, MockRateLimitPolicy, MockTimeProvider>::parse_http_429_ttl("120"),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            PublishOrchestrator::<MockRegistryClient, MockRateLimitPolicy, MockTimeProvider>::parse_http_429_ttl(
                "invalid"
            ),
            None
        );
    }

    // ---------------------------------------------------------------- pypi

    /// `execute` must iterate `pypi_packages` and submit each one to the
    /// registry client under `Ecosystem::Pypi`, recording a per-package
    /// attempt just as it does for Cargo and npm packages.
    #[test]
    fn test_execute_dispatches_pypi_packages() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![
                Ok(PublishOutcome::AlreadyPublished), // pypi-b
                Ok(PublishOutcome::Published),        // pypi-a
            ]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![pypi_publish_entry("pypi-a"), pypi_publish_entry("pypi-b")],
            releases: vec![],
            diagnostics: vec![],
        };

        let report = orchestrator.execute(&plan, &permit());

        assert_eq!(report.attempts.len(), 2, "expected one attempt per pypi package");
        assert_eq!(report.attempts[0].package.name(), "pypi-a");
        assert!(
            matches!(report.attempts[0].result, PublishAttemptResult::Published),
            "pypi-a should be Published"
        );
        assert_eq!(report.attempts[1].package.name(), "pypi-b");
        assert!(
            matches!(report.attempts[1].result, PublishAttemptResult::AlreadyPublished),
            "pypi-b should be AlreadyPublished"
        );
    }

    /// npm platform packages must be published before npm main packages because
    /// main packages list platforms in their `optionalDependencies` and the
    /// registry resolver requires platforms to already exist.
    #[test]
    fn test_npm_platforms_published_before_mains() {
        struct RecordingClient {
            order: Mutex<Vec<String>>,
        }

        impl RegistryClient for RecordingClient {
            fn is_published(&self, _pkg: &PackageId, _ver: &Version) -> Result<bool, RegistryError> {
                Ok(false)
            }

            fn publish(
                &self,
                pkg: &PackageId,
                _ver: &Version,
                _permit: &ApplyPermit,
            ) -> Result<PublishOutcome, RegistryError> {
                self.order.lock().unwrap().push(pkg.name().to_string());
                Ok(PublishOutcome::Published)
            }
        }

        let client = RecordingClient {
            order: Mutex::new(Vec::new()),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let npm_version = v100();
        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![],
            npm_platform_packages: vec![callisto_model::NpmPublish {
                name: "platform-linux".to_string(),
                version: npm_version.clone(),
                publish_to: callisto_model::RegistryKey(callisto_model::RegistryKey::NPM.to_string()),
                package_dir: std::path::PathBuf::new(),
                registry: None,
                tag: None,
                access: None,
            }],
            npm_main_packages: vec![callisto_model::NpmMainPublish {
                name: "main-package".to_string(),
                version: npm_version.clone(),
                publish_to: callisto_model::RegistryKey(callisto_model::RegistryKey::NPM.to_string()),
                package_dir: std::path::PathBuf::new(),
                registry: None,
                tag: None,
                access: None,
                depends_on_platforms: vec!["platform-linux".to_string()],
            }],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };

        drop(orchestrator.execute(&plan, &permit()));

        let order = orchestrator.client.order.lock().unwrap();
        let platform_pos = order
            .iter()
            .position(|n| n == "platform-linux")
            .expect("platform-linux was not published");
        let main_pos = order
            .iter()
            .position(|n| n == "main-package")
            .expect("main-package was not published");
        assert!(
            platform_pos < main_pos,
            "platform packages must be published before main packages, but got order: {order:?}"
        );
    }

    /// An auth failure on a PyPI package must be recorded as `Failed` and
    /// must not propagate to abort remaining packages in the same execute run.
    #[test]
    fn test_execute_pypi_auth_failure_is_recorded_not_propagated() {
        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![Err(RegistryError::AuthFailed("invalid PyPI token".to_string()))]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![pypi_publish_entry("bad-pkg")],
            releases: vec![],
            diagnostics: vec![],
        };

        let report = orchestrator.execute(&plan, &permit());

        assert_eq!(report.attempts.len(), 1);
        match &report.attempts[0].result {
            PublishAttemptResult::Failed { error, .. } => {
                assert!(error.contains("invalid PyPI token"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    /// PUB-005: the orchestrator must invoke a progress callback before each
    /// package attempt so that the CLI layer (or any other caller) can report
    /// "Publishing pkg@version…" lines in real time rather than printing
    /// nothing until the entire batch completes.
    #[test]
    fn progress_callback_is_called_once_per_package_before_attempt() {
        use std::sync::Arc;

        let messages: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let messages_clone = Arc::clone(&messages);

        let client = MockRegistryClient {
            published: Mutex::new(std::collections::HashSet::new()),
            responses: Mutex::new(vec![Ok(PublishOutcome::Published), Ok(PublishOutcome::Published)]),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(std::time::SystemTime::UNIX_EPOCH),
        };

        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![crate_publish("crate-a"), crate_publish("crate-b")],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };

        let orchestrator = PublishOrchestrator::new(client, policy, time).with_progress(move |msg: String| {
            messages_clone.lock().unwrap().push(msg);
        });

        let _report = orchestrator.execute(&plan, &permit());

        let recorded = messages.lock().unwrap().clone();
        assert_eq!(
            recorded.len(),
            2,
            "expected 2 progress messages (one per package); got: {recorded:?}"
        );
        assert!(
            recorded[0].contains("crate-a"),
            "first progress message must mention crate-a; got: {:?}",
            recorded[0]
        );
        assert!(
            recorded[1].contains("crate-b"),
            "second progress message must mention crate-b; got: {:?}",
            recorded[1]
        );
    }

    /// When `is_published` returns an error (e.g. a transient rate-limit on the
    /// npm pre-check), the orchestrator must treat the package as "not yet
    /// published" and proceed to call `publish()`. Propagating the error aborts
    /// the publish entirely without ever calling the actual publish command —
    /// the user sees a misleading "rate-limited" failure when in reality no
    /// publish was attempted at all.
    #[test]
    fn is_published_error_is_ignored_and_publish_proceeds() {
        struct FlakyPreCheckClient {
            publish_called: Mutex<bool>,
        }

        impl RegistryClient for FlakyPreCheckClient {
            fn is_published(&self, _pkg: &PackageId, _ver: &Version) -> Result<bool, RegistryError> {
                Err(RegistryError::RateLimited(Duration::from_secs(5)))
            }

            fn publish(
                &self,
                _pkg: &PackageId,
                _ver: &Version,
                _permit: &ApplyPermit,
            ) -> Result<PublishOutcome, RegistryError> {
                *self.publish_called.lock().unwrap() = true;
                Ok(PublishOutcome::Published)
            }
        }

        let client = FlakyPreCheckClient {
            publish_called: Mutex::new(false),
        };
        let policy = MockRateLimitPolicy;
        let time = MockTimeProvider {
            time: Mutex::new(SystemTime::UNIX_EPOCH),
        };
        let orchestrator = PublishOrchestrator::new(client, policy, time);

        let report = orchestrator.execute(&create_test_plan(), &permit());

        assert!(
            *orchestrator.client.publish_called.lock().unwrap(),
            "publish() must be called even when is_published() returns an error"
        );
        assert_eq!(report.attempts.len(), 1, "one attempt must be recorded for the package");
        assert!(
            matches!(report.attempts[0].result, PublishAttemptResult::Published),
            "result must be Published when is_published() errs and publish() succeeds; \
             got: {:?}",
            report.attempts[0].result
        );
    }

    /// Records how many times each `RegistryClient` method was called, so
    /// tests can assert the `is_published` pre-check was (or wasn't) skipped
    /// without depending on `publish()`'s outcome to infer it indirectly.
    struct PrecheckTrackingClient {
        is_published_calls: std::sync::atomic::AtomicUsize,
        publish_calls: std::sync::atomic::AtomicUsize,
        already_published: bool,
        publish_outcome: PublishOutcome,
    }

    impl RegistryClient for PrecheckTrackingClient {
        fn is_published(&self, _pkg: &PackageId, _ver: &Version) -> Result<bool, RegistryError> {
            self.is_published_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.already_published)
        }

        fn publish(
            &self,
            _pkg: &PackageId,
            _ver: &Version,
            _permit: &ApplyPermit,
        ) -> Result<PublishOutcome, RegistryError> {
            self.publish_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.publish_outcome)
        }
    }

    /// `with_skip_precheck(true)` must skip the `is_published` call entirely
    /// and publish directly.
    #[test]
    fn skip_precheck_true_skips_is_published_and_publishes_directly() {
        let client = PrecheckTrackingClient {
            is_published_calls: std::sync::atomic::AtomicUsize::new(0),
            publish_calls: std::sync::atomic::AtomicUsize::new(0),
            already_published: false,
            publish_outcome: PublishOutcome::Published,
        };
        let orchestrator = PublishOrchestrator::new(
            client,
            MockRateLimitPolicy,
            MockTimeProvider {
                time: Mutex::new(SystemTime::UNIX_EPOCH),
            },
        )
        .with_skip_precheck(true);

        let report = orchestrator.execute(&create_test_plan(), &permit());

        assert_eq!(
            orchestrator
                .client
                .is_published_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "is_published must not be called when skip_precheck is true"
        );
        assert_eq!(
            orchestrator
                .client
                .publish_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert!(matches!(report.attempts[0].result, PublishAttemptResult::Published));
    }

    /// Even with the pre-check skipped, a registry that classifies the
    /// publish call itself as "already there" (e.g. npm's own
    /// EPUBLISHCONFLICT/E409 handling) must still surface as
    /// AlreadyPublished, not a failure.
    #[test]
    fn skip_precheck_true_still_classifies_already_published_via_publish_call() {
        let client = PrecheckTrackingClient {
            is_published_calls: std::sync::atomic::AtomicUsize::new(0),
            publish_calls: std::sync::atomic::AtomicUsize::new(0),
            already_published: false,
            publish_outcome: PublishOutcome::AlreadyPublished,
        };
        let orchestrator = PublishOrchestrator::new(
            client,
            MockRateLimitPolicy,
            MockTimeProvider {
                time: Mutex::new(SystemTime::UNIX_EPOCH),
            },
        )
        .with_skip_precheck(true);

        let report = orchestrator.execute(&create_test_plan(), &permit());

        assert_eq!(
            orchestrator
                .client
                .is_published_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert!(matches!(
            report.attempts[0].result,
            PublishAttemptResult::AlreadyPublished
        ));
    }

    /// The default (`skip_precheck` unset) must preserve today's behaviour
    /// exactly: `is_published` runs, and a positive result short-circuits
    /// before `publish()` is ever called.
    #[test]
    fn skip_precheck_default_false_preserves_existing_precheck_behavior() {
        let client = PrecheckTrackingClient {
            is_published_calls: std::sync::atomic::AtomicUsize::new(0),
            publish_calls: std::sync::atomic::AtomicUsize::new(0),
            already_published: true,
            publish_outcome: PublishOutcome::Published,
        };
        let orchestrator = PublishOrchestrator::new(
            client,
            MockRateLimitPolicy,
            MockTimeProvider {
                time: Mutex::new(SystemTime::UNIX_EPOCH),
            },
        );

        let report = orchestrator.execute(&create_test_plan(), &permit());

        assert_eq!(
            orchestrator
                .client
                .is_published_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            orchestrator
                .client
                .publish_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "publish() must not be called when is_published() already returned true"
        );
        assert!(matches!(
            report.attempts[0].result,
            PublishAttemptResult::AlreadyPublished
        ));
    }

    fn attempt(ecosystem: Ecosystem, name: &str, result: PublishAttemptResult) -> PublishAttempt {
        PublishAttempt {
            package: PackageId::Prefixed {
                ecosystem,
                name: name.to_string(),
            },
            version: v100(),
            result,
        }
    }

    fn failed_attempt(ecosystem: Ecosystem, name: &str) -> PublishAttempt {
        attempt(
            ecosystem,
            name,
            PublishAttemptResult::Failed {
                kind: "other".to_string(),
                error: "boom".to_string(),
            },
        )
    }

    fn published_attempt(ecosystem: Ecosystem, name: &str) -> PublishAttempt {
        attempt(ecosystem, name, PublishAttemptResult::Published)
    }

    #[test]
    fn filter_plan_by_report_drops_a_failed_rust_crate() {
        let mut plan = create_test_plan();
        plan.rust_crates.push(callisto_model::CratePublish {
            name: "other-crate".to_string(),
            version: v100(),
            publish_to: RegistryKey(RegistryKey::CRATES_IO.to_string()),
            registry: None,
            package_dir: None,
        });

        let report = PublishReport {
            schema_version: SCHEMA_VERSION,
            attempts: vec![
                published_attempt(Ecosystem::Cargo, "test-crate"),
                failed_attempt(Ecosystem::Cargo, "other-crate"),
            ],
            diagnostics: vec![],
        };

        let filtered = filter_plan_by_report(&plan, &report);
        let names: Vec<&str> = filtered.rust_crates.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["test-crate"],
            "the failed crate must be dropped; kept: {names:?}"
        );
    }

    #[test]
    fn filter_plan_by_report_keeps_already_published_entries() {
        let plan = create_test_plan();
        let report = PublishReport {
            schema_version: SCHEMA_VERSION,
            attempts: vec![attempt(
                Ecosystem::Cargo,
                "test-crate",
                PublishAttemptResult::AlreadyPublished,
            )],
            diagnostics: vec![],
        };

        let filtered = filter_plan_by_report(&plan, &report);
        assert_eq!(
            filtered.rust_crates.len(),
            1,
            "AlreadyPublished must count as kept, not failed"
        );
    }

    #[test]
    fn filter_plan_by_report_drops_entry_with_no_matching_attempt_at_all() {
        let plan = create_test_plan();
        let report = PublishReport {
            schema_version: SCHEMA_VERSION,
            attempts: vec![],
            diagnostics: vec![],
        };

        let filtered = filter_plan_by_report(&plan, &report);
        assert!(
            filtered.rust_crates.is_empty(),
            "a plan entry the report never attempted at all must not be kept by default"
        );
    }

    #[test]
    fn filter_plan_by_report_drops_release_when_one_of_its_multiple_ecosystem_targets_failed() {
        let mut plan = create_test_plan();
        plan.releases.push(callisto_model::ReleaseEntry {
            package: PackageId::Bare("test-crate".to_string()),
            tag_name: callisto_model::TagName("test-crate@1.0.0".to_string()),
            sha: callisto_model::CommitSha::parse(&"a".repeat(40)).unwrap(),
            changelog_section: None,
            is_prerelease: false,
        });

        // test-crate published fine on Cargo, but also (hypothetically)
        // targets npm under the same bare name and that target failed.
        let report = PublishReport {
            schema_version: SCHEMA_VERSION,
            attempts: vec![
                published_attempt(Ecosystem::Cargo, "test-crate"),
                failed_attempt(Ecosystem::Npm, "test-crate"),
            ],
            diagnostics: vec![],
        };

        let filtered = filter_plan_by_report(&plan, &report);
        assert!(
            filtered.releases.is_empty(),
            "a release must be dropped when ANY of its targets failed, even if another target succeeded"
        );
    }

    #[test]
    fn filter_plan_by_report_keeps_release_when_all_its_targets_succeeded() {
        let mut plan = create_test_plan();
        plan.releases.push(callisto_model::ReleaseEntry {
            package: PackageId::Bare("test-crate".to_string()),
            tag_name: callisto_model::TagName("test-crate@1.0.0".to_string()),
            sha: callisto_model::CommitSha::parse(&"a".repeat(40)).unwrap(),
            changelog_section: None,
            is_prerelease: false,
        });

        let report = PublishReport {
            schema_version: SCHEMA_VERSION,
            attempts: vec![published_attempt(Ecosystem::Cargo, "test-crate")],
            diagnostics: vec![],
        };

        let filtered = filter_plan_by_report(&plan, &report);
        assert_eq!(
            filtered.releases.len(),
            1,
            "a release whose only target succeeded must be kept"
        );
    }

    /// A same-named Cargo crate failure must not drop an unrelated npm
    /// package's release entry -- mirrors the ecosystem-scoping bug already
    /// fixed once in this file for `depends_on_platforms`.
    #[test]
    fn filter_plan_by_report_does_not_false_positive_match_release_by_bare_name_across_unrelated_ecosystem_packages() {
        let mut plan = create_test_plan(); // rust_crates: ["test-crate"]
        plan.npm_main_packages.push(NpmMainPublish {
            name: "test-crate".to_string(),
            version: v100(),
            publish_to: RegistryKey(RegistryKey::NPM.to_string()),
            package_dir: std::path::PathBuf::new(),
            registry: None,
            tag: None,
            access: None,
            depends_on_platforms: vec![],
        });
        plan.releases.push(callisto_model::ReleaseEntry {
            package: PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name: "test-crate".to_string(),
            },
            tag_name: callisto_model::TagName("npm-test-crate@1.0.0".to_string()),
            sha: callisto_model::CommitSha::parse(&"a".repeat(40)).unwrap(),
            changelog_section: None,
            is_prerelease: false,
        });

        // The Cargo crate fails; the unrelated npm package of the same bare
        // name succeeds.
        let report = PublishReport {
            schema_version: SCHEMA_VERSION,
            attempts: vec![
                failed_attempt(Ecosystem::Cargo, "test-crate"),
                published_attempt(Ecosystem::Npm, "test-crate"),
            ],
            diagnostics: vec![],
        };

        let filtered = filter_plan_by_report(&plan, &report);
        assert_eq!(
            filtered.releases.len(),
            1,
            "the npm release must survive the unrelated Cargo crate's failure"
        );
        assert_eq!(filtered.npm_main_packages.len(), 1);
        assert!(
            filtered.rust_crates.is_empty(),
            "the failed Cargo crate itself must still be dropped"
        );
    }
}
