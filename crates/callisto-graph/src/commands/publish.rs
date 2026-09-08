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
/// `cargo publish` (run without `--no-verify`, see `registry_argv.rs`)
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
/// constructs at publish time (not `resolve_entry_id`'s bare-vs-prefixed
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

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::{PublishAttempt, PublishAttemptResult, PublishReport, Version};

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

    fn v100() -> Version {
        Version::parse("1.0.0", callisto_model::VersionGrammar::SemVer).unwrap()
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
            tag_name: callisto_model::TagName::parse("test-crate@1.0.0").unwrap(),
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
            tag_name: callisto_model::TagName::parse("test-crate@1.0.0").unwrap(),
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
            tag_name: callisto_model::TagName::parse("npm-test-crate@1.0.0").unwrap(),
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
