use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Ecosystem, ModelError};

/// Package identity across ecosystems.
#[derive(Clone, Debug, PartialEq, Eq, Hash, JsonSchema)]
#[schemars(with = "String")]
pub enum PackageId {
    Bare(String),
    Prefixed { ecosystem: Ecosystem, name: String },
}

impl PackageId {
    /// Parses package identity string.
    pub fn parse(s: &str) -> Result<Self, PackageIdParseError> {
        if s.is_empty() {
            return Err(PackageIdParseError::Empty);
        }
        if s.starts_with('/') {
            return Err(PackageIdParseError::LeadingSlash { raw: s.to_string() });
        }
        if s.starts_with('-') {
            return Err(PackageIdParseError::PathTraversal { raw: s.to_string() });
        }
        if s.contains("..") {
            return Err(PackageIdParseError::PathTraversal { raw: s.to_string() });
        }

        if let Some((prefix, remainder)) = s.split_once(':') {
            if let Some(ecosystem) = Ecosystem::from_prefix(prefix) {
                if remainder.is_empty() {
                    return Err(PackageIdParseError::EmptyNameAfterPrefix {
                        raw: s.to_string(),
                        prefix: prefix.to_string(),
                    });
                }
                if remainder.starts_with('-') || remainder.contains("..") {
                    return Err(PackageIdParseError::PathTraversal { raw: s.to_string() });
                }
                return Ok(PackageId::Prefixed {
                    ecosystem,
                    name: remainder.to_string(),
                });
            }
        }

        if let Some((prefix, remainder)) = s.split_once('/') {
            if let Some(ecosystem) = Ecosystem::from_prefix(prefix) {
                if remainder.is_empty() {
                    return Err(PackageIdParseError::EmptyNameAfterPrefix {
                        raw: s.to_string(),
                        prefix: prefix.to_string(),
                    });
                }
                if remainder.starts_with('-') || remainder.contains("..") {
                    return Err(PackageIdParseError::PathTraversal { raw: s.to_string() });
                }
                return Ok(PackageId::Prefixed {
                    ecosystem,
                    name: remainder.to_string(),
                });
            }
        }

        Ok(PackageId::Bare(s.to_string()))
    }

    pub fn display_name(&self) -> String {
        match self {
            PackageId::Bare(name) => name.clone(),
            PackageId::Prefixed { ecosystem, name } => {
                format!("{}/{}", ecosystem.prefix(), name)
            }
        }
    }

    pub fn ecosystem(&self) -> Option<Ecosystem> {
        match self {
            PackageId::Bare(_) => None,
            PackageId::Prefixed { ecosystem, .. } => Some(*ecosystem),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            PackageId::Bare(name) => name,
            PackageId::Prefixed { name, .. } => name,
        }
    }

    /// Returns true if two package IDs refer to the same logical package,
    /// matching bare and prefixed representations when names and ecosystems align.
    pub fn matches(&self, other: &Self) -> bool {
        if self == other {
            return true;
        }
        if self.name() == other.name() {
            match (self.ecosystem(), other.ecosystem()) {
                (None, _) | (_, None) => true,
                (Some(e1), Some(e2)) => e1 == e2,
            }
        } else {
            false
        }
    }
}

/// Trait for package identity resolution across ecosystem boundaries.
pub trait PackageIdentityResolver {
    /// Returns true if two package IDs refer to the same logical package.
    fn matches_id(&self, other: &PackageId) -> bool;
}

impl PackageIdentityResolver for PackageId {
    fn matches_id(&self, other: &PackageId) -> bool {
        self.matches(other)
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_name())
    }
}

impl FromStr for PackageId {
    type Err = PackageIdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PackageId::parse(s)
    }
}

impl PartialOrd for PackageId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PackageId {
    fn cmp(&self, other: &Self) -> Ordering {
        let key_self = (self.ecosystem(), self.name());
        let key_other = (other.ecosystem(), other.name());
        key_self.cmp(&key_other)
    }
}

