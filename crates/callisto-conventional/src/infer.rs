use std::path::{Path, PathBuf};

use callisto_model::{CommandRunner, PackageId, Severity, Version};

use crate::{fetch_commits, raw_severity_of, ConventionalError, InferenceWindow, ParsedCommit};

pub struct InferenceInput<'a> {
    pub package: &'a PackageId,
    pub pathspecs: &'a [PathBuf],
    pub window: InferenceWindow,
    pub current_version: &'a Version,
    pub has_prior_release: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferredSeverity {
    pub severity: Severity,
    pub commit_count: usize,
    pub commits: Vec<ParsedCommit>,
}

pub fn infer_severity(
    runner: &dyn CommandRunner,
    cwd: &Path,
    input: &InferenceInput<'_>,
) -> Result<InferredSeverity, ConventionalError> {
    let commits = fetch_commits(runner, cwd, &input.window, input.pathspecs)?;
    let commit_count = commits.len();

    let mut max_severity = Severity::None;
    for commit in &commits {
        let sev = raw_severity_of(commit);
        if sev > max_severity {
            max_severity = sev;
        }
    }

    Ok(InferredSeverity {
        severity: max_severity,
        commit_count,
        commits,
    })
}
