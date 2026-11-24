//! Remotes. Git is always given a remote's name, so it resolves the URLs (`insteadOf`, `pushInsteadOf`, `pushurl`)
//! itself, once per command. Callisto keeps only a fingerprint of what git reports, and a coarse classification of the
//! push URL for forge features; the URLs themselves can never be printed.

use std::collections::BTreeMap;
use std::time::Duration;

use super::access::GitAccess;
use super::{GitStdout, VcsError};
use crate::{
    BranchName, CanonicalTranscript, CommandOutput, CommitSha, GitHubRepository, SemanticInputDigest, TagName,
};

/// A configured remote, as git resolved it when it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRemote {
    name: String,
    urls: RemoteUrls,
}

/// The URLs exactly as git resolved them. They may hold credentials, so nothing prints them.
#[derive(Clone, PartialEq, Eq)]
struct RemoteUrls {
    fetch: Vec<String>,
    push: Vec<String>,
}

impl std::fmt::Debug for RemoteUrls {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RemoteUrls(..)")
    }
}

/// What a remote holds at `refs/tags/<name>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteTag {
    /// An annotated tag (it has a peeled `^{}` entry), with the commit it peels to.
    Annotated { commit: CommitSha },
    /// A lightweight tag, with the object it names.
    Lightweight { object: CommitSha },
}

impl GitRemote {
    /// The remote Callisto releases to.
    pub const ORIGIN: &'static str = "origin";

    pub fn name(&self) -> &str {
        &self.name
    }

    /// A digest of the remote's name and every fetch and push URL git resolves for it. Equal fingerprints mean git
    /// would contact the same places.
    pub fn fingerprint(&self) -> SemanticInputDigest {
        let mut transcript = CanonicalTranscript::semantic_input_v1();
        transcript.push_str("gitRemote.name", &self.name);
        for url in &self.urls.fetch {
            transcript.push_str("gitRemote.fetchUrl", url);
        }
        for url in &self.urls.push {
            transcript.push_str("gitRemote.pushUrl", url);
        }
        SemanticInputDigest::from_transcript(&transcript)
    }

    /// The GitHub repository every push URL names, for forge features only; `None` when one is not on GitHub or two
    /// name different repositories.
    pub fn github_repository(&self) -> Option<GitHubRepository> {
        let mut repositories = self.urls.push.iter().map(|url| {
            let hosted = HostedUrl::classify(url)?;
            hosted
                .host
                .eq_ignore_ascii_case("github.com")
                .then(|| GitHubRepository::parse(&format!("{}/{}", hosted.owner, hosted.repository)).ok())?
        });
        let first = repositories.next()??;
        repositories
            .all(|other| other.as_ref() == Some(&first))
            .then_some(first)
    }

    /// `host/owner/repo`, lowercased, when every push URL is a hosted repository URL naming the same one.
    pub fn repository_identity(&self) -> Option<String> {
        let mut identities = self
            .urls
            .push
            .iter()
            .map(|url| HostedUrl::classify(url).map(|hosted| hosted.identity()));
        let first = identities.next()??;
        identities.all(|other| other.as_ref() == Some(&first)).then_some(first)
    }

    /// Whether a fetch or push URL carries a password, or a user name or query over HTTP(S), where either is usually a
    /// token.
    pub fn embeds_credentials(&self) -> bool {
        self.urls
            .fetch
            .iter()
            .chain(&self.urls.push)
            .any(|url| embeds_credentials(url))
    }
}

/// The parts of `scheme://[user@]host[:port]/owner/repo[.git][/]` or scp-like `[user@]host:owner/repo[.git][/]`.
struct HostedUrl<'a> {
    host: &'a str,
    owner: &'a str,
    repository: &'a str,
}

