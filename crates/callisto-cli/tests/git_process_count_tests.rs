//! The commands that read a whole workspace start the same git processes however many packages it has.

use std::process::Command;

use callisto_fixtures::perf::{PackageKind, ScaledWorkspace};

/// Runs `callisto` over a workspace of `packages` and returns how many git processes it started.
fn git_starts(kind: PackageKind, packages: usize, args: &[&str]) -> Vec<String> {
    let repository = tempfile::tempdir().unwrap();
    ScaledWorkspace {
        kind,
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
        .env("GIT_TERMINAL_PROMPT", "0")
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
        .map(str::to_owned)
        .collect()
}

fn git_processes(kind: PackageKind, packages: usize, args: &[&str]) -> usize {
    git_starts(kind, packages, args).len()
}

fn assert_independent_of_package_count(args: &[&str]) {
    let few = git_processes(PackageKind::Cargo, 6, args);
    let many = git_processes(PackageKind::Cargo, 30, args);
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

/// Tags made on one commit by another package that held the package's name share that commit's history probes.
#[test]
fn foreign_tags_of_one_commit_start_no_git_process_per_tag() {
    for args in [&["status"][..], &["--dry-run", "version"][..]] {
        let few = git_processes(PackageKind::ForeignTags, 6, args);
        let many = git_processes(PackageKind::ForeignTags, 30, args);
        assert_eq!(
            few, many,
            "`callisto {args:?}` started {few} git processes for 6 foreign tags and {many} for 30: a git process per tag"
        );
    }
}

/// The tag probe runs `ls-remote` against the HTTPS origin, and no credential helper adds a process when it is refused.
#[test]
fn release_dry_run_probes_origin_once_and_starts_no_credential_helper() {
    let starts = git_starts(PackageKind::Cargo, 6, &["--dry-run", "release"]);
    let count = |needle: &str| starts.iter().filter(|line| line.contains(needle)).count();
    assert_eq!(count("\"ls-remote\""), 1, "{starts:#?}");
    assert_eq!(count("credential"), 0, "{starts:#?}");
}
