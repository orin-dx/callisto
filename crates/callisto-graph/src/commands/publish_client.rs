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

use callisto_manifests::WorkspaceCargoResolver;
use callisto_model::{
    ApplyPermit, CommandOutput, CommandRunner, Ecosystem, NpmAccess, PackageId, PublishOutcome,
    RegistryClient, RegistryError, Version,
};
use toml;

use super::publish::parse_retry_after;

/// Maximum wall-clock time (in seconds) allowed for a single publish command
/// (`cargo publish`, `npm publish`, `twine upload`). If the process does not
/// exit within this window it is killed and the attempt is recorded as a
/// timeout failure.
const PUBLISH_TIMEOUT_SECS: u64 = 300;

/// Per-package npm publish metadata stored in the metadata map.
struct NpmMeta {
    tag: Option<String>,
    access: Option<NpmAccess>,
    /// Private npm registry URL from `publishConfig.registry` in `package.json`.
    /// When `Some`, `--registry <url>` is appended to `npm publish` and
    /// `npm view`. When `None`, the public npm registry is used.
    registry: Option<String>,
}

/// Dispatches [`RegistryClient`] calls to the right ecosystem CLI based on
/// each [`PackageId`]'s ecosystem tag.
///
/// Before calling through the [`RegistryClient`] trait, invoke
/// [`Self::load_plan`] so that per-package metadata (dist-tag, access level,
/// registry URL, private index URL, package directory) is threaded through to
/// the CLI args and working directory.
pub struct SubprocessRegistryClient<R: CommandRunner> {
    runner: R,
    /// Workspace root. Used as the base for resolving per-package directories
    /// and as the working directory for package managers that resolve packages
    /// by name from the workspace root (`cargo`, `pnpm`, `yarn`, `npm`).
    cwd: PathBuf,
    /// npm publish metadata keyed by package name.
    npm_meta: HashMap<String, NpmMeta>,
    /// Cargo registry name (e.g. `"my-private-registry"`) keyed by crate name.
    cargo_registry: HashMap<String, Option<String>>,
    /// Per-package directory (relative to workspace root) for Cargo crates.
    /// Used to read the on-disk `Cargo.toml` for a pre-publish version check.
    cargo_pkg_dir: HashMap<String, std::path::PathBuf>,
    /// Planned publish version per crate name, populated from the plan.
    cargo_planned_version: HashMap<String, Version>,
    /// PyPI index URL keyed by distribution name.
    pypi_index: HashMap<String, Option<String>>,
    /// Per-package directory (relative to workspace root) for npm packages,
    /// keyed by package name. Used for package managers that must be invoked
    /// from the package directory (bun).
    npm_pkg_dir: HashMap<String, PathBuf>,
    /// Per-package directory (relative to workspace root) for PyPI packages,
    /// keyed by distribution name. `twine upload` runs from this directory so
    /// that `dist/` resolves to the package's own dist directory in a monorepo.
    pypi_pkg_dir: HashMap<String, PathBuf>,
}

