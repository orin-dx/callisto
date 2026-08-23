use std::path::{Path, PathBuf};

use callisto_model::{ApplyPermit, CommandError, CommitRecord, CommitSha, CommitWalkError, CommitWalker, TagName};
use thiserror::Error;

pub mod access;
pub mod shell;

pub use access::GitAccess;
pub use shell::ShellGit;

#[derive(Clone, Debug, Error, miette::Diagnostic, PartialEq, Eq)]
#[non_exhaustive]
pub enum VcsError {
    #[error("failed to discover Git repository at `{path}`: {message}")]
    #[diagnostic(code(E050), help("Ensure target directory is inside a valid Git repository."))]
    RepoNotFound { path: PathBuf, message: String },

    #[error("git error: {0}")]
    #[diagnostic(code(E051))]
    Git(String),

    #[error("reference `{ref_name}` was not found")]
    #[diagnostic(code(E052), help("Check if reference or tag exists in local or remote Git refs."))]
    RefNotFound { ref_name: String },

    #[error("tag glob pattern `{pattern}` is not a valid glob: {message}")]
    #[diagnostic(
        code(E053),
        help("Fix the glob syntax (e.g. balance `{{`/`}}` and `[`/`]`) or use a literal tag name.")
    )]
    InvalidGlob { pattern: String, message: String },

    /// Wraps a [`CommandError`] surfaced by the [`ShellGit`] (`CommandRunner`-
    /// shelled) backend -- e.g. the `git` binary itself couldn't be spawned.
    /// Kept `transparent` so callers that only care about the underlying
    /// `CommandError` (most of them, since it's the same error every direct
    /// `CommandRunner::run` call site already surfaces) can match through it.
    #[error(transparent)]
    Command(#[from] CommandError),
}

/// Narrows a [`VcsError`] to the Layer 1 [`CommitWalkError`] vocabulary at
/// the [`CommitWalker`] boundary.
///
/// [`CommitWalkError::Command`] and [`CommitWalkError::RefNotFound`] survive
/// as themselves -- they are the two distinctions consumers branch on. Every
/// other variant is gix- or repository-specific with no Layer 1 equivalent,
/// so it collapses into [`CommitWalkError::Backend`] carrying this error's
/// own `Display` rendering; nothing is lost from the message a user sees.
impl From<VcsError> for CommitWalkError {
    fn from(err: VcsError) -> Self {
        match err {
            VcsError::Command(inner) => CommitWalkError::Command(inner),
            VcsError::RefNotFound { ref_name } => CommitWalkError::RefNotFound { ref_name },
            other => CommitWalkError::Backend {
                message: other.to_string(),
            },
        }
    }
}

/// A commit as produced by a history walk.
///
/// Aliases [`callisto_model::CommitRecord`] rather than redeclaring it, so a
/// walk's output crosses the [`CommitWalker`] seam without a conversion and
/// there is exactly one definition of a commit's shape in the workspace.
pub type GitCommit = CommitRecord;

/// Trait for Git VCS operations.
pub trait GitVcsProvider {
    fn head_sha(&self) -> Result<CommitSha, VcsError>;
    fn list_tags(&self, glob_pattern: Option<&str>) -> Result<Vec<TagName>, VcsError>;
}

/// Unified git-data access surface covering every operation callisto's
/// release-graph logic needs, independent of *how* the data is sourced.
///
/// Two backends implement this trait:
///
/// - [`GitRepository`] -- native, backed by `gix`. Fast and side-effect-free
///   for reads, but entirely unavailable on `wasm32` (gix is excluded from
///   that target's dependency set, so [`GitRepository::discover`] always
///   returns `Err` there).
/// - [`ShellGit`] -- shells out to the real `git` binary via a
///   [`callisto_model::CommandRunner`], so it works everywhere a `git`
///   binary is reachable, including through the `wasm32`/Extism host
///   bridge.
///
/// Callers should not implement or select between these directly; use
/// [`GitAccess::discover`], which tries native `gix` first and transparently
/// falls back to the `CommandRunner` shell-out, applying the correct
/// per-operation fallback policy (see [`GitAccess`]'s docs).
pub trait GitDataSource {
    /// Returns the commit SHA that `HEAD` currently resolves to.
    fn head_sha(&self) -> Result<CommitSha, VcsError>;

    /// Lists tag names, optionally filtered by `glob` (a [`globset::Glob`]
    /// pattern). Both backends filter with the exact same `globset`
    /// matching semantics, so tag selection is byte-identical regardless of
    /// which one served the request. `None` matches every tag.
    fn list_tags(&self, glob: Option<&str>) -> Result<Vec<TagName>, VcsError>;

    /// Resolves `refname` (tag, branch, or partial/full SHA) to the commit
    /// it points at. An unresolvable ref is *not* an error -- it resolves
    /// to `Ok(None)`, which callers typically treat as "no bound" / "infer
    /// over full history".
    fn resolve_commit(&self, refname: &str) -> Result<Option<CommitSha>, VcsError>;

