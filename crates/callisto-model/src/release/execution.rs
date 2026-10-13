use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{ArtifactDigest, IntentDigest, ReleaseIntentV1, ReleaseOperationId, SourceIdentity};
use crate::release_observation::{
    ProviderEvidenceV1, ProviderObservationError, ProviderObservationV1, ReleaseOperationObservationV1,
};
use crate::release_transition::OperationEvent;
use crate::CommitSha;

/// A closed, credential-safe reason why an operation was blocked.
///
/// This deliberately carries no endpoint, command output, or arbitrary error
/// text: durable execution records can safely outlive the process that made
/// the observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum OperationBlockReason {
    /// An existing remote or VCS object does not exactly match the intent.
    ConflictingExistingOperation,
    /// A completed attempt could not be conclusively observed after restart.
    IndeterminateAttempt,
    /// Fresh validation no longer agrees with the approved intent.
    StaleValidation,
    /// An exact prerequisite has not reached a successful terminal outcome.
    UnmetPrerequisite,
}

/// A terminal outcome safe to persist in a receipt. No arbitrary error text is durable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
#[non_exhaustive]
pub enum OperationOutcome {
    Published,
    AlreadySatisfied,
    Failed,
    Blocked { reason: OperationBlockReason },
}

/// The immutable identity of one release run, created and validated before
/// the first effect and recorded in the receipt.
///
/// Every field is derived from the intent and the verified artifact manifest
/// rather than asserted by a caller, so no fact here has a second authority.
/// The revisions are intentionally separate even when equal: a run may release
/// an older merged source with the current coordinator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseRunEnvelopeV1 {
    schema_version: u8,
    orchestration_revision: CommitSha,
    release_source_revision: CommitSha,
    intent_digest: IntentDigest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artifact_manifest_digest: Option<ArtifactDigest>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseRunEnvelopeV1Wire {
    schema_version: u8,
    orchestration_revision: CommitSha,
    release_source_revision: CommitSha,
    intent_digest: IntentDigest,
    #[serde(default)]
    artifact_manifest_digest: Option<ArtifactDigest>,
}

impl<'de> Deserialize<'de> for ReleaseRunEnvelopeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ReleaseRunEnvelopeV1Wire::deserialize(deserializer)?;
        if wire.schema_version != ReleaseRunEnvelopeV1::SCHEMA_VERSION {
            return Err(serde::de::Error::custom(ReleaseRunEnvelopeError::UnsupportedSchema {
                found: wire.schema_version,
            }));
        }
        Ok(Self {
            schema_version: wire.schema_version,
            orchestration_revision: wire.orchestration_revision,
            release_source_revision: wire.release_source_revision,
            intent_digest: wire.intent_digest,
            artifact_manifest_digest: wire.artifact_manifest_digest,
        })
    }
}

impl ReleaseRunEnvelopeV1 {
    /// 3 removes the release `profile`.
    pub const SCHEMA_VERSION: u8 = 3;

    /// The only constructor. Source revision, and intent digest are
    /// read out of `intent`; only the coordinator revision and the verified
    /// manifest digest come from the caller, and both are cross-checked
    /// against the intent here, before any effect.
    pub fn new(
        orchestration_revision: CommitSha,
        intent: &ReleaseIntentV1,
        artifact_manifest_digest: Option<ArtifactDigest>,
    ) -> Result<Self, ReleaseRunEnvelopeError> {
        let release_source_revision = match &intent.snapshot.source {
            SourceIdentity::GitCommit { sha } => sha.clone(),
            SourceIdentity::HermeticContent { .. } => return Err(ReleaseRunEnvelopeError::NonGitReleaseSource),
        };
        let envelope = Self {
            schema_version: Self::SCHEMA_VERSION,
            orchestration_revision,
            release_source_revision,
            intent_digest: intent.digest.clone(),
            artifact_manifest_digest,
        };
        envelope.validate_for_intent(intent)?;
        Ok(envelope)
    }

    pub fn orchestration_revision(&self) -> &CommitSha {
        &self.orchestration_revision
    }

