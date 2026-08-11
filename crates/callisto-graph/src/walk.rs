use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use callisto_manifests::{
    detect_npm_workspace_kind, Manifest, OpenContext, WorkspaceCargoResolver,
};
use callisto_model::{
    CommandRunner, DepEdge, Ecosystem, ManifestDecl, ManifestFormat, ManifestRole, Package,
    PackageId, PublishTarget, ReleaseTrigger,
};

use crate::config::ResolvedConfig;
use crate::crosscheck::crosscheck_declared_edges;
use crate::error::GraphError;
use crate::identity::IdentityIndex;
use crate::locate::ProjectLocator;
use crate::manifest_cache::open_cached;
use crate::resolver::ManifestWalkResolver;

impl ManifestWalkResolver {
    pub fn build<L: ProjectLocator, R: CommandRunner>(
        root: &Path,
        locator: &L,
        _runner: &R,
        cfg: &ResolvedConfig,
        manifest_cache: &RefCell<BTreeMap<PathBuf, Arc<dyn Manifest>>>,
    ) -> Result<Self, GraphError> {
        let projects = locator.projects()?;

        let cargo_workspace = if root.join("Cargo.toml").exists() {
            if let Ok(resolver) = WorkspaceCargoResolver::load(&root.join("Cargo.toml")) {
                resolver.inheritance().ok().map(Arc::new)
            } else {
                None
            }
        } else {
            None
        };

        let npm_workspace_kind = detect_npm_workspace_kind(root).ok().flatten();

        let ctx = OpenContext {
            workspace_root: root,
            cargo_workspace,
            npm_workspace_kind,
        };

        let mut package_manifest_decls: BTreeMap<PackageId, (PathBuf, Vec<ManifestDecl>)> =
            BTreeMap::new();
        let mut index = IdentityIndex::default();
        let mut diagnostics = Vec::new();

        // Use the identity already resolved by the locator (`proj.id`) rather
        // than re-reading manifests through `IdentityResolver::resolve`.
        // The locator (e.g. `IgnoreWalkLocator`) already parsed each manifest
        // to discover the project, so re-resolving from scratch is redundant
        // and fragile — in particular, `IdentityResolver` historically had no
        // `Ecosystem::Pypi` arm and would crash for Python projects.
        let mut by_path: BTreeMap<PathBuf, Vec<(Ecosystem, PackageId)>> = BTreeMap::new();
        for proj in &projects {
            by_path
                .entry(proj.path.clone())
                .or_default()
                .push((proj.ecosystem, proj.id.clone()));
        }

        for (rel_path, mut list) in by_path {
            // Explicit precedence: Cargo (0) > Npm (1) > Pypi (2) > others.
            // Do NOT rely on enum discriminant order -- sort by this named
            // priority function so the precedence survives future variant
            // additions to `Ecosystem`.
            list.sort_by_key(|a| ecosystem_primary_priority(a.0));
            let primary_id = list[0].1.clone();

            index
                .bare
                .insert(primary_id.name().to_string(), primary_id.clone());

            let mut decls = Vec::new();
            for (eco, _id) in &list {
                let (fmt, filename) = match eco {
                    Ecosystem::Cargo => (ManifestFormat::CargoToml, "Cargo.toml"),
                    Ecosystem::Npm => (ManifestFormat::PackageJson, "package.json"),
                    Ecosystem::Pypi => (ManifestFormat::PyprojectToml, "pyproject.toml"),
                    _ => (ManifestFormat::PackageJson, "package.json"),
                };
                let manifest_rel = rel_path.join(filename);
                if let Ok(decl) =
                    ManifestDecl::new(manifest_rel.clone(), ManifestRole::Canonical, fmt)
                {
                    decls.push(decl);
                }
                // For napi platform packages (os + cpu constraints in package.json),
                // also push a Platform-role decl so plan_publish can route them
                // into npm_platform_packages instead of npm_main_packages.
                if *eco == Ecosystem::Npm {
                    if let ManifestRole::Platform {
                        platform,
                        arch,
                        abi,
                    } = detect_npm_role(&root.join(&manifest_rel))
                    {
                        if let Ok(platform_decl) = ManifestDecl::new(
                            manifest_rel,
                            ManifestRole::Platform {
                                platform,
                                arch,
                                abi,
                            },
                            fmt,
                        ) {
                            decls.push(platform_decl);
                        }
                    }
                }
                index
                    .native
                    .insert((*eco, primary_id.name().to_string()), primary_id.clone());
            }

            if let Some((existing_path, _)) =
                package_manifest_decls.insert(primary_id.clone(), (rel_path.clone(), decls))
            {
                return Err(GraphError::DuplicatePackage {
                    id: primary_id,
                    paths: vec![existing_path, rel_path],
                });
            }
        }

        let mut packages = BTreeMap::new();
        for (id, (rel_path, decls)) in package_manifest_decls {
            let ch_path = rel_path.join("CHANGELOG.md");
            let mut publish_to = Vec::new();
            for decl in &decls {
                if let Ok(editor) = open_cached(manifest_cache, decl, &ctx) {
                    for target in editor.publish_targets() {
                        if target != PublishTarget::None && !publish_to.contains(&target) {
                            publish_to.push(target);
                        }
                    }
                }
            }
            if publish_to.is_empty() {
                publish_to.push(PublishTarget::None);
            }

            // Two-pass specificity search for [[package]] rules (SPEC-002 AC-1/2/3).
            // Pass 1: find the first Prefixed rule (pattern.ecosystem().is_some())
            //         that matches this package's ID. Prefixed rules always win
            //         over Bare rules regardless of declaration order in callisto.toml.
            // Pass 2: only if pass 1 found nothing, find the first Bare rule
            //         (any rule, since no Prefixed rule matched, the first match
            //          is necessarily Bare) that matches this package's ID.
            // Within each pass, first-match-wins (TOML declaration order) applies.
            let pkg_override = cfg
                .packages
                .iter()
                .find(|(pattern, _)| pattern.matches(&id) && pattern.ecosystem().is_some())
                .or_else(|| {
                    cfg.packages
                        .iter()
                        .find(|(pattern, _)| pattern.matches(&id))
                })
                .map(|(_, cfg)| cfg);

            // If no [[package]] rule matched, look for a [[package-set]] fallback.
            // [[package-set]] uses glob patterns and can match many packages at once;
            // [[package]] always takes priority over [[package-set]] for the same package.
            let set_override = if pkg_override.is_none() {
                cfg.package_sets
                    .iter()
                    .find(|(pattern, _)| pattern.matches(&id))
                    .map(|(_, cfg)| cfg)
            } else {
                None
            };

            let active_override = pkg_override.or(set_override);

            let release_trigger = active_override
                .and_then(|o| o.release_trigger)
                .unwrap_or(ReleaseTrigger::Changeset);

            let tag_template = active_override.and_then(|o| o.tag_template.clone());

            let changelog =
                if let Some(override_path) = active_override.and_then(|o| o.changelog.as_ref()) {
                    Some(rel_path.join(override_path))
                } else {
                    Some(ch_path)
                };

            // Apply the resolved override's publish-to if the operator explicitly set it.
            if let Some(override_targets) = active_override.and_then(|o| o.publish_to.as_deref()) {
                publish_to = override_targets.to_vec();
            }

            let pkg = Package {
                id: id.clone(),
                manifests: decls,
                changelog,
                release_trigger,
                publish_to,
                tag_template,
            };
            packages.insert(id, pkg);
        }

        let mut edges = Vec::new();
        let mut out_index: BTreeMap<PackageId, Vec<usize>> = BTreeMap::new();
        let mut in_index: BTreeMap<PackageId, Vec<usize>> = BTreeMap::new();

        for pkg in packages.values() {
            for decl in &pkg.manifests {
                if decl.role != ManifestRole::Canonical {
                    continue;
                }
                if let Ok(m) = open_cached(manifest_cache, decl, &ctx) {
                    for entry in m.iter_dependencies() {
                        let (spec, declaring_path) = if entry.inherited {
                            if let Some(ref inh) = ctx.cargo_workspace {
                                if let Some(inherited_dep) = inh.inherited(&entry.name) {
                                    (
                                        inherited_dep.spec.clone(),
                                        inherited_dep.declared_in.to_path_buf(),
                                    )
                                } else {
                                    (entry.spec.clone(), decl.path.clone())
                                }
                            } else {
                                (entry.spec.clone(), decl.path.clone())
                            }
                        } else {
                            (entry.spec.clone(), decl.path.clone())
                        };

                        if let Some(to) = index.resolve_native_with_fallback(
                            decl.ecosystem(),
                            &entry.name,
                            &mut diagnostics,
                        ) {
                            let idx = edges.len();
                            let edge = DepEdge {
                                from: pkg.id.clone(),
                                to: to.clone(),
                                kind: entry.kind,
                                spec,
                                from_manifest: declaring_path,
                                inherited: entry.inherited,
                            };
                            edges.push(edge);

                            out_index.entry(pkg.id.clone()).or_default().push(idx);
                            in_index.entry(to.clone()).or_default().push(idx);
                        }
                    }
                }
            }
        }

        if let Some(declared) = locator.declared_edges() {
            let cross_diags = crosscheck_declared_edges(&packages, &edges, &declared);
            diagnostics.extend(cross_diags);
        }

        Ok(ManifestWalkResolver {
            packages,
            edges,
            out_index,
            in_index,
            index,
            diagnostics,
        })
    }
}

