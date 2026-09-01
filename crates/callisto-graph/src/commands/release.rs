//! Fresh, graph-owned authorization for durable release intents.
//!
//! This module intentionally takes a root, locator, and runner rather than a
//! [`Workspace`]. A workspace caches parsed manifests and config for a normal
//! command invocation; accepting one here would make a previously observed
//! graph look current after the filesystem changed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use callisto_model::{
    ApplyPermit, CanonicalTranscript, CommandRunner, CommitSha, DepKind, Ecosystem, ExecutionTrustProfileV1, NpmAccess,
    OperationOutcome, PublishOutcome, PublishTarget, RegistryBindingDigest, RegistryBindingId, RegistryKey,
    ReleaseDecisionV1, ReleaseInputSnapshotV1, ReleaseIntentV1, ReleaseOperation, ReleaseOperationId, ReleasePackageId,
    ReleasePackageInputV1, SemanticInputDigest, SourceIdentity, TagName, Version,
};
use callisto_vcs::{
    access::{GitCommitTrustEvidence, GitHeadDisposition},
    release_lock::ReleaseWorkspaceLock,
};

use crate::{commands::publish_client, DependencyResolver, GraphError, ProjectLocator, Workspace};

/// The invocation data for one effect. This is deliberately graph-private:
/// callers can inspect the serializable intent, but cannot substitute a new
/// endpoint, tag target, or package directory at execution time.
#[allow(dead_code)] // consumed by the durable executor batch
#[derive(Debug)]
enum PreparedOperation {
    RegistryPublish {
        package_dir: std::path::PathBuf,
        package_name: String,
        version: Version,
        registry: PreparedRegistryBinding,
        npm_access: Option<NpmAccess>,
    },
    Tag {
        name: TagName,
        target: CommitSha,
        annotation: String,
    },
    ForgeRelease {
        tag: TagName,
    },
}

/// Credential-free, canonical registry routing. `endpoint` is populated only
/// after parsing rejects userinfo, query, and fragments, so a later executor
/// cannot recover credentials from this capability.
#[allow(dead_code)] // consumed by the durable executor batch
#[derive(Debug)]
struct PreparedRegistryBinding {
    key: RegistryKey,
    endpoint: Option<String>,
    identity: RegistryBindingDigest,
}

/// Immutable, credential-free destination for Git effects. Its canonical
/// digest is included in each tag-producing package fingerprint; the URL is
/// retained only in the validated, non-serializable capability.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedGitRemote {
    endpoint: String,
    identity: SemanticInputDigest,
    github_repository: Option<String>,
}

/// Graph-private inputs prepared from the same fresh observation as an intent.
/// The executor will consume these when introduced; retaining them in the
/// capability prevents a validated public intent being paired with new inputs.
#[allow(dead_code)] // consumed by the durable executor batch
#[derive(Debug)]
struct PreparedReleaseInputs {
    root: std::path::PathBuf,
    source: SourceIdentity,
    trust: GitCommitTrustEvidence,
    git_remote: Option<PreparedGitRemote>,
    _lock: ReleaseWorkspaceLock,
    operations: BTreeMap<ReleaseOperationId, PreparedOperation>,
}

#[derive(Debug, PartialEq, Eq)]
struct ObservedTag {
    target: CommitSha,
    annotation: String,
}

#[derive(Debug, PartialEq, Eq)]
enum ForgeReleaseObservation {
    Missing,
    Exact,
    Conflict,
}

type DerivedReleaseInputs = (
    ReleaseInputSnapshotV1,
    Vec<ReleaseOperation>,
    BTreeMap<ReleaseOperationId, PreparedOperation>,
    Option<PreparedGitRemote>,
);

struct PreparedDerivation {
    operations: BTreeMap<ReleaseOperationId, PreparedOperation>,
    git_remote: Option<PreparedGitRemote>,
}

/// In-memory, non-transferable proof that an intent was rebuilt from a fresh
/// workspace observation. It is intentionally neither cloneable nor serializable.
pub struct ValidatedReleaseIntent<'a> {
    intent: ReleaseIntentV1,
    prepared: PreparedReleaseInputs,
    #[allow(dead_code)] // consumed by the durable execution coordinator
    runner: &'a dyn CommandRunner,
}

impl std::fmt::Debug for ValidatedReleaseIntent<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedReleaseIntent")
            .field("intent", &self.intent)
            .finish_non_exhaustive()
    }
}

impl ValidatedReleaseIntent<'_> {
    pub fn intent(&self) -> &ReleaseIntentV1 {
        &self.intent
    }

    #[allow(dead_code)] // consumed by the durable executor batch
    fn prepared(&self) -> &PreparedReleaseInputs {
        &self.prepared
    }

    /// Re-observes the held workspace immediately before an effect.
    #[allow(dead_code)] // consumed by the durable execution coordinator
    pub(crate) fn recheck_trust(&self) -> Result<(), GraphError> {
        let evidence =
            callisto_vcs::GitAccess::discover(&self.prepared.root, self.runner).observe_git_commit_trust()?;
        if evidence != self.prepared.trust || source_from_trust(&evidence) != self.prepared.source {
            return Err(GraphError::ReleaseIntentStale);
        }
        if self
            .prepared
            .git_remote
            .as_ref()
            .is_some_and(|expected| prepared_git_remote(&self.prepared.root, self.runner).as_ref() != Ok(expected))
        {
            return Err(GraphError::ReleaseIntentStale);
        }
        Ok(())
    }
}

/// Builds a release intent from a fresh root-bound observation.
pub fn build_release_intent<L: ProjectLocator, R: CommandRunner>(
    root: &Path,
    locator: &L,
    runner: &R,
    decision: &ReleaseDecisionV1,
    trust_profile: ExecutionTrustProfileV1,
) -> Result<ReleaseIntentV1, GraphError> {
    let root = canonical_root(root)?;
    let workspace = Workspace::load(root.clone(), locator, runner)?;
    let source = observe_source(&workspace, trust_profile)?;
    let intent = derive_release_intent(&workspace, decision, source.clone(), trust_profile)?;

    // Recheck after all input reads. A concurrent edit or checkout cannot be
    // authorized merely because it happened after the first check.
    if observe_source(&workspace, trust_profile)? != source {
        return Err(GraphError::ReleaseIntentStale);
    }
    Ok(intent)
}

/// Re-observes root, config, package discovery, manifests, Git evidence, and
/// the exact operation DAG before creating an opaque authorization capability.
pub fn validate_release_intent<'a, L: ProjectLocator, R: CommandRunner>(
    root: &Path,
    locator: &L,
    runner: &'a R,
    received: ReleaseIntentV1,
) -> Result<ValidatedReleaseIntent<'a>, GraphError> {
    validate_release_intent_with_state_directory(root, locator, runner, None, received)
}

