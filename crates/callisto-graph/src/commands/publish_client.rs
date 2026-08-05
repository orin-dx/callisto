//! [`RegistryClient`] implementation that shells out to each ecosystem's own
//! publisher CLI (`cargo publish`, `npm publish`, `twine upload`) via
//! [`CommandRunner`], instead of talking to any registry's HTTP API
//! directly. This is a deliberate design constraint: registry HTTP protocols
//! are a moving target that the native tools already track for us, so we
//! never reimplement them here.
//!
//! [`PublishOrchestrator`](super::publish::PublishOrchestrator) is generic
//! over a single [`RegistryClient`] type, so this module exposes one
//! dispatching client ([`SubprocessRegistryClient`]) rather than three
//! separate types — it routes each call to the right ecosystem's command
//! construction/classification based on the [`PackageId`]'s ecosystem tag.
//!
//! ## Known trait-boundary limitation
//!
//! [`RegistryClient::publish`] only receives a [`PackageId`] and [`Version`].
//! Per-package metadata recorded in a [`PublishPlan`](callisto_model::PublishPlan)
//! entry — e.g. `CratePublish::registry`, `NpmPublish::tag`/`registry` — is
//! *not* threaded through this call boundary, because
//! [`PublishOrchestrator::execute`](super::publish::PublishOrchestrator::execute)
//! itself only forwards package identity + version. This client therefore
//! always publishes to each ecosystem's default registry. Extending
//! `RegistryClient::publish` to accept richer per-package publish
//! parameters is a reasonable follow-up, but out of scope here.

use std::path::PathBuf;
use std::time::Duration;

use callisto_model::{
    ApplyPermit, CommandOutput, CommandRunner, Ecosystem, PackageId, PublishOutcome,
    RegistryClient, RegistryError, Version,
};

use super::publish::parse_retry_after;

/// Dispatches [`RegistryClient`] calls to the right ecosystem CLI based on
/// each [`PackageId`]'s ecosystem tag.
pub struct SubprocessRegistryClient<R: CommandRunner> {
    runner: R,
    /// Working directory the ecosystem CLIs are invoked from — typically the
    /// callisto workspace root. `cargo publish -p <name>` and `npm publish
    /// --workspace <name>` both resolve their target package relative to
    /// this directory rather than needing a per-package path.
    cwd: PathBuf,
}

impl<R: CommandRunner> SubprocessRegistryClient<R> {
    pub fn new(runner: R, cwd: PathBuf) -> Self {
        Self { runner, cwd }
    }

    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, RegistryError> {
        self.runner
            .run(program, args, &self.cwd)
            .map_err(|e| RegistryError::Other(e.to_string()))
    }

    // ---- Cargo / crates.io -------------------------------------------

    fn cargo_publish(&self, package: &PackageId) -> Result<PublishOutcome, RegistryError> {
        if package.name().starts_with('-') {
            return Err(RegistryError::Other(format!(
                "invalid package name `{}`: names may not begin with '-' (possible flag injection)",
                package.name()
            )));
        }
        let output = self.run("cargo", &["publish", "-p", package.name(), "--locked"])?;
        classify_cargo_output(&output)
    }

    // ---- npm ------------------------------------------------------------