    pub fn release_source_revision(&self) -> &CommitSha {
        &self.release_source_revision
    }

    pub fn intent_digest(&self) -> &IntentDigest {
        &self.intent_digest
    }

    pub fn artifact_manifest_digest(&self) -> Option<&ArtifactDigest> {
        self.artifact_manifest_digest.as_ref()
    }

    /// Cross-field validity against the exact intent this run releases.
    pub fn validate_for_intent(&self, intent: &ReleaseIntentV1) -> Result<(), ReleaseRunEnvelopeError> {
        if self.schema_version != Self::SCHEMA_VERSION {
            return Err(ReleaseRunEnvelopeError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        if self.intent_digest != intent.digest {
            return Err(ReleaseRunEnvelopeError::MismatchedIntent);
        }
        match (intent.artifact_slots.is_empty(), &self.artifact_manifest_digest) {
            (false, None) => return Err(ReleaseRunEnvelopeError::MissingArtifactManifest),
            (true, Some(_)) => return Err(ReleaseRunEnvelopeError::UnexpectedArtifactManifest),
            _ => {}
        }
        // The coordinator that built and attested the binaries is the same
        // revision the slots' attestation policy pins; a receipt must not be
        // able to name a coordinator that did not produce these artifacts.
        for slot in &intent.artifact_slots {
            if slot.attestation_policy.workflow_commit != self.orchestration_revision {
                return Err(ReleaseRunEnvelopeError::MismatchedOrchestrationRevision);
            }
        }
        match &intent.snapshot.source {
            SourceIdentity::GitCommit { sha } if sha == &self.release_source_revision => Ok(()),
            SourceIdentity::GitCommit { .. } => Err(ReleaseRunEnvelopeError::MismatchedReleaseSource),
            SourceIdentity::HermeticContent { .. } => Err(ReleaseRunEnvelopeError::NonGitReleaseSource),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseRunEnvelopeError {
    #[error("unsupported release run envelope schema version {found}")]
    UnsupportedSchema { found: u8 },
    #[error("release run envelope is bound to a different intent")]
    MismatchedIntent,
    #[error("release run envelope source does not match the release intent")]
    MismatchedReleaseSource,
    #[error("release run envelope requires a Git commit release source")]
    NonGitReleaseSource,
    #[error("artifact release envelope requires an artifact manifest digest")]
    MissingArtifactManifest,
    #[error("release run envelope carries an artifact manifest digest for a slot-less intent")]
    UnexpectedArtifactManifest,
    #[error("release run envelope orchestration revision is not the revision that attested the artifact slots")]
    MismatchedOrchestrationRevision,
}

/// In-memory state for one intent-bound execution. Pending and Attempting are
/// nonterminal. Each successful operation keeps the exact provider evidence
/// that moved it there, so a receipt is derived from this state alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseExecutionStateV1 {
    intent_digest: IntentDigest,
    envelope: ReleaseRunEnvelopeV1,
    operations: BTreeMap<ReleaseOperationId, OperationState>,
    evidence: BTreeMap<ReleaseOperationId, ProviderEvidenceV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum OperationState {
    Pending,
    Attempting,
    Published,
    AlreadySatisfied,
    Failed,
    Blocked { reason: OperationBlockReason },
}

impl OperationState {
    /// `Published` or `AlreadySatisfied`: the states that carry exact evidence.
    pub fn is_success(self) -> bool {
        matches!(self, Self::Published | Self::AlreadySatisfied)
    }
}

impl ReleaseExecutionStateV1 {
    /// Starts execution for exactly the roster authorized by `intent`, under
    /// the run envelope that was validated before any effect. There is no way
    /// to create a state without an envelope.
    pub fn new(intent: &ReleaseIntentV1, envelope: ReleaseRunEnvelopeV1) -> Result<Self, ReleaseStateError> {
        envelope
            .validate_for_intent(intent)
            .map_err(ReleaseStateError::InvalidEnvelope)?;
        Ok(Self {
            intent_digest: intent.digest.clone(),
            envelope,
            operations: intent
                .operations
                .iter()
                .map(|operation| (operation.id.clone(), OperationState::Pending))
                .collect(),
            evidence: BTreeMap::new(),
        })
    }

    pub fn intent_digest(&self) -> &IntentDigest {
        &self.intent_digest
    }

    /// The run this state belongs to: the single authority for coordinator
    /// revision, release source, and manifest digest.
    pub fn envelope(&self) -> &ReleaseRunEnvelopeV1 {
        &self.envelope
    }

    /// Validates that this state belongs to this exact intent and operation roster.
    pub fn validate_for_intent(&self, intent: &ReleaseIntentV1) -> Result<(), ReleaseStateError> {
        if self.intent_digest != intent.digest {
            return Err(ReleaseStateError::MismatchedIntent);
        }
        self.envelope
            .validate_for_intent(intent)
            .map_err(ReleaseStateError::InvalidEnvelope)?;
        let expected: BTreeSet<_> = intent.operations.iter().map(|operation| operation.id.clone()).collect();
        let actual: BTreeSet<_> = self.operations.keys().cloned().collect();
        if actual != expected {
            return Err(ReleaseStateError::MismatchedOperationRoster);
        }
        Ok(())
    }

    /// Returns the state for an exact operation identity.
    pub fn operation_state(&self, id: &ReleaseOperationId) -> Option<OperationState> {
        self.operations.get(id).copied()
    }

    /// The exact evidence recorded when `id` reached a successful state.
    pub fn operation_evidence(&self, id: &ReleaseOperationId) -> Option<&ProviderEvidenceV1> {
        self.evidence.get(id)
    }

    /// The only way an operation's state ever changes.
    pub fn apply(
        &mut self,
        id: &ReleaseOperationId,
        event: &OperationEvent,
    ) -> Result<OperationState, ReleaseStateError> {
        let state = self
            .operations
            .get_mut(id)
            .ok_or_else(|| ReleaseStateError::UnknownOperation {
                id: Box::new(id.clone()),
            })?;
        let next = crate::release_transition::transition(*state, event).map_err(|source| {
            ReleaseStateError::InvalidTransition {
                id: Box::new(id.clone()),
                source,
            }
        })?;
        let proof = if next.is_success() {
            let proof = event
                .exact_evidence()
                .ok_or_else(|| ReleaseStateError::MissingEvidence {
                    id: Box::new(id.clone()),
                })?;
            check_evidence_role(id, proof.evidence())?;
            Some(proof.evidence().clone())
        } else {
            None
        };
        *state = next;
        if let Some(proof) = proof {
            self.evidence.insert(id.clone(), proof);
        }
        Ok(next)
    }
}

/// Evidence recorded for an operation must belong to that operation's role.
fn check_evidence_role(id: &ReleaseOperationId, evidence: &ProviderEvidenceV1) -> Result<(), ReleaseStateError> {
    ReleaseOperationObservationV1::new(
        id.clone(),
        ProviderObservationV1::Exact {
            evidence: evidence.clone(),
        },
    )
    .map(|_| ())
    .map_err(ReleaseStateError::EvidenceRejected)
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseStateError {
    #[error("release state is bound to a different intent")]
    MismatchedIntent,
    #[error("release state operation roster differs from the bound intent")]
    MismatchedOperationRoster,
    #[error("release state carries an invalid run envelope: {0}")]
    InvalidEnvelope(ReleaseRunEnvelopeError),
    #[error("release state does not contain operation `{id:?}`")]
    UnknownOperation { id: Box<ReleaseOperationId> },
    #[error("operation `{id:?}`: {source}")]
    InvalidTransition {
        id: Box<ReleaseOperationId>,
        source: crate::release_transition::InvalidTransition,
    },
    #[error("successful release operation `{id:?}` carries no provider evidence")]
    MissingEvidence { id: Box<ReleaseOperationId> },
    #[error("release state evidence rejected: {0}")]
    EvidenceRejected(ProviderObservationError),
}

/// A terminal receipt derived only from a complete execution state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseReceiptV1 {
    schema_version: u8,
    intent_digest: IntentDigest,
    envelope: ReleaseRunEnvelopeV1,
    outcomes: BTreeMap<ReleaseOperationId, OperationOutcome>,
    observations: BTreeMap<ReleaseOperationId, ProviderObservationV1>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseReceiptV1Wire {
    schema_version: u8,
    intent_digest: IntentDigest,
    envelope: ReleaseRunEnvelopeV1,
    outcomes: Vec<OperationOutcomeEntryV1>,
    observations: Vec<ReleaseOperationObservationV1>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperationOutcomeEntryV1 {
    operation: ReleaseOperationId,
    outcome: OperationOutcome,
}

impl Serialize for ReleaseReceiptV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        ReleaseReceiptV1Wire {
            schema_version: self.schema_version,
            intent_digest: self.intent_digest.clone(),
            envelope: self.envelope.clone(),
            outcomes: self
                .outcomes
                .iter()
                .map(|(operation, outcome)| OperationOutcomeEntryV1 {
                    operation: operation.clone(),
                    outcome: *outcome,
                })
                .collect(),
            observations: self
                .observations
                .iter()
                .map(|(operation, observation)| {
                    ReleaseOperationObservationV1::new(operation.clone(), observation.clone())
                        .expect("receipt observations were role-checked on construction")
                })
                .collect(),
        }
        .serialize(serializer)
    }
}

impl JsonSchema for ReleaseReceiptV1 {
    fn schema_name() -> String {
        "ReleaseReceiptV1".to_owned()
    }

    fn json_schema(generator: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        ReleaseReceiptV1Wire::json_schema(generator)
    }
}

impl<'de> Deserialize<'de> for ReleaseReceiptV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ReleaseReceiptV1Wire::deserialize(deserializer)?;
        Self::from_wire(wire).map_err(serde::de::Error::custom)
    }
}

impl ReleaseReceiptV1 {
    pub const SCHEMA_VERSION: u8 = 2;

    /// Constructs a receipt from a terminal state alone: every operation must
    /// be successful, and its recorded evidence becomes its observation.
    ///
    /// The run envelope comes from the state, which was created and validated
    /// before the first effect; nothing is assembled after the fact here.
    pub fn from_state(intent: &ReleaseIntentV1, state: &ReleaseExecutionStateV1) -> Result<Self, ReleaseReceiptError> {
        state
            .validate_for_intent(intent)
            .map_err(ReleaseReceiptError::InvalidState)?;
        let mut outcomes = BTreeMap::new();
        let mut observations = BTreeMap::new();
        for (id, operation_state) in &state.operations {
            let outcome = match operation_state {
                OperationState::Published => OperationOutcome::Published,
                OperationState::AlreadySatisfied => OperationOutcome::AlreadySatisfied,
                OperationState::Failed | OperationState::Blocked { .. } => {
                    return Err(ReleaseReceiptError::NonSuccessfulOperation {
                        id: Box::new(id.clone()),
                    });
                }
                OperationState::Pending | OperationState::Attempting => {
                    return Err(ReleaseReceiptError::NonTerminalOperation {
                        id: Box::new(id.clone()),
                    });
                }
            };
            let evidence = state.operation_evidence(id).cloned().ok_or_else(|| {
                ReleaseReceiptError::InvalidState(ReleaseStateError::MissingEvidence {
                    id: Box::new(id.clone()),
                })
            })?;
            outcomes.insert(id.clone(), outcome);
            observations.insert(id.clone(), ProviderObservationV1::Exact { evidence });
        }
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            intent_digest: state.intent_digest.clone(),
            envelope: state.envelope().clone(),
            outcomes,
            observations,
        })
    }

    /// The exact provider observation this receipt records for `id`.
    pub fn observation(&self, id: &ReleaseOperationId) -> Option<&ProviderObservationV1> {
        self.observations.get(id)
    }

    pub fn intent_digest(&self) -> &IntentDigest {
        &self.intent_digest
    }

    /// Validates exact intent and operation-roster binding before reconciliation.
    pub fn validate_for_intent(&self, intent: &ReleaseIntentV1) -> Result<(), ReleaseReceiptError> {
        if self.schema_version != Self::SCHEMA_VERSION {
            return Err(ReleaseReceiptError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        if self.intent_digest != intent.digest {
            return Err(ReleaseReceiptError::MismatchedIntent);
        }
        self.envelope
            .validate_for_intent(intent)
            .map_err(ReleaseReceiptError::InvalidEnvelope)?;
        let expected: BTreeSet<_> = intent.operations.iter().map(|operation| operation.id.clone()).collect();
        let actual: BTreeSet<_> = self.outcomes.keys().cloned().collect();
        if actual != expected {
            return Err(ReleaseReceiptError::MismatchedOperationRoster);
        }
        let observed: BTreeSet<_> = self.observations.keys().cloned().collect();
        if observed != expected {
            return Err(ReleaseReceiptError::MismatchedObservationRoster);
        }
        if self
            .observations
            .values()
            .any(|observation| !observation.is_terminal_success())
        {
            return Err(ReleaseReceiptError::NonExactReceiptObservation);
        }
        Ok(())
    }

    fn from_wire(wire: ReleaseReceiptV1Wire) -> Result<Self, ReleaseReceiptError> {
        if wire.schema_version != Self::SCHEMA_VERSION {
            return Err(ReleaseReceiptError::UnsupportedSchema {
                found: wire.schema_version,
            });
        }
        let mut outcomes = BTreeMap::new();
        for entry in wire.outcomes {
            if !matches!(
                entry.outcome,
                OperationOutcome::Published | OperationOutcome::AlreadySatisfied
            ) {
                return Err(ReleaseReceiptError::NonSuccessfulOperation {
                    id: Box::new(entry.operation),
                });
            }
            if outcomes.insert(entry.operation.clone(), entry.outcome).is_some() {
                return Err(ReleaseReceiptError::DuplicateOperation {
                    id: Box::new(entry.operation),
                });
            }
        }
        let mut observations = BTreeMap::new();
        for entry in wire.observations {
            let (operation, observation) = entry.into_parts();
            if !observation.is_terminal_success() {
                return Err(ReleaseReceiptError::NonExactObservation {
                    id: Box::new(operation),
                    observation: Box::new(observation),
                });
            }
            if observations.insert(operation.clone(), observation).is_some() {
                return Err(ReleaseReceiptError::DuplicateObservation {
                    id: Box::new(operation),
                });
            }
        }
        Ok(Self {
            schema_version: wire.schema_version,
            intent_digest: wire.intent_digest,
            envelope: wire.envelope,
            outcomes,
            observations,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseReceiptError {
    #[error("unsupported release receipt schema version {found}")]
    UnsupportedSchema { found: u8 },
    #[error("release receipt is bound to a different intent")]
    MismatchedIntent,
    #[error("release receipt operation roster differs from the bound intent")]
    MismatchedOperationRoster,
    #[error("release receipt contains duplicate operation `{id:?}")]
    DuplicateOperation { id: Box<ReleaseOperationId> },
    #[error("release receipt cannot be derived from invalid state: {0}")]
    InvalidState(ReleaseStateError),
    #[error("release receipt envelope is not bound to the intent: {0}")]
    InvalidEnvelope(ReleaseRunEnvelopeError),
    #[error("release operation `{id:?}` is not terminal")]
    NonTerminalOperation { id: Box<ReleaseOperationId> },
    #[error("release operation `{id:?}` did not complete successfully")]
    NonSuccessfulOperation { id: Box<ReleaseOperationId> },
    #[error("release receipt observation roster differs from the bound intent")]
    MismatchedObservationRoster,
    #[error("release receipt contains duplicate observation for `{id:?}`")]
    DuplicateObservation { id: Box<ReleaseOperationId> },
    #[error("release receipt observation for `{id:?}` is not exact: {observation:?}")]
    NonExactObservation {
        id: Box<ReleaseOperationId>,
        observation: Box<ProviderObservationV1>,
    },
    #[error("release receipt contains a non-exact provider observation")]
    NonExactReceiptObservation,
}
