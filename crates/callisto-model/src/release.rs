//! Durable, forge-neutral release identity and digest primitives.
//!
//! This module deliberately owns only values and deterministic byte encodings.
//! Workspace discovery and release authorization remain in callisto-graph.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Read;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::{CommitSha, Ecosystem, PackageId, RegistryKey, Version};

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
release_digest!(DecisionDigest, "SHA-256 over a release-decision transcript.");
release_digest!(SemanticInputDigest, "SHA-256 over a semantic-input transcript.");
release_digest!(IntentDigest, "SHA-256 over a release-intent transcript.");
release_digest!(
    RegistryBindingDigest,
    "SHA-256 over a normalized, credential-free registry binding."
);

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

    /// Streams exact artifact bytes into SHA-256 without retaining the full
    /// asset in memory. Release artifacts can be substantially larger than
    /// manifests, so executor verification must use this path.
    pub fn from_reader(mut reader: impl Read) -> std::io::Result<(Self, u64)> {
        let mut hasher = Sha256::new();
        let mut length = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            length = length.checked_add(read as u64).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "artifact exceeds supported length")
            })?;
        }
        Ok((Self(format!("{:x}", hasher.finalize())), length))
    }
}

impl DecisionDigest {
    /// Hashes a versioned release-decision transcript.
    pub fn from_transcript(transcript: &CanonicalTranscript) -> Self {
        Self::from_sha256(transcript.as_bytes())
    }
}

/// The closed set of facts that can include a package in a durable release.
///
/// This is intentionally a model value: graph derives it, but every later
/// process can validate the exact approved roster without importing graph.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    deny_unknown_fields
)]
#[non_exhaustive]
pub enum ReleaseInclusionReason {
    Changeset,
    Inference,
    ExplicitSelection,
    LinkedGroup { group_id: String },
    FixedGroup { group_id: String },
    Cascade { from: ReleasePackageId, edge_kind: String },
    PreReleasePolicy { policy_id: String },
}

/// One exact package and version authorized by a release decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseDecisionEntry {
    pub package: ReleasePackageId,
    pub target_version: Version,
    pub reasons: Vec<ReleaseInclusionReason>,
}

/// Credential-free, deterministic release authority derived by callisto-graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseDecisionV1 {
    pub schema_version: u8,
    pub entries: Vec<ReleaseDecisionEntry>,
    pub digest: DecisionDigest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseDecisionV1Wire {
    schema_version: u8,
    entries: Vec<ReleaseDecisionEntry>,
    digest: DecisionDigest,
}

impl<'de> Deserialize<'de> for ReleaseDecisionV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ReleaseDecisionV1Wire::deserialize(deserializer)?;
        if wire.schema_version != Self::SCHEMA_VERSION {
            return Err(serde::de::Error::custom("unsupported release decision schema version"));
        }
        let decision = Self::new(wire.entries).map_err(serde::de::Error::custom)?;
        if decision.digest != wire.digest {
            return Err(serde::de::Error::custom(
                "release decision digest does not match canonical content",
            ));
        }
        Ok(decision)
    }
}

impl ReleaseDecisionV1 {
    pub const SCHEMA_VERSION: u8 = 1;

    /// Creates a canonical decision or rejects an ambiguous release roster.
    pub fn new(mut entries: Vec<ReleaseDecisionEntry>) -> Result<Self, ReleaseDecisionError> {
        if entries.is_empty() {
            return Err(ReleaseDecisionError::EmptyRoster);
        }
        for entry in &mut entries {
            entry.reasons.sort();
            entry.reasons.dedup();
            if entry.reasons.is_empty() {
                return Err(ReleaseDecisionError::MissingReason {
                    package: entry.package.clone(),
                });
            }
        }
        entries.sort_by(|left, right| left.package.cmp(&right.package));
        if entries.windows(2).any(|pair| pair[0].package == pair[1].package) {
            return Err(ReleaseDecisionError::DuplicatePackage);
        }
        let digest = decision_digest(&entries);
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            entries,
            digest,
        })
    }
}

fn decision_digest(entries: &[ReleaseDecisionEntry]) -> DecisionDigest {
    let mut transcript = CanonicalTranscript::decision_v1();
    for entry in entries {
        transcript.push_str("package", &entry.package.to_string());
        transcript.push_str("target-version", entry.target_version.render());
        for reason in &entry.reasons {
            transcript.push_str(
                "reason",
                &serde_json::to_string(reason).expect("closed reason serializes"),
            );
        }
    }
    DecisionDigest::from_transcript(&transcript)
}

/// Decision construction or validation errors.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseDecisionError {
    #[error("a durable release decision must contain at least one package")]
    EmptyRoster,
    #[error("release decision repeats a package")]
    DuplicatePackage,
    #[error("release decision entry for {package} has no inclusion reason")]
    MissingReason { package: ReleasePackageId },
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

impl RegistryBindingDigest {
    /// Hashes a graph-normalized registry binding without retaining its source
    /// endpoint in a durable model value.
    ///
    /// The caller is responsible for URL parsing, rejecting credentials and
    /// query/fragment data, and producing a canonical binding before calling
    /// this method. Only this digest is retained.
    pub fn from_normalized_binding(bytes: impl AsRef<[u8]>) -> Self {
        let mut transcript = CanonicalTranscript::registry_binding_v1();
        transcript.push_bytes("binding", bytes);
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

    /// Starts the v1 transcript used for release decisions.
    pub fn decision_v1() -> Self {
        Self::v1(b"release-decision")
    }

    /// Starts the v1 transcript used for release intents.
    pub fn intent_v1() -> Self {
        Self::v1(b"release-intent")
    }

    /// Starts the v1 transcript used for normalized registry bindings.
    fn registry_binding_v1() -> Self {
        Self::v1(b"registry-binding")
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

/// The durable source identity used to bind an approved release intent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum SourceIdentity {
    GitCommit { sha: CommitSha },
    HermeticContent { digest: SemanticInputDigest },
}

impl SourceIdentity {
    pub fn git_commit(raw_sha: impl AsRef<str>) -> Result<Self, crate::ModelError> {
        Ok(Self::GitCommit {
            sha: CommitSha::parse(raw_sha.as_ref())?,
        })
    }
}

/// The execution trust model selected for an intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionTrustProfileV1 {
    GitCommit,
    HermeticContent,
}

/// A closed semantic projection for one exact package in a release intent.
///
/// The model never accepts caller-defined component names: adding a durable
/// input must add a typed field and update the transcript below. This makes
/// the schema itself the inventory of release-authorizing inputs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleasePackageInputV1 {
    pub package: ReleasePackageId,
    pub fingerprint: SemanticInputDigest,
}

/// The versioned semantic input projection built and compared by callisto-graph.
///
/// This model deliberately stores only typed, already-derived fingerprints.
/// Graph owns the projection and never puts raw endpoints, credentials,
/// command lines, or environment values here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseInputSnapshotV1 {
    pub schema_version: u8,
    pub source: SourceIdentity,
    pub packages: Vec<ReleasePackageInputV1>,
}

impl ReleaseInputSnapshotV1 {
    pub const SCHEMA_VERSION: u8 = 1;

    pub fn new(
        source: SourceIdentity,
        mut packages: Vec<ReleasePackageInputV1>,
    ) -> Result<Self, ReleaseInputSnapshotError> {
        packages.sort();
        if packages.windows(2).any(|pair| pair[0].package == pair[1].package) {
            return Err(ReleaseInputSnapshotError::DuplicatePackage);
        }
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            source,
            packages,
        })
    }

    pub fn digest(&self) -> SemanticInputDigest {
        let mut transcript = CanonicalTranscript::semantic_input_v1();
        transcript.push_bytes("schema", [self.schema_version]);
        match &self.source {
            SourceIdentity::GitCommit { sha } => transcript.push_str("source.git", sha.as_str()),
            SourceIdentity::HermeticContent { digest } => transcript.push_str("source.hermetic", digest.as_str()),
        }
        for package in &self.packages {
            transcript.push_str("package", &package.package.to_string());
            transcript.push_str("package.fingerprint", package.fingerprint.as_str());
        }
        SemanticInputDigest::from_transcript(&transcript)
    }
}

