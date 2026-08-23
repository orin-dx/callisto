use crate::{ApplyPermit, PackageId, Version};
use std::time::Duration;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryError {
    #[error("Rate limited. Retry after {0:?}")]
    RateLimited(Duration),
    #[error("Authentication failed: {0}")]
    AuthFailed(String),
    /// Distinct network-layer failure (e.g. DNS resolution, TCP connect
    /// timeout) separate from a registry-level auth or rate-limit response.
    /// Reserved for registry implementations that can reliably distinguish
    /// network failures from other errors; `SubprocessRegistryClient` currently
    /// maps all unrecognised subprocess failures to `Other`.
    #[error("Network error: {0}")]
    Network(String),
    #[error("Other registry error: {0}")]
    Other(String),
}

impl RegistryError {
    /// Returns a camelCase string identifying the variant for use as a
    /// machine-readable discriminator in JSON output. Matches the `errorKind`
    /// field on `PublishAttemptResult::Failed`.
    pub fn kind_str(&self) -> &'static str {
        match self {
            RegistryError::RateLimited(_) => "rateLimited",
            RegistryError::AuthFailed(_) => "authFailed",
            RegistryError::Network(_) => "network",
            RegistryError::Other(_) => "other",
        }
    }
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

/// Gate on whether a publish retry should proceed after a registry reports
/// [`RegistryError::RateLimited`]. Called with the registry-supplied `retry_after` duration
/// immediately before the caller would sleep and retry; returning `Err` aborts the retry
/// instead (the `?`-propagated error becomes the publish attempt's final failure). The
/// production implementation (`AlwaysRetryPolicy`, `callisto-graph`) always permits the
/// retry — the caller's own overall wait-time cutoff is what bounds retries, not this trait —
/// but the seam exists so a caller with a stricter policy (or a test asserting retry
/// behavior without real waiting) can substitute one without touching the retry loop itself.
pub trait RateLimitPolicy: Send + Sync {
    fn check_rate_limit(&self, retry_after: Duration) -> Result<(), RegistryError>;
}

/// Injectable clock and sleep seam for publish-retry backoff, so retry logic is testable
/// without a real wall-clock wait. The production implementation (`SystemTimeProvider`,
/// `callisto-graph`) wraps `std::time::SystemTime::now()`/`std::thread::sleep`; a test
/// double can record calls and return instantly instead.
pub trait TimeProvider: Send + Sync {
    fn now(&self) -> std::time::SystemTime;
    fn sleep(&self, duration: Duration);
}

/// Redacts every occurrence of any non-empty string in `secrets` from
/// `text`, replacing each with `[REDACTED]`, then strips any URL userinfo
/// component (`scheme://user:pass@host` -> `scheme://host`) regardless of
/// whether it matched a known secret.
///
/// Callers use this on raw subprocess stderr before it's embedded in a
/// [`RegistryError`] and, downstream, a JSON report or CI log — registry
/// CLIs (`cargo publish`, `npm publish`, `twine upload`) can echo a
/// credential verbatim in their own error diagnostics on auth failure.
///
/// Pure and injectable (`secrets` rather than reading the environment
/// directly) so it stays deterministic and test-friendly; see
/// [`known_credential_env_values`] for the real caller-side source of
/// `secrets`.
pub fn redact_known_secrets(text: &str, secrets: &[String]) -> String {
    let mut redacted = text.to_string();
    for secret in secrets {
        if !secret.is_empty() {
            redacted = redacted.replace(secret.as_str(), "[REDACTED]");
        }
    }
    redact_url_userinfo(&redacted)
}

