//! Deriving an immutable intent, and the graph-private inputs beside it.
//!
//! Every fingerprint, operation identity, and artifact slot in a release comes
//! from here, from one fresh observation of the workspace.

use std::collections::{BTreeMap, BTreeSet};

use callisto_model::{
    ArtifactSlotId, CanonicalTranscript, CommandRunner, CommitSha, DepKind, ExecutionTrustProfileV1, GitHubRepository,
    PlatformPackageV1, PublishTarget, RegistryBindingDigest, RegistryBindingId, ReleaseDecisionV1,
    ReleaseInputSnapshotV1, ReleaseIntentV1, ReleaseOperation, ReleaseOperationId, ReleasePackageId,
    ReleasePackageInputV1, SemanticInputDigest, SourceIdentity, Version,
};

use crate::error::{ReleasePreconditionRequirement, ReleaseSelectionInvalidReason, UnsupportedReleaseFeature};
use crate::{DependencyResolver, GraphError, Workspace};

use super::binding::{prepared_git_remote, prepared_registry_binding, PreparedGitRemote};
use super::notes::release_notes;
use super::provider::{
    ArtifactUploadOperation, ForgePublishOperation, ForgeReleaseOperation, PreparedOperation, RegistryPublishOperation,
    TagOperation,
};
use crate::commands::registry_argv::npm_default_access;
use crate::toposort::PublishEdgeFilter;

/// A package that is itself an npm platform package (its own package.json has
/// `os`+`cpu`). An owner's attached (Case E) platform manifests do not count.
pub(crate) fn is_platform_package(pkg: &callisto_model::Package) -> bool {
    pkg.manifests.iter().any(|m| {
        matches!(m.role, callisto_model::ManifestRole::Platform { .. })
            && pkg.canonical_manifests().any(|c| c.path == m.path)
    })
}

/// Coordinator-owned identity used to bind a product artifact to the exact
/// workflow revision that built it. This is distinct from a historic release
/// source during recovery.
#[derive(Clone, Debug)]
pub struct ArtifactBuildPolicy {
    pub repository: GitHubRepository,
    pub workflow_path: String,
    pub workflow_commit: CommitSha,
}

pub(crate) type DerivedReleaseInputs = (
    ReleaseInputSnapshotV1,
    Vec<ReleaseOperation>,
    BTreeMap<ReleaseOperationId, PreparedOperation>,
    Option<PreparedGitRemote>,
    Vec<ArtifactSlotId>,
);

pub(crate) struct PreparedDerivation {
    pub(crate) operations: BTreeMap<ReleaseOperationId, PreparedOperation>,
    pub(crate) git_remote: Option<PreparedGitRemote>,
}

pub(crate) fn derive_release_intent<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    decision: &ReleaseDecisionV1,
    source: SourceIdentity,
    trust_profile: ExecutionTrustProfileV1,
    artifact_policy: Option<&ArtifactBuildPolicy>,
) -> Result<ReleaseIntentV1, GraphError> {
    let (snapshot, operations, _, _, slots) = derive_release_inputs(workspace, decision, source, artifact_policy)?;
    Ok(ReleaseIntentV1::new(
        decision.clone(),
        snapshot,
        trust_profile,
        operations,
        slots,
    )?)
}

pub(crate) fn derive_release_intent_with_prepared<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    decision: &ReleaseDecisionV1,
    source: SourceIdentity,
    trust_profile: ExecutionTrustProfileV1,
    artifact_policy: Option<&ArtifactBuildPolicy>,
) -> Result<(ReleaseIntentV1, PreparedDerivation), GraphError> {
    let (snapshot, operations, prepared, git_remote, slots) =
        derive_release_inputs(workspace, decision, source, artifact_policy)?;
    let intent = ReleaseIntentV1::new(decision.clone(), snapshot, trust_profile, operations, slots)?;
    Ok((
        intent,
        PreparedDerivation {
            operations: prepared,
            git_remote,
        },
    ))
}

pub(crate) fn artifact_policy_from_intent(intent: &ReleaseIntentV1) -> Result<Option<ArtifactBuildPolicy>, GraphError> {
    let Some(first) = intent.artifact_slots.first() else {
        return Ok(None);
    };
    let policy = &first.attestation_policy;
    if intent
        .artifact_slots
        .iter()
        .any(|slot| slot.attestation_policy != *policy)
    {
        return Err(GraphError::ReleaseInvariant {
            detail: "release intent mixes artifact attestation policies".to_owned(),
        });
    }
    Ok(Some(ArtifactBuildPolicy {
        repository: policy.repository.clone(),
        workflow_path: policy.workflow_path.clone(),
        workflow_commit: policy.workflow_commit.clone(),
    }))
}

