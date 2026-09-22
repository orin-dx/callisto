//! Deterministic fault-injection simulator for the real release executor.
//!
//! The executor's ordering hazards were found by reading and then pinned by a
//! hand-written test each time. This drives the production
//! [`execute_release`](super::release_execution::execute_release) against a
//! simulated world that crashes at every persistence point and injects every
//! provider outcome, then asserts the lifecycle invariants after each run.
//!
//! Nothing here is random and nothing reads a clock: a scenario is a pair of
//! integers plus a fault kind, and the whole enumeration is a nested loop.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use callisto_model::{
    AbsentProof, ApplyPermit, ArtifactDigest, ArtifactManifestEntryV1, ArtifactManifestV1, ArtifactSlotId, CommitSha,
    Ecosystem, ExactEvidence, ExecutionTrustProfileV1, GitHubArtifactAttestationV1, GitHubRepository, OperationState,
    ProviderConflictReason, ProviderEvidenceV1, ProviderIndeterminateCause, ProviderObservationV1,
    RegistryBindingDigest, RegistryBindingId, ReleaseDecisionEntry, ReleaseDecisionV1, ReleaseExecutionStateV1,
    ReleaseInclusionReason, ReleaseInputSnapshotV1, ReleaseIntentV1, ReleaseOperation, ReleaseOperationId,
    ReleasePackageId, ReleaseProfileId, ReleaseReceiptV1, ReleaseRunEnvelopeV1, ReleaseRunKindV1, SourceIdentity,
    Version,
};

use crate::commands::release::provider::preflight_from_observation;
use crate::commands::release_artifacts::VerifiedArtifactManifest;
use crate::commands::release_execution::execute_release;
use crate::commands::release_test_support::{envelope_of_kind, evidence_for};
use crate::commands::{
    observe_release_operations, ReleasePreflight, ReleaseProviderSet, ReleaseStateStore, ReleaseStateWriter,
};
use crate::error::{CommandFailure, RemoteConflict};
use crate::GraphError;

/// How many observations a landed effect stays invisible under injected
/// registry lag. Two is deliberate: one is absorbed by the post-effect
/// confirmation, so only a second exposes the rerun paths to a stale `Absent`.
const LAG_SPAN: usize = 2;

/// Reruns the operator is simulated as performing after a failed run. Runs 0
/// and 1 may still carry the scenario's faults, so runs 2 through 4 are the
/// three clean reruns convergence is asserted within.
const MAX_RERUNS: usize = 4;

/// The fault budget is spent inside the first two executions.
const FAULT_ARMED_RUNS: usize = 2;

// ---------------------------------------------------------------------------
// The intent under simulation
// ---------------------------------------------------------------------------

/// Two packages covering every operation role, wired with the production DAG
/// order: platform -> registry -> tag -> draft release -> every upload -> publication.
fn simulator_intent() -> (ReleaseIntentV1, ArtifactManifestV1) {
    let version = Version::semver(1, 2, 3);
    let registry = RegistryBindingId::new(
        "crates",
        RegistryBindingDigest::from_normalized_binding(b"crates-default"),
    )
    .unwrap();
    let alpha = ReleasePackageId::new(Ecosystem::Cargo, "alpha").unwrap();
    let beta = ReleasePackageId::new(Ecosystem::Cargo, "beta").unwrap();
    let repository = GitHubRepository::parse("orin-dx/callisto").unwrap();
    let workflow_commit = CommitSha::parse(&"b".repeat(40)).unwrap();
    let slot = |platform: &str, asset: &str| {
        ArtifactSlotId::new(
            alpha.clone(),
            version.clone(),
            platform,
            asset,
            repository.clone(),
            ".github/workflows/release.yml",
            workflow_commit.clone(),
        )
        .unwrap()
    };
    let linux = slot("x86_64-unknown-linux-gnu", "alpha-linux.tar.gz");
    let macos = slot("aarch64-apple-darwin", "alpha-macos.tar.gz");

    let alpha_registry =
        ReleaseOperation::registry_publish(alpha.clone(), version.clone(), registry.clone(), vec![]).unwrap();
    let alpha_tag = ReleaseOperation::tag(alpha.clone(), version.clone(), vec![alpha_registry.id().clone()]).unwrap();
    let alpha_draft =
        ReleaseOperation::forge_release(alpha.clone(), version.clone(), vec![alpha_tag.id().clone()]).unwrap();
    let linux_upload = ReleaseOperation::artifact_upload(linux.clone(), vec![alpha_draft.id().clone()]).unwrap();
    let macos_upload = ReleaseOperation::artifact_upload(macos.clone(), vec![alpha_draft.id().clone()]).unwrap();
    let alpha_publish = ReleaseOperation::forge_publish(
        alpha.clone(),
        version.clone(),
        vec![
            alpha_draft.id().clone(),
            linux_upload.id().clone(),
            macos_upload.id().clone(),
        ],
    )
    .unwrap();
    let beta_platform = ReleaseOperation::new(
        ReleaseOperationId::platform_publish(
            beta.clone(),
            version.clone(),
            registry.clone(),
            callisto_model::PlatformPackageV1::new(
                ReleasePackageId::new(Ecosystem::Npm, "beta-linux-x64-gnu").unwrap(),
                "beta/npm/linux-x64-gnu",
            )
            .unwrap(),
        ),
        vec![],
    )
    .unwrap();
    let beta_registry = ReleaseOperation::registry_publish(
        beta.clone(),
        version.clone(),
        registry,
        vec![beta_platform.id().clone()],
    )
    .unwrap();
    let beta_tag = ReleaseOperation::tag(beta.clone(), version.clone(), vec![beta_registry.id().clone()]).unwrap();

    let decision = ReleaseDecisionV1::new(vec![
        ReleaseDecisionEntry {
            package: alpha,
            target_version: version.clone(),
            reasons: vec![ReleaseInclusionReason::ExplicitSelection],
        },
        ReleaseDecisionEntry {
            package: beta,
            target_version: version.clone(),
            reasons: vec![ReleaseInclusionReason::ExplicitSelection],
        },
    ])
    .unwrap();
    let operations = vec![
        alpha_registry,
        alpha_tag,
        alpha_draft,
        linux_upload,
        macos_upload,
        alpha_publish,
        beta_platform,
        beta_registry,
        beta_tag,
    ];
    let operations = crate::commands::release::canonical_operation_order(
        operations
            .into_iter()
            .map(|operation| (operation.id().clone(), operation))
            .collect(),
    )
    .unwrap();
    let mut slots = vec![linux, macos];
    slots.sort();
    let intent = ReleaseIntentV1::new(
        ReleaseProfileId::production(),
        decision,
        ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]).unwrap(),
        ExecutionTrustProfileV1::GitCommit,
        operations,
        slots.clone(),
    )
    .unwrap();
    let manifest = ArtifactManifestV1::new(&intent, slots.into_iter().map(manifest_entry).collect()).unwrap();
    (intent, manifest)
}

