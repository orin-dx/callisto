//! Deriving an immutable intent, and the graph-private inputs beside it.
//!
//! Every fingerprint, operation identity, and artifact slot in a release comes
//! from here, from one fresh observation of the workspace.

use std::collections::{BTreeMap, BTreeSet};

use callisto_model::{
    ArtifactSlotId, CanonicalTranscript, CommandRunner, CommitSha, DepKind, Diagnostic, DiagnosticCode,
    DiagnosticSeverity, ExecutionTrustProfileV1, GitHubRepository, PlatformPackageV1, PublishTarget,
    RegistryBindingDigest, RegistryBindingId, ReleaseDecisionV1, ReleaseInputSnapshotV1, ReleaseIntentV1,
    ReleaseOperation, ReleaseOperationId, ReleasePackageId, ReleasePackageInputV1, SemanticInputDigest, SourceIdentity,
    Version,
};

use crate::error::{ReleasePreconditionRequirement, ReleaseSelectionInvalidReason, UnsupportedReleaseFeature};
use crate::{DependencyResolver, GraphError, Workspace};

use super::binding::{optional_git_remote, prepared_git_remote, prepared_registry_binding, PreparedGitRemote};
use super::notes::release_notes;
use super::provider::{
    ArtifactUploadOperation, ForgePublishOperation, ForgeReleaseOperation, PreparedOperation, RegistryPublishOperation,
    TagOperation,
};
use crate::commands::registry_argv::npm_default_access;
use crate::commands::release_decision::{has_dispatchable_target, version_is_tagged};
use crate::toposort::PublishEdgeFilter;

/// A package that is itself an npm platform package (its own package.json has
/// `os`+`cpu`). An owner's attached platform manifests do not count.
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

/// Whether a missing `origin` push URL fails derivation or leaves tags unbound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitRemoteRequirement {
    Required,
    /// A preview only: tag operations are derived without a remote identity.
    OptionalForPreview,
}

/// `diagnostics` accumulates advisory findings from this derivation (e.g. an
/// artifact owner selected without its product); the caller decides whether and
/// where to surface them, since a re-derivation for integrity verification has no
/// use for them.
pub(crate) fn derive_release_intent<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    decision: &ReleaseDecisionV1,
    source: SourceIdentity,
    trust_profile: ExecutionTrustProfileV1,
    artifact_policy: Option<&ArtifactBuildPolicy>,
    remote: GitRemoteRequirement,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<ReleaseIntentV1, GraphError> {
    let (snapshot, operations, _, _, slots) =
        derive_release_inputs_with(workspace, decision, source, artifact_policy, remote, diagnostics)?;
    Ok(ReleaseIntentV1::new(
        decision.clone(),
        snapshot,
        trust_profile,
        operations,
        slots,
    )?)
}

/// See [`derive_release_intent`] for `diagnostics`.
pub(crate) fn derive_release_intent_with_prepared<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    decision: &ReleaseDecisionV1,
    source: SourceIdentity,
    trust_profile: ExecutionTrustProfileV1,
    artifact_policy: Option<&ArtifactBuildPolicy>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(ReleaseIntentV1, PreparedDerivation), GraphError> {
    let (snapshot, operations, prepared, git_remote, slots) =
        derive_release_inputs(workspace, decision, source, artifact_policy, diagnostics)?;
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
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<DerivedReleaseInputs, GraphError> {
    derive_release_inputs_with(
        workspace,
        decision,
        source,
        artifact_policy,
        GitRemoteRequirement::Required,
        diagnostics,
    )
}

/// The decision's packages, by release identity, and each package's release identities.
struct Selection<'w> {
    selected: BTreeMap<ReleasePackageId, (&'w callisto_model::Package, Version)>,
    package_ids: BTreeMap<callisto_model::PackageId, Vec<ReleasePackageId>>,
    /// The first selected identity of each package in manifest order: it carries the package's shared effects.
    owners: BTreeMap<callisto_model::PackageId, ReleasePackageId>,
}

type Owners = BTreeMap<callisto_model::PackageId, ReleasePackageId>;

/// The selected identities that own their package's shared effects: one tag, forge release and upload set per package.
fn owned<'a, 'w>(
    selected: &'a BTreeMap<ReleasePackageId, (&'w callisto_model::Package, Version)>,
    owners: &'a Owners,
) -> impl Iterator<Item = (&'a ReleasePackageId, &'a (&'w callisto_model::Package, Version))> {
    selected
        .iter()
        .filter(move |(id, (package, _))| owners.get(&package.id) == Some(*id))
}

/// The operations built so far, with the per-package indexes later stages read their prerequisites from.
#[derive(Default)]
struct Ops {
    operations: BTreeMap<ReleaseOperationId, ReleaseOperation>,
    prepared: BTreeMap<ReleaseOperationId, PreparedOperation>,
    index: OpIndex,
}

#[derive(Default)]
struct OpIndex {
    publishes_by_package: BTreeMap<ReleasePackageId, Vec<ReleaseOperationId>>,
    /// Owner registry publish -> its platform publishes, which must land first.
    platforms_by_publish: BTreeMap<ReleaseOperationId, Vec<ReleaseOperationId>>,
    tag_by_package: BTreeMap<ReleasePackageId, ReleaseOperationId>,
    forge_by_package: BTreeMap<ReleasePackageId, ReleaseOperationId>,
    /// Keyed by the product's id: a plugin-built asset must still gate the product's own forge-publish.
    uploads_by_package: BTreeMap<ReleasePackageId, Vec<ReleaseOperationId>>,
}

impl OpIndex {
    /// The registry publishes a package's tag waits for: those of every release identity of the package, since a
    /// dual Cargo and npm owner has one physical tag but a publish per manifest.
    fn tag_prerequisites(&self, owners: &[ReleasePackageId]) -> Vec<ReleaseOperationId> {
        let publishes: BTreeSet<_> = owners
            .iter()
            .flat_map(|owner| self.publishes_by_package.get(owner).into_iter().flatten().cloned())
            .collect();
        publishes.into_iter().collect()
    }
}

fn derive_release_inputs_with<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    decision: &ReleaseDecisionV1,
    source: SourceIdentity,
    artifact_policy: Option<&ArtifactBuildPolicy>,
    remote: GitRemoteRequirement,
    diagnostics: &mut Vec<Diagnostic>,
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
    let Selection {
        selected,
        package_ids,
        owners,
    } = select_packages(workspace, decision)?;
    require_dependencies_selected(workspace, &selected, &package_ids)?;
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

    let git_remote = prepare_remote(workspace, &selected, artifact_policy, remote)?;

    let mut ops = Ops::default();
    build_publish_leaves(workspace, &selected, git_remote.as_ref(), &mut package_inputs, &mut ops)?;
    link_publish_prerequisites(workspace, &selected, &package_ids, &publish_edges, &mut ops)?;
    add_tags(&selected, &package_ids, &owners, &source, &mut ops)?;
    add_forge_releases(workspace, &selected, &owners, &mut ops)?;
    let artifact_slots = add_artifact_uploads(workspace, &selected, &owners, artifact_policy, &mut ops, diagnostics)?;
    add_forge_publishes(&selected, &owners, &mut ops)?;

    Ok((
        ReleaseInputSnapshotV1::new(source, package_inputs)?,
        canonical_operation_order(ops.operations)?,
        ops.prepared,
        git_remote,
        artifact_slots,
    ))
}

