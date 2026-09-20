//! The two forge roles: one draft GitHub release per prepared tag, and the
//! publication that closes it once every asset has been uploaded.

use callisto_model::{
    ExactEvidence, ProviderConflictReason, ProviderEvidenceV1, ProviderIndeterminateCause, ProviderObservationV1,
    TagName,
};

use crate::error::{CommandFailure, RemoteConflict};
use crate::GraphError;

use super::super::github::{github_release_for_tag, GitHubReleaseLookup};
use super::policy::timeouts;
use super::{
    confirmed_evidence, wrong_role, EffectAuthorization, ForgePublishOperation, ForgeReleaseOperation,
    PreparedOperation, ProviderCapabilities, ProviderContext, ProviderRequest, ReleaseProvider,
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
        observed_forge_release(
            context,
            &operation.tag,
            operation.prerelease,
            &repository,
            Draft::Either,
        )
    }

    fn publish(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
        _effect: &EffectAuthorization<'_>,
    ) -> Result<ExactEvidence, GraphError> {
        let operation = forge_operation(request)?;
        let repository = context.github_repository_slug()?;
        let mut create_args = vec![
            "release",
            "create",
            operation.tag.as_str(),
            "--repo",
            repository.as_str(),
            "--verify-tag",
            "--draft",
            "--generate-notes",
        ];
        if operation.prerelease {
            create_args.push("--prerelease");
        }
        run_gh(context, &create_args, timeouts::FORGE_RELEASE_CREATE)?;
        confirmed_evidence(
            observed_forge_release(
                context,
                &operation.tag,
                operation.prerelease,
                &repository,
                Draft::Either,
            )?,
            request.id,
            RemoteConflict::ForgeReleaseNotObservedAfterCreate,
        )
    }
}

pub(crate) struct ForgePublishProvider;

impl ReleaseProvider for ForgePublishProvider {
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
        let operation = publish_operation(request)?;
        let repository = context.github_repository_slug()?;
        observed_forge_release(
            context,
            &operation.tag,
            operation.prerelease,
            &repository,
            Draft::PublishedOnly,
        )
    }

    fn publish(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
        _effect: &EffectAuthorization<'_>,
    ) -> Result<ExactEvidence, GraphError> {
        let operation = publish_operation(request)?;
        let repository = context.github_repository_slug()?;
        run_gh(
            context,
            &[
                "release",
                "edit",
                operation.tag.as_str(),
                "--repo",
                repository.as_str(),
                "--draft=false",
            ],
            timeouts::FORGE_RELEASE_CREATE,
        )?;
        confirmed_evidence(
            observed_forge_release(
                context,
                &operation.tag,
                operation.prerelease,
                &repository,
                Draft::PublishedOnly,
            )?,
            request.id,
            RemoteConflict::ForgeReleaseNotObservedAfterPublish,
        )
    }
}

/// Whether a still-draft release satisfies the observing role.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Draft {
    Either,
    PublishedOnly,
}

fn run_gh(context: &ProviderContext<'_>, args: &[&str], timeout: std::time::Duration) -> Result<(), GraphError> {
    let output = context.runner().run_with_timeout("gh", args, context.root(), timeout)?;
    if output.success() {
        return Ok(());
    }
    Err(GraphError::ReleaseCommand {
        program: "gh".to_owned(),
        args: args.iter().map(ToString::to_string).collect(),
        failure: CommandFailure::NonZeroExit {
            exit_code: output.exit_code,
            stderr: output.stderr,
        },
    })
}

fn forge_operation<'a>(request: &ProviderRequest<'a>) -> Result<&'a ForgeReleaseOperation, GraphError> {
    match request.operation {
        PreparedOperation::ForgeRelease(operation) => Ok(operation),
        _ => Err(wrong_role(request.id, "forge release")),
    }
}

fn publish_operation<'a>(request: &ProviderRequest<'a>) -> Result<&'a ForgePublishOperation, GraphError> {
    match request.operation {
        PreparedOperation::ForgePublish(operation) => Ok(operation),
        _ => Err(wrong_role(request.id, "forge publish")),
    }
}

/// A release created for an existing tag reports the repository's default
/// branch as `target_commitish`, so that field proves nothing about the
/// released commit. The tag operation is a DAG prerequisite of the forge
/// release, so the tag already binds this release's name to its commit.
fn observed_forge_release(
    context: &ProviderContext<'_>,
    tag: &TagName,
    prerelease: bool,
    repository: &str,
    draft_policy: Draft,
) -> Result<ProviderObservationV1, GraphError> {
    let value = match github_release_for_tag(context.root(), context.runner(), context.sleeper(), repository, tag)? {
        GitHubReleaseLookup::Absent => return Ok(ProviderObservationV1::Absent),
        GitHubReleaseLookup::Indeterminate { status } => {
            return Ok(ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::ProviderStatus { status },
            })
        }
        GitHubReleaseLookup::Found(value) => value,
    };
    if value.get("tag_name").and_then(serde_json::Value::as_str) != Some(tag.as_str()) {
        return Ok(ProviderObservationV1::Conflict {
            reason: ProviderConflictReason::ForgeReleaseDiffers,
        });
    }
    if value
        .get("prerelease")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        != prerelease
    {
        return Ok(ProviderObservationV1::Conflict {
            reason: ProviderConflictReason::ForgeReleasePrereleaseDiffers,
        });
    }
    let draft = value.get("draft").and_then(serde_json::Value::as_bool).unwrap_or(false);
    if draft && draft_policy == Draft::PublishedOnly {
        return Ok(ProviderObservationV1::Absent);
    }
    Ok(ProviderObservationV1::Exact {
        evidence: ProviderEvidenceV1::ForgeRelease {
            tag_name: tag.clone(),
            draft,
        },
    })
}

