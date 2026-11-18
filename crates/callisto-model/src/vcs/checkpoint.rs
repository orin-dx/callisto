//! A checkpoint records a local command's inputs before its first write, so an interrupted run can be resumed from
//! them or undone ([ADR 13](../../../../docs/adr/0013-interrupted-version-runs-resume-from-a-checkpoint-ref.md)).

use std::path::Path;

use sha2::{Digest, Sha256};

use super::access::GitAccess;
use super::VcsError;
use crate::CommitSha;

/// Where an interrupted `callisto version` run keeps its inputs. A per-worktree ref, so a linked worktree has its own
/// checkpoint and never sees or discards another's.
pub const VERSION_CHECKPOINT: &str = "refs/worktree/callisto/checkpoint/version";

const HEAD_TRAILER: &str = "Callisto-Head";
const TAGS_TRAILER: &str = "Callisto-Tags";
const VERSION_TRAILER: &str = "Callisto-Version";
const DATE_TRAILER: &str = "Callisto-Date";
const WORKSPACE_TRAILER: &str = "Callisto-Workspace";

/// The inputs a plan reads besides files, recorded in the checkpoint commit's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointFacts {
    pub head: String,
    pub tags: String,
    pub callisto_version: String,
    /// The workspace directory inside the repository (empty at the top). Workspaces share a worktree's checkpoint ref,
    /// so a run in another workspace must not resume this one's.
    pub workspace: String,
}

/// A recorded checkpoint: a commit whose tree is the working tree, whose first parent is `HEAD` and whose second
/// parent records the index.
#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub commit: CommitSha,
    pub facts: CheckpointFacts,
    /// The date the run planned with (`YYYY-MM-DD`), so a resume on a later day writes the same changelog dates.
    pub date: String,
}

