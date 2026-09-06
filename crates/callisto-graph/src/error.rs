use std::path::PathBuf;

use callisto_model::{
    ArtifactManifestError, Ecosystem, GroupName, ManifestError, PackageId, TagTemplateError, VersionParseError,
};

pub use crate::locate::LocateError;

#[derive(Clone, Debug, thiserror::Error, miette::Diagnostic, PartialEq, Eq)]
#[allow(clippy::result_large_err)]
#[non_exhaustive]
pub enum GraphError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Locate(#[from] LocateError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Manifest(#[from] ManifestError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Format(#[from] callisto_format::ParseError),

    #[error("parsing changeset {}: {source}", .path.display())]
    ParseChangeset {
        path: PathBuf,
        source: callisto_format::ParseError,
    },

    #[error(transparent)]
    #[diagnostic(transparent)]
    Bump(#[from] callisto_format::BumpError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Changelog(#[from] callisto_changelog::ChangelogError),

    #[cfg(feature = "inference")]
    #[error(transparent)]
    Conventional(#[from] callisto_conventional::ConventionalError),

    #[error(transparent)]
    TagTemplate(#[from] callisto_model::TagTemplateError),

    #[error(transparent)]
    VersionParse(#[from] callisto_model::VersionParseError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Model(#[from] callisto_model::ModelError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Vcs(#[from] callisto_vcs::VcsError),

    #[error("command error: {0}")]
    Command(#[from] callisto_model::CommandError),

    #[error("package `{id}` is defined at multiple paths: {}", .paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "))]
    #[diagnostic(code(E100), help("Ensure package IDs are unique across workspace manifest paths."))]
    DuplicatePackage { id: PackageId, paths: Vec<PathBuf> },

    #[error("package at `{path}` declares conflicting identities: {}", .ids.iter().map(|i| i.display_name()).collect::<Vec<_>>().join(", "))]
    #[diagnostic(code(E101), help("Align package name declarations in manifest files."))]
    SplitIdentity { path: PathBuf, ids: Vec<PackageId> },

    #[error("package `{id}` was not found in the workspace")]
    #[diagnostic(
        code(E102),
        help("Verify package is included in workspace members in callisto.toml.")
    )]
    UnknownPackage { id: PackageId },

    #[error("name `{name}` is ambiguous in this workspace; candidates: {}", .candidates.iter().map(|c| c.display_name()).collect::<Vec<_>>().join(", "))]
    #[diagnostic(
        code(E103),
        help("Use fully-qualified package ID with ecosystem prefix (e.g. cargo:pkg).")
    )]
    AmbiguousName { name: String, candidates: Vec<PackageId> },

    #[error("dependency cycle detected: {}", .cycle.iter().map(|i| i.display_name()).collect::<Vec<_>>().join(" -> "))]
    #[diagnostic(
        code(E104),
        help("Refactor workspace dependencies to break the cyclic dependency chain.")
    )]
    Cycle { cycle: Vec<PackageId> },

    #[error("cascade failed to converge after {iterations} iterations")]
    #[diagnostic(code(E105), help("Check for oscillating peer or linked group dependencies."))]
    CascadeNotConverged { iterations: usize },

    #[error("fixed group `{group}` members have divergent on-disk versions: {}", .members.iter().map(|(id, v)| format!("{}={}", id.display_name(), v.render())).collect::<Vec<_>>().join(", "))]
    #[diagnostic(code(E106), help("Align on-disk versions for all members of the fixed group."))]
    FixedGroupDivergent {
        group: GroupName,
        members: Vec<(PackageId, callisto_model::Version)>,
    },

    #[error("group `{group}` members use incompatible versioning grammars: {}", .members.iter().map(|(id, v)| format!("{}={:?}", id.display_name(), v.grammar())).collect::<Vec<_>>().join(", "))]
    #[diagnostic(code(E107))]
    GroupGrammarMismatch {
        group: GroupName,
        members: Vec<(PackageId, callisto_model::Version)>,
    },

    #[error("group `{group}` lists member `{member}`, which was not found in the workspace")]
    #[diagnostic(code(E108))]
    MissingGroupMember { group: GroupName, member: String },

    #[error("package `{package}` is listed in multiple conflicting groups: {}", .groups.iter().map(|g| g.as_str()).collect::<Vec<_>>().join(", "))]
    #[diagnostic(code(E109))]
    ConflictingGroupMembership { package: PackageId, groups: Vec<GroupName> },

    #[error("version dependency edge from `{from}` to `{to}` involves incompatible grammars: {source}")]
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

    #[error("cannot apply version plan: manifest `{}` is at version {}, expected {} (pre-apply) or {} (already applied — safe to retry)", .path.display(), .found.render(), .expected_from.render(), .expected_to.render())]
    #[diagnostic(
        code(E117),
        help(
            "The manifest version does not match the plan's from or to version. \
              This may indicate the manifest was modified outside of callisto after the plan was generated."
        )
    )]
    UnexpectedManifestVersion {
        path: PathBuf,
        expected_from: callisto_model::Version,
        expected_to: callisto_model::Version,
        found: callisto_model::Version,
    },

    #[error("workspace root `{root_manifest}` has conflicting version updates: {details}")]
    WorkspaceVersionConflict { root_manifest: PathBuf, details: String },

    #[error("failed to parse .changeset/pre.json: {0}")]
    #[diagnostic(
        code(E114),
        help("Check that .changeset/pre.json is valid JSON and was not partially written. Delete the file and re-run `callisto pre enter` to recover.")
    )]
    PreJson(callisto_format::PreJsonError),

    #[error("failed to read .changeset/pre.json: {message}")]
    #[diagnostic(
        code(E115),
        help(
            "Check that .changeset/pre.json is readable. Delete the file and re-run `callisto pre enter` to recover."
        )
    )]
    PreJsonRead { message: String },

    #[error("package `{package}` declares platform targets via both `{napi_source}` and `{maturin_source}`; only one source is allowed")]
    #[diagnostic(
        code(E118),
        help("Remove one of the two target declarations -- either napi.targets in package.json or [tool.maturin].targets in pyproject.toml -- from the package's manifest.")
    )]
    ConflictingPlatformTargetSources {
        package: PackageId,
        napi_source: &'static str,
        maturin_source: &'static str,
    },

    #[error(
        "package `{package}` configures publish-to target `{target}` (ecosystem `{}`), but its detected ecosystem is `{}`",
        .target_ecosystem.prefix(),
        .package_ecosystems.iter().map(|e| e.prefix()).collect::<Vec<_>>().join(", ")
    )]
    #[diagnostic(
        code(E119),
        help("Remove the mismatched target from publish-to, or fix the [[package]]/[[package-set]] rule so it only matches packages in that ecosystem.")
    )]
    PublishTargetEcosystemMismatch {
        package: PackageId,
        target: String,
        target_ecosystem: Ecosystem,
        package_ecosystems: Vec<Ecosystem>,
    },

    #[error(
        "package `{package}` sets `publishConfig.registry` to `{url}`, which is not an operator-approved npm registry"
    )]
    #[diagnostic(
        code(E120),
        help(
            "`publishConfig.registry` in package.json is manifest-controlled data (a PR author \
             can set it in their own package.json), not operator config, so it is never trusted \
             verbatim as a publish destination. The URL must use the `https` scheme and must \
             exactly match a `url` configured on an `npm`-kind entry in `[registries]` in \
             callisto.toml. Add the registry there if it is a legitimate private registry, or \
             remove the override from package.json."
        )
    )]
    UntrustedNpmRegistry { package: PackageId, url: String },

    #[error(
        "npm main package `{main}` depends on platform package `{depends_on}`, which is neither \
         in this publish plan nor already published"
    )]
    #[diagnostic(
        code(E121),
        help(
            "The platform package is either misconfigured (missing a `publish-to = [\"npm\"]` \
             target) or was excluded from this run by `--package`. Either fix its publish-to \
             configuration, or include it in this run alongside its dependent main package."
        )
    )]
    MissingPlatformDependency { main: PackageId, depends_on: String },

    #[error("package `{id}` was requested via `--package` but is not part of this publish plan: {reason}")]
    #[diagnostic(code(E122))]
    PackageNotInPublishPlan { id: PackageId, reason: NotInPlanReason },

    #[error("release intent references package `{package}` which is not an exact selected workspace package")]
    #[diagnostic(code(E123), help("Rebuild the release intent from the current workspace instead of reusing a selection from another workspace."))]
    ReleasePackageNotSelected { package: callisto_model::ReleasePackageId },

    #[error("release intent no longer matches the current workspace snapshot")]
    #[diagnostic(
        code(E124),
        help("Regenerate and reapprove the release intent; no release operation was authorized.")
    )]
    ReleaseIntentStale,

    #[error("fixed/linked group `{group}` did not converge to one target version in the merged release commit: {}", .members.iter().map(|(id, v)| format!("{}={}", id.display_name(), v.render())).collect::<Vec<_>>().join(", "))]
    #[diagnostic(
        code(E155),
        help("A cascade-authorized group must land every member on the same target version; regenerate the release PR instead of hand-editing manifests.")
    )]
    ReleaseCommitCascadeDivergent {
        group: GroupName,
        members: Vec<(PackageId, callisto_model::Version)>,
    },

    #[error("artifact manifest is not authorized by this release intent: {source}")]
    #[diagnostic(
        code(E136),
        help("Regenerate the artifact manifest for this exact release intent; no artifact was uploaded.")
    )]
    ArtifactManifest {
        #[source]
        source: ArtifactManifestError,
    },

    #[error("artifact path `{path}` is unsafe: {reason}")]
    #[diagnostic(
        code(E137),
        help("Place the built asset directly beneath the explicit artifact directory without symbolic links.")
    )]
    UnsafeArtifactPath { path: PathBuf, reason: &'static str },

    #[error("cannot read artifact `{path}`: {message}")]
    #[diagnostic(
        code(E138),
        help("Ensure the build artifact is readable and has not changed since it was attested.")
    )]
    ArtifactRead { path: PathBuf, message: String },

    #[error("artifact `{path}` does not match its manifest digest or byte length")]
    #[diagnostic(code(E139), help("Rebuild and re-attest the artifact; it was not uploaded."))]
    ArtifactBytesMismatch { path: PathBuf },

    #[error("GitHub could not verify the attestation for artifact `{path}`: {message}")]
    #[diagnostic(
        code(E140),
        help("Verify that the artifact was built by the exact trusted workflow and source commit declared in the release intent.")
    )]
    ArtifactAttestation { path: PathBuf, message: String },

    #[error("cannot read release input `{}`: {message}", .path.display())]
    #[diagnostic(
        code(E125),
        help("Ensure the release manifest and configuration files remain readable until validation completes.")
    )]
    ReleaseInputRead { path: PathBuf, message: String },

    #[error("registry binding `{registry}` is not a credential-free canonical URL: {reason}")]
    #[diagnostic(
        code(E126),
        help("Use a URL without userinfo, query parameters, or fragments in callisto.toml.")
    )]
    UnsafeRegistryBinding { registry: String, reason: &'static str },

    #[error("unsafe Git push remote: {reason}")]
    #[diagnostic(code(E132), help("Configure origin with a credential-free HTTPS or SSH URL."))]
    UnsafeGitRemote { reason: &'static str },

    #[error("release execution state is invalid: {source}")]
    #[diagnostic(
        code(E127),
        help("Regenerate the release intent or remove the invalid state file; no release operation was authorized.")
    )]
    ReleaseExecutionState {
        #[source]
        source: callisto_model::ReleaseStateError,
    },

    #[error("cannot read release execution state `{}`: {message}", .path.display())]
    #[diagnostic(code(E128), help("Ensure the explicit release state path is readable."))]
    ReleaseStateRead { path: PathBuf, message: String },

    #[error("release execution state `{}` is not valid JSON: {message}", .path.display())]
    #[diagnostic(
        code(E129),
        help("Restore the state file from a known-good receipt or start a new release intent; it was not treated as success.")
    )]
    ReleaseStateDecode { path: PathBuf, message: String },

    #[error("cannot persist release execution state `{}`: {message}", .path.display())]
    #[diagnostic(
        code(E130),
        help("Resolve the local filesystem error before any remote release effect is attempted.")
    )]
    ReleaseStateWrite { path: PathBuf, message: String },
}

