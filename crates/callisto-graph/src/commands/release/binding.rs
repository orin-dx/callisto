//! Canonical, credential-free registry and Git destinations.
//!
//! Every release effect is routed through a binding produced here, so no
//! caller-supplied endpoint or credential can reach a provider.

use std::path::Path;

use callisto_model::vcs::{GitAccess, GitRemote};
use callisto_model::{CommandRunner, Ecosystem, PackageId, PublishTarget, RegistryBindingDigest, RegistryKey};

use crate::registry_endpoint::{canonical_registry_url, split_routes_digest, RegistryBindingV1};
use crate::{DependencyResolver, GraphError, Workspace};

use super::StaleReason;

/// Credential-free, canonical registry routing. `endpoint` is populated only
/// after parsing rejects userinfo, query, and fragments, so a later executor
/// cannot recover credentials from this capability.
#[derive(Debug)]
pub(crate) struct PreparedRegistryBinding {
    pub(crate) key: RegistryKey,
    pub(crate) endpoint: Option<String>,
    /// Where uploads go when that differs from `endpoint` (a PyPI index that is not also its upload route).
    pub(crate) publish_endpoint: Option<String>,
    pub(crate) identity: RegistryBindingDigest,
    /// How long a version this run published may take to appear; execution policy, so not part of `identity`.
    pub(crate) confirm_timeout: Option<std::time::Duration>,
    /// The longest one publish command may run; execution policy, so not part of `identity`.
    pub(crate) publish_timeout: Option<std::time::Duration>,
}

impl PreparedRegistryBinding {
    /// The endpoint an upload is sent to: the separate upload route if one is bound, else the index.
    pub(crate) fn upload_endpoint(&self) -> Option<&str> {
        self.publish_endpoint.as_deref().or(self.endpoint.as_deref())
    }
}

/// The single registry-trust validator: resolves `target`'s own registry key, rejects a
/// non-https or credentialed URL, and rejects an npm `publishConfig.registry` override (package-
/// controlled data) unless it matches a `url` on an npm-kind `[registries]` entry.
pub(crate) fn prepared_registry_binding<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    target: &PublishTarget,
    package: &PackageId,
) -> Result<PreparedRegistryBinding, GraphError> {
    let key = target.registry_key().ok_or_else(|| GraphError::ReleaseInvariant {
        detail: format!("publish target `{}` has no registry key", target.config_str()),
    })?;
    let registry = workspace
        .config
        .registries
        .get(&key)
        .ok_or_else(|| GraphError::ReleaseInvariant {
            detail: format!("unknown registry `{}`", key.as_str()),
        })?;
    let explicit = target.registry_override();
    // An explicit per-package index names both routes, so the registry entry's upload route does not apply.
    let publish_raw = match explicit {
        Some(_) => None,
        None => registry.publish_url.as_deref(),
    };
    let raw = explicit.or(registry.url.as_deref());
    let index = raw
        .map(|raw| canonical_registry_binding(key.as_str(), raw))
        .transpose()?;
    let publish = publish_raw
        .map(|raw| canonical_registry_binding(key.as_str(), raw))
        .transpose()?;
    if let (Some(raw), Some(binding)) = (explicit, &index)
        && matches!(target, PublishTarget::Npm { .. })
        && !is_approved_npm_registry(workspace, binding)
    {
        return Err(GraphError::UntrustedNpmRegistry {
            package: package.clone(),
            url: raw.to_owned(),
        });
    }
    let identity = match (&index, &publish) {
        (_, Some(publish)) => split_routes_digest(index.as_ref(), publish),
        (Some(index), None) => index.digest(),
        (None, None) => RegistryBindingDigest::from_normalized_binding(key.as_str().as_bytes()),
    };
    Ok(PreparedRegistryBinding {
        key,
        endpoint: index.as_ref().map(RegistryBindingV1::endpoint),
        publish_endpoint: publish.as_ref().map(RegistryBindingV1::endpoint),
        identity,
        confirm_timeout: registry.confirm_timeout,
        publish_timeout: registry.publish_timeout,
    })
}

fn is_approved_npm_registry<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    binding: &RegistryBindingV1,
) -> bool {
    workspace.config.registries.values().any(|registry| {
        registry.kind == Ecosystem::Npm
            && registry
                .url
                .as_deref()
                .and_then(|url| canonical_registry_url(url).ok())
                .is_some_and(|approved| approved == *binding)
    })
}

pub(crate) fn canonical_registry_binding(registry: &str, raw: &str) -> Result<RegistryBindingV1, GraphError> {
    canonical_registry_url(raw).map_err(|reason| GraphError::UnsafeRegistryBinding {
        registry: registry.to_string(),
        reason,
    })
}

