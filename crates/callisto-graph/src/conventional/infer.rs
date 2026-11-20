use std::path::PathBuf;

use callisto_model::{CommitWalker, Severity};

use crate::conventional::{fetch_commits, raw_severity_of, ConventionalError, InferenceWindow, ParsedCommit};

pub struct InferenceInput<'a> {
    pub pathspecs: &'a [PathBuf],
    pub window: InferenceWindow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferredSeverity {
    pub severity: Severity,
    pub commits: Vec<ParsedCommit>,
}

pub fn infer_severity(
    walker: &dyn CommitWalker,
    input: &InferenceInput<'_>,
) -> Result<InferredSeverity, ConventionalError> {
    let commits = fetch_commits(walker, &input.window, input.pathspecs)?;

    let mut max_severity = Severity::None;
    for commit in &commits {
        let sev = raw_severity_of(commit);
        if sev > max_severity {
            max_severity = sev;
        }
    }

    Ok(InferredSeverity {
        severity: max_severity,
        commits,
    })
}
