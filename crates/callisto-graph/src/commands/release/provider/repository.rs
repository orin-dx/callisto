//! Whether a version already on a registry names this repository, so adopting it can be flagged when it does not.

/// `host/owner/repo` for a repository URL in any form a registry records, lowercased and without a `.git` suffix.
///
/// Reads `https://`, `git+https://`, `ssh://`, `git://`, `git@host:owner/repo`, `github:owner/repo` and npm's bare
/// `owner/repo` shorthand. Only the first two path segments count, so a `/tree/main` or `#readme` tail is ignored.
pub(crate) fn repository_identity(url: &str) -> Option<String> {
    let url = url.trim();
    let url = url.strip_prefix("git+").unwrap_or(url);
    let (host, path) = if let Some(rest) = url.strip_prefix("github:") {
        ("github.com", rest)
    } else if let Some((_, rest)) = url.split_once("://") {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, after)| after);
        rest.split_once('/')?
    } else if let Some((front, path)) = url.split_once(':') {
        (front.rsplit_once('@').map_or(front, |(_, host)| host), path)
    } else if url.matches('/').count() == 1 && !url.contains('.') {
        ("github.com", url)
    } else {
        return None;
    };
    let host = host.split(':').next().unwrap_or(host);
    let path = path.split(['#', '?']).next().unwrap_or(path);
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let owner = segments.next()?;
    let repo = segments.next()?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    (!host.is_empty() && !repo.is_empty()).then(|| format!("{host}/{owner}/{repo}").to_ascii_lowercase())
}

/// The warning for adopting a published version whose registry entry names another repository, if it does.
///
/// Silent when the registry names no repository, when either side cannot be read as a repository, and when they match:
/// a missing or unreadable value proves nothing about who owns the name.
pub(crate) fn foreign_repository_warning(
    package: &str,
    version: &str,
    registry_repository: Option<&str>,
    origin: &str,
) -> Option<String> {
    let registry = repository_identity(registry_repository?)?;
    let ours = repository_identity(origin)?;
    (registry != ours).then(|| {
        format!(
            "warning: {package} {version} is already published, and its registry entry names {registry}, not this \
             repository ({ours}). Callisto adopts the published version and tags this commit; check that the name is \
             yours."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_form_a_registry_records_reads_as_host_owner_repo() {
        for url in [
            "https://github.com/Orin-DX/Callisto",
            "https://github.com/orin-dx/callisto.git",
            "git+https://github.com/orin-dx/callisto.git",
            "git://github.com/orin-dx/callisto.git",
            "ssh://git@github.com/orin-dx/callisto.git",
            "git@github.com:orin-dx/callisto.git",
            "github:orin-dx/callisto",
            "orin-dx/callisto",
            "https://github.com/orin-dx/callisto/tree/main/crates/x#readme",
            "https://token@github.com/orin-dx/callisto/",
            "  https://github.com/orin-dx/callisto  ",
        ] {
            assert_eq!(
                repository_identity(url).as_deref(),
                Some("github.com/orin-dx/callisto"),
                "{url}"
            );
        }
    }

    #[test]
    fn other_hosts_keep_their_host_and_nonsense_reads_as_nothing() {
        assert_eq!(
            repository_identity("https://gitlab.com/group/project.git").as_deref(),
            Some("gitlab.com/group/project")
        );
        assert_eq!(
            repository_identity("ssh://git@git.example.com:2222/team/app.git").as_deref(),
            Some("git.example.com/team/app")
        );
        for url in [
            "",
            "   ",
            "https://github.com",
            "https://github.com/onlyowner",
            "not a url",
            "a/b/c",
        ] {
            assert_eq!(repository_identity(url), None, "{url:?}");
        }
    }

    #[test]
    fn a_different_repository_is_warned_about_and_the_same_one_is_not() {
        let origin = "https://github.com/orin-dx/callisto.git";
        let warning =
            foreign_repository_warning("cargo/itoa", "1.0.0", Some("https://github.com/dtolnay/itoa"), origin)
                .expect("a foreign repository warns");
        assert!(warning.contains("cargo/itoa 1.0.0"), "{warning}");
        assert!(warning.contains("github.com/dtolnay/itoa"), "{warning}");
        assert!(warning.contains("github.com/orin-dx/callisto"), "{warning}");
        assert_eq!(
            foreign_repository_warning(
                "cargo/itoa",
                "1.0.0",
                Some("git+https://github.com/Orin-DX/callisto"),
                origin
            ),
            None
        );
    }

    #[test]
    fn a_missing_or_unreadable_repository_is_never_a_warning() {
        let origin = "https://github.com/orin-dx/callisto";
        assert_eq!(foreign_repository_warning("npm/x", "1.0.0", None, origin), None);
        assert_eq!(
            foreign_repository_warning("npm/x", "1.0.0", Some("nonsense"), origin),
            None
        );
        assert_eq!(
            foreign_repository_warning("npm/x", "1.0.0", Some("https://github.com/a/b"), "nonsense"),
            None
        );
    }
}
