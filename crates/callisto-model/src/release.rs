//! Durable, forge-neutral release identity and digest primitives.
//!
//! This module deliberately owns only values and deterministic byte encodings.
//! Workspace discovery and release authorization remain in callisto-graph.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::{Ecosystem, PackageId};

/// An exact, ecosystem-qualified package identity for durable release operations.
///
/// Unlike PackageId, this type has no bare form and therefore cannot use
/// wildcard matching across ecosystems.
#[derive(Clone, Debug, PartialEq, Eq, Hash, JsonSchema)]
#[schemars(with = "String")]
pub struct ReleasePackageId {
    ecosystem: Ecosystem,
    name: String,
}

impl ReleasePackageId {
    /// Creates an exact release package identity.
    ///
    /// # Errors
    ///
    /// Returns ReleasePackageIdParseError when name is not valid in the
    /// existing package-identity grammar.
    pub fn new(ecosystem: Ecosystem, name: impl AsRef<str>) -> Result<Self, ReleasePackageIdParseError> {
        let name = name.as_ref();
        let raw = format!("{}/{}", ecosystem.prefix(), name);
        match PackageId::parse(&raw) {
            Ok(PackageId::Prefixed {
                ecosystem: parsed_ecosystem,
                name: parsed_name,
            }) if parsed_ecosystem == ecosystem && parsed_name == name => Ok(Self {
                ecosystem,
                name: parsed_name,
            }),
            _ => Err(ReleasePackageIdParseError::Malformed { raw }),
        }
    }

    /// Parses the canonical 'ecosystem/name' release identity form.
    ///
    /// PackageId also accepts 'ecosystem:name'; release identities reject it
    /// so every durable encoding has exactly one spelling.
    pub fn parse(s: &str) -> Result<Self, ReleasePackageIdParseError> {
        if s.contains(':') {
            return Err(ReleasePackageIdParseError::NonCanonical { raw: s.to_string() });
        }
        let (prefix, name) = s
            .split_once('/')
            .ok_or_else(|| ReleasePackageIdParseError::MissingEcosystem { raw: s.to_string() })?;
        let ecosystem = Ecosystem::from_prefix(prefix)
            .ok_or_else(|| ReleasePackageIdParseError::UnknownEcosystem { raw: s.to_string() })?;
        let id = Self::new(ecosystem, name)?;
        if id.to_string() != s {
            return Err(ReleasePackageIdParseError::NonCanonical { raw: s.to_string() });
        }
        Ok(id)
    }

    /// Returns the exact ecosystem.
    pub fn ecosystem(&self) -> Ecosystem {
        self.ecosystem
    }

    /// Returns the package name without the ecosystem prefix.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for ReleasePackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.ecosystem.prefix(), self.name)
    }
}

impl FromStr for ReleasePackageId {
    type Err = ReleasePackageIdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl PartialOrd for ReleasePackageId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ReleasePackageId {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.ecosystem.prefix(), self.name.as_str()).cmp(&(other.ecosystem.prefix(), other.name.as_str()))
    }
}

impl Serialize for ReleasePackageId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ReleasePackageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Errors produced while parsing a ReleasePackageId.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleasePackageIdParseError {
    #[error("release package identity {raw} must include an ecosystem prefix")]
    MissingEcosystem { raw: String },
    #[error("release package identity {raw} has an unknown ecosystem prefix")]
    UnknownEcosystem { raw: String },
    #[error("release package identity {raw} is malformed")]
    Malformed { raw: String },
    #[error("release package identity {raw} is not in canonical ecosystem/name form")]
    NonCanonical { raw: String },
}

macro_rules! release_digest {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, JsonSchema)]
        #[schemars(with = "String")]
        pub struct $name(String);

        impl $name {
            /// Parses a lowercase hexadecimal SHA-256 digest.
            pub fn parse(raw: &str) -> Result<Self, DigestParseError> {
                if raw.len() != 64
                    || !raw
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err(DigestParseError {
                        digest_kind: stringify!($name),
                        raw: raw.to_string(),
                    });
                }
                Ok(Self(raw.to_string()))
            }

            /// Returns the canonical lowercase hexadecimal digest.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            fn from_sha256(bytes: impl AsRef<[u8]>) -> Self {
                let rendered = format!("{:x}", Sha256::digest(bytes.as_ref()));
                Self(rendered)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let raw = String::deserialize(deserializer)?;
                Self::parse(&raw).map_err(serde::de::Error::custom)
            }
        }
    };
}

release_digest!(ArtifactDigest, "SHA-256 over exact artifact bytes.");
release_digest!(SemanticInputDigest, "SHA-256 over a semantic-input transcript.");
release_digest!(IntentDigest, "SHA-256 over a release-intent transcript.");

/// Error returned when a durable digest is not canonical lowercase SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{digest_kind} must be exactly 64 lowercase hexadecimal characters, got {raw}")]
pub struct DigestParseError {
    digest_kind: &'static str,
    raw: String,
}

impl ArtifactDigest {
    /// Hashes the exact artifact bytes without normalization.
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        Self::from_sha256(bytes)
    }
}

impl SemanticInputDigest {
    /// Hashes a versioned semantic-input transcript.
    pub fn from_transcript(transcript: &CanonicalTranscript) -> Self {
        Self::from_sha256(transcript.as_bytes())
    }
}

impl IntentDigest {
    /// Hashes a versioned release-intent transcript.
    pub fn from_transcript(transcript: &CanonicalTranscript) -> Self {
        Self::from_sha256(transcript.as_bytes())
    }
}