/// Why a workspace package that a `--package` filter named is nonetheless
/// absent from the computed [`crate::commands::plan_publish`] plan, distinct
/// from [`GraphError::UnknownPackage`] (which means the name matched no
/// workspace package at all).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NotInPlanReason {
    #[error("its on-disk version already matches its last release tag; there is nothing pending to publish")]
    NotARelease,
    #[error("it configures no publish target with an implemented dispatch (e.g. only NuGet or GitHub Release)")]
    NoDispatchableTarget,
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_vcs::VcsError;

    /// Spec: GraphError::Vcs must be transparent — wrapping a VcsError must
    /// not add any prefix (e.g. "vcs error: ") to the display message.
    /// Before the fix, format!("{err}") produces "vcs error: <inner>".
    #[test]
    fn graph_error_vcs_is_transparent_no_prefix() {
        let inner = VcsError::Git("some git error".to_string());
        let expected_msg = format!("{inner}");
        let graph_err = GraphError::Vcs(inner);
        assert_eq!(
            format!("{graph_err}"),
            expected_msg,
            "GraphError::Vcs must be transparent (no 'vcs error: ' prefix)"
        );
    }

    /// AC-017 (message shape): ConflictingPlatformTargetSources must name the
    /// package and both source field names in its Display text, and carry
    /// diagnostic code E118 with help text pointing at the fix.
    #[test]
    fn conflicting_platform_target_sources_message_names_package_and_both_sources() {
        use callisto_model::PackageId;

        let err = GraphError::ConflictingPlatformTargetSources {
            package: PackageId::Bare("native-mod".to_string()),
            napi_source: "napi.targets",
            maturin_source: "[tool.maturin].targets",
        };
        let msg = format!("{err}");
        assert!(msg.contains("native-mod"), "message must name the package: {msg}");
        assert!(msg.contains("napi.targets"), "message must name napi_source: {msg}");
        assert!(
            msg.contains("[tool.maturin].targets"),
            "message must name maturin_source: {msg}"
        );
    }
}