fn select_packages<'w, R: CommandRunner, D: DependencyResolver>(
    workspace: &'w Workspace<'_, R, D>,
    decision: &ReleaseDecisionV1,
) -> Result<Selection<'w>, GraphError> {
    let mut selected = BTreeMap::new();
    let mut package_ids = BTreeMap::<callisto_model::PackageId, Vec<ReleasePackageId>>::new();
    let mut owners = Owners::new();
    for package in workspace.graph.packages() {
        let ids = crate::commands::release_decision::release_package_ids(&workspace.identity, package)?;
        if let Some(owner) = ids
            .iter()
            .find(|id| decision.entries.iter().any(|entry| &entry.package == *id))
        {
            owners.insert(package.id.clone(), owner.clone());
        }
        // One canonical manifest per ecosystem is the expected shape, but
        // dedup defensively -- `release_package_ids` maps every canonical
        // manifest, and two manifests sharing an ecosystem would otherwise
        // select/push the same identity twice.
        for id in ids.into_iter().collect::<BTreeSet<_>>() {
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
    Ok(Selection {
        selected,
        package_ids,
        owners,
    })
}

fn prepare_remote<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    selected: &BTreeMap<ReleasePackageId, (&callisto_model::Package, Version)>,
    artifact_policy: Option<&ArtifactBuildPolicy>,
    remote: GitRemoteRequirement,
) -> Result<Option<PreparedGitRemote>, GraphError> {
    let requires_git_remote = selected.values().any(|(package, _)| {
        package
            .publish_to
            .iter()
            .any(|target| !matches!(target, PublishTarget::None))
    });
    let git_remote = match (requires_git_remote, remote) {
        (false, _) => None,
        (true, GitRemoteRequirement::Required) => Some(prepared_git_remote(&workspace.root, workspace.runner)?),
        (true, GitRemoteRequirement::OptionalForPreview) => optional_git_remote(&workspace.root, workspace.runner)?,
    };
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
    Ok(git_remote)
}

/// One registry publish per package target, with its platform publishes, and each package's input fingerprint.
fn build_publish_leaves<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    selected: &BTreeMap<ReleasePackageId, (&callisto_model::Package, Version)>,
    git_remote: Option<&PreparedGitRemote>,
    package_inputs: &mut Vec<ReleasePackageInputV1>,
    ops: &mut Ops,
) -> Result<(), GraphError> {
    let Ops {
        operations,
        prepared,
        index: OpIndex {
            publishes_by_package,
            platforms_by_publish,
            ..
        },
    } = ops;

    // First construct leaves so dependency prerequisites can refer only to
    // exact selected release identities, never PackageId's wildcard matcher.
    for (id, (package, version)) in selected {
        let produces_tag = package
            .publish_to
            .iter()
            .any(|target| !matches!(target, PublishTarget::None));
        let fingerprint = package_fingerprint(
            workspace,
            id,
            package,
            version,
            produces_tag.then_some(git_remote).flatten(),
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
    Ok(())
}

/// Replaces each publish leaf with prerequisites from selected dependency packages.
fn link_publish_prerequisites<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    selected: &BTreeMap<ReleasePackageId, (&callisto_model::Package, Version)>,
    package_ids: &BTreeMap<callisto_model::PackageId, Vec<ReleasePackageId>>,
    publish_edges: &PublishEdgeFilter,
    ops: &mut Ops,
) -> Result<(), GraphError> {
    let Ops {
        operations,
        index: OpIndex {
            publishes_by_package,
            platforms_by_publish,
            ..
        },
        ..
    } = ops;

    // Replace each publish leaf with prerequisites from selected dependency
    // packages, ordered by the shared publish edge filter.
    for (id, (package, _)) in selected {
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
    Ok(())
}

fn add_tags(
    selected: &BTreeMap<ReleasePackageId, (&callisto_model::Package, Version)>,
    package_ids: &BTreeMap<callisto_model::PackageId, Vec<ReleasePackageId>>,
    owners: &Owners,
    source: &SourceIdentity,
    ops: &mut Ops,
) -> Result<(), GraphError> {
    let Ops {
        operations,
        prepared,
        index,
    } = ops;

    for (id, (package, version)) in owned(selected, owners) {
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
            index.tag_prerequisites(
                package_ids
                    .get(&package.id)
                    .map_or(std::slice::from_ref(id), Vec::as_slice),
            ),
        )?;
        let tag_name = package
            .tag_template
            .clone()
            .unwrap_or_else(|| callisto_model::TagTemplate::default_for(&package.id))
            .render(version);
        let target = match source {
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
        index.tag_by_package.insert(id.clone(), tag.id().clone());
        operations.insert(tag.id().clone(), tag);
    }
    Ok(())
}

fn add_forge_releases<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    selected: &BTreeMap<ReleasePackageId, (&callisto_model::Package, Version)>,
    owners: &Owners,
    ops: &mut Ops,
) -> Result<(), GraphError> {
    let Ops {
        operations,
        prepared,
        index: OpIndex {
            tag_by_package,
            forge_by_package,
            ..
        },
        ..
    } = ops;

    for (id, (package, version)) in owned(selected, owners) {
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
                    notes: release_notes(&workspace.root, package.changelog.as_deref(), version)?,
                }),
            );
            forge_by_package.insert(id.clone(), operation.id().clone());
            operations.insert(operation.id().clone(), operation);
        }
    }
    Ok(())
}

