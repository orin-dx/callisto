//! The GitHub Releases read path: argv, HTTP framing, and response parsing.
//!
//! Kept free of any provider so the forge-release and artifact-upload roles
//! share one lookup and cannot drift in how they read an absence.

use std::path::Path;

use callisto_model::{CommandOutput, CommandRunner, TagName};

use crate::error::CommandFailure;
use crate::GraphError;

use super::provider::http::{parse_http_response, HttpResponse};
use super::provider::policy::{programs, retry_observation, run_observation, timeouts, Attempt, Sleeper};

/// One GitHub release lookup, shared by the forge-release and asset adapters.
#[derive(Debug)]
pub(crate) enum GitHubReleaseLookup {
    Absent,
    Indeterminate {
        status: u16,
    },
    /// `gh api` itself timed out or failed to run -- there is no HTTP status
    /// to report, unlike [`Self::Indeterminate`].
    CommandFailed,
    Found(serde_json::Value),
}

pub(crate) fn github_release_endpoint(repository: &str, tag: &TagName) -> String {
    format!("repos/{repository}/releases/tags/{tag}")
}

/// GitHub serves at most 100 releases per page; ten pages is the bound this
/// scan will spend looking for one draft before reporting it unobservable.
const RELEASE_LIST_PAGE_SIZE: usize = 100;
const RELEASE_LIST_PAGE_LIMIT: usize = 10;

fn github_release_list_endpoint(repository: &str, page: usize) -> String {
    format!("repos/{repository}/releases?per_page={RELEASE_LIST_PAGE_SIZE}&page={page}")
}

/// `gh api` defines no `--repo`; the endpoint already carries owner and repository.
pub(crate) fn github_release_api_args(endpoint: &str) -> [&str; 5] {
    ["api", "--include", "--method", "GET", endpoint]
}

fn parse_github_api_response(program: &str, args: &[&str], stdout: &str) -> Result<HttpResponse, GraphError> {
    parse_http_response(stdout).map_err(|detail| GraphError::ReleaseCommand {
        program: program.to_string(),
        args: args.iter().map(ToString::to_string).collect(),
        failure: CommandFailure::MalformedOutput { detail },
    })
}

/// `gh api --include` preserves the HTTP response even for a 404 and exits
/// non-zero. Parse that authoritative response first so absence is not
/// mistaken for a transport failure; retain the command failure when no
/// parseable HTTP response was produced.
fn github_api_response(program: &str, args: &[&str], output: &CommandOutput) -> Result<HttpResponse, GraphError> {
    match parse_github_api_response(program, args, &output.stdout) {
        Ok(response) => Ok(response),
        Err(_) if !output.success() => Err(GraphError::ReleaseCommand {
            program: program.to_owned(),
            args: args.iter().map(ToString::to_string).collect(),
            failure: CommandFailure::NonZeroExit {
                exit_code: output.exit_code,
                stderr: output.stderr.clone(),
            },
        }),
        Err(error) => Err(error),
    }
}

/// Classifies an HTTP response before parsing a GitHub Release body. A 404
/// proves absence; all other non-success statuses leave remote identity
/// unknown. In particular, authentication, rate-limit, and server failures
/// must never be reported as a conflicting release or asset.
fn github_release_response_status(status: u16) -> Option<GitHubReleaseLookup> {
    match status {
        200 => None,
        404 => Some(GitHubReleaseLookup::Absent),
        other => Some(GitHubReleaseLookup::Indeterminate { status: other }),
    }
}

/// The one GET every forge observation shares, under the same bounded retry as
/// every other read-only observation.
///
/// This endpoint never serves drafts, so it proves publication, not existence;
/// [`github_release_for_tag`] is what the roles call.
fn github_release_by_tag(
    root: &Path,
    runner: &dyn CommandRunner,
    sleeper: &dyn Sleeper,
    repository: &str,
    tag: &TagName,
) -> Result<GitHubReleaseLookup, GraphError> {
    let endpoint = github_release_endpoint(repository, tag);
    retry_observation(sleeper, || github_api_get_once(root, runner, &endpoint))
}

