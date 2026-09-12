//! Pure argv construction for "publish a package to its registry", plus
//! subprocess-output classification for the same three ecosystems (Cargo,
//! npm, PyPI).
//!
//! This module owns exactly two questions per ecosystem: *what command do we
//! run* (given package/plan facts and already-detected environment facts --
//! never a [`callisto_model::CommandRunner`] call of its own) and *what did
//! the finished command's output mean*. Actually invoking the command, and
//! any retry/backoff policy around a failed attempt, is the caller's job --
//! see [`super::release::ValidatedReleaseIntent::dispatch_prepared`], the
//! durable release executor's single production publish path.
//!
//! A small amount of local filesystem reading happens here (the Cargo
//! on-disk version check, npm package-manager detection from lockfiles) --
//! that's "gathering the facts needed to build the right argv", not
//! executing a publish. It never shells out.

use std::path::{Path, PathBuf};
use std::time::Duration;

use callisto_manifests::WorkspaceCargoResolver;
use callisto_model::{
    normalize_pypi_package_name, CommandOutput, Ecosystem, NpmAccess, PackageId, PublishOutcome, RegistryError, Version,
};

use crate::error::GraphError;

/// Default fallback wait (seconds) when a registry does not supply a
/// `retry_after` value and the classifier cannot parse one from output.
const DEFAULT_RATE_LIMIT_WAIT_SECS: u64 = 60;

/// One subprocess invocation: program, args, and working directory. Building
/// this is the entire job of the `*_publish_argv` functions below --
/// executing it is the caller's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argv {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

// ---------------------------------------------------------------- cargo

/// Builds the exact `cargo publish` argv for one crate: `--manifest-path`
/// (so the caller can keep the workspace root as its own working directory)
/// plus `--locked`, so a stale `Cargo.lock` fails the publish immediately
/// instead of `cargo` silently re-resolving one at publish time.
/// `registry_key` is `Some` only for a non-default registry -- crates.io is
/// the implicit default and never needs `--registry`.
///
/// Before returning, verifies that the on-disk `Cargo.toml` version under
/// `package_dir` (resolving `version.workspace = true` against the
/// workspace root's `[workspace.package].version` when present) matches
/// `version` -- this is the one behavior `SubprocessRegistryClient` had that
/// the durable release executor's own `dispatch_registry` had silently
/// dropped: it catches the common mistake of running `release execute`
/// without first running `callisto version`, which would otherwise silently
/// publish the old on-disk version.
pub fn cargo_publish_argv(
    workspace_root: &Path,
    package_dir: &Path,
    package_name: &str,
    version: &Version,
    registry_key: Option<&str>,
) -> Result<Argv, GraphError> {
    verify_cargo_ondisk_version(workspace_root, package_dir, package_name, version)?;

    let manifest_path = workspace_root.join(package_dir).join("Cargo.toml");
    let mut args = vec![
        "publish".to_string(),
        "--manifest-path".to_string(),
        manifest_path.to_string_lossy().into_owned(),
        "--locked".to_string(),
    ];
    if let Some(reg) = registry_key {
        args.push("--registry".to_string());
        args.push(reg.to_string());
    }
    Ok(Argv {
        program: "cargo".to_string(),
        args,
        cwd: workspace_root.to_path_buf(),
    })
}