impl<'a> HostedUrl<'a> {
    /// A local path, `file://`, a remote helper (`<transport>::<address>`) or any other path shape is not a hosted
    /// repository, so it has none.
    fn classify(url: &'a str) -> Option<Self> {
        if url.contains("::") {
            return None;
        }
        let (host, path) = match url.split_once("://") {
            Some((scheme, rest)) => {
                if scheme.eq_ignore_ascii_case("file") {
                    return None;
                }
                let (authority, path) = rest.split_once('/')?;
                let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
                (host.split(':').next()?, path)
            }
            None => {
                // Git reads `host:path` as scp-like only when no `/` comes before the first `:`.
                let (front, path) = url.split_once(':')?;
                if front.contains('/') || path.starts_with('/') {
                    return None;
                }
                (front.rsplit_once('@').map_or(front, |(_, host)| host), path)
            }
        };
        let path = path.strip_suffix('/').unwrap_or(path);
        let (owner, repository) = path.split_once('/')?;
        let repository = repository.strip_suffix(".git").unwrap_or(repository);
        let plain = |part: &str| !part.is_empty() && !part.contains('/') && part != "." && part != "..";
        (!host.is_empty() && plain(owner) && plain(repository)).then_some(Self {
            host,
            owner,
            repository,
        })
    }

    fn identity(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.repository).to_ascii_lowercase()
    }
}

fn embeds_credentials(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let scheme = scheme.rsplit("::").next().unwrap_or(scheme);
    let authority = rest.split('/').next().unwrap_or(rest);
    let http = scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http");
    (http && rest.contains('?'))
        || authority
            .rsplit_once('@')
            .is_some_and(|(userinfo, _)| userinfo.contains(':') || http)
}

