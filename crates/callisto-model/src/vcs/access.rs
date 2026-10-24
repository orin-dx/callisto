//! Git access by shelling the real `git` binary through a
//! [`crate::CommandRunner`].

use std::path::{Path, PathBuf};

use crate::{
    ApplyPermit, CommandOutput, CommandRunner, CommitSha, StagedChangeKindV1, StagedChangeV1, TagHints, TagName,
};

use crate::vcs::{GitCommit, TagSignPolicy, VcsError};

/// Record separator placed immediately before each commit's fields in
/// `git log --format=` output, so a commit message containing `\n` can
/// never be mistaken for a record boundary.
const RECORD_SEP: char = '\u{1e}';

/// Unit separator between a commit's sha and its raw message body in
/// `git log --format=` output.
const FIELD_SEP: char = '\u{1f}';

/// One tag as [`GitAccess::tag_records`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRecord {
    pub name: TagName,
    pub commit: Option<CommitSha>,
    pub hints: Option<TagHints>,
}

/// Every Git read and write callisto performs, each one `git` subprocess
/// run in `root` through a [`CommandRunner`].
pub struct GitAccess<'r> {
    runner: &'r dyn CommandRunner,
    root: PathBuf,
}

/// Fresh Git checkout facts used to bind a durable release intent to the
/// exact repository it was validated against.
///
/// This is intentionally evidence, not a lossy `bool`: consumers must retain
/// the canonical root and complete `HEAD` they compare to source identity.
/// Its fields are private so only the VCS observation path can construct it.
#[derive(Debug, PartialEq, Eq)]
pub struct GitCommitTrustEvidence {
    canonical_root: PathBuf,
    head: CommitSha,
    head_disposition: GitHeadDisposition,
}

impl GitCommitTrustEvidence {
    pub(crate) fn new(canonical_root: PathBuf, head: CommitSha, head_disposition: GitHeadDisposition) -> Self {
        Self {
            canonical_root,
            head,
            head_disposition,
        }
    }

    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    pub fn head(&self) -> &CommitSha {
        &self.head
    }

    pub fn head_disposition(&self) -> GitHeadDisposition {
        self.head_disposition
    }

    /// The checkout facts a durable release intent is bound to.
    pub fn identity(&self) -> GitCommitTrustIdentity<'_> {
        GitCommitTrustIdentity {
            canonical_root: &self.canonical_root,
            head: &self.head,
            head_disposition: self.head_disposition,
        }
    }
}

/// Borrowed identity view of a [`GitCommitTrustEvidence`]; see
/// [`GitCommitTrustEvidence::identity`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitCommitTrustIdentity<'a> {
    pub canonical_root: &'a Path,
    pub head: &'a CommitSha,
    pub head_disposition: GitHeadDisposition,
}

/// Whether `HEAD` was attached to a symbolic reference when it was observed.
/// A detached checkout is valid when its full [`CommitSha`] matches the
/// release intent's source identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitHeadDisposition {
    Attached,
    Detached,
}

impl<'r> GitAccess<'r> {
    pub fn new(root: impl Into<PathBuf>, runner: &'r dyn CommandRunner) -> Self {
        GitAccess {
            runner,
            root: root.into(),
        }
    }

    /// Every staged (index) change relative to `base`, resolved from the repository's toplevel rather than
    /// `self.root` so this works from any subdirectory. Worktree bytes are read directly (for CRLF/binary
    /// fidelity) and verified against the index blob sha; a mismatch fails loudly instead of returning stale bytes.
    pub fn staged_changes_since(&self, base: &CommitSha) -> Result<Vec<StagedChangeV1>, VcsError> {
        let root = self.repo_root()?;

        let output = self.runner.run(
            "git",
            &[
                "diff",
                "--cached",
                "--raw",
                "-z",
                "--no-renames",
                "--no-abbrev",
                base.as_str(),
            ],
            &root,
        )?;
        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git diff --cached --raw` against `{}` failed: {}",
                base.as_str(),
                output.redacted_stderr()
            )));
        }

        let entries = parse_raw_diff_z(&output.stdout)?;
        let mut changes = Vec::with_capacity(entries.len());
        for entry in entries {
            let kind = staged_change_kind(entry.status);
            let contents = match kind {
                StagedChangeKindV1::Added | StagedChangeKindV1::Modified => {
                    let bytes = std::fs::read(root.join(&entry.path)).map_err(|error| {
                        VcsError::Git(format!("could not read staged file `{}`: {error}", entry.path))
                    })?;
                    self.verify_staged_blob(&root, &entry.path, &entry.new_sha)?;
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

    /// Resolves the repository's toplevel directory, whether `self.root` is that toplevel or a subdirectory.
    fn repo_root(&self) -> Result<PathBuf, VcsError> {
        self.resolved_toplevel(&format!("in `{}`", self.root.display()))
    }

    /// The repository's toplevel directory, via `git rev-parse --show-toplevel` -- honors
    /// `GIT_DIR`/`GIT_WORK_TREE`/`GIT_CEILING_DIRECTORIES` the way `git` itself does, unlike a
    /// hand-rolled walk up the filesystem looking for a `.git` entry.
    pub fn toplevel(&self) -> Result<PathBuf, VcsError> {
        self.repo_root()
    }

    /// The `git rev-parse --show-toplevel` resolution shared by every caller; `context` names the caller on failure.
    fn resolved_toplevel(&self, context: &str) -> Result<PathBuf, VcsError> {
        let output = self.runner.run("git", &["rev-parse", "--show-toplevel"], &self.root)?;
        if !output.success() {
            return Err(VcsError::Git(format!(
                "could not determine the Git repository root {context}: {}",
                output.redacted_stderr()
            )));
        }
        canonical_git_root(output.stdout_trimmed())
    }

    /// Confirms the worktree bytes just read for `path` hash to the same blob sha the index has staged, without
    /// round-tripping content through a lossy `String` capture.
    fn verify_staged_blob(&self, root: &Path, path: &str, expected_sha: &str) -> Result<(), VcsError> {
        let output = self.runner.run("git", &["hash-object", "--", path], root)?;
        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git hash-object` failed for `{path}` in `{}`: {}",
                root.display(),
                output.redacted_stderr()
            )));
        }
        let actual_sha = output.stdout_trimmed();
        if actual_sha != expected_sha {
            return Err(VcsError::StagedContentMismatch {
                path: path.to_string(),
                expected_sha: expected_sha.to_string(),
            });
        }
        Ok(())
    }

    /// Returns explicit, fresh Git trust evidence for a durable release.
    /// Errors omit raw stderr; ignored paths (build output) are allowed.
    pub fn observe_git_commit_trust(&self) -> Result<GitCommitTrustEvidence, VcsError> {
        let canonical_root = self.resolved_toplevel("for release trust")?;

        let object_format = self
            .runner
            .run("git", &["rev-parse", "--show-object-format"], &self.root)?;
        if !object_format.success() || object_format.stdout_trimmed() != "sha1" {
            return Err(VcsError::UnsupportedObjectFormat {
                found: object_format.stdout_trimmed().to_string(),
            });
        }
        if !self.has_full_history()? {
            return Err(VcsError::ShallowRepository);
        }

        let head = self
            .runner
            .run("git", &["rev-parse", "--verify", "HEAD^{commit}"], &self.root)?;
        if !head.success() {
            return Err(VcsError::Git(
                "could not resolve a commit for release trust".to_string(),
            ));
        }
        let head = CommitSha::parse(head.stdout_trimmed()).map_err(|error| VcsError::InvalidCommitSha {
            context: "for release trust".to_string(),
            message: error.to_string(),
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
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            &self.root,
        )?;
        if !status.success() {
            return Err(VcsError::Git(
                "could not inspect the worktree for release trust".to_string(),
            ));
        }
        check_release_worktree_status(&status.stdout)?;

        Ok(GitCommitTrustEvidence::new(canonical_root, head, head_disposition))
    }
}

