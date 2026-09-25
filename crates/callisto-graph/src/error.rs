use std::path::PathBuf;

use callisto_model::{
    ArtifactManifestError, Ecosystem, GroupName, ManifestError, PackageId, TagTemplateError, VersionParseError,
};

pub use crate::locate::LocateError;

use crate::commands::release::StaleReason;

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

    #[error("release intent references package `{package}` which is not an exact selected workspace package")]
    #[diagnostic(code(E123), help("Rebuild the release intent from the current workspace instead of reusing a selection from another workspace."))]
    ReleasePackageNotSelected { package: callisto_model::ReleasePackageId },

    #[error("release intent no longer matches the current workspace snapshot: {reason}")]
    #[diagnostic(
        code(E124),
        help("Regenerate and reapprove the release intent; no release operation was authorized.")
    )]
    ReleaseIntentStale { reason: StaleReason },

    #[error("registry reported `{package}@{version}` published, but does not yet show it")]
    #[diagnostic(
        code(E157),
        help(
            "The publish command already ran and the registry client reported success; this is \
             registry propagation lag, not an unauthorized or stale operation. Re-run \
             reconciliation once the registry catches up -- do not regenerate the release intent."
        )
    )]
    RegistryPublishUnconfirmed {
        package: String,
        version: callisto_model::Version,
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
        help("Regenerate the release intent; no release operation was authorized.")
    )]
    ReleaseExecutionState {
        #[source]
        source: callisto_model::ReleaseStateError,
    },

    #[error("ecosystem `{}` has no canonical manifest format for identity resolution", .ecosystem.prefix())]
    #[diagnostic(
        code(E155),
        help(
            "Identity resolution only supports Cargo, npm, and PyPI packages \
             (the ecosystems with an `Ecosystem::canonical_manifest_format`). \
             Remove this package's identity requirement for the unsupported \
             ecosystem, or resolve its identity through a different path."
        )
    )]
    UnsupportedIdentityEcosystem { ecosystem: Ecosystem },

    #[error("cannot parse package identifier `{name}` declared in `{}`: {source}", .path.display())]
    #[diagnostic(
        code(E156),
        help(
            "Fix the `name` field in the manifest so it forms a valid package \
             identifier: no leading `/`, no leading `-`, no `..` path traversal, \
             and non-empty."
        )
    )]
    PackageIdentifierParse {
        path: PathBuf,
        name: String,
        #[source]
        source: callisto_model::PackageIdParseError,
    },

    #[error("release intent could not be constructed: {source}")]
    #[diagnostic(
        code(E158),
        help(
            "Fix the reported release-intent construction problem (for example, an unsupported \
             execution trust profile) and rebuild the release intent from a valid decision."
        )
    )]
    ReleaseIntent {
        #[from]
        source: callisto_model::ReleaseIntentError,
    },

    #[error("release operation could not be constructed: {source}")]
    #[diagnostic(
        code(E159),
        help(
            "Fix the reported release-operation construction problem in the source release decision or package graph."
        )
    )]
    ReleaseOperation {
        #[from]
        source: callisto_model::ReleaseOperationError,
    },

    #[error("release decision could not be constructed: {source}")]
    #[diagnostic(
        code(E160),
        help("Fix the reported release-decision problem (for example an empty or duplicate roster) and re-derive it.")
    )]
    ReleaseDecision {
        #[from]
        source: callisto_model::ReleaseDecisionError,
    },

    #[error("release input snapshot could not be constructed: {source}")]
    #[diagnostic(
        code(E161),
        help("Fix the reported release-input-snapshot problem (for example a duplicated package) and re-derive it.")
    )]
    ReleaseInputSnapshot {
        #[from]
        source: callisto_model::ReleaseInputSnapshotError,
    },

    #[error("release package identifier is invalid: {source}")]
    #[diagnostic(
        code(E162),
        help(
            "Correct the malformed release package identifier reported here; it must be an exact ecosystem/name pair."
        )
    )]
    ReleasePackageId {
        #[from]
        source: callisto_model::ReleasePackageIdParseError,
    },

    #[error("registry operation for package `{package}` failed: {source}")]
    #[diagnostic(
        code(E163),
        help(
            "The registry itself rejected or could not complete the operation (authentication, \
             rate limiting, or a network failure). Resolve the underlying registry condition and \
             retry; this is not a stale or unauthorized release intent."
        )
    )]
    Registry {
        package: String,
        #[source]
        source: callisto_model::RegistryError,
    },

    #[error("command `{program}` failed: {failure}")]
    #[diagnostic(
        code(E164),
        help(
            "Inspect the reported program, arguments, and stderr to diagnose why the release \
             subprocess failed or produced output that could not be parsed."
        )
    )]
    ReleaseCommand {
        program: String,
        args: Vec<String>,
        #[source]
        failure: CommandFailure,
    },

    #[error("release commit verification failed for `{}`: {reason}", .commit.as_str())]
    #[diagnostic(
        code(E165),
        help(
            "The committed release decision, changelog, or manifest diff does not match what \
             this commit claims to release. Reconcile the commit's contents with its release \
             decision instead of regenerating the intent."
        )
    )]
    ReleaseCommitVerificationFailed {
        commit: callisto_model::CommitSha,
        #[source]
        reason: CommitVerificationFailure,
    },

    #[error("cannot decode committed release decision at `{}`: {message}", .path.display())]
    #[diagnostic(
        code(E166),
        help("Restore the committed release-decision file from a known-good commit; it was not treated as authorizing a release.")
    )]
    ReleaseDecisionDecode { path: PathBuf, message: String },

    #[error("remote release state conflicts with this release intent: {conflict}")]
    #[diagnostic(
        code(E167),
        help(
            "A tag or forge release already exists remotely with content that differs from this \
             release intent. Reconcile the remote state by hand -- this intent's authorization is \
             not in question."
        )
    )]
    ReleaseRemoteConflict {
        #[source]
        conflict: RemoteConflict,
    },

    #[error("unsupported release {feature}")]
    #[diagnostic(
        code(E168),
        help("This combination is not implemented for release; adjust the release configuration to use a supported combination.")
    )]
    UnsupportedRelease { feature: UnsupportedReleaseFeature },

    #[error("release selection for package `{package}` is invalid: {reason}")]
    #[diagnostic(
        code(E169),
        help("Select each package at most once, only packages with a pending release and a publish target, and every unreleased platform package a selected npm package depends on.")
    )]
    ReleaseSelectionInvalid {
        package: callisto_model::ReleasePackageId,
        reason: ReleaseSelectionInvalidReason,
    },

    #[error("release precondition unmet: requires {requirement}")]
    #[diagnostic(
        code(E170),
        help("Satisfy the reported precondition (for example a detached HEAD or a configured GitHub remote) before retrying.")
    )]
    ReleasePreconditionUnmet {
        requirement: ReleasePreconditionRequirement,
    },

    #[error("asset `{asset}` is built by `{package}`, which is not part of this release")]
    #[diagnostic(
        code(E179),
        help(
            "Release `{package}` in the same run: add a changeset for it, or put it in the product's [[fixed-group]]."
        )
    )]
    ReleaseArtifactOwnerNotReleased { asset: String, package: String },

    #[error(
        "the remote refused tag `{tag}` at {target}: a GitHub App token cannot push a commit whose \
         .github/workflows/ differs from every branch tip"
    )]
    #[diagnostic(
        code(E180),
        help(
            "This happens when releasing a commit that is not a branch tip (for example, recovering \
             an older release) with GITHUB_TOKEN, which cannot be granted the `workflows` scope. \
             Push tag `{tag}`, and every other tag of this release, as an annotated tag at \
             {target} with a non-App credential (a PAT or deploy key), then re-run the release."
        )
    )]
    ReleaseTagPushRefusedWorkflowGuard { tag: String, target: String },

    #[error("internal release invariant violated: {detail}")]
    #[diagnostic(
        code(E171),
        help(
            "This is an internal callisto defect, not an operator action; report it along with the full error detail."
        )
    )]
    ReleaseInvariant { detail: String },

    #[error("release execution is incomplete: {count} operation(s) lack verified terminal success")]
    #[diagnostic(
        code(E172),
        help("Use release reconcile to inspect the exact incomplete operations; do not treat this release as successful.")
    )]
    ReleaseIncomplete { count: usize },

    #[error("artifact repository `{configured}` does not match the prepared GitHub push remote `{remote}`")]
    #[diagnostic(
        code(E175),
        help("Use the repository derived from the trusted Git remote; Callisto will not upload product assets to a caller-selected repository.")
    )]
    ReleaseArtifactRepositoryMismatch {
        configured: callisto_model::GitHubRepository,
        remote: callisto_model::GitHubRepository,
    },

    #[error("release run envelope is not valid for this intent: {source}")]
    #[diagnostic(
        code(E178),
        help("Re-plan the release intent, or run execute with the orchestration revision and artifact manifest the intent was planned against.")
    )]
    ReleaseRunEnvelope {
        source: callisto_model::ReleaseRunEnvelopeError,
    },

    #[error("provider observation is not usable as release evidence: {source}")]
    #[diagnostic(
        code(E177),
        help("This is an internal callisto defect: a provider adapter produced evidence that does not belong to the operation's role. Report it with the full error detail.")
    )]
    ReleaseProviderObservation {
        source: callisto_model::ProviderObservationError,
    },

    #[error("cannot dispatch release operation `{operation:?}` because its provider observation is indeterminate")]
    #[diagnostic(
        code(E176),
        help(
            "Restore provider credentials or connectivity, then retry. Callisto will not dispatch an effect while it cannot determine the remote identity."
        )
    )]
    ReleaseProviderIndeterminate {
        operation: Box<callisto_model::ReleaseOperationId>,
    },

    #[error("[release] declares no forge-repository")]
    #[diagnostic(
        code(E198),
        help("Add forge-repository = \"owner/repo\" under [release] in callisto.toml.")
    )]
    ReleaseForgeRepositoryMissing,

    #[error("package `{package}` configures publish-to = [\"{target}\"], which release cannot dispatch yet")]
    #[diagnostic(
        code(E199),
        help("Remove the target from `publish-to` for this package, or publish it outside `callisto release`.")
    )]
    PublishTargetNotImplemented {
        package: callisto_model::ReleasePackageId,
        target: &'static str,
    },

    #[error("`{}` already exists; this workspace is already initialized", .path.display())]
    #[diagnostic(
        code(E190),
        help("Edit callisto.toml directly; `callisto init` only scaffolds a workspace without one.")
    )]
    InitAlreadyInitialized { path: PathBuf },

    #[error("`{}` is not a Git repository", .root.display())]
    #[diagnostic(
        code(E191),
        help("Run `git init` in the workspace root, then re-run `callisto init`.")
    )]
    InitNotGitRepository { root: PathBuf },

    #[error("no `origin` remote is configured")]
    #[diagnostic(
        code(E192),
        help("Add the repository's remote as `origin`: `git remote add origin <url>`.")
    )]
    InitOriginMissing,

    #[error("package `{package}` matches more than one tag convention: {}", .matches.join("; "))]
    #[diagnostic(
        code(E193),
        help("Write a [[package]] entry for it with `tag-template` set to the current convention and `previous-tag-templates` listing the older ones.")
    )]
    InitTagTemplateAmbiguous { package: String, matches: Vec<String> },

    #[error("several packages have `v{{version}}` tags: {}", .packages.join(", "))]
    #[diagnostic(
        code(E194),
        help("Give each package its own [[package]] `tag-template` (and `previous-tag-templates` for the shared `v{{version}}` tags).")
    )]
    InitSharedVersionTag { packages: Vec<String> },

    #[error("forge repository `{value}` is invalid: {reason}")]
    #[diagnostic(code(E195), help("Use the GitHub `owner/repo` the product releases to."))]
    InitInvalidForgeRepository { value: String, reason: String },

    #[error("forge repository `{configured}` does not match the origin remote `{origin}`")]
    #[diagnostic(
        code(E187),
        help("The release plan requires [release].forge-repository to be origin's GitHub repository. Use that `owner/repo`, or point origin at the repository binaries release to.")
    )]
    InitForgeRepositoryMismatch { configured: String, origin: String },

    #[error("artifact target `{triple}` is invalid: {reason}")]
    #[diagnostic(
        code(E196),
        help("Use distinct Rust target triples from `rustc --print target-list`.")
    )]
    InitInvalidTargetTriple { triple: String, reason: &'static str },

    #[error("no package produces a binary artifact, so there is nothing to ship")]
    #[diagnostic(
        code(E188),
        help("Remove --artifact-target, or add a binary target ([[bin]], an npm `bin`, or [project.scripts]).")
    )]
    InitNoBinaryPackage,

    #[error("product package `{package}` is invalid: {reason}")]
    #[diagnostic(code(E189), help("Name one of the binary-producing packages: {}.", .candidates.join(", ")))]
    InitInvalidProductPackage {
        package: String,
        reason: &'static str,
        candidates: Vec<String>,
    },

    #[error("`{}` already exists; init refuses to overwrite a generated workflow", .path.display())]
    #[diagnostic(
        code(E200),
        help("Remove or edit the existing file directly; `callisto init` never merges into it.")
    )]
    InitWorkflowExists { path: PathBuf },

    #[error("couldn't resolve callisto@{version} to a commit on orin-dx/callisto")]
    #[diagnostic(
        code(E201),
        help("check network access to github.com, or run `callisto init --no-workflow` to skip workflow generation")
    )]
    InitWorkflowVersionUnresolved { version: String },

    #[error(
        "callisto init cannot generate a workflow yet: build-matrix workflow generation is not supported ({reason})"
    )]
    #[diagnostic(
        code(E202),
        help("omit --workflow for this workspace; SPEC-DX-SETUP-WORKFLOW-MATRIX will add matrix-workflow generation")
    )]
    InitWorkflowNeedsMatrix { reason: String },
}

