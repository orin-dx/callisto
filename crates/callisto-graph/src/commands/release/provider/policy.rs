//! Wall-clock and retry policy shared by every release provider.

use std::path::Path;
use std::time::Duration;

use callisto_model::{CommandError, CommandOutput, CommandRunner, ProviderObservationV1, Version};

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

/// How long a version this run just published may take to appear on its registry's read path.
///
/// Sized to the cache lifetime of the read the observation uses, plus a minute of slack: npm serves a package's
/// document with `max-age=300` (a missing package's 404 too), so a read that primed the cache just before the publish
/// can answer stale for up to five minutes. `cargo publish` waits for the index itself.
pub(crate) fn default_visibility_window(ecosystem: callisto_model::Ecosystem) -> Duration {
    match ecosystem {
        callisto_model::Ecosystem::Npm | callisto_model::Ecosystem::Pypi => Duration::from_secs(360),
        _ => Duration::from_secs(120),
    }
}

/// Longest single wait while a published version is not yet visible.
const VISIBILITY_STEP_CAP: Duration = Duration::from_secs(30);

/// Waits of 2s, 4s, 8s and 16s, then 30s steps, so a stale read is retried often without hammering the registry.
pub(crate) fn visibility_backoff(attempt: u32) -> Duration {
    observation_backoff(attempt).min(VISIBILITY_STEP_CAP)
}

/// Retries `attempt` while it reports `Transient`, until `window` of waiting is spent; the last answer is then reported.
///
/// Unlike [`retry_observation`] the bound is time, not tries: how long a registry lags is a property of the registry,
/// not of how many times it was asked.
pub(crate) fn retry_within_window<T>(
    sleeper: &dyn Sleeper,
    window: Duration,
    mut attempt: impl FnMut() -> Result<Attempt<T>, GraphError>,
) -> Result<T, GraphError> {
    let mut waited = Duration::ZERO;
    let mut index = 0;
    loop {
        match attempt()? {
            Attempt::Settled(value) => return Ok(value),
            Attempt::Transient { value, retry_after } => {
                let remaining = window.saturating_sub(waited);
                if remaining.is_zero() {
                    return Ok(value);
                }
                let wait = retry_after
                    .unwrap_or_else(|| visibility_backoff(index))
                    .clamp(Duration::from_secs(1), OBSERVATION_BACKOFF_CAP)
                    .min(remaining);
                sleeper.sleep(wait);
                waited += wait;
                index += 1;
            }
        }
    }
}

/// Re-observes this run's own publish while the registry reads `Absent` or cannot be asked, for up to `window`; any
/// other answer is final.
pub(crate) fn observe_within_window(
    sleeper: &dyn Sleeper,
    window: Duration,
    mut observe: impl FnMut() -> Result<ProviderObservationV1, GraphError>,
) -> Result<ProviderObservationV1, GraphError> {
    retry_within_window(sleeper, window, || {
        let observation = observe()?;
        Ok(match observation {
            ProviderObservationV1::Absent | ProviderObservationV1::Indeterminate { .. } => Attempt::Transient {
                value: observation,
                retry_after: None,
            },
            settled => Attempt::Settled(settled),
        })
    })
}

/// GitHub's reads lag its writes by seconds; waits of 2s to 32s cover about a minute.
pub(crate) const EFFECT_VISIBILITY_ATTEMPTS: u32 = 6;

/// Re-observes this run's own effect while it reads `Absent`; any other answer is final.
pub(crate) fn observe_until_visible(
    sleeper: &dyn Sleeper,
    mut observe: impl FnMut() -> Result<ProviderObservationV1, GraphError>,
) -> Result<ProviderObservationV1, GraphError> {
    let mut index = 0;
    loop {
        let observation = observe()?;
        if !matches!(observation, ProviderObservationV1::Absent) || index + 1 >= EFFECT_VISIBILITY_ATTEMPTS {
            return Ok(observation);
        }
        sleeper.sleep(observation_backoff(index));
        index += 1;
    }
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
        std::thread::sleep(scaled_for_tests(duration));
    }
}