/// One `git diff --raw -z` record, before Git's single-letter status is
/// mapped to a [`StagedChangeKindV1`] and file contents are read.
struct RawDiffEntry {
    path: String,
    /// The post-image Git file mode, or `None` for a pure deletion.
    new_mode: Option<u32>,
    /// The post-image blob sha staged for this path, verified against a worktree read in `verify_staged_blob`.
    new_sha: String,
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
            return Err(VcsError::MalformedDiffOutput {
                detail: format!("unexpected token, expected a metadata line starting with `:`: {meta:?}"),
            });
        };
        let path = tokens.next().ok_or_else(|| VcsError::MalformedDiffOutput {
            detail: "truncated output: missing a path after its metadata line".to_string(),
        })?;
        let fields: Vec<&str> = meta.split(' ').collect();
        let [_old_mode, new_mode, _old_sha, new_sha, status_field] = fields.as_slice() else {
            return Err(VcsError::MalformedDiffOutput {
                detail: format!("malformed metadata line: {meta:?}"),
            });
        };
        let status = status_field
            .chars()
            .next()
            .ok_or_else(|| VcsError::MalformedDiffOutput {
                detail: format!("empty status in metadata line: {meta:?}"),
            })?;
        let new_mode = if status == 'D' {
            None
        } else {
            Some(
                u32::from_str_radix(new_mode, 8).map_err(|error| VcsError::MalformedDiffOutput {
                    detail: format!("invalid Git mode `{new_mode}`: {error}"),
                })?,
            )
        };
        entries.push(RawDiffEntry {
            path: path.to_string(),
            new_mode,
            new_sha: (*new_sha).to_string(),
            status,
        });
    }
    Ok(entries)
}

/// Maps a `git diff --raw` single-letter status to a [`StagedChangeKindV1`].
/// Any status this codebase does not have a named case for (Git's own docs
/// list `X` as "unknown") conservatively maps to `Unmerged`, which
/// [`crate::ReleasePrCommitPlanV1::from_changes`] always rejects --
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
        return Err(VcsError::InvalidRepositoryRoot {
            detail: "Git returned an empty repository root".to_string(),
        });
    }
    dunce::canonicalize(raw_root).map_err(|error| VcsError::InvalidRepositoryRoot {
        detail: format!("could not canonicalize `{raw_root}`: {error}"),
    })
}

/// Ignored paths are build output and never block; tracked or untracked changes do.
fn check_release_worktree_status(status: &str) -> Result<(), VcsError> {
    match status.split('\0').find(|record| !record.is_empty()) {
        None => Ok(()),
        Some(record) => Err(VcsError::DirtyWorktree {
            path: record.get(3..).unwrap_or(record).to_string(),
        }),
    }
}

/// A Git probe that failed for a reason other than "the thing asked about is absent".
fn probe_failed(args: &[&str], root: &Path, output: &CommandOutput) -> VcsError {
    VcsError::Git(format!(
        "`git {}` failed in `{}`: {}",
        args.join(" "),
        root.display(),
        output.redacted_stderr()
    ))
}