/// Filters an env-var snapshot down to registry-credential values this
/// codebase's publish flow reads: fixed `NPM_TOKEN`/`TWINE_PASSWORD`/
/// `CARGO_REGISTRY_TOKEN`/`GITHUB_TOKEN`/`GH_TOKEN` names, plus any
/// `CARGO_REGISTRIES_<NAME>_TOKEN` (operator-configured, unbounded name,
/// so matched by pattern). `GITHUB_TOKEN`/`GH_TOKEN` cover GitHub Actions'
/// ambient credential -- `redact_url_userinfo` already catches its most
/// realistic leak shape (an authenticated remote URL) independently, but
/// matching the env var names directly is cheap insurance against other
/// leak shapes.
///
/// Takes the snapshot as a parameter (callers pass `std::env::vars()`)
/// rather than reading the environment directly -- same
/// `env: impl Fn(&str) -> Result<String, VarError>` pattern
/// `callisto-cli`'s `check_credentials` uses, for the same reason:
/// deterministic, no process-env mutation in tests.
pub fn known_credential_env_values(vars: impl Iterator<Item = (String, String)>) -> Vec<String> {
    vars.filter(|(key, value)| {
        !value.is_empty()
            && (matches!(
                key.as_str(),
                "NPM_TOKEN" | "TWINE_PASSWORD" | "CARGO_REGISTRY_TOKEN" | "GITHUB_TOKEN" | "GH_TOKEN"
            ) || (key.starts_with("CARGO_REGISTRIES_") && key.ends_with("_TOKEN")))
    })
    .map(|(_, value)| value)
    .collect()
}

