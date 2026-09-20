//! The closed transition table for one release operation's durable state.
//!
//! [`transition`] is the only place an [`OperationState`] is computed. Every
//! event that leaves `Pending` or reaches a success carries a proof token that
//! can only be minted from a provider observation, so `Attempting` is
//! unreachable without having observed the operation absent first.

use crate::release::{OperationBlockReason, OperationState, ReleaseRunKindV1};
use crate::release_observation::{AbsentProof, ExactEvidence};

/// Something that happened to one operation, carrying the proof that
/// authorizes it.
#[derive(Debug)]
pub enum OperationEvent {
    /// About to issue the effect; the provider was observed absent.
    Attempt {
        proof: AbsentProof,
    },
    /// Pre-effect observation found the provider already holds exactly this result.
    ObservedExactBeforeEffect {
        evidence: ExactEvidence,
    },
    /// A recovery run adopted a pre-existing exact effect into a missing journal.
    AdoptedExact {
        evidence: ExactEvidence,
    },
    /// The effect was issued and then observed exact.
    Confirmed {
        evidence: ExactEvidence,
    },
    /// An interrupted attempt was observed exact after restart: it landed.
    RecoveredExact {
        evidence: ExactEvidence,
    },
    /// The effect failed and the provider is proven to hold nothing.
    EffectFailedAndAbsent {
        proof: AbsentProof,
    },
    Blocked {
        reason: OperationBlockReason,
    },
}

/// The discriminant of an [`OperationEvent`], for error reporting and for
/// enumerating the table without minting proof tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OperationEventKind {
    Attempt,
    ObservedExactBeforeEffect,
    AdoptedExact,
    Confirmed,
    RecoveredExact,
    EffectFailedAndAbsent,
    Blocked,
}

impl OperationEvent {
    pub fn kind(&self) -> OperationEventKind {
        match self {
            Self::Attempt { .. } => OperationEventKind::Attempt,
            Self::ObservedExactBeforeEffect { .. } => OperationEventKind::ObservedExactBeforeEffect,
            Self::AdoptedExact { .. } => OperationEventKind::AdoptedExact,
            Self::Confirmed { .. } => OperationEventKind::Confirmed,
            Self::RecoveredExact { .. } => OperationEventKind::RecoveredExact,
            Self::EffectFailedAndAbsent { .. } => OperationEventKind::EffectFailedAndAbsent,
            Self::Blocked { .. } => OperationEventKind::Blocked,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("release operation cannot transition from {from:?} on {event:?} during a {run_kind:?} run")]
pub struct InvalidTransition {
    pub from: OperationState,
    pub event: OperationEventKind,
    pub run_kind: ReleaseRunKindV1,
}

/// The whole legal edge set. Anything absent from this match is unreachable.
pub fn transition(
    current: OperationState,
    event: &OperationEvent,
    run_kind: ReleaseRunKindV1,
) -> Result<OperationState, InvalidTransition> {
    let next = match (current, event.kind(), run_kind) {
        (OperationState::Pending, OperationEventKind::Attempt, _) => Some(OperationState::Attempting),
        (OperationState::Pending, OperationEventKind::ObservedExactBeforeEffect, _) => {
            Some(OperationState::AlreadySatisfied)
        }
        // Adopting a pre-existing effect into a missing journal is a recovery
        // lifecycle privilege; a normal run must dispatch or fail closed.
        (OperationState::Pending, OperationEventKind::AdoptedExact, ReleaseRunKindV1::Recovery) => {
            Some(OperationState::AlreadySatisfied)
        }
        // Our own attempt landed, which is not the same fact as "it already existed".
        (OperationState::Attempting, OperationEventKind::Confirmed, _)
        | (OperationState::Attempting, OperationEventKind::RecoveredExact, _) => Some(OperationState::Published),
        (OperationState::Attempting, OperationEventKind::EffectFailedAndAbsent, _) => Some(OperationState::Failed),
        (OperationState::Pending | OperationState::Attempting, OperationEventKind::Blocked, _) => {
            let OperationEvent::Blocked { reason } = event else {
                unreachable!("event kind Blocked is produced only by OperationEvent::Blocked")
            };
            Some(OperationState::Blocked { reason: *reason })
        }
        _ => None,
    };
    next.ok_or(InvalidTransition {
        from: current,
        event: event.kind(),
        run_kind,
    })
}
