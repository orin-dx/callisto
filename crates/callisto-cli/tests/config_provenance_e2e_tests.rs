//! End-to-end coverage for the ConfigProvenance rendering fix: before it,
//! `render_version`'s text output never consulted `BumpRecord.governed_by`,
//! so an operator running the real CLI had no way to see which
//! `callisto.toml` key (or default) drove a cascade bump. The in-process
//! unit test next to the fix (`render/mod.rs`) proves `render_version`
//! itself is wired correctly against a hand-built `VersionReport`; this
//! test proves the attribution line survives all the way from a real
//! dependency graph, through the cascade solver, to the actual text a user
//! sees on stdout.

use std::path::Path;
use std::process::Command;

fn callisto_bin() -> &'static str {
    env!("CARGO_BIN_EXE_callisto")
}

fn git_init(root: &Path) {
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.name", "Callisto Tester"],
        vec!["config", "user.email", "tester@callisto.dev"],
        vec!["config", "commit.gpgsign", "false"],
        vec!["config", "tag.gpgsign", "false"],
    ] {
        assert!(Command::new("git")
            .args(&args)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }
}

fn git_commit_all(root: &Path, message: &str) {
    assert!(Command::new("git")
        .args(["add", "-A"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", message])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
}

fn run_callisto_text(root: &Path, args: &[&str]) -> String {
    let output = Command::new(callisto_bin())
        .arg("--cwd")
        .arg(root)
        .args(args)
        .output()
        .expect("failed to spawn callisto binary");
    assert!(
        output.status.success(),
        "callisto {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A cascade-driven bump (upstream-crate goes major, downstream-crate's
/// `"1.0"` requirement no longer covers it) must print a "governed by
/// [cascade].bump-severity" attribution line beneath the bump in the real
/// text renderer a user sees -- not just in a hand-constructed
/// `VersionReport` fed directly to `render_version` in a unit test.
#[test]
fn version_dry_run_text_output_shows_cascade_attribution_for_a_real_dependency_graph() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git_init(root);

    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/upstream-crate\", \"crates/downstream-crate\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("callisto.toml"), "").unwrap();

    std::fs::create_dir_all(root.join("crates/upstream-crate/src")).unwrap();
    std::fs::write(
        root.join("crates/upstream-crate/Cargo.toml"),
        "[package]\nname = \"upstream-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(root.join("crates/upstream-crate/src/lib.rs"), "pub fn hello() {}\n").unwrap();

    std::fs::create_dir_all(root.join("crates/downstream-crate/src")).unwrap();
    std::fs::write(
        root.join("crates/downstream-crate/Cargo.toml"),
        "[package]\nname = \"downstream-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[dependencies]\nupstream-crate = { path = \"../upstream-crate\", version = \"1.0\" }\n",
    )
    .unwrap();
    std::fs::write(root.join("crates/downstream-crate/src/lib.rs"), "pub fn world() {}\n").unwrap();

    git_commit_all(root, "Initial commit");

    let add_out = run_callisto_text(
        root,
        &[
            "add",
            "--package",
            "upstream-crate:major",
            "--summary",
            "breaking change",
        ],
    );
    assert!(add_out.contains("Added changeset"), "add did not succeed: {add_out}");

    let plan_text = run_callisto_text(root, &["version", "--dry-run"]);

    assert!(
        plan_text.contains("upstream-crate 1.0.0"),
        "upstream-crate's own bump must be listed: {plan_text}"
    );
    assert!(
        plan_text.contains("downstream-crate"),
        "downstream-crate must cascade since its \"1.0\" requirement no longer covers 2.0.0: {plan_text}"
    );
    assert!(
        plan_text.contains("governed by [cascade].bump-severity"),
        "the cascade-driven bump must print its ConfigProvenance attribution line in real CLI text output: {plan_text}"
    );
}