impl Serialize for PackageId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.display_name())
    }
}

impl<'de> Deserialize<'de> for PackageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        PackageId::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PackageIdParseError {
    #[error("package identity is empty")]
    Empty,
    #[error("package identity `{raw}` has ecosystem prefix `{prefix}` but no name after it")]
    EmptyNameAfterPrefix { raw: String, prefix: String },
    #[error("`{raw}` starts with `/`")]
    LeadingSlash { raw: String },
    #[error("package identity `{raw}` contains path traversal `..`")]
    PathTraversal { raw: String },
}

/// A group name for fixed or linked package groups.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[schemars(with = "String")]
#[serde(transparent)]
pub struct GroupName(pub String);

impl GroupName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GroupName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Group kind: Fixed vs Linked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum GroupKind {
    Fixed,
    Linked,
}

/// Registry key string e.g. "cratesIo", "npm".
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[schemars(with = "String")]
#[serde(transparent)]
pub struct RegistryKey(pub String);

impl RegistryKey {
    pub const CRATES_IO: &'static str = "cratesIo";
    pub const NPM: &'static str = "npm";

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RegistryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A 40-character hex Git commit SHA.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, JsonSchema)]
#[schemars(with = "String")]
pub struct CommitSha(String);

impl CommitSha {
    pub fn parse(s: &str) -> Result<Self, ModelError> {
        let trimmed = s.trim();
        if trimmed.len() != 40 || !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ModelError::InvalidCommitSha {
                raw: s.to_string(),
                reason: "must be exactly 40 hexadecimal characters".to_string(),
            });
        }
        Ok(CommitSha(trimmed.to_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn short(&self) -> &str {
        &self.0[..7]
    }
}

impl Serialize for CommitSha {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CommitSha {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        CommitSha::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_package_ids() {
        assert_eq!(
            PackageId::parse("foo").unwrap(),
            PackageId::Bare("foo".to_string())
        );
        assert_eq!(
            PackageId::parse("@myorg/foo").unwrap(),
            PackageId::Bare("@myorg/foo".to_string())
        );
        assert_eq!(
            PackageId::parse("cargo/foo").unwrap(),
            PackageId::Prefixed {
                ecosystem: Ecosystem::Cargo,
                name: "foo".to_string()
            }
        );
        assert_eq!(
            PackageId::parse("npm/@myorg/foo").unwrap(),
            PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name: "@myorg/foo".to_string()
            }
        );
        assert_eq!(
            PackageId::parse("npm:@myorg/foo").unwrap(),
            PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name: "@myorg/foo".to_string()
            }
        );
    }

    #[test]
    fn test_package_id_matches() {
        let bare = PackageId::parse("foo").unwrap();
        let prefixed_cargo = PackageId::parse("cargo/foo").unwrap();
        let prefixed_npm = PackageId::parse("npm/foo").unwrap();

        assert!(bare.matches(&prefixed_cargo));
        assert!(prefixed_cargo.matches(&bare));
        assert!(bare.matches(&prefixed_npm));
        assert!(!prefixed_cargo.matches(&prefixed_npm));
    }

    #[test]
    fn parses_valid_commit_sha() {
        let sha_str = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0";
        let sha = CommitSha::parse(sha_str).unwrap();
        assert_eq!(sha.as_str(), sha_str);
        assert_eq!(sha.short(), "a1b2c3d");
    }

    use proptest::prelude::*;
    proptest! {
        #[test]
        fn proptest_package_id_parse_never_panics(s in "\\PC*") {
            let _res = PackageId::parse(&s);
        }

        #[test]
        fn proptest_package_id_matches_identity(name in "[a-z][a-z0-9_-]{0,29}") {
            let bare = PackageId::parse(&name).unwrap();
            let prefixed = PackageId::parse(&format!("cargo/{}", name)).unwrap();
            prop_assert!(bare.matches(&prefixed));
            prop_assert!(prefixed.matches(&bare));
        }
    }
}