impl GitAccess<'_> {
    /// The checkpoint at `name`, or `None` when there is none.
    pub fn checkpoint(&self, name: &str) -> Result<Option<Checkpoint>, VcsError> {
        let Some(commit) = self.resolve_commit(name)? else {
            return Ok(None);
        };
        let message = self.git_stdout(&["show", "-s", "--no-show-signature", "--format=%B", commit.as_str()])?;
        let trailer = |key: &str| {
            message
                .lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix(": "))
                .unwrap_or_default()
                .to_string()
        };
        Ok(Some(Checkpoint {
            facts: CheckpointFacts {
                head: trailer(HEAD_TRAILER),
                tags: trailer(TAGS_TRAILER),
                callisto_version: trailer(VERSION_TRAILER),
                workspace: trailer(WORKSPACE_TRAILER),
            },
            date: trailer(DATE_TRAILER),
            commit,
        }))
    }

    /// The facts a checkpoint taken now would record.
    pub fn checkpoint_facts(&self, callisto_version: &str) -> Result<CheckpointFacts, VcsError> {
        let tags = self.git_stdout(&["for-each-ref", "--format=%(objectname) %(refname)", "refs/tags"])?;
        Ok(CheckpointFacts {
            head: self.head_sha()?.as_str().to_string(),
            tags: crate::release::digest::lower_hex(Sha256::digest(tags.as_bytes())),
            callisto_version: callisto_version.to_string(),
            workspace: self.root_prefix()?.display().to_string(),
        })
    }

    /// Records `HEAD`, the index, and the working tree under this root (untracked files included, ignored files
    /// left out) at `name`, without touching the user's index or working tree.
    ///
    /// # Errors
    ///
    /// [`VcsError::CheckpointHeld`] when `name` already exists, so two runs never share one; a failed git command
    /// otherwise.
    pub fn create_checkpoint(&self, name: &str, facts: &CheckpointFacts, date: &str) -> Result<CommitSha, VcsError> {
        let git_dir = self.git_stdout(&["rev-parse", "--absolute-git-dir"])?;
        let index = Path::new(&git_dir).join("callisto-checkpoint.index");
        let index_str = index.to_string_lossy().into_owned();
        let env = [("GIT_INDEX_FILE", index_str.as_str())];
        let real_index = self.git_stdout(&["rev-parse", "--git-path", "index"])?;
        let real_index = self.root().join(real_index);
        let created = (|| {
            let index_commit = self.internal_commit(&self.git_stdout(&["write-tree"])?, &[&facts.head], "index")?;
            // Seeded from the real index so Git's stat cache spares rehashing every tracked file; without one it is
            // built from `HEAD`.
            if std::fs::copy(&real_index, &index).is_err() {
                self.git_stdout_env(&["read-tree", "HEAD"], &env)?;
            }
            // A file Git cannot read, or whose line endings `core.safecrlf` objects to, is not something `version`
            // reads or writes, so it must not stop the run: it is left out of the record.
            let args = ["-c", "core.safecrlf=false", "add", "-A", "--ignore-errors", "--", "."];
            let output = self.runner().run_with_env("git", &args, self.root(), &env)?;
            if !output.success() && !output.stderr.contains("unable to index") {
                return Err(git_failed(&args, self.root(), &output.stderr));
            }
            let tree = self.git_stdout_env(&["write-tree"], &env)?;
            let message = format!(
                "callisto checkpoint\n\n{HEAD_TRAILER}: {}\n{TAGS_TRAILER}: {}\n{VERSION_TRAILER}: {}\n{WORKSPACE_TRAILER}: {}\n{DATE_TRAILER}: {date}\n",
                facts.head, facts.tags, facts.callisto_version, facts.workspace
            );
            self.internal_commit(&tree, &[&facts.head, index_commit.as_str()], &message)
        })();
        std::fs::remove_file(&index).ok();
        let commit = created?;
        let args = ["update-ref", name, commit.as_str(), ""];
        let output = self.runner().run("git", &args, self.root())?;
        if !output.success() {
            return Err(if output.stderr.contains("already exists") {
                VcsError::CheckpointHeld { name: name.to_string() }
            } else {
                git_failed(&args, self.root(), &output.stderr)
            });
        }
        Ok(commit)
    }

    /// Removes the checkpoint at `name`; removing one that does not exist is not an error.
    pub fn delete_checkpoint(&self, name: &str) -> Result<(), VcsError> {
        self.git_stdout(&["update-ref", "-d", name]).map(drop)
    }

    /// Whether `path` (relative to this root) has an entry in the index.
    pub fn is_staged(&self, path: &Path) -> Result<bool, VcsError> {
        let path = path.to_string_lossy();
        self.git_stdout(&["--literal-pathspecs", "ls-files", "--", &path])
            .map(|listed| !listed.is_empty())
    }

    /// Whether `path` (relative to this root) is on disk as `rev` recorded it, compared as Git compares: through its
    /// line-ending and attribute filters, so a checkout with CRLF endings matches the blob it was written from. Absent
    /// on both sides matches.
    pub fn path_matches_at(&self, rev: &str, path: &Path) -> Result<bool, VcsError> {
        Ok(self.blob_hash_at(rev, path)? == self.file_hash(path)?)
    }

    /// The object id of `path` (relative to this root) in `rev`'s tree, or `None` when it is absent there.
    pub fn blob_hash_at(&self, rev: &str, path: &Path) -> Result<Option<String>, VcsError> {
        if !self.path_exists_at(rev, path)? {
            return Ok(None);
        }
        let spec = format!("{rev}:./{}", path.display());
        self.git_stdout(&["rev-parse", &spec]).map(Some)
    }

    /// The object id Git would give the file at `path` (relative to this root) as it is on disk now, through its
    /// line-ending and attribute filters, or `None` when it is absent. Two ids are equal exactly when Git would
    /// store the same content.
    pub fn file_hash(&self, path: &Path) -> Result<Option<String>, VcsError> {
        let full = self.root().join(path);
        if !full.is_file() {
            return Ok(None);
        }
        let name = path.to_string_lossy();
        let file = full.to_string_lossy();
        self.git_stdout(&["hash-object", "--path", &name, &file]).map(Some)
    }

    /// Checks the checkpoint at `name` out at `dir` (which must not exist) with `HEAD` on the commit the run started
    /// from, so history reads (commit inference) see the real commits and not the checkpoint's own. The files and the
    /// index stay as the checkpoint recorded them.
    pub fn add_checkpoint_worktree(&self, name: &str, dir: &Path, head: &str) -> Result<(), VcsError> {
        // The scratch checkout is internal: none of the user's hooks run in it, and a failed step takes its
        // registration with it.
        let dir_text = dir.to_string_lossy();
        let no_hooks = ["-c", "core.hooksPath=/dev/null"];
        let add: Vec<&str> = no_hooks
            .iter()
            .copied()
            .chain(["worktree", "add", "--detach", "--quiet", &dir_text, name])
            .collect();
        self.git_stdout(&add)?;
        let reset: Vec<&str> = no_hooks
            .iter()
            .copied()
            .chain(["reset", "--quiet", "--soft", head])
            .collect();
        let output = self.runner().run("git", &reset, dir)?;
        if !output.success() {
            self.remove_worktree(dir).ok();
            return Err(git_failed(&reset, dir, &output.stderr));
        }
        Ok(())
    }

    /// This root's path inside the repository (empty at the top), so the same directory can be found in a worktree.
    pub fn root_prefix(&self) -> Result<std::path::PathBuf, VcsError> {
        self.git_stdout(&["rev-parse", "--show-prefix"])
            .map(std::path::PathBuf::from)
    }

    /// The files under `dir` (relative to this root) in `rev`'s tree, relative to this root.
    pub fn files_at(&self, rev: &str, dir: &Path) -> Result<Vec<std::path::PathBuf>, VcsError> {
        let dir = dir.to_string_lossy();
        let listed = self.git_stdout(&["ls-tree", "-r", "-z", "--name-only", rev, "--", &dir])?;
        Ok(nul_separated(&listed).map(std::path::PathBuf::from).collect())
    }

    /// Every file under this root with one of `names` as its file name, in `rev`'s tree or on disk (tracked or
    /// untracked, not ignored), relative to this root.
    pub fn files_named(
        &self,
        rev: &str,
        names: &[&str],
    ) -> Result<std::collections::BTreeSet<std::path::PathBuf>, VcsError> {
        let at_rev = self.git_stdout(&["ls-tree", "-r", "-z", "--name-only", rev, "--", "."])?;
        let on_disk = self.git_stdout(&[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            ".",
        ])?;
        Ok(nul_separated(&at_rev)
            .chain(nul_separated(&on_disk))
            .map(std::path::PathBuf::from)
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| names.contains(&name))
            })
            .collect())
    }

    /// Removes a worktree [`Self::add_checkpoint_worktree`] added.
    pub fn remove_worktree(&self, dir: &Path) -> Result<(), VcsError> {
        let dir = dir.to_string_lossy();
        self.git_stdout(&["worktree", "remove", "--force", &dir]).map(drop)
    }

    /// An unsigned commit of `tree` with fixed identity: it records state and is never pushed, so the user's signing
    /// key and identity play no part.
    fn internal_commit(&self, tree: &str, parents: &[&str], message: &str) -> Result<CommitSha, VcsError> {
        let mut args = vec![
            "-c",
            "user.name=Callisto",
            "-c",
            "user.email=callisto@localhost",
            "commit-tree",
            "--no-gpg-sign",
            tree,
        ];
        for parent in parents {
            args.extend(["-p", parent]);
        }
        args.extend(["-m", message]);
        let sha = self.git_stdout(&args)?;
        CommitSha::parse(&sha).map_err(|e| VcsError::InvalidCommitSha {
            context: "in `git commit-tree` output".to_string(),
            message: e.to_string(),
        })
    }

    fn git_stdout(&self, args: &[&str]) -> Result<String, VcsError> {
        self.git_stdout_env(args, &[])
    }

    fn git_stdout_env(&self, args: &[&str], env: &[(&str, &str)]) -> Result<String, VcsError> {
        let output = self.runner().run_with_env("git", args, self.root(), env)?;
        if !output.success() {
            return Err(git_failed(args, self.root(), &output.stderr));
        }
        Ok(output.stdout_line().to_string())
    }
}

/// The entries of `-z` output: Git quotes no name there, so each is the path's own bytes.
fn nul_separated(listed: &str) -> impl Iterator<Item = &str> {
    listed.split('\0').filter(|entry| !entry.is_empty())
}

fn git_failed(args: &[&str], root: &Path, stderr: &str) -> VcsError {
    VcsError::Git(format!(
        "`git {}` failed in `{}`: {}",
        args.join(" "),
        root.display(),
        stderr.trim()
    ))
}