    /// Lists commits reachable from `HEAD`, down to (exclusive) `since_ref`
    /// when given, filtered to those that touch at least one of
    /// `pathspecs` (an empty slice disables filtering and returns every
    /// commit in the walk). Merge commits are always excluded, matching
    /// `git log --no-merges`.
    ///
    /// Unlike [`Self::resolve_commit`], a `since_ref` that's given but
    /// fails to resolve to a commit *is* an error: silently ignoring it and
    /// walking unbounded history instead of the caller's requested bound
    /// is a correctness bug (e.g. it can re-surface already-released
    /// commits into changelog/severity inference). `since_ref: None` is not
    /// this case -- it's a deliberate request for the full history and
    /// always succeeds.
    fn commits_since(&self, since_ref: Option<&str>, pathspecs: &[PathBuf]) -> Result<Vec<GitCommit>, VcsError>;

    /// Creates the tag `name` at `target_sha`: annotated with `message`
    /// when `Some`, lightweight when `None`. Fails if a ref of that name
    /// already exists.
    ///
    /// Writes a git ref, so it requires an [`ApplyPermit`]; a dry run has
    /// none to give and therefore cannot call this at all.
    fn create_tag(
        &self,
        name: &str,
        target_sha: &CommitSha,
        message: Option<&str>,
        permit: &ApplyPermit,
    ) -> Result<(), VcsError>;

    /// Force-creates or -moves the floating tag `major_name` to point at
    /// `target_sha`, overwriting any existing ref of that name.
    ///
    /// Writes a git ref, so it requires an [`ApplyPermit`].
    fn create_floating_major(
        &self,
        major_name: &str,
        target_sha: &CommitSha,
        permit: &ApplyPermit,
    ) -> Result<(), VcsError>;
}

/// Wires a [`GitDataSource`] backend up to the Layer 1
/// [`callisto_model::CommitWalker`] contract.
///
/// The bodies are identical for every backend -- delegate `commits_since` and
/// narrow [`VcsError`] to [`CommitWalkError`] -- but they cannot be written
/// once as a blanket `impl<T: GitDataSource> CommitWalker for T`: `CommitWalker`
/// is foreign to this crate and `T` is uncovered, which the orphan rules
/// forbid. The macro keeps the repetition honest instead.
macro_rules! impl_commit_walker {
    ($ty:ty) => {
        impl CommitWalker for $ty {
            fn commits_since(
                &self,
                since_ref: Option<&str>,
                pathspecs: &[PathBuf],
            ) -> Result<Vec<CommitRecord>, CommitWalkError> {
                GitDataSource::commits_since(self, since_ref, pathspecs).map_err(Into::into)
            }
        }
    };
}

