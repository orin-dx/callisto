use std::path::PathBuf;

use callisto_model::{CommitSha, Package, Severity, Version};
use callisto_vcs::GitAccess;

use crate::config::PreMajorInferencePolicy;
use crate::error::GraphError;

pub trait SeverityInference: Send + Sync {
    /// `git` is the caller's already-discovered [`GitAccess`] (e.g.
    /// `aggregate()`'s own parameter, itself `Workspace`-shared) -- an impl
    /// that needs commit history must use this rather than discovering its
    /// own, so an N-package workspace pays for one discovery, not N.
    fn infer(
        &self,
        pkg: &Package,
        git: &GitAccess<'_>,
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
        _git: &GitAccess<'_>,
        _window: InferenceWindowSpec<'_>,
    ) -> Result<Option<InferenceOutcome>, GraphError> {
        Ok(None)
    }
}

/// v0.2's impl, behind the `inference` feature. A thin, stateless adapter over
/// `callisto_conventional::infer_severity` -- it holds no fields of its own; the caller's
/// [`GitAccess`] is handed in per call via [`SeverityInference::infer`]'s `git` parameter
/// rather than discovered here, so this type carries nothing to discover it with.
#[cfg(feature = "inference")]
pub struct CommitInference;

#[cfg(feature = "inference")]
impl SeverityInference for CommitInference {
    fn infer(
        &self,
        pkg: &Package,
        git: &GitAccess<'_>,
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

        // Selecting the VCS backend is this layer's job now: callisto-conventional
        // takes any `callisto_model::CommitWalker` and never names a VCS crate.
        let raw = infer_severity(git, &input)?;
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
