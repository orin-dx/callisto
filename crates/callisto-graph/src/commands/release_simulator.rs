//! Deterministic fault-injection simulator for the real release executor.
//!
//! The executor's ordering hazards were found by reading and then pinned by a
//! hand-written test each time. This drives the production
//! [`execute_release`](super::release_execution::execute_release) against a
//! simulated world that crashes at every provider call and injects every
//! provider outcome, then asserts the lifecycle invariants after each run.
//! Every rerun starts fresh, as it does on a new CI runner.
//!
//! Nothing here is random and nothing reads a clock: a scenario is a pair of
//! integers plus a fault kind, and the whole enumeration is a nested loop.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;

use callisto_model::{
    AbsentProof, ApplyPermit, ArtifactDigest, ArtifactManifestEntryV1, ArtifactManifestV1, ArtifactSlotId, CommitSha,
    Ecosystem, ExactEvidence, ExecutionTrustProfileV1, GitHubArtifactAttestationV1, GitHubRepository,
    ProviderConflictReason, ProviderEvidenceV1, ProviderIndeterminateCause, ProviderObservationV1,
    RegistryBindingDigest, RegistryBindingId, ReleaseDecisionEntry, ReleaseDecisionV1, ReleaseInclusionReason,
    ReleaseInputSnapshotV1, ReleaseIntentV1, ReleaseOperation, ReleaseOperationId, ReleasePackageId, ReleaseReceiptV1,
    SourceIdentity, Version,
};

use crate::commands::release::provider::preflight_from_observation;
use crate::commands::release_artifacts::VerifiedArtifactManifest;
use crate::commands::release_execution::execute_release;
use crate::commands::release_test_support::{envelope, evidence_for};
use crate::commands::{ReleasePreflight, ReleaseProviderSet};
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

/// `crash` is process death at that provider call of the first run. An effect
/// lands first: the harmless variant is already covered by `EffectFailsBeforeLanding`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Scenario {
    crash: Option<usize>,
    fault: Option<Fault>,
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

#[derive(Clone, Debug)]
enum TraceEvent {
    RunStart {
        run: usize,
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
    Crashed {
        run: usize,
    },
    RunEnd {
        run: usize,
        outcome: String,
    },
}

/// State shared between the simulated world and the scenario driver.
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
    /// Provider calls made while issuing the last receipt; must stay zero.
    calls_after_execution: Cell<usize>,
    /// Every observation outside a post-effect confirmation answers `Absent`
    /// about an effect the world already holds. Used only to prove the checker.
    lying_provider: bool,
}