fn manifest_entry(slot: ArtifactSlotId) -> ArtifactManifestEntryV1 {
    let digest = ArtifactDigest::from_bytes(b"binary");
    ArtifactManifestEntryV1 {
        attestation: GitHubArtifactAttestationV1 {
            repository: slot.attestation_policy.repository.clone(),
            workflow_path: slot.attestation_policy.workflow_path.clone(),
            workflow_commit: slot.attestation_policy.workflow_commit.clone(),
            subject_digest: digest.clone(),
            source_commit: CommitSha::parse(&"b".repeat(40)).unwrap(),
        },
        slot,
        digest,
        byte_length: 6,
    }
}

// ---------------------------------------------------------------------------
// Scenario vocabulary
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FaultKind {
    /// The provider cannot establish remote identity at all.
    ObserveIndeterminate,
    /// A disagreeing remote object. Permanent: no rerun resolves it.
    ObserveConflict,
    /// The effect was refused before it could land.
    EffectFailsBeforeLanding,
    /// The effect landed and the response was lost.
    EffectLandsThenErrors,
    /// The effect landed, but the next `LAG_SPAN` observations still answer absent.
    RegistryLag,
}

impl FaultKind {
    const ALL: [Self; 5] = [
        Self::ObserveIndeterminate,
        Self::ObserveConflict,
        Self::EffectFailsBeforeLanding,
        Self::EffectLandsThenErrors,
        Self::RegistryLag,
    ];

    fn is_observation_fault(self) -> bool {
        matches!(self, Self::ObserveIndeterminate | Self::ObserveConflict)
    }

    /// Whether the effect is left on the provider when the fault fires.
    fn lands_the_effect(self) -> bool {
        matches!(self, Self::EffectLandsThenErrors | Self::RegistryLag)
    }
}

/// A fault armed at the `at`-th call of its phase (observations and effects
/// are counted separately, so no scheduled fault is a no-op).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fault {
    kind: FaultKind,
    at: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CrashPoint {
    /// Process death at the `at`-th provider call. An effect lands first: the
    /// harmless variant is already covered by `EffectFailsBeforeLanding`.
    ProviderCall { at: usize },
    /// Process death at the `at`-th durable save, before or after it lands.
    Save { at: usize, after: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Scenario {
    crash: Option<CrashPoint>,
    fault: Option<Fault>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// The operator reruns with the persisted state and the same envelope.
    SameRunner,
    /// The operator reruns elsewhere: no local state, a recovery envelope.
    FreshRunner,
}

// ---------------------------------------------------------------------------
// Trace
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObservationTag {
    Absent,
    Exact,
    Conflict,
    Indeterminate,
}

/// Every field is read by an operator through the `{:?}` trace a failing
/// scenario prints, which dead-code analysis deliberately does not see.
#[allow(dead_code)]
#[derive(Clone, Debug)]
enum TraceEvent {
    RunStart {
        run: usize,
        kind: ReleaseRunKindV1,
    },
    Observed {
        run: usize,
        id: ReleaseOperationId,
        tag: ObservationTag,
    },
    Effect {
        run: usize,
        id: ReleaseOperationId,
        landed: bool,
    },
    Saved {
        run: usize,
        states: BTreeMap<ReleaseOperationId, OperationState>,
    },
    Crashed {
        run: usize,
    },
    RunEnd {
        run: usize,
        outcome: String,
    },
}

/// State shared between the simulated world and the simulated disk.
#[derive(Debug, Default)]
struct SimContext {
    run: Cell<usize>,
    trace: RefCell<Vec<TraceEvent>>,
    crashed: Cell<bool>,
}

impl SimContext {
    fn push(&self, event: TraceEvent) {
        self.trace.borrow_mut().push(event);
    }
}

// ---------------------------------------------------------------------------
// The simulated world
// ---------------------------------------------------------------------------

/// What the providers hold for one operation.
#[derive(Clone, Debug)]
enum Remote {
    Absent,
    /// The effect landed, carrying evidence of this operation's own role.
    Landed {
        evidence: ProviderEvidenceV1,
    },
    /// A disagreeing object that is not what this intent authorized.
    Conflicting {
        reason: ProviderConflictReason,
    },
}

/// Ground truth for the whole simulated release, plus the fault schedule.
struct SimWorld {
    intent: ReleaseIntentV1,
    context: Rc<SimContext>,
    scenario: Scenario,
    remote: RefCell<BTreeMap<ReleaseOperationId, Remote>>,
    lag: RefCell<BTreeMap<ReleaseOperationId, usize>>,
    landings: RefCell<Vec<ReleaseOperationId>>,
    observation_calls: Cell<usize>,
    effect_calls: Cell<usize>,
    provider_calls: Cell<usize>,
    duplicate_landings: RefCell<Vec<ReleaseOperationId>>,
    order_violations: RefCell<Vec<(ReleaseOperationId, ReleaseOperationId)>>,
    /// Operations whose landing was ever hidden by injected registry lag.
    lagged_ever: RefCell<BTreeSet<ReleaseOperationId>>,
    /// Set while this run is a recovery run that started with no journal.
    lost_journal_recovery: Cell<bool>,
    sabotage: Sabotage,
}

/// Deliberate defects injected into the world or the disk, used only to prove
/// the invariant checker reports what it claims to report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Sabotage {
    /// Every observation outside a post-effect confirmation answers `Absent`
    /// about an effect the world already holds.
    lying_provider: bool,
    /// The writer accepts every save and persists none of them.
    lose_saves: bool,
}

impl SimWorld {
    fn new(intent: ReleaseIntentV1, scenario: Scenario, context: Rc<SimContext>, sabotage: Sabotage) -> Self {
        let remote = intent
            .operations
            .iter()
            .map(|operation| (operation.id().clone(), Remote::Absent))
            .collect();
        Self {
            intent,
            context,
            scenario,
            remote: RefCell::new(remote),
            lag: RefCell::new(BTreeMap::new()),
            landings: RefCell::new(Vec::new()),
            observation_calls: Cell::new(0),
            effect_calls: Cell::new(0),
            provider_calls: Cell::new(0),
            duplicate_landings: RefCell::new(Vec::new()),
            order_violations: RefCell::new(Vec::new()),
            lagged_ever: RefCell::new(BTreeSet::new()),
            lost_journal_recovery: Cell::new(false),
            sabotage,
        }
    }

    fn faults_armed(&self) -> bool {
        self.context.run.get() < FAULT_ARMED_RUNS
    }

    /// Consumes one global provider-call slot and crashes if this is the point.
    fn take_provider_call(&self) -> Option<()> {
        let index = self.provider_calls.get();
        self.provider_calls.set(index + 1);
        if self.context.run.get() == 0 && self.scenario.crash == Some(CrashPoint::ProviderCall { at: index }) {
            return None;
        }
        Some(())
    }

    fn crash(&self) -> GraphError {
        self.context.crashed.set(true);
        self.context.push(TraceEvent::Crashed {
            run: self.context.run.get(),
        });
        command_failure("simulated process death")
    }

    fn armed_fault(&self, observation_phase: bool, index: usize) -> Option<FaultKind> {
        let fault = self.scenario.fault?;
        (self.faults_armed() && fault.kind.is_observation_fault() == observation_phase && fault.at == index)
            .then_some(fault.kind)
    }

    /// The one observation seam. Every provider read in the simulation lands here.
    fn observe_internal(&self, id: &ReleaseOperationId) -> Result<ProviderObservationV1, GraphError> {
        let index = self.observation_calls.get();
        self.observation_calls.set(index + 1);
        if self.take_provider_call().is_none() {
            return Err(self.crash());
        }
        let observation = match self.armed_fault(true, index) {
            Some(FaultKind::ObserveIndeterminate) => ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::Timeout,
            },
            Some(FaultKind::ObserveConflict) => {
                let reason = conflict_reason_for(id);
                self.remote
                    .borrow_mut()
                    .insert(id.clone(), Remote::Conflicting { reason });
                ProviderObservationV1::Conflict { reason }
            }
            _ => self.remote_observation(id, false),
        };
        self.context.push(TraceEvent::Observed {
            run: self.context.run.get(),
            id: id.clone(),
            tag: tag_of(&observation),
        });
        Ok(observation)
    }

    /// `confirming` marks the post-effect read a provider performs inside its
    /// own publish, which the lying-provider fixture leaves honest so the
    /// first effect can still be confirmed.
    fn remote_observation(&self, id: &ReleaseOperationId, confirming: bool) -> ProviderObservationV1 {
        match self.remote.borrow().get(id).cloned().unwrap_or(Remote::Absent) {
            Remote::Absent => ProviderObservationV1::Absent,
            Remote::Conflicting { reason } => ProviderObservationV1::Conflict { reason },
            Remote::Landed { evidence } => {
                if self.sabotage.lying_provider && !confirming {
                    return ProviderObservationV1::Absent;
                }
                let mut lag = self.lag.borrow_mut();
                match lag.get(id).copied().unwrap_or(0) {
                    0 => ProviderObservationV1::Exact { evidence },
                    remaining => {
                        lag.insert(id.clone(), remaining - 1);
                        ProviderObservationV1::Absent
                    }
                }
            }
        }
    }

    /// Records the landing of one effect and every ordering fact it proves.
    fn land(&self, id: &ReleaseOperationId) {
        self.remote.borrow_mut().insert(
            id.clone(),
            Remote::Landed {
                evidence: evidence_for(id),
            },
        );
        self.landings.borrow_mut().push(id.clone());
        self.context.push(TraceEvent::Effect {
            run: self.context.run.get(),
            id: id.clone(),
            landed: true,
        });
    }

    fn already_landed(&self, id: &ReleaseOperationId) -> bool {
        matches!(self.remote.borrow().get(id), Some(Remote::Landed { .. }))
    }

    /// The single narrow exception to I2, encoded deliberately.
    ///
    /// A recovery run whose journal was lost, facing an index that is still
    /// serving the pre-publication answer, has observed nothing but `Absent`
    /// for this operation and cannot distinguish lag from a missing effect.
    /// Re-issuing there is the provider's job to refuse -- which the world
    /// does, and the single-landing check above asserts. Every other
    /// re-issue, including one caused by a provider that simply lies, is a
    /// violation.
    fn excused_duplicate(&self, id: &ReleaseOperationId) -> bool {
        self.lost_journal_recovery.get() && self.lagged_ever.borrow().contains(id)
    }

    fn check_prerequisites(&self, id: &ReleaseOperationId) {
        let Some(operation) = self.intent.operations.iter().find(|candidate| candidate.id() == id) else {
            return;
        };
        for prerequisite in operation.prerequisites() {
            if !self.already_landed(prerequisite) {
                self.order_violations
                    .borrow_mut()
                    .push((id.clone(), prerequisite.clone()));
            }
        }
    }
}