/// `origin`, as git resolves it. Release intents record a digest of its URLs, and a digest of a guessable password
/// can be recovered by guessing, so a URL that carries credentials is refused.
pub(crate) fn optional_git_remote(root: &Path, runner: &dyn CommandRunner) -> Result<Option<GitRemote>, GraphError> {
    let remote = GitAccess::new(root, runner).remote(GitRemote::ORIGIN)?;
    if remote.as_ref().is_some_and(GitRemote::embeds_credentials) {
        return Err(GraphError::UnsafeGitRemote {
            reason: "a fetch or push URL of origin carries credentials",
        });
    }
    Ok(remote)
}

/// [`optional_git_remote`] for a run that pushes tags, which needs `origin`.
pub(crate) fn prepared_git_remote(root: &Path, runner: &dyn CommandRunner) -> Result<GitRemote, GraphError> {
    optional_git_remote(root, runner)?.ok_or(GraphError::UnsafeGitRemote {
        reason: "origin is not configured",
    })
}

/// Re-reads `origin` and refuses unless git still resolves it to exactly the URLs the intent was validated against.
pub(crate) fn recheck_git_remote(
    root: &Path,
    runner: &dyn CommandRunner,
    expected: &GitRemote,
) -> Result<(), GraphError> {
    if prepared_git_remote(root, runner)?.fingerprint() != expected.fingerprint() {
        return Err(GraphError::ReleaseIntentStale {
            reason: StaleReason::git_remote_changed(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_binding_rejects_credentials_and_ambiguous_routing() {
        for value in [
            "https://token@registry.example.test/index",
            "https://registry.example.test/index?token=secret",
            "https://registry.example.test/index#fragment",
        ] {
            assert!(
                canonical_registry_binding("test", value).is_err(),
                "{value} must be rejected"
            );
        }
    }

    #[test]
    fn registry_binding_normalizes_host_and_default_port_without_retaining_url() {
        let explicit = canonical_registry_binding("test", "HTTPS://Registry.Example.Test:443/a/../index").unwrap();
        let implicit = canonical_registry_binding("test", "https://registry.example.test/index").unwrap();
        assert_eq!(explicit, implicit);
        assert_eq!(explicit.host, "registry.example.test");
        assert_eq!(explicit.effective_port, Some(443));
        assert_eq!(explicit.path, "/index");
    }

    /// Answers `git remote -v` with no remotes and records whether the probe went through the live-echoing
    /// `run_with_timeout` path or the quiet one: a remote read never echoes git's own output.
    struct NoRemote {
        used_live_echo: std::sync::atomic::AtomicBool,
    }

    impl NoRemote {
        fn new() -> Self {
            Self {
                used_live_echo: std::sync::atomic::AtomicBool::new(false),
            }
        }

        fn failing_output() -> callisto_model::CommandOutput {
            callisto_model::CommandOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            }
        }
    }

    impl callisto_model::CommandRunner for NoRemote {
        fn run(
            &self,
            _program: &str,
            _args: &[&str],
            _cwd: &Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            Ok(Self::failing_output())
        }

        fn run_with_timeout(
            &self,
            _program: &str,
            _args: &[&str],
            _cwd: &Path,
            _timeout: std::time::Duration,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            self.used_live_echo.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(Self::failing_output())
        }

        fn run_quiet(
            &self,
            _program: &str,
            _args: &[&str],
            _cwd: &Path,
            _timeout: std::time::Duration,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            Ok(Self::failing_output())
        }
    }

    /// Answers `git remote -v` with `origin` at the URL held, or with no remotes when it holds none.
    struct Origin(std::sync::Mutex<&'static str>);

    impl callisto_model::CommandRunner for Origin {
        fn run(
            &self,
            _program: &str,
            args: &[&str],
            _cwd: &Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            assert_eq!(args, ["remote", "-v"]);
            let url = *self.0.lock().unwrap();
            Ok(callisto_model::CommandOutput {
                exit_code: Some(0),
                stdout: if url.is_empty() {
                    String::new()
                } else {
                    format!("origin\t{url} (fetch)\norigin\t{url} (push)\n")
                },
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn recheck_reports_a_missing_origin_as_itself_and_moved_urls_as_stale() {
        let runner = Origin(std::sync::Mutex::new("https://github.com/example/release-fixture.git"));
        let root = Path::new(".");
        let expected = prepared_git_remote(root, &runner).unwrap();
        recheck_git_remote(root, &runner, &expected).unwrap();
        *runner.0.lock().unwrap() = "https://github.com/example/other.git";
        assert!(matches!(
            recheck_git_remote(root, &runner, &expected),
            Err(GraphError::ReleaseIntentStale { .. })
        ));
        *runner.0.lock().unwrap() = "";
        assert!(matches!(
            recheck_git_remote(root, &runner, &expected),
            Err(GraphError::UnsafeGitRemote { .. })
        ));
        *runner.0.lock().unwrap() = "https://token@github.com/example/release-fixture.git";
        assert!(matches!(
            optional_git_remote(root, &runner),
            Err(GraphError::UnsafeGitRemote { .. })
        ));
    }

    #[test]
    fn optional_git_remote_probes_quietly_when_origin_is_missing() {
        let runner = NoRemote::new();
        let result = optional_git_remote(Path::new("."), &runner);
        assert!(matches!(result, Ok(None)), "got: {result:?}");
        assert!(
            !runner.used_live_echo.load(std::sync::atomic::Ordering::SeqCst),
            "optional_git_remote must probe via run_quiet, not the live-echoing run_with_timeout"
        );
    }

    #[test]
    fn prepared_git_remote_probes_quietly_when_origin_is_missing() {
        let runner = NoRemote::new();
        let result = prepared_git_remote(Path::new("."), &runner);
        assert!(
            matches!(result, Err(GraphError::UnsafeGitRemote { .. })),
            "got: {result:?}"
        );
        assert!(
            !runner.used_live_echo.load(std::sync::atomic::Ordering::SeqCst),
            "prepared_git_remote must probe via run_quiet, not the live-echoing run_with_timeout"
        );
    }

    #[test]
    fn a_target_binds_its_own_configured_registry_key() {
        let (dir, runner) = super::super::tests::fixture();
        std::fs::write(
            dir.path().join("callisto.toml"),
            "[registries.crates-io]\nkind = \"cargo\"\nconfirm-timeout = \"3m\"\n\n[[package]]\nmatch = \"cargo/release-fixture\"\npublish-to = [\"crates-io\"]\n",
        )
        .unwrap();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let workspace = Workspace::load(dir.path().to_path_buf(), &locator, &runner).unwrap();
        let package = PackageId::parse("release-fixture").unwrap();
        let binding = prepared_registry_binding(&workspace, &PublishTarget::CratesIo, &package).unwrap();
        assert_eq!(binding.key.as_str(), "crates-io");
        assert_eq!(binding.endpoint, None);
        assert_eq!(binding.confirm_timeout, Some(std::time::Duration::from_secs(180)));
    }

    /// A cargo registry has no endpoint override: cargo takes its destination from `.cargo/config.toml`, so a `url`
    /// that nothing would route to is an error, not a silent pin.
    #[test]
    fn a_cargo_registry_url_is_rejected_not_ignored() {
        let (dir, runner) = super::super::tests::fixture();
        std::fs::write(
            dir.path().join("callisto.toml"),
            "[registries.crates-io]\nkind = \"cargo\"\nurl = \"https://registry.example.test/index\"\n",
        )
        .unwrap();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let error = Workspace::load(dir.path().to_path_buf(), &locator, &runner)
            .err()
            .expect("a cargo registry url must be rejected");
        assert!(
            matches!(&error, GraphError::Config(crate::error::ConfigError::CargoRegistryUrl { registry, .. }) if registry == "crates-io"),
            "{error}"
        );
    }

    /// A predeclared registry with no entry binds its own default.
    #[test]
    fn an_unconfigured_builtin_key_binds_itself() {
        let (dir, runner) = super::super::tests::fixture();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let workspace = Workspace::load(dir.path().to_path_buf(), &locator, &runner).unwrap();
        let package = PackageId::parse("release-fixture").unwrap();
        let binding = prepared_registry_binding(&workspace, &PublishTarget::Pypi { index: None }, &package).unwrap();
        assert_eq!(binding.key.as_str(), RegistryKey::PYPI);
        assert_eq!(binding.endpoint, None);
    }

    fn bind_pypi(config: &str, target: &PublishTarget) -> Result<PreparedRegistryBinding, GraphError> {
        let (dir, runner) = super::super::tests::fixture();
        std::fs::write(dir.path().join("callisto.toml"), config).unwrap();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let workspace = Workspace::load(dir.path().to_path_buf(), &locator, &runner).unwrap();
        let package = PackageId::parse("release-fixture").unwrap();
        prepared_registry_binding(&workspace, target, &package)
    }

    const PYPI_INDEX: &str = "[registries.pypi]\nkind = \"pypi\"\nurl = \"https://index.example.test/simple\"\n";
    const PYPI_SPLIT: &str = "[registries.pypi]\nkind = \"pypi\"\nurl = \"https://index.example.test/simple\"\npublish-url = \"https://upload.example.test/legacy/\"\n";

    #[test]
    fn a_pypi_registry_without_an_upload_route_uploads_to_its_index() {
        let binding = bind_pypi(PYPI_INDEX, &PublishTarget::Pypi { index: None }).unwrap();
        assert_eq!(binding.endpoint.as_deref(), Some("https://index.example.test/simple"));
        assert_eq!(binding.publish_endpoint, None);
        assert_eq!(binding.upload_endpoint(), binding.endpoint.as_deref());
        let digest = canonical_registry_binding("pypi", "https://index.example.test/simple")
            .unwrap()
            .digest();
        assert_eq!(binding.identity, digest, "an index-only binding keeps its digest");
    }

    #[test]
    fn a_pypi_registry_binds_its_index_and_its_upload_route_separately() {
        let binding = bind_pypi(PYPI_SPLIT, &PublishTarget::Pypi { index: None }).unwrap();
        assert_eq!(binding.endpoint.as_deref(), Some("https://index.example.test/simple"));
        assert_eq!(binding.upload_endpoint(), Some("https://upload.example.test/legacy/"));
        let index_only = bind_pypi(PYPI_INDEX, &PublishTarget::Pypi { index: None }).unwrap();
        assert_ne!(
            binding.identity, index_only.identity,
            "the upload route is part of the binding"
        );
        let moved = PYPI_SPLIT.replace("upload.example.test", "other.example.test");
        let moved = bind_pypi(&moved, &PublishTarget::Pypi { index: None }).unwrap();
        assert_ne!(binding.identity, moved.identity);
    }

    #[test]
    fn an_upload_route_without_an_index_leaves_reads_on_the_default() {
        let config = "[registries.pypi]\nkind = \"pypi\"\npublish-url = \"https://upload.example.test/legacy/\"\n";
        let binding = bind_pypi(config, &PublishTarget::Pypi { index: None }).unwrap();
        assert_eq!(binding.endpoint, None);
        assert_eq!(binding.upload_endpoint(), Some("https://upload.example.test/legacy/"));
    }

    #[test]
    fn an_explicit_index_on_a_package_names_both_routes() {
        let target = PublishTarget::Pypi {
            index: Some("https://own.example.test/simple".to_owned()),
        };
        let binding = bind_pypi(PYPI_SPLIT, &target).unwrap();
        assert_eq!(binding.endpoint.as_deref(), Some("https://own.example.test/simple"));
        assert_eq!(binding.publish_endpoint, None);
        assert_eq!(binding.upload_endpoint(), Some("https://own.example.test/simple"));
    }

    #[test]
    fn an_unsafe_upload_route_is_refused_like_an_unsafe_index() {
        for unsafe_url in [
            "http://upload.example.test/legacy/",
            "https://user:pass@upload.example.test/legacy/",
            "https://upload.example.test/legacy/?token=1",
        ] {
            let config = format!("[registries.pypi]\nkind = \"pypi\"\npublish-url = \"{unsafe_url}\"\n");
            let error = bind_pypi(&config, &PublishTarget::Pypi { index: None }).unwrap_err();
            assert!(
                matches!(error, GraphError::UnsafeRegistryBinding { .. }),
                "{unsafe_url}: {error:?}"
            );
        }
    }

    fn npm_workspace(registries: &str) -> (tempfile::TempDir, super::super::tests::RealGitRunner) {
        let (dir, runner) = super::super::tests::fixture();
        std::fs::write(dir.path().join("callisto.toml"), registries).unwrap();
        (dir, runner)
    }

    fn npm_binding(registries: &str, url: &str) -> Result<PreparedRegistryBinding, GraphError> {
        let (dir, runner) = npm_workspace(registries);
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let workspace = Workspace::load(dir.path().to_path_buf(), &locator, &runner).unwrap();
        let target = PublishTarget::Npm {
            registry: Some(url.to_owned()),
            access: None,
        };
        prepared_registry_binding(&workspace, &target, &PackageId::parse("lib").unwrap())
    }

    const CORP: &str = "[registries.corp]\nkind = \"npm\"\nurl = \"https://npm.corp.example/\"\n";

    /// Only an npm override matching a configured npm registry is trusted.
    #[test]
    fn npm_override_must_match_a_configured_npm_registry() {
        assert!(npm_binding(CORP, "https://npm.corp.example/").is_ok());
        for untrusted in ["https://npm.evil.example/", "https://npm.corp.example/other/"] {
            assert!(
                matches!(
                    npm_binding(CORP, untrusted),
                    Err(GraphError::UntrustedNpmRegistry { .. })
                ),
                "{untrusted}"
            );
        }
        let cargo_kind = "[registries.corp]\nkind = \"cargo\"\n";
        assert!(matches!(
            npm_binding(cargo_kind, "https://npm.corp.example/"),
            Err(GraphError::UntrustedNpmRegistry { .. })
        ));
    }

    /// The scheme check runs first, so a cleartext approved host is unsafe, not untrusted.
    #[test]
    fn non_https_npm_override_is_unsafe_before_host_matching() {
        assert!(matches!(
            npm_binding(CORP, "http://npm.corp.example/"),
            Err(GraphError::UnsafeRegistryBinding { .. })
        ));
    }
}
