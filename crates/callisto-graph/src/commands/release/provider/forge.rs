//! The forge-release role: one GitHub release per prepared tag.

use callisto_model::{
    CommandRunner, ExactEvidence, ProviderConflictReason, ProviderEvidenceV1, ProviderIndeterminateCause,
    ProviderObservationV1, TagName,
};

use crate::error::{CommandFailure, RemoteConflict};
use crate::GraphError;

use super::super::github::{github_release_by_tag, GitHubReleaseLookup};
use super::policy::timeouts;
use super::{
    confirmed_evidence, wrong_role, EffectAuthorization, ForgeReleaseOperation, PreparedOperation,
    ProviderCapabilities, ProviderContext, ProviderRequest, ReleaseProvider,
};

pub(crate) struct ForgeReleaseProvider;

impl ReleaseProvider for ForgeReleaseProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            can_observe: true,
            can_publish: true,
        }
    }

    fn preflight_conflict(&self) -> RemoteConflict {
        RemoteConflict::ForgeReleaseDiffers
    }

    fn observe(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
    ) -> Result<ProviderObservationV1, GraphError> {
        let operation = forge_operation(request)?;
        let repository = context.github_repository_slug()?;
        observed_forge_release_target(context.root(), context.runner(), &operation.tag, &repository)
    }

    fn publish(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
        _effect: &EffectAuthorization<'_>,
    ) -> Result<ExactEvidence, GraphError> {
        let operation = forge_operation(request)?;
        let repository = context.github_repository_slug()?;
        let create_args = [
            "release",
            "create",
            operation.tag.as_str(),
            "--repo",
            repository.as_str(),
            "--verify-tag",
            "--generate-notes",
        ];
        let created =
            context
                .runner()
                .run_with_timeout("gh", &create_args, context.root(), timeouts::FORGE_RELEASE_CREATE)?;
        if created.exit_code != Some(0) {
            return Err(GraphError::ReleaseCommand {
                program: "gh".to_string(),
                args: create_args.iter().map(ToString::to_string).collect(),
                failure: CommandFailure::NonZeroExit {
                    exit_code: created.exit_code,
                    stderr: created.stderr,
                },
            });
        }
        confirmed_evidence(
            observed_forge_release_target(context.root(), context.runner(), &operation.tag, &repository)?,
            request.id,
            RemoteConflict::ForgeReleaseNotObservedAfterCreate,
        )
    }
}

fn forge_operation<'a>(request: &ProviderRequest<'a>) -> Result<&'a ForgeReleaseOperation, GraphError> {
    match request.operation {
        PreparedOperation::ForgeRelease(operation) => Ok(operation),
        _ => Err(wrong_role(request.id, "forge release")),
    }
}

/// A release created for an existing tag reports the repository's default
/// branch as `target_commitish`, so that field proves nothing about the
/// released commit. The tag operation is a DAG prerequisite of the forge
/// release, so the tag already binds this release's name to its commit.
pub(crate) fn observed_forge_release_target(
    root: &std::path::Path,
    runner: &dyn CommandRunner,
    tag: &TagName,
    repository: &str,
) -> Result<ProviderObservationV1, GraphError> {
    let value = match github_release_by_tag(root, runner, repository, tag)? {
        GitHubReleaseLookup::Absent => return Ok(ProviderObservationV1::Absent),
        GitHubReleaseLookup::Indeterminate { status } => {
            return Ok(ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::ProviderStatus { status },
            })
        }
        GitHubReleaseLookup::Found(value) => value,
    };
    let draft = value.get("draft").and_then(serde_json::Value::as_bool).unwrap_or(false);
    Ok(
        if !draft && value.get("tag_name").and_then(serde_json::Value::as_str) == Some(tag.as_str()) {
            ProviderObservationV1::Exact {
                evidence: ProviderEvidenceV1::ForgeRelease {
                    tag_name: tag.clone(),
                    draft,
                },
            }
        } else {
            ProviderObservationV1::Conflict {
                reason: ProviderConflictReason::ForgeReleaseDiffers,
            }
        },
    )
}