impl_commit_walker!(GitAccess<'_>);
impl_commit_walker!(GitRepository);
impl_commit_walker!(ShellGit<'_>);

pub struct GitRepository {
    #[cfg(not(target_arch = "wasm32"))]
    repo: gix::Repository,
}

impl GitRepository {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn discover(path: impl AsRef<Path>) -> Result<Self, VcsError> {
        let p = path.as_ref();
        let clean_path = dunce::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let repo = gix::discover(&clean_path).map_err(|e| VcsError::RepoNotFound {
            path: clean_path,
            message: e.to_string(),
        })?;
        Ok(Self { repo })
    }

    #[cfg(target_arch = "wasm32")]
    pub fn discover(path: impl AsRef<Path>) -> Result<Self, VcsError> {
        Err(VcsError::RepoNotFound {
            path: path.as_ref().to_path_buf(),
            message: "gix native git operations disabled on WASM target".to_string(),
        })
    }

    pub fn head_sha(&self) -> Result<CommitSha, VcsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let head = self
                .repo
                .head_commit()
                .map_err(|e| VcsError::Git(format!("Failed to get HEAD commit: {e}")))?;
            CommitSha::parse(&head.id.to_hex().to_string()).map_err(|e| VcsError::Git(format!("Invalid HEAD SHA: {e}")))
        }
        #[cfg(target_arch = "wasm32")]
        {
            Err(VcsError::Git("WASM unsupported".to_string()))
        }
    }

    pub fn list_tags(&self, glob_pattern: Option<&str>) -> Result<Vec<TagName>, VcsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let platform = self
                .repo
                .references()
                .map_err(|e| VcsError::Git(format!("Failed to read references: {e}")))?;

            let mut tags = Vec::new();
            let tag_refs = platform
                .tags()
                .map_err(|e| VcsError::Git(format!("Failed to list tag refs: {e}")))?;

            // A pattern that fails to compile must not silently disable
            // filtering (which would match every tag in the repo -- a real
            // correctness risk for release tagging, since a malformed tag
            // template could then make "last tag" resolution pick an
            // unrelated package's tag). Surface it as an error instead;
            // `None` (no pattern requested at all) still means "match
            // everything".
            let matcher = glob_pattern
                .map(|p| {
                    globset::Glob::new(p)
                        .map(|g| g.compile_matcher())
                        .map_err(|e| VcsError::InvalidGlob {
                            pattern: p.to_string(),
                            message: e.to_string(),
                        })
                })
                .transpose()?;

            for r in tag_refs.flatten() {
                let name = r.name().shorten().to_string();
                if let Some(ref m) = matcher {
                    if !m.is_match(&name) {
                        continue;
                    }
                }
                tags.push(TagName(name));
            }

            Ok(tags)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _unused = glob_pattern;
            Ok(Vec::new())
        }
    }

    /// Resolves an arbitrary ref (tag, branch, or partial SHA) to the commit
    /// SHA it points at.
    ///
    /// Follows the same resolution and error-handling convention as the
    /// `from_ref` handling inside [`Self::commits_since`]: an unresolvable
    /// ref (missing tag, unborn repo, ref pointing at a non-commit object
    /// that can't be peeled to one, etc.) is not an error condition -- it
    /// degrades gracefully to `Ok(None)`, which callers treat as "no bound"
    /// / "infer over full history".
    pub fn resolve_commit(&self, refname: &str) -> Result<Option<CommitSha>, VcsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Ok(spec) = self.repo.rev_parse_single(refname) else {
                return Ok(None);
            };
            let Ok(object) = spec.object() else {
                return Ok(None);
            };
            let Ok(commit) = object.peel_to_kind(gix::object::Kind::Commit) else {
                return Ok(None);
            };

            let hex = commit.id.to_hex().to_string();
            let sha = CommitSha::parse(&hex).map_err(|e| VcsError::Git(format!("Invalid commit SHA: {e}")))?;
            Ok(Some(sha))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _unused = refname;
            Ok(None)
        }
    }

    /// Like [`Self::commits_since`], but additionally filters the walked
    /// commits down to those that touched at least one path under one of
    /// the given `pathspecs`.
    ///
    /// `pathspecs` are matched as simple path prefixes against every path
    /// touched by a commit's tree diff against its (first) parent -- a
    /// pathspec matches a changed path if the changed path is exactly equal
    /// to it, or the changed path is nested underneath it as a directory
    /// prefix. This mirrors the directory/path-prefix scoping `git log --
    /// <pathspecs>` provides for per-package commit filtering; it does not
    /// implement full git pathspec magic syntax (`:!`, glob magic, etc).
    ///
    /// An empty `pathspecs` slice disables filtering entirely and returns
    /// every commit in the walk, matching `git log` with no trailing `--`
    /// pathspec.
    ///
    /// `since` is an exclusive lower bound, same as `commits_since`'s
    /// `from_ref`: the commit `since` points at is not itself included in
    /// the result.
    ///
    /// Merge commits (more than one parent) are skipped, matching `git log
    /// --no-merges` -- this method exists to power per-package commit
    /// scoping for severity inference, where a merge commit's message
    /// duplicates work already represented by its non-merge ancestors.
    pub fn commits_since_with_pathspec(
        &self,
        since: Option<&CommitSha>,
        pathspecs: &[PathBuf],
    ) -> Result<Vec<GitCommit>, VcsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let head = self
                .repo
                .head_commit()
                .map_err(|e| VcsError::Git(format!("Failed to get HEAD commit: {e}")))?;

            let revwalk = self
                .repo
                .rev_walk(vec![head.id])
                .all()
                .map_err(|e| VcsError::Git(format!("Failed to create revwalk: {e}")))?;

            // Build the excluded set: all SHAs reachable from `since`
            // (inclusive). Using `continue` rather than `break` is critical
            // for branchy history: a topological walk can visit the `since`
            // commit before it has emitted all commits on merged branches
            // that diverged *before* the tag. A `break` would silently drop
            // those in-queue commits; `continue` skips only the already-seen
            // ancestors.
            let excluded: std::collections::HashSet<String> = if let Some(s) = since {
                let since_oid = gix::ObjectId::from_hex(s.as_ref().as_bytes())
                    .map_err(|e| VcsError::Git(format!("Invalid since SHA: {e}")))?;
                let since_walk = self
                    .repo
                    .rev_walk(vec![since_oid])
                    .all()
                    .map_err(|e| VcsError::Git(format!("Failed to walk from since: {e}")))?;
                let mut set = std::collections::HashSet::new();
                for info in since_walk {
                    let info = info.map_err(|e| VcsError::Git(e.to_string()))?;
                    set.insert(info.id.to_hex().to_string());
                }
                set
            } else {
                std::collections::HashSet::new()
            };

            let mut commits = Vec::new();
            for info in revwalk {
                let info = info.map_err(|e| VcsError::Git(e.to_string()))?;
                let hex = info.id.to_hex().to_string();

                if excluded.contains(&hex) {
                    continue;
                }

                if info.parent_ids().count() > 1 {
                    continue;
                }

                let commit_obj = info
                    .object()
                    .map_err(|e| VcsError::Git(format!("Failed to load commit object: {e}")))?;

                if !pathspecs.is_empty() {
                    let touched = commit_touches_pathspecs(&self.repo, &info, &commit_obj, pathspecs)?;
                    if !touched {
                        continue;
                    }
                }

                let sha = CommitSha::parse(&hex).map_err(|e| VcsError::Git(format!("Invalid commit SHA: {e}")))?;

                let message = commit_obj.message().map_err(|e| VcsError::Git(e.to_string()))?;
                let summary = message.title.to_string().replace("\r\n", "\n").trim_end().to_string();
                let body = message.body.map(|b| b.to_string().replace("\r\n", "\n"));

                commits.push(GitCommit { sha, summary, body });
            }

            Ok(commits)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _unused = (since, pathspecs);
            Ok(Vec::new())
        }
    }

    pub fn create_tag(
        &self,
        name: &str,
        target_sha: &CommitSha,
        message: Option<&str>,
        _permit: &ApplyPermit,
    ) -> Result<(), VcsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let oid = gix::ObjectId::from_hex(target_sha.as_ref().as_bytes())
                .map_err(|e| VcsError::Git(format!("Invalid SHA: {e}")))?;

            if let Some(msg) = message {
                self.repo
                    .tag(
                        name,
                        oid,
                        gix::object::Kind::Commit,
                        None,
                        msg,
                        gix::refs::transaction::PreviousValue::MustNotExist,
                    )
                    .map_err(|e| VcsError::Git(format!("Failed to create tag: {e}")))?;
            } else {
                let clean_name = name.strip_prefix("refs/tags/").unwrap_or(name);
                let ref_name = format!("refs/tags/{}", clean_name);
                let _unused = self
                    .repo
                    .reference(
                        ref_name,
                        oid,
                        gix::refs::transaction::PreviousValue::MustNotExist,
                        "callisto create tag",
                    )
                    .map_err(|e| VcsError::Git(format!("Failed to create lightweight tag: {e}")))?;
            }
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (name, target_sha, message);
            Err(VcsError::Git("WASM unsupported".to_string()))
        }
    }

    pub fn create_floating_major(
        &self,
        major_name: &str,
        target_sha: &CommitSha,
        _permit: &ApplyPermit,
    ) -> Result<(), VcsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let oid = gix::ObjectId::from_hex(target_sha.as_ref().as_bytes())
                .map_err(|e| VcsError::Git(format!("Invalid SHA: {e}")))?;

            let ref_name = format!("refs/tags/{}", major_name);
            let _unused = self
                .repo
                .reference(
                    ref_name,
                    oid,
                    gix::refs::transaction::PreviousValue::Any,
                    "callisto floating major",
                )
                .map_err(|e| VcsError::Git(format!("Failed to update floating major: {e}")))?;

            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (major_name, target_sha);
            Err(VcsError::Git("WASM unsupported".to_string()))
        }
    }
}

