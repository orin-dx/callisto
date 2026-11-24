//! Canonical, credential-free registry and Git destinations.
//!
//! Every release effect is routed through a binding produced here, so no
//! caller-supplied endpoint or credential can reach a provider.

use std::path::Path;

use callisto_model::{
    CanonicalTranscript, CommandRunner, Ecosystem, GitHubRepository, GitHubRepositoryParseError, PackageId,
    PublishTarget, RegistryBindingDigest, RegistryKey, SemanticInputDigest,
};

use crate::error::{OriginRefusal, ShownRegistry};
use crate::registry_endpoint::{
    canonical_registry_url, split_routes_digest, url_parse_error_reason, RegistryBindingV1,
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
            registry: ShownRegistry::new(raw),
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

/// Maps a [`GitHubRepositoryParseError`] to a specific, static reason `origin` is not hosted, shown in
/// [`GraphError::UnsafeGitRemote`], rather than discarding it behind one generic message.
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

/// `origin`'s push URL, read and parsed once: every release, preview and `init` decision reads this value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// No `origin`, or one without a push URL.
    Missing,
    /// A remote tags and the forge repository can bind to.
    Hosted(PreparedGitRemote),
    /// Anything else, such as a local path: only the fixed reason is kept, never the text, so no diagnostic can echo it.
    NotHosted(&'static str),
}

impl Origin {
    pub(crate) fn read(root: &Path, runner: &dyn CommandRunner) -> Result<Self, GraphError> {
        Ok(Self::parse_push_url(Self::read_push_url(root, runner)?.as_deref()))
    }

    /// `origin`'s push URL, or `None` when there is no such remote.
    pub(crate) fn read_push_url(root: &Path, runner: &dyn CommandRunner) -> Result<Option<String>, GraphError> {
        // A missing `origin` is an answer, not a failure: probe quietly so git's "No such remote" never prints.
        let output = runner.run_quiet(
            programs::GIT,
            &["remote", "get-url", "--push", programs::GIT_REMOTE],
            root,
            timeouts::LOCAL_GIT,
        )?;
        Ok((output.exit_code == Some(0)).then(|| output.stdout.trim().to_owned()))
    }

    pub(crate) fn parse_push_url(raw: Option<&str>) -> Self {
        raw.map_or(Self::Missing, Self::parse)
    }

    fn parse(raw: &str) -> Self {
        match canonical_git_remote(raw) {
            Ok(remote) => Self::Hosted(remote),
            Err(reason) => Self::NotHosted(reason),
        }
    }

    /// A real release: only a hosted remote.
    pub(crate) fn required(self) -> Result<PreparedGitRemote, GraphError> {
        self.optional()?.ok_or(GraphError::UnsafeGitRemote {
            refusal: OriginRefusal::NoPushUrl,
        })
    }

    /// A real run that works without a remote: none, or a hosted one.
    pub(crate) fn optional(self) -> Result<Option<PreparedGitRemote>, GraphError> {
        match self {
            Self::Missing => Ok(None),
            Self::Hosted(remote) => Ok(Some(remote)),
            Self::NotHosted(reason) => Err(GraphError::UnsafeGitRemote {
                refusal: OriginRefusal::NotHosted { reason },
            }),
        }
    }

    /// A preview: a remote that is not hosted reads exactly like no remote.
    pub(crate) fn preview(self) -> Option<PreparedGitRemote> {
        match self {
            Self::Hosted(remote) => Some(remote),
            Self::Missing | Self::NotHosted(_) => None,
        }
    }
}

/// Re-reads `origin`'s push URL and refuses unless it is still exactly the
/// remote the intent was validated against.
pub(crate) fn recheck_git_remote(
    root: &Path,
    runner: &dyn CommandRunner,
    expected: &PreparedGitRemote,
) -> Result<(), GraphError> {
    if Origin::read(root, runner)?.required()? != *expected {
        return Err(GraphError::ReleaseIntentStale {
            reason: StaleReason::git_remote_changed(),
        });
    }
    Ok(())
}

/// The strict parser behind [`Origin::Hosted`]: `https://` or `ssh://git@` with an owner and repository, or
/// `git@host:owner/repo`.
pub(crate) fn canonical_git_remote(raw: &str) -> Result<PreparedGitRemote, &'static str> {
    let ssh_url = if raw.starts_with("git@") && !raw.contains("//") {
        let (host, path) = raw
            .strip_prefix("git@")
            .and_then(|value| value.split_once(':'))
            .ok_or("invalid SSH remote syntax")?;
        format!("ssh://git@{host}/{path}")
    } else {
        raw.to_string()
    };
    let parsed = url::Url::parse(&ssh_url).map_err(url_parse_error_reason)?;
    if parsed.cannot_be_a_base() || parsed.host_str().is_none() {
        return Err("URL must have an authority");
    }
    if !matches!(parsed.scheme(), "https" | "ssh") {
        return Err("URL scheme must be HTTPS or SSH");
    }
    if parsed.password().is_some() || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("credentials, query strings, and fragments are forbidden");
    }
    if parsed.scheme() == "https" && !parsed.username().is_empty() {
        return Err("HTTPS userinfo is forbidden");
    }
    if parsed.scheme() == "ssh" && parsed.username() != "git" {
        return Err("SSH remotes must use the git account");
    }
    let host = parsed.host_str().expect("validated authority").to_ascii_lowercase();
    // One trailing slash is the same repository, as `git` itself treats it.
    let mut segments: Vec<&str> = parsed.path_segments().ok_or("remote path is invalid")?.collect();
    if segments.len() > 1 && segments.last() == Some(&"") {
        segments.pop();
    }
    let mut components = segments.into_iter();
    let owner = components
        .next()
        .filter(|value| !value.is_empty())
        .ok_or("remote path must name an owner and repository")?;
    let repository = components
        .next()
        .filter(|value| !value.is_empty())
        .ok_or("remote path must name an owner and repository")?;
    if components.next().is_some() || owner == "." || owner == ".." || repository == "." || repository == ".." {
        return Err("remote path must contain exactly an owner and repository");
    }
    let repository = repository.strip_suffix(".git").unwrap_or(repository);
    if repository.is_empty() {
        return Err("repository name is empty");
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
            GitHubRepository::parse(&format!("{owner}/{repository}"))
                .map_err(|error| github_repository_parse_error_reason(&error))?,
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

    /// Answers `git remote get-url` with a failure (no `origin`) and records
    /// whether the probe went through the live-echoing `run_with_timeout` path
    /// or the quiet one -- `Origin::read` must never let git's own
    /// "No such remote" reach the terminal for this expected, common case.
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
                exit_code: Some(2),
                stdout: String::new(),
                stderr: "error: No such remote 'origin'".to_string(),
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

    #[test]
    fn origin_probes_quietly_when_origin_is_missing() {
        let runner = NoRemote::new();
        let origin = Origin::read(Path::new("."), &runner).unwrap();
        assert_eq!(origin, Origin::Missing);
        assert!(matches!(origin.required(), Err(GraphError::UnsafeGitRemote { .. })));
        assert!(
            !runner.used_live_echo.load(std::sync::atomic::Ordering::SeqCst),
            "Origin::read must probe via run_quiet, not the live-echoing run_with_timeout"
        );
    }

    #[test]
    fn a_refused_origin_never_echoes_the_remote() {
        for raw in [
            "https://user:pw@github.com/o/r.git?token=t#f",
            "ssh://deploy@example.test/o/r.git",
            "http://github.com/o/r.git",
            "remotes/origin.git",
            "https://user:secret@host:99999/x.git",
        ] {
            let error = Origin::parse(raw).optional().unwrap_err();
            assert!(
                error.to_string().contains("`origin` is not a hosted remote: "),
                "{error}"
            );
            assert!(error.to_string().contains("(use an https://"), "{error}");
            for text in [error.to_string(), format!("{error:?}")] {
                for leak in ["secret", "pw", "deploy", "example.test", "github.com", "remotes/origin"] {
                    assert!(!text.contains(leak), "{text}");
                }
            }
        }
    }

    #[test]
    fn an_untrusted_registry_shows_only_its_scheme_and_host_unless_it_carries_credentials() {
        let shown = |raw: &str| ShownRegistry::new(raw).to_string();
        assert_eq!(shown("https://Registry.Example.com"), "`https://registry.example.com`");
        assert_eq!(
            shown("https://registry.example.com/private/team-path"),
            "`https://registry.example.com`"
        );
        for secret in [
            "https://u:pw@r.example.test/",
            "https://r.example.test/?token=pw",
            "not a url pw",
        ] {
            assert!(!shown(secret).contains("pw"), "{secret}");
        }
    }

    #[test]
    fn a_refused_origin_says_why_in_fixed_words() {
        for (raw, reason) in [
            ("https://token@github.com/o/r.git", "HTTPS userinfo is forbidden"),
            ("http://github.com/o/r.git", "URL scheme must be HTTPS or SSH"),
            ("remotes/origin.git", "URL has no scheme"),
        ] {
            let error = Origin::parse(raw).optional().unwrap_err();
            assert!(error.to_string().contains(reason), "{raw}: {error}");
        }
    }

    #[test]
    fn a_trailing_slash_and_an_ssh_port_are_accepted() {
        let plain = canonical_git_remote("https://github.com/o/r.git").unwrap();
        let slash = canonical_git_remote("https://github.com/o/r/").unwrap();
        assert_eq!(slash.endpoint, plain.endpoint);
        assert_eq!(slash.identity, plain.identity);
        let ported = canonical_git_remote("ssh://git@git.example.test:2222/o/r.git").unwrap();
        assert_eq!(ported.endpoint, "ssh://git@git.example.test:2222/o/r.git");
        assert!(canonical_git_remote("https://github.com/o/").is_err());
        assert!(canonical_git_remote("git@git.example.test:2222/o/r").is_err());
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
