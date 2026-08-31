//! Durable, forge-neutral release identity and digest primitives.
//!
//! This module deliberately owns only values and deterministic byte encodings.
//! Workspace discovery and release authorization remain in callisto-graph.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
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

/// A named, credential-free fingerprint of one graph-derived release input.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseInputComponentV1 {
    pub kind: String,
    pub fingerprint: SemanticInputDigest,
}

/// The versioned semantic input projection built and compared by callisto-graph.
///
/// This model deliberately stores only already-derived fingerprints. Graph is
/// responsible for selecting the complete set of components and never puts
/// raw endpoints, credentials, command lines, or environment values here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseInputSnapshotV1 {
    pub schema_version: u8,
    pub source: SourceIdentity,
    pub components: Vec<ReleaseInputComponentV1>,
}

impl ReleaseInputSnapshotV1 {
    pub const SCHEMA_VERSION: u8 = 1;

    pub fn new(source: SourceIdentity, mut components: Vec<ReleaseInputComponentV1>) -> Self {
        components.sort();
        components.dedup();
        Self {
            schema_version: Self::SCHEMA_VERSION,
            source,
            components,
        }
    }

    pub fn digest(&self) -> SemanticInputDigest {
        let mut transcript = CanonicalTranscript::semantic_input_v1();
        transcript.push_bytes("schema", [self.schema_version]);
        match &self.source {
            SourceIdentity::GitCommit { sha } => transcript.push_str("source.git", sha.as_str()),
            SourceIdentity::HermeticContent { digest } => transcript.push_str("source.hermetic", digest.as_str()),
        }
        for component in &self.components {
            transcript.push_str("component.kind", &component.kind);
            transcript.push_str("component.fingerprint", component.fingerprint.as_str());
        }
        SemanticInputDigest::from_transcript(&transcript)
    }
}

/// A release operation's explicit role.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum ReleaseOperationRole {
    RegistryPublish { registry_key: RegistryKey },
    Tag,
    ForgeRelease,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
enum ReleaseOperationRoleWire {
    RegistryPublish { registry_key: String },
    Tag,
    ForgeRelease,
}

impl<'de> Deserialize<'de> for ReleaseOperationRole {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match ReleaseOperationRoleWire::deserialize(deserializer)? {
            ReleaseOperationRoleWire::RegistryPublish { registry_key } => validated_registry_key(registry_key)
                .map(|registry_key| Self::RegistryPublish { registry_key })
                .map_err(serde::de::Error::custom),
            ReleaseOperationRoleWire::Tag => Ok(Self::Tag),
            ReleaseOperationRoleWire::ForgeRelease => Ok(Self::ForgeRelease),
        }
    }
}

impl ReleaseOperationRole {
    fn discriminator(&self) -> u8 {
        match self {
            Self::RegistryPublish { .. } => 0,
            Self::Tag => 1,
            Self::ForgeRelease => 2,
        }
    }

    fn registry_key(&self) -> Option<&RegistryKey> {
        match self {
            Self::RegistryPublish { registry_key } => Some(registry_key),
            Self::Tag | Self::ForgeRelease => None,
        }
    }
}

/// Exact identity for one durable release operation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseOperationId {
    pub package: ReleasePackageId,
    pub role: ReleaseOperationRole,
    pub version: Version,
}

impl ReleaseOperationId {
    pub fn registry_publish(
        package: ReleasePackageId,
        version: Version,
        registry_key: impl Into<String>,
    ) -> Result<Self, ReleaseOperationError> {
        let registry_key = validated_registry_key(registry_key.into())?;
        Ok(Self {
            package,
            role: ReleaseOperationRole::RegistryPublish { registry_key },
            version,
        })
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
            self.role.registry_key().map(RegistryKey::as_str).unwrap_or(""),
        )
            .cmp(&(
                other.package.ecosystem().prefix(),
                other.package.name(),
                other.role.discriminator(),
                other.version.render(),
                other.role.registry_key().map(RegistryKey::as_str).unwrap_or(""),
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
        registry_key: impl Into<String>,
        prerequisites: Vec<ReleaseOperationId>,
    ) -> Result<Self, ReleaseOperationError> {
        Self::new(
            ReleaseOperationId::registry_publish(package, version, registry_key)?,
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

    pub fn new(
        id: ReleaseOperationId,
        mut prerequisites: Vec<ReleaseOperationId>,
    ) -> Result<Self, ReleaseOperationError> {
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
    pub snapshot: ReleaseInputSnapshotV1,
    pub trust_profile: ExecutionTrustProfileV1,
    pub operations: Vec<ReleaseOperation>,
    digest: IntentDigest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseIntentV1Wire {
    schema_version: u8,
    snapshot: ReleaseInputSnapshotV1,
    trust_profile: ExecutionTrustProfileV1,
    operations: Vec<ReleaseOperation>,
    digest: IntentDigest,
}

impl<'de> Deserialize<'de> for ReleaseIntentV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ReleaseIntentV1Wire::deserialize(deserializer)?;
        if wire.schema_version != Self::SCHEMA_VERSION
            || wire.snapshot.schema_version != ReleaseInputSnapshotV1::SCHEMA_VERSION
        {
            return Err(serde::de::Error::custom("unsupported release intent schema version"));
        }
        if wire.snapshot.components.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(serde::de::Error::custom("release input components are not canonical"));
        }
        let intent = Self::new(wire.snapshot, wire.trust_profile, wire.operations).map_err(serde::de::Error::custom)?;
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
        snapshot: ReleaseInputSnapshotV1,
        trust_profile: ExecutionTrustProfileV1,
        operations: Vec<ReleaseOperation>,
    ) -> Result<Self, ReleaseIntentError> {
        validate_operations(&operations)?;
        let digest = digest_intent(&snapshot, trust_profile, &operations);
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            snapshot,
            trust_profile,
            operations,
            digest,
        })
    }

