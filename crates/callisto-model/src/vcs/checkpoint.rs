//! A checkpoint records a local command's inputs before its first write, so an interrupted run can be resumed from
//! them or undone ([ADR 13](../../../../docs/adr/0013-interrupted-version-runs-resume-from-a-checkpoint-ref.md)).

use std::path::Path;

use sha2::{Digest, Sha256};

use super::access::GitAccess;
use super::VcsError;
use crate::CommitSha;

/// Where an interrupted `callisto version` run keeps its inputs.
pub const VERSION_CHECKPOINT: &str = "refs/callisto/checkpoint/version";

const HEAD_TRAILER: &str = "Callisto-Head";
const TAGS_TRAILER: &str = "Callisto-Tags";
const VERSION_TRAILER: &str = "Callisto-Version";

/// The inputs a plan reads besides files, recorded in the checkpoint commit's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointFacts {
    pub head: String,
    pub tags: String,
    pub callisto_version: String,
}

/// A recorded checkpoint: a commit whose tree is the working tree, whose first parent is `HEAD` and whose second
/// parent records the index.
#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub commit: CommitSha,
    pub facts: CheckpointFacts,
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
            },
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
        })
    }

    /// Records `HEAD`, the index, and the working tree under this root (untracked files included, ignored files
    /// left out) at `name`, without touching the user's index or working tree.
    ///
    /// # Errors
    ///
    /// [`VcsError::CheckpointHeld`] when `name` already exists, so two runs never share one; a failed git command
    /// otherwise.
    pub fn create_checkpoint(&self, name: &str, facts: &CheckpointFacts) -> Result<CommitSha, VcsError> {
        let git_dir = self.git_stdout(&["rev-parse", "--absolute-git-dir"])?;
        let index = Path::new(git_dir.trim()).join("callisto-checkpoint.index");
        let index_str = index.to_string_lossy().into_owned();
        let env = [("GIT_INDEX_FILE", index_str.as_str())];
        let created = (|| {
            let index_commit = self.internal_commit(&self.git_stdout(&["write-tree"])?, &[&facts.head], "index")?;
            self.git_stdout_env(&["read-tree", "HEAD"], &env)?;
            self.git_stdout_env(&["add", "-A", "--", "."], &env)?;
            let tree = self.git_stdout_env(&["write-tree"], &env)?;
            let message = format!(
                "callisto checkpoint\n\n{HEAD_TRAILER}: {}\n{TAGS_TRAILER}: {}\n{VERSION_TRAILER}: {}\n",
                facts.head, facts.tags, facts.callisto_version
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

    /// Whether `path` (relative to this root) is on disk as `rev` recorded it, compared as Git compares: through its
    /// line-ending and attribute filters, so a checkout with CRLF endings matches the blob it was written from. Absent
    /// on both sides matches.
    pub fn path_matches_at(&self, rev: &str, path: &Path) -> Result<bool, VcsError> {
        let recorded = if self.path_exists_at(rev, path)? {
            let spec = format!("{rev}:./{}", path.display());
            Some(self.git_stdout(&["rev-parse", &spec])?)
        } else {
            None
        };
        let full = self.root().join(path);
        let current = if full.is_file() {
            let path = path.to_string_lossy();
            let file = full.to_string_lossy();
            Some(self.git_stdout(&["hash-object", "--path", &path, &file])?)
        } else {
            None
        };
        Ok(recorded == current)
    }

    /// Checks the checkpoint at `name` out, detached, at `dir` (which must not exist).
    pub fn add_checkpoint_worktree(&self, name: &str, dir: &Path) -> Result<(), VcsError> {
        let dir = dir.to_string_lossy();
        self.git_stdout(&["worktree", "add", "--detach", "--quiet", &dir, name])
            .map(drop)
    }

    /// This root's path inside the repository (empty at the top), so the same directory can be found in a worktree.
    pub fn root_prefix(&self) -> Result<std::path::PathBuf, VcsError> {
        self.git_stdout(&["rev-parse", "--show-prefix"])
            .map(std::path::PathBuf::from)
    }

    /// Untracked, not ignored files under this root, relative to it.
    pub fn untracked_files(&self) -> Result<Vec<std::path::PathBuf>, VcsError> {
        let listed = self.git_stdout(&["ls-files", "--others", "--exclude-standard", "--", "."])?;
        Ok(listed.lines().map(std::path::PathBuf::from).collect())
    }

    /// The files under `dir` (relative to this root) in `rev`'s tree, relative to this root.
    pub fn files_at(&self, rev: &str, dir: &Path) -> Result<Vec<std::path::PathBuf>, VcsError> {
        let dir = dir.to_string_lossy();
        let listed = self.git_stdout(&["ls-tree", "-r", "--name-only", rev, "--", &dir])?;
        Ok(listed.lines().map(std::path::PathBuf::from).collect())
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
        Ok(output.stdout_trimmed().to_string())
    }
}

fn git_failed(args: &[&str], root: &Path, stderr: &str) -> VcsError {
    VcsError::Git(format!(
        "`git {}` failed in `{}`: {}",
        args.join(" "),
        root.display(),
        stderr.trim()
    ))
}