/// Errors in a typed semantic input snapshot.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseInputSnapshotError {
    #[error("release input snapshot repeats a package")]
    DuplicatePackage,
}

/// A credential-free identity for one configured registry binding.
///
/// This is intentionally not a URL. `registry_key` says which configured
/// target was selected, while `binding_digest` commits to the graph-normalized
/// target that will receive the effect. Durable intents never retain the raw
/// endpoint, credentials, query parameters, or fragments.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistryBindingId {
    registry_key: RegistryKey,
    binding_digest: RegistryBindingDigest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistryBindingIdWire {
    registry_key: String,
    binding_digest: RegistryBindingDigest,
}

impl<'de> Deserialize<'de> for RegistryBindingId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RegistryBindingIdWire::deserialize(deserializer)?;
        Self::new(wire.registry_key, wire.binding_digest).map_err(serde::de::Error::custom)
    }
}

impl RegistryBindingId {
    /// Builds an exact, validated registry binding identity.
    pub fn new(
        registry_key: impl Into<String>,
        binding_digest: RegistryBindingDigest,
    ) -> Result<Self, ReleaseOperationError> {
        Ok(Self {
            registry_key: validated_registry_key(registry_key.into())?,
            binding_digest,
        })
    }

    /// Returns the logical configured registry key, never an endpoint.
    pub fn registry_key(&self) -> &RegistryKey {
        &self.registry_key
    }

    /// Returns the commitment to the normalized registry binding.
    pub fn binding_digest(&self) -> &RegistryBindingDigest {
        &self.binding_digest
    }
}

/// A release operation's explicit role.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum ReleaseOperationRole {
    RegistryPublish {
        #[serde(flatten)]
        registry: RegistryBindingId,
    },
    Tag,
    ForgeRelease,
    ArtifactUpload {
        slot: ArtifactSlotId,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
enum ReleaseOperationRoleWire {
    RegistryPublish {
        #[serde(rename = "registryKey")]
        registry_key: String,
        #[serde(rename = "bindingDigest")]
        binding_digest: RegistryBindingDigest,
    },
    Tag,
    ForgeRelease,
    ArtifactUpload {
        slot: ArtifactSlotId,
    },
}

impl<'de> Deserialize<'de> for ReleaseOperationRole {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match ReleaseOperationRoleWire::deserialize(deserializer)? {
            ReleaseOperationRoleWire::RegistryPublish {
                registry_key,
                binding_digest,
            } => RegistryBindingId::new(registry_key, binding_digest)
                .map(|registry| Self::RegistryPublish { registry })
                .map_err(serde::de::Error::custom),
            ReleaseOperationRoleWire::Tag => Ok(Self::Tag),
            ReleaseOperationRoleWire::ForgeRelease => Ok(Self::ForgeRelease),
            ReleaseOperationRoleWire::ArtifactUpload { slot } => Ok(Self::ArtifactUpload { slot }),
        }
    }
}

impl ReleaseOperationRole {
    fn discriminator(&self) -> u8 {
        match self {
            Self::RegistryPublish { .. } => 0,
            Self::Tag => 1,
            Self::ForgeRelease => 2,
            Self::ArtifactUpload { .. } => 3,
        }
    }

    fn registry(&self) -> Option<&RegistryBindingId> {
        match self {
            Self::RegistryPublish { registry } => Some(registry),
            Self::Tag | Self::ForgeRelease | Self::ArtifactUpload { .. } => None,
        }
    }

    fn artifact_slot(&self) -> Option<&ArtifactSlotId> {
        match self {
            Self::ArtifactUpload { slot } => Some(slot),
            Self::RegistryPublish { .. } | Self::Tag | Self::ForgeRelease => None,
        }
    }
}

/// Exact identity for one durable release operation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseOperationId {
    pub package: ReleasePackageId,
    pub role: ReleaseOperationRole,
    pub version: Version,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseOperationIdWire {
    package: ReleasePackageId,
    role: ReleaseOperationRole,
    version: Version,
}

impl<'de> Deserialize<'de> for ReleaseOperationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ReleaseOperationIdWire::deserialize(deserializer)?;
        let id = Self {
            package: wire.package,
            role: wire.role,
            version: wire.version,
        };
        id.validate_artifact_slot().map_err(serde::de::Error::custom)?;
        Ok(id)
    }
}

impl ReleaseOperationId {
    pub fn registry_publish(package: ReleasePackageId, version: Version, registry: RegistryBindingId) -> Self {
        Self {
            package,
            role: ReleaseOperationRole::RegistryPublish { registry },
            version,
        }
    }

    pub fn tag(package: ReleasePackageId, version: Version) -> Self {
        Self {
            package,
            role: ReleaseOperationRole::Tag,
            version,
        }
    }

    pub fn forge_release(package: ReleasePackageId, version: Version) -> Self {
        Self {
            package,
            role: ReleaseOperationRole::ForgeRelease,
            version,
        }
    }

    pub fn artifact_upload(slot: ArtifactSlotId) -> Self {
        Self {
            package: slot.package.clone(),
            version: slot.version.clone(),
            role: ReleaseOperationRole::ArtifactUpload { slot },
        }
    }

    fn validate_artifact_slot(&self) -> Result<(), ReleaseOperationError> {
        let Some(slot) = self.role.artifact_slot() else {
            return Ok(());
        };
        if slot.package != self.package || slot.version != self.version {
            return Err(ReleaseOperationError::MismatchedArtifactSlot);
        }
        Ok(())
    }
}

impl PartialOrd for ReleaseOperationId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ReleaseOperationId {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            self.package.ecosystem().prefix(),
            self.package.name(),
            self.role.discriminator(),
            self.version.render(),
            self.role
                .registry()
                .map(|registry| registry.registry_key().as_str())
                .unwrap_or(""),
            self.role
                .registry()
                .map(|registry| registry.binding_digest().as_str())
                .unwrap_or(""),
            self.role
                .artifact_slot()
                .map(|slot| format!("{}|{}", slot.platform, slot.asset_name))
                .unwrap_or_default(),
        )
            .cmp(&(
                other.package.ecosystem().prefix(),
                other.package.name(),
                other.role.discriminator(),
                other.version.render(),
                other
                    .role
                    .registry()
                    .map(|registry| registry.registry_key().as_str())
                    .unwrap_or(""),
                other
                    .role
                    .registry()
                    .map(|registry| registry.binding_digest().as_str())
                    .unwrap_or(""),
                other
                    .role
                    .artifact_slot()
                    .map(|slot| format!("{}|{}", slot.platform, slot.asset_name))
                    .unwrap_or_default(),
            ))
    }
}

/// One canonical DAG node in a durable release intent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseOperation {
    id: ReleaseOperationId,
    prerequisites: Vec<ReleaseOperationId>,
}

impl ReleaseOperation {
    pub fn registry_publish(
        package: ReleasePackageId,
        version: Version,
        registry: RegistryBindingId,
        prerequisites: Vec<ReleaseOperationId>,
    ) -> Result<Self, ReleaseOperationError> {
        Self::new(
            ReleaseOperationId::registry_publish(package, version, registry),
            prerequisites,
        )
    }

    pub fn tag(
        package: ReleasePackageId,
        version: Version,
        prerequisites: Vec<ReleaseOperationId>,
    ) -> Result<Self, ReleaseOperationError> {
        Self::new(ReleaseOperationId::tag(package, version), prerequisites)
    }