fn verify_cargo_ondisk_version(
    workspace_root: &Path,
    package_dir: &Path,
    package_name: &str,
    planned: &Version,
) -> Result<(), GraphError> {
    let manifest_path = workspace_root.join(package_dir).join("Cargo.toml");
    let contents = std::fs::read_to_string(&manifest_path).map_err(|error| GraphError::ReleaseInputRead {
        path: manifest_path.clone(),
        message: error.to_string(),
    })?;
    // Parse only the [package].version field -- no need for full CST. A
    // permissive `toml::Value` intermediate rather than a direct `String`
    // deserialization, because `version.workspace = true` (Cargo's
    // workspace-version-inheritance syntax) parses `version` as a table, not
    // a string.
    let parsed = toml::from_str::<toml::Value>(&contents).map_err(|error| GraphError::ReleaseInputRead {
        path: manifest_path.clone(),
        message: error.to_string(),
    })?;
    let version_value =
        parsed
            .get("package")
            .and_then(|p| p.get("version"))
            .ok_or_else(|| GraphError::ReleaseInputRead {
                path: manifest_path.clone(),
                message: "missing [package].version".to_string(),
            })?;

    let on_disk = if let Some(s) = version_value.as_str() {
        s.to_string()
    } else if version_value.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
        let root_manifest_path = workspace_root.join("Cargo.toml");
        let resolver = WorkspaceCargoResolver::load(&root_manifest_path)?;
        let ws_version = resolver
            .workspace_version()?
            .ok_or_else(|| GraphError::ReleaseInputRead {
                path: root_manifest_path.clone(),
                message: "`version.workspace = true` but the workspace root has no [workspace.package].version"
                    .to_string(),
            })?;
        ws_version.render().to_string()
    } else {
        return Err(GraphError::ReleaseInputRead {
            path: manifest_path,
            message: "[package].version is neither a string nor a `{ workspace = true }` table".to_string(),
        });
    };

    let found = Version::parse(&on_disk, planned.grammar())?;
    if found != *planned {
        return Err(GraphError::OnDiskVersionDrift {
            package: PackageId::Prefixed {
                ecosystem: Ecosystem::Cargo,
                name: package_name.to_string(),
            },
            expected: planned.clone(),
            found,
        });
    }
    Ok(())
}

// ------------------------------------------------------------------ npm

/// The npm-compatible package manager a workspace uses, detected from
/// lockfile presence. Each one needs a materially different invocation to
/// publish a single named package out of a monorepo -- see
/// [`npm_publish_argv`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NpmPackageManager {
    Pnpm,
    Yarn,
    Bun,
    Npm,
}

/// Detects the npm-compatible package manager for `workspace_root` from
/// lockfile presence. Detection priority: pnpm > yarn > bun > npm (default).
///
/// This is real, live-today correctness content: the durable release
/// executor's `dispatch_registry` used to hardcode plain `npm publish`
/// regardless of which package manager a workspace actually uses, so a pnpm
/// or yarn workspace published with the wrong tool.
pub fn detect_npm_package_manager(workspace_root: &Path) -> NpmPackageManager {
    if workspace_root.join("pnpm-lock.yaml").exists() {
        NpmPackageManager::Pnpm
    } else if workspace_root.join("yarn.lock").exists() {
        NpmPackageManager::Yarn
    } else if workspace_root.join("bun.lockb").exists() || workspace_root.join("bun.lock").exists() {
        // bun.lockb is the legacy binary lockfile format; bun.lock (text) is
        // written by Bun >= 1.1.1. Both signal a bun-managed workspace.
        NpmPackageManager::Bun
    } else {
        NpmPackageManager::Npm
    }
}