/// Diffs `commit_obj`'s tree against its first parent's tree (or the empty
/// tree, for a root commit) and reports whether any changed path falls
/// under one of `pathspecs`.
#[cfg(not(target_arch = "wasm32"))]
fn commit_touches_pathspecs(
    repo: &gix::Repository,
    info: &gix::revision::walk::Info<'_>,
    commit_obj: &gix::Commit<'_>,
    pathspecs: &[PathBuf],
) -> Result<bool, VcsError> {
    let commit_tree = commit_obj
        .tree()
        .map_err(|e| VcsError::Git(format!("Failed to load commit tree: {e}")))?;

    let parent_tree = match info.parent_ids().next() {
        Some(parent_id) => {
            let parent_commit = parent_id
                .object()
                .map_err(|e| VcsError::Git(format!("Failed to load parent commit object: {e}")))?
                .into_commit();
            parent_commit
                .tree()
                .map_err(|e| VcsError::Git(format!("Failed to load parent tree: {e}")))?
        }
        None => repo.empty_tree(),
    };

    let mut touched = false;
    let mut platform = parent_tree
        .changes()
        .map_err(|e| VcsError::Git(format!("Failed to initialize tree diff: {e}")))?;
    // Rewrite (rename/copy) detection needs blob-similarity computation we
    // don't otherwise need: pathspec matching only cares about which paths
    // changed, not whether they're related across a rename.
    platform.options(|o| {
        o.track_rewrites(None);
    });
    // Note: we deliberately always return `Continue` here, never `Break` --
    // gix's tree-diff machinery treats an early `Break` as a cancellation
    // and surfaces it as an `Err` from `for_each_to_obtain_tree`, which
    // would turn "found a match" into a spurious failure.
    platform
        .for_each_to_obtain_tree(&commit_tree, |change| {
            if change_matches_pathspecs(&change, pathspecs) {
                touched = true;
            }
            Ok::<_, std::convert::Infallible>(std::ops::ControlFlow::Continue(()))
        })
        .map_err(|e| VcsError::Git(format!("Failed to diff commit tree: {e}")))?;

    Ok(touched)
}

#[cfg(not(target_arch = "wasm32"))]
fn change_matches_pathspecs(change: &gix::object::tree::diff::Change<'_, '_, '_>, pathspecs: &[PathBuf]) -> bool {
    use gix::object::tree::diff::Change;
    match change {
        Change::Addition { location, .. }
        | Change::Deletion { location, .. }
        | Change::Modification { location, .. } => location_matches(location, pathspecs),
        Change::Rewrite {
            location,
            source_location,
            ..
        } => location_matches(location, pathspecs) || location_matches(source_location, pathspecs),
    }
}

/// Simple directory/path-prefix pathspec match: a changed path matches a
/// pathspec if it's exactly equal to it, or nested underneath it as a
/// directory prefix. Does not implement full git pathspec magic syntax.
#[cfg(not(target_arch = "wasm32"))]
fn location_matches(location: &gix::bstr::BStr, pathspecs: &[PathBuf]) -> bool {
    let path_str = String::from_utf8_lossy(location);
    let changed_path = Path::new(path_str.as_ref());
    pathspecs
        .iter()
        .any(|spec| changed_path == spec.as_path() || changed_path.starts_with(spec))
}