    fn npm_is_published(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<bool, RegistryError> {
        let spec = format!("{}@{}", package.name(), version.render());
        let output = self.run("npm", &["view", &spec, "--json"])?;

        if output.success() && !output.stdout_trimmed().is_empty() {
            return Ok(true);
        }

        let combined = combined_lower(&output);
        if combined.contains("e404")
            || combined.contains("etarget")
            || combined.contains("no matching version")
            || combined.contains("is not in this registry")
        {
            return Ok(false);
        }

        if let Some(err) = detect_rate_limit(&combined) {
            return Err(err);
        }
        if let Some(err) = detect_auth_failure(&combined, &output.stderr) {
            return Err(err);
        }

        // Ambiguous failure: don't silently treat as "not published" — a
        // false-negative here would cause a duplicate publish attempt.
        Err(RegistryError::Other(format!(
            "npm view failed ambiguously (exit {:?}): {}",
            output.exit_code,
            output.stderr.trim()
        )))
    }

    fn npm_publish(&self, package: &PackageId) -> Result<PublishOutcome, RegistryError> {
        if package.name().starts_with('-') {
            return Err(RegistryError::Other(format!(
                "invalid package name `{}`: names may not begin with '-' (possible flag injection)",
                package.name()
            )));
        }
        let output = self.run(
            "npm",
            &[
                "publish",
                "--workspace",
                package.name(),
                "--access",
                "public",
            ],
        )?;
        classify_npm_publish_output(&output)
    }

    // ---- PyPI -------------------------------------------------------------

    /// Uploads a Python distribution to PyPI (or a compatible index) by
    /// running `twine upload --skip-existing dist/<normalized-name>-<version>*`.
    ///
    /// The package name is normalized to its PEP 427 wheel-filename form
    /// (lowercased, hyphens and dots replaced with underscores) before constructing the
    /// dist-file glob. `twine` expands the glob internally — no shell
    /// expansion is involved, so the literal `*` in the argument is safe.
    ///
    /// Output classification delegates to [`classify_twine_output`]:
    /// - `--skip-existing` causes twine to print `"Skipping … because it
    ///   appears to already exist"` for files already on the index; any such
    ///   mention yields [`PublishOutcome::AlreadyPublished`].
    /// - A clean zero-exit with no skip message yields
    ///   [`PublishOutcome::Published`].
    /// - Rate-limit and auth-failure signals in the output map to the
    ///   corresponding [`RegistryError`] variants; everything else becomes
    ///   [`RegistryError::Other`].
    fn pypi_publish(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<PublishOutcome, RegistryError> {
        if package.name().starts_with('-') {
            return Err(RegistryError::Other(format!(
                "invalid package name `{}`: names may not begin with '-' (possible flag injection)",
                package.name()
            )));
        }
        let normalized = package.name().to_lowercase().replace(['-', '.'], "_");
        let pattern = format!("dist/{normalized}-{}*", version.render());
        let output = self.run("twine", &["upload", "--skip-existing", &pattern])?;
        classify_twine_output(&output)
    }
}

impl<R: CommandRunner> RegistryClient for SubprocessRegistryClient<R> {
    fn is_published(&self, package: &PackageId, version: &Version) -> Result<bool, RegistryError> {
        match package {
            PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                ..
            } => self.npm_is_published(package, version),

            // Cargo: no reliable cargo-CLI-only existence check exists
            // (crates.io has no local "is this on the index" subcommand
            // short of hitting its HTTP API, which this design deliberately
            // avoids). `publish()`'s own AlreadyPublished classification is
            // the real source of truth for Cargo.
            //
            // PyPI: twine's own --skip-existing flag absorbs idempotency at
            // upload time, so a pre-check buys nothing extra here either.
            PackageId::Prefixed {
                ecosystem: Ecosystem::Cargo | Ecosystem::Pypi,
                ..
            } => Ok(false),

            other => Err(RegistryError::Other(format!(
                "no subprocess is_published check configured for package identity `{}`",
                other.display_name()
            ))),
        }
    }

    fn publish(
        &self,
        package: &PackageId,
        version: &Version,
        _permit: &ApplyPermit,
    ) -> Result<PublishOutcome, RegistryError> {
        match package {
            PackageId::Prefixed {
                ecosystem: Ecosystem::Cargo,
                ..
            } => self.cargo_publish(package),
            PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                ..
            } => self.npm_publish(package),
            PackageId::Prefixed {
                ecosystem: Ecosystem::Pypi,
                ..
            } => self.pypi_publish(package, version),
            other => Err(RegistryError::Other(format!(
                "no subprocess publisher configured for package identity `{}`",
                other.display_name()
            ))),
        }
    }
}

// ---- shared classification helpers ---------------------------------------

fn combined_lower(output: &CommandOutput) -> String {
    format!("{}\n{}", output.stdout, output.stderr).to_lowercase()
}