/// Finds the release for `tag` whether it is published or still a draft.
///
/// `GET /releases/tags/{tag}` omits drafts entirely, so a 404 there is not
/// absence: the bounded list scan below is the only way to observe a draft.
pub(crate) fn github_release_for_tag(
    root: &Path,
    runner: &dyn CommandRunner,
    sleeper: &dyn Sleeper,
    repository: &str,
    tag: &TagName,
) -> Result<GitHubReleaseLookup, GraphError> {
    match github_release_by_tag(root, runner, sleeper, repository, tag)? {
        GitHubReleaseLookup::Absent => {}
        found_or_unknown => return Ok(found_or_unknown),
    }
    for page in 1..=RELEASE_LIST_PAGE_LIMIT {
        let endpoint = github_release_list_endpoint(repository, page);
        let listed = match retry_observation(sleeper, || github_api_get_once(root, runner, &endpoint))? {
            GitHubReleaseLookup::Found(listed) => listed,
            absent_or_unknown => return Ok(absent_or_unknown),
        };
        let releases = listed
            .as_array()
            .ok_or_else(|| malformed_github_response(&endpoint, "GitHub release list response is not an array"))?;
        if releases.is_empty() {
            break;
        }
        if let Some(release) = releases
            .iter()
            .find(|release| release.get("tag_name").and_then(serde_json::Value::as_str) == Some(tag.as_str()))
        {
            return Ok(GitHubReleaseLookup::Found(release.clone()));
        }
    }
    Ok(GitHubReleaseLookup::Absent)
}

pub(crate) fn malformed_github_response(endpoint: &str, detail: &str) -> GraphError {
    GraphError::ReleaseCommand {
        program: programs::GH.to_owned(),
        args: github_release_api_args(endpoint)
            .iter()
            .map(ToString::to_string)
            .collect(),
        failure: CommandFailure::MalformedOutput {
            detail: detail.to_owned(),
        },
    }
}