/// Source of a [`GraphError::ReleaseCommand`] (E164) failure: either the
/// subprocess exited non-zero, or it exited zero but produced output this
/// crate could not parse.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CommandFailure {
    /// `stderr` is held raw so failure classification keeps matching on the
    /// client's exact words; redaction happens on the way out, because this
    /// `Display` is the only route by which it reaches a diagnostic, a JSON
    /// report, or a release receipt.
    #[error(
        "exited with status {exit_code:?}: {}",
        callisto_model::redact_command_stderr(stderr)
    )]
    NonZeroExit { exit_code: Option<i32>, stderr: String },
    #[error("produced malformed output: {detail}")]
    MalformedOutput { detail: String },
}

/// One distinct disagreement between a merged release commit and the release
/// decision it claims to satisfy, carried by
/// [`GraphError::ReleaseCommitVerificationFailed`] (E165).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CommitVerificationFailure {
    #[error("commit does not match the expected HEAD")]
    HeadMismatch,
    #[error("the release-decision file was not written by this commit")]
    DecisionNotWrittenByCommit,
    #[error("no changeset was consumed by this commit")]
    NoChangesetConsumed,
    #[error("the changelog was not touched by this commit")]
    ChangelogNotTouched,
    #[error("the manifest is not part of this commit's diff")]
    ManifestNotInDiff,
    #[error("the manifest version does not match the claimed version")]
    ManifestVersionMismatch,
    #[error("an unclaimed package's version changed in this commit")]
    UnclaimedVersionChange,
    #[error("a claimed package was not observed in this commit")]
    ClaimedPackageNotObserved,
}

