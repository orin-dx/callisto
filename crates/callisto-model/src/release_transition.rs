//! The closed transition table for one release operation's state.
//!
//! [`transition`] is the only place an [`OperationState`] is computed. Every
//! event that leaves `Pending` or reaches a success carries a proof token that
//! can only be minted from a provider observation, so `Attempting` is
//! unreachable without having observed the operation absent first.

use crate::release::{OperationBlockReason, OperationState};
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
    /// The effect was issued and then observed exact.
    Confirmed {
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
    Confirmed,
    EffectFailedAndAbsent,
    Blocked,
}

impl OperationEvent {
    /// The exact evidence this event carries, if it is a success event.
    pub fn exact_evidence(&self) -> Option<&ExactEvidence> {
        match self {
            Self::ObservedExactBeforeEffect { evidence } | Self::Confirmed { evidence } => Some(evidence),
            Self::Attempt { .. } | Self::EffectFailedAndAbsent { .. } | Self::Blocked { .. } => None,
        }
    }

    pub fn kind(&self) -> OperationEventKind {
        match self {
            Self::Attempt { .. } => OperationEventKind::Attempt,
            Self::ObservedExactBeforeEffect { .. } => OperationEventKind::ObservedExactBeforeEffect,
            Self::Confirmed { .. } => OperationEventKind::Confirmed,
            Self::EffectFailedAndAbsent { .. } => OperationEventKind::EffectFailedAndAbsent,
            Self::Blocked { .. } => OperationEventKind::Blocked,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("release operation cannot transition from {from:?} on {event:?}")]
pub struct InvalidTransition {
    pub from: OperationState,
    pub event: OperationEventKind,
}

/// The whole legal edge set. Anything absent from this match is unreachable.
pub fn transition(current: OperationState, event: &OperationEvent) -> Result<OperationState, InvalidTransition> {
    let next = match (current, event.kind()) {
        (OperationState::Pending, OperationEventKind::Attempt) => Some(OperationState::Attempting),
        (OperationState::Pending, OperationEventKind::ObservedExactBeforeEffect) => {
            Some(OperationState::AlreadySatisfied)
        }
        (OperationState::Attempting, OperationEventKind::Confirmed) => Some(OperationState::Published),
        (OperationState::Attempting, OperationEventKind::EffectFailedAndAbsent) => Some(OperationState::Failed),
        (OperationState::Pending | OperationState::Attempting, OperationEventKind::Blocked) => {
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
    })
}