    pub fn forge_release(
        package: ReleasePackageId,
        version: Version,
        prerequisites: Vec<ReleaseOperationId>,
    ) -> Result<Self, ReleaseOperationError> {
        Self::new(ReleaseOperationId::forge_release(package, version), prerequisites)
    }

    pub fn artifact_upload(
        slot: ArtifactSlotId,
        prerequisites: Vec<ReleaseOperationId>,
    ) -> Result<Self, ReleaseOperationError> {
        Self::new(ReleaseOperationId::artifact_upload(slot), prerequisites)
    }

    pub fn new(
        id: ReleaseOperationId,
        mut prerequisites: Vec<ReleaseOperationId>,
    ) -> Result<Self, ReleaseOperationError> {
        id.validate_artifact_slot()?;
        prerequisites.sort();
        if prerequisites.iter().any(|prerequisite| prerequisite == &id) {
            return Err(ReleaseOperationError::SelfPrerequisite { id: Box::new(id) });
        }
        if prerequisites.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ReleaseOperationError::DuplicatePrerequisite { id: Box::new(id) });
        }
        Ok(Self { id, prerequisites })
    }

    pub fn id(&self) -> &ReleaseOperationId {
        &self.id
    }

    pub fn prerequisites(&self) -> &[ReleaseOperationId] {
        &self.prerequisites
    }
}

/// Errors in exact release-operation construction.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseOperationError {
    #[error("registry key `{raw}` is malformed")]
    MalformedRegistryKey { raw: String },
    #[error("release operation `{id:?}` cannot require itself")]
    SelfPrerequisite { id: Box<ReleaseOperationId> },
    #[error("release operation `{id:?}` has duplicate prerequisites")]
    DuplicatePrerequisite { id: Box<ReleaseOperationId> },
    #[error("artifact upload operation identity must match its slot package and version")]
    MismatchedArtifactSlot,
}

/// Immutable GitHub provenance policy declared by a binary release slot.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitHubAttestationPolicyV1 {
    pub repository: String,
    pub workflow_path: String,
    pub workflow_commit: CommitSha,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GitHubAttestationPolicyV1Wire {
    repository: String,
    workflow_path: String,
    workflow_commit: CommitSha,
}

impl GitHubAttestationPolicyV1 {
    pub fn new(
        repository: impl Into<String>,
        workflow_path: impl Into<String>,
        workflow_commit: CommitSha,
    ) -> Result<Self, ArtifactSlotError> {
        let repository = repository.into();
        let workflow_path = workflow_path.into();
        if !is_safe_github_repository(&repository) || !is_safe_workflow_path(&workflow_path) {
            return Err(ArtifactSlotError::UnsafeSlotComponent);
        }
        Ok(Self {
            repository,
            workflow_path,
            workflow_commit,
        })
    }
}

impl<'de> Deserialize<'de> for GitHubAttestationPolicyV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = GitHubAttestationPolicyV1Wire::deserialize(deserializer)?;
        Self::new(wire.repository, wire.workflow_path, wire.workflow_commit).map_err(serde::de::Error::custom)
    }
}

/// Exact binary asset declaration authorized by a durable release intent.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactSlotId {
    pub package: ReleasePackageId,
    pub version: Version,
    pub platform: String,
    pub asset_name: String,
    pub attestation_policy: GitHubAttestationPolicyV1,
}

impl ArtifactSlotId {
    pub fn new(
        package: ReleasePackageId,
        version: Version,
        platform: impl Into<String>,
        asset_name: impl Into<String>,
        repository: impl Into<String>,
        workflow_path: impl Into<String>,
        workflow_commit: CommitSha,
    ) -> Result<Self, ArtifactSlotError> {
        let platform = platform.into();
        let asset_name = asset_name.into();
        let policy = GitHubAttestationPolicyV1::new(repository, workflow_path, workflow_commit)?;
        if !is_safe_artifact_component(&platform) || !is_safe_artifact_component(&asset_name) {
            return Err(ArtifactSlotError::UnsafeSlotComponent);
        }
        Ok(Self {
            package,
            version,
            platform,
            asset_name,
            attestation_policy: policy,
        })
    }
}

impl PartialOrd for ArtifactSlotId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ArtifactSlotId {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            &self.package,
            self.version.render(),
            &self.platform,
            &self.asset_name,
            &self.attestation_policy.repository,
            &self.attestation_policy.workflow_path,
            &self.attestation_policy.workflow_commit,
        )
            .cmp(&(
                &other.package,
                other.version.render(),
                &other.platform,
                &other.asset_name,
                &other.attestation_policy.repository,
                &other.attestation_policy.workflow_path,
                &other.attestation_policy.workflow_commit,
            ))
    }
}

fn is_safe_github_repository(value: &str) -> bool {
    let Some((owner, repository)) = value.split_once('/') else {
        return false;
    };
    !owner.is_empty() && !repository.is_empty() && !repository.contains('/')
}

fn is_safe_workflow_path(value: &str) -> bool {
    value.starts_with(".github/workflows/")
        && value.ends_with(".yml")
        && !value.contains("..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
}

fn is_safe_artifact_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.contains("..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Credential-free GitHub attestation policy and verified provenance facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitHubArtifactAttestationV1 {
    pub repository: String,
    pub workflow_path: String,
    pub workflow_commit: CommitSha,
    pub subject_digest: ArtifactDigest,
    pub source_commit: CommitSha,
}

/// One exact built asset and its verified provenance binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactManifestEntryV1 {
    pub slot: ArtifactSlotId,
    pub digest: ArtifactDigest,
    pub byte_length: u64,
    pub attestation: GitHubArtifactAttestationV1,
}

/// Typed build output bound to one durable intent and source commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactManifestV1 {
    pub schema_version: u8,
    pub intent_digest: IntentDigest,
    pub source_commit: CommitSha,
    pub entries: Vec<ArtifactManifestEntryV1>,
}

impl ArtifactManifestV1 {
    pub const SCHEMA_VERSION: u8 = 1;

    pub fn new(
        intent: &ReleaseIntentV1,
        mut entries: Vec<ArtifactManifestEntryV1>,
    ) -> Result<Self, ArtifactManifestError> {
        let SourceIdentity::GitCommit { sha } = &intent.snapshot.source else {
            return Err(ArtifactManifestError::NonGitSource);
        };
        entries.sort_by(|left, right| left.slot.cmp(&right.slot));
        if entries.windows(2).any(|pair| pair[0].slot == pair[1].slot) {
            return Err(ArtifactManifestError::DuplicateSlot);
        }
        let expected = &intent.artifact_slots;
        if entries.iter().map(|entry| &entry.slot).ne(expected.iter()) {
            return Err(ArtifactManifestError::MismatchedSlotRoster);
        }
        if entries.iter().any(|entry| {
            entry.digest != entry.attestation.subject_digest
                || entry.attestation.source_commit != *sha
                || entry.attestation.repository != entry.slot.attestation_policy.repository
                || entry.attestation.workflow_path != entry.slot.attestation_policy.workflow_path
                || entry.attestation.workflow_commit != entry.slot.attestation_policy.workflow_commit
        }) {
            return Err(ArtifactManifestError::MismatchedAttestation);
        }
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            intent_digest: intent.digest.clone(),
            source_commit: sha.clone(),
            entries,
        })
    }

    pub fn validate_for_intent(&self, intent: &ReleaseIntentV1) -> Result<(), ArtifactManifestError> {
        if self.schema_version != Self::SCHEMA_VERSION || self.intent_digest != intent.digest {
            return Err(ArtifactManifestError::MismatchedIntent);
        }
        Self::new(intent, self.entries.clone()).map(|_| ())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ArtifactSlotError {
    #[error("artifact slot platform and asset name must be safe relative identifiers")]
    UnsafeSlotComponent,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ArtifactManifestError {
    #[error("artifact manifests require a Git commit source")]
    NonGitSource,
    #[error("artifact manifest repeats an artifact slot")]
    DuplicateSlot,
    #[error("artifact manifest slot roster differs from the intent")]
    MismatchedSlotRoster,
    #[error("artifact attestation does not bind its exact bytes and intent source commit")]
    MismatchedAttestation,
    #[error("artifact manifest is bound to a different intent or schema")]
    MismatchedIntent,
}

fn validated_registry_key(raw: String) -> Result<RegistryKey, ReleaseOperationError> {
    if raw.is_empty()
        || raw.len() > 128
        || !raw
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ReleaseOperationError::MalformedRegistryKey { raw });
    }
    Ok(RegistryKey(raw))
}

