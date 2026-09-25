use callisto_model::{Diagnostic, DiagnosticSeverity};

pub mod init;
pub mod matrix;
pub mod pr_body;
pub mod registry_argv;
pub mod release;
pub mod release_artifacts;
pub mod release_decision;
pub mod release_execution;
#[cfg(test)]
mod release_simulator;
#[cfg(test)]
pub(crate) mod release_test_support;
pub mod snapshot;
pub mod status;
pub mod validate;
pub mod version;

pub use callisto_model::{ReleaseDecisionEntry, ReleaseDecisionError, ReleaseDecisionV1, ReleaseInclusionReason};
pub use init::{init, InitOptions};
pub use matrix::{matrix, MatrixOptions};
pub use pr_body::{compose_pr_body, PrBodyOptions};
pub use release::{
    build_release_intent, build_release_intent_with_artifacts, cargo_registry_name, ci_release_route,
    plan_local_release, validate_local_release_intent, validate_release_intent, ArtifactBuildPolicy, CiReleaseRoute,
    LocalReleasePlan, LocalReleaseSource, ReleasePreflight, ReleaseProviderSet, ValidatedReleaseIntent,
};
pub use release_artifacts::{verify_artifact_manifest, VerifiedArtifactManifest};
pub use release_decision::{
    derive_release_commit_decision, derive_release_decision, derive_selected_release_decision,
    derive_unreleased_decision,
};
pub use release_execution::execute_release;
pub use snapshot::plan_snapshot;
pub use status::{status, StatusOptions};
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
