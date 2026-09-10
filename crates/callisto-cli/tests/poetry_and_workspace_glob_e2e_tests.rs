//! End-to-end coverage for two silent-graph-corruption bugs fixed together:
//! Poetry caret/tilde/wildcard specs being dropped from the dependency graph
//! (`callisto-manifests/src/python.rs`), and npm/pnpm workspace glob
//! negation being ignored (`callisto-graph/src/locate/membership.rs`).
//!
//! Every test here shells out to the real compiled binary and asserts on its
//! actual stdout, exactly what a user would see -- the unit tests next to
//! the fix only prove the parsing/matching functions are correct in
//! isolation; these prove the fix survives all the way through workspace
//! discovery, the dependency graph, and the cascade solver.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

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

fn run_callisto(root: &Path, args: &[&str]) -> serde_json::Value {
    let output = Command::new(callisto_bin())
        .arg("--cwd")
        .arg(root)
        .arg("--format")
        .arg("json")
        .args(args)
        .output()
        .expect("failed to spawn callisto binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "callisto {args:?} did not emit valid JSON: {e}\nstdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// Sets up a `.moon`-marked workspace root with two Poetry packages:
/// `pkg-a` at 1.0.0, and `pkg-b` at 1.0.0 declaring
/// `[tool.poetry.dependencies] pkg-a = "^1.0"`.
fn setup_poetry_workspace() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git_init(root);

    std::fs::create_dir_all(root.join(".moon")).unwrap();
    std::fs::write(root.join("callisto.toml"), "").unwrap();
    std::fs::write(
        root.join("pyproject.toml"),
        "[tool.uv.workspace]\nmembers = [\"packages/*\"]\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("packages/pkg-a")).unwrap();
    std::fs::write(
        root.join("packages/pkg-a/pyproject.toml"),
        "[project]\nname = \"pkg-a\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("packages/pkg-b")).unwrap();
    std::fs::write(
        root.join("packages/pkg-b/pyproject.toml"),
        "[project]\nname = \"pkg-b\"\nversion = \"1.0.0\"\n\n[tool.poetry.dependencies]\npkg-a = \"^1.0\"\n",
    )
    .unwrap();

    git_commit_all(root, "Initial commit");
    dir
}

/// Regression test for the confirmed silent-drop bug: before the fix,
/// `iter_dependencies` never yielded a `DependencyEntry` for a Poetry
/// caret/tilde/wildcard spec because the raw PEP 440 parse attempt failed,
/// so no graph edge was ever created and `pkg-b` never saw `pkg-a`'s bump.
/// A major bump on `pkg-a` (1.0.0 -> 2.0.0) falls outside `pkg-b`'s `^1.0`
/// range (`>=1.0.0,<2.0.0`), so a correctly-wired dependency graph must
/// cascade a bump onto `pkg-b`.
#[test]
fn poetry_caret_dependency_cascades_a_major_bump_that_breaks_its_range() {
    let dir = setup_poetry_workspace();
    let root = dir.path();

    let add = run_callisto(
        root,
        &["add", "--package", "pkg-a:major", "--summary", "breaking change"],
    );
    assert!(add.get("error").is_none(), "add failed: {add}");

    let plan = run_callisto(root, &["version", "--dry-run"]);
    let bumps = plan["bumps"]
        .as_array()
        .expect("version --dry-run must emit a bumps array");

    let pkg_a = bumps
        .iter()
        .find(|b| b["package"] == "pkg-a")
        .expect("pkg-a must be in the plan");
    assert_eq!(pkg_a["to"], "2.0.0");

    let pkg_b = bumps.iter().find(|b| b["package"] == "pkg-b").unwrap_or_else(|| {
        panic!("pkg-b must cascade from pkg-a's major bump (its ^1.0 range no longer covers 2.0.0); got plan: {plan}")
    });
    assert_eq!(pkg_b["from"], "1.0.0");
    assert_eq!(pkg_b["reason"]["kind"], "cascade");
    assert_eq!(pkg_b["reason"]["via"], "pkg-a");
    assert_eq!(
        pkg_b["reason"]["spec"], "^1.0",
        "the cascade reason must carry the original Poetry spec text"
    );
}

/// Corner case guarding against over-eager cascading: a minor bump on
/// `pkg-a` (1.0.0 -> 1.1.0) stays within `pkg-b`'s `^1.0` range
/// (`>=1.0.0,<2.0.0`), so the normalized Poetry spec must still correctly
/// report coverage and `pkg-b` must NOT be bumped.
#[test]
fn poetry_caret_dependency_within_range_does_not_cascade() {
    let dir = setup_poetry_workspace();
    let root = dir.path();

    let add = run_callisto(
        root,
        &["add", "--package", "pkg-a:minor", "--summary", "compatible addition"],
    );
    assert!(add.get("error").is_none(), "add failed: {add}");

    let plan = run_callisto(root, &["version", "--dry-run"]);
    let bumps = plan["bumps"]
        .as_array()
        .expect("version --dry-run must emit a bumps array");

    assert_eq!(
        bumps.len(),
        1,
        "only pkg-a should bump; pkg-b's ^1.0 range still covers 1.1.0: {plan}"
    );
    assert_eq!(bumps[0]["package"], "pkg-a");
}

/// A Poetry dependency this normalizer cannot represent (here: no `version`
/// key at all, e.g. a path/git dependency) must never make the package
/// vanish from workspace discovery -- the pre-fix code's failure mode was a
/// silently *dropped dependency edge*, not a crash, so the sibling-safe
/// assertion is that the workspace still loads and both packages are still
/// visible in `status`.
#[test]
fn poetry_dependency_with_no_version_key_does_not_break_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git_init(root);

    std::fs::create_dir_all(root.join(".moon")).unwrap();
    std::fs::write(root.join("callisto.toml"), "").unwrap();
    std::fs::write(
        root.join("pyproject.toml"),
        "[tool.uv.workspace]\nmembers = [\"packages/*\"]\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("packages/pkg-a")).unwrap();
    std::fs::write(
        root.join("packages/pkg-a/pyproject.toml"),
        "[project]\nname = \"pkg-a\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("packages/pkg-b")).unwrap();
    std::fs::write(
        root.join("packages/pkg-b/pyproject.toml"),
        "[project]\nname = \"pkg-b\"\nversion = \"1.0.0\"\n\n[tool.poetry.dependencies]\npkg-a = { git = \"https://example.invalid/pkg-a.git\" }\n",
    )
    .unwrap();

    git_commit_all(root, "Initial commit");

    let status = run_callisto(root, &["status"]);
    assert!(
        status.get("error").is_none(),
        "status must not error on a non-versioned Poetry dependency: {status}"
    );
    let packages = status["packages"]
        .as_array()
        .expect("status must emit a packages array");
    let names: Vec<&str> = packages.iter().filter_map(|p| p["package"].as_str()).collect();
    assert!(names.contains(&"pkg-a"), "pkg-a must still be discovered: {names:?}");
    assert!(
        names.contains(&"pkg-b"),
        "pkg-b must still be discovered despite its unrepresentable dependency: {names:?}"
    );
}

/// Regression test for the confirmed silent-mishandling bug: before the fix,
/// `build_globset` compiled a `!`-prefixed entry as a literal positive glob
/// that can never match any real path, so the exclusion was silently a
/// no-op and `excluded-one` was wrongly admitted as a workspace member.
#[test]
fn npm_workspace_negated_glob_excludes_matching_package() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git_init(root);

    std::fs::write(
        root.join("package.json"),
        r#"{"name":"root-workspace","version":"0.0.0","private":true,"workspaces":["packages/*","!packages/excluded-one"]}"#,
    )
    .unwrap();
    std::fs::write(root.join("callisto.toml"), "").unwrap();

    std::fs::create_dir_all(root.join("packages/kept")).unwrap();
    std::fs::write(
        root.join("packages/kept/package.json"),
        r#"{"name":"kept","version":"1.0.0"}"#,
    )
    .unwrap();

    std::fs::create_dir_all(root.join("packages/excluded-one")).unwrap();
    std::fs::write(
        root.join("packages/excluded-one/package.json"),
        r#"{"name":"excluded-one","version":"9.9.9"}"#,
    )
    .unwrap();

    git_commit_all(root, "Initial commit");

    let status = run_callisto(root, &["status"]);
    assert!(status.get("error").is_none(), "status failed: {status}");
    let packages = status["packages"]
        .as_array()
        .expect("status must emit a packages array");
    let names: Vec<&str> = packages.iter().filter_map(|p| p["package"].as_str()).collect();

    assert!(
        names.contains(&"kept"),
        "a package not named by the negative pattern must still be admitted: {names:?}"
    );
    assert!(
        !names.contains(&"excluded-one"),
        "a `!`-prefixed workspaces entry must exclude the package from discovery entirely, not just fail to error on it: {names:?}"
    );
}
