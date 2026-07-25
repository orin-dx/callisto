use callisto_model::{Diagnostic, DiagnosticSeverity};

pub mod init;
pub mod pr_body;
pub mod publish;
pub mod snapshot;
pub mod status;
pub mod tag;
pub mod validate;
pub mod version;

pub use init::{init, InitOptions};
pub use pr_body::{compose_pr_body, PrBodyOptions};
pub use publish::{plan_publish, PublishOptions};
pub use snapshot::plan_snapshot;
pub use status::{status, StatusOptions};
pub use tag::create_tags;
pub use validate::{validate, ValidateOptions};
pub use version::{plan_version, VersionOptions};

pub fn escalate(diagnostics: &mut [Diagnostic], strict: bool, strict_graph: bool) {
    for d in diagnostics {
        let should_escalate = match d.escalated_by {
            Some(callisto_model::StrictFlag::Strict) => strict,
            Some(callisto_model::StrictFlag::StrictGraph) => strict_graph,
            None => false,
        };
        if should_escalate {
            d.severity = DiagnosticSeverity::Error;
        }
    }
}
