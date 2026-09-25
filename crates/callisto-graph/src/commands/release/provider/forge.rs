//! The two forge roles: one draft GitHub release per prepared tag, and the
//! publication that closes it once every asset has been uploaded.

use callisto_model::{
    ExactEvidence, ProviderConflictReason, ProviderEvidenceV1, ProviderIndeterminateCause, ProviderObservationV1,
    ReleasePackageId, TagName,
};

use crate::error::{CommandFailure, RemoteConflict};
use crate::GraphError;

use super::super::github::{github_release_for_tag, GitHubReleaseLookup};
use super::policy::{programs, timeouts};
use super::{
    confirmed_evidence, wrong_role, EffectAuthorization, ForgePublishOperation, ForgeReleaseOperation, NotesFallback,
    PreparedOperation, ProviderCapabilities, ProviderContext, ProviderRequest, ReleaseNotes, ReleaseProvider,
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
        effect: &EffectAuthorization<'_>,
    ) -> Result<ExactEvidence, GraphError> {
        let operation = forge_operation(request)?;
        let repository = context.github_repository_slug()?;
        // Held until `gh` has read the file.
        let notes_dir;
        let notes_file = match &operation.notes {
            ReleaseNotes::Section(section) => {
                notes_dir = tempfile::tempdir().map_err(notes_file_error)?;
                let path = notes_dir.path().join("notes.md");
                callisto_model::atomic::atomic_write(&path, section, effect.permit).map_err(notes_file_error)?;
                Some(path.to_string_lossy().into_owned())
            }
            ReleaseNotes::Generated { reason } => {
                eprintln!("{}", generated_notes_notice(&request.id.package, *reason));
                None
            }
        };
        let create_args = release_create_args(operation, &repository, notes_file.as_deref());
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

/// `gh release create` for a draft: the changelog section when `notes_file` is set, else generated notes.
fn release_create_args<'a>(
    operation: &'a ForgeReleaseOperation,
    repository: &'a str,
    notes_file: Option<&'a str>,
) -> Vec<&'a str> {
    let mut args = vec![
        "release",
        "create",
        operation.tag.as_str(),
        "--repo",
        repository,
        "--verify-tag",
        "--draft",
    ];
    match notes_file {
        Some(path) => args.extend(["--notes-file", path]),
        None => args.push("--generate-notes"),
    }
    if operation.prerelease {
        args.push("--prerelease");
    }
    args
}

fn generated_notes_notice(package: &ReleasePackageId, reason: NotesFallback) -> String {
    format!("notes: using generated notes ({reason}) for {package}")
}

