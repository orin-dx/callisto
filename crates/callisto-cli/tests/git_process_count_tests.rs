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

/// How the bulk tag listing peels a tag of a tag.
#[derive(Clone, Copy, Debug)]
enum Peel {
    /// Through to the commit, as Git 2.44 and later list it.
    Recursive,
    /// One level, to the inner tag, as Git before 2.44 listed it: the listing names no commit for the tag.
    OneLevel,
}

/// A directory holding a `git` whose bulk tag listing peels the fixture's release tags as `peel` says, whichever Git is
/// on PATH, `commit` being the commit they point at. Every other call is the real Git.
fn tag_listing_git(peel: Peel, commit: &str) -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt as _;

    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|directory| directory.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git on PATH");
    let peeled = match peel {
        Peel::Recursive => format!("commit\t{commit}"),
        Peel::OneLevel => "tag\t\\2".to_owned(),
    };
    let dir = tempfile::tempdir().unwrap();
    // NUL-separated fields become tabs (no ref name holds one) so `sed` can rewrite a release tag's peel.
    let script = format!(
        "#!/bin/sh\ncase \"$*\" in\nfor-each-ref*'%(*objectname)'*)\n  '{real}' \"$@\" | tr '\\000' '\\t' \
         | sed 's/^\\(c[0-9]*@[^\t]*\ttag\t[0-9a-f]*\t\\)[a-z]*\t\\([0-9a-f]*\\)$/\\1{peeled}/' | tr '\\t' '\\000'\n  ;;\n\
         *) exec '{real}' \"$@\" ;;\nesac\n",
        real = real.display()
    );
    let path = dir.path().join("git");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

/// Git processes `release --dry-run` starts over [`crates_with_tags_of_tags`] under a tag listing that peels as `peel`
/// says.
fn release_dry_run_starts(crates: usize, trigger: &str, tag_version: &str, peel: Peel) -> usize {
    let repository = crates_with_tags_of_tags(crates, trigger, tag_version);
    // Read before the counted run, so the listing's oracle starts no process of its own there.
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repository.path())
        .output()
        .unwrap();
    assert!(head.status.success());
    let listing = tag_listing_git(peel, String::from_utf8(head.stdout).unwrap().trim());
    let path = std::env::var_os("PATH").unwrap();
    let directories = std::iter::once(listing.path().to_path_buf()).chain(std::env::split_paths(&path));
    let path = std::env::join_paths(directories.collect::<Vec<_>>()).unwrap();
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
/// released by changesets starts, under the recursive and the one-level tag listing, for 2 crates and for 6. The tags
/// are at an older version than the manifests, so resuming the release looks no tag up.
#[test]
fn the_pending_hint_starts_no_git_process_of_its_own() {
    for peel in [Peel::Recursive, Peel::OneLevel] {
        let starts = [2, 6].map(|crates| {
            let auto = release_dry_run_starts(crates, "auto", "0.9.0", peel);
            let changeset = release_dry_run_starts(crates, "changeset", "0.9.0", peel);
            assert_eq!(
                auto, changeset,
                "{peel:?} listing, {crates} crates: auto against changeset"
            );
            auto
        });
        assert_eq!(starts[0], starts[1], "{peel:?} listing: 2 crates against 6");
    }
}

/// The one-level listing really leaves tags of tags to a lookup each: with every crate's current version tagged,
/// resuming the release starts one more git process per crate there, and none more under the recursive listing.
#[test]
fn the_legacy_tag_listing_leaves_each_tag_of_a_tag_to_a_lookup() {
    let one_level = [2, 6].map(|crates| release_dry_run_starts(crates, "changeset", "1.0.0", Peel::OneLevel));
    assert_eq!(one_level[1] - one_level[0], 4, "{one_level:?}");
    let recursive = [2, 6].map(|crates| release_dry_run_starts(crates, "changeset", "1.0.0", Peel::Recursive));
    assert_eq!(recursive[0], recursive[1], "{recursive:?}");
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

/// [`crates_in_v2_pre_mode`] after every crate moved to `crates/`, so the commit that added `pre.json` holds no
/// manifest at any crate's current path.
fn moved_crates_in_v2_pre_mode(crates: usize) -> tempfile::TempDir {
    let dir = crates_in_v2_pre_mode(crates);
    let root = dir.path();
    let git = |args: &[&str]| {
        let status = Command::new("git").args(args).current_dir(root).status().unwrap();
        assert!(status.success(), "git {args:?}");
    };
    std::fs::create_dir_all(root.join("crates")).unwrap();
    for i in 0..crates {
        git(&["mv", &format!("c{i}"), &format!("crates/c{i}")]);
    }
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "move crates"]);
    dir
}

/// `packages` dual Cargo and npm packages whose names differ, in a Changesets v2 pre cycle keyed by the npm names, so
/// no key names a package's first manifest.
fn dual_packages_in_v2_pre_mode_keyed_by_npm(packages: usize) -> tempfile::TempDir {
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
    let members: Vec<String> = (0..packages).map(|i| format!("\"d{i}\"")).collect();
    std::fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nmembers = [{}]\nresolver = \"2\"\n", members.join(", ")),
    )
    .unwrap();
    std::fs::write(
        root.join("package.json"),
        format!(
            "{{\"name\":\"root\",\"private\":true,\"workspaces\":[{}]}}\n",
            members.join(",")
        ),
    )
    .unwrap();
    std::fs::write(root.join("callisto.toml"), "# pre\n").unwrap();
    for i in 0..packages {
        let package = root.join(format!("d{i}"));
        std::fs::create_dir_all(package.join("src")).unwrap();
        std::fs::write(
            package.join("Cargo.toml"),
            format!(
                "[package]\nname = \"d{i}-rs\"\nversion = \"1.0.0\"\nedition = \"2021\"\ndescription = \"d\"\nlicense = \"MIT\"\n"
            ),
        )
        .unwrap();
        std::fs::write(package.join("src/lib.rs"), "").unwrap();
        std::fs::write(
            package.join("package.json"),
            format!("{{\"name\":\"d{i}-js\",\"version\":\"1.0.0\"}}\n"),
        )
        .unwrap();
    }
    let initial: Vec<String> = (0..packages).map(|i| format!("\"d{i}-js\":\"1.0.0\"")).collect();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(
        root.join(".changeset/pre.json"),
        format!(
            "{{\"mode\":\"pre\",\"tag\":\"alpha\",\"initialVersions\":{{{}}},\"changesets\":[]}}\n",
            initial.join(",")
        ),
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "enter pre"]);
    dir
}

/// Keys whose package's first manifest did not name them where `pre.json` was added (it moved, or the key is another
/// ecosystem's name) are read from that commit's manifests together, not a search per key.
#[test]
fn pre_json_keys_read_from_history_start_no_git_process_per_package() {
    type Fixture = fn(usize) -> tempfile::TempDir;
    let fixtures: [(&str, Fixture); 2] = [
        ("moved manifests", moved_crates_in_v2_pre_mode),
        ("npm keys of dual packages", dual_packages_in_v2_pre_mode_keyed_by_npm),
    ];
    for (shape, fixture) in fixtures {
        for args in [&["--dry-run", "release"][..], &["--dry-run", "version"][..]] {
            let few = git_processes_in(fixture(2).path(), args);
            let many = git_processes_in(fixture(6).path(), args);
            assert_eq!(
                few, many,
                "{shape}, `callisto {args:?}`: {few} git processes for 2 packages, {many} for 6"
            );
        }
    }
}
