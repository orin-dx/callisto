//! Byte-compatible reader/writer for `@changesets/cli`'s file formats.
//!
//! - The changeset markdown format (§6.1): frontmatter parsing/writing, the quoted-vs-bare
//!   name grammar, the empty-changeset validity rule.
//! - `bump_version` (§6.2) and the `Versioning` trait (§7.7).
//! - `pre.json`'s byte shape (§6.4, §8).

pub mod bump;
pub mod changeset;
pub mod pre;

pub use bump::{bump_version, BumpError, SemVerVersioning, Versioning};
pub use changeset::{parse_changeset, write_changeset, Changeset, Entry, ParseError, WriteError};
pub use pre::{parse_pre_json, write_pre_json, PreJsonError, PreMode, PreState};

pub use callisto_model::{Severity, SeverityParseError};
