//! Conventional commit inference for callisto.

pub mod commit;
pub mod error;
pub mod infer;
pub mod severity;
pub mod window;

pub use commit::{parse_commit, CommitFooter, ConventionalCommit, ParsedCommit};
pub use error::ConventionalError;
pub use infer::{infer_severity, InferenceInput, InferredSeverity};
pub use severity::{raw_severity, raw_severity_of};
pub use window::{fetch_commits, InferenceWindow};
