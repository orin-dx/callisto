//! Shared types and traits for callisto: package identity, versions, manifests, dependency
//! specs, and the versioned JSON report contract. [`format`](mod@format) reads and writes changeset files and
//! `pre.json`; [`vcs`] is Git access through the git CLI.

pub mod atomic;

pub mod strict_fs;

pub mod format;

pub mod vcs;

pub mod permit;
pub use permit::*;

pub mod tag;
pub use tag::*;

pub mod error;
pub use error::*;

pub mod path;
pub use path::*;

pub mod identity;
pub use identity::*;

pub mod ecosystem;
pub use ecosystem::*;

pub mod version;
pub use version::*;

pub mod severity;
pub use severity::*;

pub mod package;
pub use package::*;

pub mod dependency;
pub use dependency::*;

pub mod discovery;
pub use discovery::*;

pub mod exec;
pub use exec::*;

pub mod commit;
pub use commit::*;

pub mod diagnostic;
pub use diagnostic::*;

pub mod plan;
pub use plan::*;

pub mod report;
pub use report::*;

pub mod matrix;
pub use matrix::*;

pub mod registry;
pub use registry::*;

pub mod release;
pub use release::*;

pub mod release_observation;
pub use release_observation::*;

pub mod release_transition;
pub use release_transition::*;

pub mod release_pr;
pub use release_pr::*;
