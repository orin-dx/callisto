//! Deriving an immutable intent, and the graph-private inputs beside it.
//!
//! Every fingerprint, operation identity, and artifact slot in a release comes
//! from here, from one fresh observation of the workspace.

use std::collections::{BTreeMap, BTreeSet};

use callisto_model::{
    ArtifactSlotId, CanonicalTranscript, CommandRunner, CommitSha, DepKind, ExecutionTrustProfileV1, GitHubRepository,
    PublishTarget, RegistryBindingDigest, RegistryBindingId, ReleaseDecisionV1, ReleaseInputSnapshotV1,
    ReleaseIntentV1, ReleaseOperation, ReleaseOperationId, ReleasePackageId, ReleasePackageInputV1, ReleaseProfileId,
    SemanticInputDigest, SourceIdentity, Version,
};

use crate::config::ReleaseProfileConfig;
use crate::error::{ReleasePreconditionRequirement, ReleaseSelectionInvalidReason, UnsupportedReleaseFeature};
use crate::{DependencyResolver, GraphError, Workspace};

use super::binding::{prepared_git_remote, prepared_registry_binding, PreparedGitRemote};
use super::provider::{
    ArtifactUploadOperation, ForgePublishOperation, ForgeReleaseOperation, PreparedOperation, RegistryPublishOperation,
    TagOperation,
};

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
    profile: ReleaseProfileId,
    source: SourceIdentity,
    trust_profile: ExecutionTrustProfileV1,
    artifact_policy: Option<&ArtifactBuildPolicy>,
) -> Result<ReleaseIntentV1, GraphError> {
    let (snapshot, operations, _, _, slots) =
        derive_release_inputs(workspace, decision, &profile, source, artifact_policy)?;
    Ok(ReleaseIntentV1::new(
        profile,
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
    profile: ReleaseProfileId,
    source: SourceIdentity,
    trust_profile: ExecutionTrustProfileV1,
    artifact_policy: Option<&ArtifactBuildPolicy>,
) -> Result<(ReleaseIntentV1, PreparedDerivation), GraphError> {
    let (snapshot, operations, prepared, git_remote, slots) =
        derive_release_inputs(workspace, decision, &profile, source, artifact_policy)?;
    let intent = ReleaseIntentV1::new(profile, decision.clone(), snapshot, trust_profile, operations, slots)?;
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
    profile: &ReleaseProfileId,
    source: SourceIdentity,
    artifact_policy: Option<&ArtifactBuildPolicy>,
) -> Result<DerivedReleaseInputs, GraphError> {
    // The single authority for profile validity; callers only plumb the id through.
    let profile_config = match workspace.config.product_release.as_ref() {
        Some(release) => Some(
            release
                .profile(profile)
                .ok_or_else(|| GraphError::ReleaseProfileUnknown {
                    profile: profile.as_str().to_owned(),
                })?,
        ),
        None if profile.as_str() == ReleaseProfileId::PRODUCTION => None,
        None => {
            return Err(GraphError::ReleaseProfileUnknown {
                profile: profile.as_str().to_owned(),
            })
        }
    };
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
            profile_config,
        )?;
        package_inputs.push(ReleasePackageInputV1 {
            package: id.clone(),
            fingerprint,
        });

        let mut publishes = Vec::new();
        for target in &package.publish_to {
            if target.ecosystem() == Some(id.ecosystem()) {
                super::provider::registry::require_observable_registry(id.ecosystem())?;
                let binding = prepared_registry_binding(workspace, target, profile_config)?;
                let operation = ReleaseOperation::registry_publish(
                    id.clone(),
                    version.clone(),
                    RegistryBindingId::new(binding.key.as_str(), binding.identity.clone())?,
                    Vec::new(),
                )?;
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
                prepared.insert(
                    operation.id().clone(),
                    PreparedOperation::RegistryPublish(RegistryPublishOperation {
                        package_dir: package_dir(package)?,
                        package_name: id.name().to_string(),
                        version: version.clone(),
                        registry: binding,
                        npm_access: match target {
                            PublishTarget::Npm { access, .. } => *access,
                            _ => None,
                        },
                        npm_tag: matches!(target, PublishTarget::Npm { .. })
                            .then(|| version.is_prerelease().then_some("next".to_string()))
                            .flatten(),
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
            let replacement = ReleaseOperation::new(operation.id().clone(), prerequisites.iter().cloned().collect())?;
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
    profile: Option<&ReleaseProfileConfig>,
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
        transcript.push_str(
            "package.target",
            target_fingerprint(workspace, target, profile)?.as_str(),
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
    target: &PublishTarget,
    profile: Option<&ReleaseProfileConfig>,
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
                prepared_registry_binding(workspace, target, profile)?.identity,
            )?;
        }
        PublishTarget::Npm { access, .. } => {
            push_registry_binding(
                &mut transcript,
                prepared_registry_binding(workspace, target, profile)?.identity,
            )?;
            transcript.push_str(
                "target.access",
                match access {
                    Some(callisto_model::NpmAccess::Public) => "public",
                    Some(callisto_model::NpmAccess::Restricted) => "restricted",
                    None => "default",
                },
            );
        }
        PublishTarget::Pypi { .. } | PublishTarget::NuGet { .. } => {
            push_registry_binding(
                &mut transcript,
                prepared_registry_binding(workspace, target, profile)?.identity,
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
        let source = super::super::capability::observe_source(&workspace, ExecutionTrustProfileV1::GitCommit).unwrap();
        let (before_snapshot, before_operations, _, _, _) = derive_release_inputs(
            &workspace,
            &super::super::tests::decision(),
            &ReleaseProfileId::production(),
            source.clone(),
            None,
        )
        .unwrap();

        std::fs::write(
            root.join("callisto.toml"),
            "# formatting/comments are not release policy\n[[package]]\nmatch = \"release-fixture\"\npublish-to = [\"crates-io\"]\n",
        )
        .unwrap();
        let reread = Workspace::load(root, &locator, &runner).unwrap();
        let (after_snapshot, after_operations, _, _, _) = derive_release_inputs(
            &reread,
            &super::super::tests::decision(),
            &ReleaseProfileId::production(),
            source,
            None,
        )
        .unwrap();

        assert_eq!(before_snapshot, after_snapshot);
        assert_eq!(before_operations, after_operations);
    }
}
