#![allow(clippy::unwrap_used)]
//! Dev-only byte-compat corpus samples and shared test doubles for
//! callisto's crates.

pub mod corpus;
pub mod dry_run;
pub mod git;
pub mod locked_dir;
pub mod perf;

/// Writes what an answer-free `callisto init` produces: a header-only
/// `callisto.toml` and `.changeset/README.md`.
pub fn scaffold_callisto(root: &std::path::Path) {
    std::fs::write(root.join("callisto.toml"), "# callisto configuration\n").unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(root.join(".changeset/README.md"), "# Changesets\n").unwrap();
}
