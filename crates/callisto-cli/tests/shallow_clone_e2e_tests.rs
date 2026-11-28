//! Commit-based inference reads history; in a shallow clone it would read a shorter one and plan a smaller bump.
#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;
use std::path::Path;

use release_harness::{callisto, git};

/// A crate with three commits; `auto` makes it infer its bump from commits.
fn history(auto: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "\n").unwrap();
    if auto {
        fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"cargo/lib\"\nrelease-trigger = \"auto\"\n",
        )
        .unwrap();
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "chore: start"]);
    for message in ["feat: one", "feat: two"] {
        fs::write(root.join("src/lib.rs"), format!("// {message}\n")).unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-m", message]);
    }
    dir
}

fn shallow_clone(source: &Path) -> tempfile::TempDir {
    let clone = tempfile::tempdir().unwrap();
    git(
        clone.path(),
        &["clone", "--depth", "1", &format!("file://{}", source.display()), "."],
    );
    assert_eq!(git(clone.path(), &["rev-parse", "--is-shallow-repository"]), "true");
    clone
}

fn stdout_and_stderr(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn inference_in_a_shallow_clone_is_refused_not_planned_from_partial_history() {
    let source = history(true);
    let clone = shallow_clone(source.path());

    let status = callisto(clone.path(), &["status"]);
    assert!(!status.status.success());
    assert!(
        stdout_and_stderr(&status).contains("E315"),
        "{}",
        stdout_and_stderr(&status)
    );

    let full = callisto(source.path(), &["status"]);
    assert!(full.status.success(), "{}", stdout_and_stderr(&full));
}

#[test]
fn a_shallow_clone_is_fine_when_no_package_infers_from_commits() {
    let source = history(false);
    let clone = shallow_clone(source.path());

    let status = callisto(clone.path(), &["status"]);
    assert!(status.status.success(), "{}", stdout_and_stderr(&status));
}

// The pending-version-work hint reads changesets only, so an auto package elsewhere must not hide it in a shallow clone.
#[test]
fn the_pending_changeset_hint_survives_a_shallow_clone_with_an_auto_package() {
    let source = tempfile::tempdir().unwrap();
    let root = source.path();
    callisto_fixtures::git::init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"auto\", \"manual\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    for name in ["auto", "manual"] {
        fs::create_dir_all(root.join(name).join("src")).unwrap();
        fs::write(
            root.join(name).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\ndescription = \"d\"\nlicense = \"MIT\"\n"
            ),
        )
        .unwrap();
        fs::write(root.join(name).join("src/lib.rs"), "\n").unwrap();
    }
    fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"cargo/auto\"\nrelease-trigger = \"auto\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(
        root.join(".changeset/pending.md"),
        "---\n\"cargo/manual\": minor\n---\n\nAdd a thing.\n",
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "chore: start"]);
    fs::write(root.join("auto/src/lib.rs"), "// change\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "feat: change"]);
    let clone = shallow_clone(source.path());

    let output = callisto(clone.path(), &["release", "--dry-run", "--package", "cargo/manual"]);
    let text = stdout_and_stderr(&output);
    assert!(text.contains("pending version work for `cargo/manual`"), "{text}");
}
