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

/// Names and commits of every tag come from one process, not a `tag --list` and a `for-each-ref`.
#[test]
fn status_and_version_enumerate_tags_in_one_git_process() {
    for args in [&["status"][..], &["--dry-run", "version"][..]] {
        let starts = git_starts(PackageKind::Cargo, 6, args);
        let listings = starts
            .iter()
            .filter(|line| line.contains("\"--list\"") || line.contains("refname:strip=2"))
            .count();
        assert_eq!(listings, 1, "`callisto {args:?}`: {starts:#?}");
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

/// A Cargo workspace of `crates` auto-triggered crates whose release tags point at another annotated tag, which the
/// bulk tag listing cannot peel to a commit.
fn auto_crates_with_tags_of_tags(crates: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let git = |args: &[&str]| {
        let status = Command::new("git").args(args).current_dir(root).status().unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.test"]);
    git(&["config", "user.name", "t"]);
    git(&["config", "commit.gpgsign", "false"]);
    git(&["config", "tag.gpgsign", "false"]);
    let members: Vec<String> = (0..crates).map(|i| format!("\"c{i}\"")).collect();
    std::fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nmembers = [{}]\nresolver = \"2\"\n", members.join(", ")),
    )
    .unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"cargo/*\"\nrelease-trigger = \"auto\"\n",
    )
    .unwrap();
    for i in 0..crates {
        let crate_dir = root.join(format!("c{i}"));
        std::fs::create_dir_all(crate_dir.join("src")).unwrap();
        std::fs::write(
            crate_dir.join("Cargo.toml"),
            format!("[package]\nname = \"c{i}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        std::fs::write(crate_dir.join("src/lib.rs"), "").unwrap();
    }
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "release"]);
    for i in 0..crates {
        git(&["tag", "-a", &format!("inner-c{i}"), "-m", "inner"]);
        git(&[
            "tag",
            "-a",
            &format!("c{i}@1.0.0"),
            "-m",
            "outer",
            &format!("inner-c{i}"),
        ]);
    }
    dir
}

fn release_dry_run_git_processes(repository: &std::path::Path) -> usize {
    git_processes_in(repository, &["--dry-run", "release"])
}

fn git_processes_in(repository: &std::path::Path, args: &[&str]) -> usize {
    let trace_dir = tempfile::tempdir().unwrap();
    let trace = trace_dir.path().join("trace.json");
    let output = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .arg("--cwd")
        .arg(repository)
        .args(args)
        .env("GIT_TRACE2_EVENT", &trace)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    std::fs::read_to_string(&trace)
        .unwrap()
        .lines()
        .filter(|line| line.contains("\"event\":\"start\""))
        .count()
}

/// The changesets-only pending hint works out no inference window, so tags of tags are not peeled package by package.
#[test]
fn release_dry_run_peels_no_tag_per_auto_package_for_the_pending_hint() {
    let few = release_dry_run_git_processes(auto_crates_with_tags_of_tags(2).path());
    let many = release_dry_run_git_processes(auto_crates_with_tags_of_tags(6).path());
    assert_eq!(
        few, many,
        "{few} git processes for 2 auto crates, {many} for 6: a git process per package"
    );
}

/// A Cargo workspace of `crates` crates in a Changesets v2 pre cycle whose committed `pre.json` keys every baseline by
/// the bare crate name, with a changeset for the first crate.
fn crates_in_v2_pre_mode(crates: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let git = |args: &[&str]| {
        let status = Command::new("git").args(args).current_dir(root).status().unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.test"]);
    git(&["config", "user.name", "t"]);
    git(&["config", "commit.gpgsign", "false"]);
    let members: Vec<String> = (0..crates).map(|i| format!("\"c{i}\"")).collect();
    std::fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nmembers = [{}]\nresolver = \"2\"\n", members.join(", ")),
    )
    .unwrap();
    std::fs::write(root.join("callisto.toml"), "# pre\n").unwrap();
    for i in 0..crates {
        let crate_dir = root.join(format!("c{i}"));
        std::fs::create_dir_all(crate_dir.join("src")).unwrap();
        std::fs::write(
            crate_dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"c{i}\"\nversion = \"1.0.0\"\nedition = \"2021\"\ndescription = \"c\"\nlicense = \"MIT\"\n"
            ),
        )
        .unwrap();
        std::fs::write(crate_dir.join("src/lib.rs"), "").unwrap();
    }
    let initial: Vec<String> = (0..crates).map(|i| format!("\"c{i}\":\"1.0.0\"")).collect();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(
        root.join(".changeset/pre.json"),
        format!(
            "{{\"mode\":\"pre\",\"tag\":\"alpha\",\"initialVersions\":{{{}}},\"changesets\":[]}}\n",
            initial.join(",")
        ),
    )
    .unwrap();
    std::fs::write(
        root.join(".changeset/fix-c0.md"),
        "---\n\"c0\": patch\n---\n\nFix c0.\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "enter pre"]);
    dir
}

/// Bare `pre.json` keys are attributed at the commit that added it from one batched read of every package's manifest.
#[test]
fn bare_pre_json_keys_start_no_git_process_per_package() {
    for args in [&["--dry-run", "release"][..], &["--dry-run", "version"][..]] {
        let few = git_processes_in(crates_in_v2_pre_mode(2).path(), args);
        let many = git_processes_in(crates_in_v2_pre_mode(6).path(), args);
        assert_eq!(
            few, many,
            "`callisto {args:?}`: {few} git processes for 2 crates, {many} for 6: a git process per package"
        );
    }
}
