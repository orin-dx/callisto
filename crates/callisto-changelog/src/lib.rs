//! Changelog generation and management for callisto.

pub mod error;
pub mod group;
pub mod input;
pub mod render;
pub mod write;

pub use error::ChangelogError;
pub use group::{group_entries, GroupedEntries};
pub use input::{ChangeSource, ChangelogEntry, ChangelogInput};
pub use render::render_section;
pub use write::{extract_section, prepend};
