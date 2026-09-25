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

use crate::release_observation::{
    ProviderEvidenceV1, ProviderObservationError, ProviderObservationV1, ReleaseOperationObservationV1,
};
use crate::release_transition::OperationEvent;
use crate::{CommitSha, Ecosystem, PackageId, RegistryKey, Version, VersionGrammar};

/// Sort key for a `Version` field inside a hand-written `Ord` impl.
///
/// `(grammar, raw)` is exactly the subset of `Version`'s own derived
/// `PartialEq`/`Eq` fields needed to keep a containing type's `Ord`
/// consistent with its `Eq`: `parsed` is a pure function of `(grammar,
/// raw)` (see `Version::parse`), so it draws no further distinctions once
/// grammar and raw already agree. `render()` (== `raw`) alone is NOT
/// enough -- the same literal string parses under more than one grammar
/// (e.g. `"1.2.3"` is valid SemVer *and* PEP 440), so two `Version`s with
/// different `grammar` but identical `render()` output would compare
/// unequal via derived `Eq` yet `Ordering::Equal` via a `render()`-only
/// `Ord`, violating the invariant `BTreeSet`/`BTreeMap` require:
/// `a.cmp(b) == Equal` iff `a == b`.
fn version_ord_key(v: &Version) -> (VersionGrammar, &str) {
    (v.grammar(), v.render())
}

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
        if let Err(reason) = check_release_package_name(ecosystem, name) {
            return Err(ReleasePackageIdParseError::UnsafeName {
                raw: format!("{}/{}", ecosystem.prefix(), name),
                reason,
            });
        }
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
    #[error("release package identity {raw} is unsafe: {reason}")]
    UnsafeName { raw: String, reason: &'static str },
}

/// The charset every release package name must satisfy, per ecosystem.
///
/// A release package name is not just a label: it becomes an argv word
/// (`npm view <name>@<version>`, `pnpm publish --filter=<name>`) and a URL
/// path segment (the cargo sparse index, the PyPI JSON API). `TagName` and
/// `GitHubRepository` already rule out the same hazards for their own
/// identities; this is the equivalent rule for package names, applied where
/// [`ReleasePackageId`] is minted so no release path can skip it.
///
/// Each ecosystem's real grammar is enforced rather than a lowest common
/// denominator, because a name outside it cannot name a publishable package
/// anyway and a plan-time rejection is cheaper than a half-published release.
fn check_release_package_name(ecosystem: Ecosystem, name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("name is empty");
    }
    if name.len() > 214 {
        return Err("name is longer than any registry accepts");
    }
    if name.starts_with('-') {
        return Err("a leading `-` would be read as a command-line option");
    }
    if name.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("name contains a control character or whitespace");
    }
    // `/` is allowed only as the single separator of an npm `@scope/name`,
    // handled below; every other character here can change the meaning of a
    // URL path or an argv word.
    if name.contains(['?', '#', '%', '\\', ':', '@']) && !(ecosystem == Ecosystem::Npm && name.starts_with('@')) {
        return Err("name contains a character that can alter a URL path or argv word");
    }
    match ecosystem {
        Ecosystem::Cargo => check_cargo_name(name),
        Ecosystem::Npm => check_npm_name(name),
        Ecosystem::Pypi => check_pypi_name(name),
        // No publish path is implemented for these, so only the shared
        // argv/URL rules above apply; a `/` would still traverse a URL path.
        _ => {
            if name.contains('/') {
                Err("name contains a path separator")
            } else {
                Ok(())
            }
        }
    }
}

/// crates.io: ASCII alphanumerics, `-` and `_`, starting with a letter or `_`.
fn check_cargo_name(name: &str) -> Result<(), &'static str> {
    let first = name.chars().next().unwrap_or('\0');
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err("a cargo crate name must start with a letter or underscore");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err("a cargo crate name may only contain letters, digits, `-` and `_`");
    }
    Ok(())
}

/// npm: lowercase, URL-safe, with an optional single `@scope/` prefix.
fn check_npm_name(name: &str) -> Result<(), &'static str> {
    let (scoped, unscoped) = match name.strip_prefix('@') {
        Some(rest) => {
            let (scope, package) = rest
                .split_once('/')
                .ok_or("an npm scope must be followed by `/` and a package name")?;
            check_npm_segment(scope, true)?;
            (true, package)
        }
        None => (false, name),
    };
    check_npm_segment(unscoped, scoped)
}

/// npm allows legacy uppercase names and, only under a scope, a leading `.` or `_`.
fn check_npm_segment(segment: &str, scoped: bool) -> Result<(), &'static str> {
    let first = segment.chars().next().unwrap_or('\0');
    let leading_ok = first.is_ascii_alphanumeric() || (scoped && matches!(first, '.' | '_'));
    if !leading_ok {
        return Err(
            "an npm name segment must start with a letter or digit (a scoped name may also start with `.` or `_`)",
        );
    }
    if !segment
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err("an npm name segment may only contain letters, digits, `-`, `_` and `.`");
    }
    Ok(())
}

/// PEP 508: alphanumerics separated by single `-`, `_` or `.` runs, starting
/// and ending with an alphanumeric.
fn check_pypi_name(name: &str) -> Result<(), &'static str> {
    let boundary_ok = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    if !boundary_ok(name.chars().next()) || !boundary_ok(name.chars().last()) {
        return Err("a PyPI project name must start and end with a letter or digit");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err("a PyPI project name may only contain letters, digits, `-`, `_` and `.`");
    }
    Ok(())
}

/// An exact `owner/repo` GitHub repository identity.
///
/// Replaces three previously independent, inconsistent ad hoc validations of
/// the same concept (a bare `split_once('/')` check with no character-class
/// restriction, a stricter ASCII-alphanumeric-plus-`-_.` check, and an
/// entirely unvalidated `format!("{owner}/{repository}")`); every caller now
/// parses through this single charset rule instead.
#[derive(Clone, Debug, PartialEq, Eq, Hash, JsonSchema)]
#[schemars(with = "String")]
pub struct GitHubRepository {
    owner: String,
    repo: String,
}

impl GitHubRepository {
    /// Parses an exact `owner/repo` GitHub repository identity.
    ///
    /// A trailing `.git` on the repository is dropped and both parts are
    /// lowercased. Both `owner` and `repo` must be non-empty, ASCII alphanumeric plus
    /// `-`, `_`, and `.`, and must not start or end with `-`. This is
    /// deliberately one charset rule applied uniformly to both parts,
    /// matching GitHub's own allowed repository-name charset closely enough
    /// to reject the unsafe inputs that matter (for example, embedded
    /// whitespace) without re-implementing GitHub's full, occasionally
    /// stricter, username rules.
    pub fn parse(s: &str) -> Result<Self, GitHubRepositoryParseError> {
        let Some((owner, repo)) = s.split_once('/') else {
            return Err(GitHubRepositoryParseError::MissingSeparator { raw: s.to_string() });
        };
        if repo.contains('/') {
            return Err(GitHubRepositoryParseError::TooManyParts { raw: s.to_string() });
        }
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        if matches!(owner, "." | "..") || matches!(repo, "." | "..") {
            return Err(GitHubRepositoryParseError::DotComponent { raw: s.to_string() });
        }
        if !is_valid_github_repository_part(owner) {
            return Err(GitHubRepositoryParseError::InvalidOwner { raw: s.to_string() });
        }
        if !is_valid_github_repository_part(repo) {
            return Err(GitHubRepositoryParseError::InvalidRepo { raw: s.to_string() });
        }
        // GitHub names are case-insensitive, so the lowercase form is the identity.
        Ok(Self {
            owner: owner.to_ascii_lowercase(),
            repo: repo.to_ascii_lowercase(),
        })
    }

    /// Returns the owner component.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Returns the repository-name component.
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// Renders the canonical `owner/repo` slug, for example for `gh --repo`.
    pub fn as_slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

impl fmt::Display for GitHubRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)
    }
}

impl FromStr for GitHubRepository {
    type Err = GitHubRepositoryParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl PartialOrd for GitHubRepository {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for GitHubRepository {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.owner.as_str(), self.repo.as_str()).cmp(&(other.owner.as_str(), other.repo.as_str()))
    }
}

impl Serialize for GitHubRepository {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_slug())
    }
}

