//! The validated authorization capability and the staleness rules behind it.

use std::collections::BTreeMap;
use std::path::Path;

use callisto_model::vcs::access::{GitCommitTrustEvidence, GitHeadDisposition};
use callisto_model::{
    AbsentProof, ApplyPermit, CommandRunner, ExactEvidence, ExecutionTrustProfileV1, ProviderObservationV1,
    ReleaseDecisionV1, ReleaseIntentError, ReleaseIntentV1, ReleaseOperationId, SourceIdentity,
};

use crate::error::ReleasePreconditionRequirement;
use crate::{DependencyResolver, GraphError, ProjectLocator, Workspace};

use super::binding::{recheck_git_remote, PreparedGitRemote};
use super::derive::{
    artifact_policy_from_intent, derive_release_intent, derive_release_intent_with_prepared, ArtifactBuildPolicy,
    GitRemoteRequirement,
};
use super::provider::{
    checked_provider_for, EffectAuthorization, PreparedOperation, ProviderContext, ProviderRequest, PypiFileSets,
    ReleasePreflight, ReleaseProviderSet,
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
    file_sets: PypiFileSets,
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
            .with_file_sets(&self.file_sets)
    }

    /// Builds every Python publish of the intent into `outdir` and returns the packages built.
    ///
    /// This is the credential-free build job's work: the executing job later uploads these exact files.
    pub fn build_python_distributions(&self, outdir: &Path) -> Result<Vec<String>, GraphError> {
        let context = self.context();
        let mut built = Vec::new();
        for (id, prepared) in &self.prepared.operations {
            let PreparedOperation::RegistryPublish(operation) = prepared else {
                continue;
            };
            if id.package.ecosystem() != callisto_model::Ecosystem::Pypi {
                continue;
            }
            super::provider::build_python_into(&context, operation, outdir)?;
            built.push(operation.package_name.clone());
        }
        Ok(built)
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
            callisto_model::vcs::GitAccess::new(&self.prepared.root, self.runner).observe_git_commit_trust()?;
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

    fn observe_within(
        &self,
        id: &ReleaseOperationId,
        artifacts: Option<&VerifiedArtifactManifest<'_>>,
        patience: std::time::Duration,
    ) -> Result<ProviderObservationV1, GraphError> {
        let operation = self.prepared_operation(id)?;
        checked_provider_for(operation)?.observe(
            &self.context().with_patience(patience),
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

    fn defers_confirmation(&self, id: &ReleaseOperationId) -> bool {
        let Ok(operation) = self.prepared_operation(id) else {
            return false;
        };
        checked_provider_for(operation).is_ok_and(|provider| {
            provider.defers_confirmation(&ProviderRequest {
                id,
                operation,
                artifacts: None,
            })
        })
    }

    fn issue(
        &self,
        permit: &ApplyPermit,
        proof: &AbsentProof,
        id: &ReleaseOperationId,
        artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<(), GraphError> {
        let operation = self.prepared_operation(id)?;
        checked_provider_for(operation)?.issue(
            &self.context(),
            &ProviderRequest {
                id,
                operation,
                artifacts,
            },
            &EffectAuthorization { permit, proof },
        )
    }

    fn confirmation_window(&self, id: &ReleaseOperationId) -> std::time::Duration {
        let Ok(operation) = self.prepared_operation(id) else {
            return std::time::Duration::ZERO;
        };
        checked_provider_for(operation).map_or(std::time::Duration::ZERO, |provider| {
            provider.confirmation_window(&ProviderRequest {
                id,
                operation,
                artifacts: None,
            })
        })
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
        &mut Vec::new(),
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
///
/// Returns advisory diagnostics from derivation (e.g. an artifact owner selected
/// without its product) alongside the intent, for a caller to surface.
pub fn build_release_intent_with_artifacts<L: ProjectLocator, R: CommandRunner>(
    root: &Path,
    locator: &L,
    runner: &R,
    decision: &ReleaseDecisionV1,
    trust_profile: ExecutionTrustProfileV1,
    artifact_policy: ArtifactBuildPolicy,
) -> Result<(ReleaseIntentV1, Vec<callisto_model::Diagnostic>), GraphError> {
    let root = canonical_root(root)?;
    let workspace = Workspace::load(root.clone(), locator, runner)?;
    let source = observe_source(&workspace, trust_profile, ReleaseCheckout::Detached)?;
    let mut diagnostics = Vec::new();
    let intent = derive_release_intent(
        &workspace,
        decision,
        source.clone(),
        trust_profile,
        Some(&artifact_policy),
        GitRemoteRequirement::Required,
        &mut diagnostics,
    )?;
    if observe_source(&workspace, trust_profile, ReleaseCheckout::Detached)? != source {
        return Err(GraphError::ReleaseIntentStale {
            reason: StaleReason::source_identity_changed(),
        });
    }
    Ok((intent, diagnostics))
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
    // This is an integrity re-derivation, not a fresh plan: any diagnostic it would
    // raise was already surfaced when `received` was first derived.
    let (expected, prepared) = derive_release_intent_with_prepared(
        &workspace,
        &received.decision,
        source.clone(),
        received.trust_profile,
        artifact_policy.as_ref(),
        &mut Vec::new(),
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
        file_sets: PypiFileSets::default(),
    })
}

/// The longest an advisory observation waits between its own tries: one retry past a transient failure, no more.
const ADVISORY_OBSERVATION_PATIENCE: std::time::Duration = std::time::Duration::from_secs(1);

/// The most registry or remote observations in flight at once.
const OBSERVATION_WORKERS: usize = 8;

/// `work` over every item, results in item order. Observations are independent reads that spend their time waiting on a
/// registry or remote, so they run side by side instead of one after another.
pub(super) fn in_parallel<I: Sync, T: Send>(items: &[I], work: impl Fn(&I) -> T + Sync) -> Vec<T> {
    let workers = items.len().min(OBSERVATION_WORKERS);
    if workers <= 1 {
        return items.iter().map(work).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(item) = items.get(index) else {
                    break;
                };
                let value = work(item);
                done.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((index, value));
            });
        }
    });
    let mut done = done.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    done.sort_by_key(|(index, _)| *index);
    done.into_iter().map(|(_, value)| value).collect()
}

