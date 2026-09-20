//! The GitHub Releases read path: argv, HTTP framing, and response parsing.
//!
//! Kept free of any provider so the forge-release and artifact-upload roles
//! share one lookup and cannot drift in how they read an absence.

use std::path::Path;

use callisto_model::{CommandOutput, CommandRunner, TagName};

use crate::error::CommandFailure;
use crate::GraphError;

use super::provider::policy::timeouts;

/// One GitHub release lookup, shared by the forge-release and asset adapters.
#[derive(Debug)]
pub(crate) enum GitHubReleaseLookup {
    Absent,
    Indeterminate { status: u16 },
    Found(serde_json::Value),
}

pub(crate) fn github_release_endpoint(repository: &str, tag: &TagName) -> String {
    format!("repos/{repository}/releases/tags/{tag}")
}

/// `gh api` defines no `--repo`; the endpoint already carries owner and repository.
pub(crate) fn github_release_api_args(endpoint: &str) -> [&str; 5] {
    ["api", "--include", "--method", "GET", endpoint]
}

fn parse_github_api_response<'a>(program: &str, args: &[&str], stdout: &'a str) -> Result<(u16, &'a str), GraphError> {
    let malformed = |detail: String| GraphError::ReleaseCommand {
        program: program.to_string(),
        args: args.iter().map(ToString::to_string).collect(),
        failure: CommandFailure::MalformedOutput { detail },
    };
    let (headers, body) = stdout
        .rsplit_once("\r\n\r\n")
        .or_else(|| stdout.rsplit_once("\n\n"))
        .ok_or_else(|| malformed("response has no blank line separating headers from body".to_string()))?;
    let status_line = headers
        .lines()
        .rev()
        .find(|line| line.trim_end_matches('\r').starts_with("HTTP/"))
        .ok_or_else(|| malformed("response headers contain no HTTP status line".to_string()))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| malformed(format!("HTTP status line has no status code: {status_line:?}")))?
        .parse::<u16>()
        .map_err(|error| malformed(format!("HTTP status code is not a valid number: {error}")))?;
    Ok((status, body))
}

/// `gh api --include` preserves the HTTP response even for a 404 and exits
/// non-zero. Parse that authoritative response first so absence is not
/// mistaken for a transport failure; retain the command failure when no
/// parseable HTTP response was produced.
fn github_api_response<'a>(
    program: &str,
    args: &[&str],
    output: &'a CommandOutput,
) -> Result<(u16, &'a str), GraphError> {
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

/// The one GET both forge observations share.
pub(crate) fn github_release_by_tag(
    root: &Path,
    runner: &dyn CommandRunner,
    repository: &str,
    tag: &TagName,
) -> Result<GitHubReleaseLookup, GraphError> {
    let endpoint = github_release_endpoint(repository, tag);
    let api_args = github_release_api_args(&endpoint);
    let observed = runner.run_with_timeout("gh", &api_args, root, timeouts::FORGE_API)?;
    let (status, body) = github_api_response("gh", &api_args, &observed)?;
    if let Some(lookup) = github_release_response_status(status) {
        return Ok(lookup);
    }
    let value: serde_json::Value = serde_json::from_str(body).map_err(|error| GraphError::ReleaseCommand {
        program: "gh".to_string(),
        args: api_args.iter().map(ToString::to_string).collect(),
        failure: CommandFailure::MalformedOutput {
            detail: error.to_string(),
        },
    })?;
    Ok(GitHubReleaseLookup::Found(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_api_observation_parser_requires_an_explicit_http_status() {
        let (status, body) = parse_github_api_response(
            "gh",
            &["api"],
            "HTTP/2 404\r\ncontent-type: application/json\r\n\r\n{\"message\":\"Not Found\"}",
        )
        .unwrap();
        assert_eq!(status, 404);
        assert_eq!(body, "{\"message\":\"Not Found\"}");
        assert!(parse_github_api_response("gh", &["api"], "not a response").is_err());
    }

    #[test]
    fn github_api_response_preserves_a_parseable_not_found_on_nonzero_exit() {
        let output = CommandOutput {
            exit_code: Some(1),
            stdout: "HTTP/2 404\r\ncontent-type: application/json\r\n\r\n{\"message\":\"Not Found\"}".to_owned(),
            stderr: "gh: Not Found (HTTP 404)".to_owned(),
        };
        let (status, _) = github_api_response("gh", &["api"], &output).unwrap();
        assert_eq!(status, 404);
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
}
