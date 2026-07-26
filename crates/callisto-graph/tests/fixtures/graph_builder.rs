use std::collections::BTreeMap;
use std::path::PathBuf;

use callisto_graph::resolver::DependencyResolver;
use callisto_graph::GraphError;
use callisto_model::{
    DepEdge, DepKind, DepSpec, ManifestDecl, ManifestFormat, ManifestRole, Package, PackageId,
    PublishTarget, ReleaseTrigger, TagTemplate,
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

    pub fn package(
        mut self,
        id: impl Into<PackageId>,
        f: impl FnOnce(PackageBuilder) -> PackageBuilder,
    ) -> Self {
        let pkg_id = id.into();
        let builder = PackageBuilder::new(pkg_id.clone());
        self.packages.insert(pkg_id, f(builder));
        self
    }

    pub fn edge(
        mut self,
        from: impl Into<PackageId>,
        to: impl Into<PackageId>,
        kind: DepKind,
        spec: DepSpec,
    ) -> Self {
        let from_id = from.into();
        let to_id = to.into();
        let from_manifest = PathBuf::from(format!("{}/Cargo.toml", from_id.name()));
        self.edges.push(DepEdge {
            from: from_id,
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
        let decl = ManifestDecl::new(
            manifest_path,
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
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
