use std::collections::{BTreeMap, HashSet};

use callisto_model::{DepEdge, Diagnostic, Package, PackageId};

use crate::error::GraphError;
use crate::identity::IdentityIndex;
use crate::toposort::toposort_impl;

pub use callisto_model::DependencyResolver;

pub trait DependencyResolverExt: DependencyResolver {
    fn toposort(&self, subset: &HashSet<PackageId>) -> Result<Vec<PackageId>, GraphError> {
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
