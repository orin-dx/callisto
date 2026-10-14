use std::collections::BTreeMap;
use std::path::PathBuf;

use callisto_graph::resolver::DependencyResolver;
use callisto_graph::GraphError;
use callisto_model::{
    DepEdge, DepKind, DepSpec, ManifestDecl, ManifestFormat, ManifestRole, Package, PackageId, PublishTarget,
    ReleaseTrigger, TagTemplate,
};

#[derive(Default)]
pub struct GraphBuilder {
    packages: BTreeMap<PackageId, PackageBuilder>,
    edges: Vec<DepEdge>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
    pub fn package(mut self, id: impl Into<PackageId>, f: impl FnOnce(PackageBuilder) -> PackageBuilder) -> Self {
        let pkg_id = id.into();
        let builder = PackageBuilder::new(pkg_id.clone());
        self.packages.insert(pkg_id, f(builder));
        self
    }

    #[allow(dead_code)]
    pub fn edge(mut self, from: impl Into<PackageId>, to: impl Into<PackageId>, kind: DepKind, spec: DepSpec) -> Self {
        let from_id = from.into();
        let to_id = to.into();
        let from_manifest = PathBuf::from(format!("{}/Cargo.toml", from_id.name()));
        self.edges.push(DepEdge {
            from: from_id,
            name: to_id.name().to_string(),
            to: to_id,
            kind,
            spec,
            from_manifest,
            inherited: false,
        });
        self
    }

    #[allow(clippy::result_large_err)]
    pub fn build(self) -> Result<InMemoryGraph, GraphError> {
        let mut built_packages = BTreeMap::new();
        for (id, builder) in self.packages {
            built_packages.insert(id, builder.build());
        }

        let mut out_index: BTreeMap<PackageId, Vec<usize>> = BTreeMap::new();
        let mut in_index: BTreeMap<PackageId, Vec<usize>> = BTreeMap::new();

        for (idx, edge) in self.edges.iter().enumerate() {
            out_index.entry(edge.from.clone()).or_default().push(idx);
            in_index.entry(edge.to.clone()).or_default().push(idx);
        }

        Ok(InMemoryGraph {
            packages: built_packages,
            edges: self.edges,
            out_index,
            in_index,
        })
    }
}

pub struct PackageBuilder {
    id: PackageId,
    release_trigger: ReleaseTrigger,
    publish_to: Vec<PublishTarget>,
    changelog: Option<PathBuf>,
    tag_template: Option<TagTemplate>,
    manifests: Vec<ManifestDecl>,
}

#[allow(dead_code)]
impl PackageBuilder {
    pub fn new(id: PackageId) -> Self {
        let manifest_path = PathBuf::from(format!("{}/Cargo.toml", id.name()));
        let decl = ManifestDecl::new(manifest_path, ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
        PackageBuilder {
            id,
            release_trigger: ReleaseTrigger::Changeset,
            publish_to: Vec::new(),
            changelog: None,
            tag_template: None,
            manifests: vec![decl],
        }
    }

    pub fn release_trigger(mut self, rt: ReleaseTrigger) -> Self {
        self.release_trigger = rt;
        self
    }

    pub fn publish_to(mut self, targets: Vec<PublishTarget>) -> Self {
        self.publish_to = targets;
        self
    }

    pub fn changelog(mut self, path: Option<PathBuf>) -> Self {
        self.changelog = path;
        self
    }

    pub fn tag_template(mut self, t: Option<TagTemplate>) -> Self {
        self.tag_template = t;
        self
    }

    pub fn manifests(mut self, m: Vec<ManifestDecl>) -> Self {
        self.manifests = m;
        self
    }

    pub fn build(self) -> Package {
        Package {
            id: self.id,
            manifests: self.manifests,
            changelog: self.changelog,
            release_trigger: self.release_trigger,
            publish_to: self.publish_to,
            tag_template: self.tag_template,
        }
    }
}

pub struct InMemoryGraph {
    packages: BTreeMap<PackageId, Package>,
    edges: Vec<DepEdge>,
    out_index: BTreeMap<PackageId, Vec<usize>>,
    in_index: BTreeMap<PackageId, Vec<usize>>,
}

/// An `IdentityIndex` reflecting every package's own canonical manifests, as
/// `ManifestWalkResolver::build` would populate it for a real workspace --
/// package-selector resolution (`--package`, `[[package]]`, `[[fixed-group]]`)
/// runs through `IdentityIndex`, not a `GraphBuilder`-built graph directly, so
/// a hand-built `Workspace` fixture needs one to resolve anything at all.
#[allow(dead_code)]
pub fn identity_for(graph: &InMemoryGraph) -> callisto_graph::IdentityIndex {
    let mut index = callisto_graph::IdentityIndex::default();
    for pkg in graph.packages() {
        for manifest in pkg.canonical_manifests() {
            index
                .native
                .insert((manifest.ecosystem(), pkg.id.name().to_string()), pkg.id.clone());
        }
    }
    index
}

impl DependencyResolver for InMemoryGraph {
    fn packages(&self) -> impl Iterator<Item = &Package> {
        self.packages.values()
    }