pub(crate) fn derive_release_inputs<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    decision: &ReleaseDecisionV1,
    source: SourceIdentity,
    artifact_policy: Option<&ArtifactBuildPolicy>,
) -> Result<DerivedReleaseInputs, GraphError> {
    if workspace
        .config
        .product_release
        .as_ref()
        .is_some_and(|release| release.forge_repository.is_none())
    {
        return Err(GraphError::ReleaseForgeRepositoryMissing);
    }
    let mut package_inputs = Vec::new();

    let mut selected = BTreeMap::new();
    let mut package_ids = BTreeMap::<callisto_model::PackageId, Vec<ReleasePackageId>>::new();
    for package in workspace.graph.packages() {
        // One canonical manifest per ecosystem is the expected shape, but
        // dedup defensively -- `release_package_ids` maps every canonical
        // manifest, and two manifests sharing an ecosystem would otherwise
        // select/push the same identity twice.
        for id in crate::commands::release_decision::release_package_ids(&workspace.identity, package)?
            .into_iter()
            .collect::<BTreeSet<_>>()
        {
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
    require_platform_dependencies_selected(workspace, &selected, &package_ids)?;
    let all_packages: Vec<_> = workspace.graph.packages().map(|package| package.id.clone()).collect();
    let publish_edges = PublishEdgeFilter::new(
        &package_ids.keys().cloned().collect(),
        &all_packages,
        |id: &callisto_model::PackageId| {
            workspace
                .graph
                .dependencies_of(id)
                .map(|edge| (edge.to.clone(), edge.kind))
                .collect()
        },
    )?;

    let requires_git_remote = selected.values().any(|(package, _)| {
        package
            .publish_to
            .iter()
            .any(|target| !matches!(target, PublishTarget::None))
    });
    let git_remote = requires_git_remote
        .then(|| prepared_git_remote(&workspace.root, workspace.runner))
        .transpose()?;
    if let Some(policy) = artifact_policy {
        let remote = git_remote
            .as_ref()
            .and_then(|remote| remote.github_repository.as_ref())
            .ok_or(GraphError::ReleasePreconditionUnmet {
                requirement: ReleasePreconditionRequirement::GitHubRemote,
            })?;
        if remote != &policy.repository {
            return Err(GraphError::ReleaseArtifactRepositoryMismatch {
                configured: policy.repository.clone(),
                remote: remote.clone(),
            });
        }
    }

    let mut operations = BTreeMap::<ReleaseOperationId, ReleaseOperation>::new();
    let mut prepared = BTreeMap::<ReleaseOperationId, PreparedOperation>::new();
    let mut publishes_by_package = BTreeMap::<ReleasePackageId, Vec<ReleaseOperationId>>::new();
    // Owner registry publish -> its platform publishes, which must land first.
    let mut platforms_by_publish = BTreeMap::<ReleaseOperationId, Vec<ReleaseOperationId>>::new();
    let mut tag_by_package = BTreeMap::<ReleasePackageId, ReleaseOperationId>::new();
    let mut forge_by_package = BTreeMap::<ReleasePackageId, ReleaseOperationId>::new();

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
            if !target.is_implemented() {
                return Err(GraphError::PublishTargetNotImplemented {
                    package: id.clone(),
                    target: target.config_str(),
                });
            }
            if target.ecosystem() == Some(id.ecosystem()) {
                super::provider::registry::require_observable_registry(id.ecosystem())?;
                let binding = prepared_registry_binding(workspace, target, &package.id)?;
                let binding_id = RegistryBindingId::new(binding.key.as_str(), binding.identity.clone())?;
                let operation =
                    ReleaseOperation::registry_publish(id.clone(), version.clone(), binding_id.clone(), Vec::new())?;
                // A durable registry operation must have one exact endpoint
                // binding. The model-level operation identity currently has
                // only a registry key, so reject a second target that would
                // collapse to the same operation until the typed binding
                // identity is added there.
                if prepared.contains_key(operation.id()) {
                    return Err(GraphError::ReleaseSelectionInvalid {
                        package: id.clone(),
                        reason: ReleaseSelectionInvalidReason::DuplicateRegistryTarget,
                    });
                }
                let explicit_access = match target {
                    PublishTarget::Npm { access, .. } => *access,
                    _ => None,
                };
                let npm_tag = matches!(target, PublishTarget::Npm { .. })
                    .then(|| version.is_prerelease().then_some("next".to_string()))
                    .flatten();
                if matches!(target, PublishTarget::Npm { .. }) {
                    let mut platform_ids = Vec::new();
                    for (name, manifest) in workspace.identity.attached_platforms(package) {
                        let directory = manifest.parent().unwrap_or(std::path::Path::new(""));
                        let platform = PlatformPackageV1::new(
                            ReleasePackageId::new(callisto_model::Ecosystem::Npm, name)?,
                            directory.to_string_lossy().replace('\\', "/"),
                        )?;
                        let platform_operation = ReleaseOperation::new(
                            callisto_model::ReleaseOperationId::platform_publish(
                                id.clone(),
                                version.clone(),
                                binding_id.clone(),
                                platform,
                            ),
                            Vec::new(),
                        )?;
                        prepared.insert(
                            platform_operation.id().clone(),
                            PreparedOperation::RegistryPublish(RegistryPublishOperation {
                                package_dir: directory.to_path_buf(),
                                package_name: name.to_string(),
                                version: version.clone(),
                                registry: prepared_registry_binding(workspace, target, &package.id)?,
                                npm_access: npm_default_access(name, explicit_access),
                                npm_tag: npm_tag.clone(),
                                by_directory: true,
                            }),
                        );
                        platform_ids.push(platform_operation.id().clone());
                        operations.insert(platform_operation.id().clone(), platform_operation);
                    }
                    platforms_by_publish.insert(operation.id().clone(), platform_ids);
                }
                prepared.insert(
                    operation.id().clone(),
                    PreparedOperation::RegistryPublish(RegistryPublishOperation {
                        package_dir: package_dir(package)?,
                        package_name: id.name().to_string(),
                        version: version.clone(),
                        registry: binding,
                        npm_access: matches!(target, PublishTarget::Npm { .. })
                            .then(|| npm_default_access(id.name(), explicit_access))
                            .flatten(),
                        npm_tag,
                        by_directory: false,
                    }),
                );
                publishes.push(operation.id().clone());
                operations.insert(operation.id().clone(), operation);
            }
        }
        publishes.sort();
        publishes_by_package.insert(id.clone(), publishes);
    }

    // Replace each publish leaf with prerequisites from selected dependency
    // packages, ordered by the shared publish edge filter.
    for (id, (package, _)) in &selected {
        let mut prerequisites = BTreeSet::new();
        for edge in workspace.graph.dependencies_of(&package.id) {
            if !publish_edges.allows(&package.id, &edge.to, edge.kind) {
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
            let mut prerequisites = prerequisites.clone();
            prerequisites.extend(platforms_by_publish.get(publish_id).into_iter().flatten().cloned());
            let replacement = ReleaseOperation::new(operation.id().clone(), prerequisites.into_iter().collect())?;
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
        )?;
        let tag_name = package
            .tag_template
            .clone()
            .unwrap_or_else(|| callisto_model::TagTemplate::default_for(&package.id))
            .render(version);
        let target = match &source {
            SourceIdentity::GitCommit { sha } => sha.clone(),
            SourceIdentity::HermeticContent { .. } => {
                return Err(GraphError::UnsupportedRelease {
                    feature: UnsupportedReleaseFeature::SourceIdentity,
                })
            }
        };
        prepared.insert(
            tag.id().clone(),
            PreparedOperation::Tag(TagOperation {
                annotation: format!("Release {tag_name}"),
                name: tag_name,
                target,
            }),
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
            let operation = ReleaseOperation::forge_release(id.clone(), version.clone(), vec![tag])?;
            let tag = match prepared.get(operation.prerequisites().first().expect("forge release has tag")) {
                Some(PreparedOperation::Tag(prepared_tag)) => prepared_tag.name.clone(),
                _ => {
                    return Err(GraphError::ReleaseInvariant {
                        detail: format!(
                            "forge release `{:?}` prerequisite is not a prepared Tag operation",
                            operation.id()
                        ),
                    })
                }
            };
            prepared.insert(
                operation.id().clone(),
                PreparedOperation::ForgeRelease(ForgeReleaseOperation {
                    tag,
                    prerelease: version.is_prerelease(),
                    notes: release_notes(&workspace.root, package.changelog.as_deref(), version),
                }),
            );
            forge_by_package.insert(id.clone(), operation.id().clone());
            operations.insert(operation.id().clone(), operation);
        }
    }

    let mut artifact_slots = Vec::new();
    let mut uploads_by_package = BTreeMap::<ReleasePackageId, Vec<ReleaseOperationId>>::new();
    if let (Some(product), Some(policy)) = (&workspace.config.product_release, artifact_policy) {
        require_product_package_publishes_to_forge(workspace, &product.package)?;
        for (id, (package, _)) in &selected {
            // Workspace package identities may remain bare even when a
            // policy intentionally qualifies the product by ecosystem. Use
            // the model's compatibility relation and retain the explicit
            // ecosystem check so a same-name package in another ecosystem
            // cannot acquire the product's binary release slots.
            if !product.package.matches(&package.id) || product.package.ecosystem() != Some(id.ecosystem()) {
                continue;
            }
            let forge = forge_by_package.get(id).ok_or_else(|| GraphError::ReleaseInvariant {
                detail: format!("product package `{id}` has no forge release operation"),
            })?;
            let tag = match prepared.get(forge) {
                Some(PreparedOperation::ForgeRelease(prepared_forge)) => prepared_forge.tag.clone(),
                _ => {
                    return Err(GraphError::ReleaseInvariant {
                        detail: format!("product forge release `{forge:?}` is not prepared"),
                    })
                }
            };
            for artifact in &product.artifacts {
                // The slot names the package whose build produced the bytes, which
                // need not be the product -- a plugin can ship on the product's
                // release. Every slot still attaches to the product's one release.
                let (owner_id, owner_version) = selected
                    .iter()
                    .find(|(candidate, (pkg, _))| {
                        artifact.package.matches(&pkg.id) && artifact.package.ecosystem() == Some(candidate.ecosystem())
                    })
                    .map(|(candidate, (_, owner_version))| (candidate.clone(), owner_version.clone()))
                    .ok_or_else(|| GraphError::ReleaseArtifactOwnerNotReleased {
                        asset: artifact.asset_name.clone(),
                        package: artifact.package.to_string(),
                    })?;
                let slot = ArtifactSlotId::new(
                    owner_id.clone(),
                    owner_version.clone(),
                    &artifact.target,
                    artifact.asset_name.clone(),
                    policy.repository.clone(),
                    policy.workflow_path.clone(),
                    policy.workflow_commit.clone(),
                )
                .map_err(|error| GraphError::ReleaseInvariant {
                    detail: format!("invalid configured artifact slot: {error}"),
                })?;
                let operation = ReleaseOperation::artifact_upload(slot.clone(), vec![forge.clone()])?;
                prepared.insert(
                    operation.id().clone(),
                    PreparedOperation::ArtifactUpload(ArtifactUploadOperation {
                        slot: slot.clone(),
                        tag: tag.clone(),
                        prerelease: owner_version.is_prerelease(),
                    }),
                );
                uploads_by_package
                    .entry(owner_id.clone())
                    .or_default()
                    .push(operation.id().clone());
                operations.insert(operation.id().clone(), operation);
                artifact_slots.push(slot);
            }
        }
    }

    // Publishing is the last forge step: a GitHub Release is only complete
    // once every asset it carries has been uploaded to it as a draft.
    for (id, (_, version)) in &selected {
        let Some(forge) = forge_by_package.get(id) else {
            continue;
        };
        let mut prerequisites = vec![forge.clone()];
        prerequisites.extend(uploads_by_package.get(id).into_iter().flatten().cloned());
        let operation = ReleaseOperation::forge_publish(id.clone(), version.clone(), prerequisites)?;
        let tag = match prepared.get(forge) {
            Some(PreparedOperation::ForgeRelease(prepared_forge)) => prepared_forge.tag.clone(),
            _ => {
                return Err(GraphError::ReleaseInvariant {
                    detail: format!("forge publish `{:?}` has no prepared draft release", operation.id()),
                })
            }
        };
        prepared.insert(
            operation.id().clone(),
            PreparedOperation::ForgePublish(ForgePublishOperation {
                tag,
                prerelease: version.is_prerelease(),
            }),
        );
        operations.insert(operation.id().clone(), operation);
    }

    Ok((
        ReleaseInputSnapshotV1::new(source, package_inputs)?,
        canonical_operation_order(operations)?,
        prepared,
        git_remote,
        artifact_slots,
    ))
}

/// A selected npm package's unreleased standalone platform dependency must be
/// selected with it, or the owner would publish pointing at a missing version.
fn require_platform_dependencies_selected<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    selected: &BTreeMap<ReleasePackageId, (&callisto_model::Package, Version)>,
    package_ids: &BTreeMap<callisto_model::PackageId, Vec<ReleasePackageId>>,
) -> Result<(), GraphError> {
    let mut base_versions = None;
    for (id, (package, _)) in selected {
        if id.ecosystem() != callisto_model::Ecosystem::Npm {
            continue;
        }
        for edge in workspace.graph.dependencies_of(&package.id) {
            if !matches!(edge.kind, DepKind::Runtime | DepKind::Optional) || package_ids.contains_key(&edge.to) {
                continue;
            }
            let Some(platform) = workspace
                .graph
                .packages()
                .find(|candidate| candidate.id == edge.to && is_platform_package(candidate))
            else {
                continue;
            };
            if base_versions.is_none() {
                base_versions = Some(workspace.base_versions()?);
            }
            let current = base_versions.as_ref().and_then(|versions| versions.get(&platform.id));
            let last_tag = workspace.tags()?.last_tag(&platform.id);
            let released = current.is_some_and(|current| last_tag.is_some_and(|tag| &tag.version == current));
            if !released {
                let name = workspace
                    .identity
                    .native_name(&platform.id, callisto_model::Ecosystem::Npm)
                    .unwrap_or(platform.id.name());
                return Err(GraphError::ReleaseSelectionInvalid {
                    package: ReleasePackageId::new(callisto_model::Ecosystem::Npm, name)?,
                    reason: ReleaseSelectionInvalidReason::PlatformDependencyNotSelected,
                });
            }
        }
    }
    Ok(())
}

