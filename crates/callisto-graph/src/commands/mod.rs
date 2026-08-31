use callisto_model::{Diagnostic, DiagnosticSeverity};

pub mod init;
pub mod matrix;
pub mod pr_body;
pub mod publish;
pub mod publish_client;
pub mod release;
pub mod release_execution;
pub mod release_store;
pub mod snapshot;
pub mod status;
pub mod tag;
pub mod validate;
pub mod version;

pub use init::{init, InitOptions};
pub use matrix::{matrix, MatrixOptions};
pub use pr_body::{compose_pr_body, PrBodyOptions};
pub use publish::{
    filter_plan_by_report, parse_retry_after, plan_publish, AlwaysRetryPolicy, PublishOptions, PublishOrchestrator,
    SystemTimeProvider,
};
pub use publish_client::SubprocessRegistryClient;
pub use release::{build_release_intent, validate_release_intent, ReleaseSelection, ValidatedReleaseIntent};
pub use release_execution::{reconcile_release_execution, ReconciledReleaseExecution};
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
