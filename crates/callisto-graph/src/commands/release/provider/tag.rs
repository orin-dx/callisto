//! The tag role: a local annotated tag plus the push that makes it remote.

use callisto_model::{
    CommitSha, ExactEvidence, ProviderConflictReason, ProviderEvidenceV1, ProviderIndeterminateCause,
    ProviderObservationV1, TagName,
};
use callisto_vcs::{GitAccess, GitDataSource, TagSignPolicy};

use crate::error::{CommandFailure, RemoteConflict};
use crate::GraphError;

use super::policy::{self, timeouts, Attempt};
use super::{
    confirmed_evidence, wrong_role, EffectAuthorization, PreparedOperation, ProviderCapabilities, ProviderContext,
    ProviderRequest, ReleaseProvider, TagOperation,
};

/// What the local repository holds at `refs/tags/<name>`.
#[derive(Debug, PartialEq, Eq)]
enum LocalTagObservation {
    Absent,
    Annotated {
        target: CommitSha,
        annotation: String,
    },
    /// A lightweight tag, or any other object this adapter did not write.
    Unannotated,
}

/// What the prepared remote holds at `refs/tags/<name>`. `ls-remote` cannot
/// read annotation text, so only the tagged commit is comparable.
#[derive(Debug, PartialEq, Eq)]
enum RemoteTagObservation {
    Absent,
    Annotated { target: CommitSha },
    Unannotated,
    Indeterminate,
}

pub(crate) struct TagProvider;

impl ReleaseProvider for TagProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            can_observe: true,
            can_publish: true,
        }
    }

    fn preflight_conflict(&self) -> RemoteConflict {
        RemoteConflict::TagTargetDiffers
    }

    fn observe(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
    ) -> Result<ProviderObservationV1, GraphError> {
        tag_observation(context, tag_operation(request)?)
    }

    fn publish(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
        effect: &EffectAuthorization<'_>,
    ) -> Result<ExactEvidence, GraphError> {
        let operation = tag_operation(request)?;
        // The preflight observation already proved the remote absent. It leaves
        // exactly as prepared by a run whose push failed; only the push remains.
        if observed_local_tag(context, &operation.name)? == LocalTagObservation::Absent {
            // Delegates to `GitAccess::create_tag` rather than inlining `git
            // tag` argv, so this inherits its `--` end-of-options separator
            // (defends `name` against being misread as a flag) on top of the
            // `--no-sign` this durable path has always needed (this repo's CI
            // sets `tag.gpgSign`/`commit.gpgsign` globally in some contexts,
            // with no tag-signing key available here).
            let git = GitAccess::discover(context.root(), context.runner());
            git.create_tag(
                operation.name.as_str(),
                &operation.target,
                Some(&operation.annotation),
                TagSignPolicy::ForceUnsigned,
                effect.permit,
            )?;
        }
        let remote_endpoint = context.checked_git_remote()?.endpoint.clone();
        let push_args = ["push", remote_endpoint.as_str(), operation.name.as_str()];
        let pushed = context
            .runner()
            .run_with_timeout("git", &push_args, context.root(), timeouts::GIT_PUSH)?;
        if pushed.exit_code != Some(0) {
            return Err(GraphError::ReleaseCommand {
                program: "git".to_string(),
                args: push_args.iter().map(ToString::to_string).collect(),
                failure: CommandFailure::NonZeroExit {
                    exit_code: pushed.exit_code,
                    stderr: pushed.stderr,
                },
            });
        }
        confirmed_evidence(
            tag_observation(context, operation)?,
            request.id,
            RemoteConflict::TagNotObservedAfterPush,
        )
    }
}

fn tag_operation<'a>(request: &ProviderRequest<'a>) -> Result<&'a TagOperation, GraphError> {
    match request.operation {
        PreparedOperation::Tag(operation) => Ok(operation),
        _ => Err(wrong_role(request.id, "tag")),
    }
}

/// The remote decides whether the tag operation is satisfied, but a local
/// ref that contradicts the prepared tag is still a conflict: the next push
/// would carry it.
fn tag_observation(
    context: &ProviderContext<'_>,
    operation: &TagOperation,
) -> Result<ProviderObservationV1, GraphError> {
    match observed_local_tag(context, &operation.name)? {
        LocalTagObservation::Absent => {}
        LocalTagObservation::Annotated {
            target: observed,
            annotation: observed_annotation,
        } if observed == operation.target && observed_annotation == operation.annotation => {}
        _ => {
            return Ok(ProviderObservationV1::Conflict {
                reason: ProviderConflictReason::LocalTagDiffers,
            })
        }
    }
    let remote = policy::retry_observation(context.sleeper(), || observed_remote_tag(context, &operation.name))?;
    Ok(match remote {
        RemoteTagObservation::Absent => ProviderObservationV1::Absent,
        RemoteTagObservation::Annotated { target: observed } if observed == operation.target => {
            ProviderObservationV1::Exact {
                evidence: ProviderEvidenceV1::GitTag {
                    peeled_commit: observed,
                },
            }
        }
        RemoteTagObservation::Annotated { .. } => ProviderObservationV1::Conflict {
            reason: ProviderConflictReason::RemoteTagTargetDiffers,
        },
        RemoteTagObservation::Unannotated => ProviderObservationV1::Conflict {
            reason: ProviderConflictReason::UnannotatedTag,
        },
        RemoteTagObservation::Indeterminate => ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::CommandFailed,
        },
    })
}