impl<R: CommandRunner> SubprocessRegistryClient<R> {
    pub fn new(runner: R, cwd: PathBuf) -> Self {
        Self {
            runner,
            cwd,
            npm_meta: HashMap::new(),
            cargo_registry: HashMap::new(),
            cargo_pkg_dir: HashMap::new(),
            cargo_planned_version: HashMap::new(),
            pypi_index: HashMap::new(),
            npm_pkg_dir: HashMap::new(),
            pypi_pkg_dir: HashMap::new(),
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
                    registry: pkg.registry.clone(),
                },
            );
            self.npm_pkg_dir
                .insert(pkg.name.clone(), pkg.package_dir.clone());
        }
        for pkg in &plan.npm_main_packages {
            self.npm_meta.insert(
                pkg.name.clone(),
                NpmMeta {
                    tag: pkg.tag.clone(),
                    access: pkg.access.clone(),
                    registry: pkg.registry.clone(),
                },
            );
            self.npm_pkg_dir
                .insert(pkg.name.clone(), pkg.package_dir.clone());
        }
        for pkg in &plan.rust_crates {
            self.cargo_registry
                .insert(pkg.name.clone(), pkg.registry.clone());
            self.cargo_planned_version
                .insert(pkg.name.clone(), pkg.version.clone());
            if let Some(dir) = &pkg.package_dir {
                self.cargo_pkg_dir.insert(pkg.name.clone(), dir.clone());
            }
        }
        for pkg in &plan.pypi_packages {
            self.pypi_index.insert(pkg.name.clone(), pkg.index.clone());
            self.pypi_pkg_dir
                .insert(pkg.name.clone(), pkg.package_dir.clone());
        }
    }

    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, RegistryError> {
        self.run_in(program, args, &self.cwd)
    }

    fn run_in(
        &self,
        program: &str,
        args: &[&str],
        cwd: &std::path::Path,
    ) -> Result<CommandOutput, RegistryError> {
        self.runner
            .run_with_timeout(
                program,
                args,
                cwd,
                Duration::from_secs(PUBLISH_TIMEOUT_SECS),
            )
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

        // Pre-publish version sanity check: if load_plan provided a package
        // directory, read the on-disk Cargo.toml and verify its version matches
        // the planned version before invoking `cargo publish`. This catches the
        // common mistake of running `callisto publish` without first running
        // `callisto version`, which would silently publish the old version.
        if let (Some(pkg_dir), Some(planned)) = (
            self.cargo_pkg_dir.get(package.name()),
            self.cargo_planned_version.get(package.name()),
        ) {
            let manifest_path = self.cwd.join(pkg_dir).join("Cargo.toml");
            // Parse only the [package].version field — no need for full CST.
            // Use a permissive `toml::Value` intermediate rather than
            // deserializing straight into `String`: `version.workspace = true`
            // (Cargo's workspace-version-inheritance syntax) parses `version`
            // as a table, not a string, and a direct `String` deserialization
            // would fail with "invalid type: map, expected a string" on every
            // crate using that pattern.
            let contents = std::fs::read_to_string(&manifest_path).map_err(|e| {
                RegistryError::Other(format!(
                    "could not read `{}` for pre-publish version check: {e}. \
                         Ensure the package_dir in the publish plan points to the \
                         individual crate directory, not the workspace root.",
                    manifest_path.display(),
                ))
            })?;
            let parsed = toml::from_str::<toml::Value>(&contents).map_err(|e| {
                RegistryError::Other(format!(
                    "could not parse `{}` for pre-publish version check: {e}. \
                     The file must contain a [package].version field. \
                     If this is a workspace Cargo.toml, set package_dir to the \
                     individual crate subdirectory instead.",
                    manifest_path.display(),
                ))
            })?;
            let version_value = parsed
                .get("package")
                .and_then(|p| p.get("version"))
                .ok_or_else(|| {
                    RegistryError::Other(format!(
                        "could not find [package].version in `{}` for pre-publish version check. \
                     The file must contain a [package].version field. \
                     If this is a workspace Cargo.toml, set package_dir to the \
                     individual crate subdirectory instead.",
                        manifest_path.display(),
                    ))
                })?;
            let on_disk = if let Some(s) = version_value.as_str() {
                s.to_string()
            } else if version_value.get("workspace").and_then(|w| w.as_bool()) == Some(true) {
                let root_manifest_path = self.cwd.join("Cargo.toml");
                let resolver = WorkspaceCargoResolver::load(&root_manifest_path).map_err(|e| {
                    RegistryError::Other(format!(
                        "`{}` has version.workspace = true but the workspace root `{}` \
                             could not be loaded to resolve it: {e}",
                        manifest_path.display(),
                        root_manifest_path.display(),
                    ))
                })?;
                let ws_version = resolver
                    .workspace_version()
                    .map_err(|e| {
                        RegistryError::Other(format!(
                            "could not resolve [workspace.package].version from `{}` \
                             for `{}`: {e}",
                            root_manifest_path.display(),
                            manifest_path.display(),
                        ))
                    })?
                    .ok_or_else(|| {
                        RegistryError::Other(format!(
                            "`{}` has version.workspace = true but the workspace root `{}` \
                             has no [workspace.package].version",
                            manifest_path.display(),
                            root_manifest_path.display(),
                        ))
                    })?;
                ws_version.render().to_string()
            } else {
                return Err(RegistryError::Other(format!(
                    "could not parse [package].version in `{}` for pre-publish version \
                     check: expected a string or a `{{ workspace = true }}` table.",
                    manifest_path.display(),
                )));
            };
            let expected = planned.render();
            if on_disk != expected {
                return Err(RegistryError::Other(format!(
                    "version mismatch for `{}`: Cargo.toml on disk has version `{}` \
                     but the publish plan expects `{}`. \
                     Run `callisto version` first to write the new version to disk.",
                    package.name(),
                    on_disk,
                    expected,
                )));
            }
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
        registry: Option<&str>,
    ) -> Result<bool, RegistryError> {
        let spec = format!("{}@{}", package.name(), version.render());
        let output = if let Some(reg) = registry {
            self.run("npm", &["view", &spec, "--json", "--registry", reg])?
        } else {
            self.run("npm", &["view", &spec, "--json"])?
        };

        if output.success() && !output.stdout_trimmed().is_empty() {
            return Ok(true);
        }
        if output.success() && output.stdout_trimmed().is_empty() {
            return Ok(false);
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

    /// Detects the npm-compatible package manager from lockfiles in the
    /// workspace root (`self.cwd`) and returns the appropriate program name
    /// and base argument list for a publish command.
    ///
    /// Detection priority: pnpm > yarn > bun > npm (default).
    ///
    /// | Lockfile present   | Program | Base args                                    |
    /// |--------------------|---------|----------------------------------------------|
    /// | `pnpm-lock.yaml`   | `pnpm`  | `publish --filter <name>`                    |
    /// | `yarn.lock`        | `yarn`  | `workspace <name> npm publish`               |
    /// | `bun.lockb`/`.lock`| `bun`   | `publish`                                    |
    /// | (none)             | `npm`   | `publish --workspace <name>`                 |
    ///
    /// `tag` and `access` flags are appended after the base args by the caller
    /// ([`Self::npm_publish`]).
    fn build_npm_publish_command(
        &self,
        package_name: &str,
        tag: Option<&str>,
        access: Option<&str>,
        registry: Option<&str>,
    ) -> (String, Vec<String>) {
        let mut extra: Vec<String> = Vec::new();
        if let Some(t) = tag {
            extra.push("--tag".to_string());
            extra.push(t.to_string());
        }
        if let Some(av) = access {
            extra.push("--access".to_string());
            extra.push(av.to_string());
        }
        if let Some(reg) = registry {
            extra.push("--registry".to_string());
            extra.push(reg.to_string());
        }

        if self.cwd.join("pnpm-lock.yaml").exists() {
            let mut args = vec![
                "publish".to_string(),
                "--filter".to_string(),
                package_name.to_string(),
                // pnpm ≥ 7 refuses to publish from a dirty working tree by
                // default. After `callisto version` stages manifest bumps,
                // the tree is always dirty until the operator commits, so
                // we must bypass the git-status check here.
                "--no-git-checks".to_string(),
            ];
            args.extend(extra);
            return ("pnpm".to_string(), args);
        }

        if self.cwd.join("yarn.lock").exists() {
            let mut args = vec![
                "workspace".to_string(),
                package_name.to_string(),
                "npm".to_string(),
                "publish".to_string(),
            ];
            args.extend(extra);
            return ("yarn".to_string(), args);
        }

        // bun.lockb is the legacy binary lockfile format; bun.lock (text) is
        // written by Bun >= 1.1.1. Both signal a bun-managed workspace.
        if self.cwd.join("bun.lockb").exists() || self.cwd.join("bun.lock").exists() {
            let mut args = vec!["publish".to_string()];
            args.extend(extra);
            return ("bun".to_string(), args);
        }

        // Default: npm
        let mut args = vec![
            "publish".to_string(),
            "--workspace".to_string(),
            package_name.to_string(),
        ];
        args.extend(extra);
        ("npm".to_string(), args)
    }

    /// Publishes an npm package using the workspace package manager detected
    /// from lockfiles in the workspace root (`self.cwd`).
    ///
    /// - `tag`: when `Some`, appends `--tag <tag>` (e.g. `"next"` for pre-releases).
    ///   Without this, the package manager uses the implicit `"latest"` dist-tag.
    /// - `access`: when `Some(NpmAccess::Public)`, appends `--access public`;
    ///   when `Some(NpmAccess::Restricted)`, appends `--access restricted`;
    ///   when `None`, omits `--access` entirely so the package manager applies
    ///   its ecosystem default (`restricted` for `@scoped/packages`, `public`
    ///   for unscoped).
    fn npm_publish(
        &self,
        package: &PackageId,
        tag: Option<&str>,
        access: Option<&NpmAccess>,
        registry: Option<&str>,
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
        let (program, args_owned) =
            self.build_npm_publish_command(package.name(), tag, access_value, registry);
        let args_refs: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();

        // bun publish must run from the package directory — it has no
        // --filter or --workspace flag to target a named package. All other
        // package managers (pnpm --filter, yarn workspace, npm --workspace)
        // run from the workspace root and select the package by name.
        let effective_cwd = if program == "bun" {
            self.npm_pkg_dir
                .get(package.name())
                .map(|rel| self.cwd.join(rel))
                .unwrap_or_else(|| self.cwd.clone())
        } else {
            self.cwd.clone()
        };

        let output = self.run_in(&program, &args_refs, &effective_cwd)?;
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
        // PEP 427 wheel filename normalization: lowercase, then collapse any run
        // of hyphens, dots, or underscores into a single underscore. Using a
        // simple split-and-join avoids pulling in the `regex` crate.
        let normalized = package
            .name()
            .to_lowercase()
            .split(['-', '.', '_'])
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("_");
        let pattern = format!("dist/{normalized}-{}*", version.render());

        // Both build and upload run from the package directory so that `dist/`
        // resolves to the package's own dist/ folder. When no package_dir was
        // stored by load_plan (e.g. tests that call this method directly),
        // fall back to the workspace root.
        let pkg_cwd = self
            .pypi_pkg_dir
            .get(package.name())
            .map(|rel| self.cwd.join(rel))
            .unwrap_or_else(|| self.cwd.clone());

        // Build step (spec §8.3): produce the sdist and wheel before uploading.
        // Build failures are not transient registry errors — do NOT retry them.
        let build_out = self.run_in(
            "python",
            &["-m", "build", "--sdist", "--wheel", "--outdir", "dist/"],
            &pkg_cwd,
        )?;
        if !build_out.success() {
            return Err(RegistryError::Other(format!(
                "python -m build failed for `{}` (exit {:?}): {}",
                package.name(),
                build_out.exit_code,
                build_out.stderr.trim()
            )));
        }

        let output = if let Some(idx) = index {
            self.run_in(
                "twine",
                &[
                    "upload",
                    "--skip-existing",
                    "--repository-url",
                    idx,
                    &pattern,
                ],
                &pkg_cwd,
            )?
        } else {
            self.run_in("twine", &["upload", "--skip-existing", &pattern], &pkg_cwd)?
        };
        classify_twine_output(&output)
    }
}

impl<R: CommandRunner> RegistryClient for SubprocessRegistryClient<R> {
    fn is_published(&self, package: &PackageId, version: &Version) -> Result<bool, RegistryError> {
        match package {
            PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name,
                ..
            } => {
                let registry = self
                    .npm_meta
                    .get(name.as_str())
                    .and_then(|m| m.registry.as_deref());
                self.npm_is_published(package, version, registry)
            }

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
                let registry = meta.and_then(|m| m.registry.as_deref());
                self.npm_publish(package, tag, access, registry)
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
    // Match rate-limit signals with context, never on bare digits alone.
    // Bare "429" in cargo's compressed-size output ("429.1KiB compressed")
    // would otherwise trigger this — causing an infinite retry on unrelated
    // failures. Require at least one contextual phrase alongside any "429".
    let is_rate_limited = text_lower.contains("too many requests")
        || text_lower.contains("rate limit")
        || text_lower.contains("erate_limit")
        || (text_lower.contains("429")
            && (text_lower.contains("too many")
                || text_lower.contains("rate")
                || text_lower.contains("retry")));
    if is_rate_limited {
        let dur = extract_retry_after_duration(text_lower).unwrap_or(Duration::from_secs(
            crate::commands::publish::DEFAULT_RATE_LIMIT_WAIT_SECS,
        ));
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
        || combined.contains("e409")
        || combined.contains("409 conflict")
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
/// 1. `"already exist"` substring (from `--skip-existing`) without any
///    `"uploading"` line → [`PublishOutcome::AlreadyPublished`] (all artifacts
///    were skipped; none were freshly uploaded).
///    When both phrases appear, at least one artifact was newly uploaded
///    alongside the skipped stale artifact, so the result is `Published`.
///    This handles the common case where `dist/` accumulates stale pre-release
///    artifacts: glob `dist/my_pkg-1.0.0*` matches both `my_pkg-1.0.0a0.whl`
///    (skipped) and `my_pkg-1.0.0.whl` (uploaded), producing mixed output.
/// 2. Zero exit code with no skip-only mention → [`PublishOutcome::Published`].
/// 3. Rate-limit signal (`429`, `too many requests`, `rate limit`) →
///    [`RegistryError::RateLimited`] with a parsed or default 60-second
///    retry-after duration.
/// 4. Auth-failure signal (`401`, `403`, `authentication`, etc.) →
///    [`RegistryError::AuthFailed`].
/// 5. Anything else → [`RegistryError::Other`].
fn classify_twine_output(output: &CommandOutput) -> Result<PublishOutcome, RegistryError> {
    let combined = combined_lower(output);

    let has_skip = combined.contains("already exist");
    let has_upload = combined.contains("uploading");
    if has_skip && !has_upload {
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

    /// Returns the configured output for ecosystem CLI calls (`cargo`, `npm`,
    /// `twine`) but always returns exit(0) for `python` (the build step added
    /// in front of twine). Tests that need to exercise build-step failures use
    /// [`OrderedCallCapture`] directly.
    struct ScriptedRunner(CommandOutput);

    impl CommandRunner for ScriptedRunner {
        fn run(
            &self,
            program: &str,
            _args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<CommandOutput, CommandError> {
            if program == "python" {
                return Ok(output(0, "", ""));
            }
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
        assert_eq!(
            err,
            RegistryError::RateLimited(Duration::from_secs(
                crate::commands::publish::DEFAULT_RATE_LIMIT_WAIT_SECS
            ))
        );
    }

    /// Regression guard: cargo's packaging summary ("Packaged X files, 1.4MiB
    /// (429.1KiB compressed)") contains the digits "429" in a non-rate-limit
    /// context. A genuine auth or manifest failure whose stderr happens to also
    /// carry cargo's packaging summary must NOT be classified as rate-limited —
    /// it must be `RegistryError::Other`, not an infinite retry trigger.
    #[test]
    fn cargo_publish_compressed_size_containing_429_is_not_rate_limited() {
        let c = client(output(
            101,
            "Packaged 42 files, 1.4MiB (429.1KiB compressed)\n",
            "error: failed to publish to the registry\n",
        ));
        let err = c.publish(&cargo_pkg(), &v1(), &permit()).unwrap_err();
        assert!(
            matches!(err, RegistryError::Other(_)),
            "compressed-size '429' in output must NOT be treated as rate-limited; got: {err:?}"
        );
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
    fn npm_publish_private_registry_409_is_already_published() {
        // Verdaccio, Nexus, and Artifactory return E409 / 409 Conflict instead
        // of EPUBLISHCONFLICT when a version already exists on a private registry.
        let c = client(output(1, "", "npm ERR! code E409\nnpm ERR! 409 Conflict\n"));
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
    fn npm_is_published_exit_zero_empty_stdout_returns_false() {
        // exit 0 with empty stdout signals "not found" on some private registry
        // configurations (e.g. 204 / no-content responses from Verdaccio). Must
        // return Ok(false), not fall through to the ambiguous Err path.
        let c = client(output(0, "", ""));
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

    /// When twine's output includes both a "skip" line for a stale pre-release
    /// artifact AND an "Uploading" line for the target stable version, the
    /// result must be `Published`, not `AlreadyPublished`. A stale pre-release
    /// in `dist/` triggers the "already exist" phrase via `--skip-existing`, but
    /// the stable artifact was still successfully uploaded in the same run.
    #[test]
    fn pypi_publish_mixed_skip_and_upload_is_published() {
        // Twine output when dist/ contains both stale pre-release (skipped) and
        // new stable artifact (uploaded). The "already exist" phrase appears for
        // the skipped pre-release; "Uploading" appears for the stable wheel.
        let c = client(output(
            0,
            "Skipping callisto_py-1.2.3a0-py3-none-any.whl because it appears to already exist\n\
             Uploading callisto_py-1.2.3-py3-none-any.whl\n\
             100%|████████| 43.2k/43.2k\n",
            "",
        ));
        assert_eq!(
            c.publish(&pypi_pkg(), &v1(), &permit()).unwrap(),
            PublishOutcome::Published,
            "when twine both skips a stale artifact and uploads a new one, the \
             result must be Published, not AlreadyPublished"
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

    // ---- python build step tests ----------------------------------------

    type OrderedCallLog = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<String>)>>>;

    /// Records every (program, args) call in order. Used to verify call
    /// sequencing: python -m build must appear before twine upload.
    struct OrderedCallCapture {
        #[allow(clippy::type_complexity)]
        calls: OrderedCallLog,
        response: CommandOutput,
    }

    impl CommandRunner for OrderedCallCapture {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<CommandOutput, CommandError> {
            self.calls.lock().unwrap().push((
                program.to_string(),
                args.iter().map(|s| s.to_string()).collect(),
            ));
            Ok(self.response.clone())
        }
    }

    /// Spec §8.3: python -m build MUST be called before twine upload.
    /// On a clean checkout there is no dist/ directory; skipping the build
    /// step causes twine to fail with "Cannot find file (or expand pattern)".
    #[test]
    fn pypi_publish_runs_python_build_before_twine() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, Vec<String>)>::new()));
        let runner = OrderedCallCapture {
            calls: std::sync::Arc::clone(&calls),
            response: output(0, "Uploading my_lib-1.2.3-py3-none-any.whl\n", ""),
        };
        let c = SubprocessRegistryClient::new(runner, PathBuf::from("/workspace"));
        c.publish(&pypi_pkg(), &v1(), &permit()).unwrap();

        let recorded = calls.lock().unwrap().clone();
        // First call must be python -m build
        assert!(
            recorded.first().is_some_and(|(prog, args)| {
                prog == "python" && args.iter().any(|a| a == "build")
            }),
            "first call must be `python -m build`; calls: {recorded:?}"
        );
        // Second call must be twine
        assert!(
            recorded.get(1).is_some_and(|(prog, _)| prog == "twine"),
            "second call must be `twine`; calls: {recorded:?}"
        );
    }

    /// Spec §8.3: a build failure must NOT invoke twine and must return an
    /// error (not a rate-limit error — build failures are not transient).
    #[test]
    fn pypi_publish_build_failure_does_not_call_twine() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, Vec<String>)>::new()));
        let runner = OrderedCallCapture {
            calls: std::sync::Arc::clone(&calls),
            response: output(1, "", "error: No module named 'build'\n"), // build exits non-zero
        };
        let c = SubprocessRegistryClient::new(runner, PathBuf::from("/workspace"));
        let err = c.publish(&pypi_pkg(), &v1(), &permit()).unwrap_err();

        let recorded = calls.lock().unwrap().clone();
        assert!(
            !recorded.iter().any(|(prog, _)| prog == "twine"),
            "twine must NOT be called after a build failure; calls: {recorded:?}"
        );
        assert!(
            matches!(err, RegistryError::Other(_)),
            "build failure must be RegistryError::Other (non-retriable); got: {err:?}"
        );
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

    /// PEP 427 also requires dots to be replaced with underscores, matching
    /// the same normalization applied to hyphens. A package named `my-pkg.lib`
    /// must produce `dist/my_pkg_lib-1.2.3*`, not `dist/my-pkg.lib-1.2.3*`.
    #[test]
    fn pypi_glob_pattern_normalizes_dots_to_underscore() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let runner = CapturingRunner {
            captured_args: std::sync::Arc::clone(&captured),
            response: output(0, "Uploading my_pkg_lib-1.2.3-py3-none-any.whl\n", ""),
        };
        let pkg = PackageId::Prefixed {
            ecosystem: Ecosystem::Pypi,
            name: "my-pkg.lib".to_string(),
        };
        let version = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
        let c = SubprocessRegistryClient::new(runner, PathBuf::from("/workspace"));
        c.publish(&pkg, &version, &permit()).unwrap();

        let args = captured.lock().unwrap();
        assert!(
            args.iter().any(|a| a == "dist/my_pkg_lib-1.2.3*"),
            "expected glob 'dist/my_pkg_lib-1.2.3*' (PEP 427 dot+hyphen → underscore) but got: {:?}",
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
        c.npm_publish(&npm_pkg(), Some("next"), None, None).unwrap();
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
        c.npm_publish(&npm_pkg(), None, None, None).unwrap();
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
        c.npm_publish(&npm_pkg(), None, Some(&NpmAccess::Public), None)
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
    fn npm_publish_passes_registry_when_set() {
        // AUTH-005 / F-04: when a private npm registry URL is supplied,
        // `--registry <url>` must appear in the npm publish args.
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let runner = CapturingRunner {
            captured_args: std::sync::Arc::clone(&captured),
            response: output(0, "+ @callisto/cli@1.2.3\n", ""),
        };
        let c = SubprocessRegistryClient::new(runner, PathBuf::from("/workspace"));
        c.npm_publish(
            &npm_pkg(),
            None,
            None,
            Some("https://npm.my-org.example.com"),
        )
        .unwrap();
        let args = captured.lock().unwrap();
        assert!(
            args.contains(&"--registry".to_string()),
            "expected --registry in npm publish args: {args:?}"
        );
        assert!(
            args.contains(&"https://npm.my-org.example.com".to_string()),
            "expected registry URL in npm publish args: {args:?}"
        );
    }

    #[test]
    fn npm_publish_no_registry_flag_when_registry_none() {
        // When no registry is set, --registry must NOT appear — npm uses its
        // own default (the public registry).
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let runner = CapturingRunner {
            captured_args: std::sync::Arc::clone(&captured),
            response: output(0, "+ @callisto/cli@1.2.3\n", ""),
        };
        let c = SubprocessRegistryClient::new(runner, PathBuf::from("/workspace"));
        c.npm_publish(&npm_pkg(), None, None, None).unwrap();
        let args = captured.lock().unwrap();
        assert!(
            !args.contains(&"--registry".to_string()),
            "expected no --registry flag when registry is None: {args:?}"
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
            args.contains(&"-p".to_string()),
            "expected -p in cargo publish args: {args:?}"
        );
        assert!(
            args.contains(&"--locked".to_string()),
            "expected --locked in cargo publish args: {args:?}"
        );
        assert!(
            args.contains(&"--registry".to_string()),
            "expected --registry in args: {args:?}"
        );
        assert!(
            args.contains(&"my-private-registry".to_string()),
            "expected registry name in args: {args:?}"
        );
    }

    #[test]
    fn cargo_publish_always_includes_p_and_locked_without_registry() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let runner = CapturingRunner {
            captured_args: std::sync::Arc::clone(&captured),
            response: output(0, "Uploading callisto-model v1.2.3\n", ""),
        };
        let c = SubprocessRegistryClient::new(runner, PathBuf::from("/workspace"));
        c.cargo_publish(&cargo_pkg(), None).unwrap();
        let args = captured.lock().unwrap();
        assert!(
            args.contains(&"-p".to_string()),
            "expected -p in cargo publish args (no registry): {args:?}"
        );
        assert!(
            args.contains(&"--locked".to_string()),
            "expected --locked in cargo publish args (no registry): {args:?}"
        );
        assert!(
            !args.contains(&"--registry".to_string()),
            "--registry must NOT be present when no registry specified: {args:?}"
        );
    }

    // ---- package manager detection (lockfile-based) ----------------------

    /// Runner that captures both the program name and all args.
    struct ProgramCapturingRunner {
        captured_program: std::sync::Arc<std::sync::Mutex<String>>,
        captured_args: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        response: CommandOutput,
    }

    impl CommandRunner for ProgramCapturingRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<CommandOutput, CommandError> {
            *self.captured_program.lock().unwrap() = program.to_string();
            *self.captured_args.lock().unwrap() = args.iter().map(|s| s.to_string()).collect();
            Ok(self.response.clone())
        }
    }

    #[allow(clippy::type_complexity)]
    fn program_capturing_runner(
        out: CommandOutput,
    ) -> (
        ProgramCapturingRunner,
        std::sync::Arc<std::sync::Mutex<String>>,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        let captured_program = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let captured_args = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let runner = ProgramCapturingRunner {
            captured_program: std::sync::Arc::clone(&captured_program),
            captured_args: std::sync::Arc::clone(&captured_args),
            response: out,
        };
        (runner, captured_program, captured_args)
    }

    #[test]
    fn npm_publish_uses_pnpm_when_pnpm_lockfile_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();

        let (runner, captured_program, captured_args) = program_capturing_runner(output(0, "", ""));
        let c = SubprocessRegistryClient::new(runner, dir.path().to_path_buf());
        c.npm_publish(&npm_pkg(), None, None, None).unwrap();

        assert_eq!(
            *captured_program.lock().unwrap(),
            "pnpm",
            "expected pnpm when pnpm-lock.yaml is present"
        );
        let args = captured_args.lock().unwrap();
        assert!(
            args.contains(&"publish".to_string()),
            "expected 'publish' in args: {args:?}"
        );
        assert!(
            args.contains(&"--filter".to_string()),
            "expected '--filter' in args: {args:?}"
        );
        assert!(
            args.contains(&"@callisto/cli".to_string()),
            "expected package name after --filter in args: {args:?}"
        );
    }

    /// pnpm ≥ 7 refuses to publish from a dirty working tree by default
    /// (`ERR_PNPM_GIT_UNCLEAN`). After `callisto version` bumps and stages
    /// manifests, the tree is always dirty until the operator commits —
    /// `--no-git-checks` must always be passed to pnpm publish so that the
    /// publish step is not blocked by staged-but-uncommitted version bumps.
    #[test]
    fn pnpm_publish_includes_no_git_checks_flag() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();

        let (runner, _, captured_args) = program_capturing_runner(output(0, "", ""));
        let c = SubprocessRegistryClient::new(runner, dir.path().to_path_buf());
        c.npm_publish(&npm_pkg(), None, None, None).unwrap();

        let args = captured_args.lock().unwrap();
        assert!(
            args.contains(&"--no-git-checks".to_string()),
            "pnpm publish must include --no-git-checks to allow publishing from a \
             dirty working tree (staged version bumps); got args: {args:?}"
        );
    }

    #[test]
    fn npm_publish_uses_yarn_when_yarn_lock_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("yarn.lock"), "").unwrap();

        let (runner, captured_program, captured_args) = program_capturing_runner(output(0, "", ""));
        let c = SubprocessRegistryClient::new(runner, dir.path().to_path_buf());
        c.npm_publish(&npm_pkg(), None, None, None).unwrap();

        assert_eq!(
            *captured_program.lock().unwrap(),
            "yarn",
            "expected yarn when yarn.lock is present"
        );
        let args = captured_args.lock().unwrap();
        assert!(
            args.contains(&"workspace".to_string()),
            "expected 'workspace' in args: {args:?}"
        );
        assert!(
            args.contains(&"@callisto/cli".to_string()),
            "expected package name in args: {args:?}"
        );
        assert!(
            args.contains(&"npm".to_string()),
            "expected 'npm' subcommand in args: {args:?}"
        );
        assert!(
            args.contains(&"publish".to_string()),
            "expected 'publish' in args: {args:?}"
        );
    }

    #[test]
    fn npm_publish_uses_bun_when_bun_lockb_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bun.lockb"), "").unwrap();

        let (runner, captured_program, captured_args) = program_capturing_runner(output(0, "", ""));
        let c = SubprocessRegistryClient::new(runner, dir.path().to_path_buf());
        c.npm_publish(&npm_pkg(), None, None, None).unwrap();

        assert_eq!(
            *captured_program.lock().unwrap(),
            "bun",
            "expected bun when bun.lockb is present"
        );
        let args = captured_args.lock().unwrap();
        assert!(
            args.contains(&"publish".to_string()),
            "expected 'publish' in args: {args:?}"
        );
    }

    #[test]
    fn npm_publish_uses_bun_when_bun_lock_text_present() {
        // Bun >= 1.1.1 writes "bun.lock" (text TOML-like format) instead of
        // "bun.lockb" (binary). Both must resolve to the bun CLI.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bun.lock"), "").unwrap();

        let (runner, captured_program, captured_args) = program_capturing_runner(output(0, "", ""));
        let c = SubprocessRegistryClient::new(runner, dir.path().to_path_buf());
        c.npm_publish(&npm_pkg(), None, None, None).unwrap();

        assert_eq!(
            *captured_program.lock().unwrap(),
            "bun",
            "expected bun when bun.lock (text format) is present"
        );
        let args = captured_args.lock().unwrap();
        assert!(
            args.contains(&"publish".to_string()),
            "expected 'publish' in args: {args:?}"
        );
    }

    #[test]
    fn npm_publish_uses_npm_when_no_lockfile_present() {
        let dir = tempfile::tempdir().unwrap();
        // No lockfile written — default fallback to npm.

        let (runner, captured_program, captured_args) = program_capturing_runner(output(0, "", ""));
        let c = SubprocessRegistryClient::new(runner, dir.path().to_path_buf());
        c.npm_publish(&npm_pkg(), None, None, None).unwrap();

        assert_eq!(
            *captured_program.lock().unwrap(),
            "npm",
            "expected npm as default when no lockfile is present"
        );
        let args = captured_args.lock().unwrap();
        assert!(
            args.contains(&"publish".to_string()),
            "expected 'publish' in args: {args:?}"
        );
        assert!(
            args.contains(&"--workspace".to_string()),
            "expected '--workspace' in args: {args:?}"
        );
        assert!(
            args.contains(&"@callisto/cli".to_string()),
            "expected package name after --workspace in args: {args:?}"
        );
    }

    // ---- per-package directory routing (RED → GREEN) --------------------

    /// Captures the working directory that CommandRunner::run is called with.
    struct CwdCapturingRunner {
        captured_cwd: std::sync::Arc<std::sync::Mutex<Option<PathBuf>>>,
        response: CommandOutput,
    }

    impl CommandRunner for CwdCapturingRunner {
        fn run(
            &self,
            _program: &str,
            _args: &[&str],
            cwd: &std::path::Path,
        ) -> Result<CommandOutput, CommandError> {
            *self.captured_cwd.lock().unwrap() = Some(cwd.to_path_buf());
            Ok(self.response.clone())
        }
    }

    #[test]
    fn pypi_publish_uses_package_dir_as_cwd() {
        // load_plan with a PypiPublish carrying package_dir must cause twine to
        // run from <workspace>/<package_dir>, not the workspace root.
        use callisto_model::{PypiPublish, RegistryKey, SCHEMA_VERSION};

        let captured_cwd = std::sync::Arc::new(std::sync::Mutex::new(None::<PathBuf>));
        let runner = CwdCapturingRunner {
            captured_cwd: std::sync::Arc::clone(&captured_cwd),
            response: output(0, "Uploading callisto_py-1.2.3-py3-none-any.whl\n", ""),
        };
        let mut c = SubprocessRegistryClient::new(runner, PathBuf::from("/workspace"));

        let plan = callisto_model::PublishPlan {
            schema_version: SCHEMA_VERSION,
            rust_crates: vec![],
            npm_platform_packages: vec![],
            npm_main_packages: vec![],
            pypi_packages: vec![PypiPublish {
                name: "callisto-py".to_string(),
                version: v1(),
                publish_to: RegistryKey(RegistryKey::PYPI.to_string()),
                index: None,
                package_dir: PathBuf::from("packages/callisto-py"),
            }],
            releases: vec![],
            diagnostics: vec![],
        };
        c.load_plan(&plan);
        c.publish(&pypi_pkg(), &v1(), &permit()).unwrap();

        let cwd = captured_cwd
            .lock()
            .unwrap()
            .clone()
            .expect("CommandRunner::run was not called");
        assert_eq!(
            cwd,
            PathBuf::from("/workspace/packages/callisto-py"),
            "twine must run from the package directory, not the workspace root"
        );
    }

    #[test]
    fn bun_publish_uses_package_dir_as_cwd() {
        // bun publish is invoked from the workspace root by the current impl but
        // must be invoked from the per-package directory after the fix.
        use callisto_model::{NpmMainPublish, RegistryKey, SCHEMA_VERSION};

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bun.lockb"), "").unwrap();

        let captured_cwd = std::sync::Arc::new(std::sync::Mutex::new(None::<PathBuf>));
        let runner = CwdCapturingRunner {
            captured_cwd: std::sync::Arc::clone(&captured_cwd),
            response: output(0, "+ @callisto/cli@1.2.3\n", ""),
        };
        let mut c = SubprocessRegistryClient::new(runner, dir.path().to_path_buf());

        let plan = callisto_model::PublishPlan {
            schema_version: SCHEMA_VERSION,
            rust_crates: vec![],
            npm_platform_packages: vec![],
            npm_main_packages: vec![NpmMainPublish {
                name: "@callisto/cli".to_string(),
                version: v1(),
                publish_to: RegistryKey("npm".to_string()),
                registry: None,
                tag: None,
                access: None,
                depends_on_platforms: vec![],
                package_dir: PathBuf::from("packages/callisto-cli"),
            }],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };
        c.load_plan(&plan);
        c.publish(&npm_pkg(), &v1(), &permit()).unwrap();

        let cwd = captured_cwd
            .lock()
            .unwrap()
            .clone()
            .expect("CommandRunner::run was not called");
        assert_eq!(
            cwd,
            dir.path().join("packages/callisto-cli"),
            "bun publish must run from the package directory, not the workspace root"
        );
    }

    /// PUB-001 regression guard: a client that has NOT had load_plan called
    /// must fall back to the workspace root for the cwd, not the per-package
    /// directory. This test documents the pre-fix behavior and ensures that
    /// the fix (calling load_plan before execute) cannot be silently removed
    /// without a test failure.
    #[test]
    fn publish_without_load_plan_uses_workspace_root_as_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bun.lockb"), "").unwrap();

        let captured_cwd = std::sync::Arc::new(std::sync::Mutex::new(None::<PathBuf>));
        let runner = CwdCapturingRunner {
            captured_cwd: std::sync::Arc::clone(&captured_cwd),
            response: output(0, "+ @callisto/cli@1.2.3\n", ""),
        };
        // Deliberately NOT calling load_plan — simulates the PUB-001 bug.
        let c = SubprocessRegistryClient::new(runner, dir.path().to_path_buf());
        c.publish(&npm_pkg(), &v1(), &permit()).unwrap();

        let cwd = captured_cwd
            .lock()
            .unwrap()
            .clone()
            .expect("CommandRunner::run was not called");
        assert_eq!(
            cwd,
            dir.path().to_path_buf(),
            "without load_plan, publish must fall back to the workspace root"
        );
    }

    // -- NEW-version: pre-publish version mismatch guard ---------------------

    /// `cargo publish` publishes whatever version is on disk in Cargo.toml.
    /// If a user runs `callisto publish` without first running `callisto version`
    /// (or if Cargo.toml wasn't saved), the on-disk version is the OLD one,
    /// and the wrong version gets published silently.
    ///
    /// After `load_plan`, `SubprocessRegistryClient::cargo_publish` must read
    /// the on-disk Cargo.toml for the package and fail with a clear error if
    /// the version there doesn't match the planned version.
    #[test]
    fn cargo_publish_fails_when_on_disk_version_does_not_match_plan() {
        let dir = tempfile::tempdir().unwrap();

        // On disk: version 1.0.0
        let pkg_subdir = dir.path().join("crates/my-crate");
        std::fs::create_dir_all(&pkg_subdir).unwrap();
        std::fs::write(
            pkg_subdir.join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        // Plan: version 2.0.0 (not yet written to disk)
        let planned_version = Version::parse("2.0.0", VersionGrammar::SemVer).unwrap();
        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![callisto_model::CratePublish {
                name: "my-crate".to_string(),
                version: planned_version.clone(),
                publish_to: callisto_model::RegistryKey(
                    callisto_model::RegistryKey::CRATES_IO.to_string(),
                ),
                registry: None,
                package_dir: Some(std::path::PathBuf::from("crates/my-crate")),
            }],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };

        let pkg = PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "my-crate".to_string(),
        };

        let mut c = SubprocessRegistryClient::new(
            ScriptedRunner(output(0, "", "")),
            dir.path().to_path_buf(),
        );
        c.load_plan(&plan);

        let err = c.publish(&pkg, &planned_version, &permit()).unwrap_err();
        assert!(
            matches!(err, RegistryError::Other(ref msg) if msg.contains("version mismatch")),
            "expected version mismatch error, got: {err:?}"
        );
    }

    /// When the on-disk version matches the plan, `cargo_publish` must proceed
    /// normally and not raise a version mismatch error.
    #[test]
    fn cargo_publish_proceeds_when_on_disk_version_matches_plan() {
        let dir = tempfile::tempdir().unwrap();

        let pkg_subdir = dir.path().join("crates/my-crate");
        std::fs::create_dir_all(&pkg_subdir).unwrap();
        std::fs::write(
            pkg_subdir.join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"2.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        let planned_version = Version::parse("2.0.0", VersionGrammar::SemVer).unwrap();
        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![callisto_model::CratePublish {
                name: "my-crate".to_string(),
                version: planned_version.clone(),
                publish_to: callisto_model::RegistryKey(
                    callisto_model::RegistryKey::CRATES_IO.to_string(),
                ),
                registry: None,
                package_dir: Some(std::path::PathBuf::from("crates/my-crate")),
            }],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };

        let pkg = PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "my-crate".to_string(),
        };

        let mut c = SubprocessRegistryClient::new(
            ScriptedRunner(output(0, "Uploading my-crate v2.0.0\n", "")),
            dir.path().to_path_buf(),
        );
        c.load_plan(&plan);

        let outcome = c.publish(&pkg, &planned_version, &permit()).unwrap();
        assert_eq!(outcome, PublishOutcome::Published);
    }

    /// Version guard: when load_plan provides a package_dir but the on-disk
    /// Cargo.toml cannot be read (e.g. the path is wrong or the file is
    /// missing), publish must return Err rather than silently skipping the guard
    /// and potentially publishing the wrong version.
    #[test]
    fn cargo_version_guard_errors_when_manifest_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        // Note: we do NOT create <dir>/crates/my-crate/Cargo.toml — that is the
        // point. The guard must detect the missing file and refuse to proceed.

        let planned_version = Version::parse("2.0.0", VersionGrammar::SemVer).unwrap();
        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![callisto_model::CratePublish {
                name: "my-crate".to_string(),
                version: planned_version.clone(),
                publish_to: callisto_model::RegistryKey(
                    callisto_model::RegistryKey::CRATES_IO.to_string(),
                ),
                registry: None,
                package_dir: Some(std::path::PathBuf::from("crates/my-crate")),
            }],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };
        let pkg = PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "my-crate".to_string(),
        };
        let mut c = SubprocessRegistryClient::new(
            ScriptedRunner(output(0, "Uploading my-crate v2.0.0\n", "")),
            dir.path().to_path_buf(),
        );
        c.load_plan(&plan);

        let err = c
            .publish(&pkg, &planned_version, &permit())
            .expect_err("expected an error when the on-disk Cargo.toml cannot be read");
        assert!(
            matches!(err, RegistryError::Other(_)),
            "unreadable manifest must produce RegistryError::Other, got: {err:?}"
        );
    }

    /// Version guard: when the on-disk Cargo.toml exists but cannot be parsed
    /// as a minimal `[package].version` manifest (e.g. it is malformed TOML or
    /// a workspace root without a `[package]` table), publish must return Err
    /// rather than silently skipping the guard.
    #[test]
    fn cargo_version_guard_errors_when_manifest_unparseable() {
        let dir = tempfile::tempdir().unwrap();
        let crate_dir = dir.path().join("crates/my-crate");
        std::fs::create_dir_all(&crate_dir).unwrap();
        // Write TOML that has no [package] table — matches a workspace root or a
        // subtly malformed crate manifest.
        std::fs::write(
            crate_dir.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();

        let planned_version = Version::parse("2.0.0", VersionGrammar::SemVer).unwrap();
        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![callisto_model::CratePublish {
                name: "my-crate".to_string(),
                version: planned_version.clone(),
                publish_to: callisto_model::RegistryKey(
                    callisto_model::RegistryKey::CRATES_IO.to_string(),
                ),
                registry: None,
                package_dir: Some(std::path::PathBuf::from("crates/my-crate")),
            }],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };
        let pkg = PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "my-crate".to_string(),
        };
        let mut c = SubprocessRegistryClient::new(
            ScriptedRunner(output(0, "Uploading my-crate v2.0.0\n", "")),
            dir.path().to_path_buf(),
        );
        c.load_plan(&plan);

        let err = c
            .publish(&pkg, &planned_version, &permit())
            .expect_err("expected an error when Cargo.toml has no [package] table");
        assert!(
            matches!(err, RegistryError::Other(_)),
            "unparseable manifest must produce RegistryError::Other, got: {err:?}"
        );
    }

    /// Regression: `version.workspace = true` (Cargo's workspace-version
    /// inheritance syntax, used by all 10 of this repo's own crates) parses
    /// to a TOML *table* for `[package]`, not a string, so the old
    /// `MinimalManifest { package: MinimalPackage { version: String } }`
    /// deserialization failed with "invalid type: map, expected a string" on
    /// every crate using this pattern — even when the on-disk version was
    /// correct. The pre-publish guard must resolve the inherited version via
    /// the workspace root's `[workspace.package].version` and succeed when it
    /// matches the plan.
    #[test]
    fn cargo_publish_proceeds_when_on_disk_version_is_workspace_inherited() {
        let dir = tempfile::tempdir().unwrap();

        // Workspace root Cargo.toml declaring the inherited version, matching
        // this repo's own real layout (see e.g. crates/callisto-model/Cargo.toml).
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.package]\nversion = \"1.2.0\"\n",
        )
        .unwrap();

        // Member crate uses `version.workspace = true` instead of a literal
        // string version.
        let pkg_subdir = dir.path().join("crates/my-crate");
        std::fs::create_dir_all(&pkg_subdir).unwrap();
        std::fs::write(
            pkg_subdir.join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion.workspace = true\nedition = \"2021\"\n",
        )
        .unwrap();

        let planned_version = Version::parse("1.2.0", VersionGrammar::SemVer).unwrap();
        let plan = callisto_model::PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![callisto_model::CratePublish {
                name: "my-crate".to_string(),
                version: planned_version.clone(),
                publish_to: callisto_model::RegistryKey(
                    callisto_model::RegistryKey::CRATES_IO.to_string(),
                ),
                registry: None,
                package_dir: Some(std::path::PathBuf::from("crates/my-crate")),
            }],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };

        let pkg = PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "my-crate".to_string(),
        };

        let mut c = SubprocessRegistryClient::new(
            ScriptedRunner(output(0, "Uploading my-crate v1.2.0\n", "")),
            dir.path().to_path_buf(),
        );
        c.load_plan(&plan);

        let outcome = c
            .publish(&pkg, &planned_version, &permit())
            .expect("pre-publish version guard must resolve version.workspace = true, not error");
        assert_eq!(outcome, PublishOutcome::Published);
    }
}