    fn dependencies_of(&self, id: &PackageId) -> impl Iterator<Item = &DepEdge> {
        let empty = Vec::new();
        let indices = self.out_index.get(id).unwrap_or(&empty);
        let mut result = Vec::new();
        for &idx in indices {
            result.push(&self.edges[idx]);
        }
        result.into_iter()
    }

    fn dependents_of(&self, id: &PackageId) -> impl Iterator<Item = &DepEdge> {
        let empty = Vec::new();
        let indices = self.in_index.get(id).unwrap_or(&empty);
        let mut result = Vec::new();
        for &idx in indices {
            result.push(&self.edges[idx]);
        }
        result.into_iter()
    }
}

#[allow(dead_code)]
pub fn native_dependency_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let files = [
        ("callisto.toml", "[cascade]\npreserve-npm-ranges = false\n"),
        (
            "Cargo.toml",
            r#"[workspace]
members = ["crates/*"]
resolver = "2"

[workspace.dependencies]
workspace_alias = { package = "core-lib", path = "crates/core", version = "^1.0.0" } # inherited

[workspace.dependencies.workspace_optional]
package = "core-lib"
path = "crates/core"
version = "^1.0.0" # optional
"#,
        ),
        (
            "package.json",
            "{\"name\":\"workspace\",\"private\":true,\"workspaces\":[\"crates/*\",\"packages/*\"]}\n",
        ),
        (
            "crates/core/Cargo.toml",
            "[package]\nname = \"core-lib\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[features]\nkeep = []\n",
        ),
        (
            "crates/core/package.json",
            "{\"name\":\"@scope/core\",\"version\":\"1.0.0\"}\n",
        ),
        (
            "crates/app/Cargo.toml",
            r#"[package]
name = "app"
version = "1.0.0"
edition = "2021"

[dependencies]
local_alias = { package = "core-lib", path = "../core", version = "^1.0.0", features = ["keep"] } # local
optional_alias = { package = "core-lib", path = "../core", version = "^1.0.0", optional = true }
workspace_alias.workspace = true
workspace_optional = { workspace = true, optional = true } # member

[build-dependencies.build_alias]
package = "core-lib"
path = "../core"
version = "^1.0.0" # build

[dev-dependencies]
dev_alias = { package = "core-lib", path = "../core", version = "=1.0.0" }
covered_dev = { package = "core-lib", path = "../core", version = "^1.0.0" }
"#,
        ),
        (
            "crates/app/package.json",
            r#"{
  "name": "@scope/app",
  "version": "1.0.0",
  "dependencies": { "@scope/core": "^1.0.0", "external": "^9.0.0" },
  "optionalDependencies": { "@scope/core": "^1.0.0" },
  "peerDependencies": { "@scope/core": "^1.0.0" },
  "devDependencies": { "@scope/core": "=1.0.0" }
}
"#,
        ),
        (
            "crates/tools/Cargo.toml",
            r#"[package]
name = "tools"
version = "1.0.0"

[dev-dependencies]
tool_alias = { package = "core-lib", path = "../core", version = "=1.0.0" } # dev-only
"#,
        ),
        (
            "packages/core/package.json",
            "{\"name\":\"core-lib\",\"version\":\"1.0.0\"}\n",
        ),
        (
            "packages/app/package.json",
            "{\"name\":\"app\",\"version\":\"1.0.0\"}\n",
        ),
        (
            "packages/protocol/package.json",
            "{\"name\":\"protocol\",\"version\":\"1.0.0\",\"dependencies\":{\"@scope/core\":\"workspace:*\"}}\n",
        ),
    ];
    for (path, source) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    callisto_fixtures::git::init_repo(root);
    callisto_fixtures::git::run_git(root, &["add", "."]);
    callisto_fixtures::git::run_git(root, &["commit", "-q", "-m", "init"]);
    dir
}