fn validate_release_intent_with_state_directory<'a, L: ProjectLocator, R: CommandRunner>(
    root: &Path,
    locator: &L,
    runner: &'a R,
    state_directory: Option<&Path>,
    received: ReleaseIntentV1,
) -> Result<ValidatedReleaseIntent<'a>, GraphError> {
    let root = canonical_root(root)?;
    let workspace = Workspace::load(root.clone(), locator, runner)?;
    let initial_trust = observe_git_trust(&workspace, received.trust_profile)?;
    let lock = ReleaseWorkspaceLock::acquire(&root, state_directory)?;
    let trust = observe_git_trust(&workspace, received.trust_profile)?;
    if trust != initial_trust {
        return Err(GraphError::ReleaseIntentStale);
    }
    let source = source_from_trust(&trust);
    let (expected, prepared) =
        derive_release_intent_with_prepared(&workspace, &received.decision, source.clone(), received.trust_profile)?;
    let final_trust = observe_git_trust(&workspace, received.trust_profile)?;
    if expected != received || final_trust != trust {
        return Err(GraphError::ReleaseIntentStale);
    }

    Ok(ValidatedReleaseIntent {
        prepared: PreparedReleaseInputs {
            root,
            source,
            trust: final_trust,
            git_remote: prepared.git_remote,
            _lock: lock,
            operations: prepared.operations,
        },
        intent: received,
        runner,
    })
}

fn canonical_root(root: &Path) -> Result<std::path::PathBuf, GraphError> {
    dunce::canonicalize(root).map_err(|error| GraphError::ReleaseInputRead {
        path: root.to_path_buf(),
        message: error.to_string(),
    })
}

fn observe_source<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    trust_profile: ExecutionTrustProfileV1,
) -> Result<SourceIdentity, GraphError> {
    Ok(source_from_trust(&observe_git_trust(workspace, trust_profile)?))
}

fn observe_git_trust<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    trust_profile: ExecutionTrustProfileV1,
) -> Result<GitCommitTrustEvidence, GraphError> {
    if !matches!(trust_profile, ExecutionTrustProfileV1::GitCommit) {
        return Err(GraphError::ReleaseIntentStale);
    }
    let evidence = workspace.git_access().observe_git_commit_trust()?;
    if evidence.canonical_root() != workspace.root || evidence.head_disposition() != GitHeadDisposition::Detached {
        return Err(GraphError::ReleaseIntentStale);
    }
    Ok(evidence)
}

fn source_from_trust(evidence: &GitCommitTrustEvidence) -> SourceIdentity {
    SourceIdentity::GitCommit {
        sha: evidence.head().clone(),
    }
}

