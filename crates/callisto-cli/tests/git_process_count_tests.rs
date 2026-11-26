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

/// A Cargo workspace of `crates` crates at 1.0.0 released by `trigger`, each tagged at `tag_version` by an annotated
/// tag that points at another annotated tag.
fn crates_with_tags_of_tags(crates: usize, trigger: &str, tag_version: &str) -> tempfile::TempDir {
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
        format!("[[package]]\nmatch = \"cargo/*\"\nrelease-trigger = \"{trigger}\"\n"),
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
            &format!("c{i}@{tag_version}"),
            "-m",
            "outer",
            &format!("inner-c{i}"),
        ]);
    }
    dir
}

/// A directory holding a `git` that answers the bulk tag listing as Git before 2.44 did, peeling a tag of a tag only to
/// the inner tag, so the listing names no commit for the fixture's release tags. Every other call is the real Git.
fn legacy_tag_listing_git() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt as _;

    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|directory| directory.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git on PATH");
    let dir = tempfile::tempdir().unwrap();
    // NUL-separated fields become tabs (no ref name holds one) so `sed` can mark a release tag's peel as a tag.
    let script = format!(
        "#!/bin/sh\ncase \"$*\" in\nfor-each-ref*'%(*objectname)'*)\n  '{real}' \"$@\" | tr '\\000' '\\t' \
         | sed 's/^\\(c[0-9]*@[^\t]*\ttag\t[0-9a-f]*\t\\)commit\t/\\1tag\t/' | tr '\\t' '\\000'\n  ;;\n\
         *) exec '{real}' \"$@\" ;;\nesac\n",
        real = real.display()
    );
    let path = dir.path().join("git");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

/// Git processes `release --dry-run` starts over [`crates_with_tags_of_tags`], under the pre-2.44 tag listing when
/// `legacy`.
fn release_dry_run_starts(crates: usize, trigger: &str, tag_version: &str, legacy: bool) -> usize {
    let repository = crates_with_tags_of_tags(crates, trigger, tag_version);
    let legacy_git = legacy_tag_listing_git();
    let mut path = std::env::var_os("PATH").unwrap();
    if legacy {
        let directories = std::iter::once(legacy_git.path().to_path_buf()).chain(std::env::split_paths(&path));
        path = std::env::join_paths(directories.collect::<Vec<_>>()).unwrap();
    }
    git_processes_with(repository.path(), &["--dry-run", "release"], &path)
}

fn git_processes_in(repository: &std::path::Path, args: &[&str]) -> usize {
    git_processes_with(repository, args, &std::env::var_os("PATH").unwrap())
}

fn git_processes_with(repository: &std::path::Path, args: &[&str], path: &std::ffi::OsStr) -> usize {
    let trace_dir = tempfile::tempdir().unwrap();
    let trace = trace_dir.path().join("trace.json");
    let output = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .arg("--cwd")
        .arg(repository)
        .args(args)
        .env("PATH", path)
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

/// The disabled pending hint starts no git process of its own: an auto workspace starts exactly what the same workspace
/// released by changesets starts, under the modern and the pre-2.44 tag listing, for 2 crates and for 6. The tags are
/// at an older version than the manifests, so resuming the release looks no tag up.
#[test]
fn the_pending_hint_starts_no_git_process_of_its_own() {
    for legacy in [false, true] {
        let starts = [2, 6].map(|crates| {
            let auto = release_dry_run_starts(crates, "auto", "0.9.0", legacy);
            let changeset = release_dry_run_starts(crates, "changeset", "0.9.0", legacy);
            assert_eq!(
                auto, changeset,
                "legacy listing {legacy}, {crates} crates: auto against changeset"
            );
            auto
        });
        assert_eq!(starts[0], starts[1], "legacy listing {legacy}: 2 crates against 6");
    }
}

/// The pre-2.44 listing really leaves tags of tags to a lookup each: with every crate's current version tagged, resuming
/// the release starts one more git process per crate there, and none more under the modern listing.
#[test]
fn the_legacy_tag_listing_leaves_each_tag_of_a_tag_to_a_lookup() {
    let legacy = [2, 6].map(|crates| release_dry_run_starts(crates, "changeset", "1.0.0", true));
    assert_eq!(legacy[1] - legacy[0], 4, "{legacy:?}");
    let modern = [2, 6].map(|crates| release_dry_run_starts(crates, "changeset", "1.0.0", false));
    assert_eq!(modern[0], modern[1], "{modern:?}");
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
