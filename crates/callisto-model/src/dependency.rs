use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{PackageId, Version, VersionReq};

/// Dependency kind (section in manifest).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum DepKind {
    Runtime,
    Dev,
    Peer,
    Optional,
    Build,
}

/// Dependency specification requirement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum DepSpec {
    Exact(Version),
    Range(VersionReq, String),
    Workspace(WorkspaceKind),
    Catalog(Option<String>),
    CargoBare(Version),
    Opaque(String),
}

impl DepSpec {
    pub fn render(&self) -> String {
        match self {
            DepSpec::Exact(v) => v.render().to_string(),
            DepSpec::Range(_, raw) => raw.clone(),
            DepSpec::Workspace(kind) => match kind {
                WorkspaceKind::Pnpm | WorkspaceKind::Yarn | WorkspaceKind::Npm => "workspace:*".to_string(),
            },
            DepSpec::Catalog(opt) => match opt {
                Some(name) => format!("catalog:{name}"),
                None => "catalog:".to_string(),
            },
            DepSpec::CargoBare(v) => v.render().to_string(),
            DepSpec::Opaque(raw) => raw.clone(),
        }
    }
}

/// Ecosystem workspace protocol type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceKind {
    Pnpm,
    Yarn,
    Npm,
}

/// Evaluation of whether a version specification covers a candidate version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Coverage {
    Covers,
    DoesNotCover,
    Unknown,
}

/// Dependency entry read directly from a manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependencyEntry {
    pub name: String,
    pub kind: DepKind,
    pub spec: DepSpec,
    pub inherited: bool,
}

/// Resolved dependency edge between two packages in the workspace graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepEdge {
    pub from: PackageId,
    pub to: PackageId,
    pub kind: DepKind,
    pub spec: DepSpec,
    pub from_manifest: PathBuf,
    pub inherited: bool,
}