impl ValidatedReleaseIntent<'_> {
    /// Dispatches exactly one graph-private operation prepared during fresh
    /// validation. This is intentionally the only production effect path: it
    /// never accepts a path, endpoint, tag, package name, or version from a
    /// caller.
    pub(crate) fn dispatch_prepared(
        &self,
        permit: &ApplyPermit,
        id: &ReleaseOperationId,
    ) -> Result<OperationOutcome, GraphError> {
        let operation = self.prepared.operations.get(id).ok_or(GraphError::ReleaseIntentStale)?;
        match operation {
            PreparedOperation::RegistryPublish { .. } => self.dispatch_registry(permit, id, operation),
            PreparedOperation::Tag {
                name,
                target,
                annotation,
            } => self.dispatch_tag(name, target, annotation),
            PreparedOperation::ForgeRelease { tag } => self.dispatch_forge_release(tag),
        }
    }

    fn dispatch_registry(
        &self,
        _permit: &ApplyPermit,
        id: &ReleaseOperationId,
        prepared: &PreparedOperation,
    ) -> Result<OperationOutcome, GraphError> {
        let PreparedOperation::RegistryPublish {
            package_dir,
            package_name,
            version,
            registry,
            npm_access,
        } = prepared
        else {
            return Err(GraphError::ReleaseIntentStale);
        };
        let cwd = self.prepared.root.join(package_dir);
        let output = match id.package.ecosystem() {
            Ecosystem::Cargo => {
                let manifest = cwd.join("Cargo.toml");
                let manifest = manifest.to_string_lossy();
                let mut args = vec!["publish", "--manifest-path", manifest.as_ref()];
                if registry.key.as_str() != RegistryKey::CRATES_IO {
                    args.extend(["--registry", registry.key.as_str()]);
                }
                self.runner.run("cargo", &args, &self.prepared.root)?
            }
            Ecosystem::Npm => {
                if self.npm_version_is_published(package_name, version, registry.endpoint.as_deref())? {
                    return Ok(OperationOutcome::AlreadySatisfied);
                }
                let mut args = vec!["publish"];
                if let Some(endpoint) = registry.endpoint.as_deref() {
                    args.extend(["--registry", endpoint]);
                }
                if let Some(access) = npm_access {
                    args.extend([
                        "--access",
                        match access {
                            NpmAccess::Public => "public",
                            NpmAccess::Restricted => "restricted",
                        },
                    ]);
                }
                self.runner.run("npm", &args, &cwd)?
            }
            Ecosystem::Pypi => {
                // Never upload from the workspace's mutable `dist/` directory.
                // The temporary directory is created for this exact operation
                // and dropped immediately afterwards, excluding stale files.
                let output_dir = tempfile::tempdir().map_err(|error| GraphError::ReleaseInputRead {
                    path: cwd.clone(),
                    message: error.to_string(),
                })?;
                let output_path = output_dir.path().to_string_lossy();
                let built = self
                    .runner
                    .run("python", &["-m", "build", "--outdir", output_path.as_ref()], &cwd)?;
                if built.exit_code != Some(0) {
                    return Err(GraphError::ReleaseIntentStale);
                }
                let files = std::fs::read_dir(output_dir.path())
                    .map_err(|error| GraphError::ReleaseInputRead {
                        path: output_dir.path().to_path_buf(),
                        message: error.to_string(),
                    })?
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.is_file())
                    .collect::<Vec<_>>();
                if files.is_empty() {
                    return Err(GraphError::ReleaseIntentStale);
                }
                let mut args = vec!["upload", "--skip-existing"];
                if let Some(endpoint) = registry.endpoint.as_deref() {
                    args.extend(["--repository-url", endpoint]);
                }
                let files = files.iter().map(|path| path.to_string_lossy()).collect::<Vec<_>>();
                args.extend(files.iter().map(std::convert::AsRef::as_ref));
                self.runner.run("twine", &args, &cwd)?
            }
            _ => return Err(GraphError::ReleaseIntentStale),
        };
        let outcome = match id.package.ecosystem() {
            Ecosystem::Cargo => publish_client::classify_cargo_output(&output),
            Ecosystem::Npm => publish_client::classify_npm_publish_output(&output),
            Ecosystem::Pypi => publish_client::classify_twine_output(&output),
            _ => return Err(GraphError::ReleaseIntentStale),
        }
        .map_err(|_error| GraphError::ReleaseIntentStale)?;
        if matches!(id.package.ecosystem(), Ecosystem::Npm)
            && matches!(outcome, PublishOutcome::Published)
            && !self.npm_version_is_published(package_name, version, registry.endpoint.as_deref())?
        {
            // A successful client process is not a receipt. The operation
            // remains Attempting for reconciliation rather than asserting a
            // release the registry cannot yet prove exists.
            return Err(GraphError::ReleaseIntentStale);
        }
        Ok(match outcome {
            PublishOutcome::Published => OperationOutcome::Published,
            PublishOutcome::AlreadyPublished => OperationOutcome::AlreadySatisfied,
        })
    }

    fn npm_version_is_published(
        &self,
        package_name: &str,
        version: &Version,
        registry: Option<&str>,
    ) -> Result<bool, GraphError> {
        let spec = format!("{package_name}@{}", version.render());
        let mut args = vec!["view", spec.as_str(), "--json"];
        if let Some(registry) = registry {
            args.extend(["--registry", registry]);
        }
        let output = self
            .runner
            .run_quiet("npm", &args, &self.prepared.root, std::time::Duration::from_secs(300))?;
        if output.success() {
            return Ok(!output.stdout_trimmed().is_empty());
        }
        let details = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
        if details.contains("e404")
            || details.contains("etarget")
            || details.contains("no matching version")
            || details.contains("is not in this registry")
        {
            return Ok(false);
        }
        Err(GraphError::ReleaseIntentStale)
    }

    fn dispatch_tag(
        &self,
        name: &TagName,
        target: &CommitSha,
        annotation: &str,
    ) -> Result<OperationOutcome, GraphError> {
        if let Some(observed) = self.observed_tag(name)? {
            return if observed.target == *target && observed.annotation == annotation {
                Ok(OperationOutcome::AlreadySatisfied)
            } else {
                Err(GraphError::ReleaseIntentStale)
            };
        }
        let created = self.runner.run(
            "git",
            &[
                "tag",
                "--no-sign",
                "-a",
                name.as_str(),
                target.as_str(),
                "-m",
                annotation,
            ],
            &self.prepared.root,
        )?;
        if created.exit_code != Some(0) {
            return Err(GraphError::ReleaseIntentStale);
        }
        let pushed = self.runner.run(
            "git",
            &["push", self.checked_git_remote()?.endpoint.as_str(), name.as_str()],
            &self.prepared.root,
        )?;
        if pushed.exit_code != Some(0) {
            return Err(GraphError::ReleaseIntentStale);
        }
        if self
            .observed_tag(name)?
            .is_some_and(|observed| observed.target == *target && observed.annotation == annotation)
        {
            Ok(OperationOutcome::Published)
        } else {
            Err(GraphError::ReleaseIntentStale)
        }
    }

    fn dispatch_forge_release(&self, tag: &TagName) -> Result<OperationOutcome, GraphError> {
        let remote = self.checked_git_remote()?;
        let repository = remote
            .github_repository
            .as_deref()
            .ok_or(GraphError::ReleaseIntentStale)?;
        match self.observed_forge_release_target(tag, repository)? {
            ForgeReleaseObservation::Exact => return Ok(OperationOutcome::AlreadySatisfied),
            ForgeReleaseObservation::Conflict => return Err(GraphError::ReleaseIntentStale),
            ForgeReleaseObservation::Missing => {}
        }
        let created = self.runner.run(
            "gh",
            &[
                "release",
                "create",
                tag.as_str(),
                "--repo",
                repository,
                "--verify-tag",
                "--generate-notes",
            ],
            &self.prepared.root,
        )?;
        if created.exit_code == Some(0)
            && self.observed_forge_release_target(tag, repository)? == ForgeReleaseObservation::Exact
        {
            Ok(OperationOutcome::Published)
        } else {
            Err(GraphError::ReleaseIntentStale)
        }
    }

    fn observed_tag(&self, name: &TagName) -> Result<Option<ObservedTag>, GraphError> {
        let reference = format!("refs/tags/{name}^{{commit}}");
        let observed = self
            .runner
            .run("git", &["rev-parse", "--verify", &reference], &self.prepared.root)?;
        if observed.exit_code != Some(0) {
            return Ok(None);
        }
        let target = CommitSha::parse(observed.stdout.trim()).map_err(|_error| GraphError::ReleaseIntentStale)?;
        let details = self.runner.run(
            "git",
            &[
                "for-each-ref",
                "--format=%(objecttype)%00%(contents:subject)%00%(contents:body)",
                &format!("refs/tags/{name}"),
            ],
            &self.prepared.root,
        )?;
        if details.exit_code != Some(0) {
            return Err(GraphError::ReleaseIntentStale);
        }
        let line = details.stdout.trim_end_matches(['\r', '\n']);
        let mut fields = line.split('\0');
        let object_type = fields.next();
        let annotation = fields.next();
        let body = fields.next();
        if object_type != Some("tag") || body.is_none_or(|body| !body.trim().is_empty()) || fields.next().is_some() {
            return Err(GraphError::ReleaseIntentStale);
        }
        let annotation = annotation.ok_or(GraphError::ReleaseIntentStale)?;
        Ok(Some(ObservedTag {
            target,
            annotation: annotation.to_string(),
        }))
    }

    fn observed_forge_release_target(
        &self,
        tag: &TagName,
        repository: &str,
    ) -> Result<ForgeReleaseObservation, GraphError> {
        let observed = self.runner.run(
            "gh",
            &[
                "api",
                "--include",
                "--method",
                "GET",
                &format!("repos/{repository}/releases/tags/{tag}"),
                "--repo",
                repository,
            ],
            &self.prepared.root,
        )?;
        let (status, body) = parse_github_api_response(&observed.stdout)?;
        if status == 404 {
            return Ok(ForgeReleaseObservation::Missing);
        }
        if status != 200 || observed.exit_code != Some(0) {
            return Err(GraphError::ReleaseIntentStale);
        }
        let value: serde_json::Value = serde_json::from_str(body).map_err(|_error| GraphError::ReleaseIntentStale)?;
        let expected = match &self.prepared.source {
            SourceIdentity::GitCommit { sha } => sha.as_str(),
            SourceIdentity::HermeticContent { .. } => return Err(GraphError::ReleaseIntentStale),
        };
        Ok(
            if value.get("tag_name").and_then(serde_json::Value::as_str) == Some(tag.as_str())
                && value.get("target_commitish").and_then(serde_json::Value::as_str) == Some(expected)
            {
                ForgeReleaseObservation::Exact
            } else {
                ForgeReleaseObservation::Conflict
            },
        )
    }

    fn checked_git_remote(&self) -> Result<&PreparedGitRemote, GraphError> {
        let expected = self
            .prepared
            .git_remote
            .as_ref()
            .ok_or(GraphError::ReleaseIntentStale)?;
        if prepared_git_remote(&self.prepared.root, self.runner)? != *expected {
            return Err(GraphError::ReleaseIntentStale);
        }
        Ok(expected)
    }
}

