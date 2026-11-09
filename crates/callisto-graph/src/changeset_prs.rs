//! Which pull request added each changeset, read from the merge commits on the current branch.

use std::collections::BTreeMap;
use std::path::Path;

use callisto_model::vcs::{GitAccess, VcsError};

use crate::error::GraphError;

const COMMIT_MARK: char = '\u{1}';

/// Changeset file name to the number of the pull request that added it. One `git log`, however many changesets there
/// are. A changeset whose commit subject names no pull request is left out.
pub fn changeset_pull_requests(
    git: &GitAccess<'_>,
    changesets_dir: &Path,
) -> Result<BTreeMap<String, u64>, GraphError> {
    let dir = changesets_dir.to_string_lossy();
    let args = [
        "log",
        "--first-parent",
        "-m",
        "--diff-filter=A",
        "--name-only",
        "--format=%x01%s",
        "--",
        dir.as_ref(),
    ];
    let output = git.runner().run("git", &args, git.root()).map_err(VcsError::from)?;
    if !output.success() {
        return Err(VcsError::Git(format!(
            "`git log` failed in `{}`: {}",
            git.root().display(),
            output.redacted_stderr()
        ))
        .into());
    }
    Ok(parse_changeset_prs(&output.stdout))
}

/// The pull request number in a commit subject: `Merge pull request #12 from ...` or `Title (#12)`.
fn pull_request_of(subject: &str) -> Option<u64> {
    if let Some(rest) = subject.strip_prefix("Merge pull request #") {
        return rest.split_whitespace().next()?.parse().ok();
    }
    let tail = subject.trim_end().strip_suffix(')')?;
    tail.rsplit_once("(#")?.1.parse().ok()
}

fn parse_changeset_prs(log: &str) -> BTreeMap<String, u64> {
    let mut found = BTreeMap::new();
    for block in log.split(COMMIT_MARK).skip(1) {
        let mut lines = block.lines();
        let Some(pr) = lines.next().and_then(pull_request_of) else {
            continue;
        };
        for path in lines.filter(|line| !line.is_empty()) {
            if let Some(name) = path.rsplit('/').next() {
                // The log is newest first; the commit that added it first is the last one seen.
                found.insert(name.to_owned(), pr);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_subject_names_its_pull_request_in_either_merge_style() {
        assert_eq!(
            pull_request_of("Merge pull request #479 from orin-dx/feat/pr-labels"),
            Some(479)
        );
        assert_eq!(pull_request_of("feat(cli): label the release PR (#12)"), Some(12));
        assert_eq!(pull_request_of("fix: a thing (see #5) later"), None);
        assert_eq!(pull_request_of("chore: plain subject"), None);
        assert_eq!(pull_request_of("Merge branch 'main' into feat/x"), None);
    }

    /// Real merge commits: the changeset arrives on a branch and the merge commit names the pull request.
    #[test]
    fn real_merge_commits_name_the_pull_request_of_each_changeset() {
        use callisto_fixtures::git::{init_repo, run_git, GitRunner};

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(root.join("README.md"), "x\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "init"]);
        run_git(root, &["switch", "-q", "-c", "feature"]);
        std::fs::write(root.join(".changeset/one.md"), "a\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "add one"]);
        run_git(root, &["switch", "-q", "main"]);
        run_git(
            root,
            &[
                "merge",
                "-q",
                "--no-ff",
                "-m",
                "Merge pull request #7 from o/feature",
                "feature",
            ],
        );
        std::fs::write(root.join(".changeset/two.md"), "b\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "direct push, no pull request"]);

        let git = GitAccess::new(root, &GitRunner);
        let found = changeset_pull_requests(&git, Path::new(".changeset")).unwrap();

        assert_eq!(found.get("one.md"), Some(&7));
        assert_eq!(found.get("two.md"), None);
    }

    #[test]
    fn each_changeset_maps_to_the_pull_request_that_added_it() {
        let log = "\u{1}Merge pull request #481 from o/b\n\n.changeset/a.md\n.changeset/b.md\n\
                   \u{1}Merge pull request #480 from o/a\n\n.changeset/c.md\n\
                   \u{1}Merge branch 'main' into x\n\n.changeset/d.md\n";
        let found = parse_changeset_prs(log);
        assert_eq!(found.get("a.md"), Some(&481));
        assert_eq!(found.get("b.md"), Some(&481));
        assert_eq!(found.get("c.md"), Some(&480));
        assert_eq!(found.get("d.md"), None);
    }
}
