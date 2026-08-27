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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Ecosystem, VersionGrammar};

    #[test]
    fn dep_spec_render_covers_every_variant() {
        let exact = DepSpec::Exact(Version::parse("1.2.3", VersionGrammar::SemVer).unwrap());
        assert_eq!(exact.render(), "1.2.3");

        let range = DepSpec::Range(VersionReq::parse("^1.0", Ecosystem::Cargo).unwrap(), "^1.0".to_string());
        assert_eq!(range.render(), "^1.0");

        for kind in [WorkspaceKind::Pnpm, WorkspaceKind::Yarn, WorkspaceKind::Npm] {
            assert_eq!(DepSpec::Workspace(kind).render(), "workspace:*", "kind={kind:?}");
        }

        assert_eq!(
            DepSpec::Catalog(Some("react18".to_string())).render(),
            "catalog:react18"
        );
        assert_eq!(DepSpec::Catalog(None).render(), "catalog:");

        let cargo_bare = DepSpec::CargoBare(Version::parse("2.0.0", VersionGrammar::SemVer).unwrap());
        assert_eq!(cargo_bare.render(), "2.0.0");

        assert_eq!(
            DepSpec::Opaque("git+https://example.com/repo".to_string()).render(),
            "git+https://example.com/repo"
        );
    }
}
