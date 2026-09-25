//! The artifact-upload role: one verified asset attached to a forge release.

use callisto_model::{
    ExactEvidence, ProviderConflictReason, ProviderEvidenceV1, ProviderIndeterminateCause, ProviderObservationV1,
};

use crate::error::{CommandFailure, ReleasePreconditionRequirement, RemoteConflict};
use crate::GraphError;

use super::super::github::{
    github_release_endpoint, github_release_for_tag, malformed_github_response, GitHubReleaseLookup,
};
use super::forge::observed_draft_or_published_release;
use super::policy::{programs, timeouts};
use super::{
    confirmed_evidence, wrong_role, ArtifactUploadOperation, EffectAuthorization, PreparedOperation,
    ProviderCapabilities, ProviderContext, ProviderRequest, ReleaseProvider,
};

pub(crate) struct ArtifactUploadProvider;

impl ReleaseProvider for ArtifactUploadProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            can_observe: true,
            can_publish: true,
        }
    }

    fn preflight_conflict(&self) -> RemoteConflict {
        RemoteConflict::ArtifactDiffers
    }

    fn observe(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
    ) -> Result<ProviderObservationV1, GraphError> {
        observe_artifact_upload(context, artifact_operation(request)?, request.artifacts)
    }

    fn preflight(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
    ) -> Result<super::ReleasePreflight, GraphError> {
        // An upload cannot be judged absent without the manifest that names
        // the exact bytes it would carry.
        request.artifacts.ok_or(GraphError::ReleasePreconditionUnmet {
            requirement: ReleasePreconditionRequirement::VerifiedArtifactManifest,
        })?;
        super::preflight_from_observation(self.observe(context, request)?, request.id, self.preflight_conflict())
    }

    fn publish(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
        _effect: &EffectAuthorization<'_>,
    ) -> Result<ExactEvidence, GraphError> {
        let operation = artifact_operation(request)?;
        let artifacts = request.artifacts.ok_or(GraphError::ReleasePreconditionUnmet {
            requirement: ReleasePreconditionRequirement::VerifiedArtifactManifest,
        })?;
        let path = artifacts.path_for(&operation.slot)?;
        let repository = operation.slot.attestation_policy.repository.as_slug();
        confirmed_evidence(
            observed_draft_or_published_release(context, &operation.tag, operation.prerelease, &repository)?,
            request.id,
            RemoteConflict::ForgeReleaseDiffers,
        )?;
        let path_argument = path.to_string_lossy();
        let args = [
            "release",
            "upload",
            operation.tag.as_str(),
            path_argument.as_ref(),
            "--repo",
            repository.as_str(),
        ];
        let uploaded =
            context
                .runner()
                .run_with_timeout(programs::GH, &args, context.root(), timeouts::FORGE_ASSET_UPLOAD)?;
        if !uploaded.success() {
            return Err(GraphError::ReleaseCommand {
                program: programs::GH.to_owned(),
                args: args.iter().map(ToString::to_string).collect(),
                failure: CommandFailure::NonZeroExit {
                    exit_code: uploaded.exit_code,
                    stderr: uploaded.stderr,
                },
            });
        }
        confirmed_evidence(
            observe_artifact_upload(context, operation, Some(artifacts))?,
            request.id,
            RemoteConflict::ArtifactNotObservedAfterUpload,
        )
    }
}

fn artifact_operation<'a>(request: &ProviderRequest<'a>) -> Result<&'a ArtifactUploadOperation, GraphError> {
    match request.operation {
        PreparedOperation::ArtifactUpload(operation) => Ok(operation),
        _ => Err(wrong_role(request.id, "artifact upload")),
    }
}

fn observe_artifact_upload(
    context: &ProviderContext<'_>,
    operation: &ArtifactUploadOperation,
    artifacts: Option<&super::VerifiedArtifactManifest<'_>>,
) -> Result<ProviderObservationV1, GraphError> {
    let Some(artifacts) = artifacts else {
        return Ok(ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::ArtifactManifestUnavailable,
        });
    };
    let entry = artifacts.entry_for(&operation.slot)?;
    let repository = operation.slot.attestation_policy.repository.as_slug();
    let release = match github_release_for_tag(
        context.root(),
        context.runner(),
        context.sleeper(),
        &repository,
        &operation.tag,
    )? {
        GitHubReleaseLookup::Absent => return Ok(ProviderObservationV1::Absent),
        GitHubReleaseLookup::Indeterminate { status } => {
            return Ok(ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::ProviderStatus { status },
            })
        }
        GitHubReleaseLookup::CommandFailed => {
            return Ok(ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::CommandFailed,
            })
        }
        GitHubReleaseLookup::Found(release) => release,
    };
    let assets = release
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            malformed_github_response(
                &github_release_endpoint(&repository, &operation.tag),
                "GitHub release response has no assets array",
            )
        })?;
    let matching: Vec<_> = assets
        .iter()
        .filter(|asset| {
            asset.get("name").and_then(serde_json::Value::as_str) == Some(operation.slot.asset_name.as_str())
        })
        .collect();
    match matching.as_slice() {
        [] => Ok(ProviderObservationV1::Absent),
        [asset] => {
            let digest = format!("sha256:{}", entry.digest.as_str());
            if asset.get("size").and_then(serde_json::Value::as_u64) == Some(entry.byte_length)
                && asset.get("digest").and_then(serde_json::Value::as_str) == Some(digest.as_str())
            {
                Ok(ProviderObservationV1::Exact {
                    evidence: ProviderEvidenceV1::ArtifactUpload {
                        byte_length: entry.byte_length,
                        sha256: entry.digest.clone(),
                    },
                })
            } else {
                Ok(ProviderObservationV1::Conflict {
                    reason: ProviderConflictReason::ArtifactAssetDiffers,
                })
            }
        }
        _ => Ok(ProviderObservationV1::Conflict {
            reason: ProviderConflictReason::DuplicateArtifactAsset,
        }),
    }
}
