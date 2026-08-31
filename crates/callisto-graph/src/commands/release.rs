//! Fresh, graph-owned authorization for durable release intents.
//!
//! This module intentionally takes a root, locator, and runner rather than a
//! [`Workspace`]. A workspace caches parsed manifests and config for a normal
//! command invocation; accepting one here would make a previously observed
//! graph look current after the filesystem changed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use callisto_model::{
    CanonicalTranscript, CommandRunner, DepKind, ExecutionTrustProfileV1, PublishTarget, ReleaseInputComponentV1,
    ReleaseInputSnapshotV1, ReleaseIntentV1, ReleaseOperation, ReleaseOperationId, ReleasePackageId,
    SemanticInputDigest, SourceIdentity, Version,
};
use callisto_vcs::access::GitHeadDisposition;

use crate::{DependencyResolver, GraphError, ProjectLocator, Workspace};

/// Exact package/version values selected by a preceding release decision.
///
/// Selection is data, not a planner seam. Authorization never calls
/// `plan_version` or `plan_publish` and therefore cannot silently replace an
/// approved decision with a newly computed one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseSelection {
    versions: BTreeMap<ReleasePackageId, Version>,
}

impl ReleaseSelection {
    pub fn new(versions: BTreeMap<ReleasePackageId, Version>) -> Self {
        Self { versions }
    }

    pub fn versions(&self) -> &BTreeMap<ReleasePackageId, Version> {
        &self.versions
    }
}

/// Graph-private inputs prepared from the same fresh observation as an intent.
/// The executor will consume this when introduced; keeping it with the
/// capability prevents a validated public intent being paired with new inputs.
#[allow(dead_code)] // executor batch consumes these private prepared values
#[derive(Debug)]
struct PreparedReleaseInputs {
    root: std::path::PathBuf,
    source: SourceIdentity,
    operation_ids: BTreeSet<ReleaseOperationId>,
}

/// In-memory, non-transferable proof that an intent was rebuilt from a fresh
/// workspace observation. It is intentionally neither cloneable nor serializable.
#[derive(Debug)]
pub struct ValidatedReleaseIntent {
    intent: ReleaseIntentV1,
    prepared: PreparedReleaseInputs,
}

impl ValidatedReleaseIntent {
    pub fn intent(&self) -> &ReleaseIntentV1 {
        &self.intent
    }

    #[allow(dead_code)] // consumed by the durable executor batch
    fn prepared(&self) -> &PreparedReleaseInputs {
        &self.prepared
    }
}

/// Builds a release intent from a fresh root-bound observation.
pub fn build_release_intent<L: ProjectLocator, R: CommandRunner>(
    root: &Path,
    locator: &L,
    runner: &R,
    selection: &ReleaseSelection,
    trust_profile: ExecutionTrustProfileV1,
) -> Result<ReleaseIntentV1, GraphError> {
    let root = canonical_root(root)?;
    let workspace = Workspace::load(root.clone(), locator, runner)?;
    let source = observe_source(&workspace, trust_profile)?;
    let intent = derive_release_intent(&workspace, selection, source.clone(), trust_profile)?;

    // Recheck after all input reads. A concurrent edit or checkout cannot be
    // authorized merely because it happened after the first check.
    if observe_source(&workspace, trust_profile)? != source {
        return Err(GraphError::ReleaseIntentStale);
    }
    Ok(intent)
}

/// Re-observes root, config, package discovery, manifests, Git evidence, and
/// the exact operation DAG before creating an opaque authorization capability.
pub fn validate_release_intent<L: ProjectLocator, R: CommandRunner>(
    root: &Path,
    locator: &L,
    runner: &R,
    selection: &ReleaseSelection,
    received: ReleaseIntentV1,
) -> Result<ValidatedReleaseIntent, GraphError> {
    let root = canonical_root(root)?;
    let workspace = Workspace::load(root.clone(), locator, runner)?;
    let source = observe_source(&workspace, received.trust_profile)?;
    let expected = derive_release_intent(&workspace, selection, source.clone(), received.trust_profile)?;
    if expected != received || observe_source(&workspace, received.trust_profile)? != source {
        return Err(GraphError::ReleaseIntentStale);
    }

    Ok(ValidatedReleaseIntent {
        prepared: PreparedReleaseInputs {
            root,
            source,
            operation_ids: received
                .operations
                .iter()
                .map(|operation| operation.id().clone())
                .collect(),
        },
        intent: received,
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
    match trust_profile {
        ExecutionTrustProfileV1::GitCommit => {
            let evidence = workspace.git_access().observe_git_commit_trust()?;
            if evidence.canonical_root() != workspace.root
                || evidence.head_disposition() != GitHeadDisposition::Detached
            {
                return Err(GraphError::ReleaseIntentStale);
            }
            Ok(SourceIdentity::GitCommit {
                sha: evidence.head().clone(),
            })
        }
        // No closed-root observer exists yet. Reject rather than claim Git
        // cleanliness proves a hermetic content identity.
        ExecutionTrustProfileV1::HermeticContent => Err(GraphError::ReleaseIntentStale),
    }
}

fn derive_release_intent<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    selection: &ReleaseSelection,
    source: SourceIdentity,
    trust_profile: ExecutionTrustProfileV1,
) -> Result<ReleaseIntentV1, GraphError> {
    let (snapshot, operations) = derive_release_inputs(workspace, selection, source)?;
    ReleaseIntentV1::new(snapshot, trust_profile, operations).map_err(|_error| GraphError::ReleaseIntentStale)
}

fn derive_release_inputs<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    selection: &ReleaseSelection,
    source: SourceIdentity,
) -> Result<(ReleaseInputSnapshotV1, Vec<ReleaseOperation>), GraphError> {
    let mut components = Vec::new();
    let mut kinds = BTreeSet::new();

    // Raw config participates as bytes, not Debug formatting of internal
    // structs. The parser/resolver is still rebuilt above; this component
    // ensures formatting and every policy field influence authorization.
    let config_path = workspace.root.join("callisto.toml");
    let config = std::fs::read(&config_path).map_err(|error| GraphError::ReleaseInputRead {
        path: config_path,
        message: error.to_string(),
    })?;
    push_fingerprint(
        &mut components,
        &mut kinds,
        "config.callisto-toml",
        digest_bytes("config", &config),
    );

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
            if let Some(version) = selection.versions().get(&id) {
                selected.insert(id.clone(), (package, version.clone()));
                package_ids.entry(package.id.clone()).or_default().push(id);
            }
        }
    }
    for id in selection.versions().keys() {
        if !selected.contains_key(id) {
            return Err(GraphError::ReleasePackageNotSelected { package: id.clone() });
        }
    }

    let mut operations = BTreeMap::<ReleaseOperationId, ReleaseOperation>::new();
    let mut publishes_by_package = BTreeMap::<ReleasePackageId, Vec<ReleaseOperationId>>::new();
    let mut tag_by_package = BTreeMap::<ReleasePackageId, ReleaseOperationId>::new();

    // First construct leaves so dependency prerequisites can refer only to
    // exact selected release identities, never PackageId's wildcard matcher.
    for (id, (package, version)) in &selected {
        let fingerprint = package_fingerprint(workspace, id, package, version)?;
        push_fingerprint(&mut components, &mut kinds, format!("package:{id}"), fingerprint);

        let mut publishes = Vec::new();
        for target in &package.publish_to {
            if target.ecosystem() == Some(id.ecosystem()) {
                let registry_key = target.registry_key().expect("registry target has registry key");
                let operation =
                    ReleaseOperation::registry_publish(id.clone(), version.clone(), registry_key.as_str(), Vec::new())
                        .map_err(|_error| GraphError::ReleaseIntentStale)?;
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
            operations.insert(operation.id().clone(), operation);
        }
    }

    Ok((
        ReleaseInputSnapshotV1::new(source, components),
        canonical_operation_order(operations)?,
    ))
}