impl GitVcsProvider for GitRepository {
    fn head_sha(&self) -> Result<CommitSha, VcsError> {
        self.head_sha()
    }

    fn list_tags(&self, glob_pattern: Option<&str>) -> Result<Vec<TagName>, VcsError> {
        self.list_tags(glob_pattern)
    }
}

/// Native (`gix`) implementation of [`GitDataSource`].
///
/// [`Self::commits_since`] is where this crate's `commits_since` ref-not-
/// found bug used to live: the old inherent `commits_since(from_ref:
/// Option<&str>)` method resolved `from_ref` internally and, if that
/// resolution silently failed (bad tag, unborn repo, non-commit object,
/// etc.), fell through to walking the *entire* history unbounded instead of
/// stopping where the caller asked -- a correctness bug (e.g. it could
/// re-surface already-released commits into changelog/severity inference)
/// masquerading as graceful degradation. The fix: `since_ref` resolution
/// now goes through [`Self::resolve_commit`] and an explicit `Ok(None) =>
/// Err(VcsError::RefNotFound)` step below, so an unresolvable *explicit*
/// bound is always a surfaced error. `since_ref: None` (no bound requested
/// at all) is unaffected and still walks full history, same as before.
impl GitDataSource for GitRepository {
    fn head_sha(&self) -> Result<CommitSha, VcsError> {
        self.head_sha()
    }

    fn list_tags(&self, glob: Option<&str>) -> Result<Vec<TagName>, VcsError> {
        self.list_tags(glob)
    }

    fn resolve_commit(&self, refname: &str) -> Result<Option<CommitSha>, VcsError> {
        self.resolve_commit(refname)
    }

    fn commits_since(&self, since_ref: Option<&str>, pathspecs: &[PathBuf]) -> Result<Vec<GitCommit>, VcsError> {
        let since_sha = since_ref
            .map(|r| {
                self.resolve_commit(r)?.ok_or_else(|| VcsError::RefNotFound {
                    ref_name: r.to_string(),
                })
            })
            .transpose()?;
        self.commits_since_with_pathspec(since_sha.as_ref(), pathspecs)
    }

    fn create_tag(
        &self,
        name: &str,
        target_sha: &CommitSha,
        message: Option<&str>,
        permit: &ApplyPermit,
    ) -> Result<(), VcsError> {
        self.create_tag(name, target_sha, message, permit)
    }

    fn create_floating_major(
        &self,
        major_name: &str,
        target_sha: &CommitSha,
        permit: &ApplyPermit,
    ) -> Result<(), VcsError> {
        self.create_floating_major(major_name, target_sha, permit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec: a malformed glob pattern (fails to compile) must make
    /// `list_tags` return `Err(VcsError::InvalidGlob)`, not silently
    /// disable filtering and match every tag in the repo. Matching every
    /// tag is a real correctness risk for release tagging: e.g.
    /// `TagIndex::build`'s per-package tag-template matching (see
    /// `callisto-graph`'s `tags.rs`) relies on this, and a caller path
    /// that constructs a broken pattern must not spuriously report
    /// "already exists" against an unrelated tag.
    #[test]
    fn test_list_tags_rejects_malformed_glob_instead_of_matching_everything() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        std::fs::write(root.join("f.txt"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);
        run_git(
            root,
            &["-c", "tag.gpgSign=false", "tag", "-m", "release", "pkg-a@1.0.0"],
        );
        run_git(
            root,
            &["-c", "tag.gpgSign=false", "tag", "-m", "release", "unrelated-tag"],
        );

        let repo = GitRepository::discover(root).unwrap();

        // Unbalanced `{` makes this an uncompilable globset pattern.
        let result = repo.list_tags(Some("pkg-a@{malformed"));

        assert!(
            matches!(result, Err(VcsError::InvalidGlob { .. })),
            "malformed glob must be surfaced as Err(VcsError::InvalidGlob), got {result:?}"
        );
    }

    #[test]
    fn test_discovers_repo() {
        let repo = GitRepository::discover(".");
        assert!(repo.is_ok());

        let r = repo.unwrap();
        let head = r.head_sha();
        assert!(head.is_ok());
    }

    /// Runs the real `git` binary to build a temp repo fixture. Needed
    /// because `GitRepository::discover`/`resolve_commit`/
    /// `commits_since_with_pathspec` operate on a real on-disk repo, not a
    /// mocked `CommandRunner`.
    fn run_git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git must be installed to run this test");
        assert!(status.success(), "git {args:?} failed in {dir:?}");
    }

    fn init_repo(root: &Path) {
        run_git(root, &["init", "-q"]);
        run_git(root, &["config", "user.email", "test@example.com"]);
        run_git(root, &["config", "user.name", "Test"]);
    }

    #[test]
    fn test_resolve_commit_returns_sha_for_valid_tag() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        std::fs::write(root.join("f.txt"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);
        run_git(root, &["-c", "tag.gpgSign=false", "tag", "-m", "release", "v1.0.0"]);

        let expected_output = std::process::Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", "v1.0.0^{commit}"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(expected_output.status.success());
        let expected_sha = CommitSha::parse(String::from_utf8_lossy(&expected_output.stdout).trim()).unwrap();

        let repo = GitRepository::discover(root).unwrap();
        let resolved = repo.resolve_commit("v1.0.0").unwrap();

        assert_eq!(resolved, Some(expected_sha));
    }

    #[test]
    fn test_resolve_commit_returns_none_for_missing_ref() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        std::fs::write(root.join("f.txt"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        let repo = GitRepository::discover(root).unwrap();
        let resolved = repo.resolve_commit("this-tag-does-not-exist").unwrap();

        assert_eq!(resolved, None);
    }

    /// Spec: `commits_since_with_pathspec` must only return commits that
    /// touched a path under one of the given pathspecs -- mirrors the
    /// filtering behavior of `git log -- <pathspecs>`, scoped to simple
    /// directory/path-prefix matching.
    #[test]
    fn test_commits_since_with_pathspec_filters_by_changed_path() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        std::fs::create_dir_all(root.join("crates/pkg-a")).unwrap();
        std::fs::write(root.join("crates/pkg-a/file.txt"), "a\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: add pkg-a"]);

        std::fs::create_dir_all(root.join("crates/pkg-b")).unwrap();
        std::fs::write(root.join("crates/pkg-b/file.txt"), "b\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: add pkg-b"]);