fn parse_github_api_response(stdout: &str) -> Result<(u16, &str), GraphError> {
    let (headers, body) = stdout
        .rsplit_once("\r\n\r\n")
        .or_else(|| stdout.rsplit_once("\n\n"))
        .ok_or(GraphError::ReleaseIntentStale)?;
    let status_line = headers
        .lines()
        .rev()
        .find(|line| line.trim_end_matches('\r').starts_with("HTTP/"))
        .ok_or(GraphError::ReleaseIntentStale)?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or(GraphError::ReleaseIntentStale)?
        .parse::<u16>()
        .map_err(|_error| GraphError::ReleaseIntentStale)?;
    Ok((status, body))
}

fn derive_release_intent<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    decision: &ReleaseDecisionV1,
    source: SourceIdentity,
    trust_profile: ExecutionTrustProfileV1,
) -> Result<ReleaseIntentV1, GraphError> {
    let (snapshot, operations, _, _) = derive_release_inputs(workspace, decision, source)?;
    ReleaseIntentV1::new(decision.clone(), snapshot, trust_profile, operations, vec![])
        .map_err(|_error| GraphError::ReleaseIntentStale)
}

fn derive_release_intent_with_prepared<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    decision: &ReleaseDecisionV1,
    source: SourceIdentity,
    trust_profile: ExecutionTrustProfileV1,
) -> Result<(ReleaseIntentV1, PreparedDerivation), GraphError> {
    let (snapshot, operations, prepared, git_remote) = derive_release_inputs(workspace, decision, source)?;
    let intent = ReleaseIntentV1::new(decision.clone(), snapshot, trust_profile, operations, vec![])
        .map_err(|_error| GraphError::ReleaseIntentStale)?;
    Ok((
        intent,
        PreparedDerivation {
            operations: prepared,
            git_remote,
        },
    ))
}

fn derive_release_inputs<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    decision: &ReleaseDecisionV1,
    source: SourceIdentity,
) -> Result<DerivedReleaseInputs, GraphError> {
    let mut package_inputs = Vec::new();

    let mut selected = BTreeMap::new();
    let mut package_ids = BTreeMap::<callisto_model::PackageId, Vec<ReleasePackageId>>::new();
    for package in workspace.graph.packages() {
        for ecosystem in package
            .canonical_manifests()
            .map(|manifest| manifest.ecosystem())
            .collect::<BTreeSet<_>>()
        {
            let id =
                ReleasePackageId::new(ecosystem, package.id.name()).map_err(|_error| GraphError::ReleaseIntentStale)?;
            if let Some(entry) = decision.entries.iter().find(|entry| entry.package == id) {
                selected.insert(id.clone(), (package, entry.target_version.clone()));
                package_ids.entry(package.id.clone()).or_default().push(id);
            }
        }
    }
    for entry in &decision.entries {
        let id = &entry.package;
        if !selected.contains_key(id) {
            return Err(GraphError::ReleasePackageNotSelected { package: id.clone() });
        }
    }

    let requires_git_remote = selected.values().any(|(package, _)| {
        package
            .publish_to
            .iter()
            .any(|target| !matches!(target, PublishTarget::None))
    });
    let git_remote = requires_git_remote
        .then(|| prepared_git_remote(&workspace.root, workspace.runner))
        .transpose()?;

    let mut operations = BTreeMap::<ReleaseOperationId, ReleaseOperation>::new();
    let mut prepared = BTreeMap::<ReleaseOperationId, PreparedOperation>::new();
    let mut publishes_by_package = BTreeMap::<ReleasePackageId, Vec<ReleaseOperationId>>::new();
    let mut tag_by_package = BTreeMap::<ReleasePackageId, ReleaseOperationId>::new();

    // First construct leaves so dependency prerequisites can refer only to
    // exact selected release identities, never PackageId's wildcard matcher.
    for (id, (package, version)) in &selected {
        let produces_tag = package
            .publish_to
            .iter()
            .any(|target| !matches!(target, PublishTarget::None));
        let fingerprint = package_fingerprint(
            workspace,
            id,
            package,
            version,
            produces_tag.then_some(git_remote.as_ref()).flatten(),
        )?;
        package_inputs.push(ReleasePackageInputV1 {
            package: id.clone(),
            fingerprint,
        });

        let mut publishes = Vec::new();
        for target in &package.publish_to {
            if target.ecosystem() == Some(id.ecosystem()) {
                let registry_key = target.registry_key().expect("registry target has registry key");
                let binding = prepared_registry_binding(workspace, target)?;
                let operation = ReleaseOperation::registry_publish(
                    id.clone(),
                    version.clone(),
                    RegistryBindingId::new(registry_key.as_str(), binding.identity.clone())
                        .map_err(|_error| GraphError::ReleaseIntentStale)?,
                    Vec::new(),
                )
                .map_err(|_error| GraphError::ReleaseIntentStale)?;
                // A durable registry operation must have one exact endpoint
                // binding. The model-level operation identity currently has
                // only a registry key, so reject a second target that would
                // collapse to the same operation until the typed binding
                // identity is added there.
                if prepared.contains_key(operation.id()) {
                    return Err(GraphError::ReleaseIntentStale);
                }
                prepared.insert(
                    operation.id().clone(),
                    PreparedOperation::RegistryPublish {
                        package_dir: package_dir(package)?,
                        package_name: id.name().to_string(),
                        version: version.clone(),
                        registry: binding,
                        npm_access: match target {
                            PublishTarget::Npm { access, .. } => *access,
                            _ => None,
                        },
                    },
                );
                publishes.push(operation.id().clone());
                operations.insert(operation.id().clone(), operation);
            }
        }
        publishes.sort();
        publishes_by_package.insert(id.clone(), publishes);
    }

    // Replace each publish leaf with prerequisites from selected dependency
    // packages. This preserves the existing publish semantics: runtime,
    // build, optional, and dev dependencies must exist before publish.
    for (id, (package, _)) in &selected {
        let mut prerequisites = BTreeSet::new();
        for edge in workspace.graph.dependencies_of(&package.id) {
            if !matches!(
                edge.kind,
                DepKind::Runtime | DepKind::Build | DepKind::Optional | DepKind::Dev
            ) {
                continue;
            }
            for dependency_release_id in package_ids.get(&edge.to).into_iter().flatten() {
                prerequisites.extend(
                    publishes_by_package
                        .get(dependency_release_id)
                        .into_iter()
                        .flatten()
                        .cloned(),
                );
            }
        }
        for publish_id in publishes_by_package.get(id).into_iter().flatten() {
            let operation = operations.get(publish_id).expect("publish operation was constructed");
            let replacement = ReleaseOperation::new(operation.id().clone(), prerequisites.iter().cloned().collect())
                .map_err(|_error| GraphError::ReleaseIntentStale)?;
            operations.insert(publish_id.clone(), replacement);
        }
    }

    for (id, (package, version)) in &selected {
        if !package
            .publish_to
            .iter()
            .any(|target| !matches!(target, PublishTarget::None))
        {
            continue;
        }
        let tag = ReleaseOperation::tag(
            id.clone(),
            version.clone(),
            publishes_by_package.get(id).cloned().unwrap_or_default(),
        )
        .map_err(|_error| GraphError::ReleaseIntentStale)?;
        let tag_name = package
            .tag_template
            .clone()
            .unwrap_or_else(|| callisto_model::TagTemplate::default_for(&package.id))
            .render(version);
        let target = match &source {
            SourceIdentity::GitCommit { sha } => sha.clone(),
            SourceIdentity::HermeticContent { .. } => return Err(GraphError::ReleaseIntentStale),
        };
        prepared.insert(
            tag.id().clone(),
            PreparedOperation::Tag {
                annotation: format!("Release {tag_name}"),
                name: tag_name,
                target,
            },
        );
        tag_by_package.insert(id.clone(), tag.id().clone());
        operations.insert(tag.id().clone(), tag);
    }
    for (id, (package, version)) in &selected {
        if package
            .publish_to
            .iter()
            .any(|target| matches!(target, PublishTarget::GitHubRelease))
        {
            let tag = tag_by_package.get(id).expect("release point has tag").clone();
            let operation = ReleaseOperation::forge_release(id.clone(), version.clone(), vec![tag])
                .map_err(|_error| GraphError::ReleaseIntentStale)?;
            let tag = match prepared.get(operation.prerequisites().first().expect("forge release has tag")) {
                Some(PreparedOperation::Tag { name, .. }) => name.clone(),
                _ => return Err(GraphError::ReleaseIntentStale),
            };
            prepared.insert(operation.id().clone(), PreparedOperation::ForgeRelease { tag });
            operations.insert(operation.id().clone(), operation);
        }
    }

    Ok((
        ReleaseInputSnapshotV1::new(source, package_inputs).map_err(|_error| GraphError::ReleaseIntentStale)?,
        canonical_operation_order(operations)?,
        prepared,
        git_remote,
    ))
}