fn github_api_get_once(
    root: &Path,
    runner: &dyn CommandRunner,
    endpoint: &str,
) -> Result<Attempt<GitHubReleaseLookup>, GraphError> {
    let api_args = github_release_api_args(endpoint);
    let observed = match run_observation(runner, programs::GH, &api_args, root, timeouts::FORGE_API, false)? {
        Ok(output) => output,
        Err(_unavailable) => {
            return Ok(Attempt::Transient {
                value: GitHubReleaseLookup::CommandFailed,
                retry_after: None,
            })
        }
    };
    let response = github_api_response(programs::GH, &api_args, &observed)?;
    if let Some(lookup) = github_release_response_status(response.status) {
        return Ok(if response.is_transient() {
            Attempt::Transient {
                value: lookup,
                retry_after: response.retry_after(),
            }
        } else {
            Attempt::Settled(lookup)
        });
    }
    let value: serde_json::Value =
        serde_json::from_str(&response.body).map_err(|error| GraphError::ReleaseCommand {
            program: programs::GH.to_string(),
            args: api_args.iter().map(ToString::to_string).collect(),
            failure: CommandFailure::MalformedOutput {
                detail: error.to_string(),
            },
        })?;
    Ok(Attempt::Settled(GitHubReleaseLookup::Found(value)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: `gh api` timing out once must retry, not hand
    /// `CommandError::TimedOut` straight to `?` and abort the observation.
    #[test]
    fn a_timed_out_gh_api_call_retries_and_settles() {
        struct FlakyGh(
            std::sync::Mutex<std::collections::VecDeque<Result<CommandOutput, callisto_model::CommandError>>>,
        );
        impl CommandRunner for FlakyGh {
            fn run(
                &self,
                program: &str,
                _args: &[&str],
                _cwd: &Path,
            ) -> Result<CommandOutput, callisto_model::CommandError> {
                assert_eq!(program, "gh");
                self.0.lock().unwrap().pop_front().expect("script exhausted")
            }
        }
        let runner = FlakyGh(std::sync::Mutex::new(
            vec![
                Err(callisto_model::CommandError::TimedOut {
                    program: "gh".to_owned(),
                    seconds: 60,
                }),
                Ok(CommandOutput {
                    exit_code: Some(1),
                    stdout: "HTTP/2 404\r\ncontent-type: application/json\r\n\r\n{\"message\":\"Not Found\"}"
                        .to_owned(),
                    stderr: "gh: Not Found (HTTP 404)".to_owned(),
                }),
            ]
            .into(),
        ));
        let sleeper = super::super::provider::policy::tests::RecordingSleeper::new();
        let root = std::env::temp_dir();
        let result = retry_observation(&sleeper, || {
            github_api_get_once(&root, &runner, "repos/o/r/releases/tags/v1")
        });
        assert!(matches!(result, Ok(GitHubReleaseLookup::Absent)));
        assert_eq!(sleeper.waits(), vec![std::time::Duration::from_secs(2)]);
    }

    #[test]
    fn github_api_observation_parser_requires_an_explicit_http_status() {
        let response = parse_github_api_response(
            "gh",
            &["api"],
            "HTTP/2 404\r\ncontent-type: application/json\r\n\r\n{\"message\":\"Not Found\"}",
        )
        .unwrap();
        assert_eq!(response.status, 404);
        assert_eq!(response.body, "{\"message\":\"Not Found\"}");
        assert!(parse_github_api_response("gh", &["api"], "not a response").is_err());
    }

    #[test]
    fn github_api_response_preserves_a_parseable_not_found_on_nonzero_exit() {
        let output = CommandOutput {
            exit_code: Some(1),
            stdout: "HTTP/2 404\r\ncontent-type: application/json\r\n\r\n{\"message\":\"Not Found\"}".to_owned(),
            stderr: "gh: Not Found (HTTP 404)".to_owned(),
        };
        assert_eq!(github_api_response("gh", &["api"], &output).unwrap().status, 404);
    }

    #[test]
    fn github_release_status_only_proves_absence_for_not_found() {
        assert!(github_release_response_status(200).is_none());
        assert!(matches!(
            github_release_response_status(404),
            Some(GitHubReleaseLookup::Absent)
        ));
        for status in [401, 403, 429, 500, 503] {
            assert!(
                matches!(
                    github_release_response_status(status),
                    Some(GitHubReleaseLookup::Indeterminate { status: found }) if found == status
                ),
                "HTTP {status} must not be reported as a conflicting remote release"
            );
        }
    }

    #[test]
    fn the_captured_gh_output_parses_with_its_status_headers_and_body() {
        use super::super::provider::loopback::fixtures;
        let output = CommandOutput {
            exit_code: Some(0),
            stdout: fixtures::GITHUB_RELEASE_PUBLISHED.to_owned(),
            stderr: String::new(),
        };
        let response = github_api_response("gh", &["api"], &output).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.header("content-type"), Some("application/json; charset=utf-8"));
        let release: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(release["tag_name"], "v2.101.0");
        assert_eq!(release["draft"], false);
        assert_eq!(release["immutable"], true);
        assert_eq!(release["assets"].as_array().unwrap().len(), 2);
        assert!(release["assets"][0]["digest"].as_str().unwrap().starts_with("sha256:"));

        let missing = CommandOutput {
            exit_code: Some(1),
            stdout: fixtures::GITHUB_RELEASE_404.to_owned(),
            stderr: "gh: Not Found (HTTP 404)".to_owned(),
        };
        assert_eq!(github_api_response("gh", &["api"], &missing).unwrap().status, 404);
    }

    #[test]
    fn a_truncated_captured_response_is_malformed_not_a_release() {
        use super::super::provider::loopback::fixtures;
        let truncated = &fixtures::GITHUB_RELEASE_PUBLISHED[..200];
        let output = CommandOutput {
            exit_code: Some(0),
            stdout: truncated.to_owned(),
            stderr: String::new(),
        };
        assert!(matches!(
            github_api_response("gh", &["api"], &output),
            Err(GraphError::ReleaseCommand {
                failure: CommandFailure::MalformedOutput { .. },
                ..
            })
        ));
    }
}