fn notes_file_error(error: std::io::Error) -> GraphError {
    GraphError::ReleaseInputRead {
        path: "notes.md".into(),
        message: error.to_string(),
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
    let output = context
        .runner()
        .run_with_timeout(programs::GH, args, context.root(), timeout)?;
    if output.success() {
        return Ok(());
    }
    Err(GraphError::ReleaseCommand {
        program: programs::GH.to_owned(),
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

    use super::super::loopback::fixtures;
    use super::super::policy::tests::RecordingSleeper;
    use super::*;

    /// A `gh` that answers only the two read endpoints with raw `gh api
    /// --include` stdout: a tag-endpoint answer (or the captured 404) and one
    /// answer per list page (an empty array past the last).
    struct ScriptedGh {
        tag_endpoint: Option<String>,
        pages: Vec<String>,
    }

    impl CommandRunner for ScriptedGh {
        fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
            assert_eq!(program, "gh");
            assert!(!args.contains(&"--repo"), "`gh api` defines no --repo: {args:?}");
            let endpoint = args.last().copied().unwrap_or_default();
            let stdout = if endpoint.contains("/releases/tags/") {
                self.tag_endpoint.clone()
            } else {
                let page: usize = endpoint
                    .rsplit_once("page=")
                    .map(|(_, page)| page.parse().expect("page number"))
                    .expect("list endpoint carries a page");
                Some(
                    self.pages
                        .get(page - 1)
                        .cloned()
                        .unwrap_or_else(|| fixtures::gh_api_stdout("[]")),
                )
            };
            Ok(match stdout {
                Some(stdout) => CommandOutput {
                    exit_code: Some(0),
                    stdout,
                    stderr: String::new(),
                },
                None => CommandOutput {
                    exit_code: Some(1),
                    stdout: fixtures::GITHUB_RELEASE_404.to_owned(),
                    stderr: "gh: Not Found (HTTP 404)".to_owned(),
                },
            })
        }
    }

    fn release_json(tag: &str, draft: bool, prerelease: bool) -> String {
        fixtures::github_release(tag, draft, prerelease, "main", &[]).to_string()
    }

    fn served(body: &str) -> Option<String> {
        Some(fixtures::gh_api_stdout(body))
    }

    fn observe_tag(runner: &ScriptedGh, tag: &str, prerelease: bool, policy: Draft) -> ProviderObservationV1 {
        let sleeper = RecordingSleeper::new();
        let root = std::env::temp_dir();
        let context = ProviderContext::new(&root, runner, None).with_sleeper(&sleeper);
        let tag = TagName::new_unchecked(tag.to_owned());
        observed_forge_release(&context, &tag, prerelease, "example/core-crate", policy).unwrap()
    }

    fn observe(runner: &ScriptedGh, prerelease: bool, policy: Draft) -> ProviderObservationV1 {
        observe_tag(runner, "callisto@0.2.0", prerelease, policy)
    }

    #[test]
    fn a_draft_is_found_through_the_list_endpoint_and_is_not_yet_published() {
        let runner = ScriptedGh {
            tag_endpoint: None,
            pages: vec![fixtures::gh_api_stdout(&format!(
                "[{}]",
                release_json("callisto@0.2.0", true, false)
            ))],
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
            tag_endpoint: served(&release_json("callisto@0.2.0", false, false)),
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
            tag_endpoint: served(&release_json("callisto@0.2.0", false, false)),
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
            tag_endpoint: None,
            pages: vec![
                fixtures::gh_api_stdout(&format!("[{}]", release_json("unrelated@0.0.1", false, false))),
                fixtures::gh_api_stdout(&format!("[{}]", release_json("callisto@0.2.0", true, true))),
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
            tag_endpoint: None,
            pages: vec![],
        };
        assert!(matches!(
            observe(&runner, false, Draft::Either),
            ProviderObservationV1::Absent
        ));
    }

    fn forge_operation_with(notes: ReleaseNotes) -> ForgeReleaseOperation {
        ForgeReleaseOperation {
            tag: TagName::new_unchecked("core@1.0.0".to_owned()),
            prerelease: false,
            notes,
        }
    }

    /// A changelog section is passed as `--notes-file`, never with `--generate-notes`.
    #[test]
    fn changelog_section_is_passed_as_a_notes_file() {
        let operation = forge_operation_with(ReleaseNotes::Section("- fix".to_owned()));
        let args = release_create_args(&operation, "example/core", Some("/tmp/notes.md"));
        assert!(
            args.windows(2).any(|pair| pair == ["--notes-file", "/tmp/notes.md"]),
            "{args:?}"
        );
        assert!(!args.contains(&"--generate-notes"), "{args:?}");
    }

    /// Without a usable section the release falls back to generated notes and says why.
    #[test]
    fn generated_notes_fallback_names_the_package_and_reason() {
        let operation = forge_operation_with(ReleaseNotes::Generated {
            reason: NotesFallback::SectionMissing,
        });
        let args = release_create_args(&operation, "example/core", None);
        assert!(args.contains(&"--generate-notes"), "{args:?}");
        assert!(!args.contains(&"--notes-file"), "{args:?}");
        let package = ReleasePackageId::new(callisto_model::Ecosystem::Cargo, "core").unwrap();
        let mut reasons = std::collections::BTreeSet::new();
        for reason in [
            NotesFallback::FileMissing,
            NotesFallback::SectionMissing,
            NotesFallback::SectionEmpty,
            NotesFallback::Unreadable,
        ] {
            let notice = generated_notes_notice(&package, reason);
            assert!(notice.starts_with("notes: using generated notes ("), "{notice}");
            assert!(notice.ends_with(&format!(") for {package}")), "{notice}");
            reasons.insert(notice);
        }
        assert_eq!(reasons.len(), 4, "each fallback reason must be distinguishable");
    }

    // Raw captured GitHub responses, headers included (testing/fixtures/providers/github).

    const CAPTURED_TAG: &str = "v2.101.0";

    fn draft_body() -> String {
        format!("[{}]", fixtures::body_of(fixtures::GITHUB_RELEASE_DRAFT).trim())
    }

    #[test]
    fn the_captured_published_release_is_exact_and_not_a_draft() {
        let runner = ScriptedGh {
            tag_endpoint: Some(fixtures::GITHUB_RELEASE_PUBLISHED.to_owned()),
            pages: vec![],
        };
        for policy in [Draft::Either, Draft::PublishedOnly] {
            assert!(matches!(
                observe_tag(&runner, CAPTURED_TAG, false, policy),
                ProviderObservationV1::Exact {
                    evidence: ProviderEvidenceV1::ForgeRelease { draft: false, .. }
                }
            ));
        }
    }

    #[test]
    fn the_captured_release_shape_under_a_draft_flag_is_a_draft_only_the_list_serves() {
        let runner = ScriptedGh {
            tag_endpoint: None,
            pages: vec![fixtures::gh_api_stdout(&draft_body())],
        };
        assert!(matches!(
            observe_tag(&runner, CAPTURED_TAG, false, Draft::Either),
            ProviderObservationV1::Exact {
                evidence: ProviderEvidenceV1::ForgeRelease { draft: true, .. }
            }
        ));
        assert!(matches!(
            observe_tag(&runner, CAPTURED_TAG, false, Draft::PublishedOnly),
            ProviderObservationV1::Absent
        ));
    }

    #[test]
    fn the_captured_release_shape_under_a_prerelease_flag_conflicts_with_a_stable_version() {
        let runner = ScriptedGh {
            tag_endpoint: Some(fixtures::GITHUB_RELEASE_PRERELEASE.to_owned()),
            pages: vec![],
        };
        assert!(matches!(
            observe_tag(&runner, CAPTURED_TAG, false, Draft::Either),
            ProviderObservationV1::Conflict {
                reason: ProviderConflictReason::ForgeReleasePrereleaseDiffers
            }
        ));
        assert!(matches!(
            observe_tag(&runner, CAPTURED_TAG, true, Draft::Either),
            ProviderObservationV1::Exact { .. }
        ));
    }

    #[test]
    fn the_captured_list_page_finds_a_listed_tag_and_reports_an_unlisted_one_absent() {
        let listed: serde_json::Value = serde_json::from_str(fixtures::body_of(fixtures::GITHUB_RELEASE_LIST)).unwrap();
        let tag = listed[1]["tag_name"].as_str().unwrap().to_owned();
        let runner = ScriptedGh {
            tag_endpoint: None,
            pages: vec![fixtures::GITHUB_RELEASE_LIST.to_owned()],
        };
        assert!(matches!(
            observe_tag(&runner, &tag, false, Draft::Either),
            ProviderObservationV1::Exact { .. }
        ));
        assert!(matches!(
            observe_tag(&runner, "callisto-no-such-tag", false, Draft::Either),
            ProviderObservationV1::Absent
        ));
    }
}
