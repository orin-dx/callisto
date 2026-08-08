use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
                if let Ok(decl) = ManifestDecl::new(manifest_rel, ManifestRole::Canonical, fmt) {
                    decls.push(decl);
                }
                index
                    .native
                    .insert((*eco, primary_id.name().to_string()), primary_id.clone());
            }

            package_manifest_decls.insert(primary_id, (rel_path, decls));
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

            // Find the first [[package]] rule in callisto.toml whose pattern
            // matches this package's ID (exact or bare-name match).
            let pkg_override = cfg
                .packages
                .iter()
                .find(|(pattern, _)| pattern.matches(&id))
                .map(|(_, cfg)| cfg);

            let release_trigger = pkg_override
                .and_then(|o| o.release_trigger)
                .unwrap_or(ReleaseTrigger::Changeset);

            let tag_template = pkg_override.and_then(|o| o.tag_template.clone());

            let changelog =
                if let Some(override_path) = pkg_override.and_then(|o| o.changelog.as_ref()) {
                    Some(rel_path.join(override_path))
                } else {
                    Some(ch_path)
                };

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
