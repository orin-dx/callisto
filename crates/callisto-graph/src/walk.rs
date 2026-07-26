use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use callisto_manifests::{detect_npm_workspace_kind, open, OpenContext, WorkspaceCargoResolver};
use callisto_model::{
    CommandRunner, DepEdge, Ecosystem, ManifestDecl, ManifestFormat, ManifestRole, Package,
    PackageId, PublishTarget, ReleaseTrigger,
};

use crate::config::ResolvedConfig;
use crate::crosscheck::crosscheck_declared_edges;
use crate::error::GraphError;
use crate::identity::{IdentityIndex, IdentityResolver};
use crate::locate::ProjectLocator;
use crate::resolver::ManifestWalkResolver;

impl ManifestWalkResolver {
    pub fn build<L: ProjectLocator, R: CommandRunner>(
        root: &Path,
        locator: &L,
        _runner: &R,
        _cfg: &ResolvedConfig,
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

        let identity_resolver = IdentityResolver::new(root)?;
        let mut package_manifest_decls: BTreeMap<PackageId, (PathBuf, Vec<ManifestDecl>)> =
            BTreeMap::new();
        let mut index = IdentityIndex::default();
        let mut diagnostics = Vec::new();

        let mut by_path: BTreeMap<PathBuf, Vec<(Ecosystem, PackageId)>> = BTreeMap::new();
        for proj in &projects {
            let id = identity_resolver.resolve(&proj.path, proj.ecosystem)?;
            by_path
                .entry(proj.path.clone())
                .or_default()
                .push((proj.ecosystem, id));
        }

        for (rel_path, mut list) in by_path {
            list.sort_by_key(|a| a.0);
            let primary_id = list[0].1.clone();

            index
                .bare
                .insert(primary_id.name().to_string(), primary_id.clone());

            let mut decls = Vec::new();
            for (eco, _id) in &list {
                let (fmt, filename) = match eco {
                    Ecosystem::Cargo => (ManifestFormat::CargoToml, "Cargo.toml"),
                    Ecosystem::Npm => (ManifestFormat::PackageJson, "package.json"),
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
            let pkg = Package {
                id: id.clone(),
                manifests: decls,
                changelog: Some(ch_path),
                release_trigger: ReleaseTrigger::Changeset,
                publish_to: vec![match id.ecosystem() {
                    Some(Ecosystem::Cargo) => PublishTarget::CratesIo,
                    Some(Ecosystem::Npm) => PublishTarget::Npm { registry: None },
                    _ => PublishTarget::None,
                }],
                tag_template: None,
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
                if let Ok(m) = open(decl, &ctx) {
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

                        if let Some(to) = index.resolve_native(decl.ecosystem(), &entry.name) {
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
            let cross_diags = crosscheck_declared_edges(
                &ManifestWalkResolver {
                    packages: packages.clone(),
                    edges: edges.clone(),
                    out_index: out_index.clone(),
                    in_index: in_index.clone(),
                    index: index.clone(),
                    diagnostics: Vec::new(),
                },
                &declared,
            );
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