impl GitAccess<'_> {
    /// Returns the commit SHA that `HEAD` currently resolves to.
    pub fn head_sha(&self) -> Result<CommitSha, VcsError> {
        let output = self.runner.run("git", &["rev-parse", "HEAD"], &self.root)?;
        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git rev-parse HEAD` failed in `{}`: {}",
                self.root.display(),
                output.redacted_stderr()
            )));
        }
        let sha_str = output.stdout_trimmed();
        CommitSha::parse(sha_str).map_err(|e| VcsError::InvalidCommitSha {
            context: format!("in `git rev-parse HEAD` output `{sha_str}`"),
            message: e.to_string(),
        })
    }

    /// The committer time of `rev` in whole seconds since the epoch.
    pub fn commit_time(&self, rev: &str) -> Result<u64, VcsError> {
        let output = self.runner.run(
            "git",
            &["show", "-s", "--no-show-signature", "--format=%ct", rev],
            &self.root,
        )?;
        let stdout = output.stdout_trimmed();
        match stdout.parse::<u64>() {
            Ok(seconds) if output.success() => Ok(seconds),
            _ => Err(VcsError::Git(format!(
                "`git show -s --no-show-signature --format=%ct {rev}` failed in `{}`: {}",
                self.root.display(),
                output.redacted_stderr()
            ))),
        }
    }

    /// Whether `path` (relative to this root) exists in `rev`'s tree; a revision that does not resolve has no paths.
    pub fn path_exists_at(&self, rev: &str, path: &Path) -> Result<bool, VcsError> {
        let spec = format!("{rev}:./{}", path.display());
        let args = ["rev-parse", "--verify", "--quiet", &spec];
        let output = self.runner.run("git", &args, &self.root)?;
        if output.success() {
            return Ok(true);
        }
        if output.exit_code == Some(1) {
            return Ok(false);
        }
        Err(probe_failed(&args, &self.root, &output))
    }

    /// Files in `rev`'s tree matching one of `pathspecs` whose text contains `needle`, ignoring case.
    ///
    /// No match is an empty list; a revision that cannot be searched is an error.
    pub fn files_containing_at(&self, rev: &str, needle: &str, pathspecs: &[&str]) -> Result<Vec<PathBuf>, VcsError> {
        let mut args = vec!["grep", "-l", "-z", "-F", "-i", "-e", needle, rev, "--"];
        args.extend_from_slice(pathspecs);
        let output = self.runner.run("git", &args, &self.root)?;
        if output.exit_code == Some(1) {
            return Ok(Vec::new());
        }
        if !output.success() {
            return Err(probe_failed(&args, &self.root, &output));
        }
        let prefix = format!("{rev}:");
        Ok(output
            .stdout
            .split('\0')
            .filter_map(|entry| entry.strip_prefix(&prefix))
            .map(PathBuf::from)
            .collect())
    }

    /// The text of `path` (relative to this root) in `rev`'s tree, decoded lossily; `None` when it is absent.
    pub fn read_at(&self, rev: &str, path: &Path) -> Result<Option<String>, VcsError> {
        if !self.path_exists_at(rev, path)? {
            return Ok(None);
        }
        let spec = format!("{rev}:./{}", path.display());
        let args = ["show", &spec];
        let output = self.runner.run("git", &args, &self.root)?;
        if !output.success() {
            return Err(probe_failed(&args, &self.root, &output));
        }
        Ok(Some(output.stdout))
    }

    /// The most recent commit that added `path`, if it is committed.
    pub fn commit_adding(&self, path: &Path) -> Result<Option<String>, VcsError> {
        // A repository with no commits has no history to search; `git log` would fail there.
        if self.resolve_commit("HEAD")?.is_none() {
            return Ok(None);
        }
        let path = path.to_string_lossy();
        let args = [
            "log",
            "--no-show-signature",
            "--diff-filter=A",
            "--format=%H",
            "-1",
            "--",
            &path,
        ];
        let output = self.runner.run("git", &args, &self.root)?;
        if !output.success() {
            return Err(probe_failed(&args, &self.root, &output));
        }
        let sha = output.stdout_trimmed();
        Ok((!sha.is_empty()).then(|| sha.to_string()))
    }

    /// The commits on HEAD's first-parent line that changed `pathspecs`, newest first. A merge commit counts when
    /// it changes them relative to its first parent, so a squash or merge landing a release is one entry.
    pub fn first_parent_commits_touching(&self, pathspecs: &[PathBuf]) -> Result<Vec<String>, VcsError> {
        let mut args: Vec<String> = ["log", "--no-show-signature", "--first-parent", "--format=%H", "--"]
            .map(str::to_owned)
            .into();
        args.extend(pathspecs.iter().map(|path| path.to_string_lossy().into_owned()));
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = self.runner.run("git", &arg_refs, &self.root)?;
        if !output.success() {
            return Err(probe_failed(&arg_refs, &self.root, &output));
        }
        Ok(output.stdout_lines().map(str::to_owned).collect())
    }

    /// Whether `pathspecs` hold identical content in the trees of `a` and `b`.
    pub fn same_content_at(&self, a: &str, b: &str, pathspecs: &[PathBuf]) -> Result<bool, VcsError> {
        let mut args: Vec<String> = ["diff", "--quiet", "--no-ext-diff", a, b, "--"]
            .map(str::to_owned)
            .into();
        args.extend(pathspecs.iter().map(|path| path.to_string_lossy().into_owned()));
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = self.runner.run("git", &arg_refs, &self.root)?;
        match output.exit_code {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(probe_failed(&arg_refs, &self.root, &output)),
        }
    }

    /// Lists tag names, optionally filtered by `glob` (a [`globset::Glob`]
    /// pattern; `None` matches every tag).
    ///
    /// Always fetches the full tag list and filters locally with the same
    /// `globset` matcher `callisto-graph`'s `tags::matching_tags` uses, never
    /// `git tag --list <pattern>`'s different glob dialect.
    /// Whether the repository holds its whole history. A shallow clone does not, and a failing answer is not "yes".
    pub fn has_full_history(&self) -> Result<bool, VcsError> {
        let shallow = self
            .runner
            .run("git", &["rev-parse", "--is-shallow-repository"], &self.root)?;
        Ok(shallow.success() && shallow.stdout_trimmed() == "false")
    }

    /// Fails with [`VcsError::ShallowHistory`] unless the repository holds its whole history, which commit-based
    /// inference needs: a shallow clone makes a window look shorter than it is and the bump smaller.
    pub fn require_full_history(&self) -> Result<(), VcsError> {
        if self.has_full_history()? {
            Ok(())
        } else {
            Err(VcsError::ShallowHistory)
        }
    }

    pub fn list_tags(&self, glob: Option<&str>) -> Result<Vec<TagName>, VcsError> {
        let output = self.runner.run("git", &["tag", "--list"], &self.root)?;
        if !output.success() {
            return Err(probe_failed(&["tag", "--list"], &self.root, &output));
        }
        let all = output.stdout_lines().map(|s| s.to_string());

        let matcher = glob.map(crate::vcs::compile_tag_glob).transpose()?;

        Ok(all
            .filter(|t| matcher.as_ref().is_none_or(|m| m.is_match(t)))
            // A legal Git ref is not necessarily safe as a bare CLI positional (a leading `-`).
            .filter_map(|t| TagName::parse(&t).ok())
            .collect())
    }

    /// Every tag with the commit it ends at and the hints in its annotation, from one `git for-each-ref`. The commit
    /// is `None` when the tag ends at something else (a tree, a blob, a tag of a tag), which [`Self::resolve_commit`]
    /// then decides. The hints are `None` for a lightweight tag and for an annotation without well-formed hints.
    pub fn tag_records(&self) -> Result<Vec<TagRecord>, VcsError> {
        let args = [
            "for-each-ref",
            "--format=%(refname:strip=2)%00%(objecttype)%00%(objectname)%00%(*objecttype)%00%(*objectname)%00\
             %(contents:trailers:only,unfold)%1e",
            "refs/tags",
        ];
        let output = self.runner.run("git", &args, &self.root)?;
        if !output.success() {
            return Err(probe_failed(&args, &self.root, &output));
        }
        Ok(output
            .stdout
            .split(RECORD_SEP)
            .filter_map(|record| {
                let mut fields = record.trim_start_matches('\n').splitn(6, '\0');
                let name = TagName::parse(fields.next()?).ok()?;
                let (kind, object, peeled_kind, peeled, trailers) = (
                    fields.next()?,
                    fields.next()?,
                    fields.next()?,
                    fields.next()?,
                    fields.next()?,
                );
                let commit = match (kind, peeled_kind) {
                    ("commit", _) => CommitSha::parse(object).ok(),
                    ("tag", "commit") => CommitSha::parse(peeled).ok(),
                    _ => None,
                };
                // A lightweight tag's contents are its commit's message, which says nothing about the tag.
                let hints = if kind == "tag" { TagHints::parse(trailers) } else { None };
                Some(TagRecord { name, commit, hints })
            })
            .collect())
    }

    /// The directory every git command runs in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The runner every git command goes through.
    pub fn runner(&self) -> &dyn CommandRunner {
        self.runner
    }

    /// Resolves `refname` (tag, branch, or partial/full SHA) to the commit
    /// it points at. An unresolvable ref resolves to `Ok(None)`, which
    /// callers treat as "no bound" / "infer over full history".
    pub fn resolve_commit(&self, refname: &str) -> Result<Option<CommitSha>, VcsError> {
        let rev = format!("{refname}^{{commit}}");
        let args = ["rev-parse", "--verify", "--quiet", &rev];
        let output = self.runner.run("git", &args, &self.root)?;

        if output.exit_code == Some(1) {
            return Ok(None);
        }
        if !output.success() {
            return Err(probe_failed(&args, &self.root, &output));
        }
        let sha_str = output.stdout_trimmed();
        if sha_str.is_empty() {
            return Ok(None);
        }
        Ok(CommitSha::parse(sha_str).ok())
    }

    /// Lists non-merge commits reachable from `HEAD`, down to (exclusive)
    /// `since_ref` when given, keeping those that touch at least one of
    /// `pathspecs` (empty keeps every commit).
    ///
    /// `--full-history`: a commit touching a pathspec counts even when a
    /// later merge discarded its change (e.g. `merge -s ours`); default
    /// history simplification would silently drop it.
    ///
    /// A `since_ref` that is given but does not resolve is
    /// [`VcsError::RefNotFound`], never a silent unbounded walk.
    pub fn commits_since(&self, since_ref: Option<&str>, pathspecs: &[PathBuf]) -> Result<Vec<GitCommit>, VcsError> {
        let mut args: Vec<String> = vec![
            "log".to_string(),
            "--no-show-signature".to_string(),
            "--no-merges".to_string(),
            "--full-history".to_string(),
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
            if let Some(r) = since_ref
                && self.resolve_commit(r)?.is_none()
            {
                return Err(VcsError::RefNotFound {
                    ref_name: r.to_string(),
                });
            }
            return Err(VcsError::Git(format!(
                "`git log` failed in `{}`: {}",
                self.root.display(),
                output.redacted_stderr()
            )));
        }

        parse_git_log_output(&output.stdout, &self.root)
    }

    /// Resolves every ref in `refs` to its commit with one `git rev-parse`.
    /// `None` when any of them does not resolve.
    pub fn resolve_commits(&self, refs: &[String]) -> Result<Option<Vec<CommitSha>>, VcsError> {
        let revs: Vec<String> = refs.iter().map(|r| format!("{r}^{{commit}}")).collect();
        let mut args = vec!["rev-parse"];
        args.extend(revs.iter().map(String::as_str));
        let output = self.runner.run("git", &args, &self.root)?;
        if !output.success() {
            // The batch exits 128 for a missing ref and for a broken repository alike; one probe each tells them apart.
            for r in refs {
                if self.resolve_commit(r)?.is_none() {
                    return Ok(None);
                }
            }
            return Err(probe_failed(&args, &self.root, &output));
        }
        let shas: Option<Vec<CommitSha>> = output.stdout_lines().map(|l| CommitSha::parse(l).ok()).collect();
        Ok(shas.filter(|shas| shas.len() == refs.len()))
    }

    /// Paths, relative to the root and limited to it, that non-merge commits
    /// in `since_ref..HEAD` touched. Selects exactly what
    /// [`Self::commits_since`] filters on: `--full-history`, and a rename
    /// counts for both its old and new path.
    pub fn paths_changed_since(&self, since_ref: &str) -> Result<Vec<PathBuf>, VcsError> {
        let range = format!("{since_ref}..HEAD");
        self.name_only(&[
            "log",
            "--no-show-signature",
            "--no-merges",
            "--full-history",
            "--no-renames",
            "--relative",
            "--name-only",
            "-z",
            "--format=",
            &range,
            "--",
        ])
    }

    /// Tracked paths, relative to the root and limited to it, whose worktree
    /// contents differ from `since_ref`.
    pub fn paths_differing_from(&self, since_ref: &str) -> Result<Vec<PathBuf>, VcsError> {
        self.name_only(&[
            "diff",
            "--no-renames",
            "--relative",
            "--name-only",
            "-z",
            since_ref,
            "--",
        ])
    }

    /// The object id each `<revision>:<path>` names, `None` for one that names nothing, from one `git cat-file
    /// --batch-check`. An empty path names the revision's root tree. Two specs name the same content exactly when their
    /// ids are equal, so this compares directories at two commits without reading either.
    pub fn object_ids(&self, specs: &[String]) -> Result<Vec<Option<String>>, VcsError> {
        if specs.is_empty() {
            return Ok(Vec::new());
        }
        let args = ["cat-file", "--batch-check"];
        let stdin = specs.join("\n") + "\n";
        let output = self.runner.run_with_stdin("git", &args, &self.root, stdin.as_bytes())?;
        if !output.success() {
            return Err(probe_failed(&args, &self.root, &output));
        }
        let malformed = |detail: String| VcsError::MalformedLogOutput {
            detail: format!("in `{}`: {detail}", self.root.display()),
        };
        let lines: Vec<&str> = output.stdout.lines().collect();
        if lines.len() != specs.len() {
            return Err(malformed(format!(
                "`git cat-file --batch-check` answered {} lines for {} objects",
                lines.len(),
                specs.len()
            )));
        }
        specs
            .iter()
            .zip(lines)
            .map(|(spec, line)| {
                if line.strip_prefix(spec.as_str()) == Some(" missing") {
                    return Ok(None);
                }
                match line.split(' ').next() {
                    Some(id) if CommitSha::parse(id).is_ok() => Ok(Some(id.to_owned())),
                    _ => Err(malformed(format!("`{spec}` was answered with `{line}`"))),
                }
            })
            .collect()
    }

    fn name_only(&self, args: &[&str]) -> Result<Vec<PathBuf>, VcsError> {
        let output = self.runner.run("git", args, &self.root)?;
        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git {}` failed in `{}`: {}",
                args[0],
                self.root.display(),
                output.redacted_stderr()
            )));
        }
        Ok(output
            .stdout
            .split('\0')
            .map(|path| path.trim_start_matches('\n'))
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect())
    }

    /// Creates the tag `name` at `target_sha`: annotated with `message`
    /// when `Some`, lightweight when `None`. Fails if the tag already
    /// exists. Writes a ref, so it requires an [`ApplyPermit`].
    pub fn create_tag(
        &self,
        name: &str,
        target_sha: &CommitSha,
        message: Option<&str>,
        sign: TagSignPolicy,
        _permit: &ApplyPermit,
    ) -> Result<(), VcsError> {
        let identity = match message {
            Some(_) => self.tagger_identity_override(target_sha)?,
            None => None,
        };
        let mut args: Vec<&str> = Vec::new();
        if let Some([name_config, email_config]) = &identity {
            args.extend(["-c", name_config.as_str(), "-c", email_config.as_str()]);
        }
        // `--` ends option parsing so `name` can never be misread as a flag.
        args.push("tag");
        if let Some(msg) = message {
            args.extend(["-a", "-m", msg]);
        }
        if matches!(sign, TagSignPolicy::ForceUnsigned) {
            args.push("--no-sign");
        }
        args.extend(["--", name, target_sha.as_str()]);
        let output = self.runner.run("git", &args, &self.root)?;
        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git tag` failed in `{}`: {}",
                self.root.display(),
                output.redacted_stderr()
            )));
        }
        Ok(())
    }

    /// `None` when Git can resolve a committer identity itself. Otherwise the
    /// target commit's committer, as `-c` overrides, since an annotated tag
    /// needs a tagger and a release runner's checkout often configures none.
    fn tagger_identity_override(&self, target_sha: &CommitSha) -> Result<Option<[String; 2]>, VcsError> {
        let ident = self.runner.run("git", &["var", "GIT_COMMITTER_IDENT"], &self.root)?;
        if ident.success() {
            return Ok(None);
        }
        let commit = self
            .runner
            .run("git", &["cat-file", "commit", target_sha.as_str()], &self.root)?;
        let committer = commit
            .stdout
            .lines()
            .take_while(|line| !line.is_empty())
            .find_map(|line| line.strip_prefix("committer "))
            .and_then(|ident| ident.split_once(" <"))
            .and_then(|(name, rest)| Some((name, rest.split_once('>')?.0)));
        match committer {
            Some((name, email)) if commit.success() && !name.is_empty() && !email.is_empty() => {
                Ok(Some([format!("user.name={name}"), format!("user.email={email}")]))
            }
            _ => Err(VcsError::Git(format!(
                "no Git identity for the tagger and could not read the committer of `{}`: {}",
                target_sha.as_str(),
                commit.redacted_stderr()
            ))),
        }
    }
}

