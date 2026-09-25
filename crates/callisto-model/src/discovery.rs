use std::path::PathBuf;

use crate::{DepEdge, Ecosystem, Package, PackageId};

/// A located project root in the workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRoot {
    pub id: PackageId,
    pub path: PathBuf,
    pub ecosystem: Ecosystem,
}

/// Core dependency graph resolver trait seam.
pub trait DependencyResolver: Send + Sync {
    fn packages(&self) -> impl Iterator<Item = &Package>;
    fn dependencies_of(&self, id: &PackageId) -> impl Iterator<Item = &DepEdge>;
    fn dependents_of(&self, id: &PackageId) -> impl Iterator<Item = &DepEdge>;
    fn diagnostics(&self) -> &[crate::Diagnostic] {
        &[]
    }
}