/// Reads `package.json` at `abs_path` and returns `ManifestRole::Platform`
/// when the manifest declares both `os` and `cpu` constraint arrays (the napi
/// platform-package convention). Returns `ManifestRole::Canonical` for all
/// other npm packages and on any read/parse failure.
///
/// Note: this performs a second `fs::read` on each npm `package.json` because
/// the `manifest_cache` stores `Arc<dyn Manifest>` (which does not expose raw
/// JSON fields like `os`/`cpu`) rather than a raw `serde_json::Value`. Fixing
/// the redundancy would require a `Manifest::npm_role()` extension method in
/// the `callisto-manifests` crate.
fn detect_npm_role(abs_path: &Path) -> ManifestRole {
    let Ok(bytes) = std::fs::read(abs_path) else {
        return ManifestRole::Canonical;
    };
    let Ok(Value::Object(map)) = serde_json::from_slice(&bytes) else {
        return ManifestRole::Canonical;
    };

    let has_os = map
        .get("os")
        .and_then(|v| v.as_array())
        .is_some_and(|a| !a.is_empty());
    let has_cpu = map
        .get("cpu")
        .and_then(|v| v.as_array())
        .is_some_and(|a| !a.is_empty());

    if !has_os || !has_cpu {
        return ManifestRole::Canonical;
    }

    let Some(platform) = map
        .get("os")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return ManifestRole::Canonical;
    };

    let Some(arch) = map
        .get("cpu")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return ManifestRole::Canonical;
    };

    ManifestRole::Platform {
        platform,
        arch,
        abi: None,
    }
}

/// Explicit ecosystem precedence for primary-ID selection when a single
/// project directory contains manifests from multiple ecosystems (e.g., both
/// `Cargo.toml` and `package.json`). Lower value = higher priority.
///
/// Precedence: Cargo (0) > Npm (1) > Pypi (2) > others (255).
///
/// This function is used instead of `Ecosystem`'s derived `Ord` so that the
/// ordering is stable even if the `Ecosystem` variant sequence changes.
fn ecosystem_primary_priority(e: Ecosystem) -> u8 {
    match e {
        Ecosystem::Cargo => 0,
        Ecosystem::Npm => 1,
        Ecosystem::Pypi => 2,
        _ => u8::MAX,
    }
}