/// Parses the `<RECORD_SEP>%H<FIELD_SEP>%B`-formatted `git log` output
/// produced by [`GitAccess::commits_since`] into [`GitCommit`]s, splitting
/// each raw message on its first blank line into `summary`/`body`.
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
            return Err(VcsError::MalformedLogOutput {
                detail: format!(
                    "in `{}`: expected a `<sha>{FIELD_SEP:?}<message>` record, got: {record:?}",
                    cwd.display()
                ),
            });
        };

        let sha = CommitSha::parse(sha_str).map_err(|e| VcsError::InvalidCommitSha {
            context: format!("in `git log` output in `{}`", cwd.display()),
            message: e.to_string(),
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
    fn permit() -> crate::ApplyPermit {
        crate::ApplyPermit::force_for_tests()
    }
    use super::*;
    use crate::{CommandError, CommandOutput};
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

        fn run_with_stdin(
            &self,
            program: &str,
            args: &[&str],
            cwd: &Path,
            _stdin: &[u8],
        ) -> Result<CommandOutput, CommandError> {
            self.run(program, args, cwd)
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
            ["status", "--porcelain=v1", "-z", "--untracked-files=all"] => Ok(ok(status.clone())),
            other => panic!("unexpected Git trust command: {other:?}"),
        })
    }

    #[test]
    fn git_commit_trust_evidence_is_root_bound() {
        let temp = tempfile::tempdir().unwrap();
        let expected_root = dunce::canonicalize(temp.path()).unwrap();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: trust_response(temp.path().to_path_buf(), String::new(), true),
        };
        let git = GitAccess::new(temp.path(), &runner);

        let evidence = git.observe_git_commit_trust().unwrap();

        assert_eq!(evidence.canonical_root(), expected_root);
        assert_eq!(evidence.head().as_str(), "a".repeat(40));
        assert_eq!(evidence.head_disposition(), GitHeadDisposition::Detached);
    }

    #[test]
    fn git_commit_trust_rejects_untracked_or_tracked_worktree_entries() {
        let temp = tempfile::tempdir().unwrap();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: trust_response(temp.path().to_path_buf(), "?? release-input.txt\0".to_string(), false),
        };
        let git = GitAccess::new(temp.path(), &runner);

        let error = git
            .observe_git_commit_trust()
            .expect_err("untracked source must reject trust");

        assert!(matches!(error, VcsError::DirtyWorktree { path } if path == "release-input.txt"));
    }

    #[test]
    fn git_commit_trust_names_a_modified_tracked_file() {
        let temp = tempfile::tempdir().unwrap();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: trust_response(temp.path().to_path_buf(), " M Cargo.toml\0".to_string(), true),
        };
        let error = GitAccess::new(temp.path(), &runner)
            .observe_git_commit_trust()
            .expect_err("modified tracked source must reject trust");
        assert!(matches!(error, VcsError::DirtyWorktree { path } if path == "Cargo.toml"));
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
            let error = GitAccess::new(temp.path(), &runner)
                .observe_git_commit_trust()
                .expect_err("unsupported repository trust evidence must reject");
            assert!(error.to_string().contains(response), "got: {error}");
        }
    }

    /// One `for-each-ref` record the way git prints it: fields split by NUL, the trailers last, then the separator.
    fn record(name: &str, kind: &str, object: &str, peeled_kind: &str, peeled: &str, trailers: &str) -> String {
        format!("{name}\0{kind}\0{object}\0{peeled_kind}\0{peeled}\0{trailers}\x1e\n")
    }

    #[test]
    fn tag_records_reads_each_commit_and_only_an_annotated_tags_hints() {
        let (a, b, c) = ("a".repeat(40), "b".repeat(40), "c".repeat(40));
        let tree = "d".repeat(40);
        let hints = format!("Callisto-Hints: 1\nCallisto-Package: cargo/core\nCallisto-Tree: crates/core {tree}\n");
        let stdout = [
            record("core@1.0.0", "tag", &a, "commit", &b, &hints),
            // A lightweight tag's contents are its commit's message, so hints there say nothing about the tag.
            record("core@0.9.0", "commit", &c, "", "", &hints),
            record("core@0.8.0", "tag", &a, "commit", &b, "Signed-off-by: someone\n"),
            record("blob", "blob", &a, "", "", ""),
        ]
        .concat();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(move |_args| Ok(ok(stdout.clone()))),
        };
        let git = GitAccess::new(PathBuf::from("."), &runner);

        let records = git.tag_records().unwrap();

        let by_name = |name: &str| records.iter().find(|r| r.name.as_str() == name).unwrap();
        assert_eq!(records.len(), 4);
        assert_eq!(
            by_name("core@1.0.0").commit.as_ref().map(CommitSha::as_str),
            Some(b.as_str())
        );
        let parsed = by_name("core@1.0.0").hints.as_ref().expect("an annotated tag's hints");
        assert_eq!(parsed.package, "cargo/core");
        assert_eq!(parsed.trees["crates/core"], tree);
        assert_eq!(
            by_name("core@0.9.0").commit.as_ref().map(CommitSha::as_str),
            Some(c.as_str())
        );
        assert_eq!(by_name("core@0.9.0").hints, None);
        assert_eq!(by_name("core@0.8.0").hints, None);
        assert_eq!(by_name("blob").commit, None);
        assert_eq!(runner.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn tag_records_fails_when_git_does() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| {
                Ok(CommandOutput {
                    exit_code: Some(128),
                    stdout: String::new(),
                    stderr: "fatal".into(),
                })
            }),
        };
        let git = GitAccess::new(PathBuf::from("."), &runner);

        assert!(git.tag_records().is_err());
    }

    #[test]
    fn test_list_tags_fetches_unfiltered_and_filters_locally_with_globset() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok("pkg-a@1.0.0\npkg-ab@1.0.0\nunrelated\n"))),
        };
        let git = GitAccess::new(PathBuf::from("."), &runner);

        let tags = git.list_tags(Some("pkg-a@*")).unwrap();

        assert_eq!(
            tags.into_iter().map(|t| t.as_str().to_string()).collect::<Vec<_>>(),
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
        let git = GitAccess::new(PathBuf::from("."), &runner);

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
        let git = GitAccess::new(PathBuf::from("."), &runner);

        let result = git.list_tags(None);

        assert!(
            matches!(result, Err(VcsError::Git(ref msg)) if msg.contains("unable to read current working directory")),
            "expected Err(VcsError::Git(..)) mentioning the failure, got: {result:?}"
        );
    }

    /// There is no repository to ask for tags, so "no tags" would be a made-up answer.
    #[test]
    fn test_list_tags_errors_when_there_is_no_git_repository() {
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
        let git = GitAccess::new(PathBuf::from("."), &runner);

        assert!(matches!(git.list_tags(None), Err(VcsError::Git(_))));
    }

    /// Runs the real `git`, which is the only way to know which exit code means "absent" and which means "broken".
    struct RealGit;

    impl CommandRunner for RealGit {
        fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
            let output = std::process::Command::new(program)
                .args(args)
                .current_dir(cwd)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .env("GIT_CEILING_DIRECTORIES", cwd.parent().unwrap_or(cwd))
                .output()
                .map_err(|error| CommandError::Io {
                    program: program.to_string(),
                    message: error.to_string(),
                })?;
            Ok(CommandOutput {
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
    }

    fn real_git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
            ])
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn probes_tell_an_absent_answer_from_a_failure_against_real_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        real_git(&root, &["init", "-q"]);
        let git = GitAccess::new(root.clone(), &RealGit);

        // No commits yet: nothing resolves, nothing was added, and none of that is an error.
        assert_eq!(git.resolve_commit("HEAD").unwrap(), None);
        assert_eq!(git.commit_adding(Path::new("a.txt")).unwrap(), None);

        std::fs::write(root.join("a.txt"), "hello needle").unwrap();
        real_git(&root, &["add", "a.txt"]);
        real_git(&root, &["commit", "-q", "-m", "one"]);

        assert!(git.resolve_commit("HEAD").unwrap().is_some());
        assert_eq!(git.resolve_commit("no-such-ref").unwrap(), None);
        assert!(git.path_exists_at("HEAD", Path::new("a.txt")).unwrap());
        assert!(!git.path_exists_at("HEAD", Path::new("nope.txt")).unwrap());
        assert!(!git.path_exists_at("no-such-ref", Path::new("a.txt")).unwrap());
        assert_eq!(
            git.read_at("HEAD", Path::new("a.txt")).unwrap().as_deref(),
            Some("hello needle")
        );
        assert_eq!(git.read_at("HEAD", Path::new("nope.txt")).unwrap(), None);
        assert_eq!(
            git.files_containing_at("HEAD", "NEEDLE", &["*.txt"]).unwrap(),
            [PathBuf::from("a.txt")]
        );
        assert!(git
            .files_containing_at("HEAD", "absent", &["*.txt"])
            .unwrap()
            .is_empty());
        assert!(git.commit_adding(Path::new("a.txt")).unwrap().is_some());
        assert_eq!(git.commit_adding(Path::new("nope.txt")).unwrap(), None);
        let heads = ["HEAD".to_string(), "no-such-ref".to_string()];
        assert_eq!(git.resolve_commits(&heads).unwrap(), None);
        assert!(git.resolve_commits(&heads[..1]).unwrap().is_some());

        // A searched revision that does not exist is a failure, not "no matches".
        assert!(git.files_containing_at("no-such-ref", "needle", &["*.txt"]).is_err());
    }

    #[test]
    fn probes_fail_loudly_outside_a_git_repository_against_real_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let git = GitAccess::new(root, &RealGit);

        assert!(git.resolve_commit("HEAD").is_err());
        assert!(git.path_exists_at("HEAD", Path::new("a.txt")).is_err());
        assert!(git.read_at("HEAD", Path::new("a.txt")).is_err());
        assert!(git.files_containing_at("HEAD", "x", &["*"]).is_err());
        assert!(git.commit_adding(Path::new("a.txt")).is_err());
        assert!(git.list_tags(None).is_err());
        assert!(git.resolve_commits(&["HEAD".to_string()]).is_err());
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
        let git = GitAccess::new(PathBuf::from("."), &runner);

        assert_eq!(git.resolve_commit("missing-tag").unwrap(), None);
    }

    #[test]
    fn test_resolve_commit_parses_sha_on_success() {
        let sha = "a".repeat(40);
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(move |_args| Ok(ok(format!("{}\n", "a".repeat(40))))),
        };
        let git = GitAccess::new(PathBuf::from("."), &runner);

        assert_eq!(
            git.resolve_commit("v1.0.0").unwrap(),
            Some(CommitSha::parse(&sha).unwrap())
        );
    }

    /// An empty `git rev-parse --show-toplevel` result is a distinct, typed
    /// [`VcsError::InvalidRepositoryRoot`] (E303), not the generic `VcsError::Git`.
    #[test]
    fn canonical_git_root_rejects_an_empty_root() {
        let err = canonical_git_root("").expect_err("empty root must fail");
        assert!(matches!(err, VcsError::InvalidRepositoryRoot { .. }), "got {err:?}");
        assert_eq!(
            miette::Diagnostic::code(&err).map(|c| c.to_string()),
            Some("E303".to_string())
        );
    }

    /// `git rev-parse HEAD` returning a value that is not a valid commit SHA is a
    /// distinct, typed [`VcsError::InvalidCommitSha`] (E302), not the generic `VcsError::Git`.
    #[test]
    fn head_sha_rejects_a_malformed_sha() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok("not-a-sha\n"))),
        };
        let git = GitAccess::new(PathBuf::from("."), &runner);
        let err = git.head_sha().expect_err("malformed SHA must fail");
        assert!(matches!(err, VcsError::InvalidCommitSha { .. }), "got {err:?}");
        assert_eq!(
            miette::Diagnostic::code(&err).map(|c| c.to_string()),
            Some("E302".to_string())
        );
    }

    /// A runner that pipes input, answering `cat-file --batch-check` with `answer`.
    fn batch_check_runner(answer: String) -> FakeRunner {
        FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(move |_args| Ok(ok(answer.clone()))),
        }
    }

    #[test]
    fn object_ids_reports_each_id_and_each_missing_object_in_order() {
        let id = "a".repeat(40);
        let runner = batch_check_runner(format!("{id} tree 100\nHEAD:gone missing\n"));
        let git = GitAccess::new(PathBuf::from("."), &runner);
        let specs = vec!["HEAD:crates/a".to_owned(), "HEAD:gone".to_owned()];

        assert_eq!(git.object_ids(&specs).unwrap(), vec![Some(id), None]);
    }

    #[test]
    fn object_ids_rejects_an_answer_that_is_not_one_line_per_object() {
        let runner = batch_check_runner(format!("{} tree 100\n", "a".repeat(40)));
        let git = GitAccess::new(PathBuf::from("."), &runner);
        let specs = vec!["HEAD:a".to_owned(), "HEAD:b".to_owned()];

        let err = git.object_ids(&specs).expect_err("a short answer must fail");
        assert!(matches!(err, VcsError::MalformedLogOutput { .. }), "got {err:?}");
    }

    #[test]
    fn object_ids_rejects_an_answer_that_is_not_an_object_id() {
        let runner = batch_check_runner("HEAD:a ambiguous\n".to_owned());
        let git = GitAccess::new(PathBuf::from("."), &runner);

        let err = git
            .object_ids(&["HEAD:a".to_owned()])
            .expect_err("an ambiguous name must fail");
        assert!(matches!(err, VcsError::MalformedLogOutput { .. }), "got {err:?}");
    }

    /// `git log`'s custom `--format=` output missing the `FIELD_SEP`-delimited message is a
    /// distinct, typed [`VcsError::MalformedLogOutput`] (E301).
    #[test]
    fn commits_since_rejects_a_record_with_no_field_separator() {
        let stdout = format!("{RECORD_SEP}{}", "a".repeat(40));
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(move |_args| Ok(ok(stdout.clone()))),
        };
        let git = GitAccess::new(PathBuf::from("."), &runner);
        let err = git
            .commits_since(None, &[])
            .expect_err("missing field separator must fail");
        assert!(matches!(err, VcsError::MalformedLogOutput { .. }), "got {err:?}");
        assert_eq!(
            miette::Diagnostic::code(&err).map(|c| c.to_string()),
            Some("E301".to_string())
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
        let git = GitAccess::new(PathBuf::from("."), &runner);

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
        let git = GitAccess::new(PathBuf::from("."), &runner);

        let sha = "c".repeat(40);
        git.commits_since(Some(&sha), &[PathBuf::from("crates/pkg-a")]).unwrap();

        let calls = runner.calls.lock().unwrap();
        let args = &calls[0];
        assert!(args.contains(&"--no-merges".to_string()));
        assert!(args.contains(&"--full-history".to_string()));
        assert!(args.contains(&format!("{sha}..HEAD")));
        assert!(args.contains(&"--".to_string()));
        assert!(args.contains(&"crates/pkg-a".to_string()));
    }

    #[test]
    fn test_commits_since_propagates_git_log_failure_instead_of_unbounded_walk() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|args| {
                Ok(if args[0] == "rev-parse" {
                    CommandOutput {
                        exit_code: Some(1),
                        stdout: String::new(),
                        stderr: String::new(),
                    }
                } else {
                    CommandOutput {
                        exit_code: Some(128),
                        stdout: String::new(),
                        stderr: "fatal: bad revision 'missing-ref..HEAD'".to_string(),
                    }
                })
            }),
        };
        let git = GitAccess::new(PathBuf::from("."), &runner);

        let result = git.commits_since(Some("missing-ref"), &[]);

        assert!(
            matches!(result, Err(VcsError::RefNotFound { .. })),
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
        let git = GitAccess::new(PathBuf::from("."), &runner);
        let sha = CommitSha::parse(&"d".repeat(40)).unwrap();

        git.create_tag(
            "pkg-a@1.0.0",
            &sha,
            Some("Release pkg-a@1.0.0"),
            crate::vcs::TagSignPolicy::RespectRepoConfig,
            &permit(),
        )
        .unwrap();

        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls[0], vec!["var", "GIT_COMMITTER_IDENT"]);
        assert_eq!(
            calls[1],
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
        let git = GitAccess::new(PathBuf::from("."), &runner);
        let sha = CommitSha::parse(&"e".repeat(40)).unwrap();

        git.create_tag(
            "pkg-a@1.0.0",
            &sha,
            None,
            crate::vcs::TagSignPolicy::RespectRepoConfig,
            &permit(),
        )
        .unwrap();

        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls[0], vec!["tag", "--", "pkg-a@1.0.0", sha.as_str()]);
    }

    /// `ForceUnsigned` must add `--no-sign` before the `--` argv separator,
    /// alongside it rather than instead of it -- both safety properties
    /// (the durable release executor's CI-signing workaround and the
    /// flag-injection guard) must hold together.
    #[test]
    fn test_create_tag_force_unsigned_adds_no_sign_before_separator() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok(""))),
        };
        let git = GitAccess::new(PathBuf::from("."), &runner);
        let sha = CommitSha::parse(&"a".repeat(40)).unwrap();

        git.create_tag(
            "pkg-a@1.0.0",
            &sha,
            Some("Release pkg-a@1.0.0"),
            crate::vcs::TagSignPolicy::ForceUnsigned,
            &permit(),
        )
        .unwrap();

        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls[0], vec!["var", "GIT_COMMITTER_IDENT"]);
        assert_eq!(
            calls[1],
            vec![
                "tag",
                "-a",
                "-m",
                "Release pkg-a@1.0.0",
                "--no-sign",
                "--",
                "pkg-a@1.0.0",
                sha.as_str(),
            ]
        );
    }

    #[test]
    fn test_head_sha_returns_current_head_commit() {
        let sha = "a".repeat(40);
        let sha_clone = sha.clone();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(move |_args| Ok(ok(format!("{}\n", sha_clone)))),
        };
        let git = GitAccess::new(PathBuf::from("."), &runner);

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
        let git = GitAccess::new(PathBuf::from("."), &runner);
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
        let git = GitAccess::new(PathBuf::from("."), &runner);
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
        let git = GitAccess::new(PathBuf::from("."), &runner);
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
        let git = GitAccess::new(PathBuf::from("."), &runner);
        let sha = CommitSha::parse(&"a".repeat(40)).unwrap();
        let err = git
            .create_tag(
                "v1.0.0",
                &sha,
                None,
                crate::vcs::TagSignPolicy::RespectRepoConfig,
                &permit(),
            )
            .expect_err("must fail");
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
        for (raw, reason) in [
            (":100644 100644 aaa bbb M\0", "metadata with no following path"),
            ("not-a-metadata-line\0path\0", "missing leading colon"),
            (":bad M\0path\0", "too few metadata fields"),
        ] {
            let err = parse_raw_diff_z(raw).err();
            assert!(
                matches!(err, Some(VcsError::MalformedDiffOutput { .. })),
                "{reason}: got {err:?}"
            );
        }
    }

    /// An unparseable Git mode octal in `--raw -z` output is a distinct, typed
    /// [`VcsError::MalformedDiffOutput`] (E300), not the generic `VcsError::Git`.
    #[test]
    fn parse_raw_diff_z_rejects_invalid_mode() {
        let raw = ":000000 zzzzzz 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 A\0VERSION\0";
        let err = parse_raw_diff_z(raw).err();
        assert!(matches!(err, Some(VcsError::MalformedDiffOutput { .. })), "got {err:?}");
        assert_eq!(
            err.and_then(|e| miette::Diagnostic::code(&e).map(|c| c.to_string())),
            Some("E300".to_string())
        );
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

        let root = temp.path().to_path_buf();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(move |args| match args {
                ["rev-parse", "--show-toplevel"] => Ok(ok(format!("{}\n", root.display()))),
                ["diff", "--cached", "--raw", "-z", "--no-renames", "--no-abbrev", sha] if sha == &"0".repeat(40) => {
                    Ok(ok(raw.clone()))
                }
                ["hash-object", "--", "VERSION"] => Ok(ok(format!("{}\n", "1".repeat(40)))),
                ["hash-object", "--", "pkg/binary.bin"] => Ok(ok(format!("{}\n", "2".repeat(40)))),
                other => panic!("unexpected command: {other:?}"),
            }),
        };
        let git = GitAccess::new(temp.path(), &runner);

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

    /// A worktree read must fail if its bytes hash to a different blob than `--raw`'s post-image sha.
    #[test]
    fn staged_changes_since_rejects_a_worktree_blob_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("VERSION"), b"1.2.3\n").unwrap();

        let base = CommitSha::parse(&"0".repeat(40)).unwrap();
        let raw = [
            ":000000 100644 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 A",
            "VERSION",
        ]
        .join("\0")
            + "\0";

        let root = temp.path().to_path_buf();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(move |args| match args {
                ["rev-parse", "--show-toplevel"] => Ok(ok(format!("{}\n", root.display()))),
                ["diff", "--cached", "--raw", "-z", "--no-renames", "--no-abbrev", sha] if sha == &"0".repeat(40) => {
                    Ok(ok(raw.clone()))
                }
                // A different sha than the `--raw` entry claims: the worktree read does not match the staged content.
                ["hash-object", "--", "VERSION"] => Ok(ok(format!("{}\n", "9".repeat(40)))),
                other => panic!("unexpected command: {other:?}"),
            }),
        };
        let git = GitAccess::new(temp.path(), &runner);

        let result = git.staged_changes_since(&base);

        assert!(
            matches!(
                result,
                Err(VcsError::StagedContentMismatch { ref path, ref expected_sha })
                    if path == "VERSION" && expected_sha == &"1".repeat(40)
            ),
            "got {result:?}"
        );
    }
}
