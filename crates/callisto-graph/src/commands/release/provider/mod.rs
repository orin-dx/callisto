//! The release provider port.
//!
//! Every remote fact and every remote effect in the release path goes through
//! one of these providers. The executor talks only to [`ReleaseProviderSet`],
//! so an in-memory set can drive it without a real registry, remote, or forge;
//! [`super::ValidatedReleaseIntent`] supplies the production implementation.

use std::path::{Path, PathBuf};

use callisto_model::{
    AbsentProof, ApplyPermit, ArtifactSlotId, CommandRunner, CommitSha, ExactEvidence, NpmAccess,
    ProviderObservationV1, ReleaseIntentV1, ReleaseOperationId, TagName, Version,
};

use crate::error::{ReleasePreconditionRequirement, RemoteConflict};
use crate::GraphError;

use super::binding::{recheck_git_remote, PreparedGitRemote, PreparedRegistryBinding};
use super::release_artifacts::VerifiedArtifactManifest;

pub(crate) mod artifact;
pub(crate) mod forge;
pub(crate) mod http;
pub(crate) mod policy;
pub(crate) mod registry;
pub(crate) mod tag;

/// The loopback HTTP server the protocol-level observation tests run against,
/// shared verbatim with the CLI end-to-end harness so both exercise one server.
#[cfg(test)]
#[path = "../../../../../../testing/loopback_http.rs"]
pub(crate) mod loopback;

/// The invocation data for one effect. This is deliberately graph-private:
/// callers can inspect the serializable intent, but cannot substitute a new
/// endpoint, tag target, or package directory at execution time.
#[derive(Debug)]
pub(crate) enum PreparedOperation {
    RegistryPublish(RegistryPublishOperation),
    Tag(TagOperation),
    ForgeRelease(ForgeReleaseOperation),
    ArtifactUpload(ArtifactUploadOperation),
}

#[derive(Debug)]
pub(crate) struct RegistryPublishOperation {
    pub(crate) package_dir: PathBuf,
    pub(crate) package_name: String,
    pub(crate) version: Version,
    pub(crate) registry: PreparedRegistryBinding,
    pub(crate) npm_access: Option<NpmAccess>,
    pub(crate) npm_tag: Option<String>,
}

#[derive(Debug)]
pub(crate) struct TagOperation {
    pub(crate) name: TagName,
    pub(crate) target: CommitSha,
    pub(crate) annotation: String,
}

#[derive(Debug)]
pub(crate) struct ForgeReleaseOperation {
    pub(crate) tag: TagName,
}

#[derive(Debug)]
pub(crate) struct ArtifactUploadOperation {
    pub(crate) slot: ArtifactSlotId,
    pub(crate) tag: TagName,
}

/// What one pre-effect observation authorizes for an operation, carrying the
/// proof token the state machine requires to act on it.
#[derive(Debug)]
pub enum ReleasePreflight {
    /// The effect has not happened and may be issued.
    Proceed { proof: AbsentProof },
    /// The provider already holds exactly this operation's intended result.
    AlreadySatisfied { evidence: ExactEvidence },
}

/// The executor's whole view of the outside world.
///
/// Keeping it this small is deliberate: a test set holding nothing but a map
/// of canned observations satisfies it, which is what makes the executor's
/// crash and convergence paths testable without a provider on `PATH`.
pub trait ReleaseProviderSet {
    /// The immutable intent every operation id must belong to.
    fn intent(&self) -> &ReleaseIntentV1;

    /// Re-observes the held workspace immediately before an effect.
    fn recheck_trust(&self) -> Result<(), GraphError>;

