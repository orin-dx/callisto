use std::path::{Path, PathBuf};

use callisto_model::{CommandRunner, CommitSha};

use crate::{parse_commit, ConventionalError, ParsedCommit};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InferenceWindow {
    SinceCommit(CommitSha),
    FullHistory,
}

pub fn fetch_commits(
    runner: &dyn CommandRunner,
    cwd: &Path,
    window: &InferenceWindow,
    pathspecs: &[PathBuf],
) -> Result<Vec<ParsedCommit>, ConventionalError> {
    let mut args = vec!["log", "--no-merges", "--format=%H%x1f%B%x1e"];

    let range_arg;
    match window {
        InferenceWindow::SinceCommit(sha) => {
            range_arg = format!("{}..HEAD", sha.as_str());
            args.push(&range_arg);
        }
        InferenceWindow::FullHistory => {
            args.push("HEAD");
        }
    }

    let path_strings: Vec<String>;
    if !pathspecs.is_empty() {
        args.push("--");
        path_strings = pathspecs.iter().map(|p| p.display().to_string()).collect();
        for p in &path_strings {
            args.push(p);
        }
    }

    let output = runner.run("git", &args, cwd)?;
    if !output.success() {
        return Err(ConventionalError::GitLogFailed {
            cwd: cwd.to_path_buf(),
            stderr: output.stderr,
        });
    }

    let mut commits = Vec::new();
    for raw_commit in output.stdout.split('\x1e') {
        let trimmed = raw_commit.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((sha_str, body)) = trimmed.split_once('\x1f') else {
            continue;
        };
        let sha =
            CommitSha::parse(sha_str).map_err(|_| ConventionalError::MalformedGitLogOutput {
                cwd: cwd.to_path_buf(),
                message: format!("invalid sha {sha_str:?}"),
            })?;
        commits.push(parse_commit(sha, body));
    }

    Ok(commits)
}
