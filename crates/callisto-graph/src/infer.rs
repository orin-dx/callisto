use std::path::PathBuf;

use callisto_model::{CommitSha, Package, Severity, Version};

use crate::config::PreMajorInferencePolicy;
use crate::error::GraphError;

pub trait SeverityInference: Send + Sync {
    fn infer(
        &self,
        pkg: &Package,
        window: InferenceWindowSpec<'_>,
    ) -> Result<Option<InferenceOutcome>, GraphError>;
}

pub struct InferenceWindowSpec<'a> {
    pub pathspecs: &'a [PathBuf],
    pub since: Option<CommitSha>,
    pub current_version: &'a Version,
    pub has_prior_release: bool,
    pub policy: PreMajorInferencePolicy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferenceOutcome {
    pub severity: Severity,
    pub commit_count: usize,
    pub remapped: bool,
    pub commits: Vec<(CommitSha, String)>,
}

pub struct NoInference;

impl SeverityInference for NoInference {
    fn infer(
        &self,
        _pkg: &Package,
        _window: InferenceWindowSpec<'_>,
    ) -> Result<Option<InferenceOutcome>, GraphError> {
        Ok(None)
    }
}

#[cfg(feature = "inference")]
pub struct CommitInference<'a, R: CommandRunner> {
    pub runner: &'a R,
    pub root: PathBuf,
}

#[cfg(feature = "inference")]
impl<'a, R: CommandRunner> SeverityInference for CommitInference<'a, R> {
    fn infer(
        &self,
        pkg: &Package,
        window: InferenceWindowSpec<'_>,
    ) -> Result<Option<InferenceOutcome>, GraphError> {
        use callisto_conventional::{infer_severity, InferenceInput, InferenceWindow};

        let inf_window = match window.since {
            Some(sha) => InferenceWindow::SinceCommit(sha),
            None => InferenceWindow::FullHistory,
        };

        let input = InferenceInput {
            package: &pkg.id,
            pathspecs: window.pathspecs,
            window: inf_window,
            current_version: window.current_version,
            has_prior_release: window.has_prior_release,
        };

        let raw = infer_severity(self.runner, &self.root, &input)?;
        if raw.commit_count == 0 {
            return Ok(None);
        }

        let (severity, remapped) = crate::aggregate::apply_pre_major(
            raw.severity,
            window.policy,
            window.current_version,
            window.has_prior_release,
        );

        let commits = raw
            .commits
            .into_iter()
            .map(|c| (c.sha().clone(), c.subject().to_string()))
            .collect();

        Ok(Some(InferenceOutcome {
            severity,
            commit_count: raw.commit_count,
            remapped,
            commits,
        }))
    }
}
