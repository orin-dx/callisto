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
//! ## Per-package publish metadata
//!
//! [`RegistryClient::publish`] only receives a [`PackageId`] and [`Version`].
//! To thread per-package metadata recorded in a
//! [`PublishPlan`](callisto_model::PublishPlan) entry — e.g.
//! `CratePublish::registry`, `NpmPublish::tag`/`access`, `PypiPublish::index`
//! — through this call boundary, callers **must** invoke
//! [`SubprocessRegistryClient::load_plan`] with the full plan before passing
//! the client to the orchestrator. The client stores the metadata in
//! per-ecosystem lookup maps keyed by package name and consults them at
//! dispatch time inside `RegistryClient::publish`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use callisto_model::{
    ApplyPermit, CommandOutput, CommandRunner, Ecosystem, NpmAccess, PackageId, PublishOutcome,
    RegistryClient, RegistryError, Version,
};

use super::publish::parse_retry_after;

/// Per-package npm publish metadata stored in the metadata map.
struct NpmMeta {
    tag: Option<String>,
    access: Option<NpmAccess>,
}

/// Dispatches [`RegistryClient`] calls to the right ecosystem CLI based on
/// each [`PackageId`]'s ecosystem tag.
///
/// Before calling through the [`RegistryClient`] trait, invoke
/// [`Self::load_plan`] so that per-package metadata (dist-tag, access level,
/// registry URL, private index URL) is threaded through to the CLI args.
pub struct SubprocessRegistryClient<R: CommandRunner> {
    runner: R,
    /// Working directory the ecosystem CLIs are invoked from — typically the
    /// callisto workspace root. `cargo publish -p <name>` and `npm publish
    /// --workspace <name>` both resolve their target package relative to
    /// this directory rather than needing a per-package path.
    cwd: PathBuf,
    /// npm publish metadata keyed by package name.
    npm_meta: HashMap<String, NpmMeta>,
    /// Cargo registry name (e.g. `"my-private-registry"`) keyed by crate name.
    cargo_registry: HashMap<String, Option<String>>,
    /// PyPI index URL keyed by distribution name.
    pypi_index: HashMap<String, Option<String>>,
}

impl<R: CommandRunner> SubprocessRegistryClient<R> {
    pub fn new(runner: R, cwd: PathBuf) -> Self {
        Self {
            runner,
            cwd,
            npm_meta: HashMap::new(),
            cargo_registry: HashMap::new(),
            pypi_index: HashMap::new(),
        }
    }

    /// Pre-loads per-package publish metadata from a [`callisto_model::PublishPlan`]
    /// so that `RegistryClient::publish` can thread the correct dist-tag,
    /// access level, registry, and private index URL through to each
    /// ecosystem CLI invocation. Must be called before any publish attempt
    /// that should respect plan-level overrides.
    pub fn load_plan(&mut self, plan: &callisto_model::PublishPlan) {
        for pkg in &plan.npm_platform_packages {
            self.npm_meta.insert(
                pkg.name.clone(),
                NpmMeta {
                    tag: pkg.tag.clone(),
                    access: pkg.access.clone(),
                },
            );
        }
        for pkg in &plan.npm_main_packages {
            self.npm_meta.insert(
                pkg.name.clone(),
                NpmMeta {
                    tag: pkg.tag.clone(),
                    access: pkg.access.clone(),
                },
            );
        }
        for pkg in &plan.rust_crates {
            self.cargo_registry
                .insert(pkg.name.clone(), pkg.registry.clone());
        }
        for pkg in &plan.pypi_packages {
            self.pypi_index.insert(pkg.name.clone(), pkg.index.clone());
        }
    }

    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, RegistryError> {
        self.runner
            .run(program, args, &self.cwd)
            .map_err(|e| RegistryError::Other(e.to_string()))
    }

    // ---- Cargo / crates.io -------------------------------------------