/// A configured product package must exist and publish to github-release; otherwise
/// derivation would silently yield zero artifact slots.
fn require_product_package_publishes_to_forge<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    product: &callisto_model::PackageId,
) -> Result<(), GraphError> {
    let mut found = false;
    for package in workspace.graph.packages() {
        let is_product = crate::commands::release_decision::release_package_ids(&workspace.identity, package)?
            .iter()
            .any(|id| product.matches(&package.id) && product.ecosystem() == Some(id.ecosystem()));
        if !is_product {
            continue;
        }
        found = true;
        if package
            .publish_to
            .iter()
            .any(|target| matches!(target, PublishTarget::GitHubRelease))
        {
            return Ok(());
        }
    }
    let detail = if found {
        format!("product-package `{product}` does not publish to github-release")
    } else {
        format!("product-package `{product}` is not a package in this workspace")
    };
    Err(crate::error::ConfigError::InvalidProductRelease { detail }.into())
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
    let npm_name = workspace
        .identity
        .native_name(&package.id, callisto_model::Ecosystem::Npm)
        .unwrap_or(package.id.name());
    for target in &package.publish_to {
        transcript.push_str(
            "package.target",
            target_fingerprint(workspace, &package.id, npm_name, target)?.as_str(),
        );
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
        .ok_or_else(|| GraphError::ReleaseInvariant {
            detail: format!("selected release package `{}` has no canonical manifest", package.id),
        })
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

pub(crate) fn canonical_operation_order(
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
        return Err(GraphError::ReleaseInvariant {
            detail: "release operation DAG contains a cycle: topological sort could not order every operation"
                .to_string(),
        });
    }
    Ok(ordered)
}

fn digest_bytes(tag: &str, bytes: &[u8]) -> SemanticInputDigest {
    let mut transcript = CanonicalTranscript::semantic_input_v1();
    transcript.push_bytes(tag, bytes);
    SemanticInputDigest::from_transcript(&transcript)
}

fn target_fingerprint<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    package: &callisto_model::PackageId,
    npm_name: &str,
    target: &PublishTarget,
) -> Result<SemanticInputDigest, GraphError> {
    let mut transcript = CanonicalTranscript::semantic_input_v1();
    // Single source of truth for the "kind" string, shared with
    // `PublishTarget::config_str`'s other diagnostic/config-parsing callers
    // instead of re-hardcoding these per-variant literals here too.
    transcript.push_str("target.kind", target.config_str());
    match target {
        PublishTarget::GitHubRelease | PublishTarget::None => {}
        PublishTarget::CratesIo => {
            push_registry_binding(
                &mut transcript,
                prepared_registry_binding(workspace, target, package)?.identity,
            )?;
        }
        PublishTarget::Npm { access, .. } => {
            push_registry_binding(
                &mut transcript,
                prepared_registry_binding(workspace, target, package)?.identity,
            )?;
            transcript.push_str(
                "target.access",
                match npm_default_access(npm_name, *access) {
                    Some(callisto_model::NpmAccess::Public) => "public",
                    Some(callisto_model::NpmAccess::Restricted) => "restricted",
                    None => "default",
                },
            );
        }
        PublishTarget::Pypi { .. } | PublishTarget::NuGet { .. } => {
            push_registry_binding(
                &mut transcript,
                prepared_registry_binding(workspace, target, package)?.identity,
            )?;
        }
        #[allow(unreachable_patterns)]
        _ => {
            return Err(GraphError::UnsupportedRelease {
                feature: UnsupportedReleaseFeature::PublishTarget,
            })
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_in_callisto_toml_do_not_change_semantic_release_inputs() {
        let (dir, runner) = super::super::tests::fixture();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let root = super::super::capability::canonical_root(dir.path()).unwrap();
        let workspace = Workspace::load(root.clone(), &locator, &runner).unwrap();
        let source = super::super::capability::observe_source(
            &workspace,
            ExecutionTrustProfileV1::GitCommit,
            super::super::capability::ReleaseCheckout::Detached,
        )
        .unwrap();
        let (before_snapshot, before_operations, _, _, _) =
            derive_release_inputs(&workspace, &super::super::tests::decision(), source.clone(), None).unwrap();

        std::fs::write(
            root.join("callisto.toml"),
            "# formatting/comments are not release policy\n[[package]]\nmatch = \"release-fixture\"\npublish-to = [\"crates-io\"]\n",
        )
        .unwrap();
        let reread = Workspace::load(root, &locator, &runner).unwrap();
        let (after_snapshot, after_operations, _, _, _) =
            derive_release_inputs(&reread, &super::super::tests::decision(), source, None).unwrap();

        assert_eq!(before_snapshot, after_snapshot);
        assert_eq!(before_operations, after_operations);
    }

    /// AC-011: a `[release]` without a forge destination cannot plan.
    #[test]
    fn a_release_section_without_forge_repository_cannot_derive() {
        let (dir, runner) = super::super::tests::fixture();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let root = super::super::capability::canonical_root(dir.path()).unwrap();
        let clean = Workspace::load(root.clone(), &locator, &runner).unwrap();
        let source = super::super::capability::observe_source(
            &clean,
            ExecutionTrustProfileV1::GitCommit,
            super::super::capability::ReleaseCheckout::Detached,
        )
        .unwrap();
        std::fs::write(
            root.join("callisto.toml"),
            "[release]\nproduct-package = \"cargo/release-fixture\"\n\n[[release.artifact]]\npackage = \"cargo/release-fixture\"\ntarget = \"t\"\nasset-name = \"a.tar.gz\"\n",
        )
        .unwrap();
        let workspace = Workspace::load(root, &locator, &runner).unwrap();
        assert!(matches!(
            derive_release_inputs(&workspace, &super::super::tests::decision(), source, None),
            Err(GraphError::ReleaseForgeRepositoryMissing)
        ));
    }

    use callisto_model::{Ecosystem, NpmAccess, ReleaseDecisionEntry, ReleaseInclusionReason, VersionGrammar};

    use super::super::provider::{NotesFallback, ReleaseNotes};
    use super::super::tests::RealGitRunner;

    fn git(root: &std::path::Path, args: &[&str]) {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }

    /// A committed, detached checkout of `files` with a GitHub `origin`.
    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, body) in files {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        for args in [
            ["init", "-q"].as_slice(),
            ["config", "user.email", "test@example.com"].as_slice(),
            ["config", "user.name", "Test"].as_slice(),
            ["config", "commit.gpgsign", "false"].as_slice(),
            ["config", "tag.gpgsign", "false"].as_slice(),
            ["remote", "add", "origin", "https://github.com/example/parity.git"].as_slice(),
            ["add", "."].as_slice(),
            ["commit", "-q", "-m", "fixture"].as_slice(),
            ["checkout", "--detach", "-q", "HEAD"].as_slice(),
        ] {
            git(dir.path(), args);
        }
        dir
    }

    fn release(entries: &[(Ecosystem, &str, &str)]) -> ReleaseDecisionV1 {
        ReleaseDecisionV1::new(
            entries
                .iter()
                .map(|(ecosystem, name, version)| ReleaseDecisionEntry {
                    package: ReleasePackageId::new(*ecosystem, name).unwrap(),
                    target_version: Version::parse(version, VersionGrammar::SemVer).unwrap(),
                    reasons: vec![ReleaseInclusionReason::ExplicitSelection],
                })
                .collect(),
        )
        .unwrap()
    }

    fn derive(dir: &tempfile::TempDir, decision: &ReleaseDecisionV1) -> Result<DerivedReleaseInputs, GraphError> {
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let root = super::super::capability::canonical_root(dir.path()).unwrap();
        let workspace = Workspace::load(root, &locator, &RealGitRunner).unwrap();
        let source = super::super::capability::observe_source(
            &workspace,
            ExecutionTrustProfileV1::GitCommit,
            super::super::capability::ReleaseCheckout::Detached,
        )
        .unwrap();
        derive_release_inputs(&workspace, decision, source, None)
    }

    fn registry_publish<'a>(operations: &'a [ReleaseOperation], name: &str) -> &'a ReleaseOperation {
        operations
            .iter()
            .find(|operation| {
                matches!(
                    operation.id().role,
                    callisto_model::ReleaseOperationRole::RegistryPublish { .. }
                ) && operation.id().package.name() == name
            })
            .unwrap_or_else(|| panic!("no registry publish for {name}"))
    }

    fn cargo_workspace(crates: &[(&str, &str)]) -> Vec<(String, String)> {
        let members: Vec<String> = crates.iter().map(|(name, _)| format!("\"{name}\"")).collect();
        let mut files = vec![
            (
                "Cargo.toml".to_owned(),
                format!("[workspace]\nmembers = [{}]\n", members.join(", ")),
            ),
            ("callisto.toml".to_owned(), String::new()),
        ];
        for (name, dependencies) in crates {
            files.push((
                format!("{name}/Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n{dependencies}"),
            ));
            files[1].1.push_str(&format!(
                "[[package]]\nmatch = \"{name}\"\npublish-to = [\"crates-io\"]\n"
            ));
        }
        files
    }

    fn repo_of(files: &[(String, String)]) -> tempfile::TempDir {
        let files: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        repo(&files)
    }

    fn cargo_release(names: &[&str]) -> ReleaseDecisionV1 {
        release(
            &names
                .iter()
                .map(|name| (Ecosystem::Cargo, *name, "1.0.0"))
                .collect::<Vec<_>>(),
        )
    }

    /// AC-2: a dev-only cycle orders instead of failing; an unrelated dev edge still orders.
    #[test]
    fn ac2_dev_only_cycle_is_ordered_not_a_cycle_error() {
        let dir = repo_of(&cargo_workspace(&[
            ("a", "[dev-dependencies]\nb = { path = \"../b\" }\n"),
            ("b", "[dev-dependencies]\na = { path = \"../a\" }\n"),
            ("c", "[dev-dependencies]\na = { path = \"../a\" }\n"),
        ]));
        let (_, operations, _, _, _) = derive(&dir, &cargo_release(&["a", "b", "c"])).unwrap();
        let a = registry_publish(&operations, "a").id().clone();
        let b = registry_publish(&operations, "b").id().clone();
        assert!(!registry_publish(&operations, "a").prerequisites().contains(&b));
        assert!(!registry_publish(&operations, "b").prerequisites().contains(&a));
        assert!(registry_publish(&operations, "c").prerequisites().contains(&a));
    }

    /// AC-3: a runtime cycle still fails, even with dev edges alongside it.
    #[test]
    fn ac3_runtime_cycle_still_fails_with_a_cycle_error() {
        let dir = repo_of(&cargo_workspace(&[
            (
                "a",
                "[dependencies]\nb = { path = \"../b\" }\n[dev-dependencies]\nc = { path = \"../c\" }\n",
            ),
            ("b", "[dependencies]\na = { path = \"../a\" }\n"),
            ("c", "[dev-dependencies]\na = { path = \"../a\" }\n"),
        ]));
        let error = derive(&dir, &cargo_release(&["a", "b", "c"])).unwrap_err();
        assert!(matches!(error, GraphError::Cycle { .. }), "{error:?}");
    }

    /// AC-3: in a runtime+dev cycle only the dev edge is excluded; the runtime edge orders.
    #[test]
    fn ac3_mixed_cycle_excludes_only_the_dev_edge() {
        let dir = repo_of(&cargo_workspace(&[
            ("a", "[dependencies]\nb = { path = \"../b\" }\n"),
            ("b", "[dev-dependencies]\na = { path = \"../a\" }\n"),
        ]));
        let (_, operations, _, _, _) = derive(&dir, &cargo_release(&["a", "b"])).unwrap();
        let a = registry_publish(&operations, "a").id().clone();
        let b = registry_publish(&operations, "b").id().clone();
        assert!(registry_publish(&operations, "a").prerequisites().contains(&b));
        assert!(!registry_publish(&operations, "b").prerequisites().contains(&a));
    }

    /// AC-12: exactly one package input per decision entry; unselected packages never appear.
    #[test]
    fn ac12_derivation_neither_adds_nor_drops_selected_packages() {
        let dir = repo_of(&cargo_workspace(&[
            ("a", "[dependencies]\nb = { path = \"../b\" }\n"),
            ("b", ""),
            ("c", ""),
        ]));
        for names in [vec!["a"], vec!["a", "c"], vec!["a", "b", "c"]] {
            let decision = cargo_release(&names);
            let (snapshot, operations, _, _, _) = derive(&dir, &decision).unwrap();
            let derived: Vec<_> = snapshot.packages.iter().map(|input| input.package.clone()).collect();
            let decided: Vec<_> = decision.entries.iter().map(|entry| entry.package.clone()).collect();
            assert_eq!(derived, decided);
            assert!(operations
                .iter()
                .all(|operation| decided.contains(&operation.id().package)));
        }
    }

    fn npm_platform_repo(tag_platform: bool) -> tempfile::TempDir {
        let dir = repo(&[
            ("pnpm-workspace.yaml", "packages:\n  - \"packages/*\"\n"),
            ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n"),
            (
                "packages/main/package.json",
                r#"{"name":"main","version":"1.0.0","dependencies":{"plat":"1.0.0"}}"#,
            ),
            (
                "packages/plat/package.json",
                r#"{"name":"plat","version":"1.0.0","os":["darwin"],"cpu":["arm64"]}"#,
            ),
            ("callisto.toml", ""),
        ]);
        if tag_platform {
            git(dir.path(), &["tag", "plat@1.0.0"]);
        }
        dir
    }

    /// AC-1: an unreleased standalone platform dependency must be selected with its npm owner.
    #[test]
    fn ac1_unselected_unreleased_platform_dependency_is_rejected() {
        let dir = npm_platform_repo(false);
        let error = derive(&dir, &release(&[(Ecosystem::Npm, "main", "1.0.0")])).unwrap_err();
        assert!(
            matches!(
                &error,
                GraphError::ReleaseSelectionInvalid {
                    package,
                    reason: ReleaseSelectionInvalidReason::PlatformDependencyNotSelected,
                } if package.name() == "plat"
            ),
            "{error:?}"
        );
        derive(
            &dir,
            &release(&[(Ecosystem::Npm, "main", "1.0.0"), (Ecosystem::Npm, "plat", "1.0.0")]),
        )
        .unwrap();
    }

    /// AC-1b: an already-released platform dependency need not be selected.
    #[test]
    fn ac1b_released_platform_dependency_need_not_be_selected() {
        let dir = npm_platform_repo(true);
        let (snapshot, _, _, _, _) = derive(&dir, &release(&[(Ecosystem::Npm, "main", "1.0.0")])).unwrap();
        assert_eq!(snapshot.packages.len(), 1);
    }

    fn npm_registry_repo(registry: &str) -> tempfile::TempDir {
        repo(&[
            (
                "package.json",
                &format!(r#"{{"name":"lib","version":"1.0.0","publishConfig":{{"registry":"{registry}"}}}}"#),
            ),
            (
                "callisto.toml",
                "[registries.corp]\nkind = \"npm\"\nurl = \"https://npm.corp.example/\"\n",
            ),
        ])
    }

    /// AC-4: a `publishConfig.registry` not configured in `[registries]` is untrusted.
    #[test]
    fn ac4_unapproved_npm_registry_override_is_rejected() {
        let dir = npm_registry_repo("https://npm.evil.example/");
        let error = derive(&dir, &release(&[(Ecosystem::Npm, "lib", "1.0.0")])).unwrap_err();
        assert!(matches!(error, GraphError::UntrustedNpmRegistry { .. }), "{error:?}");
        let approved = npm_registry_repo("https://npm.corp.example/");
        derive(&approved, &release(&[(Ecosystem::Npm, "lib", "1.0.0")])).unwrap();
    }

    /// AC-5: a cleartext override fails the scheme check before host matching.
    #[test]
    fn ac5_non_https_npm_registry_override_is_unsafe_even_on_an_approved_host() {
        let dir = npm_registry_repo("http://npm.corp.example/");
        let error = derive(&dir, &release(&[(Ecosystem::Npm, "lib", "1.0.0")])).unwrap_err();
        assert!(matches!(error, GraphError::UnsafeRegistryBinding { .. }), "{error:?}");
    }

    /// AC-6: a scoped npm package without explicit access publishes public, for the
    /// owner and its platform prerequisites, and the defaulted value is fingerprinted.
    #[test]
    fn ac6_scoped_npm_package_defaults_to_public_access() {
        let dir = repo(&[
            ("package.json", r#"{"name":"@s/lib","version":"1.0.0"}"#),
            ("callisto.toml", ""),
        ]);
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let root = super::super::capability::canonical_root(dir.path()).unwrap();
        let workspace = Workspace::load(root, &locator, &RealGitRunner).unwrap();
        let source = super::super::capability::observe_source(
            &workspace,
            ExecutionTrustProfileV1::GitCommit,
            super::super::capability::ReleaseCheckout::Detached,
        )
        .unwrap();
        let (_, _, prepared, _, _) = derive_release_inputs(
            &workspace,
            &release(&[(Ecosystem::Npm, "@s/lib", "1.0.0")]),
            source,
            None,
        )
        .unwrap();
        let access: Vec<_> = prepared
            .values()
            .filter_map(|operation| match operation {
                PreparedOperation::RegistryPublish(publish) => Some(publish.npm_access),
                _ => None,
            })
            .collect();
        assert_eq!(access, vec![Some(NpmAccess::Public)]);

        let package = callisto_model::PackageId::parse("lib").unwrap();
        let fingerprint = |name: &str, access| {
            target_fingerprint(
                &workspace,
                &package,
                name,
                &PublishTarget::Npm { registry: None, access },
            )
            .unwrap()
        };
        assert_eq!(
            fingerprint("@s/lib", None),
            fingerprint("@s/lib", Some(NpmAccess::Public))
        );
        assert_ne!(fingerprint("lib", None), fingerprint("lib", Some(NpmAccess::Public)));
    }

    /// The loaded graph with every package's `publish_to` replaced; config
    /// load itself rejects a target no workspace manifest can carry.
    struct Retargeted {
        packages: Vec<callisto_model::Package>,
        edges: Vec<callisto_model::DepEdge>,
    }

    impl DependencyResolver for Retargeted {
        fn packages(&self) -> impl Iterator<Item = &callisto_model::Package> {
            self.packages.iter()
        }

        fn dependencies_of(&self, id: &callisto_model::PackageId) -> impl Iterator<Item = &callisto_model::DepEdge> {
            self.edges.iter().filter(move |edge| &edge.from == id)
        }

        fn dependents_of(&self, id: &callisto_model::PackageId) -> impl Iterator<Item = &callisto_model::DepEdge> {
            self.edges.iter().filter(move |edge| &edge.to == id)
        }
    }

    /// AC-11: an undispatchable target fails derivation instead of being skipped.
    #[test]
    fn ac11_unimplemented_publish_target_fails_derivation() {
        let dir = repo(&[
            (
                "Cargo.toml",
                "[package]\nname = \"core\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
            ),
            (
                "callisto.toml",
                "[[package]]\nmatch = \"core\"\npublish-to = [\"crates-io\"]\n",
            ),
        ]);
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let root = super::super::capability::canonical_root(dir.path()).unwrap();
        let loaded = Workspace::load(root.clone(), &locator, &RealGitRunner).unwrap();
        let packages = loaded
            .graph
            .packages()
            .cloned()
            .map(|mut package| {
                package.publish_to.push(PublishTarget::NuGet { source: None });
                package
            })
            .collect();
        let workspace = Workspace {
            root,
            config: crate::config::load(dir.path()).unwrap(),
            graph: Retargeted {
                packages,
                edges: Vec::new(),
            },
            tags: std::cell::OnceCell::new(),
            git: std::cell::OnceCell::new(),
            runner: &RealGitRunner,
            manifest_cache: Default::default(),
            identity: crate::IdentityIndex::default(),
        };
        let source = super::super::capability::observe_source(
            &workspace,
            ExecutionTrustProfileV1::GitCommit,
            super::super::capability::ReleaseCheckout::Detached,
        )
        .unwrap();
        let error = derive_release_inputs(&workspace, &cargo_release(&["core"]), source, None).unwrap_err();
        assert!(
            matches!(
                &error,
                GraphError::PublishTargetNotImplemented { package, target: "nuget" } if package.name() == "core"
            ),
            "{error:?}"
        );
    }

    fn forge_notes(prepared: &BTreeMap<ReleaseOperationId, PreparedOperation>) -> ReleaseNotes {
        prepared
            .values()
            .find_map(|operation| match operation {
                PreparedOperation::ForgeRelease(forge) => Some(forge.notes.clone()),
                _ => None,
            })
            .expect("a forge release operation")
    }

    /// AC-7: the forge release carries the changelog section, and the notes stay
    /// out of the intent: changing them changes neither snapshot nor operations.
    #[test]
    fn ac7_forge_release_notes_come_from_the_changelog_and_do_not_affect_the_intent() {
        let dir = repo(&[
            (
                "Cargo.toml",
                "[package]\nname = \"core\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
            ),
            ("CHANGELOG.md", "# core\n\n## 1.0.0\n\n- fixed a bug\n"),
            (
                "callisto.toml",
                "[[package]]\nmatch = \"core\"\npublish-to = [\"github-release\"]\nchangelog = \"CHANGELOG.md\"\n",
            ),
        ]);
        let decision = cargo_release(&["core"]);
        let (snapshot, operations, prepared, _, _) = derive(&dir, &decision).unwrap();
        assert_eq!(
            forge_notes(&prepared),
            ReleaseNotes::Section("- fixed a bug".to_owned())
        );

        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let root = super::super::capability::canonical_root(dir.path()).unwrap();
        let workspace = Workspace::load(root, &locator, &RealGitRunner).unwrap();
        let source = super::super::capability::observe_source(
            &workspace,
            ExecutionTrustProfileV1::GitCommit,
            super::super::capability::ReleaseCheckout::Detached,
        )
        .unwrap();
        std::fs::write(dir.path().join("CHANGELOG.md"), "# core\n\n## 1.0.0\n\n").unwrap();
        let (after_snapshot, after_operations, after_prepared, _, _) =
            derive_release_inputs(&workspace, &decision, source, None).unwrap();
        assert_eq!(
            forge_notes(&after_prepared),
            ReleaseNotes::Generated {
                reason: NotesFallback::SectionEmpty
            }
        );
        assert_eq!(snapshot, after_snapshot);
        assert_eq!(operations, after_operations);
    }
}