impl SimWorld {
    fn new(intent: ReleaseIntentV1, scenario: Scenario, context: Rc<SimContext>, lying_provider: bool) -> Self {
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
            calls_after_execution: Cell::new(0),
            lying_provider,
        }
    }

    fn faults_armed(&self) -> bool {
        self.context.run.get() < FAULT_ARMED_RUNS
    }

    /// Consumes one global provider-call slot and crashes if this is the point.
    fn take_provider_call(&self) -> Option<()> {
        let index = self.provider_calls.get();
        self.provider_calls.set(index + 1);
        if self.context.run.get() == 0 && self.scenario.crash == Some(index) {
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
                if self.lying_provider && !confirming {
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
    /// A rerun facing an index that is still serving the pre-publication
    /// answer has observed nothing but `Absent` for this operation and cannot
    /// distinguish lag from a missing effect. Re-issuing there is the
    /// provider's job to refuse -- which the world does, and the
    /// single-landing check asserts. Every other re-issue, including one
    /// caused by a provider that simply lies, is a violation.
    fn excused_duplicate(&self, id: &ReleaseOperationId) -> bool {
        self.lagged_ever.borrow().contains(id)
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
        preflight_from_observation(self.observe_internal(id)?, id, remote_conflict_for(id))
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
    ReceiptEvidenceDisagreesWithWorld {
        operation: Box<ReleaseOperationId>,
        receipt: Option<ProviderObservationV1>,
    },
    ProviderCallAfterExecution {
        calls: usize,
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
    receipt: Option<ReleaseReceiptV1>,
}

/// One end-to-end simulated release, including the operator's reruns.
fn run_scenario(intent: &ReleaseIntentV1, manifest: &ArtifactManifestV1, scenario: Scenario) -> ScenarioRun {
    let context = Rc::new(SimContext::default());
    let world = SimWorld::new(intent.clone(), scenario, Rc::clone(&context), false);
    run_world(world, context, manifest)
}

fn run_world(world: SimWorld, context: Rc<SimContext>, manifest: &ArtifactManifestV1) -> ScenarioRun {
    let mut outcomes = Vec::new();
    let mut receipt = None;
    for run in 0..=MAX_RERUNS {
        context.run.set(run);
        context.crashed.set(false);
        context.push(TraceEvent::RunStart { run });
        let result = execute_and_receipt(&world, manifest);
        let detail = match &result {
            Ok(_) => "receipted".to_owned(),
            Err(failure) => format!("{failure:?}"),
        };
        let outcome = match result {
            Ok(issued) => {
                receipt = Some(issued);
                RunOutcome::Receipted
            }
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
        receipt,
    }
}

/// Which stage of the production success path refused this run. The causes
/// are read from the `{:?}` trace a failing scenario prints.
#[allow(dead_code)]
#[derive(Debug)]
enum RunFailure {
    Execution(Box<GraphError>),
    Receipt(Box<callisto_model::ReleaseReceiptError>),
}

/// The production success path in full: execute, then issue the terminal
/// receipt from the returned state. Nothing short of a receipt counts.
fn execute_and_receipt(world: &SimWorld, manifest: &ArtifactManifestV1) -> Result<ReleaseReceiptV1, RunFailure> {
    let permit = ApplyPermit::force_for_tests();
    let artifacts = VerifiedArtifactManifest::for_tests(manifest, PathBuf::from("/simulated"));
    let state = execute_release(world, &permit, &envelope(&world.intent), Some(&artifacts))
        .map_err(|error| RunFailure::Execution(Box::new(error)))?;
    let calls = world.provider_calls.get();
    let receipt =
        ReleaseReceiptV1::from_state(&world.intent, &state).map_err(|error| RunFailure::Receipt(Box::new(error)))?;
    world.calls_after_execution.set(world.provider_calls.get() - calls);
    Ok(receipt)
}

// ---------------------------------------------------------------------------
// Invariants
// ---------------------------------------------------------------------------

fn check_invariants(intent: &ReleaseIntentV1, run: &ScenarioRun) -> Result<Classification, Violation> {
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

    // I1: a receipt is produced only when the world really holds every effect.
    if run.outcomes.last() == Some(&RunOutcome::Receipted) {
        for operation in &intent.operations {
            if !world.already_landed(operation.id()) {
                return Err(Violation::ReceiptWithoutLandedEffect {
                    operation: Box::new(operation.id().clone()),
                });
            }
        }
        check_receipt_matches_world(intent, run)?;
    }

    check_convergence(intent, run)
}

/// I5: the receipt is issued from execution state alone, with no provider
/// call, and records exactly the evidence of the effect the world holds.
fn check_receipt_matches_world(intent: &ReleaseIntentV1, run: &ScenarioRun) -> Result<(), Violation> {
    let calls = run.world.calls_after_execution.get();
    if calls != 0 {
        return Err(Violation::ProviderCallAfterExecution { calls });
    }
    let receipt = run.receipt.as_ref();
    for operation in &intent.operations {
        let id = operation.id();
        let recorded = receipt.and_then(|receipt| receipt.observation(id)).cloned();
        let held = match run.world.remote.borrow().get(id) {
            Some(Remote::Landed { evidence }) => Some(ProviderObservationV1::Exact {
                evidence: evidence.clone(),
            }),
            _ => None,
        };
        if recorded.is_none() || recorded != held {
            return Err(Violation::ReceiptEvidenceDisagreesWithWorld {
                operation: Box::new(id.clone()),
                receipt: recorded,
            });
        }
    }
    Ok(())
}

/// How one scenario ended, tallied so the enumeration can prove it exercised
/// more than one outcome rather than passing everything vacuously.
#[derive(Clone, Copy, Debug, Default)]
struct Classification {
    converged: bool,
    conflicted: bool,
}

/// I3: every scenario converges within the reruns, unless a permanent
/// conflict surfaced -- and then nothing downstream of it lands.
fn check_convergence(intent: &ReleaseIntentV1, run: &ScenarioRun) -> Result<Classification, Violation> {
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
        // Effects that landed before the conflict surfaced are not
        // implicated, so this walks the trace rather than the end state.
        check_nothing_lands_after_a_conflict(intent, &run.context.trace.borrow())?;
        return Ok(Classification {
            converged: false,
            conflicted: true,
        });
    }
    if converged {
        return Ok(Classification {
            converged: true,
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

// ---------------------------------------------------------------------------
// Enumeration
// ---------------------------------------------------------------------------

/// The baseline fault-free run, which teaches the enumeration its dimensions.
struct Dimensions {
    observations: usize,
    effects: usize,
    provider_calls: usize,
}

fn measure(intent: &ReleaseIntentV1, manifest: &ArtifactManifestV1) -> Dimensions {
    let run = run_scenario(
        intent,
        manifest,
        Scenario {
            crash: None,
            fault: None,
        },
    );
    assert_eq!(
        run.outcomes,
        vec![RunOutcome::Receipted],
        "the fault-free baseline must reach a receipt in one run"
    );
    Dimensions {
        observations: run.world.observation_calls.get(),
        effects: run.world.effect_calls.get(),
        provider_calls: run.world.provider_calls.get(),
    }
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

/// Every crash point, every fault, and every crash-and-fault pair.
fn scenarios(dimensions: &Dimensions) -> (Vec<Scenario>, usize, usize, usize) {
    let crashes: Vec<usize> = (0..dimensions.provider_calls).collect();
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
    for crash in &crashes {
        for fault in &faults {
            all.push(Scenario {
                crash: Some(*crash),
                fault: Some(*fault),
            });
        }
    }
    let pairs = crashes.len() * faults.len();
    (all, crashes.len(), faults.len(), pairs)
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
            TraceEvent::RunStart { run } => format!("  run {run} start"),
            TraceEvent::Observed { run, id, tag } => format!("  run {run} observe {} -> {tag:?}", label(id)),
            TraceEvent::Effect { run, id, landed } => {
                format!("  run {run} effect  {} landed={landed}", label(id))
            }
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
    let dimensions = measure(&intent, &manifest);
    let (all, singles_crash, singles_fault, pairs) = scenarios(&dimensions);

    println!(
        "RELEASE_SIM operations={} observations={} effects={} provider_calls={} scenarios={} \
         crash_singles={singles_crash} fault_singles={singles_fault} pairs={pairs}",
        intent.operations.len(),
        dimensions.observations,
        dimensions.effects,
        dimensions.provider_calls,
        all.len(),
    );

    // A regression that silently shrinks the enumeration must fail here.
    assert_eq!(intent.operations.len(), 9, "the simulated intent lost an operation");
    assert!(singles_crash >= 18, "crash points shrank to {singles_crash}");
    assert!(singles_fault >= 40, "fault points shrank to {singles_fault}");
    assert!(pairs >= 800, "the pair enumeration shrank to {pairs}");

    let mut converged = 0usize;
    let mut conflicted = 0usize;
    for scenario in all {
        let run = run_scenario(&intent, &manifest, scenario);
        let classification = match check_invariants(&intent, &run) {
            Ok(classification) => classification,
            Err(violation) => panic!(
                "release invariant violated: {violation:?}\nscenario: {scenario:?}\ntrace:\n{}",
                trace_lines(&run.context)
            ),
        };
        converged += usize::from(classification.converged);
        conflicted += usize::from(classification.conflicted);
    }
    println!("RELEASE_SIM outcomes converged={converged} conflicted={conflicted}");
    assert!(converged >= 700, "too few scenarios converged: {converged}");
    assert!(conflicted >= 150, "too few conflict scenarios: {conflicted}");
}

/// A rerun after the registry version already landed adopts it, issues no
/// second publish, and completes with a receipt.
#[test]
fn a_rerun_after_a_crate_is_already_published_completes_with_a_receipt() {
    let (intent, manifest) = simulator_intent();
    let context = Rc::new(SimContext::default());
    let scenario = Scenario {
        crash: None,
        fault: None,
    };
    let world = SimWorld::new(intent.clone(), scenario, Rc::clone(&context), false);
    let registry = intent
        .operations
        .iter()
        .find(|operation| {
            matches!(
                operation.id().role,
                callisto_model::ReleaseOperationRole::RegistryPublish { .. }
            )
        })
        .unwrap()
        .id()
        .clone();
    for prerequisite in intent
        .operations
        .iter()
        .find(|operation| operation.id() == &registry)
        .unwrap()
        .prerequisites()
    {
        world.land(prerequisite);
    }
    world.land(&registry);

    let run = run_world(world, context, &manifest);
    assert_eq!(
        run.outcomes,
        vec![RunOutcome::Receipted],
        "{}",
        trace_lines(&run.context)
    );
    if let Err(violation) = check_invariants(&intent, &run) {
        panic!("{violation:?}\n{}", trace_lines(&run.context));
    }
    assert_eq!(
        run.world.landings.borrow().iter().filter(|id| **id == registry).count(),
        1,
        "the already-published version must not be published again"
    );
}

/// AC-13: rerunning a fully published release dispatches nothing and the receipt
/// records `AlreadySatisfied` for every operation.
#[test]
fn ac13_rerun_of_a_fully_published_release_is_already_satisfied_everywhere() {
    let (intent, manifest) = simulator_intent();
    let context = Rc::new(SimContext::default());
    let scenario = Scenario {
        crash: None,
        fault: None,
    };
    let world = SimWorld::new(intent.clone(), scenario, Rc::clone(&context), false);
    for operation in &intent.operations {
        world.land(operation.id());
    }

    let run = run_world(world, context, &manifest);
    assert_eq!(
        run.outcomes,
        vec![RunOutcome::Receipted],
        "{}",
        trace_lines(&run.context)
    );
    assert_eq!(
        run.world.landings.borrow().len(),
        intent.operations.len(),
        "no operation may be dispatched again"
    );
    let receipt = serde_json::to_value(run.receipt.expect("a receipt")).unwrap();
    let outcomes = receipt["outcomes"].as_array().unwrap();
    assert_eq!(outcomes.len(), intent.operations.len());
    for outcome in outcomes {
        assert_eq!(outcome["outcome"]["kind"], "alreadySatisfied", "{outcome}");
    }
}

/// A provider that keeps answering `Absent` about an effect it already served
/// makes a rerun issue that effect a second time. The checker must report it,
/// and the exception for genuine registry lag must not excuse it: nothing in
/// this scenario ever lagged.
#[test]
fn the_checker_reports_a_duplicate_landing_against_a_lying_provider() {
    let (intent, manifest) = simulator_intent();
    // Death right after the first effect lands forces the rerun.
    let scenario = Scenario {
        crash: Some(1),
        fault: None,
    };
    let context = Rc::new(SimContext::default());
    let world = SimWorld::new(intent.clone(), scenario, Rc::clone(&context), true);
    let run = run_world(world, context, &manifest);
    let violation = check_invariants(&intent, &run).unwrap_err();
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
    let world = SimWorld::new(intent.clone(), scenario, Rc::clone(&context), false);
    let permit = ApplyPermit::force_for_tests();
    let artifacts = VerifiedArtifactManifest::for_tests(&manifest, PathBuf::from("/simulated"));
    let proof = ProviderObservationV1::Absent.absent_proof().unwrap();
    // The tag depends on the registry publish, which has not landed.
    drop(world.publish(&permit, &proof, intent.operations[1].id(), Some(&artifacts)));
    let run = ScenarioRun {
        world,
        context,
        outcomes: vec![RunOutcome::Failed],
        receipt: None,
    };

    let violation = check_invariants(&intent, &run).unwrap_err();
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
    let scenario = Scenario {
        crash: None,
        fault: None,
    };
    let run = run_scenario(&intent, &manifest, scenario);
    assert_eq!(run.outcomes, vec![RunOutcome::Receipted]);
    run.world
        .remote
        .borrow_mut()
        .insert(intent.operations[0].id().clone(), Remote::Absent);

    let violation = check_invariants(&intent, &run).unwrap_err();
    assert!(
        matches!(violation, Violation::ReceiptWithoutLandedEffect { .. }),
        "expected ReceiptWithoutLandedEffect, got {violation:?}"
    );
}

/// A receipt whose evidence differs from what the world holds must be caught:
/// execution state is the only thing a receipt reports.
#[test]
fn the_checker_reports_a_receipt_whose_evidence_the_world_does_not_hold() {
    let (intent, manifest) = simulator_intent();
    let scenario = Scenario {
        crash: None,
        fault: None,
    };
    let run = run_scenario(&intent, &manifest, scenario);
    assert_eq!(run.outcomes, vec![RunOutcome::Receipted]);
    let tag = intent
        .operations
        .iter()
        .find(|operation| operation.id().role == callisto_model::ReleaseOperationRole::Tag)
        .unwrap()
        .id()
        .clone();
    run.world.remote.borrow_mut().insert(
        tag,
        Remote::Landed {
            evidence: ProviderEvidenceV1::GitTag {
                peeled_commit: CommitSha::parse(&"c".repeat(40)).unwrap(),
            },
        },
    );

    let violation = check_invariants(&intent, &run).unwrap_err();
    assert!(
        matches!(violation, Violation::ReceiptEvidenceDisagreesWithWorld { .. }),
        "expected ReceiptEvidenceDisagreesWithWorld, got {violation:?}"
    );
}