/// Builds the npm-ecosystem publish argv for one package, given the
/// already-detected `package_manager` (see [`detect_npm_package_manager`]).
///
/// | Package manager | Program | Base args                                  |
/// |------------------|---------|--------------------------------------------|
/// | Pnpm             | `pnpm`  | `publish --filter <name> --no-git-checks`  |
/// | Yarn             | `yarn`  | `workspace <name> npm publish`             |
/// | Bun              | `bun`   | `publish` (run from `package_dir`)         |
/// | Npm              | `npm`   | `publish --workspace <name>`               |
///
/// `tag`/`access`/`registry` are appended to every variant's base args.
/// `package_dir` is only used for bun: it has no `--filter`/`--workspace`
/// flag to target one package by name, so it alone must run from the
/// package's own directory rather than the workspace root.
pub fn npm_publish_argv(
    workspace_root: &Path,
    package_dir: &Path,
    package_name: &str,
    package_manager: NpmPackageManager,
    tag: Option<&str>,
    access: Option<NpmAccess>,
    registry: Option<&str>,
) -> Argv {
    let mut extra: Vec<String> = Vec::new();
    if let Some(t) = tag {
        extra.push("--tag".to_string());
        extra.push(t.to_string());
    }
    if let Some(access) = access {
        extra.push("--access".to_string());
        extra.push(
            match access {
                NpmAccess::Public => "public",
                NpmAccess::Restricted => "restricted",
            }
            .to_string(),
        );
    }
    if let Some(reg) = registry {
        extra.push("--registry".to_string());
        extra.push(reg.to_string());
    }

    let (program, mut args, cwd) = match package_manager {
        NpmPackageManager::Pnpm => (
            "pnpm",
            vec![
                "publish".to_string(),
                "--filter".to_string(),
                package_name.to_string(),
                // pnpm >= 7 refuses to publish from a dirty working tree by
                // default. After `callisto version` stages manifest bumps,
                // the tree is always dirty until the operator commits, so
                // this bypasses the git-status check.
                "--no-git-checks".to_string(),
            ],
            workspace_root.to_path_buf(),
        ),
        NpmPackageManager::Yarn => (
            "yarn",
            vec![
                "workspace".to_string(),
                package_name.to_string(),
                "npm".to_string(),
                "publish".to_string(),
            ],
            workspace_root.to_path_buf(),
        ),
        NpmPackageManager::Bun => ("bun", vec!["publish".to_string()], workspace_root.join(package_dir)),
        NpmPackageManager::Npm => (
            "npm",
            vec![
                "publish".to_string(),
                "--workspace".to_string(),
                package_name.to_string(),
            ],
            workspace_root.to_path_buf(),
        ),
    };
    args.extend(extra);
    Argv {
        program: program.to_string(),
        args,
        cwd,
    }
}

// ----------------------------------------------------------------- pypi

/// Builds the two-step PyPI publish argv sequence: build the sdist and
/// wheel into `dist/`, then upload the exact `dist/<normalized-name>-
/// <version>*` glob via `twine upload --skip-existing`. Both steps run from
/// `package_dir` so `dist/` resolves to that package's own directory in a
/// monorepo, and neither ever builds into or uploads from a fresh temporary
/// directory -- `twine`'s own glob targets exactly the artifacts this
/// `python -m build` invocation just produced, not whatever else happens to
/// be sitting in `dist/`.
///
/// Package name is normalized to PEP 427 wheel-filename form (lowercased,
/// `-`/`.` -> `_`) before building the glob; `twine` expands the glob
/// internally, so the literal `*` is safe with no shell involved.
/// `index: Some` inserts `--repository-url <url>` before the glob, to target
/// a private index (Nexus/Artifactory PyPI proxy) instead of public PyPI.
pub fn pypi_publish_argv(
    workspace_root: &Path,
    package_dir: &Path,
    package_name: &str,
    version: &Version,
    index: Option<&str>,
) -> Vec<Argv> {
    let cwd = workspace_root.join(package_dir);
    let build = Argv {
        program: "python".to_string(),
        args: vec![
            "-m".to_string(),
            "build".to_string(),
            "--sdist".to_string(),
            "--wheel".to_string(),
            "--outdir".to_string(),
            "dist/".to_string(),
        ],
        cwd: cwd.clone(),
    };

    let normalized = normalize_pypi_package_name(package_name);
    let pattern = format!("dist/{normalized}-{}*", version.render());
    let mut upload_args = vec!["upload".to_string(), "--skip-existing".to_string()];
    if let Some(idx) = index {
        upload_args.push("--repository-url".to_string());
        upload_args.push(idx.to_string());
    }
    upload_args.push(pattern);
    let upload = Argv {
        program: "twine".to_string(),
        args: upload_args,
        cwd,
    };

    vec![build, upload]
}

// ---- shared output-classification helpers --------------------------------

fn combined_lower(output: &CommandOutput) -> String {
    format!("{}\n{}", output.stdout, output.stderr).to_lowercase()
}

