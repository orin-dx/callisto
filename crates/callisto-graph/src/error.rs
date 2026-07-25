use std::path::PathBuf;

use callisto_model::{GroupName, ManifestError, PackageId, TagTemplateError, VersionParseError};

pub use crate::locate::LocateError;

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[allow(clippy::result_large_err)]
#[non_exhaustive]
pub enum GraphError {
    #[error(transparent)]
    Locate(#[from] LocateError),

    #[error(transparent)]
    Manifest(#[from] ManifestError),

    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    Format(#[from] callisto_format::ParseError),

    #[error(transparent)]
    Bump(#[from] callisto_format::BumpError),

    #[error(transparent)]
    Changelog(#[from] callisto_changelog::ChangelogError),

    #[cfg(feature = "inference")]
    #[error(transparent)]
    Conventional(#[from] callisto_conventional::ConventionalError),

    #[error(transparent)]
    TagTemplate(#[from] callisto_model::TagTemplateError),

    #[error(transparent)]
    VersionParse(#[from] callisto_model::VersionParseError),

    #[error(transparent)]
    Model(#[from] callisto_model::ModelError),

    #[error("command error: {0}")]
    Command(#[from] callisto_model::CommandError),

    #[error("package `{id}` is defined at multiple paths: {}", .paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "))]
    DuplicatePackage { id: PackageId, paths: Vec<PathBuf> },

    #[error("package at `{path}` declares conflicting identities: {}", .ids.iter().map(|i| i.display_name()).collect::<Vec<_>>().join(", "))]
    SplitIdentity { path: PathBuf, ids: Vec<PackageId> },

    #[error("package `{id}` was not found in the workspace")]
    UnknownPackage { id: PackageId },

    #[error("name `{name}` is ambiguous in this workspace; candidates: {}", .candidates.iter().map(|c| c.display_name()).collect::<Vec<_>>().join(", "))]
    AmbiguousName {
        name: String,
        candidates: Vec<PackageId>,
    },

    #[error("dependency cycle detected: {}", .cycle.iter().map(|i| i.display_name()).collect::<Vec<_>>().join(" -> "))]
    Cycle { cycle: Vec<PackageId> },

    #[error("cascade failed to converge after {iterations} iterations")]
    CascadeNotConverged { iterations: usize },

    #[error("fixed group `{group}` members have divergent on-disk versions: {}", .members.iter().map(|(id, v)| format!("{}={}", id.display_name(), v.render())).collect::<Vec<_>>().join(", "))]
    FixedGroupDivergent {
        group: GroupName,
        members: Vec<(PackageId, callisto_model::Version)>,
    },

    #[error("group `{group}` members use incompatible versioning grammars: {}", .members.iter().map(|(id, v)| format!("{}={:?}", id.display_name(), v.grammar())).collect::<Vec<_>>().join(", "))]
    GroupGrammarMismatch {
        group: GroupName,
        members: Vec<(PackageId, callisto_model::Version)>,
    },

    #[error("group `{group}` lists member `{member}`, which was not found in the workspace")]
    MissingGroupMember { group: GroupName, member: String },

    #[error("package `{package}` is listed in multiple conflicting groups: {}", .groups.iter().map(|g| g.as_str()).collect::<Vec<_>>().join(", "))]
    ConflictingGroupMembership {
        package: PackageId,
        groups: Vec<GroupName>,
    },

    #[error(
        "version dependency edge from `{from}` to `{to}` involves incompatible grammars: {source}"
    )]
    GrammarMismatch {
        from: PackageId,
        to: PackageId,
        #[source]
        source: callisto_model::GrammarMismatch,
    },

    #[error("on-disk versions changed since plan was generated for `{package}`: expected {}, found {}", .expected.render(), .found.render())]
    OnDiskVersionDrift {
        package: PackageId,
        expected: callisto_model::Version,
        found: callisto_model::Version,
    },

    #[error("workspace root `{root_manifest}` has conflicting version updates: {details}")]
    WorkspaceVersionConflict {
        root_manifest: PathBuf,
        details: String,
    },
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigError {
    #[error("failed to read `{path}`: {message}")]
    Read { path: PathBuf, message: String },

    #[error("`{path}` is not valid TOML: {message}")]
    ParseToml { path: PathBuf, message: String },

    #[error("`{path}` is not valid YAML: {message}")]
    ParseYaml { path: PathBuf, message: String },

    #[error("[[package-set]] `{pattern}` matched no packages")]
    PackageSetMatchedNothing { pattern: String },

    #[error("[[package]] `{pattern}` matched no package")]
    PackageMatchedNothing { pattern: String },

    #[error("package `{package}` is claimed by more than one [[package-set]]: {}", .patterns.join(", "))]
    OverlappingPackageSets {
        package: String,
        patterns: Vec<String>,
    },

    #[error("group `{group}` and group `{other}` both list `{member}`")]
    ConflictingGroupNames {
        group: GroupName,
        other: GroupName,
        member: String,
    },

    #[error("group `{group}` has no members")]
    EmptyGroup { group: GroupName },

    #[error("duplicate group name `{group}`")]
    DuplicateGroupName { group: GroupName },

    #[error("`publish-to` names registry key `{key}`, which no [registries.*] block defines")]
    UnknownRegistry { key: String },

    #[error("`{path}` sets unknown callisto key `{key}`")]
    UnknownKey { path: PathBuf, key: String },

    #[error("`cascade.bump-severity` is `{found}`; expected `patch` or `minor`")]
    InvalidBumpSeverity { found: String },

    #[error("`pre-major-inference` is `{found}`; expected `off`, `conservative`, or `conservative-feat`")]
    InvalidPreMajorInference { found: String },

    #[error(transparent)]
    Tag(#[from] TagTemplateError),

    #[error(transparent)]
    VersionParse(#[from] VersionParseError),
}