fn package_fingerprint<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    id: &ReleasePackageId,
    package: &callisto_model::Package,
    version: &Version,
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
        transcript.push_str("package.target", target_fingerprint(target)?.as_str());
    }
    Ok(SemanticInputDigest::from_transcript(&transcript))
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

fn push_fingerprint(
    components: &mut Vec<ReleaseInputComponentV1>,
    kinds: &mut BTreeSet<String>,
    kind: impl Into<String>,
    fingerprint: SemanticInputDigest,
) {
    let kind = kind.into();
    assert!(
        kinds.insert(kind.clone()),
        "release snapshot component kinds must be unique"
    );
    components.push(ReleaseInputComponentV1 { kind, fingerprint });
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
    fn digest(&self) -> SemanticInputDigest {
        let mut transcript = CanonicalTranscript::semantic_input_v1();
        transcript.push_str("registry.scheme", &self.scheme);
        transcript.push_str("registry.host", &self.host);
        transcript.push_str(
            "registry.port",
            &self.effective_port.map_or_else(String::new, |port| port.to_string()),
        );
        transcript.push_str("registry.path", &self.path);
        SemanticInputDigest::from_transcript(&transcript)
    }
}

fn target_fingerprint(target: &PublishTarget) -> Result<SemanticInputDigest, GraphError> {
    let mut transcript = CanonicalTranscript::semantic_input_v1();
    match target {
        PublishTarget::CratesIo => transcript.push_str("target.kind", "crates-io"),
        PublishTarget::Npm { registry, access } => {
            transcript.push_str("target.kind", "npm");
            push_registry_binding(&mut transcript, "npm", registry.as_ref())?;
            transcript.push_str(
                "target.access",
                match access {
                    Some(callisto_model::NpmAccess::Public) => "public",
                    Some(callisto_model::NpmAccess::Restricted) => "restricted",
                    None => "default",
                },
            );
        }
        PublishTarget::Pypi { index } => {
            transcript.push_str("target.kind", "pypi");
            push_registry_binding(&mut transcript, "pypi", index.as_ref())?;
        }
        PublishTarget::NuGet { source } => {
            transcript.push_str("target.kind", "nuget");
            push_registry_binding(&mut transcript, "nuget", source.as_ref())?;
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
    registry: &str,
    raw: Option<&String>,
) -> Result<(), GraphError> {
    let fingerprint = match raw {
        Some(raw) => canonical_registry_binding(registry, raw)?.digest(),
        None => digest_bytes("registry.default", b"default"),
    };
    transcript.push_str("target.registry", fingerprint.as_str());
    Ok(())
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
    fn selection() -> ReleaseSelection {
        ReleaseSelection::new(BTreeMap::from([(
            ReleasePackageId::new(Ecosystem::Cargo, "release-fixture").unwrap(),
            Version::parse("1.2.3", VersionGrammar::SemVer).unwrap(),
        )]))
    }
    #[test]
    fn fresh_validation_rejects_manifest_change() {
        let (dir, runner) = fixture();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let intent = build_release_intent(
            dir.path(),
            &locator,
            &runner,
            &selection(),
            ExecutionTrustProfileV1::GitCommit,
        )
        .unwrap();
        validate_release_intent(dir.path(), &locator, &runner, &selection(), intent.clone()).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"release-fixture\"\nversion = \"1.2.4\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let error = validate_release_intent(dir.path(), &locator, &runner, &selection(), intent)
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
}