impl<'de> Deserialize<'de> for GitHubRepository {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

fn is_valid_github_repository_part(part: &str) -> bool {
    !part.is_empty()
        && !part.starts_with('-')
        && !part.ends_with('-')
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// Errors produced while parsing a [`GitHubRepository`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum GitHubRepositoryParseError {
    #[error("GitHub repository `{raw}` must be in owner/repo form")]
    MissingSeparator { raw: String },
    #[error("GitHub repository `{raw}` has more than one `/`")]
    TooManyParts { raw: String },
    #[error("GitHub repository `{raw}` has an invalid owner")]
    InvalidOwner { raw: String },
    #[error("GitHub repository `{raw}` has an invalid repository name")]
    InvalidRepo { raw: String },
    #[error("GitHub repository `{raw}` has a `.` or `..` component")]
    DotComponent { raw: String },
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
    LinkedGroup {
        group_id: String,
    },
    FixedGroup {
        group_id: String,
    },
    Cascade {
        from: ReleasePackageId,
        edge_kind: String,
    },
    PreReleasePolicy {
        policy_id: String,
    },
    /// The package's current version has no tag yet.
    UnreleasedVersion,
}

/// One exact package and version authorized by a release decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseDecisionEntry {
    pub package: ReleasePackageId,
    pub target_version: Version,
    pub reasons: Vec<ReleaseInclusionReason>,
}

/// Rejects a wire schema version that doesn't match `expected`, with the
/// exact "unsupported {type_name} schema version" wording every schema-gated
/// `Deserialize` impl in this crate uses. Centralizing this comparison means
/// a schema bump that adds a nested versioned field (as `ReleaseIntentV1`
/// does for `decision`/`snapshot`) can't forget to gate it the way a
/// hand-copied check could.
pub(crate) fn check_schema_version<E: serde::de::Error>(found: u8, expected: u8, type_name: &str) -> Result<(), E> {
    if found != expected {
        return Err(E::custom(format!(
            "unsupported {type_name} schema version {found}; this build reads version {expected}"
        )));
    }
    Ok(())
}