/// An immutable, canonical authorization intent. Graph owns its construction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseIntentV1 {
    pub schema_version: u8,
    pub decision: ReleaseDecisionV1,
    pub snapshot: ReleaseInputSnapshotV1,
    pub trust_profile: ExecutionTrustProfileV1,
    pub operations: Vec<ReleaseOperation>,
    pub artifact_slots: Vec<ArtifactSlotId>,
    digest: IntentDigest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseIntentV1Wire {
    schema_version: u8,
    decision: ReleaseDecisionV1,
    snapshot: ReleaseInputSnapshotV1,
    trust_profile: ExecutionTrustProfileV1,
    operations: Vec<ReleaseOperation>,
    artifact_slots: Vec<ArtifactSlotId>,
    digest: IntentDigest,
}

impl<'de> Deserialize<'de> for ReleaseIntentV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ReleaseIntentV1Wire::deserialize(deserializer)?;
        if wire.schema_version != Self::SCHEMA_VERSION
            || wire.decision.schema_version != ReleaseDecisionV1::SCHEMA_VERSION
            || wire.snapshot.schema_version != ReleaseInputSnapshotV1::SCHEMA_VERSION
        {
            return Err(serde::de::Error::custom("unsupported release intent schema version"));
        }
        if wire.snapshot.packages.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(serde::de::Error::custom("release input packages are not canonical"));
        }
        let intent = Self::new(
            wire.decision,
            wire.snapshot,
            wire.trust_profile,
            wire.operations,
            wire.artifact_slots,
        )
        .map_err(serde::de::Error::custom)?;
        if intent.digest != wire.digest {
            return Err(serde::de::Error::custom(
                "release intent digest does not match canonical content",
            ));
        }
        Ok(intent)
    }
}

impl ReleaseIntentV1 {
    pub const SCHEMA_VERSION: u8 = 1;

    pub fn new(
        decision: ReleaseDecisionV1,
        snapshot: ReleaseInputSnapshotV1,
        trust_profile: ExecutionTrustProfileV1,
        operations: Vec<ReleaseOperation>,
        mut artifact_slots: Vec<ArtifactSlotId>,
    ) -> Result<Self, ReleaseIntentError> {
        if !matches!(trust_profile, ExecutionTrustProfileV1::GitCommit)
            || !matches!(snapshot.source, SourceIdentity::GitCommit { .. })
        {
            return Err(ReleaseIntentError::UnsupportedTrustProfile);
        }
        validate_operations(&operations)?;
        validate_operation_roster(&decision, &operations)?;
        artifact_slots.sort();
        if artifact_slots.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ReleaseIntentError::DuplicateArtifactSlot);
        }
        if artifact_slots.iter().any(|slot| {
            !decision
                .entries
                .iter()
                .any(|entry| entry.package == slot.package && entry.target_version == slot.version)
        }) {
            return Err(ReleaseIntentError::ArtifactSlotOutsideDecision);
        }
        validate_artifact_upload_roster(&operations, &artifact_slots)?;
        let digest = digest_intent(&decision, &snapshot, trust_profile, &operations, &artifact_slots);
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            decision,
            snapshot,
            trust_profile,
            operations,
            artifact_slots,
            digest,
        })
    }

    pub fn digest(&self) -> &IntentDigest {
        &self.digest
    }
}

fn digest_intent(
    decision: &ReleaseDecisionV1,
    snapshot: &ReleaseInputSnapshotV1,
    trust_profile: ExecutionTrustProfileV1,
    operations: &[ReleaseOperation],
    artifact_slots: &[ArtifactSlotId],
) -> IntentDigest {
    let mut transcript = CanonicalTranscript::intent_v1();
    transcript.push_bytes("schema", [ReleaseIntentV1::SCHEMA_VERSION]);
    transcript.push_str("decision", decision.digest.as_str());
    transcript.push_str("snapshot", snapshot.digest().as_str());
    transcript.push_str(
        "trust-profile",
        match trust_profile {
            ExecutionTrustProfileV1::GitCommit => "git-commit",
            ExecutionTrustProfileV1::HermeticContent => "hermetic-content",
        },
    );
    for operation in operations {
        transcript.push_str("operation", &operation_id_text(&operation.id));
        for prerequisite in &operation.prerequisites {
            transcript.push_str("prerequisite", &operation_id_text(prerequisite));
        }
    }
    for slot in artifact_slots {
        transcript.push_str("artifact-slot.package", &slot.package.to_string());
        transcript.push_str("artifact-slot.version", slot.version.render());
        transcript.push_str("artifact-slot.platform", &slot.platform);
        transcript.push_str("artifact-slot.asset", &slot.asset_name);
        transcript.push_str("artifact-slot.repository", &slot.attestation_policy.repository);
        transcript.push_str("artifact-slot.workflow-path", &slot.attestation_policy.workflow_path);
        transcript.push_str(
            "artifact-slot.workflow-commit",
            slot.attestation_policy.workflow_commit.as_str(),
        );
    }
    IntentDigest::from_transcript(&transcript)
}

fn validate_operation_roster(
    decision: &ReleaseDecisionV1,
    operations: &[ReleaseOperation],
) -> Result<(), ReleaseIntentError> {
    let roster = &decision.entries;
    for operation in operations {
        if !roster
            .iter()
            .any(|entry| entry.package == operation.id.package && entry.target_version == operation.id.version)
        {
            return Err(ReleaseIntentError::OperationOutsideDecision {
                id: Box::new(operation.id.clone()),
            });
        }
    }
    Ok(())
}

fn validate_artifact_upload_roster(
    operations: &[ReleaseOperation],
    artifact_slots: &[ArtifactSlotId],
) -> Result<(), ReleaseIntentError> {
    let mut uploads = operations
        .iter()
        .filter_map(|operation| operation.id.role.artifact_slot())
        .collect::<Vec<_>>();
    uploads.sort();
    if uploads.iter().copied().ne(artifact_slots.iter()) {
        return Err(ReleaseIntentError::MismatchedArtifactUploadRoster);
    }
    Ok(())
}

fn operation_id_text(id: &ReleaseOperationId) -> String {
    let role = match &id.role {
        ReleaseOperationRole::RegistryPublish { registry } => format!(
            "publish:{}:{}",
            registry.registry_key().as_str(),
            registry.binding_digest().as_str()
        ),
        ReleaseOperationRole::Tag => "tag".to_string(),
        ReleaseOperationRole::ForgeRelease => "forge-release".to_string(),
        ReleaseOperationRole::ArtifactUpload { slot } => format!(
            "artifact-upload:{}:{}:{}:{}:{}:{}",
            slot.platform,
            slot.asset_name,
            slot.attestation_policy.repository,
            slot.attestation_policy.workflow_path,
            slot.attestation_policy.workflow_commit.as_str(),
            slot.version.render(),
        ),
    };
    format!("{}|{}|{}", id.package, role, id.version.render())
}

