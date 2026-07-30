use crate::{PackageId, Version};
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

pub trait RegistryClient: Send + Sync {
    fn is_published(&self, package: &PackageId, version: &Version) -> Result<bool, RegistryError>;
    fn publish(&self, package: &PackageId, version: &Version) -> Result<(), RegistryError>;
}

pub trait RateLimitPolicy: Send + Sync {
    fn check_rate_limit(&self, retry_after: Duration) -> Result<(), RegistryError>;
}

pub trait TimeProvider: Send + Sync {
    fn now(&self) -> std::time::SystemTime;
    fn sleep(&self, duration: Duration);
}
