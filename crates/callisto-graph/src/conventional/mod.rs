//! Conventional commit inference for callisto.

pub mod commit;
pub mod error;
pub mod infer;
pub mod pre_cursor;
pub mod severity;
pub mod window;

pub use commit::{parse_commit, CommitFooter, ConventionalCommit, ParsedCommit};
pub use error::ConventionalError;
pub use infer::{infer_severity, InferenceInput, InferredSeverity};
pub use pre_cursor::{advance_pre_cursor, pre_cursor_ref_name, resolve_pre_cursor};
pub use severity::{raw_severity, raw_severity_of};
pub use window::{fetch_commits, InferenceWindow};
