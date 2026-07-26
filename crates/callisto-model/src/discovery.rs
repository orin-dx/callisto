use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{DepEdge, Ecosystem, Package, PackageId};

/// A located project root in the workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRoot {
    pub id: PackageId,
    pub path: PathBuf,
    pub ecosystem: Ecosystem,
}

/// A declared dependency edge (e.g. from moon project locator).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredEdge {
    pub from: PackageId,
    pub to: PackageId,
    pub kind: DeclaredEdgeKind,
    pub via: Option<String>,
}

/// Kind of declared dependency edge from external tool discovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeclaredEdgeKind {
    Build,
    Development,
    Peer,
    Production,
    Root,
}

/// Core dependency graph resolver trait seam.
pub trait DependencyResolver: Send + Sync {
    fn packages(&self) -> impl Iterator<Item = &Package>;
    fn dependencies_of(&self, id: &PackageId) -> impl Iterator<Item = &DepEdge>;
    fn dependents_of(&self, id: &PackageId) -> impl Iterator<Item = &DepEdge>;
}