/// The artifact slots and their upload operations, with the uploads each product package's forge publish waits on.
fn add_artifact_uploads<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    selected: &BTreeMap<ReleasePackageId, (&callisto_model::Package, Version)>,
    owners: &Owners,
    artifact_policy: Option<&ArtifactBuildPolicy>,
    ops: &mut Ops,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Vec<ArtifactSlotId>, GraphError> {
    let Ops {
        operations,
        prepared,
        index: OpIndex {
            forge_by_package,
            uploads_by_package,
            ..
        },
    } = ops;

    let mut artifact_slots = Vec::new();
    if let (Some(product), Some(policy)) = (&workspace.config.product_release, artifact_policy) {
        require_product_package_publishes_to_forge(workspace, &product.package)?;
        // Resolved through IdentityIndex, not `PackageId::matches`: a Bare
        // package id is not an ecosystem wildcard here -- a same-name
        // package in another ecosystem must never match the product/artifact selector.
        let matches_product =
            |package: &callisto_model::Package| workspace.identity.identifies(&product.package, &package.id);
        if !selected.values().any(|(package, _)| matches_product(package)) {
            // The product isn't in this release, so warn instead of leaving the artifact owner's asset unregistered.
            for artifact in &product.artifacts {
                if let Some((owner_id, _)) = owned(selected, owners)
                    .find(|(_, (pkg, _))| workspace.identity.identifies(&artifact.package, &pkg.id))
                {
                    diagnostics.push(unreleased_product_artifact_diagnostic(
                        &product.package,
                        artifact,
                        owner_id,
                    ));
                }
            }
        }
        for (id, (package, _)) in owned(selected, owners) {
            if !matches_product(package) {
                continue;
            }
            let forge = forge_by_package.get(id).ok_or_else(|| GraphError::ReleaseInvariant {
                detail: format!("product package `{id}` has no forge release operation"),
            })?;
            let (tag, prerelease) = match prepared.get(forge) {
                Some(PreparedOperation::ForgeRelease(prepared_forge)) => {
                    (prepared_forge.tag.clone(), prepared_forge.prerelease)
                }
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
                let (owner_id, owner_version) = owned(selected, owners)
                    .find(|(_, (pkg, _))| workspace.identity.identifies(&artifact.package, &pkg.id))
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
                        prerelease,
                    }),
                );
                // Keyed by the product's id: a plugin-built asset must still gate the product's own forge-publish.
                uploads_by_package
                    .entry(id.clone())
                    .or_default()
                    .push(operation.id().clone());
                operations.insert(operation.id().clone(), operation);
                artifact_slots.push(slot);
            }
        }
    }
    Ok(artifact_slots)
}

