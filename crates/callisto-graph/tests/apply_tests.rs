use std::path::{Path, PathBuf};

use callisto_graph::apply::{apply_version_plan, ApplyOptions};
use callisto_graph::plan::VersionPlan;
use callisto_model::{ApplyPermit, CommandError, CommandOutput, CommandRunner};

struct NoopRunner;

impl CommandRunner for NoopRunner {
    fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

/// When a workspace uses a custom changeset dir (e.g., `releases/`), exiting
/// pre-release mode must delete `releases/pre.json`, NOT `.changeset/pre.json`.
///
/// Before the fix: `apply_version_plan` hardcodes `.changeset/pre.json`,
/// so `releases/pre.json` is left on disk and the test fails.
#[test]
fn pre_exit_deletes_pre_json_in_custom_changeset_dir() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let root = dir.path();

    // Custom changeset dir
    let custom_dir = root.join("releases");
    std::fs::create_dir_all(&custom_dir).unwrap();
    let pre_json_path = custom_dir.join("pre.json");
    std::fs::write(&pre_json_path, r#"{"mode":"exit","tag":"beta","changesets":[]}"#).unwrap();

    // Ensure the default dir does NOT have a pre.json so we can confirm it
    // isn't the one being deleted.
    let default_pre = root.join(".changeset").join("pre.json");
    assert!(!default_pre.exists(), ".changeset/pre.json must not exist");

    let plan = VersionPlan {
        delete_pre_json: Some(PathBuf::from("releases/pre.json")),
        ..Default::default()
    };

    let permit = ApplyPermit::force_for_tests();
    let opts = ApplyOptions::default();
    apply_version_plan(root, &plan, &NoopRunner, &opts, &permit).expect("apply_version_plan should succeed");

    assert!(
        !pre_json_path.exists(),
        "releases/pre.json should have been deleted but still exists"
    );
}