impl GitAccess<'_> {
    /// The remote called `name`, or `None` when there is none.
    ///
    /// One `git remote -v` lists every fetch and push URL with `insteadOf` and `pushInsteadOf` applied, as
    /// `git remote get-url --all` and `--push --all` would, so each recheck costs one process.
    pub fn remote(&self, name: &str) -> Result<Option<GitRemote>, VcsError> {
        let output = self
            .runner()
            .run_quiet("git", &["remote", "-v"], self.root(), crate::LOCAL_GIT_TIMEOUT)?;
        let output = GitStdout::of(output)
            .map_err(|output| VcsError::Git(format!("`git remote -v` failed: {}", output.redacted_stderr())))?;
        // Each line is `<name>\t<url> (fetch)` or `<name>\t<url> (push)`; a remote name cannot hold a tab.
        let mut found = false;
        let mut urls = RemoteUrls {
            fetch: Vec::new(),
            push: Vec::new(),
        };
        for line in output.lines() {
            let Some((remote, entry)) = line.split_once('\t') else {
                continue;
            };
            if remote != name {
                continue;
            }
            found = true;
            if let Some(url) = entry.strip_suffix(" (fetch)") {
                urls.fetch.push(url.to_owned());
            } else if let Some(url) = entry.strip_suffix(" (push)") {
                urls.push.push(url.to_owned());
            }
        }
        Ok(found.then(|| GitRemote {
            name: name.to_owned(),
            urls,
        }))
    }

    /// What `remote` holds for each of `tags`, from one `git ls-remote`; a tag it does not hold is absent from the map.
    /// `None` when git could not read the remote.
    pub fn remote_tags(
        &self,
        remote: &GitRemote,
        tags: &[&TagName],
        timeout: Duration,
    ) -> Result<Option<BTreeMap<TagName, RemoteTag>>, VcsError> {
        self.ls_remote_tags(&remote.name, tags, timeout)
    }

    /// [`Self::remote_tags`] for a repository URL Callisto itself names, never one a user configured.
    pub fn url_tags(
        &self,
        url: &'static str,
        tags: &[&TagName],
        timeout: Duration,
    ) -> Result<Option<BTreeMap<TagName, RemoteTag>>, VcsError> {
        self.ls_remote_tags(url, tags, timeout)
    }

    fn ls_remote_tags(
        &self,
        remote: &str,
        tags: &[&TagName],
        timeout: Duration,
    ) -> Result<Option<BTreeMap<TagName, RemoteTag>>, VcsError> {
        if tags.is_empty() {
            return Ok(Some(BTreeMap::new()));
        }
        // The peeled `^{}` entry resolves an annotated tag to its commit; a lightweight tag has none.
        let patterns: Vec<String> = tags
            .iter()
            .flat_map(|tag| [format!("refs/tags/{tag}"), format!("refs/tags/{tag}^{{}}")])
            .collect();
        let mut args = vec!["ls-remote", "--", remote];
        args.extend(patterns.iter().map(String::as_str));
        // Quiet: an unreachable remote is an answer the caller decides about.
        let Ok(output) = GitStdout::of(self.runner().run_quiet("git", &args, self.root(), timeout)?) else {
            return Ok(None);
        };
        let mut objects = BTreeMap::new();
        let mut peeled = BTreeMap::new();
        for line in output.lines() {
            let Some((sha, reference)) = line.split_once('\t') else {
                continue;
            };
            let Some(name) = reference.strip_prefix("refs/tags/") else {
                continue;
            };
            match name.strip_suffix("^{}") {
                Some(name) => peeled.insert(name, sha),
                None => objects.insert(name, sha),
            };
        }
        let object_id = |sha: &str| {
            CommitSha::parse(sha).map_err(|error| VcsError::InvalidCommitSha {
                context: "in `git ls-remote` output".to_owned(),
                message: error.to_string(),
            })
        };
        let mut found = BTreeMap::new();
        for tag in tags {
            let state = match (peeled.get(tag.as_str()), objects.get(tag.as_str())) {
                (Some(sha), _) => RemoteTag::Annotated {
                    commit: object_id(sha)?,
                },
                (None, Some(sha)) => RemoteTag::Lightweight {
                    object: object_id(sha)?,
                },
                (None, None) => continue,
            };
            found.insert((*tag).clone(), state);
        }
        Ok(Some(found))
    }

    /// Pushes the local `refs/tags/<tag>` to `remote` under the same name; the caller reads a failure from the output.
    pub fn push_tag(&self, remote: &GitRemote, tag: &TagName, timeout: Duration) -> Result<CommandOutput, VcsError> {
        let refspec = format!("refs/tags/{tag}:refs/tags/{tag}");
        Ok(self
            .runner()
            .run_with_timeout("git", &["push", "--", &remote.name, &refspec], self.root(), timeout)?)
    }

    /// The branch the local `refs/remotes/<remote>/HEAD` names, set by `git clone` or `git remote set-head`.
    pub fn remote_head_local(&self, remote: &GitRemote) -> Option<BranchName> {
        let reference = format!("refs/remotes/{}/HEAD", remote.name);
        let output = self
            .runner()
            .run_quiet(
                "git",
                &["symbolic-ref", &reference],
                self.root(),
                crate::LOCAL_GIT_TIMEOUT,
            )
            .ok()?;
        BranchName::from_remote_ref(GitStdout::of(output).ok()?.line(), &remote.name)
    }

    /// The branch `remote`'s HEAD names, asked of the remote itself.
    pub fn remote_head_live(&self, remote: &GitRemote, timeout: Duration) -> Option<BranchName> {
        let args = ["ls-remote", "--symref", "--", &remote.name, "HEAD"];
        let output = GitStdout::of(self.runner().run_quiet("git", &args, self.root(), timeout).ok()?).ok()?;
        // Each line is `ref: refs/heads/<branch>\tHEAD`.
        output.lines().find_map(|line| {
            let (target, name) = line.strip_prefix("ref: ")?.split_once('\t')?;
            (name == "HEAD").then(|| BranchName::from_full_ref(target))?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(fetch: &[&str], push: &[&str]) -> GitRemote {
        GitRemote {
            name: GitRemote::ORIGIN.to_owned(),
            urls: RemoteUrls {
                fetch: fetch.iter().map(|url| (*url).to_owned()).collect(),
                push: push.iter().map(|url| (*url).to_owned()).collect(),
            },
        }
    }

    #[test]
    fn github_urls_in_every_git_spelling_name_the_repository() {
        for url in [
            "https://github.com/Orin-DX/Callisto.git",
            "https://github.com/orin-dx/callisto/",
            "ssh://git@github.com:22/orin-dx/callisto.git",
            "git@github.com:orin-dx/callisto.git",
            "github.com:orin-dx/callisto",
        ] {
            let classified = remote(&[url], &[url]).github_repository();
            assert_eq!(
                classified.map(|repo| repo.as_slug()).as_deref(),
                Some("orin-dx/callisto"),
                "{url}"
            );
        }
    }

    #[test]
    fn paths_helpers_and_other_shapes_are_not_hosted() {
        for url in [
            "/srv/git/callisto.git",
            "./callisto",
            "file:///srv/orin-dx/callisto",
            "hg::https://github.com/orin-dx/callisto",
            "https://github.com/orin-dx",
            "https://github.com/orin-dx/callisto/extra",
            "git@github.com:/orin-dx/callisto",
            "a/b:c/d",
            "https://github.com/./callisto",
            "https://github.com/orin-dx/..",
            "https://github.com//callisto",
            "https:///orin-dx/callisto",
        ] {
            let classified = remote(&[url], &[url]);
            assert_eq!(classified.github_repository(), None, "{url}");
            assert_eq!(classified.repository_identity(), None, "{url}");
        }
        let gitlab = "https://gitlab.com/group/project.git";
        assert_eq!(remote(&[gitlab], &[gitlab]).github_repository(), None);
        assert_eq!(
            remote(&["x"], &["ssh://git@git.example.com:2222/Team/App.git"]).repository_identity(),
            Some("git.example.com/team/app".to_owned())
        );
    }

    #[test]
    fn push_urls_naming_two_repositories_name_none() {
        let split = remote(
            &["https://github.com/a/b"],
            &["https://github.com/a/b", "https://github.com/c/d"],
        );
        assert_eq!(split.github_repository(), None);
        assert_eq!(split.repository_identity(), None);
        let fetch_elsewhere = remote(&["https://example.com/x/y"], &["git@github.com:a/b.git"]);
        assert_eq!(
            fetch_elsewhere.github_repository().map(|repo| repo.as_slug()),
            Some("a/b".to_owned())
        );
    }

    #[test]
    fn credentials_are_a_password_or_an_http_user() {
        for url in [
            "https://token@github.com/a/b",
            "https://x-access-token:secret@github.com/a/b",
            "ssh://git:secret@github.com/a/b",
            "hg::https://user@example.com/repo",
            "https://example.com/a/b?private_token=secret",
        ] {
            assert!(remote(&[url], &["git@github.com:a/b"]).embeds_credentials(), "{url}");
            assert!(remote(&["git@github.com:a/b"], &[url]).embeds_credentials(), "{url}");
        }
        for url in [
            "git@github.com:a/b",
            "ssh://git@github.com/a/b",
            "/srv/a@b/repo",
            "https://github.com/a@b",
        ] {
            assert!(!remote(&[url], &[url]).embeds_credentials(), "{url}");
        }
    }

    #[test]
    fn the_fingerprint_covers_every_url_and_debug_shows_none() {
        let base = remote(&["https://a.example/x/y"], &["https://a.example/x/y"]);
        assert_eq!(base.fingerprint(), base.clone().fingerprint());
        for other in [
            remote(&["https://b.example/x/y"], &["https://a.example/x/y"]),
            remote(&["https://a.example/x/y"], &["https://b.example/x/y"]),
            remote(
                &["https://a.example/x/y"],
                &["https://a.example/x/y", "https://b.example/x/y"],
            ),
        ] {
            assert_ne!(base.fingerprint(), other.fingerprint());
        }
        let debug = format!("{:?}", remote(&["https://user:secret@a.example/x"], &["s"]));
        assert!(!debug.contains("secret") && !debug.contains("a.example"), "{debug}");
        assert!(debug.contains("RemoteUrls(..)"), "{debug}");
    }
}
