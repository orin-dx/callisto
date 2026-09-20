//! Wall-clock and retry policy shared by every release provider.

use std::time::Duration;

use callisto_model::Version;

use crate::GraphError;

/// Wall-clock deadlines for every command the release path issues. Without
/// them one hung provider consumes the whole CI job budget; a breach surfaces
/// as `CommandError::TimedOut` (E025) and, after `Attempting` is persisted,
/// fails closed exactly as any other dispatch error.
pub(crate) mod timeouts {
    use std::time::Duration;

    /// `cargo publish`, `npm publish`, `python -m build`, `twine upload`.
    pub(crate) const PUBLISH: Duration = Duration::from_secs(900);
    /// `cargo info`, `npm view`.
    pub(crate) const REGISTRY_QUERY: Duration = Duration::from_secs(300);
    /// `gh attestation verify`.
    pub(crate) const ATTESTATION_VERIFY: Duration = Duration::from_secs(120);
    /// `gh api`.
    pub(crate) const FORGE_API: Duration = Duration::from_secs(60);
    /// `git ls-remote`.
    pub(crate) const GIT_LS_REMOTE: Duration = Duration::from_secs(60);
    /// `git push`.
    pub(crate) const GIT_PUSH: Duration = Duration::from_secs(120);
    /// `gh release create`.
    pub(crate) const FORGE_RELEASE_CREATE: Duration = Duration::from_secs(120);
    /// `gh release upload`.
    pub(crate) const FORGE_ASSET_UPLOAD: Duration = Duration::from_secs(600);
    /// Purely local git reads (`rev-parse`, `for-each-ref`, `remote get-url`).
    pub(crate) const LOCAL_GIT: Duration = Duration::from_secs(60);
}

/// Retries after this many unconfirmed checks: 3 retries (4 checks total).
pub(crate) const REGISTRY_CONFIRMATION_MAX_RETRIES: u32 = 3;

/// Exponential backoff between confirmation checks: 2s, 4s, 8s (14s worst
/// case) -- registry index propagation is normally sub-second, so this only
/// costs time on the rare lagging case, not the common one.
pub(crate) fn registry_confirmation_backoff(retry: u32) -> Duration {
    Duration::from_secs(2u64.pow(retry + 1))
}

/// Retries `check` up to `max_retries` more times, sleeping `backoff(retry)`
/// between attempts, and returns as soon as it reports published. Returns
/// `Ok(false)` if it never does. `sleep` is injected so tests can verify the
/// retry count and schedule without a real wall-clock wait.
pub(crate) fn poll_until_published(
    mut check: impl FnMut() -> Result<bool, GraphError>,
    max_retries: u32,
    backoff: impl Fn(u32) -> Duration,
    sleep: impl Fn(Duration),
) -> Result<bool, GraphError> {
    for retry in 0..=max_retries {
        if check()? {
            return Ok(true);
        }
        if retry < max_retries {
            sleep(backoff(retry));
        }
    }
    Ok(false)
}

/// A successful publish-client exit isn't a receipt: confirms the registry
/// itself has caught up before treating the operation as done. If not, it
/// stays `Attempting` for reconciliation, reported via its own variant
/// rather than `ReleaseIntentStale`'s false "no operation was authorized".
pub(crate) fn require_registry_confirmation(
    is_published: bool,
    package: &str,
    version: &Version,
) -> Result<(), GraphError> {
    if is_published {
        return Ok(());
    }
    Err(GraphError::RegistryPublishUnconfirmed {
        package: package.to_string(),
        version: version.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::VersionGrammar;

    /// Regression coverage for the confirmed bug: a successful npm publish
    /// whose registry hasn't caught up yet must report `RegistryPublishUnconfirmed`
    /// -- not the unrelated `ReleaseIntentStale`, whose "no release operation
    /// was authorized" help text is false here (an operation WAS authorized
    /// and did run).
    #[test]
    fn require_registry_confirmation_rejects_unconfirmed_publish_with_its_own_variant() {
        let version = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
        let err = require_registry_confirmation(false, "left-pad", &version).unwrap_err();
        match err {
            GraphError::RegistryPublishUnconfirmed { package, version: v } => {
                assert_eq!(package, "left-pad");
                assert_eq!(v, version);
            }
            other => panic!("expected RegistryPublishUnconfirmed, got {other:?}"),
        }
        assert!(
            !matches!(
                require_registry_confirmation(false, "left-pad", &version),
                Err(GraphError::ReleaseIntentStale { .. })
            ),
            "must not be reported as ReleaseIntentStale -- that help text would be false here"
        );
    }

    #[test]
    fn require_registry_confirmation_accepts_a_confirmed_publish() {
        let version = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
        assert!(require_registry_confirmation(true, "left-pad", &version).is_ok());
    }

    #[test]
    fn poll_until_published_succeeds_immediately_without_sleeping() {
        let sleeps = std::cell::RefCell::new(Vec::new());
        let result = poll_until_published(
            || Ok(true),
            REGISTRY_CONFIRMATION_MAX_RETRIES,
            registry_confirmation_backoff,
            |d| sleeps.borrow_mut().push(d),
        );
        assert_eq!(result, Ok(true));
        assert!(sleeps.borrow().is_empty(), "must not sleep when already published");
    }

    #[test]
    fn poll_until_published_succeeds_on_a_later_retry() {
        let attempts = std::cell::RefCell::new(0);
        let sleeps = std::cell::RefCell::new(Vec::new());
        let result = poll_until_published(
            || {
                *attempts.borrow_mut() += 1;
                Ok(*attempts.borrow() == 3)
            },
            REGISTRY_CONFIRMATION_MAX_RETRIES,
            registry_confirmation_backoff,
            |d| sleeps.borrow_mut().push(d),
        );
        assert_eq!(result, Ok(true));
        assert_eq!(*attempts.borrow(), 3);
        assert_eq!(
            *sleeps.borrow(),
            vec![Duration::from_secs(2), Duration::from_secs(4)],
            "must sleep with the exponential schedule between the two failed checks"
        );
    }

    #[test]
    fn poll_until_published_gives_up_after_max_retries_with_no_trailing_sleep() {
        let attempts = std::cell::RefCell::new(0);
        let sleeps = std::cell::RefCell::new(Vec::new());
        let result = poll_until_published(
            || {
                *attempts.borrow_mut() += 1;
                Ok(false)
            },
            REGISTRY_CONFIRMATION_MAX_RETRIES,
            registry_confirmation_backoff,
            |d| sleeps.borrow_mut().push(d),
        );
        assert_eq!(result, Ok(false));
        assert_eq!(*attempts.borrow(), 4, "one initial check plus 3 retries");
        assert_eq!(
            *sleeps.borrow(),
            vec![Duration::from_secs(2), Duration::from_secs(4), Duration::from_secs(8)],
            "must not sleep again after the final (4th) check fails"
        );
    }

    #[test]
    fn poll_until_published_propagates_a_check_error_without_retrying() {
        let attempts = std::cell::RefCell::new(0);
        let version = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
        let result = poll_until_published(
            || {
                *attempts.borrow_mut() += 1;
                Err(GraphError::RegistryPublishUnconfirmed {
                    package: "left-pad".to_string(),
                    version: version.clone(),
                })
            },
            REGISTRY_CONFIRMATION_MAX_RETRIES,
            registry_confirmation_backoff,
            |_| panic!("must not sleep after a hard error"),
        );
        assert!(result.is_err());
        assert_eq!(*attempts.borrow(), 1, "a check error must not be retried");
    }
}
