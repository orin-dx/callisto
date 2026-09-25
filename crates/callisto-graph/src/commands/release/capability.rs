//! The validated authorization capability and the staleness rules behind it.

use std::collections::BTreeMap;
use std::path::Path;

use callisto_model::{
    AbsentProof, ApplyPermit, CommandRunner, ExactEvidence, ExecutionTrustProfileV1, ProviderObservationV1,
    ReleaseDecisionV1, ReleaseIntentError, ReleaseIntentV1, ReleaseOperationId, SourceIdentity,
};
use callisto_vcs::access::{GitCommitTrustEvidence, GitHeadDisposition};

use crate::error::ReleasePreconditionRequirement;
use crate::{DependencyResolver, GraphError, ProjectLocator, Workspace};

use super::binding::{recheck_git_remote, PreparedGitRemote};
use super::derive::{
    artifact_policy_from_intent, derive_release_intent, derive_release_intent_with_prepared, ArtifactBuildPolicy,
    GitRemoteRequirement,
};
use super::provider::{
    checked_provider_for, EffectAuthorization, PreparedOperation, ProviderContext, ProviderRequest, ReleasePreflight,
    ReleaseProviderSet,
};
use super::release_artifacts::VerifiedArtifactManifest;
use super::StaleReason;

/// Graph-private inputs prepared from the same fresh observation as an intent.
/// Retaining them in the capability prevents a validated public intent being
/// paired with new inputs.
#[derive(Debug)]
pub(crate) struct PreparedReleaseInputs {
    pub(crate) root: std::path::PathBuf,
    pub(crate) source: SourceIdentity,
    trust: GitCommitTrustEvidence,
    git_remote: Option<PreparedGitRemote>,
    pub(crate) operations: BTreeMap<ReleaseOperationId, PreparedOperation>,
}

/// In-memory, non-transferable proof that an intent was rebuilt from a fresh
/// workspace observation. It is intentionally neither cloneable nor serializable.
pub struct ValidatedReleaseIntent<'a> {
    intent: ReleaseIntentV1,
    prepared: PreparedReleaseInputs,
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

    #[cfg(test)]
    pub(crate) fn prepared(&self) -> &PreparedReleaseInputs {
        &self.prepared
    }

    fn context(&self) -> ProviderContext<'_> {
        ProviderContext::new(&self.prepared.root, self.runner, self.prepared.git_remote.as_ref())
    }

    fn prepared_operation(&self, id: &ReleaseOperationId) -> Result<&PreparedOperation, GraphError> {
        self.prepared
            .operations
            .get(id)
            .ok_or_else(|| GraphError::ReleaseInvariant {
                detail: format!("no prepared operation for `{id:?}`"),
            })
    }
}