    /// The single fresh pre-effect observation for one operation, made before
    /// the executor persists `Attempting` so a failure here cannot strand the
    /// operation mid-flight.
    fn preflight(
        &self,
        id: &ReleaseOperationId,
        artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<ReleasePreflight, GraphError>;

    /// A fresh observation of what the provider holds, with no effect.
    fn observe(
        &self,
        id: &ReleaseOperationId,
        artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<ProviderObservationV1, GraphError>;

    /// Issues the one mutating action for this operation and confirms it.
    /// `proof` is only obtainable from an `Absent` pre-effect observation.
    fn publish(
        &self,
        permit: &ApplyPermit,
        proof: &AbsentProof,
        id: &ReleaseOperationId,
        artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<ExactEvidence, GraphError>;
}

/// What one provider role can do.
///
/// `can_observe` means the role answers an observation at all, not that the
/// answer can be exact: PyPI answers `Indeterminate` because its upload
/// endpoint is not a query API, and that is still an answer the executor can
/// fail closed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProviderCapabilities {
    pub(crate) can_observe: bool,
    pub(crate) can_publish: bool,
}

/// The permit and the absence proof that together authorize one effect.
pub(crate) struct EffectAuthorization<'a> {
    pub(crate) permit: &'a ApplyPermit,
    #[allow(dead_code)] // a type-level token: holding it is the authorization
    pub(crate) proof: &'a AbsentProof,
}

/// One operation's whole provider-visible input.
pub(crate) struct ProviderRequest<'a> {
    pub(crate) id: &'a ReleaseOperationId,
    pub(crate) operation: &'a PreparedOperation,
    pub(crate) artifacts: Option<&'a VerifiedArtifactManifest<'a>>,
}

/// Everything a provider may touch outside its prepared operation.
pub(crate) struct ProviderContext<'a> {
    root: &'a Path,
    runner: &'a dyn CommandRunner,
    git_remote: Option<&'a PreparedGitRemote>,
    sleeper: &'a dyn policy::Sleeper,
}

impl<'a> ProviderContext<'a> {
    pub(crate) fn new(
        root: &'a Path,
        runner: &'a dyn CommandRunner,
        git_remote: Option<&'a PreparedGitRemote>,
    ) -> Self {
        ProviderContext {
            root,
            runner,
            git_remote,
            sleeper: &policy::ThreadSleeper,
        }
    }

    /// Replaces the wall-clock wait, so a test can exercise the retry schedule
    /// without spending it.
    #[cfg(test)]
    pub(crate) fn with_sleeper(mut self, sleeper: &'a dyn policy::Sleeper) -> Self {
        self.sleeper = sleeper;
        self
    }

    pub(crate) fn root(&self) -> &'a Path {
        self.root
    }

    pub(crate) fn runner(&self) -> &'a dyn CommandRunner {
        self.runner
    }

    pub(crate) fn sleeper(&self) -> &'a dyn policy::Sleeper {
        self.sleeper
    }

    pub(crate) fn checked_git_remote(&self) -> Result<&'a PreparedGitRemote, GraphError> {
        let expected = self.git_remote.ok_or(GraphError::ReleasePreconditionUnmet {
            requirement: ReleasePreconditionRequirement::GitRemotePrepared,
        })?;
        recheck_git_remote(self.root, self.runner, expected)?;
        Ok(expected)
    }

    /// The checked remote's `owner/repo` slug.
    pub(crate) fn github_repository_slug(&self) -> Result<String, GraphError> {
        Ok(self
            .checked_git_remote()?
            .github_repository
            .as_ref()
            .ok_or(GraphError::ReleasePreconditionUnmet {
                requirement: ReleasePreconditionRequirement::GitHubRemote,
            })?
            .as_slug())
    }
}

/// One operation role's adapter: what it can do, what it sees, what it does.
pub(crate) trait ReleaseProvider {
    fn capabilities(&self) -> ProviderCapabilities;

    fn observe(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
    ) -> Result<ProviderObservationV1, GraphError>;

    /// Issues the one mutating action, then re-observes: the returned evidence
    /// is always a post-effect observation, never the client's own exit code.
    fn publish(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
        effect: &EffectAuthorization<'_>,
    ) -> Result<ExactEvidence, GraphError>;

    /// The disagreement this role reports when a pre-effect observation is
    /// neither absent nor exactly the intended result.
    fn preflight_conflict(&self) -> RemoteConflict;

    fn preflight(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
    ) -> Result<ReleasePreflight, GraphError> {
        preflight_from_observation(self.observe(context, request)?, request.id, self.preflight_conflict())
    }
}

/// The one place a prepared operation is routed to the provider that performs
/// it. Roles are selected here; the registry role selects its per-ecosystem
/// adapter in [`registry::adapter_for`], which is the only other selection.
fn provider_for(operation: &PreparedOperation) -> &'static dyn ReleaseProvider {
    match operation {
        PreparedOperation::RegistryPublish(_) => &registry::RegistryProvider,
        PreparedOperation::Tag(_) => &tag::TagProvider,
        PreparedOperation::ForgeRelease(_) => &forge::ForgeReleaseProvider,
        PreparedOperation::ArtifactUpload(_) => &artifact::ArtifactUploadProvider,
    }
}