        std::fs::write(root.join("crates/pkg-a/file.txt"), "a2\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "fix: tweak pkg-a"]);

        let repo = GitRepository::discover(root).unwrap();
        let pathspecs = vec![PathBuf::from("crates/pkg-a")];
        let commits = repo.commits_since_with_pathspec(None, &pathspecs).unwrap();

        let summaries: Vec<&str> = commits.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(summaries, vec!["fix: tweak pkg-a", "feat: add pkg-a"]);
    }

    #[test]
    fn test_commits_since_with_pathspec_excludes_unrelated_paths() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        std::fs::create_dir_all(root.join("crates/pkg-a")).unwrap();
        std::fs::write(root.join("crates/pkg-a/file.txt"), "a\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: add pkg-a"]);

        let repo = GitRepository::discover(root).unwrap();
        let pathspecs = vec![PathBuf::from("crates/pkg-c")];
        let commits = repo.commits_since_with_pathspec(None, &pathspecs).unwrap();

        assert!(commits.is_empty());
    }

    /// Spec: with no pathspecs at all, every commit in the walk is
    /// returned (mirrors `git log` with no `--` pathspec filter).
    #[test]
    fn test_commits_since_with_pathspec_empty_pathspecs_returns_all() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: c1"]);

        std::fs::write(root.join("b.txt"), "b\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: c2"]);

        let repo = GitRepository::discover(root).unwrap();
        let commits = repo.commits_since_with_pathspec(None, &[]).unwrap();

        assert_eq!(commits.len(), 2);
    }

    /// Spec: `since` is an exclusive lower bound, same as `commits_since` --
    /// the commit `since` points at must not itself be included.
    #[test]
    fn test_commits_since_with_pathspec_respects_since_bound() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        std::fs::write(root.join("a.txt"), "a1\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: c1"]);

        let repo_after_c1 = GitRepository::discover(root).unwrap();
        let since_sha = repo_after_c1.head_sha().unwrap();

        std::fs::write(root.join("a.txt"), "a2\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: c2"]);

        std::fs::write(root.join("a.txt"), "a3\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: c3"]);

        let repo = GitRepository::discover(root).unwrap();
        let pathspecs = vec![PathBuf::from("a.txt")];
        let commits = repo.commits_since_with_pathspec(Some(&since_sha), &pathspecs).unwrap();

        let summaries: Vec<&str> = commits.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(summaries, vec!["feat: c3", "feat: c2"]);
    }

    /// Spec: merge commits (more than one parent) must be excluded from the
    /// result, matching `git log --no-merges` -- this builds a *real*
    /// two-parent merge commit (a genuine `git merge --no-ff`, not a
    /// fast-forward) and asserts the merge commit itself is absent while
    /// every non-merge commit reachable through either branch is present.
    #[test]
    fn test_commits_since_with_pathspec_excludes_merge_commits() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        // Explicitly name the initial branch: the ambient git install's
        // `init.defaultBranch` may not be `main` (or may be unset, in which
        // case it falls back to `master`), and this test needs a known name
        // to `checkout` back to after creating the `feature` branch.
        run_git(root, &["init", "-q", "-b", "main"]);
        run_git(root, &["config", "user.email", "test@example.com"]);
        run_git(root, &["config", "user.name", "Test"]);

        std::fs::write(root.join("a.txt"), "a1\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: c1 on main"]);

        run_git(root, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(root.join("b.txt"), "b1\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: c2 on feature"]);

        run_git(root, &["checkout", "-q", "main"]);
        std::fs::write(root.join("c.txt"), "c1\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: c3 on main"]);

        // `--no-ff` guarantees a genuine two-parent merge commit even though
        // this history could otherwise fast-forward-free merge cleanly.
        run_git(
            root,
            &["merge", "--no-ff", "-q", "-m", "merge: feature into main", "feature"],
        );

        let repo = GitRepository::discover(root).unwrap();
        let commits = repo.commits_since_with_pathspec(None, &[]).unwrap();

        let summaries: Vec<&str> = commits.iter().map(|c| c.summary.as_str()).collect();
        assert!(
            !summaries.contains(&"merge: feature into main"),
            "merge commit must be excluded, got: {summaries:?}"
        );
        assert!(summaries.contains(&"feat: c1 on main"));
        assert!(summaries.contains(&"feat: c2 on feature"));
        assert!(summaries.contains(&"feat: c3 on main"));
        assert_eq!(summaries.len(), 3, "got: {summaries:?}");
    }

    /// Spec: a commit that adds/modifies a binary (non-UTF8) blob under a
    /// matching pathspec must still be detected as touching that pathspec --
    /// gix's tree-diff reports binary blob changes just like text changes,
    /// and `commit_touches_pathspecs` must not silently skip them.
    #[test]
    fn test_commits_since_with_pathspec_detects_binary_file_changes() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        std::fs::create_dir_all(root.join("crates/pkg-a")).unwrap();
        std::fs::write(root.join("crates/pkg-a/keep.txt"), "seed\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: seed pkg-a"]);

        // Arbitrary non-UTF8 bytes, including NUL and invalid UTF-8
        // sequences, to force git/gix to treat this blob as binary.
        let binary_bytes: Vec<u8> = vec![
            0x00, 0xFF, 0xFE, 0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x01, 0x02, 0xC0, 0xC1, 0xF5, 0xFF,
        ];
        std::fs::write(root.join("crates/pkg-a/blob.bin"), &binary_bytes).unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: add binary blob to pkg-a"]);

        let repo = GitRepository::discover(root).unwrap();
        let pathspecs = vec![PathBuf::from("crates/pkg-a")];
        let commits = repo.commits_since_with_pathspec(None, &pathspecs).unwrap();

        let summaries: Vec<&str> = commits.iter().map(|c| c.summary.as_str()).collect();
        assert!(
            summaries.contains(&"feat: add binary blob to pkg-a"),
            "binary file addition must be detected as touching the pathspec, got: {summaries:?}"
        );
    }

    /// Spec (pinning current, deliberate behavior): `commit_touches_pathspecs`
    /// configures `track_rewrites(None)`, so renames are never reported as a
    /// single `Change::Rewrite` -- gix instead reports them as a separate
    /// `Deletion` (old path) and `Addition` (new path). This test renames a
    /// file *out of* a pathspec-matching directory into a non-matching one
    /// and asserts the rename commit is still reported as touching the
    /// pathspec, because the `Deletion` at the old (matching) location is
    /// enough to trip `change_matches_pathspecs` on its own -- independent of
    /// whether the new location matches anything.
    #[test]
    fn test_commits_since_with_pathspec_rename_out_of_pathspec_dir_is_pinned_as_touched() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        std::fs::create_dir_all(root.join("crates/pkg-a")).unwrap();
        std::fs::write(root.join("crates/pkg-a/file.txt"), "a\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: add pkg-a"]);

        std::fs::create_dir_all(root.join("crates/pkg-b")).unwrap();
        run_git(root, &["mv", "crates/pkg-a/file.txt", "crates/pkg-b/file.txt"]);
        run_git(root, &["commit", "-q", "-m", "refactor: move file out of pkg-a"]);

        let repo = GitRepository::discover(root).unwrap();
        let pathspecs = vec![PathBuf::from("crates/pkg-a")];
        let commits = repo.commits_since_with_pathspec(None, &pathspecs).unwrap();

        let summaries: Vec<&str> = commits.iter().map(|c| c.summary.as_str()).collect();
        assert!(
            summaries.contains(&"refactor: move file out of pkg-a"),
            "with track_rewrites(None), the deletion at the old (matching) path must be \
             detected on its own, got: {summaries:?}"
        );
    }

    /// Spec: CRLF line endings in commit messages are normalized to LF in the
    /// returned `GitCommit.summary` and `GitCommit.body` fields. The
    /// implementation replaces `\r\n` with `\n` after reading the raw gix
    /// message bytes; this test verifies that normalization is applied and
    /// that the caller never sees bare carriage-return characters.
    #[test]
    fn test_commits_since_crlf_message_normalized_to_lf() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        // Write a commit message file that contains CRLF line endings.
        let msg_file = root.join("commit_msg.txt");
        std::fs::write(
            &msg_file,
            "fix: CRLF summary line\r\n\r\nBody paragraph with CRLF.\r\nSecond body line.\r\n",
        )
        .unwrap();

        std::fs::write(root.join("a.txt"), "content\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-F", msg_file.to_str().unwrap()]);

        let repo = GitRepository::discover(root).unwrap();
        let commits = repo.commits_since_with_pathspec(None, &[]).unwrap();

        assert_eq!(commits.len(), 1, "expected exactly one commit");
        let commit = &commits[0];

        // The summary must not contain any bare CR after normalization.
        assert!(
            !commit.summary.contains('\r'),
            "summary must not contain CR after normalization; got: {:?}",
            commit.summary
        );
        assert_eq!(
            commit.summary, "fix: CRLF summary line",
            "summary must match the first commit message line (normalized)"
        );

        // Body must also be normalized when present.
        if let Some(body) = &commit.body {
            assert!(
                !body.contains('\r'),
                "body must not contain CR after normalization; got: {:?}",
                body
            );
        }
    }

    /// Spec: `GitRepository::commits_since_with_pathspec` must not panic when
    /// HEAD is detached (i.e., no branch is checked out). Detached HEAD is a
    /// valid and common repository state (e.g. after `git checkout <sha>`,
    /// during a rebase, or in CI). The method must either return commits
    /// reachable from HEAD or a clear `VcsError`, never an unwrap panic.
    #[test]
    fn test_commits_since_detached_head_does_not_panic() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        // Commit #1 – the one we will check out by SHA to detach HEAD.
        std::fs::write(root.join("a.txt"), "first\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: first commit"]);

        let sha_out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .unwrap();
        let first_sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();

        // Commit #2 – created on the branch so detaching at #1 leaves it
        // unreachable from HEAD.
        std::fs::write(root.join("b.txt"), "second\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: second commit"]);

        // Detach HEAD by checking out the first commit SHA directly.
        run_git(root, &["checkout", "-q", &first_sha]);

        let repo = GitRepository::discover(root).unwrap();

        // Must not panic; Ok with at least one commit is the expected outcome.
        let result = repo.commits_since_with_pathspec(None, &[]);
        match result {
            Ok(commits) => {
                // Detached HEAD at commit #1: only that commit is reachable.
                assert_eq!(
                    commits.len(),
                    1,
                    "expected 1 commit reachable from detached HEAD; got {:?}",
                    commits.len()
                );
                assert_eq!(commits[0].summary, "feat: first commit");
            }
            Err(e) => {
                // A clear VcsError (not a panic) is acceptable as a fallback,
                // but gix reads detached HEAD just fine, so this arm should
                // not be reached in practice.
                assert!(
                    matches!(e, VcsError::Git(_)),
                    "unexpected error type from detached HEAD: {e:?}"
                );
            }
        }
    }

    /// Bug regression: when a feature branch was created BEFORE the `since`
    /// tag commit (branching from an ancestor of the tag), gix's topological
    /// walk visits the tag commit before some commits on the feature branch.
    /// The old `break`-on-SHA-match terminated the walk early, silently
    /// dropping branch commits still queued behind the tag.
    ///
    /// History:
    ///   A → B → S(v1 tag) → C    (main)
    ///        ↘                ↗
    ///          D  →  E            (feat, branched from B, merged into main)
    ///
    /// Expected: `commits_since_with_pathspec(Some(&v1_sha), &[])` = {C, D, E}
    #[test]
    fn test_commits_since_with_pathspec_includes_pre_tag_branch_commits() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        run_git(root, &["init", "-q", "-b", "main"]);
        run_git(root, &["config", "user.email", "test@example.com"]);
        run_git(root, &["config", "user.name", "Test"]);
        run_git(root, &["config", "commit.gpgsign", "false"]);

        // A: initial commit (shared ancestor)
        std::fs::write(root.join("base.txt"), "shared\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: A initial"]);

        // B: the feat branch will diverge from here (before the v1 tag)
        std::fs::write(root.join("b.txt"), "b\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: B second"]);

        let b_sha = String::from_utf8_lossy(
            &std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(root)
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();

        // S: the release commit; v1 tag lands here (the "since" lower bound)
        std::fs::write(root.join("s.txt"), "s\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "chore: S release"]);
        run_git(root, &["-c", "tag.gpgSign=false", "tag", "-a", "-m", "v1", "v1"]);

        let v1_sha_str = String::from_utf8_lossy(
            &std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(root)
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();

        // D and E: commits on the feat branch (diverged from B, before S).
        // Touch feat-only files so the eventual merge has no conflicts.
        run_git(root, &["checkout", "-q", "-b", "feat", &b_sha]);
        std::fs::write(root.join("d.txt"), "d\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: D on feat"]);
        std::fs::write(root.join("e.txt"), "e\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: E on feat"]);

        // C: commit on main after the v1 tag, then merge feat → merge commit M
        run_git(root, &["checkout", "-q", "main"]);
        std::fs::write(root.join("c.txt"), "c\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: C after release"]);
        run_git(root, &["merge", "--no-ff", "-q", "feat", "-m", "merge: feat into main"]);

        let since_sha = CommitSha::parse(&v1_sha_str).unwrap();
        let repo = GitRepository::discover(root).unwrap();
        let commits = repo.commits_since_with_pathspec(Some(&since_sha), &[]).unwrap();

        let summaries: Vec<&str> = commits.iter().map(|c| c.summary.as_str()).collect();
        assert!(
            summaries.contains(&"feat: C after release"),
            "C (after v1 on main) must be included, got: {summaries:?}"
        );
        assert!(
            summaries.contains(&"feat: D on feat"),
            "D (on feat branch, merged after v1) must be included — \
             break-on-SHA drops it when gix visits the tag before D, got: {summaries:?}"
        );
        assert!(
            summaries.contains(&"feat: E on feat"),
            "E (on feat branch, merged after v1) must be included, got: {summaries:?}"
        );
        assert_eq!(
            summaries.len(),
            3,
            "exactly C, D, E — no historical commits (A, B, S) and no merge commit M: \
             got {summaries:?}"
        );
    }
}