/// A remote Git tag or forge release that already exists with content
/// differing from what this release intent authorized, carried by
/// [`GraphError::ReleaseRemoteConflict`] (E167).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RemoteConflict {
    #[error("a tag with this name already exists at a different target commit")]
    TagTargetDiffers,
    #[error("a tag was pushed but was not observed on the remote afterward")]
    TagNotObservedAfterPush,
    #[error("a forge release with this tag already exists with different attributes")]
    ForgeReleaseDiffers,
    #[error("a forge release was created but was not observed afterward")]
    ForgeReleaseNotObservedAfterCreate,
    #[error("a forge release was published but was not observed as published afterward")]
    ForgeReleaseNotObservedAfterPublish,
    #[error("a release asset already exists with a different digest or length")]
    ArtifactDiffers,
    #[error("an uploaded release asset was not observed afterward")]
    ArtifactNotObservedAfterUpload,
    #[error("the registry holds this version but not the identity this intent authorized")]
    RegistryVersionDiffers,
}

/// A release feature with no implemented dispatch for the given
/// configuration, carried by [`GraphError::UnsupportedRelease`] (E168).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum UnsupportedReleaseFeature {
    #[error("ecosystem")]
    Ecosystem,
    #[error("source identity")]
    SourceIdentity,
    #[error("bump reason")]
    BumpReason,
    #[error("publish target")]
    PublishTarget,
}