/// A role that can issue an effect but cannot be asked what the provider holds
/// could never prove absence before acting, so it is refused here -- before any
/// observation or effect -- rather than at dispatch.
pub(crate) fn checked_provider_for(operation: &PreparedOperation) -> Result<&'static dyn ReleaseProvider, GraphError> {
    let provider = provider_for(operation);
    require_observable(provider.capabilities())?;
    Ok(provider)
}

fn require_observable(capabilities: ProviderCapabilities) -> Result<(), GraphError> {
    if capabilities.can_publish && !capabilities.can_observe {
        return Err(GraphError::ReleasePreconditionUnmet {
            requirement: ReleasePreconditionRequirement::ObservableProvider,
        });
    }
    Ok(())
}

/// Maps one observation to its pre-effect decision. `conflict` names the
/// operation-specific disagreement so the typed error stays precise.
fn preflight_from_observation(
    observation: ProviderObservationV1,
    id: &ReleaseOperationId,
    conflict: RemoteConflict,
) -> Result<ReleasePreflight, GraphError> {
    match &observation {
        ProviderObservationV1::Absent => Ok(ReleasePreflight::Proceed {
            proof: observation
                .absent_proof()
                .expect("an absent observation mints an absent proof"),
        }),
        ProviderObservationV1::Exact { .. } => Ok(ReleasePreflight::AlreadySatisfied {
            evidence: observation
                .exact_evidence()
                .expect("an exact observation mints its evidence"),
        }),
        ProviderObservationV1::Conflict { .. } => Err(GraphError::ReleaseRemoteConflict { conflict }),
        ProviderObservationV1::Indeterminate { .. } => Err(GraphError::ReleaseProviderIndeterminate {
            operation: Box::new(id.clone()),
        }),
    }
}

/// Requires that a post-effect observation is exact before the operation may
/// be recorded as done.
pub(crate) fn confirmed_evidence(
    observation: ProviderObservationV1,
    id: &ReleaseOperationId,
    conflict: RemoteConflict,
) -> Result<ExactEvidence, GraphError> {
    match &observation {
        ProviderObservationV1::Exact { .. } => Ok(observation
            .exact_evidence()
            .expect("an exact observation mints its evidence")),
        ProviderObservationV1::Indeterminate { .. } => Err(GraphError::ReleaseProviderIndeterminate {
            operation: Box::new(id.clone()),
        }),
        ProviderObservationV1::Absent | ProviderObservationV1::Conflict { .. } => {
            Err(GraphError::ReleaseRemoteConflict { conflict })
        }
    }
}

/// The invariant error a provider raises when it is handed another role's
/// prepared operation.
pub(crate) fn wrong_role(id: &ReleaseOperationId, expected: &str) -> GraphError {
    GraphError::ReleaseInvariant {
        detail: format!("operation `{id:?}` is not a prepared {expected} operation"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every production role must be able to answer an observation, because
    /// the executor may only issue an effect against a proved absence.
    #[test]
    fn no_production_provider_can_publish_without_being_observable() {
        for provider in [
            &registry::RegistryProvider as &dyn ReleaseProvider,
            &tag::TagProvider,
            &forge::ForgeReleaseProvider,
            &artifact::ArtifactUploadProvider,
        ] {
            let capabilities = provider.capabilities();
            assert!(
                capabilities.can_observe,
                "a publishing provider that cannot observe would act without proof of absence"
            );
            assert!(capabilities.can_publish);
        }
    }

    #[test]
    fn a_publish_only_role_is_refused_and_every_other_shape_is_allowed() {
        let error = require_observable(ProviderCapabilities {
            can_observe: false,
            can_publish: true,
        })
        .unwrap_err();
        assert!(
            matches!(
                error,
                GraphError::ReleasePreconditionUnmet {
                    requirement: ReleasePreconditionRequirement::ObservableProvider
                }
            ),
            "expected ReleasePreconditionUnmet(ObservableProvider), got {error:?}"
        );
        for capabilities in [
            ProviderCapabilities {
                can_observe: true,
                can_publish: true,
            },
            ProviderCapabilities {
                can_observe: true,
                can_publish: false,
            },
            ProviderCapabilities {
                can_observe: false,
                can_publish: false,
            },
        ] {
            assert!(
                require_observable(capabilities).is_ok(),
                "{capabilities:?} must be allowed"
            );
        }
    }
}