#[derive(Clone, Debug, thiserror::Error, miette::Diagnostic, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigError {
    #[error("failed to read `{path}`: {message}")]
    #[diagnostic(code(E110))]
    Read { path: PathBuf, message: String },

    #[error("`{path}` is not valid TOML: {message}")]
    #[diagnostic(code(E111), help("Verify callisto.toml TOML syntax formatting."))]
    ParseToml { path: PathBuf, message: String },

    #[error("[[package-set]] `{pattern}` matched no packages")]
    PackageSetMatchedNothing { pattern: String },

    #[error("[[package]] `{pattern}` matched no package")]
    PackageMatchedNothing { pattern: String },

    #[error("package `{package}` is claimed by more than one [[package-set]]: {}", .patterns.join(", "))]
    OverlappingPackageSets { package: String, patterns: Vec<String> },

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

    #[error(
        "changesets.dir `{dir}` is an absolute path or contains `..` path components and would escape the workspace root"
    )]
    #[diagnostic(
        code(E116),
        help("Use a forward-slash-separated path relative to the workspace root that is not absolute and does not contain '..' components.")
    )]
    InvalidChangesetsDir { dir: String },

    #[error("`changelog = \"{value}\"` on `{pattern}` is an absolute path or contains `..` path components and would escape the workspace root")]
    #[diagnostic(
        code(E113),
        help("Use a forward-slash-separated path relative to the package root that does not contain '..' components.")
    )]
    InvalidChangelogPath { pattern: String, value: String },

    #[error(transparent)]
    Tag(#[from] TagTemplateError),

    #[error(transparent)]
    VersionParse(#[from] VersionParseError),
}