/// Test-only, like cargo's `__CARGO_TEST_*`: e2e tests can't inject a `Sleeper`.
fn scaled_for_tests(duration: Duration) -> Duration {
    match std::env::var("__CALLISTO_TEST_SLEEP_SCALE") {
        Ok(raw) => match raw.parse::<f64>() {
            Ok(scale) if scale.is_finite() && scale >= 0.0 => duration.mul_f64(scale),
            _ => duration,
        },
        Err(_) => duration,
    }
}

/// A command that could not be run at all (timed out or failed to spawn), unlike one that ran and exited non-zero.
#[derive(Debug)]
pub(crate) struct CommandUnavailable;

/// Translates a timeout or I/O failure into `CommandUnavailable` so `retry_observation` can retry it, instead
/// of propagating a hard `GraphError`. `NotFound` stays a hard error: retrying can't install a missing program.
pub(crate) fn run_observation(
    runner: &dyn CommandRunner,
    program: &str,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
    quiet: bool,
) -> Result<Result<CommandOutput, CommandUnavailable>, GraphError> {
    let result = if quiet {
        runner.run_quiet(program, args, cwd, timeout)
    } else {
        runner.run_with_timeout(program, args, cwd, timeout)
    };
    match result {
        Ok(output) => Ok(Ok(output)),
        Err(CommandError::TimedOut { .. } | CommandError::Io { .. }) => Ok(Err(CommandUnavailable)),
        Err(other) => Err(other.into()),
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
    waited: Duration,
) -> Result<(), GraphError> {
    if is_published {
        return Ok(());
    }
    Err(GraphError::RegistryPublishUnconfirmed {
        package: package.to_string(),
        version: version.clone(),
        waited,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use callisto_model::VersionGrammar;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Hands back one scripted `run`/`run_quiet`/`run_with_timeout` result per call, in order.
    struct ScriptedCommand(Mutex<VecDeque<Result<CommandOutput, CommandError>>>);

    impl ScriptedCommand {
        fn new(results: Vec<Result<CommandOutput, CommandError>>) -> Self {
            Self(Mutex::new(results.into()))
        }
    }

    impl CommandRunner for ScriptedCommand {
        fn run(&self, _program: &str, _args: &[&str], _cwd: &std::path::Path) -> Result<CommandOutput, CommandError> {
            self.0.lock().unwrap().pop_front().expect("no more scripted results")
        }
    }

    fn ok_output() -> CommandOutput {
        CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    /// A timed-out command must become `CommandUnavailable`, not propagate as a hard `GraphError`.
    #[test]
    fn a_timed_out_command_is_unavailable_not_a_hard_error() {
        let runner = ScriptedCommand::new(vec![Err(CommandError::TimedOut {
            program: "cargo".to_string(),
            seconds: 300,
        })]);
        let result = run_observation(&runner, "cargo", &[], std::path::Path::new("."), Duration::ZERO, true);
        assert!(matches!(result, Ok(Err(CommandUnavailable))));
    }

    #[test]
    fn an_io_failure_is_unavailable_not_a_hard_error() {
        let runner = ScriptedCommand::new(vec![Err(CommandError::Io {
            program: "git".to_string(),
            message: "broken pipe".to_string(),
        })]);
        let result = run_observation(&runner, "git", &[], std::path::Path::new("."), Duration::ZERO, false);
        assert!(matches!(result, Ok(Err(CommandUnavailable))));
    }

    /// A missing program can never resolve by retrying, so it stays a hard error.
    #[test]
    fn a_missing_program_is_still_a_hard_error() {
        let runner = ScriptedCommand::new(vec![Err(CommandError::NotFound {
            program: "cargo".to_string(),
        })]);
        let result = run_observation(&runner, "cargo", &[], std::path::Path::new("."), Duration::ZERO, true);
        assert!(matches!(
            result,
            Err(GraphError::Command(CommandError::NotFound { .. }))
        ));
    }

    #[test]
    fn a_successful_run_passes_the_output_through() {
        let runner = ScriptedCommand::new(vec![Ok(ok_output())]);
        let result = run_observation(&runner, "cargo", &[], std::path::Path::new("."), Duration::ZERO, true);
        assert_eq!(result.unwrap().unwrap(), ok_output());
    }

    /// Composes `run_observation` with `retry_observation` so a command that times out once still retries and settles.
    #[test]
    fn a_write_not_yet_visible_is_observed_again_until_it_appears() {
        let sleeper = RecordingSleeper::new();
        let mut answers = vec![
            ProviderObservationV1::Absent,
            ProviderObservationV1::Absent,
            ProviderObservationV1::Indeterminate {
                cause: callisto_model::ProviderIndeterminateCause::CommandFailed,
            },
        ]
        .into_iter();
        let observed = observe_until_visible(&sleeper, || Ok(answers.next().unwrap())).unwrap();
        assert!(matches!(observed, ProviderObservationV1::Indeterminate { .. }));
        assert_eq!(sleeper.waits(), vec![Duration::from_secs(2), Duration::from_secs(4)]);
    }

    #[test]
    fn a_write_that_never_appears_is_reported_absent_after_about_a_minute() {
        let sleeper = RecordingSleeper::new();
        let mut calls = 0;
        let observed = observe_until_visible(&sleeper, || {
            calls += 1;
            Ok(ProviderObservationV1::Absent)
        })
        .unwrap();
        assert!(matches!(observed, ProviderObservationV1::Absent));
        assert_eq!(calls, EFFECT_VISIBILITY_ATTEMPTS);
        assert_eq!(sleeper.waits().iter().sum::<Duration>(), Duration::from_secs(62));
    }

    #[test]
    fn retry_observation_recovers_from_one_timed_out_attempt() {
        let runner = ScriptedCommand::new(vec![
            Err(CommandError::TimedOut {
                program: "cargo".to_string(),
                seconds: 300,
            }),
            Ok(ok_output()),
        ]);
        let sleeper = RecordingSleeper::new();
        let result = retry_observation(&sleeper, || {
            match run_observation(&runner, "cargo", &[], std::path::Path::new("."), Duration::ZERO, true)? {
                Ok(output) => Ok(Attempt::Settled(output)),
                Err(CommandUnavailable) => Ok(Attempt::Transient {
                    value: ok_output(),
                    retry_after: None,
                }),
            }
        });
        assert_eq!(result, Ok(ok_output()));
        assert_eq!(sleeper.waits(), vec![Duration::from_secs(2)]);
    }

    /// Regression coverage for the confirmed bug: a successful npm publish
    /// whose registry hasn't caught up yet must report `RegistryPublishUnconfirmed`
    /// -- not the unrelated `ReleaseIntentStale`, whose "no release operation
    /// was authorized" help text is false here (an operation WAS authorized
    /// and did run).
    #[test]
    fn require_registry_confirmation_rejects_unconfirmed_publish_with_its_own_variant() {
        let version = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
        let waited = Duration::from_secs(360);
        let err = require_registry_confirmation(false, "left-pad", &version, waited).unwrap_err();
        match err {
            GraphError::RegistryPublishUnconfirmed {
                package,
                version: v,
                waited: w,
            } => {
                assert_eq!(package, "left-pad");
                assert_eq!(v, version);
                assert_eq!(w, waited);
            }
            other => panic!("expected RegistryPublishUnconfirmed, got {other:?}"),
        }
        assert!(
            !matches!(
                require_registry_confirmation(false, "left-pad", &version, Duration::ZERO),
                Err(GraphError::ReleaseIntentStale { .. })
            ),
            "must not be reported as ReleaseIntentStale -- that help text would be false here"
        );
    }

    #[test]
    fn require_registry_confirmation_accepts_a_confirmed_publish() {
        let version = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
        assert!(require_registry_confirmation(true, "left-pad", &version, Duration::ZERO).is_ok());
    }

    fn transient_answer() -> Result<Attempt<&'static str>, GraphError> {
        Ok(Attempt::Transient {
            value: "absent",
            retry_after: None,
        })
    }

    /// The registry never shows the version: the whole window is waited out, no more and no less, then the last answer
    /// is reported.
    #[test]
    fn a_version_that_never_appears_waits_the_whole_window_and_no_longer() {
        let sleeper = RecordingSleeper::new();
        let window = Duration::from_secs(360);
        let result = retry_within_window(&sleeper, window, transient_answer);
        assert_eq!(result, Ok("absent"));
        let waits = sleeper.waits();
        assert_eq!(waits.iter().sum::<Duration>(), window);
        assert_eq!(
            waits[..6],
            [2, 4, 8, 16, 30, 30].map(Duration::from_secs),
            "short waits first, then a steady 30s"
        );
        assert!(waits.iter().all(|wait| *wait <= Duration::from_secs(30)));
    }

    /// A version that shows up partway through is returned as soon as it does, without waiting out the rest.
    #[test]
    fn a_version_that_appears_stops_the_wait() {
        let sleeper = RecordingSleeper::new();
        let tries = std::cell::Cell::new(0);
        let result = retry_within_window(&sleeper, Duration::from_secs(360), || {
            tries.set(tries.get() + 1);
            if tries.get() < 7 {
                transient_answer()
            } else {
                Ok(Attempt::Settled("exact"))
            }
        });
        assert_eq!(result, Ok("exact"));
        assert_eq!(sleeper.waits().len(), 6);
    }

    /// A server's own `Retry-After` is honoured, but never past the end of the window.
    #[test]
    fn a_retry_after_is_honoured_but_clipped_to_the_remaining_window() {
        let sleeper = RecordingSleeper::new();
        let result = retry_within_window(&sleeper, Duration::from_secs(100), || {
            Ok(Attempt::Transient {
                value: (),
                retry_after: Some(Duration::from_secs(70)),
            })
        });
        assert_eq!(result, Ok(()));
        assert_eq!(sleeper.waits(), [70, 30].map(Duration::from_secs));
    }

    /// A zero `Retry-After` cannot spin: every wait is at least a second.
    #[test]
    fn a_zero_retry_after_still_makes_progress() {
        let sleeper = RecordingSleeper::new();
        let result = retry_within_window(&sleeper, Duration::from_secs(5), || {
            Ok(Attempt::Transient {
                value: (),
                retry_after: Some(Duration::ZERO),
            })
        });
        assert_eq!(result, Ok(()));
        assert_eq!(sleeper.waits(), [1; 5].map(Duration::from_secs));
    }

    /// A hard error is never waited on.
    #[test]
    fn a_hard_error_inside_the_window_is_returned_at_once() {
        let sleeper = RecordingSleeper::new();
        let result: Result<(), GraphError> = retry_within_window(&sleeper, Duration::from_secs(360), || {
            Err(GraphError::ReleaseInvariant {
                detail: "boom".to_owned(),
            })
        });
        assert!(result.is_err());
        assert!(sleeper.waits().is_empty());
    }

    /// Absent and an unreachable registry are lag inside the window; an exact version or a conflict ends it.
    #[test]
    fn observing_within_the_window_waits_on_absent_and_unreachable_only() {
        let sleeper = RecordingSleeper::new();
        let answers = std::cell::RefCell::new(vec![
            ProviderObservationV1::Absent,
            ProviderObservationV1::Indeterminate {
                cause: callisto_model::ProviderIndeterminateCause::CommandFailed,
            },
            ProviderObservationV1::Conflict {
                reason: callisto_model::ProviderConflictReason::RegistryVersionYanked,
            },
        ]);
        let result = observe_within_window(&sleeper, Duration::from_secs(360), || {
            Ok(answers.borrow_mut().remove(0))
        });
        assert_eq!(
            result,
            Ok(ProviderObservationV1::Conflict {
                reason: callisto_model::ProviderConflictReason::RegistryVersionYanked
            })
        );
        assert_eq!(sleeper.waits(), [2, 4].map(Duration::from_secs));
    }

    #[test]
    fn the_default_window_covers_npms_five_minute_cache() {
        use callisto_model::Ecosystem;
        assert!(default_visibility_window(Ecosystem::Npm) > Duration::from_secs(300));
        assert!(default_visibility_window(Ecosystem::Pypi) > Duration::from_secs(300));
        assert!(default_visibility_window(Ecosystem::Cargo) >= Duration::from_secs(60));
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
