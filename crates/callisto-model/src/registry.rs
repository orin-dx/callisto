use crate::{ApplyPermit, PackageId, Version};
use std::time::Duration;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RegistryError {
    #[error("Rate limited. Retry after {0:?}")]
    RateLimited(Duration),
    #[error("Authentication failed: {0}")]
    AuthFailed(String),
    #[error("Network error: {0}")]
    Network(String),
    #[error("Other registry error: {0}")]
    Other(String),
}

/// Outcome of a [`RegistryClient::publish`] call that did not error.
///
/// Ecosystem CLI publishers (`cargo publish`, `npm publish`, `twine upload`)
/// commonly treat "this version is already on the index" as their own
/// success/idempotent case rather than a distinct pre-check result. Surfacing
/// that here lets `publish` itself be the source of truth for "already
/// published", instead of requiring every ecosystem to implement a reliable
/// CLI-only [`RegistryClient::is_published`] pre-check (some ecosystems have
/// none without reaching for the registry's HTTP API, which this design
/// deliberately avoids).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    /// The package/version was newly uploaded to the registry by this call.
    Published,
    /// The package/version was already present on the registry; this call
    /// took no publishing action.
    AlreadyPublished,
}

pub trait RegistryClient: Send + Sync {
    /// Best-effort, optional pre-check for whether `package@version` is
    /// already on the registry. Implementations may always return `Ok(false)`
    /// when no reliable CLI-only check exists for their ecosystem — in that
    /// case `publish`'s own [`PublishOutcome::AlreadyPublished`]
    /// classification is the real source of truth, and callers must not
    /// treat `Ok(false)` here as a guarantee the package is unpublished.
    fn is_published(&self, package: &PackageId, version: &Version) -> Result<bool, RegistryError>;

    /// Publishes `package@version`, or reports that it was already published
    /// as a non-error, non-action outcome.
    ///
    /// The highest-stakes side effect in the codebase -- an upload to a public
    /// registry is not revertible -- so it requires an [`ApplyPermit`]. A dry
    /// run cannot obtain one and therefore cannot reach this method.
    /// [`Self::is_published`] is read-only and needs no permit.
    fn publish(
        &self,
        package: &PackageId,
        version: &Version,
        permit: &ApplyPermit,
    ) -> Result<PublishOutcome, RegistryError>;
}

pub trait RateLimitPolicy: Send + Sync {
    fn check_rate_limit(&self, retry_after: Duration) -> Result<(), RegistryError>;
}

pub trait TimeProvider: Send + Sync {
    fn now(&self) -> std::time::SystemTime;
    fn sleep(&self, duration: Duration);
}