/// The production provider set: every operation is routed to the adapter
/// prepared for it during fresh validation, so no caller-supplied path,
/// endpoint, tag, package name, or version can reach a provider.
impl ReleaseProviderSet for ValidatedReleaseIntent<'_> {
    fn intent(&self) -> &ReleaseIntentV1 {
        &self.intent
    }

    fn recheck_trust(&self) -> Result<(), GraphError> {
        let evidence =
            callisto_vcs::GitAccess::discover(&self.prepared.root, self.runner).observe_git_commit_trust()?;
        if evidence.identity() != self.prepared.trust.identity() || source_from_trust(&evidence) != self.prepared.source
        {
            return Err(GraphError::ReleaseIntentStale {
                reason: StaleReason::trust_evidence_changed(),
            });
        }
        // An unreadable remote propagates as itself; only a readable, different remote is "stale".
        if let Some(expected) = self.prepared.git_remote.as_ref() {
            recheck_git_remote(&self.prepared.root, self.runner, expected)?;
        }
        Ok(())
    }

    fn preflight(
        &self,
        id: &ReleaseOperationId,
        artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<ReleasePreflight, GraphError> {
        let operation = self.prepared_operation(id)?;
        checked_provider_for(operation)?.preflight(
            &self.context(),
            &ProviderRequest {
                id,
                operation,
                artifacts,
            },
        )
    }

    fn observe(
        &self,
        id: &ReleaseOperationId,
        artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<ProviderObservationV1, GraphError> {
        let operation = self.prepared_operation(id)?;
        checked_provider_for(operation)?.observe(
            &self.context(),
            &ProviderRequest {
                id,
                operation,
                artifacts,
            },
        )
    }

    fn publish(
        &self,
        permit: &ApplyPermit,
        proof: &AbsentProof,
        id: &ReleaseOperationId,
        artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<ExactEvidence, GraphError> {
        let operation = self.prepared_operation(id)?;
        checked_provider_for(operation)?.publish(
            &self.context(),
            &ProviderRequest {
                id,
                operation,
                artifacts,
            },
            &EffectAuthorization { permit, proof },
        )
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
    let source = observe_source(&workspace, trust_profile, ReleaseCheckout::Detached)?;
    let intent = derive_release_intent(
        &workspace,
        decision,
        source.clone(),
        trust_profile,
        None,
        GitRemoteRequirement::Required,
    )?;

    // Recheck after all input reads. A concurrent edit or checkout cannot be
    // authorized merely because it happened after the first check.
    if observe_source(&workspace, trust_profile, ReleaseCheckout::Detached)? != source {
        return Err(GraphError::ReleaseIntentStale {
            reason: StaleReason::source_identity_changed(),
        });
    }
    Ok(intent)
}

/// Builds an intent whose declared binary slots are bound to the current
/// coordinator workflow. Callers recovering an old release source must pass
/// the current coordinator revision here, never substitute the source SHA.
pub fn build_release_intent_with_artifacts<L: ProjectLocator, R: CommandRunner>(
    root: &Path,
    locator: &L,
    runner: &R,
    decision: &ReleaseDecisionV1,
    trust_profile: ExecutionTrustProfileV1,
    artifact_policy: ArtifactBuildPolicy,
) -> Result<ReleaseIntentV1, GraphError> {
    let root = canonical_root(root)?;
    let workspace = Workspace::load(root.clone(), locator, runner)?;
    let source = observe_source(&workspace, trust_profile, ReleaseCheckout::Detached)?;
    let intent = derive_release_intent(
        &workspace,
        decision,
        source.clone(),
        trust_profile,
        Some(&artifact_policy),
        GitRemoteRequirement::Required,
    )?;
    if observe_source(&workspace, trust_profile, ReleaseCheckout::Detached)? != source {
        return Err(GraphError::ReleaseIntentStale {
            reason: StaleReason::source_identity_changed(),
        });
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
    validate_intent(root, locator, runner, received, ReleaseCheckout::Detached)
}

/// [`validate_release_intent`] for a local `callisto release`, which may run on any branch.
pub fn validate_local_release_intent<'a, L: ProjectLocator, R: CommandRunner>(
    root: &Path,
    locator: &L,
    runner: &'a R,
    received: ReleaseIntentV1,
) -> Result<ValidatedReleaseIntent<'a>, GraphError> {
    validate_intent(root, locator, runner, received, ReleaseCheckout::AnyHead)
}

fn validate_intent<'a, L: ProjectLocator, R: CommandRunner>(
    root: &Path,
    locator: &L,
    runner: &'a R,
    received: ReleaseIntentV1,
    checkout: ReleaseCheckout,
) -> Result<ValidatedReleaseIntent<'a>, GraphError> {
    let root = canonical_root(root)?;
    let workspace = Workspace::load(root.clone(), locator, runner)?;
    let trust = observe_git_trust(&workspace, received.trust_profile, checkout)?;
    let source = source_from_trust(&trust);
    let artifact_policy = artifact_policy_from_intent(&received)?;
    let (expected, prepared) = derive_release_intent_with_prepared(
        &workspace,
        &received.decision,
        source.clone(),
        received.trust_profile,
        artifact_policy.as_ref(),
    )?;
    let final_trust = observe_git_trust(&workspace, received.trust_profile, checkout)?;
    if expected != received || final_trust.identity() != trust.identity() {
        return Err(GraphError::ReleaseIntentStale {
            reason: StaleReason::intent_differs_from_fresh_derivation(),
        });
    }

    // Plan time is the last point before any effect at which an unusable
    // provider is still cheap to reject.
    for operation in prepared.operations.values() {
        checked_provider_for(operation)?;
    }

    Ok(ValidatedReleaseIntent {
        prepared: PreparedReleaseInputs {
            root,
            source,
            trust: final_trust,
            git_remote: prepared.git_remote,
            operations: prepared.operations,
        },
        intent: received,
        runner,
    })
}

pub(crate) fn canonical_root(root: &Path) -> Result<std::path::PathBuf, GraphError> {
    dunce::canonicalize(root).map_err(|error| GraphError::ReleaseInputRead {
        path: root.to_path_buf(),
        message: error.to_string(),
    })
}

/// Which checkout a release may run from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReleaseCheckout {
    /// The exact detached commit a CI intent is planned and executed from.
    Detached,
    /// A local checkout on any branch; the source is HEAD's commit.
    AnyHead,
}

pub(crate) fn observe_source<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    trust_profile: ExecutionTrustProfileV1,
    checkout: ReleaseCheckout,
) -> Result<SourceIdentity, GraphError> {
    Ok(source_from_trust(&observe_git_trust(
        workspace,
        trust_profile,
        checkout,
    )?))
}

fn observe_git_trust<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    trust_profile: ExecutionTrustProfileV1,
    checkout: ReleaseCheckout,
) -> Result<GitCommitTrustEvidence, GraphError> {
    if !matches!(trust_profile, ExecutionTrustProfileV1::GitCommit) {
        return Err(ReleaseIntentError::UnsupportedTrustProfile.into());
    }
    let evidence = workspace.git_access().observe_git_commit_trust()?;
    if evidence.canonical_root() != workspace.root {
        return Err(GraphError::ReleasePreconditionUnmet {
            requirement: ReleasePreconditionRequirement::CanonicalRootMatchesWorkspace,
        });
    }
    if checkout == ReleaseCheckout::Detached && evidence.head_disposition() != GitHeadDisposition::Detached {
        return Err(GraphError::ReleasePreconditionUnmet {
            requirement: ReleasePreconditionRequirement::DetachedHead,
        });
    }
    Ok(evidence)
}

fn source_from_trust(evidence: &GitCommitTrustEvidence) -> SourceIdentity {
    SourceIdentity::GitCommit {
        sha: evidence.head().clone(),
    }
}
