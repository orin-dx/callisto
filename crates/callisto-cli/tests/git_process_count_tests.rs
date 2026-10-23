//! The commands that read a whole workspace start the same git processes however many packages it has.

use std::process::Command;

use callisto_fixtures::perf::ScaledWorkspace;

/// Runs `callisto` over a workspace of `packages` and returns how many git processes it started.
fn git_processes(packages: usize, args: &[&str]) -> usize {
    let repository = tempfile::tempdir().unwrap();
    ScaledWorkspace {
        packages,
        extra_commits: packages,
    }
    .build(repository.path());
    let trace_dir = tempfile::tempdir().unwrap();
    let trace = trace_dir.path().join("trace.json");
    let output = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .arg("--cwd")
        .arg(repository.path())
        .args(args)
        .env("GIT_TRACE2_EVENT", &trace)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "callisto {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read_to_string(&trace)
        .unwrap()
        .lines()
        .filter(|line| line.contains("\"event\":\"start\""))
        .count()
}

fn assert_independent_of_package_count(args: &[&str]) {
    let few = git_processes(6, args);
    let many = git_processes(30, args);
    assert_eq!(
        few, many,
        "`callisto {args:?}` started {few} git processes for 6 packages and {many} for 30: a git process per package"
    );
}

#[test]
fn status_starts_no_git_process_per_package() {
    assert_independent_of_package_count(&["status"]);
}

#[test]
fn version_dry_run_starts_no_git_process_per_package() {
    assert_independent_of_package_count(&["--dry-run", "version"]);
}

#[test]
fn release_dry_run_starts_no_git_process_per_package() {
    assert_independent_of_package_count(&["--dry-run", "release"]);
}