/// Observes `refs/tags/<name>` on the prepared remote. A git failure leaves
/// the remote unknown; absence must be proved by a successful query. The
/// failure is transient: `ls-remote` is a read, so retrying it cannot have an
/// effect.
fn observed_remote_tag(
    context: &ProviderContext<'_>,
    name: &TagName,
) -> Result<Attempt<RemoteTagObservation>, GraphError> {
    let endpoint = context.checked_git_remote()?.endpoint.clone();
    let reference = format!("refs/tags/{name}");
    // The peeled ref resolves an annotated tag to its commit; a lightweight
    // tag has no peeled line at all.
    let peeled = format!("{reference}^{{}}");
    let args = ["ls-remote", endpoint.as_str(), reference.as_str(), peeled.as_str()];
    let observed = context
        .runner()
        .run_quiet("git", &args, context.root(), timeouts::GIT_LS_REMOTE)?;
    if observed.exit_code != Some(0) {
        return Ok(Attempt::Transient {
            value: RemoteTagObservation::Indeterminate,
            retry_after: None,
        });
    }
    let mut tag_object = None;
    let mut peeled_commit = None;
    for line in observed.stdout.lines() {
        let Some((sha, found)) = line.split_once('\t') else {
            continue;
        };
        match found.trim() {
            found if found == peeled => peeled_commit = Some(sha),
            found if found == reference => tag_object = Some(sha),
            _ => {}
        }
    }
    let Some(sha) = peeled_commit.or(tag_object) else {
        return Ok(Attempt::Settled(RemoteTagObservation::Absent));
    };
    if peeled_commit.is_none() {
        return Ok(Attempt::Settled(RemoteTagObservation::Unannotated));
    }
    let target = CommitSha::parse(sha.trim()).map_err(|error| GraphError::ReleaseCommand {
        program: "git".to_string(),
        args: args.iter().map(ToString::to_string).collect(),
        failure: CommandFailure::MalformedOutput {
            detail: error.to_string(),
        },
    })?;
    Ok(Attempt::Settled(RemoteTagObservation::Annotated { target }))
}

fn observed_local_tag(context: &ProviderContext<'_>, name: &TagName) -> Result<LocalTagObservation, GraphError> {
    let reference = format!("refs/tags/{name}^{{commit}}");
    let rev_parse_args = ["rev-parse", "--verify", "--quiet", reference.as_str()];
    let observed = context
        .runner()
        .run_with_timeout("git", &rev_parse_args, context.root(), timeouts::LOCAL_GIT)?;
    if observed.exit_code != Some(0) {
        return Ok(LocalTagObservation::Absent);
    }
    let target = CommitSha::parse(observed.stdout.trim()).map_err(|error| GraphError::ReleaseCommand {
        program: "git".to_string(),
        args: rev_parse_args.iter().map(ToString::to_string).collect(),
        failure: CommandFailure::MalformedOutput {
            detail: error.to_string(),
        },
    })?;
    let for_each_ref_target = format!("refs/tags/{name}");
    let for_each_ref_args = [
        "for-each-ref",
        "--format=%(objecttype)%00%(contents:subject)%00%(contents:body)",
        for_each_ref_target.as_str(),
    ];
    let details = context
        .runner()
        .run_with_timeout("git", &for_each_ref_args, context.root(), timeouts::LOCAL_GIT)?;
    if details.exit_code != Some(0) {
        return Err(GraphError::ReleaseCommand {
            program: "git".to_string(),
            args: for_each_ref_args.iter().map(ToString::to_string).collect(),
            failure: CommandFailure::NonZeroExit {
                exit_code: details.exit_code,
                stderr: details.stderr,
            },
        });
    }
    let line = details.stdout.trim_end_matches(['\r', '\n']);
    let mut fields = line.split('\0');
    let object_type = fields.next();
    let annotation = fields.next();
    let body = fields.next();
    // A lightweight tag is a conflicting ref, not malformed git output.
    if object_type != Some("tag") {
        return Ok(LocalTagObservation::Unannotated);
    }
    if body.is_none_or(|body| !body.trim().is_empty()) || fields.next().is_some() {
        return Err(GraphError::ReleaseCommand {
            program: "git".to_string(),
            args: for_each_ref_args.iter().map(ToString::to_string).collect(),
            failure: CommandFailure::MalformedOutput {
                detail: format!("unexpected for-each-ref output: {line:?}"),
            },
        });
    }
    let annotation = annotation.ok_or_else(|| GraphError::ReleaseInvariant {
        detail: "for-each-ref line validated as `tag`/empty-body but carried no annotation field".to_string(),
    })?;
    Ok(LocalTagObservation::Annotated {
        target,
        annotation: annotation.to_string(),
    })
}