fn validate_operations(operations: &[ReleaseOperation]) -> Result<(), ReleaseIntentError> {
    let mut ids = BTreeSet::new();
    for operation in operations {
        if !ids.insert(operation.id.clone()) {
            return Err(ReleaseIntentError::DuplicateOperation {
                id: Box::new(operation.id.clone()),
            });
        }
    }
    for operation in operations {
        if operation.prerequisites.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(ReleaseIntentError::NonCanonicalPrerequisiteOrder {
                id: Box::new(operation.id.clone()),
            });
        }
        for prerequisite in &operation.prerequisites {
            if prerequisite == &operation.id {
                return Err(ReleaseIntentError::SelfPrerequisite {
                    id: Box::new(operation.id.clone()),
                });
            }
            if !ids.contains(prerequisite) {
                return Err(ReleaseIntentError::UnknownPrerequisite {
                    id: Box::new(operation.id.clone()),
                    prerequisite: Box::new(prerequisite.clone()),
                });
            }
        }
    }
    let edges: BTreeMap<_, _> = operations
        .iter()
        .map(|operation| (operation.id.clone(), operation.prerequisites.clone()))
        .collect();
    let canonical = stable_kahn_order(&edges)?;
    if operations.iter().map(ReleaseOperation::id).ne(canonical.iter()) {
        return Err(ReleaseIntentError::NonCanonicalOperationOrder);
    }
    Ok(())
}

fn stable_kahn_order(
    prerequisites: &BTreeMap<ReleaseOperationId, Vec<ReleaseOperationId>>,
) -> Result<Vec<ReleaseOperationId>, ReleaseIntentError> {
    let mut remaining: BTreeMap<_, usize> = prerequisites
        .iter()
        .map(|(id, prerequisites)| (id.clone(), prerequisites.len()))
        .collect();
    let mut dependents = BTreeMap::<ReleaseOperationId, Vec<ReleaseOperationId>>::new();
    for (id, prerequisites) in prerequisites {
        for prerequisite in prerequisites {
            dependents.entry(prerequisite.clone()).or_default().push(id.clone());
        }
    }
    let mut ready: BTreeSet<_> = remaining
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(id.clone()))
        .collect();
    let mut ordered = Vec::with_capacity(prerequisites.len());
    while let Some(id) = ready.pop_first() {
        ordered.push(id.clone());
        for dependent in dependents.get(&id).into_iter().flatten() {
            let count = remaining.get_mut(dependent).expect("known DAG node");
            *count -= 1;
            if *count == 0 {
                ready.insert(dependent.clone());
            }
        }
    }
    if ordered.len() != prerequisites.len() {
        let id = remaining
            .into_iter()
            .find_map(|(id, count)| (count != 0).then_some(id))
            .expect("a nonempty cyclic DAG has a remaining node");
        return Err(ReleaseIntentError::Cycle { id: Box::new(id) });
    }
    Ok(ordered)
}

/// Validation failure for a durable intent DAG.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseIntentError {
    #[error("durable release intents support only clean Git commit trust in v1")]
    UnsupportedTrustProfile,
    #[error("release operation `{id:?}` is not authorized by the embedded release decision")]
    OperationOutsideDecision { id: Box<ReleaseOperationId> },
    #[error("release intent repeats an artifact slot")]
    DuplicateArtifactSlot,
    #[error("artifact slot is not authorized by the embedded release decision")]
    ArtifactSlotOutsideDecision,
    #[error("artifact upload operations must exactly match the declared artifact slots")]
    MismatchedArtifactUploadRoster,
    #[error("release operations are not in canonical order")]
    NonCanonicalOperationOrder,
    #[error("duplicate release operation `{id:?}`")]
    DuplicateOperation { id: Box<ReleaseOperationId> },
    #[error("prerequisites for `{id:?}` are not in canonical order")]
    NonCanonicalPrerequisiteOrder { id: Box<ReleaseOperationId> },
    #[error("release operation `{id:?}` requires unknown operation `{prerequisite:?}`")]
    UnknownPrerequisite {
        id: Box<ReleaseOperationId>,
        prerequisite: Box<ReleaseOperationId>,
    },
    #[error("release operation `{id:?}` requires itself")]
    SelfPrerequisite { id: Box<ReleaseOperationId> },
    #[error("release operation DAG contains a cycle at `{id:?}`")]
    Cycle { id: Box<ReleaseOperationId> },
}

/// A closed, credential-safe reason why an operation was blocked.
///
/// This deliberately carries no endpoint, command output, or arbitrary error
/// text: durable execution records can safely outlive the process that made
/// the observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum OperationBlockReason {
    /// An existing remote or VCS object does not exactly match the intent.
    ConflictingExistingOperation,
    /// A completed attempt could not be conclusively observed after restart.
    IndeterminateAttempt,
    /// Fresh validation no longer agrees with the approved intent.
    StaleValidation,
    /// An exact prerequisite has not reached a successful terminal outcome.
    UnmetPrerequisite,
}

/// A terminal outcome safe to persist in a receipt. No arbitrary error text is durable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
#[non_exhaustive]
pub enum OperationOutcome {
    Published,
    AlreadySatisfied,
    Failed,
    Blocked { reason: OperationBlockReason },
}

/// Crash-safe state for an intent-bound execution. Pending and Attempting are nonterminal.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema)]
#[schemars(with = "ReleaseExecutionStateV1Wire")]
pub struct ReleaseExecutionStateV1 {
    schema_version: u8,
    intent_digest: IntentDigest,
    operations: BTreeMap<ReleaseOperationId, OperationState>,
}

/// The on-wire shape deliberately uses entries rather than a JSON object: a
/// JSON object cannot express an exact `ReleaseOperationId` key and parsers
/// commonly discard duplicate keys before domain validation sees them.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseExecutionStateV1Wire {
    schema_version: u8,
    intent_digest: IntentDigest,
    operations: Vec<OperationStateEntryV1>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperationStateEntryV1 {
    operation: ReleaseOperationId,
    state: OperationState,
}

impl Serialize for ReleaseExecutionStateV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        ReleaseExecutionStateV1Wire {
            schema_version: self.schema_version,
            intent_digest: self.intent_digest.clone(),
            operations: self
                .operations
                .iter()
                .map(|(operation, state)| OperationStateEntryV1 {
                    operation: operation.clone(),
                    state: *state,
                })
                .collect(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ReleaseExecutionStateV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ReleaseExecutionStateV1Wire::deserialize(deserializer)?;
        Self::from_wire(wire).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum OperationState {
    Pending,
    Attempting,
    Published,
    AlreadySatisfied,
    Failed,
    Blocked { reason: OperationBlockReason },
}

impl ReleaseExecutionStateV1 {
    pub const SCHEMA_VERSION: u8 = 1;

    /// Starts execution for exactly the roster authorized by `intent`.
    pub fn pending(intent: &ReleaseIntentV1) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            intent_digest: intent.digest.clone(),
            operations: intent
                .operations
                .iter()
                .map(|operation| (operation.id.clone(), OperationState::Pending))
                .collect(),
        }
    }

    pub fn intent_digest(&self) -> &IntentDigest {
        &self.intent_digest
    }