fn package_fingerprint<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    id: &ReleasePackageId,
    package: &callisto_model::Package,
    version: &Version,
    git_remote: Option<&PreparedGitRemote>,
) -> Result<SemanticInputDigest, GraphError> {
    let mut transcript = CanonicalTranscript::semantic_input_v1();
    transcript.push_str("package.id", &id.to_string());
    transcript.push_str("package.version", version.render());
    for manifest in &package.manifests {
        let path = workspace.root.join(&manifest.path);
        let bytes = std::fs::read(&path).map_err(|error| GraphError::ReleaseInputRead {
            path: manifest.path.clone(),
            message: error.to_string(),
        })?;
        transcript.push_str("manifest.path", &manifest.path.display().to_string());
        transcript.push_str("manifest.bytes", digest_bytes("manifest", &bytes).as_str());
        transcript.push_str("manifest.role", &manifest_role_text(&manifest.role));
        transcript.push_str("manifest.format", manifest.format.ecosystem().prefix());
    }
    transcript.push_str("package.trigger", release_trigger_text(package.release_trigger));
    transcript.push_str(
        "package.tag",
        &package
            .tag_template
            .as_ref()
            .map_or_else(|| "default".to_string(), |tag| tag.as_str()),
    );
    for target in &package.publish_to {
        transcript.push_str("package.target", target_fingerprint(workspace, target)?.as_str());
    }
    if let Some(remote) = git_remote {
        transcript.push_str("package.gitRemote", remote.identity.as_str());
    }
    Ok(SemanticInputDigest::from_transcript(&transcript))
}

fn package_dir(package: &callisto_model::Package) -> Result<std::path::PathBuf, GraphError> {
    package
        .canonical_manifests()
        .next()
        .and_then(|manifest| manifest.path.parent())
        .map(std::path::Path::to_path_buf)
        .ok_or(GraphError::ReleaseIntentStale)
}

fn manifest_role_text(role: &callisto_model::ManifestRole) -> String {
    match role {
        callisto_model::ManifestRole::Canonical => "canonical".to_string(),
        callisto_model::ManifestRole::Lockfile => "lockfile".to_string(),
        callisto_model::ManifestRole::Platform { platform, arch, abi } => {
            format!("platform:{platform}:{arch}:{}", abi.as_deref().unwrap_or(""))
        }
    }
}

fn release_trigger_text(trigger: callisto_model::ReleaseTrigger) -> &'static str {
    match trigger {
        callisto_model::ReleaseTrigger::Changeset => "changeset",
        callisto_model::ReleaseTrigger::Auto => "auto",
    }
}

fn canonical_operation_order(
    operations: BTreeMap<ReleaseOperationId, ReleaseOperation>,
) -> Result<Vec<ReleaseOperation>, GraphError> {
    let mut remaining: BTreeMap<_, usize> = operations
        .iter()
        .map(|(id, operation)| (id.clone(), operation.prerequisites().len()))
        .collect();
    let mut dependents = BTreeMap::<ReleaseOperationId, Vec<ReleaseOperationId>>::new();
    for (id, operation) in &operations {
        for prerequisite in operation.prerequisites() {
            dependents.entry(prerequisite.clone()).or_default().push(id.clone());
        }
    }
    let mut ready: BTreeSet<_> = remaining
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(id.clone()))
        .collect();
    let mut ordered = Vec::with_capacity(operations.len());
    while let Some(id) = ready.pop_first() {
        ordered.push(operations.get(&id).expect("known operation").clone());
        for dependent in dependents.get(&id).into_iter().flatten() {
            let count = remaining.get_mut(dependent).expect("known operation");
            *count -= 1;
            if *count == 0 {
                ready.insert(dependent.clone());
            }
        }
    }
    if ordered.len() != operations.len() {
        return Err(GraphError::ReleaseIntentStale);
    }
    Ok(ordered)
}