impl ReleaseProviderSet for SimWorld {
    fn intent(&self) -> &ReleaseIntentV1 {
        &self.intent
    }

    fn recheck_trust(&self) -> Result<(), GraphError> {
        Ok(())
    }

    fn preflight(
        &self,
        id: &ReleaseOperationId,
        _artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<ReleasePreflight, GraphError> {
        let observation = self.observe_internal(id)?;
        // The production registry provider refuses to adopt a live version at
        // preflight: only the recovery reconstruction path may converge on an
        // exact registry observation. Modelling that here is what makes a
        // still-`Pending` registry operation a wedge rather than a no-op.
        if matches!(
            id.role,
            callisto_model::ReleaseOperationRole::RegistryPublish { .. }
                | callisto_model::ReleaseOperationRole::PlatformPublish { .. }
        ) && matches!(observation, ProviderObservationV1::Exact { .. })
        {
            return Err(GraphError::ReleaseRegistryVersionExists {
                package: id.package.name().to_owned(),
                version: id.version.clone(),
            });
        }
        preflight_from_observation(observation, id, remote_conflict_for(id))
    }

    fn observe(
        &self,
        id: &ReleaseOperationId,
        _artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<ProviderObservationV1, GraphError> {
        self.observe_internal(id)
    }

    fn publish(
        &self,
        _permit: &ApplyPermit,
        _proof: &AbsentProof,
        id: &ReleaseOperationId,
        _artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<ExactEvidence, GraphError> {
        let index = self.effect_calls.get();
        self.effect_calls.set(index + 1);
        self.check_prerequisites(id);

        if self.already_landed(id) {
            if !self.excused_duplicate(id) {
                self.duplicate_landings.borrow_mut().push(id.clone());
            }
            // Real providers refuse a second landing: cargo and gh both report
            // that the object already exists.
            return Err(command_failure("already exists"));
        }

        let fault = self.armed_fault(false, index);
        if fault == Some(FaultKind::EffectFailsBeforeLanding) {
            self.context.push(TraceEvent::Effect {
                run: self.context.run.get(),
                id: id.clone(),
                landed: false,
            });
            if self.take_provider_call().is_none() {
                return Err(self.crash());
            }
            return Err(command_failure("effect refused"));
        }

        self.land(id);
        if fault == Some(FaultKind::RegistryLag) {
            self.lag.borrow_mut().insert(id.clone(), LAG_SPAN);
            self.lagged_ever.borrow_mut().insert(id.clone());
        }
        if self.take_provider_call().is_none() {
            return Err(self.crash());
        }
        if fault.is_some_and(FaultKind::lands_the_effect) {
            return Err(command_failure("response lost after the effect landed"));
        }
        // Production confirms an effect by re-observing it, never by the
        // client's exit code, so the confirmation goes through the same seam.
        let confirmation = self.remote_observation(id, true);
        confirmation
            .exact_evidence()
            .ok_or_else(|| GraphError::ReleaseRemoteConflict {
                conflict: remote_conflict_for(id),
            })
    }
}

fn tag_of(observation: &ProviderObservationV1) -> ObservationTag {
    match observation {
        ProviderObservationV1::Absent => ObservationTag::Absent,
        ProviderObservationV1::Exact { .. } => ObservationTag::Exact,
        ProviderObservationV1::Conflict { .. } => ObservationTag::Conflict,
        ProviderObservationV1::Indeterminate { .. } => ObservationTag::Indeterminate,
    }
}

fn conflict_reason_for(id: &ReleaseOperationId) -> ProviderConflictReason {
    match &id.role {
        callisto_model::ReleaseOperationRole::RegistryPublish { .. }
        | callisto_model::ReleaseOperationRole::PlatformPublish { .. } => ProviderConflictReason::RegistryVersionYanked,
        callisto_model::ReleaseOperationRole::Tag => ProviderConflictReason::RemoteTagTargetDiffers,
        callisto_model::ReleaseOperationRole::ForgeRelease | callisto_model::ReleaseOperationRole::ForgePublish => {
            ProviderConflictReason::ForgeReleaseDiffers
        }
        callisto_model::ReleaseOperationRole::ArtifactUpload { .. } => ProviderConflictReason::ArtifactAssetDiffers,
    }
}

fn remote_conflict_for(id: &ReleaseOperationId) -> RemoteConflict {
    match &id.role {
        callisto_model::ReleaseOperationRole::RegistryPublish { .. }
        | callisto_model::ReleaseOperationRole::PlatformPublish { .. } => RemoteConflict::RegistryVersionDiffers,
        callisto_model::ReleaseOperationRole::Tag => RemoteConflict::TagTargetDiffers,
        callisto_model::ReleaseOperationRole::ForgeRelease => RemoteConflict::ForgeReleaseDiffers,
        callisto_model::ReleaseOperationRole::ForgePublish => RemoteConflict::ForgeReleaseNotObservedAfterPublish,
        callisto_model::ReleaseOperationRole::ArtifactUpload { .. } => RemoteConflict::ArtifactDiffers,
    }
}

fn command_failure(stderr: &str) -> GraphError {
    GraphError::ReleaseCommand {
        program: "simulated".to_owned(),
        args: Vec::new(),
        failure: CommandFailure::NonZeroExit {
            exit_code: Some(1),
            stderr: stderr.to_owned(),
        },
    }
}

// ---------------------------------------------------------------------------
// The simulated disk
// ---------------------------------------------------------------------------

/// A real-file writer that records every durable save and can stop writing at
/// a chosen point, retaining exactly what was saved before it.
struct SimWriter {
    context: Rc<SimContext>,
    operations: Vec<ReleaseOperationId>,
    saves: Cell<usize>,
    crash: Option<CrashPoint>,
    sabotage: Sabotage,
}

impl SimWriter {
    fn new(
        context: Rc<SimContext>,
        operations: Vec<ReleaseOperationId>,
        crash: Option<CrashPoint>,
        sabotage: Sabotage,
    ) -> Self {
        Self {
            context,
            operations,
            saves: Cell::new(0),
            crash,
            sabotage,
        }
    }

    fn record(&self, content: &str) {
        let state: ReleaseExecutionStateV1 = serde_json::from_str(content).expect("state round-trips");
        let states = self
            .operations
            .iter()
            .filter_map(|id| state.operation_state(id).map(|value| (id.clone(), value)))
            .collect();
        self.context.push(TraceEvent::Saved {
            run: self.context.run.get(),
            states,
        });
    }
}

impl ReleaseStateWriter for SimWriter {
    fn write(&self, path: &Path, content: &str, _permit: &ApplyPermit) -> std::io::Result<()> {
        let index = self.saves.get();
        self.saves.set(index + 1);
        let crash_here = |after: bool| self.crash == Some(CrashPoint::Save { at: index, after });
        if crash_here(false) {
            self.context.crashed.set(true);
            self.context.push(TraceEvent::Crashed {
                run: self.context.run.get(),
            });
            return Err(std::io::Error::other("simulated process death before the save"));
        }
        if !self.sabotage.lose_saves {
            std::fs::write(path, content)?;
            self.record(content);
        }
        if crash_here(true) {
            self.context.crashed.set(true);
            self.context.push(TraceEvent::Crashed {
                run: self.context.run.get(),
            });
            return Err(std::io::Error::other("simulated process death after the save"));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Running one scenario
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunOutcome {
    Receipted,
    Failed,
    Crashed,
}

/// The payloads name the exact operation in the panic message.
#[allow(dead_code)]
#[derive(Debug)]
enum Violation {
    ReceiptWithoutLandedEffect {
        operation: Box<ReleaseOperationId>,
    },
    ReceiptWithNonSuccessState {
        operation: Box<ReleaseOperationId>,
        state: Option<OperationState>,
    },
    DuplicateLanding {
        operation: Box<ReleaseOperationId>,
    },
    SecondLandingRecorded {
        operation: Box<ReleaseOperationId>,
    },
    EffectBeforePrerequisite {
        operation: Box<ReleaseOperationId>,
        prerequisite: Box<ReleaseOperationId>,
    },
    AttemptingWithoutAbsentObservation {
        operation: Box<ReleaseOperationId>,
        run: usize,
    },
    NoConvergence {
        outcomes: Vec<RunOutcome>,
    },
    ConvergedDespitePermanentConflict,
    DependentLandedDespiteConflict {
        operation: Box<ReleaseOperationId>,
    },
}

struct ScenarioRun {
    world: SimWorld,
    context: Rc<SimContext>,
    outcomes: Vec<RunOutcome>,
}

/// One end-to-end simulated release, including the operator's reruns.
fn run_scenario(
    intent: &ReleaseIntentV1,
    manifest: &ArtifactManifestV1,
    state_path: &Path,
    scenario: Scenario,
    mode: Mode,
    sabotage: Sabotage,
) -> ScenarioRun {
    std::fs::remove_file(state_path).ok();
    let context = Rc::new(SimContext::default());
    let world = SimWorld::new(intent.clone(), scenario, Rc::clone(&context), sabotage);
    let operations: Vec<_> = intent.operations.iter().map(|op| op.id().clone()).collect();
    let initial = envelope_of_kind(intent, ReleaseRunKindV1::Initial);
    let recovery = envelope_of_kind(intent, ReleaseRunKindV1::Recovery);

    let mut outcomes = Vec::new();
    for run in 0..=MAX_RERUNS {
        context.run.set(run);
        context.crashed.set(false);
        let envelope = if run == 0 {
            &initial
        } else {
            match mode {
                Mode::SameRunner => &initial,
                Mode::FreshRunner => {
                    std::fs::remove_file(state_path).ok();
                    &recovery
                }
            }
        };
        world
            .lost_journal_recovery
            .set(envelope.kind() == ReleaseRunKindV1::Recovery && !state_path.exists());
        context.push(TraceEvent::RunStart {
            run,
            kind: envelope.kind(),
        });
        let crash = (run == 0).then_some(scenario.crash).flatten();
        let writer = SimWriter::new(Rc::clone(&context), operations.clone(), crash, sabotage);
        let store = ReleaseStateStore::with_writer(state_path, writer);
        let result = execute_and_receipt(&world, &store, manifest, envelope);
        let detail = match &result {
            Ok(()) => "receipted".to_owned(),
            Err(failure) => format!("{failure:?}"),
        };
        let outcome = match result {
            Ok(()) => RunOutcome::Receipted,
            Err(_) if context.crashed.get() => RunOutcome::Crashed,
            Err(_) => RunOutcome::Failed,
        };
        context.push(TraceEvent::RunEnd {
            run,
            outcome: format!("{outcome:?} {detail}"),
        });
        outcomes.push(outcome);
        if outcome == RunOutcome::Receipted {
            break;
        }
    }
    ScenarioRun {
        world,
        context,
        outcomes,
    }
}

/// Which stage of the production success path refused this run. The causes
/// are read from the `{:?}` trace a failing scenario prints.
#[allow(dead_code)]
#[derive(Debug)]
enum RunFailure {
    Execution(Box<GraphError>),
    Observation(Box<GraphError>),
    Receipt(Box<callisto_model::ReleaseReceiptError>),
}

/// The production success path in full: execute, then observe every operation
/// afresh, then issue the terminal receipt. Nothing short of a receipt counts.
fn execute_and_receipt(
    world: &SimWorld,
    store: &ReleaseStateStore<SimWriter>,
    manifest: &ArtifactManifestV1,
    envelope: &ReleaseRunEnvelopeV1,
) -> Result<(), RunFailure> {
    let permit = ApplyPermit::force_for_tests();
    let artifacts = VerifiedArtifactManifest::for_tests(manifest, PathBuf::from("/simulated"));
    let state = execute_release(world, store, &permit, envelope, Some(&artifacts))
        .map_err(|error| RunFailure::Execution(Box::new(error)))?;
    let observations = observe_release_operations(world, Some(&artifacts))
        .map_err(|error| RunFailure::Observation(Box::new(error)))?;
    ReleaseReceiptV1::from_evidence(&world.intent, &state, observations)
        .map_err(|error| RunFailure::Receipt(Box::new(error)))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Invariants
// ---------------------------------------------------------------------------

fn check_invariants(intent: &ReleaseIntentV1, run: &ScenarioRun, mode: Mode) -> Result<Classification, Violation> {
    let world = &run.world;

    // I2: no effect is issued for an operation that already landed.
    if let Some(operation) = world.duplicate_landings.borrow().first() {
        return Err(Violation::DuplicateLanding {
            operation: Box::new(operation.clone()),
        });
    }
    let mut seen = BTreeSet::new();
    for landing in world.landings.borrow().iter() {
        if !seen.insert(landing.clone()) {
            return Err(Violation::SecondLandingRecorded {
                operation: Box::new(landing.clone()),
            });
        }
    }

    // I4: no effect lands before every prerequisite has landed.
    if let Some((operation, prerequisite)) = world.order_violations.borrow().first() {
        return Err(Violation::EffectBeforePrerequisite {
            operation: Box::new(operation.clone()),
            prerequisite: Box::new(prerequisite.clone()),
        });
    }

    // I5: `Attempting` is persisted only after an absent observation of that
    // operation in the same run.
    check_attempting_is_earned(&run.context.trace.borrow())?;

    // I1 and I6: a receipt is produced only when the world really holds every
    // effect and the persisted journal has no non-terminal operation left.
    let durable = durable_states(&run.context.trace.borrow());
    if run.outcomes.last() == Some(&RunOutcome::Receipted) {
        for operation in &intent.operations {
            let id = operation.id();
            let state = durable.get(id).copied();
            if !matches!(
                state,
                Some(OperationState::Published | OperationState::AlreadySatisfied)
            ) {
                return Err(Violation::ReceiptWithNonSuccessState {
                    operation: Box::new(id.clone()),
                    state,
                });
            }
            if !world.already_landed(id) {
                return Err(Violation::ReceiptWithoutLandedEffect {
                    operation: Box::new(id.clone()),
                });
            }
        }
    }

    check_convergence(intent, run, mode, &durable)
}

/// How one scenario ended, tallied so the enumeration can prove it exercised
/// more than one outcome rather than passing everything vacuously.
#[derive(Clone, Copy, Debug, Default)]
struct Classification {
    converged: bool,
    conflicted: bool,
    stranded: bool,
}

/// What the last save actually left on disk. An operation the journal never
/// mentioned is `Pending`, which is what a reader of that file would conclude.
fn durable_states(trace: &[TraceEvent]) -> BTreeMap<ReleaseOperationId, OperationState> {
    trace
        .iter()
        .rev()
        .find_map(|event| match event {
            TraceEvent::Saved { states, .. } => Some(states.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// I3: convergence, and the two documented exceptions to it.
fn check_convergence(
    intent: &ReleaseIntentV1,
    run: &ScenarioRun,
    mode: Mode,
    durable: &BTreeMap<ReleaseOperationId, OperationState>,
) -> Result<Classification, Violation> {
    let converged = run.outcomes.last() == Some(&RunOutcome::Receipted);
    // Whether a conflict actually surfaced, not whether one was scheduled: a
    // crash can consume the run before the scheduled call is ever made.
    let conflicted = run.context.trace.borrow().iter().any(|event| {
        matches!(
            event,
            TraceEvent::Observed {
                tag: ObservationTag::Conflict,
                ..
            }
        )
    });
    if conflicted {
        if converged {
            return Err(Violation::ConvergedDespitePermanentConflict);
        }
        // Once a conflict has been observed, nothing downstream of it may be
        // issued. Effects that landed before the conflict surfaced are not
        // implicated, so this walks the trace rather than the end state.
        check_nothing_lands_after_a_conflict(intent, &run.context.trace.borrow())?;
        return Ok(Classification {
            conflicted: true,
            ..Classification::default()
        });
    }
    let stranded = stranded_attempt(&run.world, mode, durable);
    if converged || stranded {
        return Ok(Classification {
            converged,
            stranded,
            conflicted: false,
        });
    }
    Err(Violation::NoConvergence {
        outcomes: run.outcomes.clone(),
    })
}

fn check_nothing_lands_after_a_conflict(intent: &ReleaseIntentV1, trace: &[TraceEvent]) -> Result<(), Violation> {
    let mut blocked: BTreeSet<ReleaseOperationId> = BTreeSet::new();
    let mut conflicting: BTreeSet<ReleaseOperationId> = BTreeSet::new();
    for event in trace {
        match event {
            TraceEvent::Observed { id, tag, .. } if *tag == ObservationTag::Conflict => {
                if conflicting.insert(id.clone()) {
                    blocked = dependents_of(intent, &conflicting);
                }
            }
            TraceEvent::Effect { id, landed: true, .. } if blocked.contains(id) => {
                return Err(Violation::DependentLandedDespiteConflict {
                    operation: Box::new(id.clone()),
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// The one non-convergence the production design chooses deliberately, E173.
///
/// `Attempting` is never downgraded to `Pending`: an absent provider cannot
/// prove that the interrupted request did not take effect, so a same-runner
/// rerun refuses to re-dispatch and waits for a human. This encodes that
/// documented behavior instead of hiding it, and holds only for the same
/// runner -- a fresh-journal recovery run is still required to converge.
fn stranded_attempt(world: &SimWorld, mode: Mode, durable: &BTreeMap<ReleaseOperationId, OperationState>) -> bool {
    mode == Mode::SameRunner
        && durable
            .iter()
            .any(|(id, state)| *state == OperationState::Attempting && !world.already_landed(id))
}

fn dependents_of(intent: &ReleaseIntentV1, roots: &BTreeSet<ReleaseOperationId>) -> BTreeSet<ReleaseOperationId> {
    let mut closure = roots.clone();
    // The intent is a DAG in canonical order; one pass per operation converges.
    for _ in 0..intent.operations.len() {
        for operation in &intent.operations {
            if operation
                .prerequisites()
                .iter()
                .any(|prerequisite| closure.contains(prerequisite))
            {
                closure.insert(operation.id().clone());
            }
        }
    }
    closure.difference(roots).cloned().collect()
}

fn check_attempting_is_earned(trace: &[TraceEvent]) -> Result<(), Violation> {
    let mut run = 0usize;
    let mut absent_this_run: BTreeSet<ReleaseOperationId> = BTreeSet::new();
    let mut previous: BTreeMap<ReleaseOperationId, OperationState> = BTreeMap::new();
    for event in trace {
        match event {
            TraceEvent::RunStart { run: index, .. } => {
                run = *index;
                absent_this_run.clear();
                previous.clear();
            }
            TraceEvent::Observed { id, tag, .. } => {
                if *tag == ObservationTag::Absent {
                    absent_this_run.insert(id.clone());
                }
            }
            TraceEvent::Saved { states, .. } => {
                for (id, state) in states {
                    let newly_attempting =
                        *state == OperationState::Attempting && previous.get(id) != Some(&OperationState::Attempting);
                    if newly_attempting && !absent_this_run.contains(id) {
                        return Err(Violation::AttemptingWithoutAbsentObservation {
                            operation: Box::new(id.clone()),
                            run,
                        });
                    }
                }
                previous = states.clone();
            }
            _ => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Enumeration
// ---------------------------------------------------------------------------

/// The baseline fault-free run, which teaches the enumeration its dimensions.
struct Dimensions {
    observations: usize,
    effects: usize,
    provider_calls: usize,
    saves: usize,
}

fn measure(intent: &ReleaseIntentV1, manifest: &ArtifactManifestV1, state_path: &Path) -> Dimensions {
    let run = run_scenario(
        intent,
        manifest,
        state_path,
        Scenario {
            crash: None,
            fault: None,
        },
        Mode::SameRunner,
        Sabotage::default(),
    );
    assert_eq!(
        run.outcomes,
        vec![RunOutcome::Receipted],
        "the fault-free baseline must reach a receipt in one run"
    );
    let saves = run
        .context
        .trace
        .borrow()
        .iter()
        .filter(|event| matches!(event, TraceEvent::Saved { .. }))
        .count();
    Dimensions {
        observations: run.world.observation_calls.get(),
        effects: run.world.effect_calls.get(),
        provider_calls: run.world.provider_calls.get(),
        saves,
    }
}

fn crash_points(dimensions: &Dimensions) -> Vec<CrashPoint> {
    let mut points: Vec<_> = (0..dimensions.provider_calls)
        .map(|at| CrashPoint::ProviderCall { at })
        .collect();
    for at in 0..dimensions.saves {
        points.push(CrashPoint::Save { at, after: false });
        points.push(CrashPoint::Save { at, after: true });
    }
    points
}

fn faults(dimensions: &Dimensions) -> Vec<Fault> {
    let mut all = Vec::new();
    for kind in FaultKind::ALL {
        let span = if kind.is_observation_fault() {
            dimensions.observations
        } else {
            dimensions.effects
        };
        for at in 0..span {
            all.push(Fault { kind, at });
        }
    }
    all
}

/// Crash-and-fault pairs are sampled on a fixed stride rather than run in
/// full. The full product is 3248 pairs, which runs clean but costs about 35
/// seconds; a stride of 3 keeps the suite near 12 seconds. The sample is a
/// deterministic slice of the same enumeration, and its size is asserted.
const PAIR_STRIDE: usize = 3;

fn scenarios(dimensions: &Dimensions) -> (Vec<Scenario>, usize, usize, usize) {
    let crashes = crash_points(dimensions);
    let faults = faults(dimensions);
    let mut all = Vec::new();
    for crash in &crashes {
        all.push(Scenario {
            crash: Some(*crash),
            fault: None,
        });
    }
    for fault in &faults {
        all.push(Scenario {
            crash: None,
            fault: Some(*fault),
        });
    }
    let singles_crash = crashes.len();
    let singles_fault = faults.len();
    let mut pairs = 0;
    for (index, (crash, fault)) in crashes
        .iter()
        .flat_map(|crash| faults.iter().map(move |fault| (crash, fault)))
        .enumerate()
    {
        if index % PAIR_STRIDE != 0 {
            continue;
        }
        pairs += 1;
        all.push(Scenario {
            crash: Some(*crash),
            fault: Some(*fault),
        });
    }
    (all, singles_crash, singles_fault, pairs)
}

fn describe(scenario: Scenario, mode: Mode) -> String {
    format!("mode={mode:?} crash={:?} fault={:?}", scenario.crash, scenario.fault)
}

/// A short, stable name for one operation, so a failing trace is readable.
fn label(id: &ReleaseOperationId) -> String {
    let role = match &id.role {
        callisto_model::ReleaseOperationRole::RegistryPublish { .. } => "registry".to_owned(),
        callisto_model::ReleaseOperationRole::Tag => "tag".to_owned(),
        callisto_model::ReleaseOperationRole::ForgeRelease => "draft".to_owned(),
        callisto_model::ReleaseOperationRole::ForgePublish => "publish".to_owned(),
        callisto_model::ReleaseOperationRole::ArtifactUpload { slot } => format!("upload({})", slot.asset_name),
        callisto_model::ReleaseOperationRole::PlatformPublish { platform, .. } => {
            format!("platform({})", platform.name().name())
        }
    };
    format!("{}/{role}", id.package.name())
}

fn trace_lines(context: &SimContext) -> String {
    context
        .trace
        .borrow()
        .iter()
        .map(|event| match event {
            TraceEvent::RunStart { run, kind } => format!("  run {run} start {kind:?}"),
            TraceEvent::Observed { run, id, tag } => format!("  run {run} observe {} -> {tag:?}", label(id)),
            TraceEvent::Effect { run, id, landed } => {
                format!("  run {run} effect  {} landed={landed}", label(id))
            }
            TraceEvent::Saved { run, states } => format!(
                "  run {run} save    {}",
                states
                    .iter()
                    .map(|(id, state)| format!("{}={state:?}", label(id)))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            TraceEvent::Crashed { run } => format!("  run {run} CRASH"),
            TraceEvent::RunEnd { run, outcome } => format!("  run {run} end {outcome}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn the_release_executor_survives_every_enumerated_crash_and_provider_fault() {
    let (intent, manifest) = simulator_intent();
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("release-state.json");
    let dimensions = measure(&intent, &manifest, &state_path);
    let (all, singles_crash, singles_fault, pairs) = scenarios(&dimensions);

    println!(
        "RELEASE_SIM operations={} observations={} effects={} provider_calls={} saves={}",
        intent.operations.len(),
        dimensions.observations,
        dimensions.effects,
        dimensions.provider_calls,
        dimensions.saves
    );
    println!(
        "RELEASE_SIM scenarios crash_singles={singles_crash} fault_singles={singles_fault} \
         pairs_sampled={pairs} stride={PAIR_STRIDE} total={} executions={}",
        all.len(),
        all.len() * 2
    );

    // A regression that silently shrinks the enumeration must fail here.
    assert_eq!(intent.operations.len(), 9, "the simulated intent lost an operation");
    assert!(singles_crash >= 50, "crash points shrank to {singles_crash}");
    assert!(singles_fault >= 40, "fault points shrank to {singles_fault}");
    assert!(pairs >= 1000, "the pair sample shrank to {pairs}");

    let mut converged = 0usize;
    let mut conflicted = 0usize;
    let mut stranded = 0usize;
    let mut fresh_unresolved = 0usize;
    for scenario in all {
        for mode in [Mode::SameRunner, Mode::FreshRunner] {
            let run = run_scenario(&intent, &manifest, &state_path, scenario, mode, Sabotage::default());
            let classification = match check_invariants(&intent, &run, mode) {
                Ok(classification) => classification,
                Err(violation) => panic!(
                    "release invariant violated: {violation:?}\nscenario: {}\ntrace:\n{}",
                    describe(scenario, mode),
                    trace_lines(&run.context)
                ),
            };
            converged += usize::from(classification.converged);
            conflicted += usize::from(classification.conflicted);
            stranded += usize::from(classification.stranded);
            if mode == Mode::FreshRunner && !classification.converged && !classification.conflicted {
                fresh_unresolved += 1;
            }
        }
    }
    println!("RELEASE_SIM outcomes converged={converged} conflicted={conflicted} stranded={stranded}");

    // A fresh-journal recovery runner has no stranding excuse: every scenario
    // it faces either converges or is a permanent conflict.
    assert_eq!(
        fresh_unresolved, 0,
        "a fresh recovery runner failed to resolve a scenario"
    );
    assert!(converged >= 1400, "too few scenarios converged: {converged}");
    assert!(conflicted >= 500, "too few conflict scenarios: {conflicted}");
    assert!(
        stranded >= 200,
        "the stranded-attempt path was barely exercised: {stranded}"
    );
}

/// A provider that keeps answering `Absent` about an effect it already served
/// makes a fresh recovery runner issue that effect a second time. The checker
/// must report it, and the exception for genuine registry lag must not excuse
/// it: nothing in this scenario ever lagged.
#[test]
fn the_checker_reports_a_duplicate_landing_against_a_lying_provider() {
    let (intent, manifest) = simulator_intent();
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("release-state.json");
    let scenario = Scenario {
        crash: None,
        fault: None,
    };
    let run = run_scenario(
        &intent,
        &manifest,
        &state_path,
        scenario,
        Mode::FreshRunner,
        Sabotage {
            lying_provider: true,
            lose_saves: false,
        },
    );
    let violation = check_invariants(&intent, &run, Mode::FreshRunner).unwrap_err();
    assert!(
        matches!(violation, Violation::DuplicateLanding { .. }),
        "expected DuplicateLanding, got {violation:?}"
    );
}

/// An effect issued ahead of its prerequisite must be reported, not absorbed.
#[test]
fn the_checker_reports_an_effect_that_precedes_its_prerequisite() {
    let (intent, manifest) = simulator_intent();
    let scenario = Scenario {
        crash: None,
        fault: None,
    };
    let context = Rc::new(SimContext::default());
    let world = SimWorld::new(intent.clone(), scenario, Rc::clone(&context), Sabotage::default());
    let permit = ApplyPermit::force_for_tests();
    let artifacts = VerifiedArtifactManifest::for_tests(&manifest, PathBuf::from("/simulated"));
    let proof = ProviderObservationV1::Absent.absent_proof().unwrap();
    // The tag depends on the registry publish, which has not landed.
    drop(world.publish(&permit, &proof, intent.operations[1].id(), Some(&artifacts)));
    let run = ScenarioRun {
        world,
        context,
        outcomes: vec![RunOutcome::Failed],
    };

    let violation = check_invariants(&intent, &run, Mode::SameRunner).unwrap_err();
    assert!(
        matches!(violation, Violation::EffectBeforePrerequisite { .. }),
        "expected EffectBeforePrerequisite, got {violation:?}"
    );
}

/// A world that loses a landing the receipt claims must be caught: a receipt is
/// only as good as the effects that are really out there.
#[test]
fn the_checker_reports_a_receipt_whose_effects_never_landed() {
    let (intent, manifest) = simulator_intent();
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("release-state.json");
    let scenario = Scenario {
        crash: None,
        fault: None,
    };
    let run = run_scenario(
        &intent,
        &manifest,
        &state_path,
        scenario,
        Mode::SameRunner,
        Sabotage::default(),
    );
    assert_eq!(run.outcomes, vec![RunOutcome::Receipted]);
    run.world
        .remote
        .borrow_mut()
        .insert(intent.operations[0].id().clone(), Remote::Absent);

    let violation = check_invariants(&intent, &run, Mode::SameRunner).unwrap_err();
    assert!(
        matches!(violation, Violation::ReceiptWithoutLandedEffect { .. }),
        "expected ReceiptWithoutLandedEffect, got {violation:?}"
    );
}

/// A writer that accepts every save and persists none of them produces a
/// receipt over a journal no operator could ever read back.
#[test]
fn the_checker_reports_a_receipt_over_a_journal_that_was_never_written() {
    let (intent, manifest) = simulator_intent();
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("release-state.json");
    let scenario = Scenario {
        crash: None,
        fault: None,
    };
    let run = run_scenario(
        &intent,
        &manifest,
        &state_path,
        scenario,
        Mode::SameRunner,
        Sabotage {
            lying_provider: false,
            lose_saves: true,
        },
    );
    assert!(!state_path.exists(), "the sabotaged writer persisted nothing");
    let violation = check_invariants(&intent, &run, Mode::SameRunner).unwrap_err();
    assert!(
        matches!(violation, Violation::ReceiptWithNonSuccessState { .. }),
        "expected ReceiptWithNonSuccessState, got {violation:?}"
    );
}

/// A journal that records `Attempting` with no absent observation behind it is
/// exactly the ordering defect this simulator exists to catch.
#[test]
fn the_checker_reports_attempting_persisted_before_any_observation() {
    let id = simulator_intent().0.operations[0].id().clone();
    let trace = vec![
        TraceEvent::RunStart {
            run: 0,
            kind: ReleaseRunKindV1::Initial,
        },
        TraceEvent::Saved {
            run: 0,
            states: BTreeMap::from([(id.clone(), OperationState::Attempting)]),
        },
    ];
    let violation = check_attempting_is_earned(&trace).unwrap_err();
    assert!(
        matches!(violation, Violation::AttemptingWithoutAbsentObservation { .. }),
        "expected AttemptingWithoutAbsentObservation, got {violation:?}"
    );
    // The same trace with the observation in front of it is legal.
    let mut legal = trace.clone();
    legal.insert(
        1,
        TraceEvent::Observed {
            run: 0,
            id,
            tag: ObservationTag::Absent,
        },
    );
    assert!(check_attempting_is_earned(&legal).is_ok());
}

/// F1: a recovery run that dies partway through reconstruction must remain
/// resumable from the state file it left behind.
///
/// Reconstruction is the only path that adopts an effect an earlier run
/// already landed. When it is interrupted, the operations it never reached
/// stay `Pending` in a state file that now exists -- and a `Pending` registry
/// operation whose version is live is a hard `E174`, not a conflict any rerun
/// can resolve. So the sweep must run on every recovery run, not only on one
/// that started with no state at all.
#[test]
fn a_recovery_run_interrupted_mid_reconstruction_resumes_from_the_same_state() {
    let (intent, manifest) = simulator_intent();
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("release-state.json");
    let context = Rc::new(SimContext::default());
    let scenario = Scenario {
        crash: None,
        fault: Some(Fault {
            kind: FaultKind::ObserveIndeterminate,
            at: 1,
        }),
    };
    let world = SimWorld::new(intent.clone(), scenario, Rc::clone(&context), Sabotage::default());
    // Every effect of an earlier run is already out there; only the journal was lost.
    for operation in &intent.operations {
        world.land(operation.id());
    }
    world.lost_journal_recovery.set(true);
    let recovery = envelope_of_kind(&intent, ReleaseRunKindV1::Recovery);
    let operations: Vec<_> = intent.operations.iter().map(|op| op.id().clone()).collect();
    let store_for = |context: &Rc<SimContext>| {
        ReleaseStateStore::with_writer(
            &state_path,
            SimWriter::new(Rc::clone(context), operations.clone(), None, Sabotage::default()),
        )
    };

    // Run 0: recovery with no journal, stopped partway through reconstruction.
    context.run.set(0);
    let interrupted = execute_and_receipt(&world, &store_for(&context), &manifest, &recovery);
    assert!(
        interrupted.is_err(),
        "the injected indeterminate observation must stop reconstruction"
    );
    assert!(state_path.exists(), "the interrupted run must leave its journal behind");
    let partial = durable_states(&context.trace.borrow());
    let pending: Vec<_> = operations
        .iter()
        .filter(|id| partial.get(*id).copied().unwrap_or(OperationState::Pending) == OperationState::Pending)
        .collect();
    assert!(
        partial.values().any(|state| *state == OperationState::AlreadySatisfied),
        "reconstruction adopted nothing, so this scenario proves nothing: {partial:?}"
    );
    assert!(
        pending
            .iter()
            .any(|id| matches!(id.role, callisto_model::ReleaseOperationRole::RegistryPublish { .. })),
        "a live registry operation must be left pending -- that is the E174 wedge: {partial:?}"
    );

    // Run 1: the same journal, recovery again, with the fault budget spent.
    context.run.set(FAULT_ARMED_RUNS);
    execute_and_receipt(&world, &store_for(&context), &manifest, &recovery)
        .expect("a recovery rerun from the interrupted state must converge");

    let run = ScenarioRun {
        world,
        context,
        outcomes: vec![RunOutcome::Failed, RunOutcome::Receipted],
    };
    if let Err(violation) = check_invariants(&intent, &run, Mode::SameRunner) {
        panic!(
            "release invariant violated: {violation:?}\ntrace:\n{}",
            trace_lines(&run.context)
        );
    }
    assert_eq!(
        run.world.landings.borrow().len(),
        intent.operations.len(),
        "recovery re-issued an effect that was already landed"
    );
}
