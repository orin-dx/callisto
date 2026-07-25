use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{RegistryKey, VersionGrammar};

/// Ecosystem supported by callisto.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Ecosystem {
    Cargo,
    Npm,
    // Demand-gated ecosystems:
    Pypi,
    Go,
    Maven,
    NuGet,
    Deno,
    Jsr,
}

impl Ecosystem {
    pub fn prefix(&self) -> &'static str {
        match self {
            Ecosystem::Cargo => "cargo",
            Ecosystem::Npm => "npm",
            Ecosystem::Pypi => "pypi",
            Ecosystem::Go => "go",
            Ecosystem::Maven => "maven",
            Ecosystem::NuGet => "nuget",
            Ecosystem::Deno => "deno",
            Ecosystem::Jsr => "jsr",
        }
    }

    pub fn from_prefix(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "cargo" => Some(Ecosystem::Cargo),
            "npm" => Some(Ecosystem::Npm),
            "pypi" => Some(Ecosystem::Pypi),
            "go" => Some(Ecosystem::Go),
            "maven" => Some(Ecosystem::Maven),
            "nuget" => Some(Ecosystem::NuGet),
            "deno" => Some(Ecosystem::Deno),
            "jsr" => Some(Ecosystem::Jsr),
            _ => None,
        }
    }

    pub fn version_grammar(&self) -> VersionGrammar {
        match self {
            Ecosystem::Cargo
            | Ecosystem::Npm
            | Ecosystem::Deno
            | Ecosystem::Jsr
            | Ecosystem::NuGet
            | Ecosystem::Go => VersionGrammar::SemVer,
            Ecosystem::Pypi => VersionGrammar::Pep440,
            Ecosystem::Maven => VersionGrammar::Maven,
        }
    }

    pub fn is_implemented(&self) -> bool {
        matches!(self, Ecosystem::Cargo | Ecosystem::Npm)
    }
}

/// Target registry or release location.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum PublishTarget {
    CratesIo,
    Npm { registry: Option<String> },
    Pypi { index: Option<String> },
    NuGet { source: Option<String> },
    GitHubRelease,
    None,
}

impl PublishTarget {
    pub fn registry_key(&self) -> Option<RegistryKey> {
        match self {
            PublishTarget::CratesIo => Some(RegistryKey(RegistryKey::CRATES_IO.to_string())),
            PublishTarget::Npm { .. } => Some(RegistryKey(RegistryKey::NPM.to_string())),
            PublishTarget::Pypi { .. } => Some(RegistryKey("pypi".to_string())),
            PublishTarget::NuGet { .. } => Some(RegistryKey("nuget".to_string())),
            PublishTarget::GitHubRelease | PublishTarget::None => None,
        }
    }

    pub fn ecosystem(&self) -> Option<Ecosystem> {
        match self {
            PublishTarget::CratesIo => Some(Ecosystem::Cargo),
            PublishTarget::Npm { .. } => Some(Ecosystem::Npm),
            PublishTarget::Pypi { .. } => Some(Ecosystem::Pypi),
            PublishTarget::NuGet { .. } => Some(Ecosystem::NuGet),
            PublishTarget::GitHubRelease | PublishTarget::None => None,
        }
    }
}

/// Trigger mechanism for generating releases.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseTrigger {
    #[default]
    Changeset,
    Auto,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_ecosystem_prefixes() {
        assert_eq!(Ecosystem::from_prefix("cargo"), Some(Ecosystem::Cargo));
        assert_eq!(Ecosystem::from_prefix("npm"), Some(Ecosystem::Npm));
        assert_eq!(Ecosystem::Cargo.prefix(), "cargo");
    }

    #[test]
    fn identifies_implemented_ecosystems() {
        assert!(Ecosystem::Cargo.is_implemented());
        assert!(Ecosystem::Npm.is_implemented());
        assert!(!Ecosystem::Pypi.is_implemented());
    }
}
