use callisto_model::{Diagnostic, DiagnosticSeverity};

pub mod init;
pub mod matrix;
pub mod pr_body;
pub mod publish;
pub mod registry_argv;
// PR1 ratchet: these call the deliberately #[deprecated] StaleReason::legacy_unclassified().
// release.rs itself reached zero call sites in PR4 (SPEC-ARCH-RELEASE-ERROR-TAXONOMY); the
// constructor stays defined -- and #[deprecated] -- here until release_decision.rs (PR3) also
// reaches zero and it can be deleted entirely.
pub mod release;
pub mod release_artifacts;
#[allow(deprecated)]
pub mod release_decision;
#[allow(deprecated)]
pub mod release_execution;
pub mod release_store;
pub mod snapshot;
pub mod status;
pub mod tag;
pub mod validate;
pub mod version;

pub use callisto_model::{ReleaseDecisionEntry, ReleaseDecisionError, ReleaseDecisionV1, ReleaseInclusionReason};
pub use init::{init, InitOptions};
pub use matrix::{matrix, MatrixOptions};
pub use pr_body::{compose_pr_body, PrBodyOptions};
pub use publish::{filter_plan_by_report, plan_publish, PublishOptions};
pub use release::{
    build_release_intent, validate_release_intent, validate_release_intent_with_state_directory, ValidatedReleaseIntent,
};
pub use release_artifacts::{verify_artifact_manifest, VerifiedArtifactManifest};
pub use release_decision::{derive_release_commit_decision, derive_release_decision, derive_selected_release_decision};
pub use release_execution::{
    execute_release, execute_release_with_artifacts, reconcile_release_execution, ReconciledReleaseExecution,
};
pub use release_store::{AtomicReleaseStateWriter, ReleaseStateStore, ReleaseStateWriter};
pub use snapshot::plan_snapshot;
pub use status::{status, StatusOptions};
pub use tag::{create_tags, create_tags_with_options, TagOptions};
pub use validate::{validate, ValidateOptions};
pub use version::{plan_version, VersionOptions};

pub fn escalate(diagnostics: &mut [Diagnostic], strict: bool, strict_graph: bool) {
    for d in diagnostics {
        let should_escalate = match d.escalated_by {
            Some(callisto_model::StrictFlag::Strict) => strict,
            Some(callisto_model::StrictFlag::StrictGraph) => strict || strict_graph,
            None => false,
        };
        if should_escalate {
            d.severity = DiagnosticSeverity::Error;
        }
    }
}
