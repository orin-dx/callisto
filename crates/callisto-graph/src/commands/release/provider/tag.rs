//! The tag role: a local annotated tag plus the push that makes it remote.

use std::path::Path;

use callisto_model::{
    CommandRunner, CommitSha, ExactEvidence, ProviderConflictReason, ProviderEvidenceV1, ProviderIndeterminateCause,
    ProviderObservationV1, TagName,
};
use callisto_vcs::{GitAccess, TagSignPolicy};

use crate::error::{CommandFailure, RemoteConflict};
use crate::GraphError;

use super::policy::{self, programs, run_observation, timeouts, Attempt};
use super::{
    confirmed_evidence, wrong_role, EffectAuthorization, PreparedOperation, ProviderCapabilities, ProviderContext,
    ProviderRequest, ReleaseProvider, TagOperation,
};

/// What the local repository holds at `refs/tags/<name>`. A tag's landed
/// effect is its target commit plus being annotated; annotation text is
/// never part of its identity.
#[derive(Debug, PartialEq, Eq)]
enum LocalTagObservation {
    Absent,
    Annotated {
        target: CommitSha,
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
            let git = GitAccess::new(context.root(), context.runner());
            git.create_tag(
                operation.name.as_str(),
                &operation.target,
                Some(&operation.annotation),
                TagSignPolicy::ForceUnsigned,
                effect.permit,
            )?;
        }
        let remote_endpoint = context.checked_git_remote()?.endpoint.clone();
        push_tag(context.runner(), context.root(), &remote_endpoint, operation)?;
        confirmed_evidence(
            tag_observation(context, operation)?,
            request.id,
            RemoteConflict::TagNotObservedAfterPush,
        )
    }
}

/// GitHub's refusal when an App token pushes a ref whose tree's workflows
/// differ from every branch tip.
const WORKFLOW_GUARD_REFUSAL: &str = "refusing to allow a GitHub App to create or update workflow";

fn push_tag(
    runner: &dyn CommandRunner,
    root: &Path,
    endpoint: &str,
    operation: &TagOperation,
) -> Result<(), GraphError> {
    let push_args = ["push", endpoint, operation.name.as_str()];
    let pushed = runner.run_with_timeout(programs::GIT, &push_args, root, timeouts::GIT_PUSH)?;
    if pushed.exit_code == Some(0) {
        return Ok(());
    }
    if pushed.stderr.contains(WORKFLOW_GUARD_REFUSAL) {
        return Err(GraphError::ReleaseTagPushRefusedWorkflowGuard {
            tag: operation.name.as_str().to_owned(),
            target: operation.target.as_str().to_owned(),
        });
    }
    Err(GraphError::ReleaseCommand {
        program: programs::GIT.to_string(),
        args: push_args.iter().map(ToString::to_string).collect(),
        failure: CommandFailure::NonZeroExit {
            exit_code: pushed.exit_code,
            stderr: pushed.stderr,
        },
    })
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
        LocalTagObservation::Annotated { target: observed } if observed == operation.target => {}
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
    let observed = match run_observation(
        context.runner(),
        programs::GIT,
        &args,
        context.root(),
        timeouts::GIT_LS_REMOTE,
        true,
    )? {
        Ok(observed) => observed,
        Err(_unavailable) => {
            return Ok(Attempt::Transient {
                value: RemoteTagObservation::Indeterminate,
                retry_after: None,
            })
        }
    };
    if observed.exit_code != Some(0) {
        return Ok(Attempt::Transient {
            value: RemoteTagObservation::Indeterminate,
            retry_after: None,
        });
    }
    classify_ls_remote(&observed.stdout, &reference, &peeled, &args).map(Attempt::Settled)
}

/// Reads `ls-remote` output for `reference` and its peeled `^{}` form.
fn classify_ls_remote(
    stdout: &str,
    reference: &str,
    peeled: &str,
    args: &[&str],
) -> Result<RemoteTagObservation, GraphError> {
    let mut tag_object = None;
    let mut peeled_commit = None;
    for line in stdout.lines() {
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
        return Ok(RemoteTagObservation::Absent);
    };
    if peeled_commit.is_none() {
        return Ok(RemoteTagObservation::Unannotated);
    }
    let target = CommitSha::parse(sha.trim()).map_err(|error| GraphError::ReleaseCommand {
        program: programs::GIT.to_string(),
        args: args.iter().map(ToString::to_string).collect(),
        failure: CommandFailure::MalformedOutput {
            detail: error.to_string(),
        },
    })?;
    Ok(RemoteTagObservation::Annotated { target })
}