/// Strips a URL's userinfo (`user:pass@` between `scheme://` and the
/// host) from every `scheme://...` occurrence in `text`, replacing it
/// with `[REDACTED]@`. Token-by-token: within the span from `scheme://`
/// to the next whitespace/quote/`<`/`>`, everything up to the *last* `@`
/// is treated as userinfo.
///
/// Deliberately conservative, not strictly RFC 3986-aware: userinfo with
/// a raw `/` or a second `@` (an unescaped base64-ish token) is realistic
/// in free-form CLI stderr, and using the *first* `@` would under-redact
/// exactly that case. Cost: a rare false positive (an `@` that's really
/// part of a path, e.g. an npm scoped package in
/// `registry.npmjs.org/@myorg/pkg`, gets redacted too) -- acceptable,
/// since over-redaction only costs log noise while under-redaction leaks
/// a credential.
fn redact_url_userinfo(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(scheme_pos) = rest.find("://") {
        let split_at = scheme_pos + 3;
        result.push_str(&rest[..split_at]);
        let after = &rest[split_at..];
        let stop = after
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>'))
            .unwrap_or(after.len());
        let candidate = &after[..stop];
        if let Some(at_pos) = candidate.rfind('@') {
            result.push_str("[REDACTED]@");
            rest = &after[at_pos + 1..];
            continue;
        }
        rest = after;
    }
    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_a_known_secret_value_from_text() {
        let text = "npm ERR! 403 Forbidden - PUT https://registry.npmjs.org/pkg - token abc123secret was rejected";
        let redacted = redact_known_secrets(text, &["abc123secret".to_string()]);
        assert!(!redacted.contains("abc123secret"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_multiple_occurrences_of_the_same_secret() {
        let text = "token=s3cr3t failed; retrying with token=s3cr3t again";
        let redacted = redact_known_secrets(text, &["s3cr3t".to_string()]);
        assert!(!redacted.contains("s3cr3t"));
        assert_eq!(redacted.matches("[REDACTED]").count(), 2);
    }

    #[test]
    fn leaves_text_unchanged_when_no_secrets_match() {
        let text = "cargo publish failed: crate version already exists";
        let redacted = redact_known_secrets(text, &["unrelated-secret".to_string()]);
        assert_eq!(redacted, text);
    }

    #[test]
    fn empty_string_secret_in_the_list_does_not_corrupt_output() {
        // `"".replace("", "X")` would insert "X" between every character --
        // confirm the empty-secret guard actually prevents that footgun.
        let text = "normal error message";
        let redacted = redact_known_secrets(text, &[String::new(), "real-secret".to_string()]);
        assert_eq!(redacted, text);
    }

    #[test]
    fn redacts_url_userinfo_even_when_it_matches_no_known_secret() {
        // A private-registry URL with embedded basic-auth credentials,
        // echoed verbatim by a registry CLI's own error diagnostics, is
        // not necessarily one of the fixed credential env vars -- this
        // must be caught independently of the `secrets` list.
        let text = "error: failed to fetch https://alice:hunter2@registry.example.com/pkg.tgz: 401";
        let redacted = redact_known_secrets(text, &[]);
        assert!(!redacted.contains("hunter2"));
        assert!(!redacted.contains("alice"));
        assert_eq!(
            redacted,
            "error: failed to fetch https://[REDACTED]@registry.example.com/pkg.tgz: 401"
        );
    }

    #[test]
    fn does_not_touch_a_url_with_no_userinfo() {
        let text = "fetching https://registry.npmjs.org/pkg failed with 404";
        let redacted = redact_known_secrets(text, &[]);
        assert_eq!(redacted, text);
    }

    #[test]
    fn redacts_userinfo_when_the_password_itself_contains_a_slash() {
        // A raw '/' inside userinfo isn't RFC 3986-valid, but this is
        // free-form CLI stderr text, not a parsed URL -- a base64-ish
        // token/password containing '/' is realistic, and under-redacting
        // it would defeat the whole point of this function. Bias toward
        // over-redaction, never under-redaction.
        let text = "error: failed to fetch https://alice:p/ss@registry.example.com/index: 401";
        let redacted = redact_known_secrets(text, &[]);
        assert!(!redacted.contains("p/ss"));
        assert_eq!(
            redacted,
            "error: failed to fetch https://[REDACTED]@registry.example.com/index: 401"
        );
    }

    #[test]
    fn redacts_userinfo_when_the_password_itself_contains_an_at_sign() {
        let text = "error: failed to fetch https://alice:p@ssw0rd@registry.example.com/index: 401";
        let redacted = redact_known_secrets(text, &[]);
        assert!(!redacted.contains("ssw0rd"));
        assert_eq!(
            redacted,
            "error: failed to fetch https://[REDACTED]@registry.example.com/index: 401"
        );
    }

    #[test]
    fn conservatively_redacts_a_bare_at_sign_after_a_path_segment_too() {
        // An `@` appearing after a `/` (e.g. an npm scoped package name
        // embedded in a URL path, `registry.npmjs.org/@myorg/pkg`) is
        // usually not userinfo -- but distinguishing that from a
        // slash-containing password with plain text scanning is not
        // reliable, and this function's whole purpose is credential
        // redaction, so it deliberately treats *any* `@` before the next
        // whitespace/quote boundary as a userinfo marker. A little log
        // noise on an unusual scoped-package error is an acceptable
        // trade-off for never under-redacting a real credential.
        let text = "GET https://registry.npmjs.org/@myorg/pkg failed";
        let redacted = redact_known_secrets(text, &[]);
        assert_eq!(redacted, "GET https://[REDACTED]@myorg/pkg failed");
    }

    #[test]
    fn known_credential_env_values_matches_fixed_and_dynamic_names() {
        let snapshot = vec![
            ("NPM_TOKEN".to_string(), "npm-secret".to_string()),
            ("TWINE_PASSWORD".to_string(), "twine-secret".to_string()),
            ("CARGO_REGISTRY_TOKEN".to_string(), "cargo-secret".to_string()),
            (
                "CARGO_REGISTRIES_MY_REGISTRY_TOKEN".to_string(),
                "dynamic-secret".to_string(),
            ),
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("NPM_TOKEN_BUT_NOT_QUITE".to_string(), "not-a-match".to_string()),
        ];
        let values = known_credential_env_values(snapshot.into_iter());
        assert_eq!(values.len(), 4);
        assert!(values.contains(&"npm-secret".to_string()));
        assert!(values.contains(&"twine-secret".to_string()));
        assert!(values.contains(&"cargo-secret".to_string()));
        assert!(values.contains(&"dynamic-secret".to_string()));
    }

    #[test]
    fn known_credential_env_values_skips_empty_values() {
        let snapshot = vec![("NPM_TOKEN".to_string(), String::new())];
        let values = known_credential_env_values(snapshot.into_iter());
        assert!(values.is_empty());
    }

    /// GITHUB_TOKEN/GH_TOKEN cover GitHub Actions' own ambient credential --
    /// realistic exposure is git echoing an authenticated remote URL
    /// (`https://x-access-token:TOKEN@github.com/...`) in subprocess stderr.
    /// The URL form is already caught by `redact_url_userinfo` regardless of
    /// this list, but matching the env var names directly is cheap
    /// insurance for a token that leaks in some other, non-URL shape.
    #[test]
    fn known_credential_env_values_matches_github_token_names() {
        let snapshot = vec![
            ("GITHUB_TOKEN".to_string(), "gh-secret".to_string()),
            ("GH_TOKEN".to_string(), "gh-cli-secret".to_string()),
        ];
        let values = known_credential_env_values(snapshot.into_iter());
        assert_eq!(values.len(), 2);
        assert!(values.contains(&"gh-secret".to_string()));
        assert!(values.contains(&"gh-cli-secret".to_string()));
    }
}