/// Accepts every decision schema in [`ReleaseDecisionV1::READABLE_SCHEMA_VERSIONS`].
fn check_decision_schema_version<E: serde::de::Error>(found: u8, type_name: &str) -> Result<(), E> {
    if ReleaseDecisionV1::READABLE_SCHEMA_VERSIONS.contains(&found) {
        return Ok(());
    }
    Err(E::custom(format!(
        "unsupported {type_name} schema version {found}; this build reads versions {:?}",
        ReleaseDecisionV1::READABLE_SCHEMA_VERSIONS
    )))
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
        check_decision_schema_version::<D::Error>(wire.schema_version, "release decision")?;
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
    pub const SCHEMA_VERSION: u8 = 2;
    /// Version 1 differs only in lacking `unreleasedVersion`, so a committed v1 decision still reads.
    pub const READABLE_SCHEMA_VERSIONS: [u8; 2] = [1, 2];

    /// Creates a canonical decision or rejects an ambiguous release roster.
    #[allow(clippy::result_large_err)]
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

        // A fixed or linked group always converges every member to one
        // shared target version -- `solve_cascade` guarantees this when a
        // decision is derived from a version plan. Entries tag their own
        // group membership via `FixedGroup`/`LinkedGroup` reasons, so this
        // invariant is checkable from the decision's own content alone, with
        // no `GroupTable`/workspace-config access needed. Rejecting a
        // violation here means it also applies to a decision deserialized
        // from a committed release-decision file (`Deserialize` calls
        // `new()`), not just one freshly derived from a plan.
        let mut group_targets: BTreeMap<&str, &Version> = BTreeMap::new();
        for entry in &entries {
            for reason in &entry.reasons {
                let group_id = match reason {
                    ReleaseInclusionReason::FixedGroup { group_id }
                    | ReleaseInclusionReason::LinkedGroup { group_id } => group_id.as_str(),
                    _ => continue,
                };
                match group_targets.get(group_id) {
                    Some(existing) if **existing != entry.target_version => {
                        return Err(ReleaseDecisionError::DivergentGroupTarget {
                            group_id: group_id.to_string(),
                            left: (*existing).clone(),
                            right: entry.target_version.clone(),
                        });
                    }
                    _ => {
                        group_targets.insert(group_id, &entry.target_version);
                    }
                }
            }
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
    #[error("release decision claims divergent target versions for group `{group_id}`: {left} vs {right}")]
    DivergentGroupTarget {
        group_id: String,
        left: Version,
        right: Version,
    },
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
    fn artifact_slot(&self) -> Option<&ArtifactSlotId> {
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

/// The one workflow whose runs may attest release artifacts; CI asserts the file exists here.
pub const RELEASE_COORDINATOR_WORKFLOW_PATH: &str = ".github/workflows/callisto-release.yml";

/// Immutable GitHub provenance policy declared by a binary release slot.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitHubAttestationPolicyV1 {
    pub repository: GitHubRepository,
    pub workflow_path: String,
    pub workflow_commit: CommitSha,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GitHubAttestationPolicyV1Wire {
    repository: GitHubRepository,
    workflow_path: String,
    workflow_commit: CommitSha,
}

impl GitHubAttestationPolicyV1 {
    pub fn new(
        repository: GitHubRepository,
        workflow_path: impl Into<String>,
        workflow_commit: CommitSha,
    ) -> Result<Self, ArtifactSlotError> {
        let workflow_path = workflow_path.into();
        if !is_safe_workflow_path(&workflow_path) {
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
        repository: GitHubRepository,
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
// `version` goes through `version_ord_key` (grammar + raw), not bare `render()` --
// same residual-`.render()` defect as `ReleaseOperationId::cmp` above; see
// `version_ord_key`'s doc comment for why bare `render()` breaks the
// `a.cmp(b) == Equal` iff `a == b` invariant `BTreeSet`/`BTreeMap` require.
impl Ord for ArtifactSlotId {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            &self.package,
            version_ord_key(&self.version),
            &self.platform,
            &self.asset_name,
            &self.attestation_policy.repository,
            &self.attestation_policy.workflow_path,
            &self.attestation_policy.workflow_commit,
        )
            .cmp(&(
                &other.package,
                version_ord_key(&other.version),
                &other.platform,
                &other.asset_name,
                &other.attestation_policy.repository,
                &other.attestation_policy.workflow_path,
                &other.attestation_policy.workflow_commit,
            ))
    }
}

fn is_safe_workflow_path(value: &str) -> bool {
    value.starts_with(".github/workflows/")
        && value.ends_with(".yml")
        && !value.contains("..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
}

pub fn is_safe_artifact_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.contains("..")
        // `.` names the directory being joined, not a file in it.
        && value != "."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Credential-free GitHub attestation policy and verified provenance facts.
///
/// `source_commit` is the source digest GitHub records for the workflow run:
/// the coordinator revision that executed the attestation action. The release
/// source is separately bound by `ArtifactManifestV1::source_commit` and the
/// immutable intent, because recovery intentionally builds an older release
/// checkout with current coordinator workflow code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitHubArtifactAttestationV1 {
    pub repository: GitHubRepository,
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
                || entry.attestation.source_commit != entry.slot.attestation_policy.workflow_commit
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

    /// Hashes the canonical serialized manifest transported from build to
    /// execution. Its entry roster was canonicalized by [`Self::new`].
    pub fn digest(&self) -> ArtifactDigest {
        ArtifactDigest::from_bytes(serde_json::to_vec(self).expect("artifact manifest serializes"))
    }

    pub fn validate_for_intent(&self, intent: &ReleaseIntentV1) -> Result<(), ArtifactManifestError> {
        if self.schema_version != Self::SCHEMA_VERSION || self.intent_digest != intent.digest {
            return Err(ArtifactManifestError::MismatchedIntent);
        }
        match &intent.snapshot.source {
            SourceIdentity::GitCommit { sha } if sha == &self.source_commit => {}
            SourceIdentity::GitCommit { .. } => return Err(ArtifactManifestError::MismatchedSourceCommit),
            SourceIdentity::HermeticContent { .. } => return Err(ArtifactManifestError::NonGitSource),
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
    #[error("artifact manifest source commit differs from the intent release source")]
    MismatchedSourceCommit,
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
        check_schema_version::<D::Error>(wire.schema_version, Self::SCHEMA_VERSION, "release intent")?;
        check_decision_schema_version::<D::Error>(wire.decision.schema_version, "release intent")?;
        check_schema_version::<D::Error>(
            wire.snapshot.schema_version,
            ReleaseInputSnapshotV1::SCHEMA_VERSION,
            "release intent",
        )?;
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
    /// 3 adds the `forgePublish` role: publication is its own operation after
    /// every artifact upload, so a version-2 intent's DAG is not executable here.
    /// 4 adds the `platformPublish` role, which an earlier reader cannot execute.
    /// 5 removes the release `profile`.
    pub const SCHEMA_VERSION: u8 = 5;

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
        if let Some(slot) = artifact_slots.iter().find(|slot| {
            !decision
                .entries
                .iter()
                .any(|entry| entry.package == slot.package && entry.target_version == slot.version)
        }) {
            return Err(ReleaseIntentError::ArtifactSlotOutsideDecision {
                package: slot.package.to_string(),
                version: slot.version.to_string(),
                asset: slot.asset_name.clone(),
            });
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
        transcript.push_str(
            "artifact-slot.repository",
            &slot.attestation_policy.repository.as_slug(),
        );
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
        ReleaseOperationRole::ForgePublish => "forge-publish".to_string(),
        ReleaseOperationRole::PlatformPublish { registry, platform } => format!(
            "platform-publish:{}:{}:{}:{}",
            registry.registry_key().as_str(),
            registry.binding_digest().as_str(),
            platform.name(),
            platform.directory(),
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
    #[error(
        "asset `{asset}` is built by `{package}@{version}`, which is not in this release; \
         release it too, or put it in the product's [[fixed-group]]"
    )]
    ArtifactSlotOutsideDecision {
        package: String,
        version: String,
        asset: String,
    },
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

/// The immutable identity of one release run, created and validated before
/// the first effect and recorded in the receipt.
///
/// Every field is derived from the intent and the verified artifact manifest
/// rather than asserted by a caller, so no fact here has a second authority.
/// The revisions are intentionally separate even when equal: a run may release
/// an older merged source with the current coordinator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseRunEnvelopeV1 {
    schema_version: u8,
    orchestration_revision: CommitSha,
    release_source_revision: CommitSha,
    intent_digest: IntentDigest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artifact_manifest_digest: Option<ArtifactDigest>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseRunEnvelopeV1Wire {
    schema_version: u8,
    orchestration_revision: CommitSha,
    release_source_revision: CommitSha,
    intent_digest: IntentDigest,
    #[serde(default)]
    artifact_manifest_digest: Option<ArtifactDigest>,
}

impl<'de> Deserialize<'de> for ReleaseRunEnvelopeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ReleaseRunEnvelopeV1Wire::deserialize(deserializer)?;
        if wire.schema_version != ReleaseRunEnvelopeV1::SCHEMA_VERSION {
            return Err(serde::de::Error::custom(ReleaseRunEnvelopeError::UnsupportedSchema {
                found: wire.schema_version,
            }));
        }
        Ok(Self {
            schema_version: wire.schema_version,
            orchestration_revision: wire.orchestration_revision,
            release_source_revision: wire.release_source_revision,
            intent_digest: wire.intent_digest,
            artifact_manifest_digest: wire.artifact_manifest_digest,
        })
    }
}

impl ReleaseRunEnvelopeV1 {
    /// 3 removes the release `profile`.
    pub const SCHEMA_VERSION: u8 = 3;

    /// The only constructor. Source revision, and intent digest are
    /// read out of `intent`; only the coordinator revision and the verified
    /// manifest digest come from the caller, and both are cross-checked
    /// against the intent here, before any effect.
    pub fn new(
        orchestration_revision: CommitSha,
        intent: &ReleaseIntentV1,
        artifact_manifest_digest: Option<ArtifactDigest>,
    ) -> Result<Self, ReleaseRunEnvelopeError> {
        let release_source_revision = match &intent.snapshot.source {
            SourceIdentity::GitCommit { sha } => sha.clone(),
            SourceIdentity::HermeticContent { .. } => return Err(ReleaseRunEnvelopeError::NonGitReleaseSource),
        };
        let envelope = Self {
            schema_version: Self::SCHEMA_VERSION,
            orchestration_revision,
            release_source_revision,
            intent_digest: intent.digest.clone(),
            artifact_manifest_digest,
        };
        envelope.validate_for_intent(intent)?;
        Ok(envelope)
    }

    pub fn orchestration_revision(&self) -> &CommitSha {
        &self.orchestration_revision
    }

    pub fn release_source_revision(&self) -> &CommitSha {
        &self.release_source_revision
    }

    pub fn intent_digest(&self) -> &IntentDigest {
        &self.intent_digest
    }

    pub fn artifact_manifest_digest(&self) -> Option<&ArtifactDigest> {
        self.artifact_manifest_digest.as_ref()
    }

    /// Cross-field validity against the exact intent this run releases.
    pub fn validate_for_intent(&self, intent: &ReleaseIntentV1) -> Result<(), ReleaseRunEnvelopeError> {
        if self.schema_version != Self::SCHEMA_VERSION {
            return Err(ReleaseRunEnvelopeError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        if self.intent_digest != intent.digest {
            return Err(ReleaseRunEnvelopeError::MismatchedIntent);
        }
        match (intent.artifact_slots.is_empty(), &self.artifact_manifest_digest) {
            (false, None) => return Err(ReleaseRunEnvelopeError::MissingArtifactManifest),
            (true, Some(_)) => return Err(ReleaseRunEnvelopeError::UnexpectedArtifactManifest),
            _ => {}
        }
        // The coordinator that built and attested the binaries is the same
        // revision the slots' attestation policy pins; a receipt must not be
        // able to name a coordinator that did not produce these artifacts.
        for slot in &intent.artifact_slots {
            if slot.attestation_policy.workflow_commit != self.orchestration_revision {
                return Err(ReleaseRunEnvelopeError::MismatchedOrchestrationRevision);
            }
        }
        match &intent.snapshot.source {
            SourceIdentity::GitCommit { sha } if sha == &self.release_source_revision => Ok(()),
            SourceIdentity::GitCommit { .. } => Err(ReleaseRunEnvelopeError::MismatchedReleaseSource),
            SourceIdentity::HermeticContent { .. } => Err(ReleaseRunEnvelopeError::NonGitReleaseSource),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseRunEnvelopeError {
    #[error("unsupported release run envelope schema version {found}")]
    UnsupportedSchema { found: u8 },
    #[error("release run envelope is bound to a different intent")]
    MismatchedIntent,
    #[error("release run envelope source does not match the release intent")]
    MismatchedReleaseSource,
    #[error("release run envelope requires a Git commit release source")]
    NonGitReleaseSource,
    #[error("artifact release envelope requires an artifact manifest digest")]
    MissingArtifactManifest,
    #[error("release run envelope carries an artifact manifest digest for a slot-less intent")]
    UnexpectedArtifactManifest,
    #[error("release run envelope orchestration revision is not the revision that attested the artifact slots")]
    MismatchedOrchestrationRevision,
}

/// In-memory state for one intent-bound execution. Pending and Attempting are
/// nonterminal. Each successful operation keeps the exact provider evidence
/// that moved it there, so a receipt is derived from this state alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseExecutionStateV1 {
    intent_digest: IntentDigest,
    envelope: ReleaseRunEnvelopeV1,
    operations: BTreeMap<ReleaseOperationId, OperationState>,
    evidence: BTreeMap<ReleaseOperationId, ProviderEvidenceV1>,
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

impl OperationState {
    /// `Published` or `AlreadySatisfied`: the states that carry exact evidence.
    pub fn is_success(self) -> bool {
        matches!(self, Self::Published | Self::AlreadySatisfied)
    }
}

impl ReleaseExecutionStateV1 {
    /// Starts execution for exactly the roster authorized by `intent`, under
    /// the run envelope that was validated before any effect. There is no way
    /// to create a state without an envelope.
    pub fn new(intent: &ReleaseIntentV1, envelope: ReleaseRunEnvelopeV1) -> Result<Self, ReleaseStateError> {
        envelope
            .validate_for_intent(intent)
            .map_err(ReleaseStateError::InvalidEnvelope)?;
        Ok(Self {
            intent_digest: intent.digest.clone(),
            envelope,
            operations: intent
                .operations
                .iter()
                .map(|operation| (operation.id.clone(), OperationState::Pending))
                .collect(),
            evidence: BTreeMap::new(),
        })
    }

    pub fn intent_digest(&self) -> &IntentDigest {
        &self.intent_digest
    }

    /// The run this state belongs to: the single authority for coordinator
    /// revision, release source, and manifest digest.
    pub fn envelope(&self) -> &ReleaseRunEnvelopeV1 {
        &self.envelope
    }

    /// Validates that this state belongs to this exact intent and operation roster.
    pub fn validate_for_intent(&self, intent: &ReleaseIntentV1) -> Result<(), ReleaseStateError> {
        if self.intent_digest != intent.digest {
            return Err(ReleaseStateError::MismatchedIntent);
        }
        self.envelope
            .validate_for_intent(intent)
            .map_err(ReleaseStateError::InvalidEnvelope)?;
        let expected: BTreeSet<_> = intent.operations.iter().map(|operation| operation.id.clone()).collect();
        let actual: BTreeSet<_> = self.operations.keys().cloned().collect();
        if actual != expected {
            return Err(ReleaseStateError::MismatchedOperationRoster);
        }
        Ok(())
    }

    /// Returns the state for an exact operation identity.
    pub fn operation_state(&self, id: &ReleaseOperationId) -> Option<OperationState> {
        self.operations.get(id).copied()
    }

    /// The exact evidence recorded when `id` reached a successful state.
    pub fn operation_evidence(&self, id: &ReleaseOperationId) -> Option<&ProviderEvidenceV1> {
        self.evidence.get(id)
    }

    /// The only way an operation's state ever changes.
    pub fn apply(
        &mut self,
        id: &ReleaseOperationId,
        event: &OperationEvent,
    ) -> Result<OperationState, ReleaseStateError> {
        let state = self
            .operations
            .get_mut(id)
            .ok_or_else(|| ReleaseStateError::UnknownOperation {
                id: Box::new(id.clone()),
            })?;
        let next = crate::release_transition::transition(*state, event).map_err(|source| {
            ReleaseStateError::InvalidTransition {
                id: Box::new(id.clone()),
                source,
            }
        })?;
        let proof = if next.is_success() {
            let proof = event
                .exact_evidence()
                .ok_or_else(|| ReleaseStateError::MissingEvidence {
                    id: Box::new(id.clone()),
                })?;
            check_evidence_role(id, proof.evidence())?;
            Some(proof.evidence().clone())
        } else {
            None
        };
        *state = next;
        if let Some(proof) = proof {
            self.evidence.insert(id.clone(), proof);
        }
        Ok(next)
    }
}

/// Evidence recorded for an operation must belong to that operation's role.
fn check_evidence_role(id: &ReleaseOperationId, evidence: &ProviderEvidenceV1) -> Result<(), ReleaseStateError> {
    ReleaseOperationObservationV1::new(
        id.clone(),
        ProviderObservationV1::Exact {
            evidence: evidence.clone(),
        },
    )
    .map(|_| ())
    .map_err(ReleaseStateError::EvidenceRejected)
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseStateError {
    #[error("release state is bound to a different intent")]
    MismatchedIntent,
    #[error("release state operation roster differs from the bound intent")]
    MismatchedOperationRoster,
    #[error("release state carries an invalid run envelope: {0}")]
    InvalidEnvelope(ReleaseRunEnvelopeError),
    #[error("release state does not contain operation `{id:?}`")]
    UnknownOperation { id: Box<ReleaseOperationId> },
    #[error("operation `{id:?}`: {source}")]
    InvalidTransition {
        id: Box<ReleaseOperationId>,
        source: crate::release_transition::InvalidTransition,
    },
    #[error("successful release operation `{id:?}` carries no provider evidence")]
    MissingEvidence { id: Box<ReleaseOperationId> },
    #[error("release state evidence rejected: {0}")]
    EvidenceRejected(ProviderObservationError),
}

/// A terminal receipt derived only from a complete execution state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseReceiptV1 {
    schema_version: u8,
    intent_digest: IntentDigest,
    envelope: ReleaseRunEnvelopeV1,
    outcomes: BTreeMap<ReleaseOperationId, OperationOutcome>,
    observations: BTreeMap<ReleaseOperationId, ProviderObservationV1>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseReceiptV1Wire {
    schema_version: u8,
    intent_digest: IntentDigest,
    envelope: ReleaseRunEnvelopeV1,
    outcomes: Vec<OperationOutcomeEntryV1>,
    observations: Vec<ReleaseOperationObservationV1>,
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
            envelope: self.envelope.clone(),
            outcomes: self
                .outcomes
                .iter()
                .map(|(operation, outcome)| OperationOutcomeEntryV1 {
                    operation: operation.clone(),
                    outcome: *outcome,
                })
                .collect(),
            observations: self
                .observations
                .iter()
                .map(|(operation, observation)| {
                    ReleaseOperationObservationV1::new(operation.clone(), observation.clone())
                        .expect("receipt observations were role-checked on construction")
                })
                .collect(),
        }
        .serialize(serializer)
    }
}

impl JsonSchema for ReleaseReceiptV1 {
    fn schema_name() -> String {
        "ReleaseReceiptV1".to_owned()
    }

    fn json_schema(generator: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        ReleaseReceiptV1Wire::json_schema(generator)
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
    pub const SCHEMA_VERSION: u8 = 2;

    /// Constructs a receipt from a terminal state alone: every operation must
    /// be successful, and its recorded evidence becomes its observation.
    ///
    /// The run envelope comes from the state, which was created and validated
    /// before the first effect; nothing is assembled after the fact here.
    pub fn from_state(intent: &ReleaseIntentV1, state: &ReleaseExecutionStateV1) -> Result<Self, ReleaseReceiptError> {
        state
            .validate_for_intent(intent)
            .map_err(ReleaseReceiptError::InvalidState)?;
        let mut outcomes = BTreeMap::new();
        let mut observations = BTreeMap::new();
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
            let evidence = state.operation_evidence(id).cloned().ok_or_else(|| {
                ReleaseReceiptError::InvalidState(ReleaseStateError::MissingEvidence {
                    id: Box::new(id.clone()),
                })
            })?;
            outcomes.insert(id.clone(), outcome);
            observations.insert(id.clone(), ProviderObservationV1::Exact { evidence });
        }
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            intent_digest: state.intent_digest.clone(),
            envelope: state.envelope().clone(),
            outcomes,
            observations,
        })
    }

    /// The exact provider observation this receipt records for `id`.
    pub fn observation(&self, id: &ReleaseOperationId) -> Option<&ProviderObservationV1> {
        self.observations.get(id)
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
        self.envelope
            .validate_for_intent(intent)
            .map_err(ReleaseReceiptError::InvalidEnvelope)?;
        let expected: BTreeSet<_> = intent.operations.iter().map(|operation| operation.id.clone()).collect();
        let actual: BTreeSet<_> = self.outcomes.keys().cloned().collect();
        if actual != expected {
            return Err(ReleaseReceiptError::MismatchedOperationRoster);
        }
        let observed: BTreeSet<_> = self.observations.keys().cloned().collect();
        if observed != expected {
            return Err(ReleaseReceiptError::MismatchedObservationRoster);
        }
        if self
            .observations
            .values()
            .any(|observation| !observation.is_terminal_success())
        {
            return Err(ReleaseReceiptError::NonExactReceiptObservation);
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
        let mut observations = BTreeMap::new();
        for entry in wire.observations {
            let (operation, observation) = entry.into_parts();
            if !observation.is_terminal_success() {
                return Err(ReleaseReceiptError::NonExactObservation {
                    id: Box::new(operation),
                    observation: Box::new(observation),
                });
            }
            if observations.insert(operation.clone(), observation).is_some() {
                return Err(ReleaseReceiptError::DuplicateObservation {
                    id: Box::new(operation),
                });
            }
        }
        Ok(Self {
            schema_version: wire.schema_version,
            intent_digest: wire.intent_digest,
            envelope: wire.envelope,
            outcomes,
            observations,
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
    #[error("release receipt envelope is not bound to the intent: {0}")]
    InvalidEnvelope(ReleaseRunEnvelopeError),
    #[error("release operation `{id:?}` is not terminal")]
    NonTerminalOperation { id: Box<ReleaseOperationId> },
    #[error("release operation `{id:?}` did not complete successfully")]
    NonSuccessfulOperation { id: Box<ReleaseOperationId> },
    #[error("release receipt observation roster differs from the bound intent")]
    MismatchedObservationRoster,
    #[error("release receipt contains duplicate observation for `{id:?}`")]
    DuplicateObservation { id: Box<ReleaseOperationId> },
    #[error("release receipt observation for `{id:?}` is not exact: {observation:?}")]
    NonExactObservation {
        id: Box<ReleaseOperationId>,
        observation: Box<ProviderObservationV1>,
    },
    #[error("release receipt contains a non-exact provider observation")]
    NonExactReceiptObservation,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release_observation::ProviderEvidenceV1;
    use crate::Ecosystem;

    fn test_envelope(intent: &ReleaseIntentV1) -> ReleaseRunEnvelopeV1 {
        let orchestration = intent
            .artifact_slots
            .first()
            .map(|slot| slot.attestation_policy.workflow_commit.clone())
            .unwrap_or_else(|| CommitSha::parse(&"b".repeat(40)).unwrap());
        let manifest = (!intent.artifact_slots.is_empty()).then(|| ArtifactDigest::from_bytes(b"manifest"));
        ReleaseRunEnvelopeV1::new(orchestration, intent, manifest).unwrap()
    }

    fn pending_state(intent: &ReleaseIntentV1) -> ReleaseExecutionStateV1 {
        ReleaseExecutionStateV1::new(intent, test_envelope(intent)).unwrap()
    }

    /// Role-matching evidence for one operation, as a real provider adapter
    /// would report it.
    fn evidence_for(id: &ReleaseOperationId) -> ProviderEvidenceV1 {
        match &id.role {
            ReleaseOperationRole::RegistryPublish { .. } | ReleaseOperationRole::PlatformPublish { .. } => {
                ProviderEvidenceV1::RegistryVersion {
                    version: id.version.clone(),
                    checksum: None,
                    yanked: None,
                }
            }
            ReleaseOperationRole::Tag => ProviderEvidenceV1::GitTag {
                peeled_commit: CommitSha::parse(&"a".repeat(40)).unwrap(),
            },
            ReleaseOperationRole::ForgeRelease | ReleaseOperationRole::ForgePublish => {
                ProviderEvidenceV1::ForgeRelease {
                    tag_name: crate::TagName::new_unchecked(format!("v{}", id.version.render())),
                    draft: false,
                }
            }
            ReleaseOperationRole::ArtifactUpload { .. } => ProviderEvidenceV1::ArtifactUpload {
                byte_length: 6,
                sha256: ArtifactDigest::from_bytes(b"binary"),
            },
        }
    }

    fn exact_observation(id: &ReleaseOperationId) -> ProviderObservationV1 {
        ProviderObservationV1::Exact {
            evidence: evidence_for(id),
        }
    }

    fn attempt_event() -> OperationEvent {
        OperationEvent::Attempt {
            proof: ProviderObservationV1::Absent
                .absent_proof()
                .expect("absent mints a proof"),
        }
    }

    fn confirmed_event(id: &ReleaseOperationId) -> OperationEvent {
        OperationEvent::Confirmed {
            evidence: exact_observation(id).exact_evidence().expect("exact mints evidence"),
        }
    }

    /// Drives one operation through the only legal path to `Published`.
    fn publish_operation(state: &mut ReleaseExecutionStateV1, id: &ReleaseOperationId) {
        state.apply(id, &attempt_event()).unwrap();
        state.apply(id, &confirmed_event(id)).unwrap();
    }

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
    fn github_repository_parses_valid_owner_repo_and_renders_slug() {
        let repo = GitHubRepository::parse("orin-dx/callisto").unwrap();
        assert_eq!(repo.owner(), "orin-dx");
        assert_eq!(repo.repo(), "callisto");
        assert_eq!(repo.as_slug(), "orin-dx/callisto");
        assert_eq!(repo.to_string(), "orin-dx/callisto");
    }

    #[test]
    fn github_repository_rejects_embedded_whitespace_extra_parts_and_leading_trailing_hyphen() {
        // The prior `is_safe_github_repository` accepted this (a bare
        // `split_once('/')` with no character-class check at all) -- this is
        // the exact behavior-change gap the audit found.
        assert!(GitHubRepository::parse("owner name/repo").is_err());
        assert!(GitHubRepository::parse("owner/repo name").is_err());
        assert!(GitHubRepository::parse("owner").is_err());
        assert!(GitHubRepository::parse("owner/repo/extra").is_err());
        assert!(GitHubRepository::parse("-owner/repo").is_err());
        assert!(GitHubRepository::parse("owner-/repo").is_err());
        assert!(GitHubRepository::parse("owner/-repo").is_err());
        assert!(GitHubRepository::parse("owner/repo-").is_err());
        assert!(GitHubRepository::parse("/repo").is_err());
        assert!(GitHubRepository::parse("owner/").is_err());
    }

    #[test]
    fn github_repository_serializes_and_deserializes_as_its_slug_string() {
        let repo = GitHubRepository::parse("orin-dx/callisto").unwrap();
        let json = serde_json::to_string(&repo).unwrap();
        assert_eq!(json, "\"orin-dx/callisto\"");
        let round_tripped: GitHubRepository = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, repo);
        assert!(serde_json::from_str::<GitHubRepository>("\"owner name/repo\"").is_err());
    }

    #[test]
    fn decision_rejects_divergent_target_versions_within_a_fixed_group() {
        let a = ReleasePackageId::parse("cargo/crate-a").unwrap();
        let b = ReleasePackageId::parse("cargo/crate-b").unwrap();
        let entries = vec![
            ReleaseDecisionEntry {
                package: a,
                target_version: Version::semver(1, 1, 0),
                reasons: vec![ReleaseInclusionReason::FixedGroup {
                    group_id: "demo".to_string(),
                }],
            },
            ReleaseDecisionEntry {
                package: b,
                target_version: Version::semver(1, 2, 0),
                reasons: vec![ReleaseInclusionReason::FixedGroup {
                    group_id: "demo".to_string(),
                }],
            },
        ];
        assert!(matches!(
            ReleaseDecisionV1::new(entries),
            Err(ReleaseDecisionError::DivergentGroupTarget { group_id, .. }) if group_id == "demo"
        ));
    }

    #[test]
    fn decision_rejects_divergent_target_versions_within_a_linked_group() {
        let a = ReleasePackageId::parse("cargo/crate-a").unwrap();
        let b = ReleasePackageId::parse("npm/crate-b").unwrap();
        let entries = vec![
            ReleaseDecisionEntry {
                package: a,
                target_version: Version::semver(2, 0, 0),
                reasons: vec![ReleaseInclusionReason::LinkedGroup {
                    group_id: "linked-demo".to_string(),
                }],
            },
            ReleaseDecisionEntry {
                package: b,
                target_version: Version::semver(2, 0, 1),
                reasons: vec![ReleaseInclusionReason::LinkedGroup {
                    group_id: "linked-demo".to_string(),
                }],
            },
        ];
        assert!(matches!(
            ReleaseDecisionV1::new(entries),
            Err(ReleaseDecisionError::DivergentGroupTarget { group_id, .. }) if group_id == "linked-demo"
        ));
    }

    #[test]
    fn decision_accepts_agreeing_target_versions_within_a_fixed_group() {
        let a = ReleasePackageId::parse("cargo/crate-a").unwrap();
        let b = ReleasePackageId::parse("cargo/crate-b").unwrap();
        let entries = vec![
            ReleaseDecisionEntry {
                package: a,
                target_version: Version::semver(1, 1, 0),
                reasons: vec![ReleaseInclusionReason::FixedGroup {
                    group_id: "demo".to_string(),
                }],
            },
            ReleaseDecisionEntry {
                package: b,
                target_version: Version::semver(1, 1, 0),
                reasons: vec![ReleaseInclusionReason::FixedGroup {
                    group_id: "demo".to_string(),
                }],
            },
        ];
        assert!(ReleaseDecisionV1::new(entries).is_ok());
    }

    /// The digest-validating `Deserialize` impl calls `new()` internally, so
    /// a hand-crafted (but internally digest-consistent) decision claiming
    /// divergent group targets must be rejected the same way a freshly
    /// derived one is -- this is the path a merge-commit verifier reading a
    /// committed decision file actually exercises, not just direct
    /// `ReleaseDecisionV1::new()` calls from a version plan.
    #[test]
    fn decision_deserialization_rejects_divergent_group_targets_even_with_a_self_consistent_digest() {
        let a = ReleasePackageId::parse("cargo/crate-a").unwrap();
        let b = ReleasePackageId::parse("cargo/crate-b").unwrap();
        let entries = vec![
            ReleaseDecisionEntry {
                package: a,
                target_version: Version::semver(1, 1, 0),
                reasons: vec![ReleaseInclusionReason::FixedGroup {
                    group_id: "demo".to_string(),
                }],
            },
            ReleaseDecisionEntry {
                package: b,
                target_version: Version::semver(1, 2, 0),
                reasons: vec![ReleaseInclusionReason::FixedGroup {
                    group_id: "demo".to_string(),
                }],
            },
        ];
        // Hand-compute a digest matching this exact (invalid) entry set, so
        // the failure below is provably the group-divergence check and not
        // just the pre-existing digest-mismatch guard.
        let digest = decision_digest(&entries);
        let wire = serde_json::json!({
            "schemaVersion": ReleaseDecisionV1::SCHEMA_VERSION,
            "entries": entries,
            "digest": digest,
        });
        let result: Result<ReleaseDecisionV1, _> = serde_json::from_value(wire);
        assert!(
            result.is_err(),
            "expected divergent group targets to be rejected on deserialize"
        );
    }

    #[test]
    fn unreleased_version_reason_is_decision_schema_two() {
        assert_eq!(ReleaseDecisionV1::SCHEMA_VERSION, 2);
        assert_eq!(
            serde_json::to_value(ReleaseInclusionReason::UnreleasedVersion).unwrap(),
            serde_json::json!({ "kind": "unreleasedVersion" })
        );
        let decision = ReleaseDecisionV1::new(vec![ReleaseDecisionEntry {
            package: ReleasePackageId::new(Ecosystem::Cargo, "demo").unwrap(),
            target_version: Version::semver(1, 0, 0),
            reasons: vec![ReleaseInclusionReason::UnreleasedVersion],
        }])
        .unwrap();
        let mut wire = serde_json::to_value(&decision).unwrap();
        assert_eq!(
            serde_json::from_value::<ReleaseDecisionV1>(wire.clone()).unwrap(),
            decision
        );
        wire["schemaVersion"] = serde_json::json!(3);
        let error = serde_json::from_value::<ReleaseDecisionV1>(wire)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unsupported release decision schema version 3"),
            "{error}"
        );
    }

    #[test]
    fn committed_v1_decision_file_still_reads_and_writes_as_v2() {
        let raw = include_str!("../tests/fixtures/release-decision-v1-0.8.0.json");
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(value["schemaVersion"], 1);
        let decision: ReleaseDecisionV1 = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(decision.entries.len(), value["entries"].as_array().unwrap().len());
        assert_eq!(decision.digest.to_string(), value["digest"].as_str().unwrap());
        assert_eq!(serde_json::to_value(&decision).unwrap()["schemaVersion"], 2);
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
            GitHubRepository::parse("orin-dx/callisto").unwrap(),
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
                    repository: GitHubRepository::parse("orin-dx/callisto").unwrap(),
                    workflow_path: ".github/workflows/release.yml".to_string(),
                    workflow_commit: CommitSha::parse(&"b".repeat(40)).unwrap(),
                    subject_digest: digest,
                    source_commit: CommitSha::parse(&"b".repeat(40)).unwrap(),
                },
            }],
        )
        .unwrap();
        manifest.validate_for_intent(&intent).unwrap();
    }

    /// Top-level `source_commit` pins to the release source (the
    /// intent's immutable snapshot), never the orchestration revision that
    /// attested the binary -- a recovery rerun deliberately builds an older
    /// release commit with current coordinator workflow code, so these two
    /// facts differ and must not be conflated.
    #[test]
    fn artifact_manifest_source_commit_is_release_source_not_orchestration_revision() {
        let package = ReleasePackageId::parse("cargo/demo").unwrap();
        let version = Version::semver(1, 0, 0);
        let release_source = CommitSha::parse(&"a".repeat(40)).unwrap();
        let orchestration_revision = CommitSha::parse(&"c".repeat(40)).unwrap();
        assert_ne!(
            release_source, orchestration_revision,
            "fixture must exercise diverging commits"
        );
        let slot = ArtifactSlotId::new(
            package,
            version,
            "x86_64-unknown-linux-gnu",
            "demo.tar.gz",
            GitHubRepository::parse("orin-dx/callisto").unwrap(),
            ".github/workflows/release.yml",
            orchestration_revision.clone(),
        )
        .unwrap();
        let operation = ReleaseOperation::artifact_upload(slot.clone(), vec![]).unwrap();
        let intent = test_intent_with_slots(
            ReleaseInputSnapshotV1::new(
                SourceIdentity::GitCommit {
                    sha: release_source.clone(),
                },
                vec![],
            ),
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
                    repository: GitHubRepository::parse("orin-dx/callisto").unwrap(),
                    workflow_path: ".github/workflows/release.yml".to_string(),
                    workflow_commit: orchestration_revision.clone(),
                    subject_digest: digest,
                    source_commit: orchestration_revision.clone(),
                },
            }],
        )
        .unwrap();
        assert_eq!(manifest.source_commit, release_source);
        assert_ne!(manifest.source_commit, orchestration_revision);
    }

    /// An entry whose `attestation.source_commit` is set to the
    /// release source instead of the orchestration revision (when the two
    /// differ) is rejected -- otherwise a recovery rerun's manifest would
    /// silently bind GitHub's attested source digest to the wrong commit.
    #[test]
    fn artifact_manifest_rejects_attestation_source_commit_swapped_for_release_source() {
        let package = ReleasePackageId::parse("cargo/demo").unwrap();
        let version = Version::semver(1, 0, 0);
        let release_source = CommitSha::parse(&"a".repeat(40)).unwrap();
        let orchestration_revision = CommitSha::parse(&"c".repeat(40)).unwrap();
        assert_ne!(
            release_source, orchestration_revision,
            "fixture must exercise diverging commits"
        );
        let slot = ArtifactSlotId::new(
            package,
            version,
            "x86_64-unknown-linux-gnu",
            "demo.tar.gz",
            GitHubRepository::parse("orin-dx/callisto").unwrap(),
            ".github/workflows/release.yml",
            orchestration_revision.clone(),
        )
        .unwrap();
        let operation = ReleaseOperation::artifact_upload(slot.clone(), vec![]).unwrap();
        let intent = test_intent_with_slots(
            ReleaseInputSnapshotV1::new(
                SourceIdentity::GitCommit {
                    sha: release_source.clone(),
                },
                vec![],
            ),
            ExecutionTrustProfileV1::GitCommit,
            vec![operation],
            vec![slot.clone()],
        )
        .unwrap();
        let digest = ArtifactDigest::from_bytes(b"binary");
        let result = ArtifactManifestV1::new(
            &intent,
            vec![ArtifactManifestEntryV1 {
                slot,
                digest: digest.clone(),
                byte_length: 6,
                attestation: GitHubArtifactAttestationV1 {
                    repository: GitHubRepository::parse("orin-dx/callisto").unwrap(),
                    workflow_path: ".github/workflows/release.yml".to_string(),
                    workflow_commit: orchestration_revision,
                    subject_digest: digest,
                    // Swapped: release source instead of the orchestration revision.
                    source_commit: release_source,
                },
            }],
        );
        assert!(matches!(result, Err(ArtifactManifestError::MismatchedAttestation)));
    }

    /// Two `ArtifactUpload` operations that share package/version/platform/asset_name
    /// but differ only in `attestation_policy` are distinct `ArtifactSlotId`s and must
    /// remain distinct `ReleaseOperationId`s under `Ord`, not just `Eq` -- otherwise
    /// `BTreeSet`/`BTreeMap` keys silently collapse them (the `a.cmp(b) == Equal implies
    /// a == b` invariant those collections require).
    #[test]
    fn artifact_upload_ids_differing_only_by_attestation_policy_stay_distinct_under_ord() {
        let package = ReleasePackageId::parse("cargo/demo").unwrap();
        let version = Version::semver(1, 0, 0);
        let slot_a = ArtifactSlotId::new(
            package.clone(),
            version.clone(),
            "x86_64-unknown-linux-gnu",
            "demo.tar.gz",
            GitHubRepository::parse("orin-dx/callisto").unwrap(),
            ".github/workflows/release.yml",
            CommitSha::parse(&"a".repeat(40)).unwrap(),
        )
        .unwrap();
        let slot_b = ArtifactSlotId::new(
            package,
            version,
            "x86_64-unknown-linux-gnu",
            "demo.tar.gz",
            GitHubRepository::parse("orin-dx/callisto").unwrap(),
            ".github/workflows/release.yml",
            CommitSha::parse(&"b".repeat(40)).unwrap(),
        )
        .unwrap();
        assert_ne!(slot_a, slot_b, "the two slots must differ only by attestation_policy");

        let id_a = ReleaseOperationId::artifact_upload(slot_a.clone());
        let id_b = ReleaseOperationId::artifact_upload(slot_b.clone());

        assert_ne!(id_a, id_b, "distinct attestation policies must stay Eq-distinct");
        assert_ne!(
            id_a.cmp(&id_b),
            Ordering::Equal,
            "distinct attestation policies must not compare Ord::Equal"
        );

        let mut ids = BTreeSet::new();
        assert!(ids.insert(id_a.clone()), "first id must insert");
        assert!(
            ids.insert(id_b.clone()),
            "second id must NOT be displaced by the first in a BTreeSet"
        );
        assert_eq!(ids.len(), 2);

        // Exercise the real ReleaseIntentV1::new / validate_operations path: this must
        // NOT be rejected as a duplicate operation.
        let op_a = ReleaseOperation::artifact_upload(slot_a.clone(), vec![]).unwrap();
        let op_b = ReleaseOperation::artifact_upload(slot_b.clone(), vec![]).unwrap();
        let (first, second) = if op_a.id() <= op_b.id() {
            (op_a, op_b)
        } else {
            (op_b, op_a)
        };
        let (first_slot, second_slot) = if first.id() == &id_a {
            (slot_a, slot_b)
        } else {
            (slot_b, slot_a)
        };
        let intent = test_intent_with_slots(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("d".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![first, second],
            vec![first_slot, second_slot],
        );
        assert!(
            intent.is_ok(),
            "two artifact uploads differing only by attestation_policy must not collide: {intent:?}"
        );
    }

    /// Residual instance of the same defect class fixed above for
    /// `attestation_policy`: `ReleaseOperationId::cmp` used to key on
    /// `version.render()` alone, dropping `version.grammar()`. The same
    /// literal string parses under more than one grammar -- `"1.2.3"` is
    /// valid both as SemVer and PEP 440 -- so two `ReleaseOperationId`s
    /// that are Eq-distinct only by `Version::grammar` collapsed to
    /// `Ordering::Equal` under the old `render()`-only `Ord`, violating
    /// the invariant `BTreeSet`/`BTreeMap` require: `a.cmp(b) == Equal`
    /// iff `a == b`.
    #[test]
    fn operation_ids_differing_only_by_version_grammar_stay_distinct_under_ord() {
        let package = ReleasePackageId::parse("cargo/demo").unwrap();
        let v_semver = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
        let v_pep440 = Version::parse("1.2.3", VersionGrammar::Pep440).unwrap();

        // Precondition: the identical literal renders identically under both
        // grammars, but the parsed Versions are NOT the same value -- this is
        // exactly the collision `version_ord_key` must resolve.
        assert_eq!(v_semver.render(), v_pep440.render());
        assert_ne!(
            v_semver, v_pep440,
            "SemVer and PEP 440 parses of the identical literal must remain Eq-distinct"
        );

        let id_a = ReleaseOperationId::tag(package.clone(), v_semver);
        let id_b = ReleaseOperationId::tag(package, v_pep440);

        assert_ne!(id_a, id_b, "distinct version grammars must stay Eq-distinct");
        assert_ne!(
            id_a.cmp(&id_b),
            Ordering::Equal,
            "distinct version grammars must not compare Ord::Equal"
        );

        let mut ids = BTreeSet::new();
        assert!(ids.insert(id_a.clone()), "first id must insert");
        assert!(
            ids.insert(id_b.clone()),
            "second id must NOT be displaced by the first in a BTreeSet"
        );
        assert_eq!(ids.len(), 2, "BTreeSet must not silently collapse distinct-grammar ids");

        // Sanity check the ordinary case still behaves: several distinct
        // plain SemVer versions of the same package/role must all survive
        // as separate BTreeSet entries too, not just the grammar-collision
        // edge case above.
        let mut ordinary_ids = BTreeSet::new();
        for raw in ["1.0.0", "1.2.3", "2.0.0", "0.9.9"] {
            let id = ReleaseOperationId::tag(
                ReleasePackageId::parse("cargo/demo").unwrap(),
                Version::parse(raw, VersionGrammar::SemVer).unwrap(),
            );
            assert!(
                ordinary_ids.insert(id),
                "version {raw} must not collide with a prior entry"
            );
        }
        assert_eq!(ordinary_ids.len(), 4);
    }

    /// Sibling instance of the same `.render()`-only defect, found in
    /// `ArtifactSlotId::cmp` (a different type in the same file) while
    /// auditing `ReleaseOperationId` above. Same construction: two slots
    /// identical in every field except `version.grammar()`.
    #[test]
    fn artifact_slot_ids_differing_only_by_version_grammar_stay_distinct_under_ord() {
        let package = ReleasePackageId::parse("cargo/demo").unwrap();
        let v_semver = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
        let v_pep440 = Version::parse("1.2.3", VersionGrammar::Pep440).unwrap();
        assert_eq!(v_semver.render(), v_pep440.render());

        let slot_a = ArtifactSlotId::new(
            package.clone(),
            v_semver,
            "x86_64-unknown-linux-gnu",
            "demo.tar.gz",
            GitHubRepository::parse("orin-dx/callisto").unwrap(),
            ".github/workflows/release.yml",
            CommitSha::parse(&"a".repeat(40)).unwrap(),
        )
        .unwrap();
        let slot_b = ArtifactSlotId::new(
            package,
            v_pep440,
            "x86_64-unknown-linux-gnu",
            "demo.tar.gz",
            GitHubRepository::parse("orin-dx/callisto").unwrap(),
            ".github/workflows/release.yml",
            CommitSha::parse(&"a".repeat(40)).unwrap(),
        )
        .unwrap();

        assert_ne!(slot_a, slot_b, "distinct version grammars must stay Eq-distinct");
        assert_ne!(
            slot_a.cmp(&slot_b),
            Ordering::Equal,
            "distinct version grammars must not compare Ord::Equal"
        );

        let mut slots = BTreeSet::new();
        assert!(slots.insert(slot_a), "first slot must insert");
        assert!(
            slots.insert(slot_b),
            "second slot must NOT be displaced by the first in a BTreeSet"
        );
        assert_eq!(slots.len(), 2);
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
    fn platform_publish_round_trips_under_its_owner_and_rejects_unsafe_platforms() {
        let owner = ReleasePackageId::parse("npm/@s/cli").unwrap();
        let platform = PlatformPackageV1::new(
            ReleasePackageId::parse("npm/@s/cli-linux-x64-gnu").unwrap(),
            "packages/cli/npm/linux-x64-gnu",
        )
        .unwrap();
        let id = ReleaseOperationId::platform_publish(
            owner.clone(),
            Version::semver(1, 2, 3),
            registry_binding("npm"),
            platform,
        );
        assert_eq!(id.package, owner);
        let wire = serde_json::to_value(&id).unwrap();
        assert_eq!(wire["role"]["kind"], "platformPublish");
        assert_eq!(wire["role"]["platform"]["directory"], "packages/cli/npm/linux-x64-gnu");
        assert_eq!(serde_json::from_value::<ReleaseOperationId>(wire.clone()).unwrap(), id);

        for directory in ["", "/abs", "-flag", "a/../b", "a//b", "./a", "a b"] {
            let mut bad = wire.clone();
            bad["role"]["platform"]["directory"] = directory.into();
            assert!(
                serde_json::from_value::<ReleaseOperationId>(bad).is_err(),
                "{directory:?}"
            );
        }
        assert!(PlatformPackageV1::new(ReleasePackageId::parse("cargo/x").unwrap(), "x").is_err());
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
    fn release_run_envelope_wire_fails_closed() {
        let intent = test_intent(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![ReleaseOperation::tag(
                ReleasePackageId::parse("cargo/callisto-model").unwrap(),
                Version::semver(1, 2, 3),
                vec![],
            )
            .unwrap()],
        )
        .unwrap();
        let envelope = test_envelope(&intent);
        assert_ne!(
            envelope.orchestration_revision(),
            envelope.release_source_revision(),
            "a run must retain distinct coordinator and release-source identities"
        );
        let mut wire = serde_json::to_value(&envelope).unwrap();
        wire["schemaVersion"] = serde_json::Value::from(9);
        assert!(serde_json::from_value::<ReleaseRunEnvelopeV1>(wire).is_err());
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
        let pending = pending_state(&intent);
        assert!(ReleaseReceiptV1::from_state(&intent, &pending).is_err());

        let mut complete = pending;
        publish_operation(&mut complete, operation.id());
        let receipt = ReleaseReceiptV1::from_state(&intent, &complete).unwrap();
        assert_eq!(receipt.intent_digest(), intent.digest());

        let mut failed = pending_state(&intent);
        failed.apply(operation.id(), &attempt_event()).unwrap();
        failed
            .apply(
                operation.id(),
                &OperationEvent::EffectFailedAndAbsent {
                    proof: ProviderObservationV1::Absent.absent_proof().unwrap(),
                },
            )
            .unwrap();
        assert!(matches!(
            ReleaseReceiptV1::from_state(&intent, &failed),
            Err(ReleaseReceiptError::NonSuccessfulOperation { .. })
        ));
        assert_eq!(
            receipt.observation(operation.id()),
            Some(&exact_observation(operation.id())),
            "the receipt must record the evidence persisted at confirmation"
        );
    }

    #[test]
    fn apply_rejects_success_evidence_of_another_role() {
        let operation = ReleaseOperation::registry_publish(
            ReleasePackageId::parse("cargo/callisto-model").unwrap(),
            Version::semver(1, 2, 3),
            registry_binding("cratesIo"),
            vec![],
        )
        .unwrap();
        let intent = test_intent(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("c".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![operation.clone()],
        )
        .unwrap();
        let mut state = pending_state(&intent);
        let tag_evidence = ProviderObservationV1::Exact {
            evidence: ProviderEvidenceV1::GitTag {
                peeled_commit: CommitSha::parse(&"a".repeat(40)).unwrap(),
            },
        };
        let event = OperationEvent::ObservedExactBeforeEffect {
            evidence: tag_evidence.exact_evidence().unwrap(),
        };
        assert!(matches!(
            state.apply(operation.id(), &event),
            Err(ReleaseStateError::EvidenceRejected(_))
        ));
        assert_eq!(state.operation_state(operation.id()), Some(OperationState::Pending));
        assert_eq!(state.operation_evidence(operation.id()), None);
    }

    #[test]
    fn receipt_wire_rejects_unknown_schema() {
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
        let mut complete = pending_state(&intent);
        publish_operation(&mut complete, operation.id());
        let receipt = ReleaseReceiptV1::from_state(&intent, &complete).unwrap();
        let mut receipt_wire = serde_json::to_value(receipt).unwrap();
        receipt_wire["schemaVersion"] = serde_json::Value::from(9);
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
        let state = pending_state(&first);
        assert!(matches!(
            state.validate_for_intent(&different_digest),
            Err(ReleaseStateError::MismatchedIntent)
        ));

        let mut complete = state;
        publish_operation(&mut complete, operation.id());
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

    /// A release package name becomes an argv word (`npm view <name>@<v>`,
    /// `pnpm publish --filter=<name>`) and a URL path segment (the cargo
    /// sparse index, the PyPI JSON API). These are the shapes that would
    /// change the meaning of one of those, so they are refused at plan time.
    #[test]
    fn release_package_names_reject_argv_and_url_hazards() {
        let hazards = [
            (Ecosystem::Cargo, "-x"),
            (Ecosystem::Cargo, "--registry"),
            (Ecosystem::Cargo, "a b"),
            (Ecosystem::Cargo, "a\nb"),
            (Ecosystem::Cargo, "a\u{0}b"),
            (Ecosystem::Cargo, "a/b"),
            (Ecosystem::Cargo, "a%2fb"),
            (Ecosystem::Cargo, "a?b"),
            (Ecosystem::Cargo, "a#b"),
            (Ecosystem::Cargo, "a\\b"),
            (Ecosystem::Cargo, "1crate"),
            (Ecosystem::Cargo, "crate.name"),
            (Ecosystem::Npm, "-x"),
            (Ecosystem::Npm, "@scope"),
            (Ecosystem::Npm, "@scope/a/b"),
            (Ecosystem::Npm, "scope/name"),
            (Ecosystem::Npm, ".hidden"),
            (Ecosystem::Npm, "_hidden"),
            (Ecosystem::Npm, "@scope/-x"),
            (Ecosystem::Pypi, "-x"),
            (Ecosystem::Pypi, "pkg/../etc"),
            (Ecosystem::Pypi, ".pkg"),
            (Ecosystem::Pypi, "pkg-"),
            (Ecosystem::Pypi, "pkg name"),
        ];
        for (ecosystem, name) in hazards {
            assert!(
                matches!(
                    ReleasePackageId::new(ecosystem, name),
                    Err(ReleasePackageIdParseError::UnsafeName { .. } | ReleasePackageIdParseError::Malformed { .. })
                ),
                "{ecosystem:?} accepted an unsafe package name: {name:?}"
            );
        }
    }

    /// The real names each ecosystem publishes under must keep working.
    #[test]
    fn release_package_names_accept_each_ecosystem_grammar() {
        for (ecosystem, name) in [
            (Ecosystem::Cargo, "callisto-model"),
            (Ecosystem::Cargo, "_private_crate"),
            (Ecosystem::Cargo, "serde"),
            (Ecosystem::Npm, "callisto"),
            (Ecosystem::Npm, "@orin-dx/callisto"),
            (Ecosystem::Npm, "left.pad"),
            (Ecosystem::Npm, "JSONStream"),
            (Ecosystem::Npm, "Base64"),
            (Ecosystem::Npm, "@_scope/_x"),
            (Ecosystem::Npm, "@types/node"),
            (Ecosystem::Pypi, "typing-extensions"),
            (Ecosystem::Pypi, "zope.interface"),
            (Ecosystem::Pypi, "Flask"),
        ] {
            let id = ReleasePackageId::new(ecosystem, name)
                .unwrap_or_else(|error| panic!("{ecosystem:?}/{name} must stay valid: {error}"));
            assert_eq!(id.name(), name);
        }
    }

    /// `.` names the directory an asset path is joined onto, not a file in it.
    #[test]
    fn artifact_slot_components_reject_dot_and_dot_dot() {
        let slot = |platform: &str, asset: &str| {
            ArtifactSlotId::new(
                ReleasePackageId::new(Ecosystem::Cargo, "demo").unwrap(),
                Version::semver(1, 0, 0),
                platform,
                asset,
                GitHubRepository::parse("orin-dx/callisto").unwrap(),
                ".github/workflows/release.yml",
                CommitSha::parse(&"b".repeat(40)).unwrap(),
            )
        };
        for (platform, asset) in [(".", "a.tar.gz"), ("linux", "."), ("..", "a.tar.gz"), ("linux", "..")] {
            assert!(
                matches!(slot(platform, asset), Err(ArtifactSlotError::UnsafeSlotComponent)),
                "unsafe slot component accepted: {platform:?}/{asset:?}"
            );
        }
        assert!(slot("x86_64-unknown-linux-gnu", "callisto.tar.gz").is_ok());
    }
}
