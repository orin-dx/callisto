//! Regression test for eager `TagIndex` construction.
//!
//! `Workspace::load` used to build the full `TagIndex` unconditionally --
//! even for callers (e.g. `callisto add`'s non-interactive path, `callisto
//! init`, `callisto add`'s interactive package-selection step) that never
//! consult tags at all. `TagIndex::build` spawns `git tag --list`; that
//! should never happen just because a `Workspace` was loaded, only once
//! something actually calls `Workspace::tags()`, and at most once.
//!
//! This test counts `git tag --list` invocations to prove laziness.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::Workspace;
use callisto_model::{CommandError, CommandOutput, CommandRunner};

/// A `CommandRunner` double that counts every `git tag --list` invocation
/// it receives; answers with an empty tag list so `TagIndex::build` (if
/// invoked) succeeds trivially.
struct CountingTagListRunner {
    tag_list_calls: AtomicUsize,
}

impl CommandRunner for CountingTagListRunner {
    fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
        assert_eq!(program, "git", "only `git` should ever be shelled out to here");
        if args == ["tag", "--list"] {
            self.tag_list_calls.fetch_add(1, Ordering::SeqCst);
        }
        Ok(CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn write_minimal_workspace(root: &Path) {
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    let pkg_dir = root.join("crates/my-app");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    std::fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
}

#[test]
fn workspace_load_does_not_build_tag_index_until_tags_is_called() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_minimal_workspace(root);

    let locator = IgnoreWalkLocator::new(root);
    let runner = CountingTagListRunner {
        tag_list_calls: AtomicUsize::new(0),
    };

    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("Workspace::load should succeed");

    assert_eq!(
        runner.tag_list_calls.load(Ordering::SeqCst),
        0,
        "Workspace::load must not build the tag index eagerly -- it should be \
         deferred until something actually calls Workspace::tags()"
    );

    let _tags = ws.tags().expect("tags() should build the index lazily");
    assert_eq!(
        runner.tag_list_calls.load(Ordering::SeqCst),
        1,
        "the first Workspace::tags() call should build the tag index exactly once"
    );

    let _tags_again = ws.tags().expect("second call should reuse the cached index");
    assert_eq!(
        runner.tag_list_calls.load(Ordering::SeqCst),
        1,
        "a second Workspace::tags() call must not rebuild the tag index"
    );
}