/// Looks for a `retry after <N>` mention in already-lowercased text and
/// parses `<N>` as a whole-second duration via the orchestrator's shared
/// [`parse_retry_after`] helper.
fn extract_retry_after_duration(text_lower: &str) -> Option<Duration> {
    const NEEDLE: &str = "retry after ";
    let idx = text_lower.find(NEEDLE)?;
    let rest = &text_lower[idx + NEEDLE.len()..];
    let token = rest.split_whitespace().next()?;
    let digits: String = token.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    parse_retry_after(&digits)
}

fn detect_rate_limit(text_lower: &str) -> Option<RegistryError> {
    if text_lower.contains("429")
        || text_lower.contains("too many requests")
        || text_lower.contains("rate limit")
    {
        let dur = extract_retry_after_duration(text_lower).unwrap_or(Duration::from_secs(60));
        Some(RegistryError::RateLimited(dur))
    } else {
        None
    }
}

fn detect_auth_failure(text_lower: &str, raw_stderr: &str) -> Option<RegistryError> {
    if text_lower.contains("401")
        || text_lower.contains("403")
        || text_lower.contains("authentication")
        || text_lower.contains("not logged in")
        || text_lower.contains("invalid token")
        || text_lower.contains("forbidden")
    {
        Some(RegistryError::AuthFailed(raw_stderr.trim().to_string()))
    } else {
        None
    }
}

fn classify_cargo_output(output: &CommandOutput) -> Result<PublishOutcome, RegistryError> {
    let combined = combined_lower(output);

    if combined.contains("already exists") || combined.contains("already uploaded") {
        return Ok(PublishOutcome::AlreadyPublished);
    }
    if output.success() {
        return Ok(PublishOutcome::Published);
    }
    if let Some(err) = detect_rate_limit(&combined) {
        return Err(err);
    }
    if let Some(err) = detect_auth_failure(&combined, &output.stderr) {
        return Err(err);
    }
    Err(RegistryError::Other(format!(
        "cargo publish failed (exit {:?}): {}",
        output.exit_code,
        output.stderr.trim()
    )))
}

fn classify_npm_publish_output(output: &CommandOutput) -> Result<PublishOutcome, RegistryError> {
    let combined = combined_lower(output);

    if combined.contains("epublishconflict")
        || combined.contains("previously published")
        || combined.contains("cannot publish over")
    {
        return Ok(PublishOutcome::AlreadyPublished);
    }
    if output.success() {
        return Ok(PublishOutcome::Published);
    }
    if let Some(err) = detect_rate_limit(&combined) {
        return Err(err);
    }
    if let Some(err) = detect_auth_failure(&combined, &output.stderr) {
        return Err(err);
    }
    Err(RegistryError::Other(format!(
        "npm publish failed (exit {:?}): {}",
        output.exit_code,
        output.stderr.trim()
    )))
}

/// Classifies combined `twine upload` output into a [`PublishOutcome`] or
/// [`RegistryError`].
///
/// Priority order (highest wins):
/// 1. Any `"already exist"` substring (from `--skip-existing`) →
///    [`PublishOutcome::AlreadyPublished`]. Partial skips (e.g. sdist
///    skipped, wheel uploaded) cannot be distinguished from a complete skip
///    by text alone, so any skip mention is conservatively classified as
///    already-published rather than over-reporting a fresh publish.
/// 2. Zero exit code with no skip mention → [`PublishOutcome::Published`].
/// 3. Rate-limit signal (`429`, `too many requests`, `rate limit`) →
///    [`RegistryError::RateLimited`] with a parsed or default 60-second
///    retry-after duration.
/// 4. Auth-failure signal (`401`, `403`, `authentication`, etc.) →
///    [`RegistryError::AuthFailed`].
/// 5. Anything else → [`RegistryError::Other`].
fn classify_twine_output(output: &CommandOutput) -> Result<PublishOutcome, RegistryError> {
    let combined = combined_lower(output);

    if combined.contains("already exist") {
        return Ok(PublishOutcome::AlreadyPublished);
    }
    if output.success() {
        return Ok(PublishOutcome::Published);
    }
    if let Some(err) = detect_rate_limit(&combined) {
        return Err(err);
    }
    if let Some(err) = detect_auth_failure(&combined, &output.stderr) {
        return Err(err);
    }
    Err(RegistryError::Other(format!(
        "twine upload failed (exit {:?}): {}",
        output.exit_code,
        output.stderr.trim()
    )))
}