    /// Publishes a crate via `cargo publish`.
    ///
    /// When `registry` is `Some`, `--registry <name>` is appended so that
    /// private registries (e.g. Cloudsmith, Artifactory) are targeted instead
    /// of crates.io.
    fn cargo_publish(
        &self,
        package: &PackageId,
        registry: Option<&str>,
    ) -> Result<PublishOutcome, RegistryError> {
        if package.name().starts_with('-') {
            return Err(RegistryError::Other(format!(
                "invalid package name `{}`: names may not begin with '-' (possible flag injection)",
                package.name()
            )));
        }
        let output = if let Some(reg) = registry {
            self.run(
                "cargo",
                &[
                    "publish",
                    "-p",
                    package.name(),
                    "--locked",
                    "--registry",
                    reg,
                ],
            )?
        } else {
            self.run("cargo", &["publish", "-p", package.name(), "--locked"])?
        };
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

    /// Publishes an npm package via `npm publish`.
    ///
    /// - `tag`: when `Some`, appends `--tag <tag>` (e.g. `"next"` for pre-releases).
    ///   Without this, npm uses the implicit `"latest"` dist-tag.
    /// - `access`: when `Some(NpmAccess::Public)`, appends `--access public`;
    ///   when `Some(NpmAccess::Restricted)`, appends `--access restricted`;
    ///   when `None`, omits `--access` entirely so npm applies its ecosystem
    ///   default (`restricted` for `@scoped/packages`, `public` for unscoped).
    fn npm_publish(
        &self,
        package: &PackageId,
        tag: Option<&str>,
        access: Option<&NpmAccess>,
    ) -> Result<PublishOutcome, RegistryError> {
        if package.name().starts_with('-') {
            return Err(RegistryError::Other(format!(
                "invalid package name `{}`: names may not begin with '-' (possible flag injection)",
                package.name()
            )));
        }
        let access_value = access.map(|a| match a {
            NpmAccess::Public => "public",
            NpmAccess::Restricted => "restricted",
        });
        let mut args = vec!["publish", "--workspace", package.name()];
        if let Some(t) = tag {
            args.push("--tag");
            args.push(t);
        }
        if let Some(av) = access_value {
            args.push("--access");
            args.push(av);
        }
        let output = self.run("npm", &args)?;
        classify_npm_publish_output(&output)
    }

    // ---- PyPI -------------------------------------------------------------

    /// Uploads a Python distribution to PyPI (or a compatible index) by running
    /// `twine upload --skip-existing dist/<normalized-name>-<version>*`.
    ///
    /// The package name is normalized to its PEP 427 wheel-filename form
    /// (lowercased, hyphens and dots replaced with underscores) before constructing
    /// the dist-file glob. `twine` expands the glob internally — no shell
    /// expansion is involved, so the literal `*` in the argument is safe.
    ///
    /// When `index` is `Some`, `--repository-url <url>` is inserted before the
    /// glob argument so that a private package index (e.g. a Nexus or Artifactory
    /// PyPI proxy) is targeted instead of the default public PyPI.
    ///
    /// Output classification delegates to [`classify_twine_output`]:
    ///
    /// - `--skip-existing` causes twine to print `"Skipping … because it
    ///   appears to already exist"` for files already on the index; any such
    ///   mention yields [`PublishOutcome::AlreadyPublished`].
    /// - A clean zero-exit with no skip message yields [`PublishOutcome::Published`].
    /// - Rate-limit and auth-failure signals in the output map to the
    ///   corresponding [`RegistryError`] variants; everything else becomes
    ///   [`RegistryError::Other`].
    fn pypi_publish(
        &self,
        package: &PackageId,
        version: &Version,
        index: Option<&str>,
    ) -> Result<PublishOutcome, RegistryError> {
        if package.name().starts_with('-') {
            return Err(RegistryError::Other(format!(
                "invalid package name `{}`: names may not begin with '-' (possible flag injection)",
                package.name()
            )));
        }
        let normalized = package.name().to_lowercase().replace(['-', '.'], "_");
        let pattern = format!("dist/{normalized}-{}*", version.render());
        let output = if let Some(idx) = index {
            self.run(
                "twine",
                &[
                    "upload",
                    "--skip-existing",
                    "--repository-url",
                    idx,
                    &pattern,
                ],
            )?
        } else {
            self.run("twine", &["upload", "--skip-existing", &pattern])?
        };
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
                name,
                ..
            } => {
                let registry = self
                    .cargo_registry
                    .get(name.as_str())
                    .and_then(|r| r.as_deref());
                self.cargo_publish(package, registry)
            }
            PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name,
                ..
            } => {
                let meta = self.npm_meta.get(name.as_str());
                let tag = meta.and_then(|m| m.tag.as_deref());
                let access = meta.and_then(|m| m.access.as_ref());
                self.npm_publish(package, tag, access)
            }
            PackageId::Prefixed {
                ecosystem: Ecosystem::Pypi,
                name,
                ..
            } => {
                let index = self
                    .pypi_index
                    .get(name.as_str())
                    .and_then(|i| i.as_deref());
                self.pypi_publish(package, version, index)
            }
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