fn observed_local_tag(context: &ProviderContext<'_>, name: &TagName) -> Result<LocalTagObservation, GraphError> {
    let reference = format!("refs/tags/{name}^{{commit}}");
    let rev_parse_args = ["rev-parse", "--verify", "--quiet", reference.as_str()];
    let observed =
        context
            .runner()
            .run_with_timeout(programs::GIT, &rev_parse_args, context.root(), timeouts::LOCAL_GIT)?;
    if observed.exit_code != Some(0) {
        return Ok(LocalTagObservation::Absent);
    }
    let target = CommitSha::parse(observed.stdout.trim()).map_err(|error| GraphError::ReleaseCommand {
        program: programs::GIT.to_string(),
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
    let details =
        context
            .runner()
            .run_with_timeout(programs::GIT, &for_each_ref_args, context.root(), timeouts::LOCAL_GIT)?;
    if details.exit_code != Some(0) {
        return Err(GraphError::ReleaseCommand {
            program: programs::GIT.to_string(),
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
            program: programs::GIT.to_string(),
            args: for_each_ref_args.iter().map(ToString::to_string).collect(),
            failure: CommandFailure::MalformedOutput {
                detail: format!("unexpected for-each-ref output: {line:?}"),
            },
        });
    }
    // Identity is target commit plus being annotated; the annotation text
    // itself is not part of that identity, but its presence still confirms
    // this is a well-formed annotated tag object.
    annotation.ok_or_else(|| GraphError::ReleaseInvariant {
        detail: "for-each-ref line validated as `tag`/empty-body but carried no annotation field".to_string(),
    })?;
    Ok(LocalTagObservation::Annotated { target })
}

#[cfg(test)]
mod tests {
    use super::super::loopback::fixtures;
    use super::*;

    /// Delegates local git plumbing (`rev-parse`, `for-each-ref`) to a real
    /// repo at `root`, but scripts `remote`/`ls-remote` so the remote side is
    /// under test control without a real push.
    struct LocalRepoRemoteScript {
        url: &'static str,
        ls_remote_stdout: &'static str,
    }

    impl CommandRunner for LocalRepoRemoteScript {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            cwd: &Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            assert_eq!(program, "git");
            match args.first() {
                Some(&"remote") => Ok(callisto_model::CommandOutput {
                    exit_code: Some(0),
                    stdout: self.url.to_owned(),
                    stderr: String::new(),
                }),
                Some(&"ls-remote") => Ok(callisto_model::CommandOutput {
                    exit_code: Some(0),
                    stdout: self.ls_remote_stdout.to_owned(),
                    stderr: String::new(),
                }),
                _ => callisto_fixtures::git::GitRunner.run(program, args, cwd),
            }
        }
    }

    /// `git rev-parse HEAD` against the real repo at `root`, via the same
    /// [`CommandRunner`] impl `LocalRepoRemoteScript` delegates non-remote
    /// commands to.
    fn head_sha(root: &Path) -> CommitSha {
        let output = callisto_fixtures::git::GitRunner
            .run("git", &["rev-parse", "HEAD"], root)
            .unwrap();
        CommitSha::parse(output.stdout.trim()).unwrap()
    }

    fn local_annotated_tag_operation(root: &Path, name: &str, message: &str) -> TagOperation {
        let target = head_sha(root);
        let git = GitAccess::new(root, &callisto_fixtures::git::GitRunner);
        git.create_tag(
            name,
            &target,
            Some(message),
            TagSignPolicy::ForceUnsigned,
            &callisto_model::ApplyPermit::force_for_tests(),
        )
        .unwrap();
        TagOperation {
            name: TagName::new_unchecked(name.to_owned()),
            target,
            annotation: message.to_owned(),
        }
    }

    /// Regression for the tag-identity fix: a local annotated tag that names
    /// the prepared commit but carries different annotation *text* is the
    /// same landed effect, not a conflict (annotation text is never part of
    /// tag identity). Before the fix this returned `Conflict` without ever
    /// checking the remote.
    #[test]
    fn local_tag_with_matching_target_and_different_annotation_text_is_not_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        callisto_fixtures::git::init_repo(dir.path());
        callisto_fixtures::git::run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "root"]);
        let mut operation = local_annotated_tag_operation(dir.path(), "callisto@0.8.0", "operation's own message");
        operation.annotation = "a completely different message than the local tag carries".to_owned();

        let remote =
            crate::commands::release::binding::canonical_git_remote("https://github.com/orin-dx/callisto").unwrap();
        let runner = LocalRepoRemoteScript {
            url: "https://github.com/orin-dx/callisto",
            ls_remote_stdout: fixtures::LS_REMOTE_ABSENT,
        };
        let context = ProviderContext::new(dir.path(), &runner, Some(&remote));

        assert_eq!(
            tag_observation(&context, &operation).unwrap(),
            ProviderObservationV1::Absent
        );
    }

    /// Regression: a local lightweight tag at the right commit still
    /// conflicts. Only annotation *text* stopped mattering; being annotated
    /// at all did not.
    #[test]
    fn local_lightweight_tag_at_the_right_commit_still_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        callisto_fixtures::git::init_repo(dir.path());
        callisto_fixtures::git::run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "root"]);
        let target = head_sha(dir.path());
        callisto_fixtures::git::run_git(dir.path(), &["tag", "callisto@0.8.0", target.as_str()]);
        let operation = TagOperation {
            name: TagName::new_unchecked("callisto@0.8.0".to_owned()),
            target,
            annotation: "Release callisto@0.8.0".to_owned(),
        };

        let remote =
            crate::commands::release::binding::canonical_git_remote("https://github.com/orin-dx/callisto").unwrap();
        let runner = LocalRepoRemoteScript {
            url: "https://github.com/orin-dx/callisto",
            ls_remote_stdout: fixtures::LS_REMOTE_ABSENT,
        };
        let context = ProviderContext::new(dir.path(), &runner, Some(&remote));

        assert_eq!(
            tag_observation(&context, &operation).unwrap(),
            ProviderObservationV1::Conflict {
                reason: ProviderConflictReason::LocalTagDiffers,
            }
        );
    }

    /// Remote path: `ls-remote` only ever compares the peeled commit
    /// (annotation text is unreadable over `ls-remote` to begin with), so an
    /// annotated remote tag on the right commit is adopted regardless of the
    /// prepared operation's own annotation text.
    #[test]
    fn remote_tag_on_the_right_commit_is_adopted_regardless_of_annotation_text() {
        let dir = tempfile::tempdir().unwrap();
        callisto_fixtures::git::init_repo(dir.path());
        let operation = TagOperation {
            name: TagName::new_unchecked("callisto-changelog@0.3.1".to_owned()),
            target: CommitSha::parse("caf945cc9d5a11a71c57f419d1a73d0627c1756b").unwrap(),
            annotation: "a message that has nothing to do with the remote tag's own message".to_owned(),
        };
        let remote =
            crate::commands::release::binding::canonical_git_remote("https://github.com/orin-dx/callisto").unwrap();
        let runner = LocalRepoRemoteScript {
            url: "https://github.com/orin-dx/callisto",
            ls_remote_stdout: fixtures::LS_REMOTE_ANNOTATED,
        };
        let context = ProviderContext::new(dir.path(), &runner, Some(&remote));

        assert_eq!(
            tag_observation(&context, &operation).unwrap(),
            ProviderObservationV1::Exact {
                evidence: ProviderEvidenceV1::GitTag {
                    peeled_commit: CommitSha::parse("caf945cc9d5a11a71c57f419d1a73d0627c1756b").unwrap(),
                },
            }
        );
    }

    fn classify(captured: &str, tag: &str) -> Result<RemoteTagObservation, GraphError> {
        let reference = format!("refs/tags/{tag}");
        let peeled = format!("{reference}^{{}}");
        classify_ls_remote(captured, &reference, &peeled, &["ls-remote"])
    }

    #[test]
    fn a_captured_annotated_tag_resolves_to_its_peeled_commit() {
        assert_eq!(
            classify(fixtures::LS_REMOTE_ANNOTATED, "callisto-changelog@0.3.1").unwrap(),
            RemoteTagObservation::Annotated {
                target: CommitSha::parse("caf945cc9d5a11a71c57f419d1a73d0627c1756b").unwrap()
            }
        );
    }

    #[test]
    fn a_captured_lightweight_tag_is_unannotated() {
        assert_eq!(
            classify(fixtures::LS_REMOTE_LIGHTWEIGHT, "callisto-vcs@0.2.0").unwrap(),
            RemoteTagObservation::Unannotated
        );
    }

    #[test]
    fn captured_empty_output_is_an_absent_tag_and_garbled_output_is_not() {
        assert_eq!(
            classify(fixtures::LS_REMOTE_ABSENT, "callisto-no-such-tag").unwrap(),
            RemoteTagObservation::Absent
        );
        let garbled = fixtures::LS_REMOTE_ANNOTATED.replace("caf945cc9d5a11a71c57f419d1a73d0627c1756b", "not-a-sha");
        assert!(matches!(
            classify(&garbled, "callisto-changelog@0.3.1"),
            Err(GraphError::ReleaseCommand {
                failure: CommandFailure::MalformedOutput { .. },
                ..
            })
        ));
    }

    /// Answers `git remote get-url` deterministically, then hands back one
    /// scripted `ls-remote` result per call.
    struct FlakyLsRemote {
        url: &'static str,
        results: std::sync::Mutex<
            std::collections::VecDeque<Result<callisto_model::CommandOutput, callisto_model::CommandError>>,
        >,
    }

    impl CommandRunner for FlakyLsRemote {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            assert_eq!(program, "git");
            if args.first() == Some(&"remote") {
                return Ok(callisto_model::CommandOutput {
                    exit_code: Some(0),
                    stdout: self.url.to_owned(),
                    stderr: String::new(),
                });
            }
            assert_eq!(args.first(), Some(&"ls-remote"));
            self.results.lock().unwrap().pop_front().expect("script exhausted")
        }
    }

    /// Regression: a `git ls-remote` that times out once must retry and
    /// settle, not abort the whole observation with a hard error (the bug
    /// was `observed_remote_tag` handing `CommandError::TimedOut` straight to
    /// `?`, which `retry_observation` never saw).
    #[test]
    fn observed_remote_tag_retries_past_a_timed_out_ls_remote() {
        let remote =
            crate::commands::release::binding::canonical_git_remote("https://github.com/orin-dx/callisto").unwrap();
        let runner = FlakyLsRemote {
            url: "https://github.com/orin-dx/callisto",
            results: std::sync::Mutex::new(
                vec![
                    Err(callisto_model::CommandError::TimedOut {
                        program: "git".to_owned(),
                        seconds: 60,
                    }),
                    Ok(callisto_model::CommandOutput {
                        exit_code: Some(0),
                        stdout: fixtures::LS_REMOTE_ABSENT.to_owned(),
                        stderr: String::new(),
                    }),
                ]
                .into(),
            ),
        };
        let sleeper = policy::tests::RecordingSleeper::new();
        let root = std::env::temp_dir();
        let context = ProviderContext::new(&root, &runner, Some(&remote)).with_sleeper(&sleeper);
        let name = TagName::new_unchecked("callisto-no-such-tag".to_owned());
        let result = policy::retry_observation(context.sleeper(), || observed_remote_tag(&context, &name));
        assert_eq!(result, Ok(RemoteTagObservation::Absent));
        assert_eq!(sleeper.waits(), vec![std::time::Duration::from_secs(2)]);
    }

    struct FailedPush(&'static str);

    impl CommandRunner for FailedPush {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            assert_eq!((program, args.first().copied()), ("git", Some("push")));
            Ok(callisto_model::CommandOutput {
                exit_code: Some(1),
                stdout: String::new(),
                stderr: self.0.to_owned(),
            })
        }
    }

    const TARGET: &str = "caf945cc9d5a11a71c57f419d1a73d0627c1756b";

    fn push(stderr: &'static str) -> GraphError {
        let operation = TagOperation {
            name: TagName::new_unchecked("callisto@0.8.0".to_owned()),
            target: CommitSha::parse(TARGET).unwrap(),
            annotation: "callisto@0.8.0".to_owned(),
        };
        push_tag(
            &FailedPush(stderr),
            &std::env::temp_dir(),
            "https://github.com/orin-dx/callisto",
            &operation,
        )
        .unwrap_err()
    }

    #[test]
    fn the_github_app_workflow_guard_refusal_is_its_own_diagnostic() {
        let error = push(
            "To https://github.com/orin-dx/callisto\n \
             ! [remote rejected] callisto@0.8.0 -> callisto@0.8.0 (refusing to allow a GitHub App to create \
             or update workflow `.github/workflows/release.yml` without `workflows` permission)\n\
             error: failed to push some refs to 'https://github.com/orin-dx/callisto'\n",
        );
        assert_eq!(
            error,
            GraphError::ReleaseTagPushRefusedWorkflowGuard {
                tag: "callisto@0.8.0".to_owned(),
                target: TARGET.to_owned(),
            }
        );
        assert_eq!(
            miette::Diagnostic::code(&error).map(|code| code.to_string()).as_deref(),
            Some("E180")
        );
        let help = miette::Diagnostic::help(&error).unwrap().to_string();
        assert!(help.contains("callisto@0.8.0") && help.contains(TARGET), "{help}");
    }

    #[test]
    fn any_other_push_failure_stays_a_release_command_failure() {
        assert!(matches!(
            push("fatal: unable to access 'https://github.com/orin-dx/callisto/': Could not resolve host\n"),
            GraphError::ReleaseCommand {
                failure: CommandFailure::NonZeroExit { .. },
                ..
            }
        ));
    }
}
