//! `CommandRunner`-shelled implementation of [`GitDataSource`], consolidated
//! from five independently hand-rolled `git`-subprocess fallbacks that used
//! to live at each call site (`callisto-graph`'s `changed.rs`, `tags.rs`,
//! `commands/tag.rs`, `aggregate.rs`, and `callisto-conventional`'s
//! `window.rs`). Every operation here is reachable purely via
//! [`callisto_model::CommandRunner`], so it works on every target,
//! including `wasm32`, where native `gix` (`GitRepository`) is entirely
//! unavailable.
//!
//! Callers should not construct [`ShellGit`] directly except in tests that
//! specifically want to exercise the shell backend in isolation; use
//! [`crate::GitAccess::discover`] instead, which selects between this and
//! the native backend automatically.

use std::path::{Path, PathBuf};

use callisto_model::{ApplyPermit, CommandRunner, CommitSha, StagedChangeKindV1, StagedChangeV1, TagName};

use crate::{
    access::{GitCommitTrustEvidence, GitHeadDisposition},
    GitCommit, GitDataSource, VcsError,
};

/// Record separator placed immediately before each commit's fields in
/// `git log --format=` output, so a commit message containing `\n` can
/// never be mistaken for a record boundary.
const RECORD_SEP: char = '\u{1e}';

/// Unit separator between a commit's sha and its raw message body in
/// `git log --format=` output.
const FIELD_SEP: char = '\u{1f}';

/// Closed release policy for ignored worktree artifacts. Repository ignore
/// rules are not authority to omit arbitrary release input from trust.
const RELEASE_IGNORED_ALLOWLIST: &[&str] = &[
    ".DS_Store",
    "target/",
    "bin/",
    ".moon/cache/",
    ".moon/docker/",
    ".claude/worktrees/",
    "mutants.out/",
    "lcov.info",
];

/// Redacts known registry/VCS credential env-var values and any URL
/// userinfo component from raw `git` subprocess stderr before it is
/// embedded in a [`VcsError`] -- a failing `git` invocation can surface an
/// authenticated remote URL (e.g. GitHub Actions'
/// `https://x-access-token:TOKEN@github.com/...`) verbatim in its own
/// error output, and that text flows into `--format json` and miette
/// diagnostic output downstream.
fn redact_git_stderr(text: &str) -> String {
    callisto_model::redact_known_secrets(text, &callisto_model::known_credential_env_values(std::env::vars()))
}

/// Shells `git` subcommands via a [`CommandRunner`] to implement
/// [`GitDataSource`]. See the module docs for the consolidation this
/// replaces.
pub struct ShellGit<'r> {
    runner: &'r dyn CommandRunner,
    root: PathBuf,
}

impl<'r> ShellGit<'r> {
    pub fn new(runner: &'r dyn CommandRunner, root: impl Into<PathBuf>) -> Self {
        ShellGit {
            runner,
            root: root.into(),
        }
    }

