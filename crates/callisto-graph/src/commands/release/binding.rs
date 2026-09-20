//! Canonical, credential-free registry and Git destinations.
//!
//! Every release effect is routed through a binding produced here, so no
//! caller-supplied endpoint or credential can reach a provider.

use std::path::Path;

use callisto_model::{
    CanonicalTranscript, CommandRunner, GitHubRepository, GitHubRepositoryParseError, PublishTarget,
    RegistryBindingDigest, RegistryKey, SemanticInputDigest,
};

use crate::config::ReleaseProfileConfig;
use crate::registry_endpoint::{
    builtin_registry_url, canonical_registry_url, url_parse_error_reason, RegistryBindingV1,
};
use crate::{DependencyResolver, GraphError, Workspace};

use super::provider::policy::timeouts;
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

pub(crate) fn prepared_registry_binding<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    target: &PublishTarget,
    profile: Option<&ReleaseProfileConfig>,
) -> Result<PreparedRegistryBinding, GraphError> {
    let logical_key = target.registry_key().ok_or_else(|| GraphError::ReleaseInvariant {
        detail: format!("publish target `{}` has no registry key", target.config_str()),
    })?;
    let key = profile
        .map(|profile| {
            profile
                .registry_routes
                .get(&logical_key)
                .cloned()
                .ok_or_else(|| GraphError::ReleaseInvariant {
                    detail: format!(
                        "release profile has no registry route for logical registry `{}`",
                        logical_key.as_str()
                    ),
                })
        })
        .transpose()?
        .unwrap_or_else(|| logical_key.clone());
    // Built-in keys (pypi, nuget) need no [registries] entry; an unknown routed key still fails.
    let builtin_unconfigured = builtin_registry_url(key.as_str()).is_some();
    let configured_registry = match workspace.config.registries.get(&key) {
        Some(registry) => Some(registry),
        None if builtin_unconfigured => None,
        None => {
            return Err(GraphError::ReleaseInvariant {
                detail: format!(
                    "release profile routes `{}` to unknown registry `{}`",
                    logical_key.as_str(),
                    key.as_str()
                ),
            })
        }
    };
    if configured_registry.is_some_and(|registry| Some(registry.kind) != target.ecosystem()) {
        return Err(GraphError::ReleaseInvariant {
            detail: format!(
                "release profile routes `{}` to registry `{}` with incompatible ecosystem",
                logical_key.as_str(),
                key.as_str()
            ),
        });
    }
    let explicit = (key == logical_key).then(|| target.registry_override()).flatten();
    let configured = configured_registry.and_then(|registry| registry.url.as_deref());
    let binding = match explicit.or(configured) {
        Some(raw) => canonical_registry_binding(key.as_str(), raw)?,
        None => {
            return Ok(PreparedRegistryBinding {
                key: key.clone(),
                endpoint: None,
                identity: RegistryBindingDigest::from_normalized_binding(key.as_str().as_bytes()),
            })
        }
    };
    Ok(PreparedRegistryBinding {
        key,
        endpoint: Some(binding.endpoint()),
        identity: binding.digest(),
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
        "git",
        &["remote", "get-url", "--push", "origin"],
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
    fn profile_registry_route_replaces_the_logical_crates_io_destination() {
        let (dir, runner) = super::super::tests::fixture();
        std::fs::write(
            dir.path().join("callisto.toml"),
            "[release]\nproduct-package = \"cargo/release-fixture\"\nartifact-targets = [\n  \"aarch64-apple-darwin\",\n  \"x86_64-unknown-linux-gnu\",\n  \"x86_64-unknown-linux-musl\",\n  \"wasm32-wasip1\",\n]\n\n[release.profiles.rehearsal]\nforge-repository = \"example/rehearsal\"\nregistry-routes = { cratesIo = \"rehearsal-cargo\" }\n\n[registries.rehearsal-cargo]\nkind = \"cargo\"\nurl = \"https://registry.example.test/index\"\n\n[[package]]\nmatch = \"release-fixture\"\npublish-to = [\"crates-io\"]\n",
        )
        .unwrap();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let workspace = Workspace::load(dir.path().to_path_buf(), &locator, &runner).unwrap();
        let profile = workspace
            .config
            .product_release
            .as_ref()
            .and_then(|release| release.profile(&callisto_model::ReleaseProfileId::parse("rehearsal").unwrap()))
            .unwrap();

        let binding = prepared_registry_binding(&workspace, &PublishTarget::CratesIo, Some(profile)).unwrap();
        assert_eq!(binding.key.as_str(), "rehearsal-cargo");
        assert_eq!(binding.endpoint.as_deref(), Some("https://registry.example.test/index"));
    }
}