fn digest_bytes(tag: &str, bytes: &[u8]) -> SemanticInputDigest {
    let mut transcript = CanonicalTranscript::semantic_input_v1();
    transcript.push_bytes(tag, bytes);
    SemanticInputDigest::from_transcript(&transcript)
}

/// Credential-free URL projection used only inside a digest transcript.
#[derive(Debug, PartialEq, Eq)]
struct RegistryBindingV1 {
    scheme: String,
    host: String,
    effective_port: Option<u16>,
    path: String,
}

impl RegistryBindingV1 {
    fn digest(&self) -> RegistryBindingDigest {
        let mut transcript = CanonicalTranscript::semantic_input_v1();
        transcript.push_str("registry.scheme", &self.scheme);
        transcript.push_str("registry.host", &self.host);
        transcript.push_str(
            "registry.port",
            &self.effective_port.map_or_else(String::new, |port| port.to_string()),
        );
        transcript.push_str("registry.path", &self.path);
        RegistryBindingDigest::from_normalized_binding(transcript.as_bytes())
    }

    fn endpoint(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let port = match (&self.scheme[..], self.effective_port) {
            ("https", Some(443)) | ("http", Some(80)) | (_, None) => String::new(),
            (_, Some(port)) => format!(":{port}"),
        };
        format!("{}://{host}{port}{}", self.scheme, self.path)
    }
}

fn target_fingerprint<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    target: &PublishTarget,
) -> Result<SemanticInputDigest, GraphError> {
    let mut transcript = CanonicalTranscript::semantic_input_v1();
    match target {
        PublishTarget::CratesIo => transcript.push_str("target.kind", "crates-io"),
        PublishTarget::Npm { registry: _, access } => {
            transcript.push_str("target.kind", "npm");
            push_registry_binding(&mut transcript, prepared_registry_binding(workspace, target)?.identity)?;
            transcript.push_str(
                "target.access",
                match access {
                    Some(callisto_model::NpmAccess::Public) => "public",
                    Some(callisto_model::NpmAccess::Restricted) => "restricted",
                    None => "default",
                },
            );
        }
        PublishTarget::Pypi { index: _ } => {
            transcript.push_str("target.kind", "pypi");
            push_registry_binding(&mut transcript, prepared_registry_binding(workspace, target)?.identity)?;
        }
        PublishTarget::NuGet { source: _ } => {
            transcript.push_str("target.kind", "nuget");
            push_registry_binding(&mut transcript, prepared_registry_binding(workspace, target)?.identity)?;
        }
        PublishTarget::GitHubRelease => transcript.push_str("target.kind", "github-release"),
        PublishTarget::None => transcript.push_str("target.kind", "none"),
        #[allow(unreachable_patterns)]
        _ => return Err(GraphError::ReleaseIntentStale),
    }
    Ok(SemanticInputDigest::from_transcript(&transcript))
}

fn push_registry_binding(
    transcript: &mut CanonicalTranscript,
    fingerprint: RegistryBindingDigest,
) -> Result<(), GraphError> {
    transcript.push_str("target.registry", fingerprint.as_str());
    Ok(())
}

fn prepared_registry_binding<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    target: &PublishTarget,
) -> Result<PreparedRegistryBinding, GraphError> {
    let key = target.registry_key().ok_or(GraphError::ReleaseIntentStale)?;
    let explicit = match target {
        PublishTarget::Npm { registry, .. } => registry.as_deref(),
        PublishTarget::Pypi { index } => index.as_deref(),
        PublishTarget::NuGet { source } => source.as_deref(),
        PublishTarget::CratesIo | PublishTarget::GitHubRelease | PublishTarget::None => None,
        #[allow(unreachable_patterns)]
        _ => return Err(GraphError::ReleaseIntentStale),
    };
    let configured = workspace
        .config
        .registries
        .get(&key)
        .and_then(|registry| registry.url.as_deref());
    let binding = match explicit.or(configured) {
        Some(raw) => canonical_registry_binding(key.as_str(), raw)?,
        None => {
            return Ok(PreparedRegistryBinding {
                key: key.clone(),
                endpoint: None,
                identity: RegistryBindingDigest::from_normalized_binding(key.as_str().as_bytes()),
            })
        }
    };
    Ok(PreparedRegistryBinding {
        key,
        endpoint: Some(binding.endpoint()),
        identity: binding.digest(),
    })
}

fn canonical_registry_binding(registry: &str, raw: &str) -> Result<RegistryBindingV1, GraphError> {
    let parsed = url::Url::parse(raw).map_err(|_error| GraphError::UnsafeRegistryBinding {
        registry: registry.to_string(),
        reason: "invalid URL",
    })?;
    if parsed.cannot_be_a_base() || parsed.host_str().is_none() {
        return Err(GraphError::UnsafeRegistryBinding {
            registry: registry.to_string(),
            reason: "URL must have an authority",
        });
    }
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(GraphError::UnsafeRegistryBinding {
            registry: registry.to_string(),
            reason: "URL scheme must be http or https",
        });
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(GraphError::UnsafeRegistryBinding {
            registry: registry.to_string(),
            reason: "userinfo is forbidden",
        });
    }
    if parsed.query().is_some() {
        return Err(GraphError::UnsafeRegistryBinding {
            registry: registry.to_string(),
            reason: "query string is forbidden",
        });
    }
    if parsed.fragment().is_some() {
        return Err(GraphError::UnsafeRegistryBinding {
            registry: registry.to_string(),
            reason: "fragment is forbidden",
        });
    }
    Ok(RegistryBindingV1 {
        scheme: parsed.scheme().to_ascii_lowercase(),
        host: parsed.host_str().expect("validated authority").to_ascii_lowercase(),
        effective_port: parsed.port_or_known_default(),
        path: parsed.path().to_string(),
    })
}

fn prepared_git_remote(root: &Path, runner: &dyn CommandRunner) -> Result<PreparedGitRemote, GraphError> {
    let output = runner.run("git", &["remote", "get-url", "--push", "origin"], root)?;
    if output.exit_code != Some(0) {
        return Err(GraphError::UnsafeGitRemote {
            reason: "origin must have a push URL",
        });
    }
    canonical_git_remote(output.stdout.trim())
}