/// The packages in `among` with any operation of `prepared` that is not observed exactly satisfied.
///
/// Other packages' operations are not observed. An observation that fails counts as unsatisfied, so the run
/// itself reports it.
pub(crate) fn unsatisfied_packages(
    root: &Path,
    runner: &dyn CommandRunner,
    prepared: &super::derive::PreparedDerivation,
    among: &std::collections::BTreeSet<callisto_model::ReleasePackageId>,
) -> std::collections::BTreeSet<callisto_model::ReleasePackageId> {
    let context = ProviderContext::new(root, runner, prepared.git_remote.as_ref());
    let operations: Vec<_> = prepared
        .operations
        .iter()
        .filter(|(id, _)| among.contains(&id.package))
        .collect();
    let unsatisfied = in_parallel(&operations, |(id, operation)| {
        let observed = checked_provider_for(operation).and_then(|provider| {
            provider.observe(
                &context,
                &ProviderRequest {
                    id,
                    operation,
                    artifacts: None,
                },
            )
        });
        !matches!(observed, Ok(ProviderObservationV1::Exact { .. }))
    });
    operations
        .into_iter()
        .zip(unsatisfied)
        .filter(|(_, unsatisfied)| *unsatisfied)
        .map(|((id, _), _)| id.package.clone())
        .collect()
}

