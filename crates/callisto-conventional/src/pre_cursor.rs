use std::path::Path;

use callisto_model::{CommandRunner, CommitSha, PackageId};

use crate::ConventionalError;

pub fn pre_cursor_ref_name(package: &PackageId) -> String {
    format!("refs/callisto/pre-cursor/{}", package.display_name())
}

pub fn resolve_pre_cursor(
    runner: &dyn CommandRunner,
    cwd: &Path,
    package: &PackageId,
) -> Result<Option<CommitSha>, ConventionalError> {
    let ref_name = pre_cursor_ref_name(package);
    let output = runner.run("git", &["rev-parse", "--verify", "--quiet", &ref_name], cwd)?;

    if !output.success() {
        return Ok(None);
    }

    let sha_str = output.stdout_trimmed();
    if sha_str.is_empty() {
        return Ok(None);
    }

    let sha =
        CommitSha::parse(sha_str).map_err(|_err| ConventionalError::MalformedPreCursorRef {
            cwd: cwd.to_path_buf(),
            ref_name,
            stderr: output.stderr,
        })?;

    Ok(Some(sha))
}

pub fn advance_pre_cursor(
    runner: &dyn CommandRunner,
    cwd: &Path,
    package: &PackageId,
    sha: &CommitSha,
) -> Result<(), ConventionalError> {
    let ref_name = pre_cursor_ref_name(package);
    let output = runner.run("git", &["update-ref", &ref_name, sha.as_str()], cwd)?;

    if !output.success() {
        return Err(ConventionalError::PreCursorAdvanceFailed {
            cwd: cwd.to_path_buf(),
            ref_name,
            sha: sha.as_str().to_string(),
            stderr: output.stderr,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use callisto_model::{CommandError, CommandOutput};

    fn sha40(fill: char) -> String {
        std::iter::repeat_n(fill, 40).collect()
    }

    // --- pre_cursor_ref_name ---------------------------------------------

    #[test]
    fn ref_name_for_bare_package_id() {
        let id = PackageId::parse("my-crate").unwrap();
        assert_eq!(
            pre_cursor_ref_name(&id),
            "refs/callisto/pre-cursor/my-crate"
        );
    }

    #[test]
    fn ref_name_for_ecosystem_prefixed_package_id() {
        let id = PackageId::parse("npm:my-pkg").unwrap();
        assert_eq!(
            pre_cursor_ref_name(&id),
            "refs/callisto/pre-cursor/npm/my-pkg"
        );
    }

    #[test]
    fn ref_name_for_scoped_npm_package_id() {
        let id = PackageId::parse("npm:@myorg/cli").unwrap();
        assert_eq!(
            pre_cursor_ref_name(&id),
            "refs/callisto/pre-cursor/npm/@myorg/cli"
        );
    }

    // --- resolve_pre_cursor / advance_pre_cursor --------------------------

    /// A `CommandRunner` double that replays one canned response for every
    /// invocation and asserts the program is always `"git"` -- these two
    /// functions never shell out to anything else.
    struct CannedRunner(Result<CommandOutput, CommandError>);

    impl CommandRunner for CannedRunner {
        fn run(
            &self,
            program: &str,
            _args: &[&str],
            _cwd: &Path,
        ) -> Result<CommandOutput, CommandError> {
            assert_eq!(program, "git");
            self.0.clone()
        }
    }

    fn ok(exit_code: i32, stdout: &str) -> CommandOutput {
        CommandOutput {
            exit_code: Some(exit_code),
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    #[test]
    fn resolve_pre_cursor_shells_expected_rev_parse_args() {
        struct AssertingRunner;
        impl CommandRunner for AssertingRunner {
            fn run(
                &self,
                program: &str,
                args: &[&str],
                _cwd: &Path,
            ) -> Result<CommandOutput, CommandError> {
                assert_eq!(program, "git");
                assert_eq!(
                    args,
                    [
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        "refs/callisto/pre-cursor/my-crate"
                    ]
                );
                Ok(ok(0, &sha40('a')))
            }
        }
        let id = PackageId::parse("my-crate").unwrap();
        let result = resolve_pre_cursor(&AssertingRunner, Path::new("."), &id);
        assert_eq!(
            result.unwrap(),
            Some(CommitSha::parse(&sha40('a')).unwrap())
        );
    }

    #[test]
    fn resolve_pre_cursor_returns_sha_when_ref_resolves() {
        let runner = CannedRunner(Ok(ok(0, &format!("{}\n", sha40('b')))));
        let id = PackageId::parse("my-crate").unwrap();
        let result = resolve_pre_cursor(&runner, Path::new("."), &id).unwrap();
        assert_eq!(result, Some(CommitSha::parse(&sha40('b')).unwrap()));
    }

    #[test]
    fn resolve_pre_cursor_returns_none_when_ref_does_not_exist() {
        // `git rev-parse --verify --quiet` exits non-zero, silently, when
        // the ref doesn't resolve -- this is the expected steady state
        // before a package has ever entered pre-release mode.
        let runner = CannedRunner(Ok(ok(1, "")));
        let id = PackageId::parse("my-crate").unwrap();
        let result = resolve_pre_cursor(&runner, Path::new("."), &id).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_pre_cursor_returns_none_when_output_empty_despite_success() {
        let runner = CannedRunner(Ok(ok(0, "")));
        let id = PackageId::parse("my-crate").unwrap();
        let result = resolve_pre_cursor(&runner, Path::new("."), &id).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_pre_cursor_returns_malformed_error_for_unparseable_output() {
        let runner = CannedRunner(Ok(ok(0, "not-a-sha")));
        let id = PackageId::parse("my-crate").unwrap();
        let result = resolve_pre_cursor(&runner, Path::new("/repo"), &id);
        match result {
            Err(ConventionalError::MalformedPreCursorRef { cwd, ref_name, .. }) => {
                assert_eq!(cwd, PathBuf::from("/repo"));
                assert_eq!(ref_name, "refs/callisto/pre-cursor/my-crate");
            }
            other => panic!("expected Err(MalformedPreCursorRef), got {other:?}"),
        }
    }

    #[test]
    fn resolve_pre_cursor_propagates_command_error() {
        let runner = CannedRunner(Err(CommandError::NotFound {
            program: "git".to_string(),
        }));
        let id = PackageId::parse("my-crate").unwrap();
        let result = resolve_pre_cursor(&runner, Path::new("."), &id);
        assert!(
            matches!(
                result,
                Err(ConventionalError::Command(CommandError::NotFound { .. }))
            ),
            "expected Err(ConventionalError::Command(NotFound)), got {result:?}"
        );
    }

    #[test]
    fn advance_pre_cursor_shells_expected_update_ref_args() {
        struct AssertingRunner;
        impl CommandRunner for AssertingRunner {
            fn run(
                &self,
                program: &str,
                args: &[&str],
                _cwd: &Path,
            ) -> Result<CommandOutput, CommandError> {
                assert_eq!(program, "git");
                assert_eq!(
                    args,
                    [
                        "update-ref",
                        "refs/callisto/pre-cursor/my-crate",
                        sha40('c').as_str()
                    ]
                );
                Ok(ok(0, ""))
            }
        }
        let id = PackageId::parse("my-crate").unwrap();
        let sha = CommitSha::parse(&sha40('c')).unwrap();
        advance_pre_cursor(&AssertingRunner, Path::new("."), &id, &sha).unwrap();
    }

    #[test]
    fn advance_pre_cursor_succeeds_when_update_ref_succeeds() {
        let runner = CannedRunner(Ok(ok(0, "")));
        let id = PackageId::parse("my-crate").unwrap();
        let sha = CommitSha::parse(&sha40('d')).unwrap();
        assert!(advance_pre_cursor(&runner, Path::new("."), &id, &sha).is_ok());
    }

    #[test]
    fn advance_pre_cursor_returns_error_when_update_ref_fails() {
        let runner = CannedRunner(Ok(CommandOutput {
            exit_code: Some(1),
            stdout: String::new(),
            stderr: "fatal: cannot lock ref".to_string(),
        }));
        let id = PackageId::parse("my-crate").unwrap();
        let sha = CommitSha::parse(&sha40('e')).unwrap();
        let result = advance_pre_cursor(&runner, Path::new("/repo"), &id, &sha);
        match result {
            Err(ConventionalError::PreCursorAdvanceFailed {
                cwd,
                ref_name,
                sha: reported_sha,
                stderr,
            }) => {
                assert_eq!(cwd, PathBuf::from("/repo"));
                assert_eq!(ref_name, "refs/callisto/pre-cursor/my-crate");
                assert_eq!(reported_sha, sha40('e'));
                assert_eq!(stderr, "fatal: cannot lock ref");
            }
            other => panic!("expected Err(PreCursorAdvanceFailed), got {other:?}"),
        }
    }
}