fn canonical_git_remote(raw: &str) -> Result<PreparedGitRemote, GraphError> {
    let ssh_url = if raw.starts_with("git@") && !raw.contains("//") {
        let (host, path) =
            raw.strip_prefix("git@")
                .and_then(|value| value.split_once(':'))
                .ok_or(GraphError::UnsafeGitRemote {
                    reason: "invalid SSH remote syntax",
                })?;
        format!("ssh://git@{host}/{path}")
    } else {
        raw.to_string()
    };
    let parsed = url::Url::parse(&ssh_url).map_err(|_error| GraphError::UnsafeGitRemote { reason: "invalid URL" })?;
    if parsed.cannot_be_a_base() || parsed.host_str().is_none() {
        return Err(GraphError::UnsafeGitRemote {
            reason: "URL must have an authority",
        });
    }
    if !matches!(parsed.scheme(), "https" | "ssh") {
        return Err(GraphError::UnsafeGitRemote {
            reason: "URL scheme must be HTTPS or SSH",
        });
    }
    if parsed.password().is_some() || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(GraphError::UnsafeGitRemote {
            reason: "credentials, query strings, and fragments are forbidden",
        });
    }
    if parsed.scheme() == "https" && !parsed.username().is_empty() {
        return Err(GraphError::UnsafeGitRemote {
            reason: "HTTPS userinfo is forbidden",
        });
    }
    if parsed.scheme() == "ssh" && parsed.username() != "git" {
        return Err(GraphError::UnsafeGitRemote {
            reason: "SSH remotes must use the git account",
        });
    }
    let host = parsed.host_str().expect("validated authority").to_ascii_lowercase();
    let mut components = parsed.path_segments().ok_or(GraphError::UnsafeGitRemote {
        reason: "remote path is invalid",
    })?;
    let owner = components
        .next()
        .filter(|value| !value.is_empty())
        .ok_or(GraphError::UnsafeGitRemote {
            reason: "remote path must name an owner and repository",
        })?;
    let repository = components
        .next()
        .filter(|value| !value.is_empty())
        .ok_or(GraphError::UnsafeGitRemote {
            reason: "remote path must name an owner and repository",
        })?;
    if components.next().is_some() || owner == "." || owner == ".." || repository == "." || repository == ".." {
        return Err(GraphError::UnsafeGitRemote {
            reason: "remote path must contain exactly an owner and repository",
        });
    }
    let repository = repository.strip_suffix(".git").unwrap_or(repository);
    if repository.is_empty() {
        return Err(GraphError::UnsafeGitRemote {
            reason: "repository name is empty",
        });
    }
    let port = parsed.port_or_known_default();
    let port_text = match (parsed.scheme(), port) {
        ("https", Some(443)) | ("ssh", Some(22)) | (_, None) => String::new(),
        (_, Some(port)) => format!(":{port}"),
    };
    let endpoint = match parsed.scheme() {
        "https" => format!("https://{host}{port_text}/{owner}/{repository}.git"),
        "ssh" => format!("ssh://git@{host}{port_text}/{owner}/{repository}.git"),
        _ => unreachable!("scheme checked above"),
    };
    let mut transcript = CanonicalTranscript::semantic_input_v1();
    transcript.push_str("gitRemote.scheme", parsed.scheme());
    transcript.push_str("gitRemote.host", &host);
    transcript.push_str(
        "gitRemote.port",
        &port.map_or_else(String::new, |port| port.to_string()),
    );
    transcript.push_str("gitRemote.owner", owner);
    transcript.push_str("gitRemote.repository", repository);
    Ok(PreparedGitRemote {
        endpoint,
        identity: SemanticInputDigest::from_transcript(&transcript),
        github_repository: (host == "github.com").then(|| format!("{owner}/{repository}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::{CommandError, CommandOutput, Ecosystem, VersionGrammar};

    struct RealGitRunner;
    impl CommandRunner for RealGitRunner {
        fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
            let output = std::process::Command::new(program)
                .args(args)
                .current_dir(cwd)
                .output()
                .map_err(|error| CommandError::Io {
                    program: program.to_string(),
                    message: error.to_string(),
                })?;
            Ok(CommandOutput {
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
    }
    fn fixture() -> (tempfile::TempDir, RealGitRunner) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"release-fixture\"\nversion = \"1.2.3\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("callisto.toml"),
            "[[package]]\nmatch = \"release-fixture\"\npublish-to = [\"crates-io\"]\n",
        )
        .unwrap();
        for args in [
            ["init", "-q"].as_slice(),
            ["config", "user.email", "test@example.com"].as_slice(),
            ["config", "user.name", "Test"].as_slice(),
            ["config", "commit.gpgsign", "false"].as_slice(),
            [
                "remote",
                "add",
                "origin",
                "https://github.com/example/release-fixture.git",
            ]
            .as_slice(),
            ["add", "."].as_slice(),
            ["commit", "-q", "-m", "fixture"].as_slice(),
            ["checkout", "--detach", "-q", "HEAD"].as_slice(),
        ] {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success());
        }
        (dir, RealGitRunner)
    }
    fn decision() -> ReleaseDecisionV1 {
        ReleaseDecisionV1::new(vec![callisto_model::ReleaseDecisionEntry {
            package: ReleasePackageId::new(Ecosystem::Cargo, "release-fixture").unwrap(),
            target_version: Version::parse("1.2.3", VersionGrammar::SemVer).unwrap(),
            reasons: vec![callisto_model::ReleaseInclusionReason::ExplicitSelection],
        }])
        .unwrap()
    }
    #[test]
    fn fresh_validation_rejects_manifest_change() {
        let (dir, runner) = fixture();
        let state_dir = tempfile::tempdir().unwrap();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let intent = build_release_intent(
            dir.path(),
            &locator,
            &runner,
            &decision(),
            ExecutionTrustProfileV1::GitCommit,
        )
        .unwrap();
        validate_release_intent_with_state_directory(
            dir.path(),
            &locator,
            &runner,
            Some(state_dir.path()),
            intent.clone(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"release-fixture\"\nversion = \"1.2.4\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let error =
            validate_release_intent_with_state_directory(dir.path(), &locator, &runner, Some(state_dir.path()), intent)
                .expect_err("a dirty checkout cannot produce Git commit trust evidence");
        assert!(matches!(error, GraphError::Vcs(_)));
    }
    #[test]
    fn registry_binding_rejects_credentials_and_ambiguous_routing() {
        for value in [
            "https://token@registry.example.test/index",
            "https://registry.example.test/index?token=secret",
            "https://registry.example.test/index#fragment",
        ] {
            assert!(
                canonical_registry_binding("test", value).is_err(),
                "{value} must be rejected"
            );
        }
    }

    #[test]
    fn registry_binding_normalizes_host_and_default_port_without_retaining_url() {
        let explicit = canonical_registry_binding("test", "HTTPS://Registry.Example.Test:443/a/../index").unwrap();
        let implicit = canonical_registry_binding("test", "https://registry.example.test/index").unwrap();
        assert_eq!(explicit, implicit);
        assert_eq!(explicit.host, "registry.example.test");
        assert_eq!(explicit.effective_port, Some(443));
        assert_eq!(explicit.path, "/index");
    }

    #[test]
    fn git_remote_binding_normalizes_credential_free_ssh_and_rejects_credentialed_https() {
        let ssh = canonical_git_remote("git@GitHub.com:example/release-fixture.git").unwrap();
        assert_eq!(ssh.endpoint, "ssh://git@github.com/example/release-fixture.git");
        assert_eq!(ssh.github_repository.as_deref(), Some("example/release-fixture"));
        assert!(canonical_git_remote("https://token@github.com/example/release-fixture.git").is_err());
    }

    #[test]
    fn github_api_observation_parser_requires_an_explicit_http_status() {
        let (status, body) = parse_github_api_response(
            "HTTP/2 404\r\ncontent-type: application/json\r\n\r\n{\"message\":\"Not Found\"}",
        )
        .unwrap();
        assert_eq!(status, 404);
        assert_eq!(body, "{\"message\":\"Not Found\"}");
        assert!(parse_github_api_response("not a response").is_err());
    }

    #[test]
    fn prepared_capability_retains_exact_tag_and_registry_inputs() {
        let (dir, runner) = fixture();
        let state_dir = tempfile::tempdir().unwrap();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let intent = build_release_intent(
            dir.path(),
            &locator,
            &runner,
            &decision(),
            ExecutionTrustProfileV1::GitCommit,
        )
        .unwrap();
        let validated =
            validate_release_intent_with_state_directory(dir.path(), &locator, &runner, Some(state_dir.path()), intent)
                .unwrap();
        let inputs = validated.prepared();
        assert!(inputs.root.is_absolute());
        assert!(matches!(&inputs.source, SourceIdentity::GitCommit { .. }));

        let tag = inputs
            .operations
            .values()
            .find_map(|operation| match operation {
                PreparedOperation::Tag {
                    name,
                    target,
                    annotation,
                } => Some((name, target, annotation)),
                _ => None,
            })
            .expect("the tag operation must retain its render, target, and annotation policy");
        assert_eq!(tag.0.as_str(), "release-fixture@1.2.3");
        assert_eq!(tag.2, "Release release-fixture@1.2.3");
        assert_eq!(tag.1.as_str().len(), 40);

        let registry = inputs
            .operations
            .values()
            .find_map(|operation| match operation {
                PreparedOperation::RegistryPublish {
                    package_dir,
                    package_name,
                    version,
                    registry,
                    npm_access,
                } => Some((package_dir, package_name, version, registry, npm_access)),
                _ => None,
            })
            .expect("the publish operation must retain its exact routing input");
        assert_eq!(registry.0, &std::path::PathBuf::new());
        assert_eq!(registry.1, "release-fixture");
        assert_eq!(registry.2.render(), "1.2.3");
        assert_eq!(registry.3.key.as_str(), "cratesIo");
        assert!(registry.3.endpoint.is_none());
        assert!(registry.4.is_none());
    }

    #[test]
    fn prepared_tag_adapter_accepts_only_the_exact_intended_target() {
        let (dir, runner) = fixture();
        let state_dir = tempfile::tempdir().unwrap();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let intent = build_release_intent(
            dir.path(),
            &locator,
            &runner,
            &decision(),
            ExecutionTrustProfileV1::GitCommit,
        )
        .unwrap();
        let validated =
            validate_release_intent_with_state_directory(dir.path(), &locator, &runner, Some(state_dir.path()), intent)
                .unwrap();
        let tag_id = validated
            .intent()
            .operations
            .iter()
            .find(|operation| matches!(operation.id().role, callisto_model::ReleaseOperationRole::Tag))
            .expect("tag operation")
            .id()
            .clone();
        let tag_name = "release-fixture@1.2.3";
        assert!(std::process::Command::new("git")
            .args([
                "-c",
                "tag.gpgSign=false",
                "tag",
                "-a",
                tag_name,
                "HEAD",
                "-m",
                "Release release-fixture@1.2.3"
            ])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
        assert_eq!(
            validated
                .dispatch_prepared(&callisto_model::ApplyPermit::force_for_tests(), &tag_id)
                .unwrap(),
            OperationOutcome::AlreadySatisfied
        );
    }

    #[test]
    fn prepared_tag_adapter_rejects_conflicting_existing_target() {
        let (dir, runner) = fixture();
        let state_dir = tempfile::tempdir().unwrap();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let intent = build_release_intent(
            dir.path(),
            &locator,
            &runner,
            &decision(),
            ExecutionTrustProfileV1::GitCommit,
        )
        .unwrap();
        let validated =
            validate_release_intent_with_state_directory(dir.path(), &locator, &runner, Some(state_dir.path()), intent)
                .unwrap();
        let tag_id = validated
            .intent()
            .operations
            .iter()
            .find(|operation| matches!(operation.id().role, callisto_model::ReleaseOperationRole::Tag))
            .expect("tag operation")
            .id()
            .clone();
        assert!(std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-qm", "other"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .args([
                "-c",
                "tag.gpgSign=false",
                "tag",
                "-a",
                "release-fixture@1.2.3",
                "HEAD",
                "-m",
                "wrong",
            ])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
        assert!(matches!(
            validated.dispatch_prepared(&callisto_model::ApplyPermit::force_for_tests(), &tag_id),
            Err(GraphError::ReleaseIntentStale)
        ));
    }

    #[test]
    fn comments_in_callisto_toml_do_not_change_semantic_release_inputs() {
        let (dir, runner) = fixture();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let root = canonical_root(dir.path()).unwrap();
        let workspace = Workspace::load(root.clone(), &locator, &runner).unwrap();
        let source = observe_source(&workspace, ExecutionTrustProfileV1::GitCommit).unwrap();
        let (before_snapshot, before_operations, _, _) =
            derive_release_inputs(&workspace, &decision(), source.clone()).unwrap();

        std::fs::write(
            root.join("callisto.toml"),
            "# formatting/comments are not release policy\n[[package]]\nmatch = \"release-fixture\"\npublish-to = [\"crates-io\"]\n",
        )
        .unwrap();
        let reread = Workspace::load(root, &locator, &runner).unwrap();
        let (after_snapshot, after_operations, _, _) = derive_release_inputs(&reread, &decision(), source).unwrap();

        assert_eq!(before_snapshot, after_snapshot);
        assert_eq!(before_operations, after_operations);
    }
}
