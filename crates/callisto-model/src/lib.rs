//! Shared types and traits for callisto: package identity, versions, manifests, dependency
//! specs, and the versioned JSON report contract.

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

pub mod registry;
pub use registry::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync_static<T: Send + Sync + 'static>() {}

    #[test]
    fn test_auto_traits() {
        assert_send_sync_static::<PackageId>();
        assert_send_sync_static::<Version>();
        assert_send_sync_static::<Severity>();
        assert_send_sync_static::<Ecosystem>();
        assert_send_sync_static::<Package>();
        assert_send_sync_static::<PublishPlan>();
        assert_send_sync_static::<PublishReport>();
        assert_send_sync_static::<VersionReport>();
        assert_send_sync_static::<StatusReport>();
    }
}