#[cfg(test)]
mod tests {

    fn permit() -> ApplyPermit {
        ApplyPermit::force_for_tests()
    }
    use super::*;
    use callisto_model::{CommandError, VersionGrammar};

    /// Returns a fixed, canned [`CommandOutput`] regardless of the program
    /// or args it's invoked with — each test constructs one client and
    /// drives exactly one `CommandRunner::run` call through it.
    struct ScriptedRunner(CommandOutput);

    impl CommandRunner for ScriptedRunner {
        fn run(
            &self,
            _program: &str,
            _args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<CommandOutput, CommandError> {
            Ok(self.0.clone())
        }
    }

    fn output(exit_code: i32, stdout: &str, stderr: &str) -> CommandOutput {
        CommandOutput {
            exit_code: Some(exit_code),
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    fn cargo_pkg() -> PackageId {
        PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "callisto-model".to_string(),
        }
    }

    fn npm_pkg() -> PackageId {
        PackageId::Prefixed {
            ecosystem: Ecosystem::Npm,
            name: "@callisto/cli".to_string(),
        }
    }

    fn pypi_pkg() -> PackageId {
        PackageId::Prefixed {
            ecosystem: Ecosystem::Pypi,
            name: "callisto-py".to_string(),
        }
    }

    fn v1() -> Version {
        Version::parse("1.2.3", VersionGrammar::SemVer).unwrap()
    }

    fn client(out: CommandOutput) -> SubprocessRegistryClient<ScriptedRunner> {
        SubprocessRegistryClient::new(ScriptedRunner(out), PathBuf::from("/workspace"))
    }

    // ---------------------------------------------------------------- cargo

    #[test]
    fn cargo_publish_success_is_published() {
        let c = client(output(0, "Uploading callisto-model v1.2.3\n", ""));
        assert_eq!(
            c.publish(&cargo_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::Published
        );
    }

    #[test]
    fn cargo_publish_already_exists_is_already_published() {
        let c = client(output(
            101,
            "",
            "error: crate version `1.2.3` is already uploaded\n",
        ));
        assert_eq!(
            c.publish(&cargo_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::AlreadyPublished
        );
    }

    #[test]
    fn cargo_publish_already_exists_alt_wording_is_already_published() {
        let c = client(output(
            101,
            "",
            "crate version already exists on crates.io\n",
        ));
        assert_eq!(
            c.publish(&cargo_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::AlreadyPublished
        );
    }

    #[test]
    fn cargo_publish_rate_limited() {
        let c = client(output(
            101,
            "",
            "error: failed to publish: 429 Too Many Requests, retry after 30 seconds\n",
        ));
        let err = c.publish(&cargo_pkg(), &v1(), &permit()).unwrap_err();
        assert_eq!(err, RegistryError::RateLimited(Duration::from_secs(30)));
    }

    #[test]
    fn cargo_publish_rate_limited_without_parseable_duration_uses_default() {
        let c = client(output(101, "", "error: 429 too many requests\n"));
        let err = c.publish(&cargo_pkg(), &v1(), &permit()).unwrap_err();
        assert_eq!(err, RegistryError::RateLimited(Duration::from_secs(60)));
    }

    #[test]
    fn cargo_publish_auth_failed() {
        let c = client(output(
            101,
            "",
            "error: 401 Unauthorized: invalid token for crates.io\n",
        ));
        let err = c.publish(&cargo_pkg(), &v1(), &permit()).unwrap_err();
        assert!(matches!(err, RegistryError::AuthFailed(_)));
    }

    #[test]
    fn cargo_publish_generic_error() {
        let c = client(output(101, "", "error: failed to parse manifest\n"));
        let err = c.publish(&cargo_pkg(), &v1(), &permit()).unwrap_err();
        assert!(matches!(err, RegistryError::Other(_)));
    }

    #[test]
    fn cargo_is_published_always_false() {
        let c = client(output(0, "", ""));
        assert!(!c.is_published(&cargo_pkg(), &v1()).unwrap());
    }

    // ------------------------------------------------------------------ npm

    #[test]
    fn npm_publish_success_is_published() {
        let c = client(output(0, "+ @callisto/cli@1.2.3\n", ""));
        assert_eq!(
            c.publish(&npm_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::Published
        );
    }

    #[test]
    fn npm_publish_conflict_is_already_published() {
        let c = client(output(
            1,
            "",
            "npm ERR! code EPUBLISHCONFLICT\nnpm ERR! 403 Forbidden - PUT - you cannot publish over the previously published version\n",
        ));
        assert_eq!(
            c.publish(&npm_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::AlreadyPublished
        );
    }

    #[test]
    fn npm_publish_rate_limited() {
        let c = client(output(
            1,
            "",
            "npm ERR! code E429\nnpm ERR! 429 Too Many Requests - retry after 45 seconds\n",
        ));
        let err = c.publish(&npm_pkg(), &v1(), &permit()).unwrap_err();
        assert_eq!(err, RegistryError::RateLimited(Duration::from_secs(45)));
    }

    #[test]
    fn npm_publish_auth_failed() {
        let c = client(output(
            1,
            "",
            "npm ERR! code ENEEDAUTH\nnpm ERR! need auth - you must be logged in (not logged in) to publish packages\n",
        ));
        let err = c.publish(&npm_pkg(), &v1(), &permit()).unwrap_err();
        assert!(matches!(err, RegistryError::AuthFailed(_)));
    }

    #[test]
    fn npm_publish_generic_error() {
        let c = client(output(
            1,
            "",
            "npm ERR! code ENOTDIR\nnpm ERR! not a directory\n",
        ));
        let err = c.publish(&npm_pkg(), &v1(), &permit()).unwrap_err();
        assert!(matches!(err, RegistryError::Other(_)));
    }

    #[test]
    fn npm_is_published_true_on_success_with_output() {
        let c = client(output(0, "{\"version\":\"1.2.3\"}\n", ""));
        assert!(c.is_published(&npm_pkg(), &v1()).unwrap());
    }

    #[test]
    fn npm_is_published_false_on_e404() {
        let c = client(output(
            1,
            "",
            "npm ERR! code E404\nnpm ERR! 404 No matching version found for @callisto/cli@1.2.3\n",
        ));
        assert!(!c.is_published(&npm_pkg(), &v1()).unwrap());
    }

    #[test]
    fn npm_is_published_false_on_etarget() {
        let c = client(output(
            1,
            "",
            "npm ERR! code ETARGET\nnpm ERR! No matching version found\n",
        ));
        assert!(!c.is_published(&npm_pkg(), &v1()).unwrap());
    }

    #[test]
    fn npm_is_published_ambiguous_failure_propagates_as_error() {
        // A network blip or unexpected npm failure must NOT be silently
        // treated as "not published" — that would risk a duplicate publish.
        let c = client(output(
            1,
            "",
            "npm ERR! code ECONNRESET\nnpm ERR! socket hang up\n",
        ));
        let err = c.is_published(&npm_pkg(), &v1()).unwrap_err();
        assert!(matches!(err, RegistryError::Other(_)));
    }

    // ----------------------------------------------------------------- pypi

    #[test]
    fn pypi_publish_success_is_published() {
        let c = client(output(
            0,
            "Uploading callisto_py-1.2.3-py3-none-any.whl\n",
            "",
        ));
        assert_eq!(
            c.publish(&pypi_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::Published
        );
    }

    #[test]
    fn pypi_publish_skip_existing_is_already_published() {
        let c = client(output(
            0,
            "Skipping callisto_py-1.2.3-py3-none-any.whl because it appears to already exist\n",
            "",
        ));
        assert_eq!(
            c.publish(&pypi_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::AlreadyPublished
        );
    }

    #[test]
    fn pypi_publish_rate_limited() {
        let c = client(output(
            1,
            "",
            "HTTPError: 429 Too Many Requests from https://upload.pypi.org/legacy/, retry after 20 seconds\n",
        ));
        let err = c.publish(&pypi_pkg(), &v1(), &permit()).unwrap_err();
        assert_eq!(err, RegistryError::RateLimited(Duration::from_secs(20)));
    }

    #[test]
    fn pypi_publish_auth_failed() {
        let c = client(output(
            1,
            "",
            "HTTPError: 403 Forbidden from https://upload.pypi.org/legacy/ - Invalid or non-existent authentication information\n",
        ));
        let err = c.publish(&pypi_pkg(), &v1(), &permit()).unwrap_err();
        assert!(matches!(err, RegistryError::AuthFailed(_)));
    }

    #[test]
    fn pypi_publish_generic_error() {
        let c = client(output(1, "", "error: dist/ does not exist\n"));
        let err = c.publish(&pypi_pkg(), &v1(), &permit()).unwrap_err();
        assert!(matches!(err, RegistryError::Other(_)));
    }

    #[test]
    fn pypi_is_published_always_false() {
        let c = client(output(0, "", ""));
        assert!(!c.is_published(&pypi_pkg(), &v1()).unwrap());
    }

    /// A runner that captures the last set of args it was called with so tests
    /// can assert on the exact command constructed (not just its output).
    struct CapturingRunner {
        captured_args: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        response: CommandOutput,
    }

    impl CommandRunner for CapturingRunner {
        fn run(
            &self,
            _program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<CommandOutput, CommandError> {
            *self.captured_args.lock().unwrap() = args.iter().map(|s| s.to_string()).collect();
            Ok(self.response.clone())
        }
    }

    /// PEP 427 wheel filenames use `_` as the separator in the distribution
    /// name component. A package named `my-lib` (or `my_lib`) must produce a
    /// glob of `dist/my_lib-1.2.3*`, not `dist/my-lib-1.2.3*`.
    #[test]
    fn pypi_glob_pattern_uses_underscore_not_hyphen() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let runner = CapturingRunner {
            captured_args: std::sync::Arc::clone(&captured),
            response: output(0, "Uploading my_lib-1.2.3-py3-none-any.whl\n", ""),
        };
        let pkg = PackageId::Prefixed {
            ecosystem: Ecosystem::Pypi,
            name: "my-lib".to_string(),
        };
        let version = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
        let c = SubprocessRegistryClient::new(runner, PathBuf::from("/workspace"));
        c.publish(&pkg, &version, &permit()).unwrap();

        let args = captured.lock().unwrap();
        assert!(
            args.iter().any(|a| a == "dist/my_lib-1.2.3*"),
            "expected glob arg 'dist/my_lib-1.2.3*' (PEP 427 underscore) but got: {:?}",
            *args
        );
    }

    // ---------------------------------------------------------- audit gaps
    //
    // Design decisions covering four previously-flagged coverage gaps in
    // the cargo/npm/pypi classifiers:
    //
    // 1. AMBIGUOUS/CONFLICTING output (both an "already exists" phrase and
    //    a rate-limit phrase in the same output): each classifier checks
    //    "already exists" wording *before* checking for a rate limit, so
    //    AlreadyPublished wins. The tests below pin that existing priority
    //    order as intended behavior rather than silently re-deciding it —
    //    changing which check runs first is a real behavior change with
    //    retry-logic implications (a caller that gets AlreadyPublished does
    //    not retry/backoff; a caller that gets RateLimited does), so it's
    //    out of scope for a coverage-gap fix. If this order ever proves
    //    wrong in practice, that should be a deliberate, reviewed change,
    //    not a byproduct of adding tests.
    //
    // 2. EMPTY stdout AND stderr with a non-zero exit code: none of the
    //    "already exists" / success / rate-limit / auth-failure checks can
    //    match on empty text, so every classifier falls through to the
    //    final `Other` branch. That branch formats `output.stderr.trim()`
    //    into the message, which is `""` for empty stderr — a `Other`
    //    variant carrying an empty-but-non-panicking message, not a
    //    misleading "empty" success/failure signal. Asserted for all three
    //    ecosystems.
    //
    // 3. EXIT CODE 0 combined with "already exists"-style text: since the
    //    "already exists" check runs unconditionally before the
    //    `output.success()` check, this is already handled correctly by
    //    existing code — but it had never been exercised with a 0 exit
    //    code (only non-zero-exit already-exists cases existed before).
    //    Tests below close that gap for all three ecosystems.
    //
    // 4. Non-UTF-8 bytes in subprocess output: `CommandOutput::stdout` /
    //    `::stderr` are `String`, which Rust guarantees is valid UTF-8, so
    //    a classifier can never observe raw invalid bytes directly. Every
    //    real `CommandRunner` impl in this workspace (callisto-cli,
    //    callisto-graph's git runner, callisto-moon) already converts
    //    subprocess bytes via `String::from_utf8_lossy` before constructing
    //    `CommandOutput`, which replaces invalid sequences with U+FFFD
    //    ('�') *before* the classifier ever sees them. So the realistic
    //    fixture for "non-UTF-8 bytes in subprocess output" is a `String`
    //    containing embedded U+FFFD characters around/inside otherwise
    //    valid text, confirming substring matching on the valid portions
    //    still works and nothing panics.

    // -- 1. ambiguous already-exists + rate-limit: already-exists wins ----

    #[test]
    fn cargo_publish_conflicting_already_exists_and_rate_limit_prefers_already_published() {
        let c = client(output(
            101,
            "",
            "error: crate version already exists on crates.io\n\
             note: internal retry log: saw 429 Too Many Requests, rate limit hit before conflict was detected\n",
        ));
        assert_eq!(
            c.publish(&cargo_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::AlreadyPublished
        );
    }

    #[test]
    fn npm_publish_conflicting_already_exists_and_rate_limit_prefers_already_published() {
        let c = client(output(
            1,
            "",
            "npm ERR! code EPUBLISHCONFLICT\n\
             npm ERR! cannot publish over the previously published version\n\
             npm ERR! retry log: 429 Too Many Requests during internal retry\n",
        ));
        assert_eq!(
            c.publish(&npm_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::AlreadyPublished
        );
    }

    #[test]
    fn pypi_publish_conflicting_already_exists_and_rate_limit_prefers_already_published() {
        let c = client(output(
            0,
            "Skipping callisto_py-1.2.3-py3-none-any.whl because it appears to already exist\n\
             retry log: encountered 429 too many requests while retrying upload internally\n",
            "",
        ));
        assert_eq!(
            c.publish(&pypi_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::AlreadyPublished
        );
    }

    // -- 2. empty stdout+stderr, non-zero exit -> Other, no panic ---------

    #[test]
    fn cargo_publish_empty_output_nonzero_exit_is_other() {
        let c = client(output(1, "", ""));
        let err = c.publish(&cargo_pkg(), &v1(), &permit()).unwrap_err();
        assert!(matches!(err, RegistryError::Other(_)));
    }

    #[test]
    fn npm_publish_empty_output_nonzero_exit_is_other() {
        let c = client(output(1, "", ""));
        let err = c.publish(&npm_pkg(), &v1(), &permit()).unwrap_err();
        assert!(matches!(err, RegistryError::Other(_)));
    }

    #[test]
    fn pypi_publish_empty_output_nonzero_exit_is_other() {
        let c = client(output(1, "", ""));
        let err = c.publish(&pypi_pkg(), &v1(), &permit()).unwrap_err();
        assert!(matches!(err, RegistryError::Other(_)));
    }

    // -- 3. exit code 0 + already-exists informational text ---------------

    #[test]
    fn cargo_publish_exit_zero_with_already_exists_text_is_already_published() {
        let c = client(output(
            0,
            "note: crate version already exists, nothing to do\n",
            "",
        ));
        assert_eq!(
            c.publish(&cargo_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::AlreadyPublished
        );
    }

    #[test]
    fn npm_publish_exit_zero_with_already_exists_text_is_already_published() {
        let c = client(output(
            0,
            "notice: previously published, nothing to do\n",
            "",
        ));
        assert_eq!(
            c.publish(&npm_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::AlreadyPublished
        );
    }

    #[test]
    fn pypi_publish_exit_zero_with_already_exists_text_is_already_published() {
        let c = client(output(
            0,
            "Skipping callisto_py-1.2.3-py3-none-any.whl because it appears to already exist\n",
            "",
        ));
        assert_eq!(
            c.publish(&pypi_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::AlreadyPublished
        );
    }

    // -- 4. lossily-converted non-UTF-8 output (embedded U+FFFD) -----------

    #[test]
    fn cargo_publish_handles_replacement_characters_without_panicking() {
        let c = client(output(
            101,
            "",
            "error: crate version \u{FFFD}\u{FFFD} already exists on crates.io\n",
        ));
        assert_eq!(
            c.publish(&cargo_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::AlreadyPublished
        );
    }

    #[test]
    fn npm_publish_handles_replacement_characters_without_panicking() {
        let c = client(output(
            1,
            "",
            "npm ERR! \u{FFFD} cannot publish over the previously published version\n",
        ));
        assert_eq!(
            c.publish(&npm_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::AlreadyPublished
        );
    }

    #[test]
    fn pypi_publish_handles_replacement_characters_without_panicking() {
        let c = client(output(
            0,
            "Skipping callisto_py\u{FFFD}-1.2.3-py3-none-any.whl because it appears to already exist\n",
            "",
        ));
        assert_eq!(
            c.publish(&pypi_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::AlreadyPublished
        );
    }

    // ---- flag-injection guard -------------------------------------------

    #[test]
    fn test_publish_client_rejects_flag_like_package_name() {
        // A package name starting with '--' must be rejected before any
        // subprocess is invoked to prevent flag injection into cargo/npm/twine.
        let flag_pkg = |ecosystem: Ecosystem| PackageId::Prefixed {
            ecosystem,
            name: "--registry=https://evil.com".to_string(),
        };

        // cargo
        let c = client(output(0, "", ""));
        let err = c
            .publish(&flag_pkg(Ecosystem::Cargo), &v1(), &permit())
            .unwrap_err();
        assert!(
            matches!(&err, RegistryError::Other(msg) if msg.contains("invalid package name")),
            "cargo: expected invalid-package-name error, got: {err:?}"
        );

        // npm
        let c = client(output(0, "", ""));
        let err = c
            .publish(&flag_pkg(Ecosystem::Npm), &v1(), &permit())
            .unwrap_err();
        assert!(
            matches!(&err, RegistryError::Other(msg) if msg.contains("invalid package name")),
            "npm: expected invalid-package-name error, got: {err:?}"
        );

        // pypi
        let c = client(output(0, "", ""));
        let err = c
            .publish(&flag_pkg(Ecosystem::Pypi), &v1(), &permit())
            .unwrap_err();
        assert!(
            matches!(&err, RegistryError::Other(msg) if msg.contains("invalid package name")),
            "pypi: expected invalid-package-name error, got: {err:?}"
        );
    }

    #[test]
    fn npm_is_published_handles_replacement_characters_without_panicking() {
        // A garbled-but-non-matching payload must still fall through to the
        // ambiguous-failure branch rather than panicking or misclassifying.
        let c = client(output(
            1,
            "",
            "npm ERR! code ECONNRESET\u{FFFD}\nnpm ERR! socket \u{FFFD} hang up\n",
        ));
        let err = c.is_published(&npm_pkg(), &v1()).unwrap_err();
        assert!(matches!(err, RegistryError::Other(_)));
    }
}