    pub fn digest(&self) -> &IntentDigest {
        &self.digest
    }
}

fn digest_intent(
    snapshot: &ReleaseInputSnapshotV1,
    trust_profile: ExecutionTrustProfileV1,
    operations: &[ReleaseOperation],
) -> IntentDigest {
    let mut transcript = CanonicalTranscript::intent_v1();
    transcript.push_bytes("schema", [ReleaseIntentV1::SCHEMA_VERSION]);
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
    IntentDigest::from_transcript(&transcript)
}

fn operation_id_text(id: &ReleaseOperationId) -> String {
    let role = match &id.role {
        ReleaseOperationRole::RegistryPublish { registry_key } => format!("publish:{}", registry_key.as_str()),
        ReleaseOperationRole::Tag => "tag".to_string(),
        ReleaseOperationRole::ForgeRelease => "forge-release".to_string(),
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
                OperationState::Failed => OperationOutcome::Failed,
                OperationState::Blocked { reason } => OperationOutcome::Blocked { reason: *reason },
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

    #[test]
    fn release_operation_orders_exactly_and_rejects_invalid_prerequisites() {
        let package = ReleasePackageId::parse("cargo/callisto-model").unwrap();
        let version = Version::semver(1, 2, 3);
        let publish = ReleaseOperation::registry_publish(package.clone(), version.clone(), "cratesIo", vec![]).unwrap();
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
        let publish = ReleaseOperation::registry_publish(package.clone(), version.clone(), "cratesIo", vec![]).unwrap();
        let tag = ReleaseOperation::tag(package, version, vec![publish.id().clone()]).unwrap();
        let snapshot = ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]);

        let reversed = ReleaseIntentV1::new(
            snapshot.clone(),
            ExecutionTrustProfileV1::GitCommit,
            vec![tag.clone(), publish.clone()],
        );
        assert!(matches!(reversed, Err(ReleaseIntentError::NonCanonicalOperationOrder)));

        let duplicate = ReleaseIntentV1::new(
            snapshot,
            ExecutionTrustProfileV1::GitCommit,
            vec![publish.clone(), publish],
        );
        assert!(matches!(duplicate, Err(ReleaseIntentError::DuplicateOperation { .. })));

        let mut cycle_publish = ReleaseOperation::registry_publish(
            ReleasePackageId::parse("cargo/callisto-model").unwrap(),
            Version::semver(1, 2, 3),
            "cratesIo",
            vec![tag.id().clone()],
        )
        .unwrap();
        // The public constructor permits referring to a distinct operation; intent validation owns cycle detection.
        cycle_publish.prerequisites = vec![tag.id().clone()];
        let cycle = ReleaseIntentV1::new(
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
            ReleaseOperation::registry_publish(package, Version::semver(1, 2, 3), "cratesIo", vec![]).unwrap();
        let intent = ReleaseIntentV1::new(
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
    }

    #[test]
    fn state_and_receipt_wire_reject_unknown_schema_and_duplicate_operations() {
        let operation = ReleaseOperation::registry_publish(
            ReleasePackageId::parse("cargo/callisto-model").unwrap(),
            Version::semver(1, 2, 3),
            "cratesIo",
            vec![],
        )
        .unwrap();
        let intent = ReleaseIntentV1::new(
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
            .mark_terminal(
                operation.id(),
                OperationOutcome::Blocked {
                    reason: OperationBlockReason::StaleValidation,
                },
            )
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
        let first = ReleaseIntentV1::new(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("f".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![operation.clone()],
        )
        .unwrap();
        let different_digest = ReleaseIntentV1::new(
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
            "cratesIo",
            vec![],
        )
        .unwrap();
        let intent = ReleaseIntentV1::new(
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