/// Deterministic length-prefixed bytes for a durable release digest.
///
/// The transcript starts with a fixed protocol marker, a one-byte schema
/// version, and a length-prefixed domain. Each appended field is a
/// length-prefixed UTF-8 tag followed by a length-prefixed byte value. Domain
/// types own ordering of fields; this primitive preserves that order exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalTranscript {
    bytes: Vec<u8>,
}

impl CanonicalTranscript {
    const MARKER: &'static [u8] = b"callisto-release-transcript";
    const VERSION: u8 = 1;

    /// Starts the v1 transcript used for semantic input snapshots.
    pub fn semantic_input_v1() -> Self {
        Self::v1(b"semantic-input")
    }

    /// Starts the v1 transcript used for release intents.
    pub fn intent_v1() -> Self {
        Self::v1(b"release-intent")
    }

    fn v1(domain: &[u8]) -> Self {
        let mut bytes = Vec::with_capacity(Self::MARKER.len() + domain.len() + 16);
        Self::push_length_prefixed(&mut bytes, Self::MARKER);
        bytes.push(Self::VERSION);
        Self::push_length_prefixed(&mut bytes, domain);
        Self { bytes }
    }

    /// Appends one named UTF-8 field.
    pub fn push_str(&mut self, tag: &str, value: &str) {
        self.push_bytes(tag, value.as_bytes());
    }

    /// Appends one named byte field.
    pub fn push_bytes(&mut self, tag: &str, value: impl AsRef<[u8]>) {
        Self::push_length_prefixed(&mut self.bytes, tag.as_bytes());
        Self::push_length_prefixed(&mut self.bytes, value.as_ref());
    }

    /// Returns the exact canonical bytes, suitable only for hashing or test vectors.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn push_length_prefixed(out: &mut Vec<u8>, value: &[u8]) {
        let length = u64::try_from(value.len()).expect("usize always fits in u64 on supported platforms");
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ecosystem;

    #[test]
    fn release_package_id_requires_a_qualified_canonical_identity() {
        assert_eq!(
            ReleasePackageId::parse("cargo/shared").unwrap().to_string(),
            "cargo/shared"
        );
        assert_eq!(ReleasePackageId::parse("npm/shared").unwrap().to_string(), "npm/shared");
        assert!(ReleasePackageId::parse("shared").is_err());
        assert!(ReleasePackageId::parse("cargo:shared").is_err());
        assert!(ReleasePackageId::parse("unknown/shared").is_err());
        assert!(ReleasePackageId::parse("cargo/../shared").is_err());
        assert!(ReleasePackageId::new(Ecosystem::Cargo, "").is_err());
    }

    #[test]
    fn release_package_ids_do_not_conflate_ecosystems() {
        let cargo = ReleasePackageId::parse("cargo/shared").unwrap();
        let npm = ReleasePackageId::parse("npm/shared").unwrap();
        assert_ne!(cargo, npm);
        assert!(cargo < npm);
    }

    #[test]
    fn release_package_id_deserialization_rejects_a_bare_identity() {
        let parsed = serde_json::from_str::<ReleasePackageId>(r#""shared""#);
        assert!(parsed.is_err());
    }

    #[test]
    fn digests_are_distinct_validated_lowercase_hex_newtypes() {
        let hex = "a".repeat(64);
        assert_eq!(IntentDigest::parse(&hex).unwrap().as_str(), hex);
        assert!(IntentDigest::parse(&"A".repeat(64)).is_err());
        assert!(ArtifactDigest::parse("abcd").is_err());
        assert!(serde_json::from_str::<SemanticInputDigest>(r#""ABC""#).is_err());
    }

    #[test]
    fn artifact_digest_hashes_exact_bytes() {
        assert_eq!(
            ArtifactDigest::from_bytes(b"callisto").to_string(),
            "04d52bfb8ce8b5a37e6a15b8c002419d2555543855cb4a7972ca80b2d8eadbf0"
        );
        assert_ne!(
            ArtifactDigest::from_bytes(b"callisto"),
            ArtifactDigest::from_bytes(b"callisto\n")
        );
    }

    #[test]
    fn transcript_has_a_fixed_sha256_vector() {
        let mut transcript = CanonicalTranscript::semantic_input_v1();
        transcript.push_str("package", "cargo/callisto-model");
        transcript.push_str("version", "0.5.0");

        assert_eq!(
            SemanticInputDigest::from_transcript(&transcript).to_string(),
            "9dfd667345a133d98c1ae1ed8984b4161d8f27f310c09412b136007331201ec4"
        );
    }

    #[test]
    fn transcript_length_and_field_boundaries_affect_the_digest() {
        let mut split = CanonicalTranscript::semantic_input_v1();
        split.push_str("role", "publish");
        split.push_str("edge", "a->b");

        let mut joined = CanonicalTranscript::semantic_input_v1();
        joined.push_str("role", "publisha->b");

        assert_ne!(
            SemanticInputDigest::from_transcript(&split),
            SemanticInputDigest::from_transcript(&joined)
        );
    }

    #[test]
    fn transcript_domain_separation_prevents_intent_and_snapshot_collisions() {
        let mut snapshot = CanonicalTranscript::semantic_input_v1();
        snapshot.push_str("role", "publish");
        let mut intent = CanonicalTranscript::intent_v1();
        intent.push_str("role", "publish");

        assert_ne!(
            SemanticInputDigest::from_transcript(&snapshot).as_str(),
            IntentDigest::from_transcript(&intent).as_str()
        );
    }
}