    /// Returns every staged (Git index) change relative to `base`. File
    /// contents for additions and modifications are read directly from the
    /// worktree with [`std::fs::read`], never through a [`CommandRunner`]
    /// (whose captured stdout is a lossy `String`), so CRLF and binary
    /// content survive exactly.
    pub fn staged_changes_since(&self, base: &CommitSha) -> Result<Vec<StagedChangeV1>, VcsError> {
        let output = self.runner.run(
            "git",
            &["diff", "--cached", "--raw", "-z", "--no-renames", base.as_str()],
            &self.root,
        )?;
        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git diff --cached --raw` against `{}` failed: {}",
                base.as_str(),
                redact_git_stderr(&output.stderr)
            )));
        }

        let entries = parse_raw_diff_z(&output.stdout)?;
        let mut changes = Vec::with_capacity(entries.len());
        for entry in entries {
            let kind = staged_change_kind(entry.status);
            let contents = match kind {
                StagedChangeKindV1::Added | StagedChangeKindV1::Modified => {
                    let bytes = std::fs::read(self.root.join(&entry.path)).map_err(|error| {
                        VcsError::Git(format!("could not read staged file `{}`: {error}", entry.path))
                    })?;
                    Some(bytes)
                }
                _ => None,
            };
            changes.push(StagedChangeV1 {
                path: entry.path,
                kind,
                new_mode: entry.new_mode,
                contents,
            });
        }
        Ok(changes)
    }

    /// Returns explicit, fresh Git trust evidence for a durable release.
    /// Errors deliberately omit raw stderr and worktree path text.
    pub fn observe_git_commit_trust(&self) -> Result<GitCommitTrustEvidence, VcsError> {
        let root = self.runner.run("git", &["rev-parse", "--show-toplevel"], &self.root)?;
        if !root.success() {
            return Err(VcsError::Git(
                "could not determine the Git repository root for release trust".to_string(),
            ));
        }
        let canonical_root = canonical_git_root(root.stdout_trimmed())?;

        let object_format = self
            .runner
            .run("git", &["rev-parse", "--show-object-format"], &self.root)?;
        if !object_format.success() || object_format.stdout_trimmed() != "sha1" {
            return Err(VcsError::Git(
                "release trust requires a SHA-1 Git object format".to_string(),
            ));
        }
        let shallow = self
            .runner
            .run("git", &["rev-parse", "--is-shallow-repository"], &self.root)?;
        if !shallow.success() || shallow.stdout_trimmed() != "false" {
            return Err(VcsError::Git(
                "release trust requires a complete, non-shallow Git repository".to_string(),
            ));
        }

        let head = self
            .runner
            .run("git", &["rev-parse", "--verify", "HEAD^{commit}"], &self.root)?;
        if !head.success() {
            return Err(VcsError::Git(
                "could not resolve a commit for release trust".to_string(),
            ));
        }
        let head = CommitSha::parse(head.stdout_trimmed()).map_err(|error| {
            drop(error);
            VcsError::Git("Git returned an invalid full HEAD commit for release trust".to_string())
        })?;

        let symbolic_head = self
            .runner
            .run("git", &["symbolic-ref", "--quiet", "HEAD"], &self.root)?;
        let head_disposition = match symbolic_head.exit_code {
            Some(0) => GitHeadDisposition::Attached,
            Some(1) => GitHeadDisposition::Detached,
            _ => {
                return Err(VcsError::Git(
                    "could not determine the symbolic HEAD state for release trust".to_string(),
                ))
            }
        };

        let status = self.runner.run(
            "git",
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignored=matching",
            ],
            &self.root,
        )?;
        if !status.success() {
            return Err(VcsError::Git(
                "could not inspect the worktree for release trust".to_string(),
            ));
        }

        Ok(GitCommitTrustEvidence::new(
            canonical_root,
            head,
            head_disposition,
            parse_release_worktree_status(&status.stdout)?,
        ))
    }
}

/// One `git diff --raw -z` record, before Git's single-letter status is
/// mapped to a [`StagedChangeKindV1`] and file contents are read.
struct RawDiffEntry {
    path: String,
    /// The post-image Git file mode, or `None` for a pure deletion.
    new_mode: Option<u32>,
    status: char,
}

/// Parses `git diff --raw -z --no-renames` output. Each record is two
/// `\0`-separated tokens: a `:<old-mode> <new-mode> <old-sha> <new-sha>
/// <status>` metadata line, then the path. `--no-renames` guarantees at
/// most one path per record (a rename or copy would otherwise emit two).
fn parse_raw_diff_z(raw: &str) -> Result<Vec<RawDiffEntry>, VcsError> {
    let mut entries = Vec::new();
    let mut tokens = raw.split('\0').filter(|token| !token.is_empty());
    while let Some(meta) = tokens.next() {
        let Some(meta) = meta.strip_prefix(':') else {
            return Err(VcsError::Git(format!(
                "unexpected token in `git diff --raw -z` output: {meta:?}"
            )));
        };
        let path = tokens.next().ok_or_else(|| {
            VcsError::Git("truncated `git diff --raw -z` output: missing a path after its metadata line".to_string())
        })?;
        let fields: Vec<&str> = meta.split(' ').collect();
        let [_old_mode, new_mode, _old_sha, _new_sha, status_field] = fields.as_slice() else {
            return Err(VcsError::Git(format!(
                "malformed `git diff --raw -z` metadata line: {meta:?}"
            )));
        };
        let status = status_field
            .chars()
            .next()
            .ok_or_else(|| VcsError::Git(format!("empty status in `git diff --raw -z` metadata line: {meta:?}")))?;
        let new_mode =
            if status == 'D' {
                None
            } else {
                Some(u32::from_str_radix(new_mode, 8).map_err(|error| {
                    VcsError::Git(format!("invalid Git mode `{new_mode}` in raw diff output: {error}"))
                })?)
            };
        entries.push(RawDiffEntry {
            path: path.to_string(),
            new_mode,
            status,
        });
    }
    Ok(entries)
}

/// Maps a `git diff --raw` single-letter status to a [`StagedChangeKindV1`].
/// Any status this codebase does not have a named case for (Git's own docs
/// list `X` as "unknown") conservatively maps to `Unmerged`, which
/// [`callisto_model::ReleasePrCommitPlanV1::from_changes`] always rejects --
/// safe by construction rather than by an exhaustive status list here.
fn staged_change_kind(status: char) -> StagedChangeKindV1 {
    match status {
        'A' => StagedChangeKindV1::Added,
        'M' => StagedChangeKindV1::Modified,
        'D' => StagedChangeKindV1::Deleted,
        'R' => StagedChangeKindV1::Renamed,
        'C' => StagedChangeKindV1::Copied,
        'T' => StagedChangeKindV1::TypeChanged,
        _ => StagedChangeKindV1::Unmerged,
    }
}

fn canonical_git_root(raw_root: &str) -> Result<PathBuf, VcsError> {
    if raw_root.is_empty() {
        return Err(VcsError::Git(
            "Git returned an empty repository root for release trust".to_string(),
        ));
    }
    dunce::canonicalize(raw_root).map_err(|error| {
        drop(error);
        VcsError::Git("could not canonicalize the Git repository root for release trust".to_string())
    })
}

fn parse_release_worktree_status(status: &str) -> Result<Vec<PathBuf>, VcsError> {
    let mut allowed_ignored_paths = Vec::new();
    for record in status.split('\0').filter(|record| !record.is_empty()) {
        let Some(path) = record.strip_prefix("!! ") else {
            return Err(VcsError::Git(
                "release trust requires a worktree with no tracked or untracked files".to_string(),
            ));
        };
        let path = PathBuf::from(path);
        if !is_release_ignored_path_allowed(&path) {
            return Err(VcsError::Git(
                "release trust found an ignored path outside its fixed allowlist".to_string(),
            ));
        }
        allowed_ignored_paths.push(path);
    }
    allowed_ignored_paths.sort();
    allowed_ignored_paths.dedup();
    Ok(allowed_ignored_paths)
}

fn is_release_ignored_path_allowed(path: &Path) -> bool {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return false;
    }
    let rendered = path.to_string_lossy().replace('\\', "/");
    RELEASE_IGNORED_ALLOWLIST.iter().any(|allowed| {
        allowed.strip_suffix('/').map_or_else(
            || rendered == *allowed,
            |directory| rendered == directory || rendered.starts_with(&format!("{directory}/")),
        )
    })
}

/// Compiles `glob` into a [`globset::GlobMatcher`], surfacing a malformed
/// pattern as [`VcsError::InvalidGlob`] rather than letting it silently
/// disable filtering (which would match every tag -- see
/// `GitRepository::list_tags`'s own doc comment for why that's a real
/// correctness risk for release tagging).
fn compile_glob(glob: &str) -> Result<globset::GlobMatcher, VcsError> {
    globset::Glob::new(glob)
        .map(|g| g.compile_matcher())
        .map_err(|e| VcsError::InvalidGlob {
            pattern: glob.to_string(),
            message: e.to_string(),
        })
}

impl GitDataSource for ShellGit<'_> {
    fn head_sha(&self) -> Result<CommitSha, VcsError> {
        let output = self.runner.run("git", &["rev-parse", "HEAD"], &self.root)?;
        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git rev-parse HEAD` failed in `{}`: {}",
                self.root.display(),
                redact_git_stderr(&output.stderr)
            )));
        }
        let sha_str = output.stdout_trimmed();
        CommitSha::parse(sha_str).map_err(|e| VcsError::Git(format!("could not parse HEAD SHA `{sha_str}`: {e}")))
    }

    /// Always fetches the *full, unfiltered* tag list via `git tag --list`
    /// and applies `glob` (if any) locally with `globset` -- deliberately
    /// never delegates filtering to `git tag --list <pattern>`'s own
    /// (different) glob dialect. This is what guarantees byte-identical
    /// tag selection between this backend and the native `gix` path
    /// (`GitRepository::list_tags`, which also filters with `globset`)
    /// regardless of which one a caller ends up using.
    fn list_tags(&self, glob: Option<&str>) -> Result<Vec<TagName>, VcsError> {
        let output = self.runner.run("git", &["tag", "--list"], &self.root)?;
        if !output.success() {
            // A directory with no `.git` anywhere in its ancestry is not a
            // hard failure -- it's the normal shape of a brand-new/pre-git
            // package, and callers (e.g. `status`) treat "no tags" as
            // "unreleased" rather than aborting. Any OTHER failure (a
            // corrupted or locked repository, a permissions error, ...)
            // must still surface as a real error rather than silently
            // becoming "zero tags".
            if !output.stderr.contains("not a git repository") {
                return Err(VcsError::Git(format!(
                    "`git tag --list` failed in `{}`: {}",
                    self.root.display(),
                    redact_git_stderr(&output.stderr)
                )));
            }
        }
        let all = output.stdout_lines().map(|s| s.to_string());

        let matcher = glob.map(compile_glob).transpose()?;

        Ok(all
            .filter(|t| matcher.as_ref().is_none_or(|m| m.is_match(t)))
            .map(TagName)
            .collect())
    }

    fn resolve_commit(&self, refname: &str) -> Result<Option<CommitSha>, VcsError> {
        let rev = format!("{refname}^{{commit}}");
        let output = self
            .runner
            .run("git", &["rev-parse", "--verify", "--quiet", &rev], &self.root)?;

        if !output.success() {
            return Ok(None);
        }
        let sha_str = output.stdout_trimmed();
        if sha_str.is_empty() {
            return Ok(None);
        }
        Ok(CommitSha::parse(sha_str).ok())
    }

    /// Shells `git log --no-merges --format=<RECORD_SEP>%H<FIELD_SEP>%B
    /// <range> [-- <pathspecs>]`, matching the native path field-for-field:
    /// `--no-merges` excludes merges like
    /// `GitRepository::commits_since_with_pathspec`'s parent-count skip;
    /// `<since>..HEAD` (or bare `HEAD`) gives the same
    /// exclusive-lower-bound range as the revwalk's stop-at-`since` logic;
    /// `-- <pathspecs>` reproduces `git log`'s own pathspec-prefix
    /// filtering.
    ///
    /// Deliberately doesn't pre-resolve `since_ref` via a separate
    /// `rev-parse`: an unresolvable ref already makes `git log
    /// <since_ref>..HEAD` fail (non-zero exit), surfaced below as `Err`
    /// like any other failure -- one shell call either way.
    fn commits_since(&self, since_ref: Option<&str>, pathspecs: &[PathBuf]) -> Result<Vec<GitCommit>, VcsError> {
        let mut args: Vec<String> = vec![
            "log".to_string(),
            "--no-merges".to_string(),
            format!("--format={RECORD_SEP}%H{FIELD_SEP}%B"),
        ];

        match since_ref {
            Some(r) => args.push(format!("{r}..HEAD")),
            None => args.push("HEAD".to_string()),
        }

        if !pathspecs.is_empty() {
            args.push("--".to_string());
            args.extend(pathspecs.iter().map(|p| p.display().to_string()));
        }

        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = self.runner.run("git", &arg_refs, &self.root)?;

        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git log` failed in `{}`: {}",
                self.root.display(),
                redact_git_stderr(&output.stderr)
            )));
        }

        parse_git_log_output(&output.stdout, &self.root)
    }

    fn create_tag(
        &self,
        name: &str,
        target_sha: &CommitSha,
        message: Option<&str>,
        _permit: &ApplyPermit,
    ) -> Result<(), VcsError> {
        // `--` marks the end of option parsing so `name` (validated
        // upstream by `is_valid_git_ref_name`, but defended here too) can
        // never be misread as a `git tag` flag even if it started with `-`.
        let output = match message {
            Some(msg) => self.runner.run(
                "git",
                &["tag", "-a", "-m", msg, "--", name, target_sha.as_str()],
                &self.root,
            )?,
            None => self
                .runner
                .run("git", &["tag", "--", name, target_sha.as_str()], &self.root)?,
        };
        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git tag` failed in `{}`: {}",
                self.root.display(),
                redact_git_stderr(&output.stderr)
            )));
        }
        Ok(())
    }

    fn create_floating_major(
        &self,
        major_name: &str,
        target_sha: &CommitSha,
        _permit: &ApplyPermit,
    ) -> Result<(), VcsError> {
        let output = self
            .runner
            .run("git", &["tag", "-f", "--", major_name, target_sha.as_str()], &self.root)?;
        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git tag -f` failed in `{}`: {}",
                self.root.display(),
                redact_git_stderr(&output.stderr)
            )));
        }
        Ok(())
    }
}

/// Parses the `<RECORD_SEP>%H<FIELD_SEP>%B`-formatted `git log` output
/// produced by [`ShellGit::commits_since`] into [`GitCommit`]s, splitting
/// each raw message on its first blank line into `summary`/`body` --
/// mirroring how `gix`'s own commit-message parsing (used by
/// `GitRepository::commits_since_with_pathspec`) splits title from body,
/// so both backends hand callers the same shape.
fn parse_git_log_output(stdout: &str, cwd: &Path) -> Result<Vec<GitCommit>, VcsError> {
    let mut commits = Vec::new();

    for record in stdout.split(RECORD_SEP) {
        // `tformat:`-style `--format=` output (the default when the format
        // string doesn't start with `format:`/`tformat:`) appends a
        // trailing newline after every entry; strip it rather than the
        // message's own content.
        let record = record.trim_end_matches('\n');
        if record.is_empty() {
            continue;
        }

        let Some((sha_str, message)) = record.split_once(FIELD_SEP) else {
            return Err(VcsError::Git(format!(
                "could not parse `git log` output in `{}` into commit records: expected a \
                 `<sha>{FIELD_SEP:?}<message>` record, got: {record:?}",
                cwd.display()
            )));
        };

        let sha = CommitSha::parse(sha_str).map_err(|e| {
            VcsError::Git(format!(
                "could not parse `git log` output in `{}`: invalid commit SHA `{sha_str}`: {e}",
                cwd.display()
            ))
        })?;

        let message = message.replace("\r\n", "\n");
        let (summary, body) = match message.split_once("\n\n") {
            Some((title, rest)) => (title.trim_end().to_string(), Some(rest.to_string())),
            None => (message.trim_end().to_string(), None),
        };

        commits.push(GitCommit { sha, summary, body });
    }

    Ok(commits)
}

#[cfg(test)]
mod tests {

    /// Tests exercise the write primitives directly rather than through a
    /// command handler, so they mint a permit without a dry-run flag to
    /// consult. Every non-test caller must go through
    /// `ApplyPermit::granted_unless_dry_run`.
    fn permit() -> callisto_model::ApplyPermit {
        callisto_model::ApplyPermit::force_for_tests()
    }
    use super::*;
    use callisto_model::{CommandError, CommandOutput};
    use std::sync::Mutex;

    type ResponseFn = dyn Fn(&[&str]) -> Result<CommandOutput, CommandError> + Send + Sync;

    struct FakeRunner {
        calls: Mutex<Vec<Vec<String>>>,
        response: Box<ResponseFn>,
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
            assert_eq!(program, "git");
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|s| s.to_string()).collect());
            (self.response)(args)
        }
    }

    fn ok(stdout: impl Into<String>) -> CommandOutput {
        CommandOutput {
            exit_code: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn trust_response(root: PathBuf, status: String, detached: bool) -> Box<ResponseFn> {
        Box::new(move |args| match args {
            ["rev-parse", "--show-toplevel"] => Ok(ok(format!("{}\n", root.display()))),
            ["rev-parse", "--show-object-format"] => Ok(ok("sha1\n")),
            ["rev-parse", "--is-shallow-repository"] => Ok(ok("false\n")),
            ["rev-parse", "--verify", "HEAD^{commit}"] => Ok(ok(format!("{}\n", "a".repeat(40)))),
            ["symbolic-ref", "--quiet", "HEAD"] if detached => Ok(CommandOutput {
                exit_code: Some(1),
                stdout: String::new(),
                stderr: String::new(),
            }),
            ["symbolic-ref", "--quiet", "HEAD"] => Ok(ok("refs/heads/main\n")),
            ["status", "--porcelain=v1", "-z", "--untracked-files=all", "--ignored=matching"] => Ok(ok(status.clone())),
            other => panic!("unexpected Git trust command: {other:?}"),
        })
    }

    #[test]
    fn git_commit_trust_evidence_is_root_bound_and_allows_only_fixed_ignored_paths() {
        let temp = tempfile::tempdir().unwrap();
        let expected_root = dunce::canonicalize(temp.path()).unwrap();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: trust_response(
                temp.path().to_path_buf(),
                "!! target/debug/callisto\0!! .moon/cache/state\0".to_string(),
                true,
            ),
        };
        let git = ShellGit::new(&runner, temp.path());

        let evidence = git.observe_git_commit_trust().unwrap();

        assert_eq!(evidence.canonical_root(), expected_root);
        assert_eq!(evidence.head().as_str(), "a".repeat(40));
        assert_eq!(evidence.head_disposition(), GitHeadDisposition::Detached);
        assert_eq!(
            evidence.allowed_ignored_paths(),
            &[
                PathBuf::from(".moon/cache/state"),
                PathBuf::from("target/debug/callisto")
            ]
        );
    }

    #[test]
    fn git_commit_trust_rejects_untracked_or_tracked_worktree_entries() {
        let temp = tempfile::tempdir().unwrap();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: trust_response(temp.path().to_path_buf(), "?? release-input.txt\0".to_string(), false),
        };
        let git = ShellGit::new(&runner, temp.path());

        let error = git
            .observe_git_commit_trust()
            .expect_err("untracked source must reject trust");

        assert!(matches!(error, VcsError::Git(message) if message.contains("no tracked or untracked files")));
    }

    #[test]
    fn git_commit_trust_rejects_ignored_paths_outside_fixed_allowlist() {
        let temp = tempfile::tempdir().unwrap();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: trust_response(
                temp.path().to_path_buf(),
                "!! callisto-schema.json\0".to_string(),
                false,
            ),
        };
        let git = ShellGit::new(&runner, temp.path());

        let error = git
            .observe_git_commit_trust()
            .expect_err("ignored generated input must not be silently accepted");

        assert!(matches!(error, VcsError::Git(message) if message.contains("fixed allowlist")));
    }

    #[test]
    fn git_commit_trust_rejects_sha256_or_shallow_repositories() {
        let temp = tempfile::tempdir().unwrap();
        for (arguments, response) in [
            (("--show-object-format", "sha256\n"), "SHA-1 Git object format"),
            (("--is-shallow-repository", "true\n"), "non-shallow Git repository"),
        ] {
            let root = temp.path().to_path_buf();
            let runner = FakeRunner {
                calls: Mutex::new(Vec::new()),
                response: Box::new(move |args| {
                    if args == ["rev-parse", arguments.0] {
                        return Ok(ok(arguments.1));
                    }
                    trust_response(root.clone(), String::new(), true)(args)
                }),
            };
            let error = ShellGit::new(&runner, temp.path())
                .observe_git_commit_trust()
                .expect_err("unsupported repository trust evidence must reject");
            assert!(matches!(error, VcsError::Git(message) if message.contains(response)));
        }
    }

    #[test]
    fn test_list_tags_fetches_unfiltered_and_filters_locally_with_globset() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok("pkg-a@1.0.0\npkg-ab@1.0.0\nunrelated\n"))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let tags = git.list_tags(Some("pkg-a@*")).unwrap();

        assert_eq!(
            tags.into_iter().map(|t| t.0).collect::<Vec<_>>(),
            vec!["pkg-a@1.0.0".to_string()]
        );
        // Exactly one shell call, and it must not bake the glob into the
        // command -- filtering happens locally so both backends share
        // identical semantics.
        assert_eq!(*runner.calls.lock().unwrap(), vec![vec!["tag", "--list"]]);
    }

    #[test]
    fn test_list_tags_rejects_malformed_glob() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok("pkg-a@1.0.0\n"))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let result = git.list_tags(Some("pkg-a@{malformed"));

        assert!(matches!(result, Err(VcsError::InvalidGlob { .. })));
    }

    /// A failing `git tag --list` for a reason OTHER than "no repository
    /// here" (e.g. a corrupted or locked repository) must surface as
    /// `Err`, not be silently treated as "zero tags" -- every sibling
    /// method on this impl (`head_sha`, `resolve_commit`, ...) already
    /// checks `output.success()` before trusting stdout.
    #[test]
    fn test_list_tags_errors_on_failed_git_invocation() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| {
                Ok(CommandOutput {
                    exit_code: Some(128),
                    stdout: String::new(),
                    stderr: "fatal: unable to read current working directory: No such file or directory".to_string(),
                })
            }),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let result = git.list_tags(None);

        assert!(
            matches!(result, Err(VcsError::Git(ref msg)) if msg.contains("unable to read current working directory")),
            "expected Err(VcsError::Git(..)) mentioning the failure, got: {result:?}"
        );
    }

    /// A directory with no `.git` anywhere in its ancestry is not a hard
    /// failure -- callers (e.g. `status` on a brand-new/pre-git package)
    /// treat "no tags" as "unreleased" rather than aborting. This is the
    /// one specific failure shape `list_tags` deliberately still tolerates
    /// as `Ok(vec![])`, distinct from `test_list_tags_errors_on_failed_git_invocation`'s
    /// genuine failure.
    #[test]
    fn test_list_tags_tolerates_missing_git_repository_as_empty() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| {
                Ok(CommandOutput {
                    exit_code: Some(128),
                    stdout: String::new(),
                    stderr: "fatal: not a git repository (or any of the parent directories): .git".to_string(),
                })
            }),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let result = git.list_tags(None);

        assert_eq!(
            result.unwrap(),
            Vec::<TagName>::new(),
            "a missing .git directory must resolve to Ok(vec![]), not Err"
        );
    }

    #[test]
    fn test_resolve_commit_returns_none_on_failed_rev_parse() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| {
                Ok(CommandOutput {
                    exit_code: Some(1),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        assert_eq!(git.resolve_commit("missing-tag").unwrap(), None);
    }

    #[test]
    fn test_resolve_commit_parses_sha_on_success() {
        let sha = "a".repeat(40);
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(move |_args| Ok(ok(format!("{}\n", "a".repeat(40))))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        assert_eq!(
            git.resolve_commit("v1.0.0").unwrap(),
            Some(CommitSha::parse(&sha).unwrap())
        );
    }

    #[test]
    fn test_commits_since_parses_record_and_field_separators_into_summary_and_body() {
        let sha_a = "a".repeat(40);
        let sha_b = "b".repeat(40);
        let stdout = format!(
            "{RECORD_SEP}{sha_a}{FIELD_SEP}feat: add thing\n\nSome body text\n{RECORD_SEP}{sha_b}{FIELD_SEP}fix: bug\n"
        );
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(move |_args| Ok(ok(stdout.clone()))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let commits = git.commits_since(None, &[]).unwrap();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha.as_str(), sha_a);
        assert_eq!(commits[0].summary, "feat: add thing");
        assert_eq!(commits[0].body.as_deref(), Some("Some body text"));
        assert_eq!(commits[1].sha.as_str(), sha_b);
        assert_eq!(commits[1].summary, "fix: bug");
        assert_eq!(commits[1].body, None);
    }

    #[test]
    fn test_commits_since_builds_since_range_and_pathspec_args() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok(""))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let sha = "c".repeat(40);
        git.commits_since(Some(&sha), &[PathBuf::from("crates/pkg-a")]).unwrap();

        let calls = runner.calls.lock().unwrap();
        let args = &calls[0];
        assert!(args.contains(&"--no-merges".to_string()));
        assert!(args.contains(&format!("{sha}..HEAD")));
        assert!(args.contains(&"--".to_string()));
        assert!(args.contains(&"crates/pkg-a".to_string()));
    }

    #[test]
    fn test_commits_since_propagates_git_log_failure_instead_of_unbounded_walk() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| {
                Ok(CommandOutput {
                    exit_code: Some(128),
                    stdout: String::new(),
                    stderr: "fatal: bad revision 'missing-ref..HEAD'".to_string(),
                })
            }),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let result = git.commits_since(Some("missing-ref"), &[]);

        assert!(
            matches!(result, Err(VcsError::Git(_))),
            "an unresolvable since_ref must surface as Err, not silently degrade to an \
             unbounded walk; got {result:?}"
        );
    }

    #[test]
    fn test_create_tag_annotated_shells_expected_args() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok(""))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        let sha = CommitSha::parse(&"d".repeat(40)).unwrap();

        git.create_tag("pkg-a@1.0.0", &sha, Some("Release pkg-a@1.0.0"), &permit())
            .unwrap();

        let calls = runner.calls.lock().unwrap();
        assert_eq!(
            calls[0],
            vec![
                "tag",
                "-a",
                "-m",
                "Release pkg-a@1.0.0",
                "--",
                "pkg-a@1.0.0",
                sha.as_str(),
            ]
        );
    }

    #[test]
    fn test_create_tag_lightweight_shells_expected_args() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok(""))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        let sha = CommitSha::parse(&"e".repeat(40)).unwrap();

        git.create_tag("pkg-a@1.0.0", &sha, None, &permit()).unwrap();

        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls[0], vec!["tag", "--", "pkg-a@1.0.0", sha.as_str()]);
    }

    #[test]
    fn test_create_floating_major_shells_force_tag() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok(""))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        let sha = CommitSha::parse(&"f".repeat(40)).unwrap();

        git.create_floating_major("pkg-a@1", &sha, &permit()).unwrap();

        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls[0], vec!["tag", "-f", "--", "pkg-a@1", sha.as_str()]);
    }

    #[test]
    fn test_head_sha_returns_current_head_commit() {
        let sha = "a".repeat(40);
        let sha_clone = sha.clone();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(move |_args| Ok(ok(format!("{}\n", sha_clone)))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let result = git.head_sha().unwrap();
        assert_eq!(result.as_str(), sha);
        assert_eq!(*runner.calls.lock().unwrap(), vec![vec!["rev-parse", "HEAD"]]);
    }

    #[test]
    fn test_head_sha_propagates_git_failure() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| {
                Ok(CommandOutput {
                    exit_code: Some(128),
                    stdout: String::new(),
                    stderr: "fatal: not a git repository".to_string(),
                })
            }),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        assert!(matches!(git.head_sha(), Err(VcsError::Git(_))));
    }

    /// A leaking authenticated remote URL in `git`'s stderr (the realistic
    /// GitHub Actions shape: `https://x-access-token:TOKEN@github.com/...`)
    /// must not survive into a `VcsError` from any of the four operations
    /// that embed raw subprocess stderr.
    fn leaky_response(_args: &[&str]) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput {
            exit_code: Some(128),
            stdout: String::new(),
            stderr: "fatal: unable to access 'https://x-access-token:ghs_leaked_secret@github.com/org/repo.git/': The requested URL returned error: 403".to_string(),
        })
    }

    #[test]
    fn head_sha_failure_redacts_credential() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(leaky_response),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        let err = git.head_sha().expect_err("must fail");
        let rendered = format!("{err}");
        assert!(!rendered.contains("ghs_leaked_secret"), "got: {rendered}");
        assert!(rendered.contains("[REDACTED]"), "got: {rendered}");
    }

    #[test]
    fn commits_since_failure_redacts_credential() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(leaky_response),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        let err = git.commits_since(None, &[]).expect_err("must fail");
        let rendered = format!("{err}");
        assert!(!rendered.contains("ghs_leaked_secret"), "got: {rendered}");
        assert!(rendered.contains("[REDACTED]"), "got: {rendered}");
    }

    #[test]
    fn create_tag_failure_redacts_credential() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(leaky_response),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        let sha = CommitSha::parse(&"a".repeat(40)).unwrap();
        let err = git.create_tag("v1.0.0", &sha, None, &permit()).expect_err("must fail");
        let rendered = format!("{err}");
        assert!(!rendered.contains("ghs_leaked_secret"), "got: {rendered}");
        assert!(rendered.contains("[REDACTED]"), "got: {rendered}");
    }

    #[test]
    fn create_floating_major_failure_redacts_credential() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(leaky_response),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        let sha = CommitSha::parse(&"a".repeat(40)).unwrap();
        let err = git.create_floating_major("v1", &sha, &permit()).expect_err("must fail");
        let rendered = format!("{err}");
        assert!(!rendered.contains("ghs_leaked_secret"), "got: {rendered}");
        assert!(rendered.contains("[REDACTED]"), "got: {rendered}");
    }

    #[test]
    fn parse_raw_diff_z_parses_modes_status_and_odd_paths() {
        let raw = [
            ":000000 100644 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 A",
            "VERSION",
            ":100644 100644 2222222222222222222222222222222222222222 3333333333333333333333333333333333333333 M",
            "path with spaces/файл.txt",
            ":100644 000000 4444444444444444444444444444444444444444 0000000000000000000000000000000000000000 D",
            ".changeset/old-entry.md",
            ":100644 120000 5555555555555555555555555555555555555555 6666666666666666666666666666666666666666 T",
            "was-a-file-now-a-symlink",
            ":000000 100644 0000000000000000000000000000000000000000 7777777777777777777777777777777777777777 U",
            "conflicted.txt",
        ]
        .join("\0")
            + "\0";

        let entries = parse_raw_diff_z(&raw).unwrap();
        assert_eq!(entries.len(), 5);

        assert_eq!(entries[0].path, "VERSION");
        assert_eq!(entries[0].status, 'A');
        assert_eq!(entries[0].new_mode, Some(0o100644));

        assert_eq!(entries[1].path, "path with spaces/файл.txt");
        assert_eq!(entries[1].status, 'M');

        assert_eq!(entries[2].path, ".changeset/old-entry.md");
        assert_eq!(entries[2].status, 'D');
        assert_eq!(entries[2].new_mode, None, "a deletion has no post-image mode");

        assert_eq!(entries[3].status, 'T');
        assert_eq!(entries[3].new_mode, Some(0o120000));

        assert_eq!(entries[4].status, 'U');

        assert_eq!(staged_change_kind('A'), StagedChangeKindV1::Added);
        assert_eq!(staged_change_kind('M'), StagedChangeKindV1::Modified);
        assert_eq!(staged_change_kind('D'), StagedChangeKindV1::Deleted);
        assert_eq!(staged_change_kind('T'), StagedChangeKindV1::TypeChanged);
        assert_eq!(staged_change_kind('U'), StagedChangeKindV1::Unmerged);
        assert_eq!(staged_change_kind('R'), StagedChangeKindV1::Renamed);
        assert_eq!(staged_change_kind('C'), StagedChangeKindV1::Copied);
        assert_eq!(
            staged_change_kind('X'),
            StagedChangeKindV1::Unmerged,
            "an unrecognized status must fail closed via Unmerged, not panic or silently pass through"
        );
    }

    #[test]
    fn parse_raw_diff_z_rejects_truncated_or_malformed_input() {
        assert!(
            parse_raw_diff_z(":100644 100644 aaa bbb M\0").is_err(),
            "metadata with no following path"
        );
        assert!(
            parse_raw_diff_z("not-a-metadata-line\0path\0").is_err(),
            "missing leading colon"
        );
        assert!(parse_raw_diff_z(":bad M\0path\0").is_err(), "too few metadata fields");
    }

    #[test]
    fn staged_changes_since_reads_worktree_bytes_and_marks_deletions() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("VERSION"), b"1.2.3\r\n").unwrap();
        std::fs::create_dir_all(temp.path().join("pkg")).unwrap();
        let binary: Vec<u8> = (0..=255u8).collect();
        std::fs::write(temp.path().join("pkg/binary.bin"), &binary).unwrap();

        let base = CommitSha::parse(&"0".repeat(40)).unwrap();
        let raw = [
            ":000000 100644 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 A",
            "VERSION",
            ":000000 100644 0000000000000000000000000000000000000000 2222222222222222222222222222222222222222 A",
            "pkg/binary.bin",
            ":100644 000000 3333333333333333333333333333333333333333 0000000000000000000000000000000000000000 D",
            ".changeset/old-entry.md",
        ]
        .join("\0")
            + "\0";

        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(move |args| match args {
                ["diff", "--cached", "--raw", "-z", "--no-renames", sha] if sha == &"0".repeat(40) => {
                    Ok(ok(raw.clone()))
                }
                other => panic!("unexpected command: {other:?}"),
            }),
        };
        let git = ShellGit::new(&runner, temp.path());

        let changes = git.staged_changes_since(&base).unwrap();
        assert_eq!(changes.len(), 3);

        let version = changes.iter().find(|c| c.path == "VERSION").unwrap();
        assert_eq!(version.kind, StagedChangeKindV1::Added);
        assert_eq!(version.new_mode, Some(0o100644));
        assert_eq!(version.contents.as_deref(), Some(b"1.2.3\r\n".as_slice()));

        let bin = changes.iter().find(|c| c.path == "pkg/binary.bin").unwrap();
        assert_eq!(bin.contents.as_deref(), Some(binary.as_slice()));

        let deleted = changes.iter().find(|c| c.path == ".changeset/old-entry.md").unwrap();
        assert_eq!(deleted.kind, StagedChangeKindV1::Deleted);
        assert_eq!(deleted.new_mode, None);
        assert_eq!(deleted.contents, None, "a deletion must carry no contents to read");
    }
}