/// Parses a numeric retry-after value (seconds) as reported by a registry
/// tool's output.
fn parse_retry_after(raw: &str) -> Option<Duration> {
    raw.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Looks for a `retry after <N>` mention in already-lowercased text and
/// parses `<N>` as a whole-second duration.
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
    // would otherwise trigger this. Require at least one contextual phrase
    // alongside any "429".
    let is_rate_limited = text_lower.contains("too many requests")
        || text_lower.contains("rate limit")
        || text_lower.contains("erate_limit")
        || (text_lower.contains("429")
            && (text_lower.contains("too many") || text_lower.contains("rate") || text_lower.contains("retry")));
    if is_rate_limited {
        let dur = extract_retry_after_duration(text_lower).unwrap_or(Duration::from_secs(DEFAULT_RATE_LIMIT_WAIT_SECS));
        Some(RegistryError::RateLimited(dur))
    } else {
        None
    }
}

fn detect_auth_failure(text_lower: &str, raw_stderr: &CommandOutput) -> Option<RegistryError> {
    // Match "401"/"403" with context, never on bare digits alone -- same
    // rationale as detect_rate_limit's guard against bare "429": a byte
    // count or elapsed-time figure in cargo's own packaging/build output
    // ("403129 bytes", "elapsed 4013ms") would otherwise misclassify an
    // unrelated failure as an auth failure.
    if (text_lower.contains("401") && text_lower.contains("auth"))
        || (text_lower.contains("403") && (text_lower.contains("auth") || text_lower.contains("forbidden")))
        || text_lower.contains("authentication")
        || text_lower.contains("not logged in")
        || text_lower.contains("invalid token")
        || text_lower.contains("forbidden")
    {
        Some(RegistryError::AuthFailed(raw_stderr.redacted_stderr()))
    } else {
        None
    }
}

/// Classifies combined `cargo publish` output into a [`PublishOutcome`] or
/// [`RegistryError`].
pub(crate) fn classify_cargo_output(output: &CommandOutput) -> Result<PublishOutcome, RegistryError> {
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
    if let Some(err) = detect_auth_failure(&combined, output) {
        return Err(err);
    }
    Err(RegistryError::Other(format!(
        "cargo publish failed (exit {:?}): {}",
        output.exit_code,
        output.redacted_stderr().trim()
    )))
}

/// Classifies `cargo info <pkg>@<version>` output into whether the registry
/// confirms that version is published. An ambiguous failure (not a clear
/// "not found") is surfaced as an error, never read as "not published".
pub(crate) fn classify_cargo_info_output(output: &CommandOutput) -> Result<bool, RegistryError> {
    if output.success() {
        return Ok(true);
    }
    let combined = combined_lower(output);
    if combined.contains("could not find") {
        return Ok(false);
    }
    Err(RegistryError::Other(format!(
        "cargo info failed (exit {:?}): {}",
        output.exit_code,
        output.redacted_stderr().trim()
    )))
}

/// Classifies combined `npm publish` (or `pnpm`/`yarn`/`bun` equivalent)
/// output into a [`PublishOutcome`] or [`RegistryError`].
pub(crate) fn classify_npm_publish_output(output: &CommandOutput) -> Result<PublishOutcome, RegistryError> {
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
    if let Some(err) = detect_auth_failure(&combined, output) {
        return Err(err);
    }
    Err(RegistryError::Other(format!(
        "npm publish failed (exit {:?}): {}",
        output.exit_code,
        output.redacted_stderr().trim()
    )))
}

