use callisto_model::ApplyPermit;
mod fixtures;
use fixtures::GraphBuilder;
use std::cell::OnceCell;

/// Verifies that the custom branch name passed via `--branch` appears in the
/// rendered PR body, not the default branch inferred from config.
#[test]
fn test_compose_pr_body_custom_branch_flag() {
    use callisto_graph::commands::{compose_pr_body, PrBodyOptions};
    use callisto_graph::infer::NoInference;

    struct DummyRunner;
    impl callisto_model::CommandRunner for DummyRunner {
        fn run(
            &self,
            _program: &str,
            _args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            Ok(callisto_model::CommandOutput {
                exit_code: Some(0),
                stdout: "".to_string(),
                stderr: "".to_string(),
            })
        }
    }

    let runner = DummyRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let cfg = callisto_graph::config::load(&ws_dir.path().join("callisto.toml")).unwrap();
    let graph = GraphBuilder::new().build().unwrap();
    let tags = callisto_graph::tags::TagIndex::build(&runner, ws_dir.path(), &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: ws_dir.path().to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        runner: &runner,
        manifest_cache: Default::default(),
    };

    let inference = NoInference;
    let opts = PrBodyOptions {
        existing_body: None,
        labels: Vec::new(),
        branch: Some("release/v1.0".to_string()),
    };

    let report = compose_pr_body(&ws, &inference, &opts).unwrap();
    assert!(report.pr_body.contains("release/v1.0"));
}

/// Verifies that `apply_version_plan` surfaces a `git add` failure instead
/// of silently reporting the path as staged. This covers the error path where
/// the manifest write succeeds but the subsequent `git add` exits non-zero.
#[test]
fn test_apply_version_plan_reports_git_add_failure() {
    use callisto_graph::apply::{apply_version_plan, ApplyOptions};
    use callisto_graph::plan::VersionPlan;
    use callisto_model::{CommandOutput, CommandRunner};

    struct FailingGitAddRunner;
    impl CommandRunner for FailingGitAddRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<CommandOutput, callisto_model::CommandError> {
            if program == "git" && args.first() == Some(&"add") {
                return Ok(CommandOutput {
                    exit_code: Some(1),
                    stdout: String::new(),
                    stderr: "fatal: pathspec rejected by hook".to_string(),
                });
            }
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    let runner = FailingGitAddRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .arg("init")
        .current_dir(ws_dir.path())
        .output()
        .unwrap();

    let pre_state = callisto_format::PreState::entering("canary", Vec::new());
    let plan = VersionPlan {
        pre_state_update: Some(pre_state),
        ..Default::default()
    };

    let opts = ApplyOptions {
        refresh_lockfiles: false,
    };

    let result = apply_version_plan(
        ws_dir.path(),
        &plan,
        &runner,
        &opts,
        &ApplyPermit::force_for_tests(),
    );

    assert!(
        result.is_err(),
        "expected apply_version_plan to return an error when `git add` fails, got: {result:?}"
    );
}