    // ---- registry metadata threading (RED → GREEN) ----------------------
    // These four tests exercise the private helper methods directly and assert
    // that per-package publish metadata is threaded through to the CLI args.

    #[test]
    fn npm_publish_passes_tag_when_set() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let runner = CapturingRunner {
            captured_args: std::sync::Arc::clone(&captured),
            response: output(0, "+ @callisto/cli@1.2.3\n", ""),
        };
        let c = SubprocessRegistryClient::new(runner, PathBuf::from("/workspace"));
        c.npm_publish(&npm_pkg(), Some("next"), None).unwrap();
        let args = captured.lock().unwrap();
        assert!(
            args.contains(&"--tag".to_string()),
            "expected --tag in args: {args:?}"
        );
        assert!(
            args.contains(&"next".to_string()),
            "expected 'next' in args: {args:?}"
        );
    }

    #[test]
    fn npm_publish_omits_access_when_none() {
        // When no access level is specified the --access flag must not appear
        // at all — npm's ecosystem default (restricted for scoped packages,
        // public for unscoped) should apply rather than being overridden.
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let runner = CapturingRunner {
            captured_args: std::sync::Arc::clone(&captured),
            response: output(0, "+ @callisto/cli@1.2.3\n", ""),
        };
        let c = SubprocessRegistryClient::new(runner, PathBuf::from("/workspace"));
        c.npm_publish(&npm_pkg(), None, None).unwrap();
        let args = captured.lock().unwrap();
        assert!(
            !args.contains(&"--access".to_string()),
            "expected no --access flag when access is None: {args:?}"
        );
    }

    #[test]
    fn npm_publish_passes_access_public_when_set() {
        use callisto_model::NpmAccess;
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let runner = CapturingRunner {
            captured_args: std::sync::Arc::clone(&captured),
            response: output(0, "+ @callisto/cli@1.2.3\n", ""),
        };
        let c = SubprocessRegistryClient::new(runner, PathBuf::from("/workspace"));
        c.npm_publish(&npm_pkg(), None, Some(&NpmAccess::Public))
            .unwrap();
        let args = captured.lock().unwrap();
        assert!(
            args.contains(&"--access".to_string()),
            "expected --access in args: {args:?}"
        );
        assert!(
            args.contains(&"public".to_string()),
            "expected 'public' in args: {args:?}"
        );
    }

    #[test]
    fn pypi_publish_passes_repository_url_when_index_set() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let runner = CapturingRunner {
            captured_args: std::sync::Arc::clone(&captured),
            response: output(0, "Uploading callisto_py-1.2.3-py3-none-any.whl\n", ""),
        };
        let c = SubprocessRegistryClient::new(runner, PathBuf::from("/workspace"));
        c.pypi_publish(
            &pypi_pkg(),
            &v1(),
            Some("https://private.example.com/simple/"),
        )
        .unwrap();
        let args = captured.lock().unwrap();
        assert!(
            args.contains(&"--repository-url".to_string()),
            "expected --repository-url in args: {args:?}"
        );
        assert!(
            args.contains(&"https://private.example.com/simple/".to_string()),
            "expected index URL in args: {args:?}"
        );
    }

    #[test]
    fn cargo_publish_passes_registry_when_set() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let runner = CapturingRunner {
            captured_args: std::sync::Arc::clone(&captured),
            response: output(0, "Uploading callisto-model v1.2.3\n", ""),
        };
        let c = SubprocessRegistryClient::new(runner, PathBuf::from("/workspace"));
        c.cargo_publish(&cargo_pkg(), Some("my-private-registry"))
            .unwrap();
        let args = captured.lock().unwrap();
        assert!(
            args.contains(&"--registry".to_string()),
            "expected --registry in args: {args:?}"
        );
        assert!(
            args.contains(&"my-private-registry".to_string()),
            "expected registry name in args: {args:?}"
        );
    }
}