    /// Validates that this state belongs to this exact intent and operation roster.
    ///
    /// Resume and reconciliation must call this before treating any state as
    /// evidence or before making a remote/VCS mutation.
    pub fn validate_for_intent(&self, intent: &ReleaseIntentV1) -> Result<(), ReleaseStateError> {
        if self.schema_version != Self::SCHEMA_VERSION {
            return Err(ReleaseStateError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        if self.intent_digest != intent.digest {
            return Err(ReleaseStateError::MismatchedIntent);
        }
        let expected: BTreeSet<_> = intent.operations.iter().map(|operation| operation.id.clone()).collect();
        let actual: BTreeSet<_> = self.operations.keys().cloned().collect();
        if actual != expected {
            return Err(ReleaseStateError::MismatchedOperationRoster);
        }
        Ok(())
    }

    /// Returns the persisted state for an exact operation identity.
    pub fn operation_state(&self, id: &ReleaseOperationId) -> Option<OperationState> {
        self.operations.get(id).copied()
    }

    fn from_wire(wire: ReleaseExecutionStateV1Wire) -> Result<Self, ReleaseStateError> {
        if wire.schema_version != Self::SCHEMA_VERSION {
            return Err(ReleaseStateError::UnsupportedSchema {
                found: wire.schema_version,
            });
        }
        let mut operations = BTreeMap::new();
        for entry in wire.operations {
            if operations.insert(entry.operation.clone(), entry.state).is_some() {
                return Err(ReleaseStateError::DuplicateOperation {
                    id: Box::new(entry.operation),
                });
            }
        }
        Ok(Self {
            schema_version: wire.schema_version,
            intent_digest: wire.intent_digest,
            operations,
        })
    }

    pub fn mark_attempting(&mut self, id: &ReleaseOperationId) -> Result<(), ReleaseStateError> {
        let state = self
            .operations
            .get_mut(id)
            .ok_or_else(|| ReleaseStateError::UnknownOperation {
                id: Box::new(id.clone()),
            })?;
        if *state != OperationState::Pending {
            return Err(ReleaseStateError::InvalidTransition {
                id: Box::new(id.clone()),
                from: *state,
            });
        }
        *state = OperationState::Attempting;
        Ok(())
    }

    pub fn mark_terminal(
        &mut self,
        id: &ReleaseOperationId,
        outcome: OperationOutcome,
    ) -> Result<(), ReleaseStateError> {
        let state = self
            .operations
            .get_mut(id)
            .ok_or_else(|| ReleaseStateError::UnknownOperation {
                id: Box::new(id.clone()),
            })?;
        if *state != OperationState::Attempting {
            return Err(ReleaseStateError::InvalidTransition {
                id: Box::new(id.clone()),
                from: *state,
            });
        }
        *state = match outcome {
            OperationOutcome::Published => OperationState::Published,
            OperationOutcome::AlreadySatisfied => OperationState::AlreadySatisfied,
            OperationOutcome::Failed => OperationState::Failed,
            OperationOutcome::Blocked { reason } => OperationState::Blocked { reason },
        };
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseStateError {
    #[error("unsupported release execution state schema version {found}")]
    UnsupportedSchema { found: u8 },
    #[error("release state is bound to a different intent")]
    MismatchedIntent,
    #[error("release state operation roster differs from the bound intent")]
    MismatchedOperationRoster,
    #[error("release state contains duplicate operation `{id:?}`")]
    DuplicateOperation { id: Box<ReleaseOperationId> },
    #[error("release state does not contain operation `{id:?}`")]
    UnknownOperation { id: Box<ReleaseOperationId> },
    #[error("operation `{id:?}` cannot transition from {from:?}")]
    InvalidTransition {
        id: Box<ReleaseOperationId>,
        from: OperationState,
    },
}

/// A terminal receipt derived only from a complete execution state.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema)]
#[schemars(with = "ReleaseReceiptV1Wire")]
pub struct ReleaseReceiptV1 {
    schema_version: u8,
    intent_digest: IntentDigest,
    outcomes: BTreeMap<ReleaseOperationId, OperationOutcome>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseReceiptV1Wire {
    schema_version: u8,
    intent_digest: IntentDigest,
    outcomes: Vec<OperationOutcomeEntryV1>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperationOutcomeEntryV1 {
    operation: ReleaseOperationId,
    outcome: OperationOutcome,
}

impl Serialize for ReleaseReceiptV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        ReleaseReceiptV1Wire {
            schema_version: self.schema_version,
            intent_digest: self.intent_digest.clone(),
            outcomes: self
                .outcomes
                .iter()
                .map(|(operation, outcome)| OperationOutcomeEntryV1 {
                    operation: operation.clone(),
                    outcome: *outcome,
                })
                .collect(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ReleaseReceiptV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ReleaseReceiptV1Wire::deserialize(deserializer)?;
        Self::from_wire(wire).map_err(serde::de::Error::custom)
    }
}

impl ReleaseReceiptV1 {
    pub const SCHEMA_VERSION: u8 = 1;

    /// Constructs a receipt only when state is complete and exact for `intent`.
    pub fn from_state(intent: &ReleaseIntentV1, state: &ReleaseExecutionStateV1) -> Result<Self, ReleaseReceiptError> {
        state
            .validate_for_intent(intent)
            .map_err(ReleaseReceiptError::InvalidState)?;
        let mut outcomes = BTreeMap::new();
        for (id, operation_state) in &state.operations {
            let outcome = match operation_state {
                OperationState::Published => OperationOutcome::Published,
                OperationState::AlreadySatisfied => OperationOutcome::AlreadySatisfied,
                OperationState::Failed | OperationState::Blocked { .. } => {
                    return Err(ReleaseReceiptError::NonSuccessfulOperation {
                        id: Box::new(id.clone()),
                    });
                }
                OperationState::Pending | OperationState::Attempting => {
                    return Err(ReleaseReceiptError::NonTerminalOperation {
                        id: Box::new(id.clone()),
                    });
                }
            };
            outcomes.insert(id.clone(), outcome);
        }
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            intent_digest: state.intent_digest.clone(),
            outcomes,
        })
    }

    pub fn intent_digest(&self) -> &IntentDigest {
        &self.intent_digest
    }

    /// Validates exact intent and operation-roster binding before reconciliation.
    pub fn validate_for_intent(&self, intent: &ReleaseIntentV1) -> Result<(), ReleaseReceiptError> {
        if self.schema_version != Self::SCHEMA_VERSION {
            return Err(ReleaseReceiptError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        if self.intent_digest != intent.digest {
            return Err(ReleaseReceiptError::MismatchedIntent);
        }
        let expected: BTreeSet<_> = intent.operations.iter().map(|operation| operation.id.clone()).collect();
        let actual: BTreeSet<_> = self.outcomes.keys().cloned().collect();
        if actual != expected {
            return Err(ReleaseReceiptError::MismatchedOperationRoster);
        }
        Ok(())
    }

    fn from_wire(wire: ReleaseReceiptV1Wire) -> Result<Self, ReleaseReceiptError> {
        if wire.schema_version != Self::SCHEMA_VERSION {
            return Err(ReleaseReceiptError::UnsupportedSchema {
                found: wire.schema_version,
            });
        }
        let mut outcomes = BTreeMap::new();
        for entry in wire.outcomes {
            if !matches!(
                entry.outcome,
                OperationOutcome::Published | OperationOutcome::AlreadySatisfied
            ) {
                return Err(ReleaseReceiptError::NonSuccessfulOperation {
                    id: Box::new(entry.operation),
                });
            }
            if outcomes.insert(entry.operation.clone(), entry.outcome).is_some() {
                return Err(ReleaseReceiptError::DuplicateOperation {
                    id: Box::new(entry.operation),
                });
            }
        }
        Ok(Self {
            schema_version: wire.schema_version,
            intent_digest: wire.intent_digest,
            outcomes,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseReceiptError {
    #[error("unsupported release receipt schema version {found}")]
    UnsupportedSchema { found: u8 },
    #[error("release receipt is bound to a different intent")]
    MismatchedIntent,
    #[error("release receipt operation roster differs from the bound intent")]
    MismatchedOperationRoster,
    #[error("release receipt contains duplicate operation `{id:?}")]
    DuplicateOperation { id: Box<ReleaseOperationId> },
    #[error("release receipt cannot be derived from invalid state: {0}")]
    InvalidState(ReleaseStateError),
    #[error("release operation `{id:?}` is not terminal")]
    NonTerminalOperation { id: Box<ReleaseOperationId> },
    #[error("release operation `{id:?}` did not complete successfully")]
    NonSuccessfulOperation { id: Box<ReleaseOperationId> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ecosystem;

    /// `rename_all` on an internally-tagged enum only renames the `kind`
    /// discriminant, never fields inside a variant -- the exact gap that
    /// shipped `ReleasePrActionV1`'s snake_case `pull_request_number` to
    /// production (see `.changeset/fix-release-pr-action-camel-case.md`).
    /// `ReleaseInclusionReason` had the identical gap.
    #[test]
    fn inclusion_reason_variant_fields_serialize_as_camel_case() {
        let fixed = ReleaseInclusionReason::FixedGroup {
            group_id: "workspace".to_string(),
        };
        let value = serde_json::to_value(&fixed).unwrap();
        assert_eq!(value["kind"], "fixedGroup");
        assert_eq!(value["groupId"], "workspace");
        assert!(value.get("group_id").is_none());

        let linked = ReleaseInclusionReason::LinkedGroup {
            group_id: "demo".to_string(),
        };
        let value = serde_json::to_value(&linked).unwrap();
        assert_eq!(value["kind"], "linkedGroup");
        assert_eq!(value["groupId"], "demo");

        let cascade = ReleaseInclusionReason::Cascade {
            from: ReleasePackageId::new(Ecosystem::Cargo, "upstream").unwrap(),
            edge_kind: "peer".to_string(),
        };
        let value = serde_json::to_value(&cascade).unwrap();
        assert_eq!(value["kind"], "cascade");
        assert_eq!(value["edgeKind"], "peer");
        assert!(value.get("edge_kind").is_none());

        let policy = ReleaseInclusionReason::PreReleasePolicy {
            policy_id: "beta".to_string(),
        };
        let value = serde_json::to_value(&policy).unwrap();
        assert_eq!(value["kind"], "preReleasePolicy");
        assert_eq!(value["policyId"], "beta");
    }

    fn registry_binding(key: &str) -> RegistryBindingId {
        RegistryBindingId::new(
            key,
            RegistryBindingDigest::from_normalized_binding(format!("https://{key}.example.test/index")),
        )
        .unwrap()
    }

    fn test_intent(
        snapshot: Result<ReleaseInputSnapshotV1, ReleaseInputSnapshotError>,
        trust_profile: ExecutionTrustProfileV1,
        operations: Vec<ReleaseOperation>,
    ) -> Result<ReleaseIntentV1, ReleaseIntentError> {
        test_intent_with_slots(snapshot, trust_profile, operations, vec![])
    }

    fn test_intent_with_slots(
        snapshot: Result<ReleaseInputSnapshotV1, ReleaseInputSnapshotError>,
        trust_profile: ExecutionTrustProfileV1,
        operations: Vec<ReleaseOperation>,
        slots: Vec<ArtifactSlotId>,
    ) -> Result<ReleaseIntentV1, ReleaseIntentError> {
        let mut entries = Vec::new();
        for operation in &operations {
            if !entries.iter().any(|entry: &ReleaseDecisionEntry| {
                entry.package == operation.id.package && entry.target_version == operation.id.version
            }) {
                entries.push(ReleaseDecisionEntry {
                    package: operation.id.package.clone(),
                    target_version: operation.id.version.clone(),
                    reasons: vec![ReleaseInclusionReason::ExplicitSelection],
                });
            }
        }
        ReleaseIntentV1::new(
            ReleaseDecisionV1::new(entries).expect("test operations define a roster"),
            snapshot.expect("test snapshot is valid"),
            trust_profile,
            operations,
            slots,
        )
    }

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
    fn artifact_manifest_requires_exact_slot_digest_and_source_binding() {
        let package = ReleasePackageId::parse("cargo/demo").unwrap();
        let version = Version::semver(1, 0, 0);
        let slot = ArtifactSlotId::new(
            package.clone(),
            version.clone(),
            "x86_64-unknown-linux-gnu",
            "demo.tar.gz",
            "orin-dx/callisto",
            ".github/workflows/release.yml",
            CommitSha::parse(&"b".repeat(40)).unwrap(),
        )
        .unwrap();
        let operation = ReleaseOperation::artifact_upload(slot.clone(), vec![]).unwrap();
        let intent = test_intent_with_slots(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![operation],
            vec![slot.clone()],
        )
        .unwrap();
        let digest = ArtifactDigest::from_bytes(b"binary");
        let manifest = ArtifactManifestV1::new(
            &intent,
            vec![ArtifactManifestEntryV1 {
                slot,
                digest: digest.clone(),
                byte_length: 6,
                attestation: GitHubArtifactAttestationV1 {
                    repository: "orin-dx/callisto".to_string(),
                    workflow_path: ".github/workflows/release.yml".to_string(),
                    workflow_commit: CommitSha::parse(&"b".repeat(40)).unwrap(),
                    subject_digest: digest,
                    source_commit: CommitSha::parse(&"a".repeat(40)).unwrap(),
                },
            }],
        )
        .unwrap();
        manifest.validate_for_intent(&intent).unwrap();
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
    fn transcript_domain_separation_prevents_decision_intent_and_snapshot_collisions() {
        let mut snapshot = CanonicalTranscript::semantic_input_v1();
        snapshot.push_str("role", "publish");
        let mut decision = CanonicalTranscript::decision_v1();
        decision.push_str("role", "publish");
        let mut intent = CanonicalTranscript::intent_v1();
        intent.push_str("role", "publish");

        assert_ne!(
            SemanticInputDigest::from_transcript(&snapshot).as_str(),
            IntentDigest::from_transcript(&intent).as_str()
        );
        assert_ne!(
            DecisionDigest::from_transcript(&decision).as_str(),
            IntentDigest::from_transcript(&intent).as_str()
        );
    }

    #[test]
    fn registry_binding_is_credential_free_and_part_of_operation_identity() {
        let first = RegistryBindingId::new(
            "cratesIo",
            RegistryBindingDigest::from_normalized_binding("https://index.example.test/v1"),
        )
        .unwrap();
        let second = RegistryBindingId::new(
            "cratesIo",
            RegistryBindingDigest::from_normalized_binding("https://mirror.example.test/v1"),
        )
        .unwrap();
        assert_ne!(first, second);

        let wire = serde_json::to_string(&first).unwrap();
        assert!(wire.contains("registryKey"));
        assert!(wire.contains("bindingDigest"));
        assert!(!wire.contains("example.test"));
        assert!(!wire.contains("token"));

        let package = ReleasePackageId::parse("cargo/callisto-model").unwrap();
        let version = Version::semver(1, 2, 3);
        let first_operation = ReleaseOperationId::registry_publish(package.clone(), version.clone(), first);
        let second_operation = ReleaseOperationId::registry_publish(package, version, second);
        assert_ne!(first_operation, second_operation);
        assert_ne!(
            operation_id_text(&first_operation),
            operation_id_text(&second_operation)
        );
    }

    #[test]
    fn release_operation_orders_exactly_and_rejects_invalid_prerequisites() {
        let package = ReleasePackageId::parse("cargo/callisto-model").unwrap();
        let version = Version::semver(1, 2, 3);
        let publish =
            ReleaseOperation::registry_publish(package.clone(), version.clone(), registry_binding("cratesIo"), vec![])
                .unwrap();
        let tag = ReleaseOperation::tag(package, version, vec![publish.id().clone()]).unwrap();

        assert!(publish.id() < tag.id());
        assert!(ReleaseOperation::tag(
            ReleasePackageId::parse("cargo/callisto-model").unwrap(),
            Version::semver(1, 2, 3),
            vec![tag.id().clone()],
        )
        .is_err());
    }

    #[test]
    fn release_intent_rejects_noncanonical_duplicate_and_cyclic_dags() {
        let package = ReleasePackageId::parse("cargo/callisto-model").unwrap();
        let version = Version::semver(1, 2, 3);
        let publish =
            ReleaseOperation::registry_publish(package.clone(), version.clone(), registry_binding("cratesIo"), vec![])
                .unwrap();
        let tag = ReleaseOperation::tag(package, version, vec![publish.id().clone()]).unwrap();
        let snapshot = ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]);

        let reversed = test_intent(
            snapshot.clone(),
            ExecutionTrustProfileV1::GitCommit,
            vec![tag.clone(), publish.clone()],
        );
        assert!(matches!(reversed, Err(ReleaseIntentError::NonCanonicalOperationOrder)));

        let duplicate = test_intent(
            snapshot,
            ExecutionTrustProfileV1::GitCommit,
            vec![publish.clone(), publish],
        );
        assert!(matches!(duplicate, Err(ReleaseIntentError::DuplicateOperation { .. })));

        let mut cycle_publish = ReleaseOperation::registry_publish(
            ReleasePackageId::parse("cargo/callisto-model").unwrap(),
            Version::semver(1, 2, 3),
            registry_binding("cratesIo"),
            vec![tag.id().clone()],
        )
        .unwrap();
        // The public constructor permits referring to a distinct operation; intent validation owns cycle detection.
        cycle_publish.prerequisites = vec![tag.id().clone()];
        let cycle = test_intent(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("b".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![cycle_publish, tag],
        );
        assert!(matches!(cycle, Err(ReleaseIntentError::Cycle { .. })));
    }

    #[test]
    fn receipt_is_bound_to_intent_and_requires_terminal_outcomes() {
        let package = ReleasePackageId::parse("cargo/callisto-model").unwrap();
        let operation =
            ReleaseOperation::registry_publish(package, Version::semver(1, 2, 3), registry_binding("cratesIo"), vec![])
                .unwrap();
        let intent = test_intent(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("c".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![operation.clone()],
        )
        .unwrap();
        let pending = ReleaseExecutionStateV1::pending(&intent);
        assert!(ReleaseReceiptV1::from_state(&intent, &pending).is_err());

        let mut complete = pending;
        complete.mark_attempting(operation.id()).unwrap();
        complete
            .mark_terminal(operation.id(), OperationOutcome::Published)
            .unwrap();
        let receipt = ReleaseReceiptV1::from_state(&intent, &complete).unwrap();
        assert_eq!(receipt.intent_digest(), intent.digest());

        let mut failed = ReleaseExecutionStateV1::pending(&intent);
        failed.mark_attempting(operation.id()).unwrap();
        failed.mark_terminal(operation.id(), OperationOutcome::Failed).unwrap();
        assert!(matches!(
            ReleaseReceiptV1::from_state(&intent, &failed),
            Err(ReleaseReceiptError::NonSuccessfulOperation { .. })
        ));
    }

    #[test]
    fn state_and_receipt_wire_reject_unknown_schema_and_duplicate_operations() {
        let operation = ReleaseOperation::registry_publish(
            ReleasePackageId::parse("cargo/callisto-model").unwrap(),
            Version::semver(1, 2, 3),
            registry_binding("cratesIo"),
            vec![],
        )
        .unwrap();
        let intent = test_intent(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("e".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![operation.clone()],
        )
        .unwrap();
        let state = ReleaseExecutionStateV1::pending(&intent);
        let mut state_wire = serde_json::to_value(&state).unwrap();
        state_wire["unexpected"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<ReleaseExecutionStateV1>(state_wire).is_err());

        let mut duplicate_state = serde_json::to_value(&state).unwrap();
        let duplicate = duplicate_state["operations"][0].clone();
        duplicate_state["operations"].as_array_mut().unwrap().push(duplicate);
        assert!(serde_json::from_value::<ReleaseExecutionStateV1>(duplicate_state).is_err());

        let mut complete = state;
        complete.mark_attempting(operation.id()).unwrap();
        complete
            .mark_terminal(operation.id(), OperationOutcome::Published)
            .unwrap();
        let receipt = ReleaseReceiptV1::from_state(&intent, &complete).unwrap();
        let mut receipt_wire = serde_json::to_value(receipt).unwrap();
        receipt_wire["schemaVersion"] = serde_json::Value::from(2);
        assert!(serde_json::from_value::<ReleaseReceiptV1>(receipt_wire).is_err());
    }

    #[test]
    fn state_and_receipt_require_exact_intent_digest_and_roster() {
        let package = ReleasePackageId::parse("cargo/callisto-model").unwrap();
        let operation = ReleaseOperation::tag(package.clone(), Version::semver(1, 2, 3), vec![]).unwrap();
        let different_operation = ReleaseOperation::forge_release(package, Version::semver(1, 2, 3), vec![]).unwrap();
        let first = test_intent(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("f".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![operation.clone()],
        )
        .unwrap();
        let different_digest = test_intent(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("0".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![operation.clone()],
        )
        .unwrap();
        let state = ReleaseExecutionStateV1::pending(&first);
        assert!(matches!(
            state.validate_for_intent(&different_digest),
            Err(ReleaseStateError::MismatchedIntent)
        ));
        let mut wrong_roster_wire = serde_json::to_value(&state).unwrap();
        wrong_roster_wire["operations"][0]["operation"] = serde_json::to_value(different_operation.id()).unwrap();
        let wrong_roster = serde_json::from_value::<ReleaseExecutionStateV1>(wrong_roster_wire).unwrap();
        assert!(matches!(
            wrong_roster.validate_for_intent(&first),
            Err(ReleaseStateError::MismatchedOperationRoster)
        ));

        let mut complete = state;
        complete.mark_attempting(operation.id()).unwrap();
        complete
            .mark_terminal(operation.id(), OperationOutcome::Published)
            .unwrap();
        let receipt = ReleaseReceiptV1::from_state(&first, &complete).unwrap();
        assert!(matches!(
            receipt.validate_for_intent(&different_digest),
            Err(ReleaseReceiptError::MismatchedIntent)
        ));
        let mut wrong_receipt_wire = serde_json::to_value(&receipt).unwrap();
        wrong_receipt_wire["outcomes"][0]["operation"] = serde_json::to_value(different_operation.id()).unwrap();
        let wrong_receipt = serde_json::from_value::<ReleaseReceiptV1>(wrong_receipt_wire).unwrap();
        assert!(matches!(
            wrong_receipt.validate_for_intent(&first),
            Err(ReleaseReceiptError::MismatchedOperationRoster)
        ));
    }

    #[test]
    fn durable_intent_wire_rejects_forged_digest_unknown_fields_and_invalid_dag() {
        let operation = ReleaseOperation::registry_publish(
            ReleasePackageId::parse("cargo/callisto-model").unwrap(),
            Version::semver(1, 2, 3),
            registry_binding("cratesIo"),
            vec![],
        )
        .unwrap();
        let intent = test_intent(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("d".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![operation],
        )
        .unwrap();
        let value = serde_json::to_value(intent).unwrap();

        let mut forged_digest = value.clone();
        forged_digest["digest"] = serde_json::Value::String("0".repeat(64));
        assert!(serde_json::from_value::<ReleaseIntentV1>(forged_digest).is_err());

        let mut unknown = value.clone();
        unknown["unexpected"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<ReleaseIntentV1>(unknown).is_err());

        let mut bad_registry = value;
        bad_registry["operations"][0]["id"]["role"]["registryKey"] =
            serde_json::Value::String("https://token@example.test/registry".to_string());
        assert!(serde_json::from_value::<ReleaseIntentV1>(bad_registry).is_err());
    }
}