/// The packages in `among` with a registry publish already observed exactly satisfied.
///
/// A registry that cannot be read is an error, never "not published": the caller decides whether to tag from this.
/// A conflicting registry state is left to the run, which stops on it before any effect.
pub(crate) fn published_packages(
    root: &Path,
    runner: &dyn CommandRunner,
    prepared: &super::derive::PreparedDerivation,
    among: &std::collections::BTreeSet<callisto_model::ReleasePackageId>,
) -> Result<std::collections::BTreeSet<callisto_model::ReleasePackageId>, GraphError> {
    let context = ProviderContext::new(root, runner, prepared.git_remote.as_ref());
    let publishes: Vec<_> = prepared
        .operations
        .iter()
        .filter(|(id, operation)| {
            among.contains(&id.package) && matches!(operation, PreparedOperation::RegistryPublish(_))
        })
        .collect();
    let observed = in_parallel(&publishes, |(id, operation)| {
        checked_provider_for(operation)?.observe(
            &context,
            &ProviderRequest {
                id,
                operation,
                artifacts: None,
            },
        )
    });
    let mut published = std::collections::BTreeSet::new();
    for ((id, _), observed) in publishes.into_iter().zip(observed) {
        match observed? {
            ProviderObservationV1::Exact { .. } => {
                published.insert(id.package.clone());
            }
            ProviderObservationV1::Indeterminate { cause } => {
                return Err(GraphError::ReleaseProviderIndeterminate {
                    operation: Box::new(id.clone()),
                    cause,
                });
            }
            ProviderObservationV1::Absent | ProviderObservationV1::Conflict { .. } => {}
        }
    }
    Ok(published)
}

/// A published version whose registry entry names another project's repository.
pub(crate) struct TakenName {
    pub(crate) package: callisto_model::ReleasePackageId,
    pub(crate) version: String,
    pub(crate) registry_repository: String,
    pub(crate) origin: String,
}

/// Of the registry publishes of `among`, those whose version the registry already holds under a repository other than
/// this one. An unreadable or unrecorded repository proves nothing, so it is not reported; neither is a publish that
/// cannot be observed.
pub(crate) fn names_taken_by_other_projects(
    root: &Path,
    runner: &dyn CommandRunner,
    prepared: &super::derive::PreparedDerivation,
    among: &std::collections::BTreeSet<callisto_model::ReleasePackageId>,
) -> Result<Vec<TakenName>, GraphError> {
    // Advisory: a registry that does not answer promptly leaves the check unable to tell, so it does not wait out the
    // full retry schedule that the checks a release depends on use.
    let context =
        ProviderContext::new(root, runner, prepared.git_remote.as_ref()).with_patience(ADVISORY_OBSERVATION_PATIENCE);
    let Some(origin) = context.origin_endpoint() else {
        return Ok(Vec::new());
    };
    let publishes: Vec<_> = prepared
        .operations
        .iter()
        .filter(|(id, operation)| {
            among.contains(&id.package) && matches!(operation, PreparedOperation::RegistryPublish(_))
        })
        .collect();
    let found = in_parallel(&publishes, |(id, operation)| -> Result<Option<TakenName>, GraphError> {
        let PreparedOperation::RegistryPublish(publish) = operation else {
            return Ok(None);
        };
        // A registry that cannot be reached, or a tool that is missing, leaves this check unable to tell: the
        // publish itself reports that when it matters.
        let observed = checked_provider_for(operation)?.observe(
            &context,
            &ProviderRequest {
                id,
                operation,
                artifacts: None,
            },
        );
        if !matches!(observed, Ok(ProviderObservationV1::Exact { .. })) {
            return Ok(None);
        }
        let recorded = super::provider::registry::recorded_repository(&context, id.package.ecosystem(), publish);
        Ok(
            super::provider::repository::differing_repositories(recorded.as_deref(), origin).map(
                |(registry_repository, origin)| TakenName {
                    package: id.package.clone(),
                    version: publish.version.render().to_string(),
                    registry_repository,
                    origin,
                },
            ),
        )
    });
    let mut taken = Vec::new();
    for found in found {
        taken.extend(found?);
    }
    Ok(taken)
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

#[cfg(test)]
mod tests {
    use super::{in_parallel, OBSERVATION_WORKERS};

    #[test]
    fn in_parallel_returns_every_result_in_item_order() {
        let items: Vec<usize> = (0..OBSERVATION_WORKERS * 4 + 3).collect();
        let doubled = in_parallel(&items, |item| {
            // Later items finish first, so order comes from the index and not from completion.
            std::thread::sleep(std::time::Duration::from_millis((items.len() - item) as u64 % 5));
            item * 2
        });
        assert_eq!(doubled, items.iter().map(|item| item * 2).collect::<Vec<_>>());
        assert!(in_parallel(&Vec::<usize>::new(), |item| *item).is_empty());
        assert_eq!(in_parallel(&[7], |item| item + 1), vec![8]);
    }
}
