//! `origin` on real git: its `https://github.com/example/` URLs reach local bare repositories only through an
//! `insteadOf` handed to `push` and `ls-remote`, so every other git command reads the URLs as configured.

use std::path::{Path, PathBuf};

use callisto_fixtures::git::{init_repo, run_git, GitRunner};
use callisto_model::{
    CommandError, CommandOutput, CommandRunner, CommitSha, ProviderEvidenceV1, ProviderObservationV1,
};

use super::*;
use crate::commands::release::binding::prepared_git_remote;
use crate::commands::release::provider::ProviderContext;

const HOST: &str = "https://github.com/example/";

/// Real git, with the network commands routed to the bare repositories under `bares`.
struct Routed {
    bares: PathBuf,
}

impl CommandRunner for Routed {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
        if program == "git" && matches!(args.first(), Some(&("push" | "ls-remote"))) {
            let route = format!("url.file://{}/.insteadOf={HOST}", self.bares.display());
            let mut routed = vec!["-c", route.as_str()];
            routed.extend_from_slice(args);
            return GitRunner.run(program, &routed, cwd);
        }
        GitRunner.run(program, args, cwd)
    }
}

/// A work repository with two commits and the bare repositories `fetch.git`, `push.git` and `wrong.git` beside it.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for bare in ["fetch.git", "push.git", "wrong.git"] {
            run_git(dir.path(), &["init", "-q", "--bare", bare]);
        }
        let fixture = Self { dir };
        std::fs::create_dir(fixture.work()).unwrap();
        init_repo(&fixture.work());
        for message in ["older", "release"] {
            run_git(&fixture.work(), &["commit", "-q", "--allow-empty", "-m", message]);
        }
        fixture
    }

    fn work(&self) -> PathBuf {
        self.dir.path().join("work")
    }

    fn runner(&self) -> Routed {
        Routed {
            bares: self.dir.path().to_path_buf(),
        }
    }

    fn config(&self, key: &str, value: &str) {
        run_git(&self.work(), &["config", key, value]);
    }

    fn commit(&self, revision: &str) -> CommitSha {
        let output = GitRunner.run("git", &["rev-parse", revision], &self.work()).unwrap();
        CommitSha::parse(output.stdout.trim()).unwrap()
    }

    /// Puts an annotated `v1` on `revision` into `bare` only; the work repository keeps no `v1`.
    fn land(&self, bare: &str, revision: &str) {
        run_git(&self.work(), &["tag", "-a", "v1", "-m", "Release v1", revision]);
        let url = format!("file://{}/{bare}", self.dir.path().display());
        run_git(&self.work(), &["push", "-q", &url, "refs/tags/v1:refs/tags/v1"]);
        run_git(&self.work(), &["tag", "-d", "v1"]);
    }

    fn holds_v1(&self, bare: &str) -> bool {
        let output = GitRunner
            .run("git", &["tag", "--list", "v1"], &self.dir.path().join(bare))
            .unwrap();
        output.stdout.trim() == "v1"
    }

    fn operation(&self) -> TagOperation {
        TagOperation {
            name: TagName::new_unchecked("v1".to_owned()),
            target: self.commit("HEAD"),
            annotation: "Release v1".to_owned(),
        }
    }

    fn observe(&self) -> Result<ProviderObservationV1, GraphError> {
        let runner = self.runner();
        let remote = prepared_git_remote(&self.work(), &runner)?;
        tag_observation(
            &ProviderContext::new(&self.work(), &runner, Some(&remote)),
            &self.operation(),
        )
    }

    fn split_origin(&self) {
        run_git(&self.work(), &["remote", "add", "origin", &format!("{HOST}fetch.git")]);
        self.config("remote.origin.pushurl", &format!("{HOST}push.git"));
    }
}

#[test]
fn a_tag_only_the_fetch_url_holds_does_not_satisfy_the_release() {
    let fixture = Fixture::new();
    fixture.split_origin();
    fixture.land("fetch.git", "HEAD");
    assert_eq!(fixture.observe().unwrap(), ProviderObservationV1::Absent);
}

#[test]
fn a_tag_the_push_url_holds_is_adopted() {
    let fixture = Fixture::new();
    fixture.split_origin();
    fixture.land("push.git", "HEAD");
    assert_eq!(
        fixture.observe().unwrap(),
        ProviderObservationV1::Exact {
            evidence: ProviderEvidenceV1::GitTag {
                peeled_commit: fixture.commit("HEAD"),
            },
        }
    );
}

/// `git remote -v` prints a partial clone's fetch URL as `<url> (fetch) [blob:none]`.
#[test]
fn a_partial_clone_fetch_url_changed_after_preparation_is_never_read() {
    let fixture = Fixture::new();
    fixture.split_origin();
    fixture.config("remote.origin.promisor", "true");
    fixture.config("remote.origin.partialclonefilter", "blob:none");
    let runner = fixture.runner();
    let remote = prepared_git_remote(&fixture.work(), &runner).unwrap();
    fixture.land("wrong.git", "HEAD~1");
    fixture.config("remote.origin.url", &format!("{HOST}wrong.git"));
    let observed = tag_observation(
        &ProviderContext::new(&fixture.work(), &runner, Some(&remote)),
        &fixture.operation(),
    );
    assert!(
        matches!(
            observed,
            Ok(ProviderObservationV1::Absent) | Err(GraphError::ReleaseIntentStale { .. })
        ),
        "{observed:?}"
    );
}

#[test]
fn an_origin_without_a_url_is_refused() {
    let fixture = Fixture::new();
    fixture.config("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*");
    let prepared = prepared_git_remote(&fixture.work(), &fixture.runner());
    assert!(
        matches!(prepared, Err(GraphError::UnsafeGitRemote { .. })),
        "{prepared:?}"
    );
}

#[test]
fn a_remote_url_with_a_line_break_is_refused() {
    let fixture = Fixture::new();
    fixture.split_origin();
    let local = fixture.dir.path().join("line\nbreak.git");
    for url in [
        format!("{HOST}pu\nsh.git"),
        format!("{HOST}pu\rsh.git"),
        local.to_str().unwrap().to_owned(),
    ] {
        fixture.config("remote.origin.pushurl", &url);
        let prepared = prepared_git_remote(&fixture.work(), &fixture.runner());
        assert!(
            matches!(prepared, Err(GraphError::UnsafeGitRemote { .. })),
            "{url:?}: {prepared:?}"
        );
    }
}

#[test]
fn a_mirror_origin_still_takes_the_tag_push() {
    let fixture = Fixture::new();
    run_git(
        &fixture.work(),
        &["remote", "add", "origin", &format!("{HOST}push.git")],
    );
    fixture.config("remote.origin.mirror", "true");
    run_git(&fixture.work(), &["tag", "-a", "v1", "-m", "Release v1"]);
    let runner = fixture.runner();
    let remote = prepared_git_remote(&fixture.work(), &runner).unwrap();
    push_tag(&runner, &fixture.work(), &remote.endpoint, &fixture.operation()).unwrap();
    assert!(fixture.holds_v1("push.git"));
}
