use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Ecosystem, GrammarMismatch, PackageId, Version, VersionReq};

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
    WorkspaceProtocol(WorkspaceSpec),
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
            DepSpec::WorkspaceProtocol(spec) => format!("workspace:{}", spec.suffix),
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

/// Retains the protocol spelling; aliases name a distribution and relative paths name a directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSpec {
    pub kind: WorkspaceKind,
    pub suffix: String,
}

impl WorkspaceSpec {
    pub fn alias(&self) -> Option<&str> {
        if self.relative_path().is_some() {
            return None;
        }
        self.suffix
            .rsplit_once('@')
            .and_then(|(name, _)| (!name.is_empty() && !name.contains([':', ' ', '\\'])).then_some(name))
    }

    /// Bind this directory relative to the declaring manifest to an admitted npm member.
    pub fn relative_path(&self) -> Option<&str> {
        (self.suffix.starts_with("../") || self.suffix.starts_with("./")).then_some(self.suffix.as_str())
    }

    pub fn range(&self) -> Option<&str> {
        if self.relative_path().is_some() {
            return None;
        }
        let range = if self.alias().is_some() {
            self.suffix.rsplit_once('@').map(|(_, range)| range)?
        } else {
            self.suffix.as_str()
        };
        (!matches!(range, "" | "*" | "^" | "~")).then_some(range)
    }

    /// Coverage assumes the workspace target has already been resolved locally.
    pub fn coverage(&self, version: &Version) -> Result<Coverage, GrammarMismatch> {
        let Some(range) = self.range() else {
            return Ok(Coverage::Covers);
        };
        let Some(requirements) = range
            .split("||")
            .map(|arm| Self::parse_range_arm(arm.trim()))
            .collect::<Option<Vec<_>>>()
        else {
            return Ok(Coverage::Unknown);
        };
        for requirement in requirements {
            if requirement.matches(version)? {
                return Ok(Coverage::Covers);
            }
        }
        Ok(Coverage::DoesNotCover)
    }

    fn parse_range_arm(range: &str) -> Option<VersionReq> {
        if range.is_empty() {
            return None;
        }
        VersionReq::parse(range, Ecosystem::Npm).ok()
    }

    pub fn with_range(&self, range: &str) -> Self {
        let suffix = match self.alias() {
            Some(target) => format!("{target}@{range}"),
            None => range.to_string(),
        };
        Self {
            kind: self.kind,
            suffix,
        }
    }
}

/// Evaluation of whether a version specification covers a candidate version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Coverage {
    Covers,
    DoesNotCover,
    Unknown,
}

/// The Python table holding a declaration.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DepContainer {
    /// The `[project].dependencies` array.
    ProjectDependencies,
    /// The array of one extra in `[project.optional-dependencies]`, by key.
    Extra(String),
    /// The `[tool.poetry.dependencies]` table.
    PoetryDependencies,
}

/// Where one dependency declaration lives; a position is valid for one plan-then-apply run only.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct DepLocator {
    /// The Cargo section as `Runtime`, `Dev` or `Build`; `None` derives it from the kind.
    pub section: Option<DepKind>,
    /// The `cfg` or triple of a `[target.<cfg>.*]` table; `None` for a top-level table.
    pub target: Option<String>,
    /// The Python table of the declaration; `None` for Cargo and npm.
    pub container: Option<DepContainer>,
    /// The index within a Python requirement array; `None` for a keyed table.
    pub position: Option<usize>,
}

impl DepLocator {
    /// A Cargo declaration in `section`, under `[target.<cfg>.*]` when `target` is set.
    pub fn cargo(section: DepKind, target: Option<String>) -> Self {
        Self {
            section: Some(section),
            target,
            ..Self::default()
        }
    }

    /// A Python declaration in `container`, at `position` when that is an array.
    pub fn python(container: DepContainer, position: Option<usize>) -> Self {
        Self {
            container: Some(container),
            position,
            ..Self::default()
        }
    }
}

/// Dependency entry read directly from a manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependencyEntry {
    /// Declared key or spelling used when editing the manifest.
    pub name: String,
    /// Native package name used to resolve the dependency's owner.
    pub package_name: String,
    pub kind: DepKind,
    pub spec: DepSpec,
    pub inherited: bool,
    /// The table this declaration lives in.
    pub locator: DepLocator,
}

