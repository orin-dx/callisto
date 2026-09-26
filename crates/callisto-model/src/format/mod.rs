//! Byte-compatible reader/writer for `@changesets/cli`'s file formats.
//!
//! - The changeset markdown format: frontmatter parsing/writing, the quoted-vs-bare
//!   name grammar, the empty-changeset validity rule.
//! - `bump_version` and the `Versioning` trait.
//! - `pre.json`'s byte shape.

pub mod bump;
pub mod changeset;
pub mod pre;

pub use bump::{bump_version, versioning_for, BumpError, Pep440Versioning, SemVerVersioning, Versioning};
pub use changeset::{parse_changeset, write_changeset, Changeset, Entry, ParseError, WriteError};
pub use pre::{parse_pre_json, write_pre_json, write_pre_json_preserving, PreJsonError, PreMode, PreState};

pub use crate::{Severity, SeverityParseError};
