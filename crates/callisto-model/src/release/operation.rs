use std::cmp::Ordering;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use super::{version_ord_key, ArtifactSlotId, RegistryBindingDigest, ReleasePackageId};
use crate::{Ecosystem, RegistryKey, Version};

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
///
/// `PartialOrd`/`Ord` are derived rather than hand-maintained: every variant's
/// field types are already `Ord` (`RegistryBindingId`, `ArtifactSlotId`), so the
/// derived impl folds in every field of every variant -- including
/// `ArtifactSlotId::attestation_policy` -- consistently with the derived `Eq`,
/// and stays correct if a role variant ever gains a field.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum ReleaseOperationRole {
    RegistryPublish {
        #[serde(flatten)]
        registry: RegistryBindingId,
    },
    Tag,
    /// Creates the GitHub Release as a draft. Publication is a separate role.
    ForgeRelease,
    ArtifactUpload {
        slot: ArtifactSlotId,
    },
    /// Publishes the draft created by [`Self::ForgeRelease`], only after every
    /// artifact upload of that release has been confirmed.
    ForgePublish,
    /// Publishes one npm platform package of the operation's
    /// owning package, by directory. It is not a package of its own, so it has
    /// no tag, forge release, or decision entry.
    PlatformPublish {
        #[serde(flatten)]
        registry: RegistryBindingId,
        platform: PlatformPackageV1,
    },
}

/// The npm platform package a [`ReleaseOperationRole::PlatformPublish`] publishes:
/// its own npm name and its workspace-relative directory.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformPackageV1 {
    name: ReleasePackageId,
    directory: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlatformPackageV1Wire {
    name: ReleasePackageId,
    directory: String,
}

impl<'de> Deserialize<'de> for PlatformPackageV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PlatformPackageV1Wire::deserialize(deserializer)?;
        Self::new(wire.name, wire.directory).map_err(serde::de::Error::custom)
    }
}

impl PlatformPackageV1 {
    /// An npm package name and a relative, forward-slash directory with no `..`
    /// component and no leading `-`, so it can be passed to a client as a path.
    pub fn new(name: ReleasePackageId, directory: impl Into<String>) -> Result<Self, ReleaseOperationError> {
        let directory = directory.into();
        let safe_directory = !directory.is_empty()
            && !directory.starts_with(['/', '-'])
            && directory
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
            && directory
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b'@'));
        if name.ecosystem() != Ecosystem::Npm || !safe_directory {
            return Err(ReleaseOperationError::UnsafePlatformPackage);
        }
        Ok(Self { name, directory })
    }

    pub fn name(&self) -> &ReleasePackageId {
        &self.name
    }

    pub fn directory(&self) -> &str {
        &self.directory
    }
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
    ForgePublish,
    PlatformPublish {
        #[serde(rename = "registryKey")]
        registry_key: String,
        #[serde(rename = "bindingDigest")]
        binding_digest: RegistryBindingDigest,
        platform: PlatformPackageV1,
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
            ReleaseOperationRoleWire::ForgePublish => Ok(Self::ForgePublish),
            ReleaseOperationRoleWire::PlatformPublish {
                registry_key,
                binding_digest,
                platform,
            } => RegistryBindingId::new(registry_key, binding_digest)
                .map(|registry| Self::PlatformPublish { registry, platform })
                .map_err(serde::de::Error::custom),
        }
    }
}

impl ReleaseOperationRole {
    pub(super) fn artifact_slot(&self) -> Option<&ArtifactSlotId> {
        match self {
            Self::ArtifactUpload { slot } => Some(slot),
            Self::RegistryPublish { .. }
            | Self::Tag
            | Self::ForgeRelease
            | Self::ForgePublish
            | Self::PlatformPublish { .. } => None,
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
    version: String,
}

impl<'de> Deserialize<'de> for ReleaseOperationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ReleaseOperationIdWire::deserialize(deserializer)?;
        let version =
            Version::parse_for_ecosystem(&wire.version, wire.package.ecosystem()).map_err(serde::de::Error::custom)?;
        let id = Self {
            package: wire.package,
            role: wire.role,
            version,
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

    pub fn forge_publish(package: ReleasePackageId, version: Version) -> Self {
        Self {
            package,
            role: ReleaseOperationRole::ForgePublish,
            version,
        }
    }

    /// `owner` and `version` are the owning package's, which is what puts the
    /// operation inside the release decision.
    pub fn platform_publish(
        owner: ReleasePackageId,
        version: Version,
        registry: RegistryBindingId,
        platform: PlatformPackageV1,
    ) -> Self {
        Self {
            package: owner,
            role: ReleaseOperationRole::PlatformPublish { registry, platform },
            version,
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

// Hand-written rather than derived: `Version` has no `Ord` impl (SemVer and PEP 440
// aren't comparable via a single total order), so this struct can't `#[derive(Ord)]`
// directly. `role` delegates to `ReleaseOperationRole`'s own derived `Ord`, which
// folds in every field of every variant -- including `ArtifactSlotId::attestation_policy`
// for `ArtifactUpload` -- instead of hand-decomposing it into a lossy sort key.
// `version` goes through `version_ord_key` (grammar + raw), not bare `render()`: two
// `Version`s under different grammars can render identically (e.g. `"1.2.3"` is valid
// both as SemVer and PEP 440), which a `render()`-only key would wrongly fold together.
impl Ord for ReleaseOperationId {
    fn cmp(&self, other: &Self) -> Ordering {
        (&self.package, &self.role, version_ord_key(&self.version)).cmp(&(
            &other.package,
            &other.role,
            version_ord_key(&other.version),
        ))
    }
}

/// One canonical DAG node in a durable release intent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseOperation {
    pub(super) id: ReleaseOperationId,
    pub(super) prerequisites: Vec<ReleaseOperationId>,
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

    pub fn forge_publish(
        package: ReleasePackageId,
        version: Version,
        prerequisites: Vec<ReleaseOperationId>,
    ) -> Result<Self, ReleaseOperationError> {
        Self::new(ReleaseOperationId::forge_publish(package, version), prerequisites)
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
    #[error("platform package must be an npm name with a safe relative directory")]
    UnsafePlatformPackage,
}

/// The one charset rule for a registry key, applied wherever a key is minted
/// from configuration or from a durable intent: a key reaches argv (`cargo
/// publish --registry <key>`), so it stays
/// alphanumeric plus `-`/`_`.
///
/// # Errors
///
/// Returns [`ReleaseOperationError::MalformedRegistryKey`] for an empty,
/// over-long, or out-of-charset key.
pub fn validated_registry_key(raw: String) -> Result<RegistryKey, ReleaseOperationError> {
    // A leading `-` would make `cargo publish --registry <key>` read the key
    // as another option, the same hazard `TagName` already rules out.
    if raw.is_empty()
        || raw.len() > 128
        || raw.starts_with('-')
        || !raw
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ReleaseOperationError::MalformedRegistryKey { raw });
    }
    Ok(RegistryKey(raw))
}
