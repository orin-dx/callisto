//! The tag a release creates is `{name}@{version}`, qualified as `ecosystem/name@{version}` once another ecosystem has a
//! package of that name, and a `tag-template` overrides both.
#![cfg(unix)]

#[path = "common/release_harness.rs"]
mod release_harness;

use std::{fs, path::Path};

use release_harness::{callisto, execute, fake_publishers, git, FakePublishers, DECISION_PATH};

fn write(root: &Path, relative: &str, content: &str) {
    fs::create_dir_all(root.join(relative).parent().unwrap()).unwrap();
    fs::write(root.join(relative), content).unwrap();
}

fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", message]);
}

/// A workspace holding cargo `foo`, plus npm `foo` when `with_npm_foo`, with `extra_config` appended to `callisto.toml`,
/// and a pending minor changeset for cargo `foo`.
fn workspace(with_npm_foo: bool, extra_config: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    git(root, &["remote", "add", "origin", "https://github.com/example/foo.git"]);
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/foo\"]\nresolver = \"2\"\n",
    );
    write(
        root,
        "crates/foo/Cargo.toml",
        "[package]\nname = \"foo\"\nversion = \"1.0.0\"\nedition = \"2021\"\ndescription = \"t\"\nlicense = \"MIT\"\n",
    );
    write(root, "crates/foo/src/lib.rs", "\n");
    if with_npm_foo {
        write(
            root,
            "packages/foo/package.json",
            "{\n  \"name\": \"foo\",\n  \"version\": \"3.0.0\",\n  \"private\": true\n}\n",
        );
    }
    callisto_fixtures::scaffold_callisto(root);
    if !extra_config.is_empty() {
        let path = root.join("callisto.toml");
        let existing = fs::read_to_string(&path).unwrap_or_default();
        fs::write(&path, format!("{existing}\n{extra_config}")).unwrap();
    }
    commit(root, "initial");
    let selector = if with_npm_foo { "cargo/foo" } else { "foo" };
    write(
        root,
        ".changeset/change.md",
        &format!("---\n\"{selector}\": minor\n---\n\nChange foo.\n"),
    );
    commit(root, "changeset");
    dir
}

/// Versions cargo `foo` to 1.1.0, executes the release with fake publishers, and returns the tags it created.
fn released_tags(root: &Path) -> Vec<String> {
    let version = callisto(
        root,
        &["version", "--no-refresh-lockfiles", "--emit-decision", DECISION_PATH],
    );
    assert!(version.status.success(), "{}", String::from_utf8_lossy(&version.stderr));
    commit(root, "release");
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &release_commit]);
    let external = tempfile::tempdir().unwrap();
    let intent = external.path().join("intent.json");
    let plan = callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            &release_commit,
            "--decision",
            DECISION_PATH,
            "--orchestration-revision",
            &release_commit,
            "--artifact-repository",
            "example/foo",
            "--out",
            intent.to_str().unwrap(),
        ],
    );
    assert!(plan.status.success(), "{}", String::from_utf8_lossy(&plan.stderr));
    let receipt = external.path().join("receipt.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);
    let publishers = FakePublishers {
        bin: &bin,
        log: &log,
        forge_marker: &forge_marker,
        git_trace: &git_trace,
    };
    let run = execute(root, &intent, &receipt, publishers);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    git(root, &["tag", "--list"]).lines().map(str::to_owned).collect()
}

#[test]
fn a_package_with_a_unique_name_is_tagged_with_the_bare_name() {
    let dir = workspace(false, "");
    assert_eq!(released_tags(dir.path()), ["foo@1.1.0"]);
}

#[test]
fn a_name_shared_across_ecosystems_is_tagged_with_the_ecosystem_prefix() {
    let dir = workspace(true, "");
    assert_eq!(released_tags(dir.path()), ["cargo/foo@1.1.0"]);
}

#[test]
fn a_tag_template_overrides_the_bare_and_the_qualified_default() {
    for with_npm_foo in [false, true] {
        let dir = workspace(
            with_npm_foo,
            "[[package]]\nmatch = \"cargo/foo\"\ntag-template = \"pinned-foo-v{version}\"\n",
        );
        assert_eq!(
            released_tags(dir.path()),
            ["pinned-foo-v1.1.0"],
            "npm foo present: {with_npm_foo}"
        );
    }
}
