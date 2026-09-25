//! `[release].forge-repository` is the one release destination; profiles are gone.

use std::{fs, path::Path, process::Command};

const HEAD: &str = "[release]\nproduct-package = \"cargo/core-crate\"\n";
const ARTIFACTS: &str = "\n[[release.artifact]]\npackage = \"cargo/core-crate\"\ntarget = \"aarch64-apple-darwin\"\nasset-name = \"callisto-aarch64-apple-darwin.tar.gz\"\n\n[[release.artifact]]\npackage = \"cargo/core-crate\"\ntarget = \"x86_64-unknown-linux-gnu\"\nasset-name = \"callisto-x86_64-unknown-linux-gnu.tar.gz\"\n\n[[release.artifact]]\npackage = \"cargo/core-crate\"\ntarget = \"x86_64-unknown-linux-musl\"\nasset-name = \"callisto-x86_64-unknown-linux-musl.tar.gz\"\n\n[[release.artifact]]\npackage = \"cargo/core-crate\"\ntarget = \"wasm32-wasip1\"\nasset-name = \"callisto-moon.wasm\"\n";

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git").args(args).current_dir(root).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs `callisto status` over `[release]` with `keys`, its artifacts, then `fragment`.
fn status_with(keys: &str, fragment: &str) -> (bool, Vec<String>, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/core\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/core/src")).unwrap();
    fs::write(
        root.join("crates/core/Cargo.toml"),
        "[package]\nname = \"core-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("crates/core/src/lib.rs"), "\n").unwrap();
    fs::write(root.join("callisto.toml"), format!("{HEAD}{keys}{ARTIFACTS}{fragment}")).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial"]);
    let out = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--format", "json", "--cwd", root.to_str().unwrap(), "status"])
        .output()
        .unwrap();
    let mut codes = Vec::new();
    for stream in [&out.stdout, &out.stderr] {
        let text = String::from_utf8_lossy(stream);
        for token in text.split(|c: char| !c.is_ascii_alphanumeric()) {
            if token.len() == 4 && token.starts_with('E') && token[1..].chars().all(|c| c.is_ascii_digit()) {
                codes.push(token.to_owned());
            }
        }
    }
    codes.sort();
    codes.dedup();
    let detail = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), codes, detail)
}

const E197: &str = "E197";

/// Runs `callisto <args>` from a directory that does not exist, so any workspace load would fail.
fn without_workspace(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--cwd", "/nonexistent/callisto-workspace"])
        .args(args)
        .output()
        .unwrap()
}

/// AC-009
#[test]
fn release_plan_rejects_profile_as_an_unknown_argument() {
    let out = without_workspace(&["release", "plan", "--profile", "production", "--out", "i.json"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("unexpected argument '--profile'"), "{err}");
}

/// AC-010
#[test]
fn release_execute_rejects_profile_as_an_unknown_argument() {
    let out = without_workspace(&[
        "release",
        "execute",
        "--profile",
        "production",
        "--intent",
        "i.json",
        "--receipt",
        "r.json",
        "--orchestration-revision",
        &"a".repeat(40),
    ]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("unexpected argument '--profile'"), "{err}");
}

/// AC-001
#[test]
fn top_level_forge_repository_loads() {
    let (ok, codes, detail) = status_with("forge-repository = \"org/one\"\n", "");
    assert!(ok && codes.is_empty(), "codes {codes:?}: {detail}");
}

/// AC-002
#[test]
fn legacy_production_profile_still_loads() {
    let (ok, codes, detail) = status_with(
        "",
        "\n[release.profiles.production]\nforge-repository = \"org/one\"\nregistry-routes = { cratesIo = \"cratesIo\" }\n",
    );
    assert!(ok && codes.is_empty(), "codes {codes:?}: {detail}");
}

/// AC-005
#[test]
fn a_non_production_profile_is_rejected_naming_it() {
    let (ok, codes, detail) = status_with(
        "",
        "\n[release.profiles.rehearsal]\nforge-repository = \"org/rehearsal\"\n",
    );
    assert!(
        !ok && codes.iter().any(|c| c == E197) && detail.contains("`rehearsal`"),
        "expected E197, got ok={ok} codes {codes:?}: {detail}"
    );
}

/// AC-003
#[test]
fn a_conflicting_legacy_forge_repository_is_rejected() {
    let (ok, codes, detail) = status_with(
        "forge-repository = \"org/one\"\n",
        "\n[release.profiles.production]\nforge-repository = \"org/two\"\n",
    );
    assert!(
        !ok && codes.iter().any(|c| c == E197) && detail.contains("org/one") && detail.contains("org/two"),
        "expected E197, got ok={ok} codes {codes:?}: {detail}"
    );
}

fn repo_file(path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)).unwrap()
}

/// AC-018
#[test]
fn release_workflow_and_its_contract_pass_no_profile() {
    for path in [
        ".github/workflows/callisto-release.yml",
        ".github/tests/verify-release-workflow-contract.sh",
    ] {
        assert!(!repo_file(path).contains("--profile"), "{path}");
    }
}

/// AC-019
#[test]
fn this_repository_config_uses_top_level_forge_repository() {
    let text = repo_file("callisto.toml");
    assert!(!text.contains("[release.profiles") && !text.contains("registry-routes"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = callisto_graph::load_config(&root).unwrap();
    let forge = config.product_release.and_then(|release| release.forge_repository);
    assert_eq!(forge.map(|repo| repo.as_slug()).as_deref(), Some("orin-dx/callisto"));
}
