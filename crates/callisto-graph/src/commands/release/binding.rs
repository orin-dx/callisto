//! Canonical, credential-free registry and Git destinations.
//!
//! Every release effect is routed through a binding produced here, so no
//! caller-supplied endpoint or credential can reach a provider.

use std::path::Path;

use callisto_model::{
    CanonicalTranscript, CommandRunner, Ecosystem, GitHubRepository, GitHubRepositoryParseError, PackageId,
    PublishTarget, RegistryBindingDigest, RegistryKey, SemanticInputDigest,
};

use crate::registry_endpoint::{
    builtin_registry_url, canonical_registry_url, url_parse_error_reason, RegistryBindingV1,
};
use crate::{DependencyResolver, GraphError, Workspace};

use super::provider::policy::{programs, timeouts};
use super::StaleReason;

/// Credential-free, canonical registry routing. `endpoint` is populated only
/// after parsing rejects userinfo, query, and fragments, so a later executor
/// cannot recover credentials from this capability.
#[derive(Debug)]
pub(crate) struct PreparedRegistryBinding {
    pub(crate) key: RegistryKey,
    pub(crate) endpoint: Option<String>,
    pub(crate) identity: RegistryBindingDigest,
}

/// Immutable, credential-free destination for Git effects. Its canonical
/// digest is included in each tag-producing package fingerprint; the URL is
/// retained only in the validated, non-serializable capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedGitRemote {
    pub(crate) endpoint: String,
    pub(crate) identity: SemanticInputDigest,
    pub(crate) github_repository: Option<GitHubRepository>,
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
    // Built-in keys (pypi, nuget) need no [registries] entry; an unknown key still fails.
    let builtin_unconfigured = builtin_registry_url(key.as_str()).is_some();
    let configured_registry = match workspace.config.registries.get(&key) {
        Some(registry) => Some(registry),
        None if builtin_unconfigured => None,
        None => {
            return Err(GraphError::ReleaseInvariant {
                detail: format!("unknown registry `{}`", key.as_str()),
            })
        }
    };
    if let Some(registry) = configured_registry.filter(|registry| Some(registry.kind) != target.ecosystem()) {
        return Err(GraphError::ReleaseInvariant {
            detail: format!(
                "registry `{}` is configured for ecosystem `{}`, not `{}`",
                key.as_str(),
                registry.kind.prefix(),
                target.ecosystem().map_or("none", |ecosystem| ecosystem.prefix())
            ),
        });
    }
    let explicit = target.registry_override();
    let configured = configured_registry.and_then(|registry| registry.url.as_deref());
    let raw = explicit.or(configured);
    let Some(raw) = raw else {
        return Ok(PreparedRegistryBinding {
            key: key.clone(),
            endpoint: None,
            identity: RegistryBindingDigest::from_normalized_binding(key.as_str().as_bytes()),
        });
    };
    let binding = canonical_registry_binding(key.as_str(), raw)?;
    if explicit.is_some()
        && matches!(target, PublishTarget::Npm { .. })
        && !is_approved_npm_registry(workspace, &binding)
    {
        return Err(GraphError::UntrustedNpmRegistry {
            package: package.clone(),
            url: raw.to_owned(),
        });
    }
    Ok(PreparedRegistryBinding {
        key,
        endpoint: Some(binding.endpoint()),
        identity: binding.digest(),
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

/// Maps a [`GitHubRepositoryParseError`] to a specific, static diagnostic
/// reason for [`GraphError::UnsafeGitRemote`], rather than discarding it
/// behind one generic message for every distinct parse failure.
fn github_repository_parse_error_reason(error: &GitHubRepositoryParseError) -> &'static str {
    match error {
        GitHubRepositoryParseError::MissingSeparator { .. } => "GitHub remote path must be in owner/repo form",
        GitHubRepositoryParseError::TooManyParts { .. } => "GitHub remote path has more than one `/`",
        GitHubRepositoryParseError::InvalidOwner { .. } => "GitHub owner name is invalid",
        GitHubRepositoryParseError::InvalidRepo { .. } => "GitHub repository name is invalid",
        GitHubRepositoryParseError::DotComponent { .. } => "GitHub owner or repository is a dot component",
        #[allow(unreachable_patterns)]
        _ => "GitHub owner or repository name is invalid",
    }
}

pub(crate) fn prepared_git_remote(root: &Path, runner: &dyn CommandRunner) -> Result<PreparedGitRemote, GraphError> {
    let output = runner.run_with_timeout(
        programs::GIT,
        &["remote", "get-url", "--push", programs::GIT_REMOTE],
        root,
        timeouts::LOCAL_GIT,
    )?;
    if output.exit_code != Some(0) {
        return Err(GraphError::UnsafeGitRemote {
            reason: "origin must have a push URL",
        });
    }
    canonical_git_remote(output.stdout.trim())
}

/// `origin`'s canonical push URL, or `None` when `origin` has no push URL.
pub(crate) fn optional_git_remote(
    root: &Path,
    runner: &dyn CommandRunner,
) -> Result<Option<PreparedGitRemote>, GraphError> {
    let output = runner.run_with_timeout(
        programs::GIT,
        &["remote", "get-url", "--push", programs::GIT_REMOTE],
        root,
        timeouts::LOCAL_GIT,
    )?;
    if output.exit_code != Some(0) {
        return Ok(None);
    }
    canonical_git_remote(output.stdout.trim()).map(Some)
}

/// Re-reads `origin`'s push URL and refuses unless it is still exactly the
/// remote the intent was validated against.
pub(crate) fn recheck_git_remote(
    root: &Path,
    runner: &dyn CommandRunner,
    expected: &PreparedGitRemote,
) -> Result<(), GraphError> {
    if prepared_git_remote(root, runner)? != *expected {
        return Err(GraphError::ReleaseIntentStale {
            reason: StaleReason::git_remote_changed(),
        });
    }
    Ok(())
}

pub(crate) fn canonical_git_remote(raw: &str) -> Result<PreparedGitRemote, GraphError> {
    let ssh_url = if raw.starts_with("git@") && !raw.contains("//") {
        let (host, path) =
            raw.strip_prefix("git@")
                .and_then(|value| value.split_once(':'))
                .ok_or(GraphError::UnsafeGitRemote {
                    reason: "invalid SSH remote syntax",
                })?;
        format!("ssh://git@{host}/{path}")
    } else {
        raw.to_string()
    };
    let parsed = url::Url::parse(&ssh_url).map_err(|error| GraphError::UnsafeGitRemote {
        reason: url_parse_error_reason(error),
    })?;
    if parsed.cannot_be_a_base() || parsed.host_str().is_none() {
        return Err(GraphError::UnsafeGitRemote {
            reason: "URL must have an authority",
        });
    }
    if !matches!(parsed.scheme(), "https" | "ssh") {
        return Err(GraphError::UnsafeGitRemote {
            reason: "URL scheme must be HTTPS or SSH",
        });
    }
    if parsed.password().is_some() || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(GraphError::UnsafeGitRemote {
            reason: "credentials, query strings, and fragments are forbidden",
        });
    }
    if parsed.scheme() == "https" && !parsed.username().is_empty() {
        return Err(GraphError::UnsafeGitRemote {
            reason: "HTTPS userinfo is forbidden",
        });
    }
    if parsed.scheme() == "ssh" && parsed.username() != "git" {
        return Err(GraphError::UnsafeGitRemote {
            reason: "SSH remotes must use the git account",
        });
    }
    let host = parsed.host_str().expect("validated authority").to_ascii_lowercase();
    let mut components = parsed.path_segments().ok_or(GraphError::UnsafeGitRemote {
        reason: "remote path is invalid",
    })?;
    let owner = components
        .next()
        .filter(|value| !value.is_empty())
        .ok_or(GraphError::UnsafeGitRemote {
            reason: "remote path must name an owner and repository",
        })?;
    let repository = components
        .next()
        .filter(|value| !value.is_empty())
        .ok_or(GraphError::UnsafeGitRemote {
            reason: "remote path must name an owner and repository",
        })?;
    if components.next().is_some() || owner == "." || owner == ".." || repository == "." || repository == ".." {
        return Err(GraphError::UnsafeGitRemote {
            reason: "remote path must contain exactly an owner and repository",
        });
    }
    let repository = repository.strip_suffix(".git").unwrap_or(repository);
    if repository.is_empty() {
        return Err(GraphError::UnsafeGitRemote {
            reason: "repository name is empty",
        });
    }
    let port = parsed.port_or_known_default();
    let port_text = match (parsed.scheme(), port) {
        ("https", Some(443)) | ("ssh", Some(22)) | (_, None) => String::new(),
        (_, Some(port)) => format!(":{port}"),
    };
    let endpoint = match parsed.scheme() {
        "https" => format!("https://{host}{port_text}/{owner}/{repository}.git"),
        "ssh" => format!("ssh://git@{host}{port_text}/{owner}/{repository}.git"),
        _ => unreachable!("scheme checked above"),
    };
    let mut transcript = CanonicalTranscript::semantic_input_v1();
    transcript.push_str("gitRemote.scheme", parsed.scheme());
    transcript.push_str("gitRemote.host", &host);
    transcript.push_str(
        "gitRemote.port",
        &port.map_or_else(String::new, |port| port.to_string()),
    );
    transcript.push_str("gitRemote.owner", owner);
    transcript.push_str("gitRemote.repository", repository);
    let github_repository = if host == "github.com" {
        Some(
            GitHubRepository::parse(&format!("{owner}/{repository}")).map_err(|error| GraphError::UnsafeGitRemote {
                reason: github_repository_parse_error_reason(&error),
            })?,
        )
    } else {
        None
    };
    Ok(PreparedGitRemote {
        endpoint,
        identity: SemanticInputDigest::from_transcript(&transcript),
        github_repository,
    })
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

    struct RemoteUrl(&'static str);
    impl callisto_model::CommandRunner for RemoteUrl {
        fn run(
            &self,
            _program: &str,
            _args: &[&str],
            _cwd: &Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            Ok(callisto_model::CommandOutput {
                exit_code: Some(if self.0.is_empty() { 2 } else { 0 }),
                stdout: self.0.to_owned(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn recheck_reports_a_unreadable_remote_as_itself_and_a_moved_remote_as_stale() {
        let expected = canonical_git_remote("https://github.com/example/release-fixture.git").unwrap();
        let root = Path::new(".");
        recheck_git_remote(
            root,
            &RemoteUrl("https://github.com/example/release-fixture.git"),
            &expected,
        )
        .unwrap();
        assert!(matches!(
            recheck_git_remote(root, &RemoteUrl(""), &expected),
            Err(GraphError::UnsafeGitRemote { .. })
        ));
        assert!(matches!(
            recheck_git_remote(root, &RemoteUrl("https://github.com/example/other.git"), &expected),
            Err(GraphError::ReleaseIntentStale { .. })
        ));
    }

    #[test]
    fn git_remote_binding_normalizes_credential_free_ssh_and_rejects_credentialed_https() {
        let ssh = canonical_git_remote("git@GitHub.com:example/release-fixture.git").unwrap();
        assert_eq!(ssh.endpoint, "ssh://git@github.com/example/release-fixture.git");
        assert_eq!(
            ssh.github_repository.as_ref().map(GitHubRepository::as_slug),
            Some("example/release-fixture".to_string())
        );
        assert!(canonical_git_remote("https://token@github.com/example/release-fixture.git").is_err());
    }

    /// AC-007
    #[test]
    fn a_target_binds_its_own_configured_registry_key() {
        let (dir, runner) = super::super::tests::fixture();
        std::fs::write(
            dir.path().join("callisto.toml"),
            "[registries.cratesIo]\nkind = \"cargo\"\nurl = \"https://registry.example.test/index\"\n\n[[package]]\nmatch = \"release-fixture\"\npublish-to = [\"crates-io\"]\n",
        )
        .unwrap();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let workspace = Workspace::load(dir.path().to_path_buf(), &locator, &runner).unwrap();
        let package = PackageId::parse("release-fixture").unwrap();
        let binding = prepared_registry_binding(&workspace, &PublishTarget::CratesIo, &package).unwrap();
        assert_eq!(binding.key.as_str(), "cratesIo");
        assert_eq!(binding.endpoint.as_deref(), Some("https://registry.example.test/index"));
    }

    /// AC-007: an unconfigured built-in key binds its own default.
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

    /// AC-008
    #[test]
    fn registry_errors_name_the_key_and_ecosystem_but_no_profile() {
        let (dir, runner) = super::super::tests::fixture();
        std::fs::write(
            dir.path().join("callisto.toml"),
            "[registries.cratesIo]\nkind = \"npm\"\n\n[[package]]\nmatch = \"release-fixture\"\npublish-to = [\"crates-io\"]\n",
        )
        .unwrap();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let workspace = Workspace::load(dir.path().to_path_buf(), &locator, &runner).unwrap();
        let package = PackageId::parse("release-fixture").unwrap();
        let Err(GraphError::ReleaseInvariant { detail }) =
            prepared_registry_binding(&workspace, &PublishTarget::CratesIo, &package)
        else {
            panic!("an npm-kind registry must not bind a cargo target");
        };
        assert!(detail.contains("`cratesIo`") && detail.contains("`npm`"), "{detail}");
        assert!(!detail.contains("profile"), "{detail}");
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

    /// AC-4: only an npm override matching a configured npm registry is trusted.
    #[test]
    fn ac4_npm_override_must_match_a_configured_npm_registry() {
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
        let cargo_kind = "[registries.corp]\nkind = \"cargo\"\nurl = \"https://npm.corp.example/\"\n";
        assert!(matches!(
            npm_binding(cargo_kind, "https://npm.corp.example/"),
            Err(GraphError::UntrustedNpmRegistry { .. })
        ));
    }

    /// AC-5: the scheme check runs first, so a cleartext approved host is unsafe, not untrusted.
    #[test]
    fn ac5_non_https_npm_override_is_unsafe_before_host_matching() {
        assert!(matches!(
            npm_binding(CORP, "http://npm.corp.example/"),
            Err(GraphError::UnsafeRegistryBinding { .. })
        ));
    }
}
