use std::collections::{BTreeMap, HashSet};

use callisto_model::{DepEdge, Diagnostic, Package, PackageId};

use crate::error::GraphError;
use crate::identity::IdentityIndex;
use crate::toposort::toposort_impl;

pub use callisto_model::DependencyResolver;

pub trait DependencyResolverExt: DependencyResolver {
    fn toposort(&self, subset: &HashSet<PackageId>) -> Result<Vec<PackageId>, GraphError> {
        // The DependencyResolver::packages() trait method yields &Package, not PackageId, so
        // we must clone each id. The Vec is required to produce a &[PackageId] slice for
        // toposort_impl; it cannot be eliminated without changing the function signature.
        let all_pkg_ids: Vec<PackageId> = self.packages().map(|p| p.id.clone()).collect();
        toposort_impl(subset, &all_pkg_ids, |id| {
            self.dependencies_of(id)
                .map(|e| (e.to.clone(), e.kind))
                .collect()
        })
    }
}

impl<T: DependencyResolver + ?Sized> DependencyResolverExt for T {}

pub struct ManifestWalkResolver {
    pub(crate) packages: BTreeMap<PackageId, Package>,
    pub(crate) edges: Vec<DepEdge>,
    pub(crate) out_index: BTreeMap<PackageId, Vec<usize>>,
    pub(crate) in_index: BTreeMap<PackageId, Vec<usize>>,
    pub(crate) index: IdentityIndex,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

impl ManifestWalkResolver {
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn identity(&self) -> &IdentityIndex {
        &self.index
    }

    pub fn get(&self, id: &PackageId) -> Option<&Package> {
        self.packages.get(id)
    }

    pub fn edges(&self) -> &[DepEdge] {
        &self.edges
    }
}

impl DependencyResolver for ManifestWalkResolver {
    fn packages(&self) -> impl Iterator<Item = &Package> {
        self.packages.values()
    }

    fn dependencies_of(&self, id: &PackageId) -> impl Iterator<Item = &DepEdge> {
        // Avoid an intermediate Vec by streaming index lookups directly as edge references.
        let edges = &self.edges;
        self.out_index
            .get(id)
            .into_iter()
            .flat_map(|v| v.iter())
            .map(move |&idx| &edges[idx])
    }

    fn dependents_of(&self, id: &PackageId) -> impl Iterator<Item = &DepEdge> {
        let edges = &self.edges;
        self.in_index
            .get(id)
            .into_iter()
            .flat_map(|v| v.iter())
            .map(move |&idx| &edges[idx])
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}