/// Resolved dependency edge between two packages in the workspace graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepEdge {
    pub from: PackageId,
    pub to: PackageId,
    /// Declared key or spelling retained from the source manifest.
    pub name: String,
    pub kind: DepKind,
    pub spec: DepSpec,
    pub from_manifest: PathBuf,
    pub inherited: bool,
    /// The table of the member manifest this declaration lives in, even when `from_manifest` is the workspace root.
    pub locator: DepLocator,
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

    #[test]
    fn workspace_protocol_keeps_suffix_and_identity_source() {
        for suffix in ["", "*", "^", "~", "^0.2.3", "@scope/lib@~0.2.3", "../lib"] {
            let workspace = WorkspaceSpec {
                kind: WorkspaceKind::Pnpm,
                suffix: suffix.to_string(),
            };
            let spec = DepSpec::WorkspaceProtocol(workspace);
            assert_eq!(spec.render(), format!("workspace:{suffix}"));
            let wire = serde_json::to_string(&spec).unwrap();
            assert_eq!(serde_json::from_str::<DepSpec>(&wire).unwrap(), spec);
        }
        let alias = WorkspaceSpec {
            kind: WorkspaceKind::Pnpm,
            suffix: "@scope/lib@^0.2.3".into(),
        };
        assert_eq!(alias.alias(), Some("@scope/lib"));
        assert_eq!(alias.relative_path(), None);
        assert_eq!(alias.range(), Some("^0.2.3"));
        assert_eq!(alias.with_range("^0.3.0").suffix, "@scope/lib@^0.3.0");
        let relative = WorkspaceSpec {
            kind: WorkspaceKind::Pnpm,
            suffix: "../lib@dev".into(),
        };
        assert_eq!(relative.alias(), None);
        assert_eq!(relative.relative_path(), Some("../lib@dev"));
        assert_eq!(relative.range(), None);
    }

    #[test]
    fn workspace_shorthand_covers_current_versions_including_prereleases() {
        for suffix in ["", "*", "^", "~", "lib@*", "@scope/lib@^", "../lib"] {
            let workspace = WorkspaceSpec {
                kind: WorkspaceKind::Pnpm,
                suffix: suffix.into(),
            };
            for raw in ["0.0.1", "0.3.0", "2.0.0", "0.0.0-canary.1"] {
                assert_eq!(
                    workspace
                        .coverage(&Version::parse(raw, VersionGrammar::SemVer).unwrap())
                        .unwrap(),
                    Coverage::Covers,
                    "{suffix}: {raw}"
                );
            }
        }
    }

    #[test]
    fn workspace_explicit_ranges_use_npm_bounds_and_exact_pins() {
        for (suffix, candidate, expected) in [
            ("0.2.3", "0.2.4", Coverage::DoesNotCover),
            ("0.2.3", "0.2.3", Coverage::Covers),
            ("^0.2.3", "0.2.9", Coverage::Covers),
            ("^0.2.3", "0.3.0", Coverage::DoesNotCover),
            ("^0.0.3", "0.0.4", Coverage::DoesNotCover),
            ("~0.2.3", "0.3.0", Coverage::DoesNotCover),
            ("1.2", "1.3.0", Coverage::DoesNotCover),
            ("1.2", "1.2.9", Coverage::Covers),
            ("@scope/lib@^0.2.3", "0.3.0", Coverage::DoesNotCover),
            ("^0.2.3", "0.2.4-canary.1", Coverage::DoesNotCover),
            ("https://example.com/lib.tgz", "1.0.0", Coverage::Unknown),
            ("^1.0.0 || ^2.0.0", "1.9.9", Coverage::Covers),
            ("^1.0.0 || ^2.0.0", "2.0.0", Coverage::Covers),
            ("^1.0.0 || ^2.0.0", "3.0.0", Coverage::DoesNotCover),
            ("^1.0.0 || ^2.0.0 || ^4.0.0", "4.1.0", Coverage::Covers),
            ("@scope/lib@^1.0.0 || ^2.0.0", "3.0.0", Coverage::DoesNotCover),
            ("1.2.3 || 2.0.0", "1.2.4", Coverage::DoesNotCover),
            ("1.2.3 || 2.0.0", "2.0.0", Coverage::Covers),
            ("^1.0.0 || ^2.0.0", "2.0.0-beta.1", Coverage::DoesNotCover),
            ("^1.0.0 || nonsense", "1.2.3", Coverage::Unknown),
            ("^1.0.0 ||", "1.2.3", Coverage::Unknown),
            ("|| ^1.0.0", "1.2.3", Coverage::Unknown),
            ("^1.0.0 | ^2.0.0", "1.2.3", Coverage::Unknown),
        ] {
            let version = Version::parse(candidate, VersionGrammar::SemVer).unwrap();
            for kind in [WorkspaceKind::Pnpm, WorkspaceKind::Npm] {
                let workspace = WorkspaceSpec {
                    kind,
                    suffix: suffix.into(),
                };
                assert_eq!(
                    workspace.coverage(&version).unwrap(),
                    expected,
                    "{kind:?}: {suffix}: {candidate}"
                );
            }
        }
    }
}
