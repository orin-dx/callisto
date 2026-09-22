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
    /// The subprocess deadline around one registry query (`cargo info`, `npm view`, `curl` PyPI simple index).
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

/// External programs and the remote name the release path invokes.
pub(crate) mod programs {
    pub(crate) const GIT: &str = "git";
    pub(crate) const GH: &str = "gh";
    pub(crate) const CARGO: &str = "cargo";
    pub(crate) const NPM: &str = "npm";
    /// The PyPI simple-index observation query (`curl -sS -i`).
    pub(crate) const CURL: &str = "curl";
    /// The remote whose push URL is the trusted release destination.
    pub(crate) const GIT_REMOTE: &str = "origin";
}

/// Total tries for one read-only observation, including the first.
pub(crate) const OBSERVATION_MAX_ATTEMPTS: u32 = 5;

/// Longest single wait, and the clamp applied to a server's `Retry-After`.
pub(crate) const OBSERVATION_BACKOFF_CAP: Duration = Duration::from_secs(300);

/// Exponential backoff between observation attempts: 2s, 4s, 8s, 16s.
pub(crate) fn observation_backoff(attempt: u32) -> Duration {
    Duration::from_secs(2u64.saturating_pow(attempt + 1)).min(OBSERVATION_BACKOFF_CAP)
}

/// One attempt's result: either it settled the question, or it left the answer
/// unknown for a reason that may resolve on its own.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Attempt<T> {
    Settled(T),
    /// `value` is the answer to report if every attempt is exhausted.
    Transient {
        value: T,
        retry_after: Option<Duration>,
    },
}

/// The wall-clock wait, injected so tests exercise the schedule without it.
pub(crate) trait Sleeper: Sync {
    fn sleep(&self, duration: Duration);
}

pub(crate) struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// The one bounded retry in the release path. It wraps read-only observations
/// only: an effect (publish, push, release create, asset upload) is never
/// re-issued from here, because a transient error cannot prove the first
/// attempt did not take effect.
pub(crate) fn retry_observation<T>(
    sleeper: &dyn Sleeper,
    mut attempt: impl FnMut() -> Result<Attempt<T>, GraphError>,
) -> Result<T, GraphError> {
    let mut exhausted = None;
    for index in 0..OBSERVATION_MAX_ATTEMPTS {
        match attempt()? {
            Attempt::Settled(value) => return Ok(value),
            Attempt::Transient { value, retry_after } => {
                exhausted = Some(value);
                if index + 1 < OBSERVATION_MAX_ATTEMPTS {
                    sleeper.sleep(
                        retry_after
                            .unwrap_or_else(|| observation_backoff(index))
                            .min(OBSERVATION_BACKOFF_CAP),
                    );
                }
            }
        }
    }
    Ok(exhausted.expect("at least one attempt always runs"))
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
pub(crate) mod tests {
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

    /// Records the schedule instead of waiting it out.
    pub(crate) struct RecordingSleeper(std::sync::Mutex<Vec<Duration>>);

    impl RecordingSleeper {
        pub(crate) fn new() -> Self {
            Self(std::sync::Mutex::new(Vec::new()))
        }

        pub(crate) fn waits(&self) -> Vec<Duration> {
            self.0.lock().unwrap().clone()
        }
    }

    impl Sleeper for RecordingSleeper {
        fn sleep(&self, duration: Duration) {
            self.0.lock().unwrap().push(duration);
        }
    }

    #[test]
    fn a_settled_first_attempt_never_sleeps() {
        let sleeper = RecordingSleeper::new();
        let result = retry_observation(&sleeper, || Ok(Attempt::Settled("published")));
        assert_eq!(result, Ok("published"));
        assert!(sleeper.waits().is_empty());
    }

    #[test]
    fn a_transient_answer_is_retried_on_the_exponential_schedule() {
        let sleeper = RecordingSleeper::new();
        let attempts = std::cell::Cell::new(0);
        let result = retry_observation(&sleeper, || {
            attempts.set(attempts.get() + 1);
            Ok(if attempts.get() == 3 {
                Attempt::Settled("published")
            } else {
                Attempt::Transient {
                    value: "unknown",
                    retry_after: None,
                }
            })
        });
        assert_eq!(result, Ok("published"));
        assert_eq!(attempts.get(), 3);
        assert_eq!(sleeper.waits(), vec![Duration::from_secs(2), Duration::from_secs(4)]);
    }

    #[test]
    fn exhausted_attempts_report_the_last_transient_answer_with_no_trailing_sleep() {
        let sleeper = RecordingSleeper::new();
        let attempts = std::cell::Cell::new(0);
        let result = retry_observation(&sleeper, || {
            attempts.set(attempts.get() + 1);
            Ok(Attempt::Transient {
                value: "unknown",
                retry_after: None,
            })
        });
        assert_eq!(result, Ok("unknown"));
        assert_eq!(attempts.get(), i32::try_from(OBSERVATION_MAX_ATTEMPTS).unwrap());
        assert_eq!(
            sleeper.waits(),
            vec![
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16)
            ]
        );
    }

    #[test]
    fn a_servers_retry_after_replaces_the_schedule_and_is_clamped_to_the_cap() {
        let sleeper = RecordingSleeper::new();
        let attempts = std::cell::Cell::new(0);
        let result = retry_observation(&sleeper, || {
            attempts.set(attempts.get() + 1);
            Ok(match attempts.get() {
                1 => Attempt::Transient {
                    value: "unknown",
                    retry_after: Some(Duration::from_secs(9)),
                },
                2 => Attempt::Transient {
                    value: "unknown",
                    retry_after: Some(Duration::from_secs(86_400)),
                },
                _ => Attempt::Settled("published"),
            })
        });
        assert_eq!(result, Ok("published"));
        assert_eq!(
            sleeper.waits(),
            vec![Duration::from_secs(9), OBSERVATION_BACKOFF_CAP],
            "an absurd Retry-After must not park the release for a day"
        );
    }

    #[test]
    fn a_hard_error_is_propagated_without_retrying() {
        let sleeper = RecordingSleeper::new();
        let attempts = std::cell::Cell::new(0);
        let result = retry_observation::<&str>(&sleeper, || {
            attempts.set(attempts.get() + 1);
            Err(GraphError::ReleaseInvariant {
                detail: "hard".to_string(),
            })
        });
        assert!(result.is_err());
        assert_eq!(attempts.get(), 1);
        assert!(sleeper.waits().is_empty());
    }
}