/// The existence check every artifact upload makes against the draft it
/// attaches to; the upload role owns no forge-release identity of its own.
pub(crate) fn observed_draft_or_published_release(
    context: &ProviderContext<'_>,
    tag: &TagName,
    prerelease: bool,
    repository: &str,
) -> Result<ProviderObservationV1, GraphError> {
    observed_forge_release(context, tag, prerelease, repository, Draft::Either)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use callisto_model::{CommandError, CommandOutput, CommandRunner};

    use super::super::policy::tests::RecordingSleeper;
    use super::*;

    /// A `gh` that answers only the two read endpoints, from a canned map of
    /// endpoint substring to JSON body.
    struct ScriptedGh {
        tag_endpoint_body: Option<String>,
        pages: Vec<String>,
    }

    impl CommandRunner for ScriptedGh {
        fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
            assert_eq!(program, "gh");
            let endpoint = args.last().copied().unwrap_or_default();
            let body = if endpoint.contains("/releases/tags/") {
                self.tag_endpoint_body.clone()
            } else {
                let page: usize = endpoint
                    .rsplit_once("page=")
                    .map(|(_, page)| page.parse().expect("page number"))
                    .expect("list endpoint carries a page");
                Some(self.pages.get(page - 1).cloned().unwrap_or_else(|| "[]".to_owned()))
            };
            Ok(match body {
                Some(body) => CommandOutput {
                    exit_code: Some(0),
                    stdout: format!("HTTP/1.1 200 OK\n\n{body}\n"),
                    stderr: String::new(),
                },
                None => CommandOutput {
                    exit_code: Some(1),
                    stdout: "HTTP/1.1 404 Not Found\n\n{}\n".to_owned(),
                    stderr: String::new(),
                },
            })
        }
    }

    fn release_json(tag: &str, draft: bool, prerelease: bool) -> String {
        format!(
            "{{\"tag_name\":\"{tag}\",\"draft\":{draft},\"prerelease\":{prerelease},\"immutable\":true,\"assets\":[]}}"
        )
    }

    fn observe(runner: &ScriptedGh, prerelease: bool, policy: Draft) -> ProviderObservationV1 {
        let sleeper = RecordingSleeper::new();
        let root = std::env::temp_dir();
        let context = ProviderContext::new(&root, runner, None).with_sleeper(&sleeper);
        let tag = TagName::new_unchecked("callisto@0.2.0".to_owned());
        observed_forge_release(&context, &tag, prerelease, "example/core-crate", policy).unwrap()
    }

    #[test]
    fn a_draft_is_found_through_the_list_endpoint_and_is_not_yet_published() {
        let runner = ScriptedGh {
            tag_endpoint_body: None,
            pages: vec![format!("[{}]", release_json("callisto@0.2.0", true, false))],
        };
        assert!(matches!(
            observe(&runner, false, Draft::Either),
            ProviderObservationV1::Exact {
                evidence: ProviderEvidenceV1::ForgeRelease { draft: true, .. }
            }
        ));
        assert!(
            matches!(
                observe(&runner, false, Draft::PublishedOnly),
                ProviderObservationV1::Absent
            ),
            "a draft must leave the publish operation eligible, never satisfied"
        );
    }

    #[test]
    fn a_published_release_satisfies_both_forge_roles() {
        let runner = ScriptedGh {
            tag_endpoint_body: Some(release_json("callisto@0.2.0", false, false)),
            pages: vec![],
        };
        for policy in [Draft::Either, Draft::PublishedOnly] {
            assert!(matches!(
                observe(&runner, false, policy),
                ProviderObservationV1::Exact {
                    evidence: ProviderEvidenceV1::ForgeRelease { draft: false, .. }
                }
            ));
        }
    }

    #[test]
    fn a_release_whose_prerelease_flag_disagrees_with_the_version_is_a_conflict() {
        let runner = ScriptedGh {
            tag_endpoint_body: Some(release_json("callisto@0.2.0", false, false)),
            pages: vec![],
        };
        assert!(matches!(
            observe(&runner, true, Draft::Either),
            ProviderObservationV1::Conflict {
                reason: ProviderConflictReason::ForgeReleasePrereleaseDiffers
            }
        ));
    }

    #[test]
    fn the_list_scan_pages_until_it_finds_the_draft() {
        let runner = ScriptedGh {
            tag_endpoint_body: None,
            pages: vec![
                format!("[{}]", release_json("unrelated@0.0.1", false, false)),
                format!("[{}]", release_json("callisto@0.2.0", true, true)),
            ],
        };
        assert!(matches!(
            observe(&runner, true, Draft::Either),
            ProviderObservationV1::Exact {
                evidence: ProviderEvidenceV1::ForgeRelease { draft: true, .. }
            }
        ));
    }

    #[test]
    fn an_exhausted_bounded_scan_reports_absence_rather_than_paging_forever() {
        let runner = ScriptedGh {
            tag_endpoint_body: None,
            pages: vec![],
        };
        assert!(matches!(
            observe(&runner, false, Draft::Either),
            ProviderObservationV1::Absent
        ));
    }
}