/// Why a `--package` release selection is invalid, carried by
/// [`GraphError::ReleaseSelectionInvalid`] (E169).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReleaseSelectionInvalidReason {
    #[error("the package is selected more than once")]
    Duplicate,
    #[error("its on-disk version already matches its last release; there is nothing pending to release")]
    NotARelease,
    #[error("it configures no publish target to dispatch")]
    NoDispatchableTarget,
    #[error("the package has a duplicate registry target")]
    DuplicateRegistryTarget,
    #[error("it is an unreleased workspace package a selected package depends on; select it too")]
    DependencyNotSelected,
}

/// An unmet precondition for a release operation, carried by
/// [`GraphError::ReleasePreconditionUnmet`] (E170).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReleasePreconditionRequirement {
    #[error("a detached HEAD")]
    DetachedHead,
    #[error("the canonical root to match the workspace root")]
    CanonicalRootMatchesWorkspace,
    #[error("a git remote prepared during validation")]
    GitRemotePrepared,
    #[error("a GitHub remote")]
    GitHubRemote,
    #[error("a configured changelog")]
    ChangelogConfigured,
    #[error("a provided artifact manifest")]
    ArtifactManifestProvided,
    #[error("a verified artifact manifest")]
    VerifiedArtifactManifest,
    #[error("a provider that can observe what it publishes")]
    ObservableProvider,
    #[error("a registry client that can prove a version absent")]
    ObservableRegistryClient,
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

    #[error("invalid product release configuration: {detail}")]
    #[diagnostic(
        code(E197),
        help("Configure one supported product package and all four required artifact targets.")
    )]
    InvalidProductRelease { detail: String },

    #[error(transparent)]
    Tag(#[from] TagTemplateError),

    #[error(transparent)]
    VersionParse(#[from] VersionParseError),
}

#[cfg(test)]
mod redaction_tests {
    use super::*;

    /// A failing registry client echoes the token it was handed. That stderr
    /// reaches a miette diagnostic, `--format json`, and the CI step summary
    /// through this `Display`, so the credential must not survive it -- for
    /// every env var name the release path actually passes.
    #[test]
    fn non_zero_exit_display_redacts_release_path_credentials() {
        for (name, secret) in [
            ("CARGO_REGISTRY_TOKEN", "cio-secret-token-value"),
            ("GH_TOKEN", "ghp-secret-token-value"),
            ("NPM_TOKEN", "npm-secret-token-value"),
        ] {
            std::env::set_var(name, secret);
            let error = GraphError::ReleaseCommand {
                program: "cargo".to_owned(),
                args: vec!["publish".to_owned()],
                failure: CommandFailure::NonZeroExit {
                    exit_code: Some(1),
                    stderr: format!("error: failed to authenticate with token {secret}\n"),
                },
            };
            let rendered = error.to_string();
            std::env::remove_var(name);
            assert!(
                !rendered.contains(secret),
                "{name} value survived into the diagnostic: {rendered}"
            );
            assert!(
                rendered.contains("[REDACTED]"),
                "{name} value must be replaced by the redaction marker: {rendered}"
            );
            assert!(
                rendered.contains("failed to authenticate"),
                "redaction must keep the surrounding diagnostic text: {rendered}"
            );
        }
    }

    /// An authenticated remote URL is the leak shape that needs no env var to
    /// be set, so it is redacted independently of the token list.
    #[test]
    fn non_zero_exit_display_redacts_url_userinfo() {
        let failure = CommandFailure::NonZeroExit {
            exit_code: Some(128),
            stderr: "fatal: could not read from https://x-access-token:ghs_live@github.com/o/r\n".to_owned(),
        };
        let rendered = failure.to_string();
        assert!(!rendered.contains("ghs_live"), "userinfo survived: {rendered}");
    }
}