fn add_forge_publishes(
    selected: &BTreeMap<ReleasePackageId, (&callisto_model::Package, Version)>,
    owners: &Owners,
    ops: &mut Ops,
) -> Result<(), GraphError> {
    let Ops {
        operations,
        prepared,
        index: OpIndex {
            forge_by_package,
            uploads_by_package,
            ..
        },
    } = ops;

    // Publishing is the last forge step: a GitHub Release is only complete
    // once every asset it carries has been uploaded to it as a draft.
    for (id, (_, version)) in owned(selected, owners) {
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
    Ok(())
}

/// Every unreleased, publishable workspace package a selected package reaches
/// through a runtime, optional, or peer edge must be selected with it, or the
/// selected package would publish pointing at a version that does not exist.
/// Standalone npm platform packages are one case of this.
fn require_dependencies_selected<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    selected: &BTreeMap<ReleasePackageId, (&callisto_model::Package, Version)>,
    package_ids: &BTreeMap<callisto_model::PackageId, Vec<ReleasePackageId>>,
) -> Result<(), GraphError> {
    let mut base_versions = None;
    for (package, _) in selected.values() {
        for edge in workspace.graph.dependencies_of(&package.id) {
            if !matches!(edge.kind, DepKind::Runtime | DepKind::Optional | DepKind::Peer)
                || package_ids.contains_key(&edge.to)
            {
                continue;
            }
            let Some(dependency) = workspace
                .graph
                .packages()
                .find(|candidate| candidate.id == edge.to && has_dispatchable_target(candidate))
            else {
                continue;
            };
            if base_versions.is_none() {
                base_versions = Some(workspace.base_versions()?);
            }
            let tags = workspace.tags()?;
            let released = base_versions
                .as_ref()
                .and_then(|versions| versions.get(&dependency.id))
                .is_some_and(|current| version_is_tagged(tags, &dependency.id, current));
            if !released {
                let id = crate::commands::release_decision::release_package_ids(&workspace.identity, dependency)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| GraphError::ReleaseInvariant {
                        detail: format!("package `{}` has no release identity", dependency.id.display_name()),
                    })?;
                return Err(GraphError::ReleaseSelectionInvalid {
                    package: id,
                    reason: ReleaseSelectionInvalidReason::DependencyNotSelected,
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
        if !workspace.identity.identifies(product, &package.id) {
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

/// Warns that `owner`'s asset has no release to attach to, since its product isn't selected for release.
fn unreleased_product_artifact_diagnostic(
    product: &callisto_model::PackageId,
    artifact: &crate::config::resolve::ProductArtifactConfig,
    owner: &ReleasePackageId,
) -> Diagnostic {
    Diagnostic {
        code: DiagnosticCode::ProductArtifactOwnerWithoutProduct,
        severity: DiagnosticSeverity::Warning,
        message: format!(
            "{owner} is selected but product `{product}` is not; its `{}` asset will not be uploaded",
            artifact.asset_name
        ),
        package: callisto_model::PackageId::parse(&owner.to_string()).ok(),
        path: None,
        escalated_by: None,
        governed_by: None,
    }
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

pub(crate) fn package_dir(package: &callisto_model::Package) -> Result<std::path::PathBuf, GraphError> {
    package
        .key()
        .map(|key| key.as_path().to_path_buf())
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
        let (before_snapshot, before_operations, _, _, _) = derive_release_inputs(
            &workspace,
            &super::super::tests::decision(),
            source.clone(),
            None,
            &mut Vec::new(),
        )
        .unwrap();

        std::fs::write(
            root.join("callisto.toml"),
            "# formatting/comments are not release policy\n[[package]]\nmatch = \"release-fixture\"\npublish-to = [\"crates-io\"]\n",
        )
        .unwrap();
        let reread = Workspace::load(root, &locator, &runner).unwrap();
        let (after_snapshot, after_operations, _, _, _) =
            derive_release_inputs(&reread, &super::super::tests::decision(), source, None, &mut Vec::new()).unwrap();

        assert_eq!(before_snapshot, after_snapshot);
        assert_eq!(before_operations, after_operations);
    }

    /// A `[release]` without a forge destination cannot plan.
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
            derive_release_inputs(
                &workspace,
                &super::super::tests::decision(),
                source,
                None,
                &mut Vec::new()
            ),
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
        callisto_fixtures::git::init_repo(dir.path());
        for args in [
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
        derive_release_inputs(&workspace, decision, source, None, &mut Vec::new())
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

    /// A dev-only cycle orders instead of failing; an unrelated dev edge still orders.
    #[test]
    fn dev_only_cycle_is_ordered_not_a_cycle_error() {
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

    /// A runtime cycle still fails, even with dev edges alongside it.
    #[test]
    fn runtime_cycle_still_fails_with_a_cycle_error() {
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

    /// In a runtime+dev cycle only the dev edge is excluded; the runtime edge orders.
    #[test]
    fn mixed_cycle_excludes_only_the_dev_edge() {
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

    /// Exactly one package input per decision entry; unselected packages never appear.
    #[test]
    fn derivation_neither_adds_nor_drops_selected_packages() {
        let dir = repo_of(&cargo_workspace(&[
            ("a", "[dependencies]\nb = { path = \"../b\" }\n"),
            ("b", ""),
            ("c", ""),
        ]));
        // `b` is already released, so selecting `a` alone is valid.
        git(dir.path(), &["tag", "b@1.0.0"]);
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

    /// An unreleased runtime, optional, or peer dependency must be selected with its dependent.
    #[test]
    fn unselected_unreleased_workspace_dependency_is_rejected() {
        let dir = repo_of(&cargo_workspace(&[
            ("a", "[dependencies]\nb = { path = \"../b\" }\n"),
            ("b", ""),
            ("c", "[dev-dependencies]\nb = { path = \"../b\" }\n"),
        ]));
        let error = derive(&dir, &cargo_release(&["a"])).unwrap_err();
        assert!(
            matches!(
                &error,
                GraphError::ReleaseSelectionInvalid {
                    package,
                    reason: ReleaseSelectionInvalidReason::DependencyNotSelected,
                } if package.name() == "b"
            ),
            "{error:?}"
        );
        assert!(error.to_string().contains("`cargo/b`"), "{error}");
        derive(&dir, &cargo_release(&["a", "b"])).unwrap();
        derive(&dir, &cargo_release(&["c"])).expect("a dev edge never requires selection");

        let peer = repo(&[
            ("pnpm-workspace.yaml", "packages:\n  - \"packages/*\"\n"),
            ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n"),
            (
                "packages/plugin/package.json",
                r#"{"name":"plugin","version":"1.0.0","peerDependencies":{"host":"1.0.0"}}"#,
            ),
            ("packages/host/package.json", r#"{"name":"host","version":"1.0.0"}"#),
            ("callisto.toml", ""),
        ]);
        assert!(matches!(
            derive(&peer, &release(&[(Ecosystem::Npm, "plugin", "1.0.0")])).unwrap_err(),
            GraphError::ReleaseSelectionInvalid {
                reason: ReleaseSelectionInvalidReason::DependencyNotSelected,
                ..
            }
        ));
        git(peer.path(), &["tag", "host@1.0.0"]);
        derive(&peer, &release(&[(Ecosystem::Npm, "plugin", "1.0.0")])).unwrap();
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

    /// An unreleased standalone platform dependency must be selected with its npm owner.
    #[test]
    fn unselected_unreleased_platform_dependency_is_rejected() {
        let dir = npm_platform_repo(false);
        let error = derive(&dir, &release(&[(Ecosystem::Npm, "main", "1.0.0")])).unwrap_err();
        assert!(
            matches!(
                &error,
                GraphError::ReleaseSelectionInvalid {
                    package,
                    reason: ReleaseSelectionInvalidReason::DependencyNotSelected,
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

    /// An already-released platform dependency need not be selected.
    #[test]
    fn released_platform_dependency_need_not_be_selected() {
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

    /// A `publishConfig.registry` not configured in `[registries]` is untrusted.
    #[test]
    fn unapproved_npm_registry_override_is_rejected() {
        let dir = npm_registry_repo("https://npm.evil.example/");
        let error = derive(&dir, &release(&[(Ecosystem::Npm, "lib", "1.0.0")])).unwrap_err();
        assert!(matches!(error, GraphError::UntrustedNpmRegistry { .. }), "{error:?}");
        let approved = npm_registry_repo("https://npm.corp.example/");
        derive(&approved, &release(&[(Ecosystem::Npm, "lib", "1.0.0")])).unwrap();
    }

    /// A cleartext override fails the scheme check before host matching.
    #[test]
    fn non_https_npm_registry_override_is_unsafe_even_on_an_approved_host() {
        let dir = npm_registry_repo("http://npm.corp.example/");
        let error = derive(&dir, &release(&[(Ecosystem::Npm, "lib", "1.0.0")])).unwrap_err();
        assert!(matches!(error, GraphError::UnsafeRegistryBinding { .. }), "{error:?}");
    }

    /// A scoped npm package without explicit access publishes public, for the
    /// owner and its platform prerequisites, and the defaulted value is fingerprinted.
    #[test]
    fn scoped_npm_package_defaults_to_public_access() {
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
            &mut Vec::new(),
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

    /// An undispatchable target fails derivation instead of being skipped.
    #[test]
    fn unimplemented_publish_target_fails_derivation() {
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
        let error =
            derive_release_inputs(&workspace, &cargo_release(&["core"]), source, None, &mut Vec::new()).unwrap_err();
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

    /// The forge release carries the changelog section, and the notes stay
    /// out of the intent: changing them changes neither snapshot nor operations.
    #[test]
    fn forge_release_notes_come_from_the_changelog_and_do_not_affect_the_intent() {
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
            derive_release_inputs(&workspace, &decision, source, None, &mut Vec::new()).unwrap();
        assert_eq!(
            forge_notes(&after_prepared),
            ReleaseNotes::Generated {
                reason: NotesFallback::SectionEmpty
            }
        );
        assert_eq!(snapshot, after_snapshot);
        assert_eq!(operations, after_operations);
    }

    /// core publishes to github-release; plugin owns one `[[release.artifact]]` slot on core's release.
    fn product_and_plugin_repo(core_version: &str, plugin_version: &str) -> tempfile::TempDir {
        repo(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"core\", \"plugin\"]\n"),
            (
                "core/Cargo.toml",
                &format!("[package]\nname = \"core\"\nversion = \"{core_version}\"\nedition = \"2021\"\n"),
            ),
            (
                "plugin/Cargo.toml",
                &format!("[package]\nname = \"plugin\"\nversion = \"{plugin_version}\"\nedition = \"2021\"\n"),
            ),
            (
                "callisto.toml",
                "[release]\nproduct-package = \"cargo/core\"\nforge-repository = \"example/parity\"\n\n\
                 [[release.artifact]]\npackage = \"cargo/plugin\"\ntarget = \"t\"\nasset-name = \"a.tar.gz\"\n\n\
                 [[package]]\nmatch = \"core\"\npublish-to = [\"github-release\"]\n\n\
                 [[package]]\nmatch = \"plugin\"\npublish-to = [\"crates-io\"]\n",
            ),
        ])
    }

    /// Same derivation as `derive`, but with the `ArtifactBuildPolicy` a real release run would build.
    fn derive_with_artifact_policy(
        dir: &tempfile::TempDir,
        decision: &ReleaseDecisionV1,
    ) -> Result<(DerivedReleaseInputs, Vec<Diagnostic>), GraphError> {
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let root = super::super::capability::canonical_root(dir.path()).unwrap();
        let workspace = Workspace::load(root, &locator, &RealGitRunner).unwrap();
        let source = super::super::capability::observe_source(
            &workspace,
            ExecutionTrustProfileV1::GitCommit,
            super::super::capability::ReleaseCheckout::Detached,
        )
        .unwrap();
        let sha = match &source {
            SourceIdentity::GitCommit { sha } => sha.clone(),
            SourceIdentity::HermeticContent { .. } => unreachable!("fixture always observes a git commit"),
        };
        let policy = ArtifactBuildPolicy {
            repository: workspace
                .config
                .product_release
                .as_ref()
                .unwrap()
                .forge_repository
                .clone()
                .unwrap(),
            workflow_path: callisto_model::RELEASE_COORDINATOR_WORKFLOW_PATH.to_owned(),
            workflow_commit: sha,
        };
        let mut diagnostics = Vec::new();
        let inputs = derive_release_inputs(&workspace, decision, source, Some(&policy), &mut diagnostics)?;
        Ok((inputs, diagnostics))
    }

    fn artifact_upload(prepared: &BTreeMap<ReleaseOperationId, PreparedOperation>) -> &ArtifactUploadOperation {
        prepared
            .values()
            .find_map(|op| match op {
                PreparedOperation::ArtifactUpload(upload) => Some(upload),
                _ => None,
            })
            .expect("no artifact upload operation was prepared")
    }

    fn forge_publish_operation<'a>(operations: &'a [ReleaseOperation], package: &str) -> &'a ReleaseOperation {
        operations
            .iter()
            .find(|operation| {
                matches!(operation.id().role, callisto_model::ReleaseOperationRole::ForgePublish)
                    && operation.id().package.name() == package
            })
            .unwrap_or_else(|| panic!("no forge publish for {package}"))
    }

    /// A slot owned by a different package than the product must still gate the product's forge-publish.
    #[test]
    fn plugin_owned_artifact_gates_product_forge_publish() {
        let dir = product_and_plugin_repo("1.0.0", "2.0.0");
        let decision = cargo_release(&["core", "plugin"]);
        let ((_, operations, prepared, _, slots), _) = derive_with_artifact_policy(&dir, &decision).unwrap();
        assert_eq!(slots.len(), 1, "one configured artifact slot");

        let upload_id = prepared
            .iter()
            .find_map(|(id, op)| matches!(op, PreparedOperation::ArtifactUpload(_)).then(|| id.clone()))
            .expect("an artifact upload operation was prepared");

        let core_publish = forge_publish_operation(&operations, "core");
        assert!(
            core_publish.prerequisites().contains(&upload_id),
            "core's forge-publish must depend on plugin's artifact upload, not just its own forge-release; \
             prerequisites were {:?}",
            core_publish.prerequisites()
        );
    }

    /// The product's release stability must set the upload's prerelease flag, not the owning package's, or
    /// `forge.rs::publish` rejects the mismatch (E167).
    #[test]
    fn artifact_upload_prerelease_flag_follows_the_product_not_the_owner() {
        let dir = product_and_plugin_repo("1.0.0", "2.0.0-beta.1");
        // `cargo_release` targets every package at 1.0.0; build the decision directly for plugin's own version.
        let decision = release(&[
            (Ecosystem::Cargo, "core", "1.0.0"),
            (Ecosystem::Cargo, "plugin", "2.0.0-beta.1"),
        ]);
        let ((_, _, prepared, _, _), _) = derive_with_artifact_policy(&dir, &decision).unwrap();
        let upload = artifact_upload(&prepared);
        assert!(
            !upload.prerelease,
            "core (the product) released 1.0.0, a stable version, so its artifact upload must not be \
             flagged prerelease even though plugin (the asset's owner) released a prerelease version"
        );
    }

    /// Derivation must still succeed without registering a dangling slot, and must report the
    /// condition as a diagnostic (reaching `--format json`) rather than only an `eprintln!`.
    #[test]
    fn owner_selected_without_product_derives_no_slot_and_diagnoses_it() {
        let dir = product_and_plugin_repo("1.0.0", "2.0.0");
        let decision = cargo_release(&["plugin"]);
        let ((_, _, prepared, _, slots), diagnostics) = derive_with_artifact_policy(&dir, &decision).unwrap();
        assert!(
            slots.is_empty(),
            "no slot should be derived when the product isn't released"
        );
        assert!(
            !prepared
                .values()
                .any(|op| matches!(op, PreparedOperation::ArtifactUpload(_))),
            "no artifact upload operation should be prepared when the product isn't released"
        );
        assert_eq!(diagnostics.len(), 1, "got: {diagnostics:?}");
        assert_eq!(diagnostics[0].code, DiagnosticCode::ProductArtifactOwnerWithoutProduct);
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Warning);
    }

    /// Unit coverage for the diagnostic's message text and package field.
    #[test]
    fn unreleased_product_artifact_diagnostic_names_owner_product_and_asset() {
        let product = callisto_model::PackageId::parse("cargo/core").unwrap();
        let artifact = crate::config::resolve::ProductArtifactConfig {
            package: callisto_model::PackageId::parse("cargo/plugin").unwrap(),
            target: "t".to_owned(),
            asset_name: "a.tar.gz".to_owned(),
        };
        let owner = ReleasePackageId::new(Ecosystem::Cargo, "plugin").unwrap();
        let diagnostic = unreleased_product_artifact_diagnostic(&product, &artifact, &owner);
        assert_eq!(
            diagnostic.message,
            "cargo/plugin is selected but product `cargo/core` is not; its `a.tar.gz` asset will not be uploaded"
        );
        assert_eq!(
            diagnostic.package,
            callisto_model::PackageId::parse("cargo/plugin").ok()
        );
    }

    /// Everything derivation produces, rendered stably: the intent digest covers the operations, so
    /// their identities, order and prerequisites must not move. Commit hashes and the checkout
    /// path are redacted.
    fn derived_text(dir: &tempfile::TempDir, decision: &ReleaseDecisionV1, artifact_policy: bool) -> String {
        let (snapshot, operations, prepared, _, slots) = if artifact_policy {
            derive_with_artifact_policy(dir, decision).unwrap().0
        } else {
            derive(dir, decision).unwrap()
        };
        let text = format!(
            "packages:\n{:#?}\n\noperations:\n{:#?}\n\nprepared:\n{prepared:#?}\n\nslots:\n{slots:#?}\n",
            snapshot.packages, operations
        );
        let root = super::super::capability::canonical_root(dir.path()).unwrap();
        let text = text.replace(root.to_str().unwrap(), "<root>");
        let mut redacted = String::with_capacity(text.len());
        let mut run = String::new();
        for character in text.chars().chain(std::iter::once('\0')) {
            if character.is_ascii_hexdigit() {
                run.push(character);
                continue;
            }
            redacted.push_str(if run.len() == 40 { "<commit>" } else { &run });
            run.clear();
            if character != '\0' {
                redacted.push(character);
            }
        }
        redacted
    }

    #[test]
    fn a_decision_entry_the_workspace_does_not_contain_is_not_selected() {
        let dir = repo_of(&cargo_workspace(&[("a", "")]));
        let error = derive(&dir, &cargo_release(&["a", "ghost"])).unwrap_err();
        assert!(
            matches!(&error, GraphError::ReleasePackageNotSelected { package } if package.name() == "ghost"),
            "{error:?}"
        );
    }

    #[test]
    fn a_product_without_its_artifact_owner_cannot_derive_slots() {
        let dir = product_and_plugin_repo("1.0.0", "2.0.0");
        let error = derive_with_artifact_policy(&dir, &cargo_release(&["core"])).unwrap_err();
        assert!(
            matches!(&error, GraphError::ReleaseArtifactOwnerNotReleased { asset, .. } if asset == "a.tar.gz"),
            "{error:?}"
        );
    }

    #[test]
    fn an_artifact_policy_for_another_repository_than_origin_is_refused() {
        let dir = product_and_plugin_repo("1.0.0", "2.0.0");
        let config = std::fs::read_to_string(dir.path().join("callisto.toml")).unwrap();
        std::fs::write(
            dir.path().join("callisto.toml"),
            config.replace("example/parity", "example/elsewhere"),
        )
        .unwrap();
        git(dir.path(), &["commit", "-qam", "retarget the forge"]);
        let error = derive_with_artifact_policy(&dir, &cargo_release(&["core", "plugin"])).unwrap_err();
        assert!(
            matches!(error, GraphError::ReleaseArtifactRepositoryMismatch { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn two_targets_for_one_registry_are_a_duplicate() {
        let mut files = cargo_workspace(&[("a", "")]);
        files[1].1 = "[[package]]\nmatch = \"a\"\npublish-to = [\"crates-io\", \"crates-io\"]\n".to_owned();
        let dir = repo_of(&files);
        let error = derive(&dir, &cargo_release(&["a"])).unwrap_err();
        assert!(
            matches!(
                error,
                GraphError::ReleaseSelectionInvalid {
                    reason: ReleaseSelectionInvalidReason::DuplicateRegistryTarget,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn derived_cargo_chain_is_pinned() {
        let dir = repo_of(&cargo_workspace(&[
            ("a", "[dependencies]\nb = { path = \"../b\" }\n"),
            ("b", ""),
            ("c", "[dev-dependencies]\na = { path = \"../a\" }\n"),
        ]));
        insta::assert_snapshot!(derived_text(&dir, &cargo_release(&["a", "b", "c"]), false));
    }

    #[test]
    fn derived_product_release_with_a_plugin_artifact_is_pinned() {
        let dir = product_and_plugin_repo("1.0.0", "2.0.0");
        insta::assert_snapshot!(derived_text(&dir, &cargo_release(&["core", "plugin"]), true));
    }

    #[test]
    fn derived_npm_owner_with_a_platform_package_is_pinned() {
        let dir = repo(&[
            ("pnpm-workspace.yaml", "packages:\n  - \"packages/*\"\n"),
            ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n"),
            (
                "packages/cli/package.json",
                r#"{"name":"@s/cli","version":"0.1.0","publishConfig":{"access":"public"},"optionalDependencies":{"@s/cli-darwin-arm64":"0.1.0","@s/cli-linux-x64-gnu":"0.1.0"}}"#,
            ),
            (
                "packages/cli/npm/darwin-arm64/package.json",
                r#"{"name":"@s/cli-darwin-arm64","version":"0.1.0","os":["darwin"],"cpu":["arm64"]}"#,
            ),
            (
                "packages/cli/npm/linux-x64-gnu/package.json",
                r#"{"name":"@s/cli-linux-x64-gnu","version":"0.1.0","os":["linux"],"cpu":["x64"]}"#,
            ),
            ("callisto.toml", ""),
        ]);
        let decision = release(&[(Ecosystem::Npm, "@s/cli", "0.1.0")]);
        insta::assert_snapshot!(derived_text(&dir, &decision, false));
    }

    #[test]
    fn derived_scoped_npm_prerelease_is_pinned() {
        let dir = repo(&[
            ("package.json", r#"{"name":"@s/lib","version":"1.0.0-beta.1"}"#),
            ("callisto.toml", ""),
        ]);
        insta::assert_snapshot!(derived_text(
            &dir,
            &release(&[(Ecosystem::Npm, "@s/lib", "1.0.0-beta.1")]),
            false
        ));
    }

    /// One directory with both a Cargo and an npm manifest, released to both registries and the forge.
    fn dual_owner_repo() -> tempfile::TempDir {
        repo(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"dual\"]\n"),
            (
                "dual/Cargo.toml",
                "[package]\nname = \"dual\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
            ),
            ("dual/package.json", r#"{"name":"dual","version":"1.0.0"}"#),
            (
                "callisto.toml",
                "[[package]]\nmatch = \"dual\"\npublish-to = [\"crates-io\", \"npm\", \"github-release\"]\n",
            ),
        ])
    }

    fn dual_owner_decision() -> ReleaseDecisionV1 {
        release(&[(Ecosystem::Cargo, "dual", "1.0.0"), (Ecosystem::Npm, "dual", "1.0.0")])
    }

    /// The dual owner's operations are shared between its two registry publishes.
    #[test]
    fn derived_dual_cargo_and_npm_owner_is_pinned() {
        let dir = dual_owner_repo();
        insta::assert_snapshot!(derived_text(&dir, &dual_owner_decision(), false));
    }

    /// The owner's one physical tag and GitHub release must wait for every registry publish of the package, not
    /// just the publish of the manifest that happens to carry the operation's identity.
    #[test]
    fn a_dual_owners_shared_tag_and_release_wait_for_both_registry_publishes() {
        use callisto_model::ReleaseOperationRole::{ForgePublish, ForgeRelease, RegistryPublish, Tag};

        let dir = dual_owner_repo();
        let decision = dual_owner_decision();
        let (snapshot, operations, _, _, slots) = derive(&dir, &decision).unwrap();
        let intent = ReleaseIntentV1::new(
            decision,
            snapshot,
            ExecutionTrustProfileV1::GitCommit,
            operations,
            slots,
        )
        .unwrap();
        let publish_of = |ecosystem| {
            intent
                .operations
                .iter()
                .map(|operation| operation.id().clone())
                .find(|id| matches!(id.role, RegistryPublish { .. }) && id.package.ecosystem() == ecosystem)
                .unwrap_or_else(|| panic!("no {ecosystem:?} registry publish"))
        };
        let (cargo, npm) = (publish_of(Ecosystem::Cargo), publish_of(Ecosystem::Npm));

        for (landed, pending) in [(&cargo, &npm), (&npm, &cargo)] {
            let mut state = super::super::super::release_test_support::pending_state(&intent);
            super::super::super::release_test_support::publish_operation(&mut state, landed);
            let eligible = crate::commands::release_execution::eligible_operations(&intent, &state);
            assert!(
                !eligible
                    .iter()
                    .any(|id| matches!(id.role, Tag | ForgeRelease | ForgePublish)),
                "{landed:?} landed but {pending:?} has not, yet the shared tag or release is eligible: {eligible:?}"
            );
        }

        let mut state = super::super::super::release_test_support::pending_state(&intent);
        super::super::super::release_test_support::publish_operation(&mut state, &cargo);
        super::super::super::release_test_support::publish_operation(&mut state, &npm);
        let eligible = crate::commands::release_execution::eligible_operations(&intent, &state);
        assert!(
            eligible.iter().any(|id| matches!(id.role, Tag)),
            "both publishes landed, so the tag is next: {eligible:?}"
        );
    }

    fn operations_of(
        operations: &[ReleaseOperation],
        role: callisto_model::ReleaseOperationRole,
    ) -> Vec<&ReleaseOperation> {
        operations
            .iter()
            .filter(|operation| operation.id().role == role)
            .collect()
    }

    /// A dual package has one physical tag and one GitHub release, so one operation of each, on the owner.
    #[test]
    fn a_dual_owner_has_one_tag_and_one_forge_release_and_publish() {
        use callisto_model::ReleaseOperationRole::{ForgePublish, ForgeRelease, Tag};

        let (_, operations, _, _, _) = derive(&dual_owner_repo(), &dual_owner_decision()).unwrap();
        for role in [Tag, ForgeRelease, ForgePublish] {
            let found = operations_of(&operations, role.clone());
            assert_eq!(found.len(), 1, "{role:?}: {found:?}");
            assert_eq!(found[0].id().package.ecosystem(), Ecosystem::Cargo, "{role:?}");
        }
    }

    /// Selecting only the npm identity makes npm the owner, so the tag is not keyed by a package the run omits.
    #[test]
    fn a_partial_dual_selection_is_owned_by_the_selected_identity() {
        use callisto_model::ReleaseOperationRole::{ForgePublish, ForgeRelease, Tag};

        let decision = release(&[(Ecosystem::Npm, "dual", "1.0.0")]);
        let (_, operations, _, _, _) = derive(&dual_owner_repo(), &decision).unwrap();
        for role in [Tag, ForgeRelease, ForgePublish] {
            let found = operations_of(&operations, role.clone());
            assert_eq!(found.len(), 1, "{role:?}: {found:?}");
            assert_eq!(found[0].id().package.ecosystem(), Ecosystem::Npm, "{role:?}");
        }
    }

    /// A dual product's one release carries each configured asset once.
    #[test]
    fn a_dual_product_with_an_artifact_derives_one_upload() {
        let dir = repo(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"dual\"]\n"),
            (
                "dual/Cargo.toml",
                "[package]\nname = \"dual\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
            ),
            ("dual/package.json", r#"{"name":"dual","version":"1.0.0"}"#),
            (
                "callisto.toml",
                "[release]\nproduct-package = \"cargo/dual\"\nforge-repository = \"example/parity\"\n\n\
                 [[release.artifact]]\npackage = \"cargo/dual\"\ntarget = \"t\"\nasset-name = \"a.tar.gz\"\n\n\
                 [[package]]\nmatch = \"dual\"\npublish-to = [\"crates-io\", \"npm\", \"github-release\"]\n",
            ),
        ]);
        let ((_, operations, _, _, slots), _) = derive_with_artifact_policy(&dir, &dual_owner_decision()).unwrap();
        assert_eq!(slots.len(), 1, "{slots:?}");
        let uploads: Vec<_> = operations
            .iter()
            .filter(|operation| {
                matches!(
                    operation.id().role,
                    callisto_model::ReleaseOperationRole::ArtifactUpload { .. }
                )
            })
            .collect();
        assert_eq!(uploads.len(), 1, "{uploads:?}");
        let publish = forge_publish_operation(&operations, "dual");
        assert!(
            publish.prerequisites().contains(uploads[0].id()),
            "the forge publish waits for the upload: {publish:?}"
        );
    }
}