/// Classifies combined `twine upload` output into a [`PublishOutcome`] or
/// [`RegistryError`].
///
/// Priority order (highest wins):
/// 1. `"already exist"` (from `--skip-existing`) without `"uploading"` ->
///    [`PublishOutcome::AlreadyPublished`] (all artifacts skipped). Both
///    phrases present means at least one artifact was freshly uploaded ->
///    `Published`. Handles `dist/` accumulating stale pre-release
///    artifacts: `dist/my_pkg-1.0.0*` matches both `my_pkg-1.0.0a0.whl`
///    (skipped) and `my_pkg-1.0.0.whl` (uploaded), producing mixed output.
/// 2. Zero exit code with no skip-only mention -> [`PublishOutcome::Published`].
/// 3. Rate-limit signal (`429`, `too many requests`, `rate limit`) ->
///    [`RegistryError::RateLimited`] with a parsed or default 60-second
///    retry-after duration.
/// 4. Auth-failure signal (`401`, `403`, `authentication`, etc.) ->
///    [`RegistryError::AuthFailed`].
/// 5. Anything else -> [`RegistryError::Other`].
pub(crate) fn classify_twine_output(output: &CommandOutput) -> Result<PublishOutcome, RegistryError> {
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
    if let Some(err) = detect_auth_failure(&combined, output) {
        return Err(err);
    }
    Err(RegistryError::Other(format!(
        "twine upload failed (exit {:?}): {}",
        output.exit_code,
        output.redacted_stderr().trim()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(exit_code: i32, stdout: &str, stderr: &str) -> CommandOutput {
        CommandOutput {
            exit_code: Some(exit_code),
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    fn v(raw: &str) -> Version {
        Version::parse(raw, callisto_model::VersionGrammar::SemVer).unwrap()
    }

    // ---------------------------------------------------------------- cargo

    #[test]
    fn cargo_publish_argv_uses_manifest_path_and_locked_with_no_registry_for_default() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("Cargo.toml");
        std::fs::write(&manifest, "[package]\nname = \"demo\"\nversion = \"1.0.0\"\n").unwrap();

        let argv = cargo_publish_argv(dir.path(), Path::new("."), "demo", &v("1.0.0"), None).unwrap();
        assert_eq!(argv.program, "cargo");
        assert!(argv.args.contains(&"--locked".to_string()));
        assert!(argv.args.contains(&"--manifest-path".to_string()));
        assert!(!argv.args.iter().any(|a| a == "--registry"));
        assert_eq!(argv.cwd, dir.path());
    }

    #[test]
    fn cargo_publish_argv_adds_registry_flag_when_non_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"2.0.0\"\n",
        )
        .unwrap();

        let argv = cargo_publish_argv(dir.path(), Path::new("."), "demo", &v("2.0.0"), Some("my-registry")).unwrap();
        assert!(argv.args.windows(2).any(|w| w == ["--registry", "my-registry"]));
    }

    #[test]
    fn cargo_publish_argv_rejects_ondisk_version_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();

        let err = cargo_publish_argv(dir.path(), Path::new("."), "demo", &v("2.0.0"), None).unwrap_err();
        assert!(
            matches!(err, GraphError::OnDiskVersionDrift { .. }),
            "expected OnDiskVersionDrift, got: {err:?}"
        );
    }

    #[test]
    fn cargo_publish_argv_resolves_workspace_inherited_version() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/demo\"]\n\n[workspace.package]\nversion = \"3.1.4\"\n",
        )
        .unwrap();
        let crate_dir = dir.path().join("crates/demo");
        std::fs::create_dir_all(&crate_dir).unwrap();
        std::fs::write(
            crate_dir.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion.workspace = true\n",
        )
        .unwrap();

        let argv = cargo_publish_argv(dir.path(), Path::new("crates/demo"), "demo", &v("3.1.4"), None).unwrap();
        assert!(argv
            .args
            .iter()
            .any(|a| a.ends_with("crates/demo/Cargo.toml") || a.ends_with("crates\\demo\\Cargo.toml")));
    }

    // ------------------------------------------------------------------ npm

    #[test]
    fn detects_pnpm_from_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(detect_npm_package_manager(dir.path()), NpmPackageManager::Pnpm);
    }

    #[test]
    fn detects_yarn_from_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("yarn.lock"), "").unwrap();
        assert_eq!(detect_npm_package_manager(dir.path()), NpmPackageManager::Yarn);
    }

    #[test]
    fn detects_bun_from_binary_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bun.lockb"), "").unwrap();
        assert_eq!(detect_npm_package_manager(dir.path()), NpmPackageManager::Bun);
    }

    #[test]
    fn detects_bun_from_text_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bun.lock"), "").unwrap();
        assert_eq!(detect_npm_package_manager(dir.path()), NpmPackageManager::Bun);
    }

    #[test]
    fn defaults_to_npm_with_no_recognized_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_npm_package_manager(dir.path()), NpmPackageManager::Npm);
    }

    #[test]
    fn pnpm_publish_argv_uses_filter_and_no_git_checks() {
        let argv = npm_publish_argv(
            Path::new("/workspace"),
            Path::new("packages/a"),
            "pkg-a",
            NpmPackageManager::Pnpm,
            None,
            None,
            None,
        );
        assert_eq!(argv.program, "pnpm");
        assert_eq!(argv.args, vec!["publish", "--filter", "pkg-a", "--no-git-checks"]);
        assert_eq!(argv.cwd, Path::new("/workspace"));
    }

    #[test]
    fn yarn_publish_argv_uses_workspace_npm_publish() {
        let argv = npm_publish_argv(
            Path::new("/workspace"),
            Path::new("packages/a"),
            "pkg-a",
            NpmPackageManager::Yarn,
            None,
            None,
            None,
        );
        assert_eq!(argv.program, "yarn");
        assert_eq!(argv.args, vec!["workspace", "pkg-a", "npm", "publish"]);
        assert_eq!(argv.cwd, Path::new("/workspace"));
    }

    #[test]
    fn bun_publish_argv_runs_from_package_dir() {
        let argv = npm_publish_argv(
            Path::new("/workspace"),
            Path::new("packages/a"),
            "pkg-a",
            NpmPackageManager::Bun,
            None,
            None,
            None,
        );
        assert_eq!(argv.program, "bun");
        assert_eq!(argv.args, vec!["publish"]);
        assert_eq!(argv.cwd, Path::new("/workspace/packages/a"));
    }

    #[test]
    fn npm_publish_argv_uses_workspace_flag() {
        let argv = npm_publish_argv(
            Path::new("/workspace"),
            Path::new("packages/a"),
            "pkg-a",
            NpmPackageManager::Npm,
            None,
            None,
            None,
        );
        assert_eq!(argv.program, "npm");
        assert_eq!(argv.args, vec!["publish", "--workspace", "pkg-a"]);
    }

    #[test]
    fn npm_publish_argv_appends_tag_access_and_registry() {
        let argv = npm_publish_argv(
            Path::new("/workspace"),
            Path::new("packages/a"),
            "pkg-a",
            NpmPackageManager::Npm,
            Some("next"),
            Some(NpmAccess::Public),
            Some("https://registry.example.com"),
        );
        assert_eq!(
            argv.args,
            vec![
                "publish",
                "--workspace",
                "pkg-a",
                "--tag",
                "next",
                "--access",
                "public",
                "--registry",
                "https://registry.example.com",
            ]
        );
    }

    // ----------------------------------------------------------------- pypi

    #[test]
    fn pypi_publish_argv_builds_into_dist_and_uploads_normalized_glob() {
        let argvs = pypi_publish_argv(
            Path::new("/workspace"),
            Path::new("packages/my-pkg"),
            "My.Pkg",
            &v("1.2.3"),
            None,
        );
        assert_eq!(argvs.len(), 2);

        let build = &argvs[0];
        assert_eq!(build.program, "python");
        assert_eq!(
            build.args,
            vec!["-m", "build", "--sdist", "--wheel", "--outdir", "dist/"]
        );
        assert_eq!(build.cwd, Path::new("/workspace/packages/my-pkg"));

        let upload = &argvs[1];
        assert_eq!(upload.program, "twine");
        assert_eq!(upload.args, vec!["upload", "--skip-existing", "dist/my_pkg-1.2.3*"]);
        assert_eq!(upload.cwd, Path::new("/workspace/packages/my-pkg"));
    }

    #[test]
    fn pypi_publish_argv_adds_repository_url_for_private_index() {
        let argvs = pypi_publish_argv(
            Path::new("/workspace"),
            Path::new("packages/my-pkg"),
            "my-pkg",
            &v("1.0.0"),
            Some("https://pypi.example.com/simple"),
        );
        assert_eq!(
            argvs[1].args,
            vec![
                "upload",
                "--skip-existing",
                "--repository-url",
                "https://pypi.example.com/simple",
                "dist/my_pkg-1.0.0*",
            ]
        );
    }

    // ---------------------------------------------------------- classification

    #[test]
    fn classify_cargo_output_recognizes_already_exists() {
        let out = output(101, "", "error: crate version `1.0.0` is already uploaded");
        assert!(matches!(
            classify_cargo_output(&out),
            Ok(PublishOutcome::AlreadyPublished)
        ));
    }

    #[test]
    fn classify_cargo_output_success_is_published() {
        let out = output(0, "Uploading demo v1.0.0", "");
        assert!(matches!(classify_cargo_output(&out), Ok(PublishOutcome::Published)));
    }

    #[test]
    fn classify_cargo_output_rate_limit_is_detected() {
        let out = output(1, "", "error: too many requests, retry after 30 seconds");
        match classify_cargo_output(&out) {
            Err(RegistryError::RateLimited(dur)) => assert_eq!(dur, Duration::from_secs(30)),
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn classify_cargo_output_ignores_bare_429_in_byte_counts() {
        let out = output(1, "", "429.1KiB compressed; some other failure");
        match classify_cargo_output(&out) {
            Err(RegistryError::Other(_)) => {}
            other => panic!("bare 429 byte count must not be classified as rate-limited, got {other:?}"),
        }
    }

    #[test]
    fn classify_cargo_output_redacts_secrets_in_error_message() {
        std::env::set_var("CARGO_REGISTRY_TOKEN", "super-secret-token-value");
        let out = output(1, "", "authentication failed: token super-secret-token-value invalid");
        let err = classify_cargo_output(&out).unwrap_err();
        assert!(
            !err.to_string().contains("super-secret-token-value"),
            "secret must be redacted from error message, got: {err}"
        );
        std::env::remove_var("CARGO_REGISTRY_TOKEN");
    }

    #[test]
    fn classify_cargo_info_output_success_confirms_published() {
        let out = output(0, "demo\nversion: 1.0.0", "");
        assert_eq!(classify_cargo_info_output(&out), Ok(true));
    }

    #[test]
    fn classify_cargo_info_output_not_found_is_not_yet_published() {
        let out = output(
            101,
            "",
            "error: could not find `demo@1.0.0` in registry `https://github.com/rust-lang/crates.io-index`",
        );
        assert_eq!(classify_cargo_info_output(&out), Ok(false));
    }

    #[test]
    fn classify_cargo_info_output_other_failure_is_an_error_not_a_false() {
        let out = output(1, "", "error: failed to update the registry index: network timeout");
        match classify_cargo_info_output(&out) {
            Err(RegistryError::Other(_)) => {}
            other => panic!("ambiguous failure must not read as 'not published', got {other:?}"),
        }
    }

    #[test]
    fn classify_npm_publish_output_recognizes_conflict_as_already_published() {
        let out = output(1, "", "npm ERR! code E409\nnpm ERR! 409 Conflict");
        assert!(matches!(
            classify_npm_publish_output(&out),
            Ok(PublishOutcome::AlreadyPublished)
        ));
    }

    #[test]
    fn classify_npm_publish_output_auth_failure() {
        let out = output(1, "", "npm ERR! code E401\nnpm ERR! 401 authentication required");
        assert!(matches!(
            classify_npm_publish_output(&out),
            Err(RegistryError::AuthFailed(_))
        ));
    }

    #[test]
    fn classify_twine_output_all_skipped_is_already_published() {
        let out = output(0, "Skipping my_pkg-1.0.0a0-py3-none-any.whl (already exists)", "");
        assert!(matches!(
            classify_twine_output(&out),
            Ok(PublishOutcome::AlreadyPublished)
        ));
    }

    #[test]
    fn classify_twine_output_mixed_skip_and_upload_is_published() {
        let out = output(
            0,
            "Skipping my_pkg-1.0.0a0-py3-none-any.whl (already exists)\nUploading my_pkg-1.0.0-py3-none-any.whl",
            "",
        );
        assert!(matches!(classify_twine_output(&out), Ok(PublishOutcome::Published)));
    }

    #[test]
    fn classify_twine_output_failure_is_other() {
        let out = output(1, "", "some unrecognized failure");
        assert!(matches!(classify_twine_output(&out), Err(RegistryError::Other(_))));
    }
}
