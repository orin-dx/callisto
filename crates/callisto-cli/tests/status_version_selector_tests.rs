//! `status` and `version` must attach a changeset entry to the same package for every selector kind.
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

fn run(root: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--format", "json", "--cwd", root.to_str().unwrap()])
        .args(args)
        .env("__CALLISTO_TEST_SLEEP_SCALE", "0")
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn cargo_pkg(root: &Path, dir: &str, name: &str, version: &str) {
    fs::create_dir_all(root.join(dir).join("src")).unwrap();
    fs::write(
        root.join(dir).join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
    )
    .unwrap();
    fs::write(root.join(dir).join("src/lib.rs"), "\n").unwrap();
}

fn npm_pkg(root: &Path, dir: &str, name: &str, version: &str) {
    fs::create_dir_all(root.join(dir)).unwrap();
    fs::write(
        root.join(dir).join("package.json"),
        format!("{{\n  \"name\": \"{name}\",\n  \"version\": \"{version}\"\n}}\n"),
    )
    .unwrap();
}

fn workspace(members: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    let list: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nmembers = [{}]\nresolver = \"2\"\n", list.join(", ")),
    )
    .unwrap();
    callisto_fixtures::scaffold_callisto(root);
    dir
}

fn commit(root: &Path) {
    for args in [vec!["add", "-A"], vec!["commit", "-m", "fixture"]] {
        let out = Command::new("git").args(&args).current_dir(root).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// Cargo `foo` alone.
fn unique() -> tempfile::TempDir {
    let dir = workspace(&["crates/foo"]);
    cargo_pkg(dir.path(), "crates/foo", "foo", "1.0.0");
    commit(dir.path());
    dir
}

/// One directory holding Cargo `native-core` and npm `@org/native-core-js`.
fn dual() -> tempfile::TempDir {
    let dir = workspace(&["crates/native-core"]);
    cargo_pkg(dir.path(), "crates/native-core", "native-core", "1.0.0");
    npm_pkg(dir.path(), "crates/native-core", "@org/native-core-js", "1.0.0");
    commit(dir.path());
    dir
}

/// Cargo `foo` and npm `foo` in different directories, so both are promoted.
fn promoted() -> tempfile::TempDir {
    let dir = workspace(&["crates/foo"]);
    cargo_pkg(dir.path(), "crates/foo", "foo", "1.0.0");
    npm_pkg(dir.path(), "packages/foo", "foo", "3.0.0");
    commit(dir.path());
    dir
}

/// Package `x` (Cargo `x`, npm `shared`) next to a separate Cargo `shared`.
fn secondary_name_collides_with_a_primary_name() -> tempfile::TempDir {
    let dir = workspace(&["crates/x", "crates/shared"]);
    cargo_pkg(dir.path(), "crates/x", "x", "1.0.0");
    npm_pkg(dir.path(), "crates/x", "shared", "1.0.0");
    cargo_pkg(dir.path(), "crates/shared", "shared", "2.0.0");
    commit(dir.path());
    dir
}

#[derive(Debug)]
struct Outcome {
    exit: Option<i32>,
    /// Packages the changeset entry is attached to; empty when the command rejected or ignored it.
    packages: BTreeSet<String>,
    /// Error-level diagnostic codes, or the failing command's error code.
    errors: Vec<String>,
}

fn package_names(value: &serde_json::Value, field: &str) -> Vec<String> {
    value[field]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| item["package"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn status_outcome(root: &Path) -> Outcome {
    let (exit, stdout, _) = run(root, &["status", "--check"]);
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("status prints a JSON report");
    let errors: Vec<String> = report["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["severity"] == "error")
        .map(|d| d["code"].as_str().unwrap_or_default().to_string())
        .collect();
    let attached = report["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["pendingChangesets"].as_array().is_some_and(|a| !a.is_empty()))
        .map(|p| p["package"].as_str().unwrap_or_default().to_string());
    let packages = if errors.is_empty() {
        attached.collect()
    } else {
        BTreeSet::new()
    };
    Outcome { exit, packages, errors }
}

fn version_outcome(root: &Path) -> Outcome {
    let (exit, stdout, stderr) = run(root, &["version", "--dry-run"]);
    if exit == Some(0) {
        let report: serde_json::Value = serde_json::from_str(&stdout).expect("version prints a JSON report");
        let packages = package_names(&report, "bumps").into_iter().collect();
        return Outcome {
            exit,
            packages,
            errors: Vec::new(),
        };
    }
    let report: serde_json::Value = serde_json::from_str(&stderr).expect("version prints a JSON error");
    Outcome {
        exit,
        packages: BTreeSet::new(),
        errors: vec![report["error"]["code"].as_str().unwrap_or_default().to_string()],
    }
}

/// Writes a changeset naming `selector` and runs both commands against it.
fn outcomes(root: &Path, selector: &str) -> (Outcome, Outcome) {
    fs::write(
        root.join(".changeset/probe.md"),
        format!("---\n\"{selector}\": minor\n---\n\nChange.\n"),
    )
    .unwrap();
    (status_outcome(root), version_outcome(root))
}

fn assert_same_packages(root: &Path, selector: &str) {
    let (status, version) = outcomes(root, selector);
    assert_eq!(
        status.packages, version.packages,
        "selector `{selector}` resolves differently:\n  status:  {status:?}\n  version: {version:?}"
    );
}

#[test]
fn unique_package_bare_and_qualified_selectors_agree() {
    let dir = unique();
    for selector in ["foo", "cargo/foo", "cargo:foo"] {
        assert_same_packages(dir.path(), selector);
        let (status, version) = outcomes(dir.path(), selector);
        assert_eq!(status.packages, BTreeSet::from(["foo".to_string()]), "{selector}");
        assert_eq!(version.packages, BTreeSet::from(["foo".to_string()]), "{selector}");
    }
}

#[test]
fn unknown_name_is_resolved_by_neither_command() {
    for dir in [unique(), dual(), promoted()] {
        let (status, version) = outcomes(dir.path(), "nonexistent");
        assert!(status.packages.is_empty(), "{status:?}");
        assert!(version.packages.is_empty(), "{version:?}");
        assert_eq!(status.exit, Some(1), "{status:?}");
        assert_eq!(status.errors, ["unknown-package"]);
    }
}

#[test]
fn dual_manifest_primary_name_selectors_agree() {
    let dir = dual();
    for selector in ["native-core", "cargo/native-core", "cargo:native-core"] {
        assert_same_packages(dir.path(), selector);
        let (status, _) = outcomes(dir.path(), selector);
        assert_eq!(
            status.packages,
            BTreeSet::from(["native-core".to_string()]),
            "{selector}"
        );
    }
}

#[test]
fn dual_manifest_name_in_the_wrong_ecosystem_is_resolved_by_neither_command() {
    let dir = dual();
    assert_same_packages(dir.path(), "cargo/@org/native-core-js");
    let (status, version) = outcomes(dir.path(), "cargo/@org/native-core-js");
    assert!(status.packages.is_empty() && version.packages.is_empty());
}

#[test]
fn promoted_qualified_selectors_agree() {
    let dir = promoted();
    for (selector, expected) in [
        ("cargo/foo", "cargo/foo"),
        ("npm/foo", "npm/foo"),
        ("npm:foo", "npm/foo"),
    ] {
        assert_same_packages(dir.path(), selector);
        let (status, _) = outcomes(dir.path(), selector);
        assert_eq!(status.packages, BTreeSet::from([expected.to_string()]), "{selector}");
    }
}

#[test]
fn promoted_bare_name_is_ambiguous_in_both_commands() {
    let dir = promoted();
    let (status, version) = outcomes(dir.path(), "foo");
    assert_eq!(status.errors, ["ambiguous-package-name"]);
    assert_eq!(version.errors, ["E103"]);
    assert_eq!(version.exit, Some(1));
    assert!(status.packages.is_empty() && version.packages.is_empty());
}

#[test]
fn promoted_qualified_selector_for_an_absent_ecosystem_is_resolved_by_neither_command() {
    let dir = promoted();
    assert_same_packages(dir.path(), "pypi/foo");
    let (status, version) = outcomes(dir.path(), "pypi/foo");
    assert!(status.packages.is_empty() && version.packages.is_empty());
}

#[test]
fn colliding_secondary_name_qualified_by_its_own_ecosystem_agrees() {
    let dir = secondary_name_collides_with_a_primary_name();
    for selector in ["x", "cargo/x", "cargo/shared"] {
        assert_same_packages(dir.path(), selector);
    }
}

#[test]
fn secondary_native_name_of_a_dual_manifest_package_resolves_the_same() {
    let dir = dual();
    assert_same_packages(dir.path(), "@org/native-core-js");
}

#[test]
fn qualified_secondary_native_name_of_a_dual_manifest_package_resolves_the_same() {
    let dir = dual();
    for selector in ["npm/@org/native-core-js", "npm:@org/native-core-js"] {
        assert_same_packages(dir.path(), selector);
    }
}

#[test]
fn qualified_selector_for_an_ecosystem_that_lacks_the_name_resolves_the_same() {
    let dir = unique();
    for selector in ["npm/foo", "npm:foo", "pypi/foo"] {
        assert_same_packages(dir.path(), selector);
    }
}

#[test]
fn qualified_primary_name_in_a_second_manifests_ecosystem_resolves_the_same() {
    let dir = dual();
    assert_same_packages(dir.path(), "npm/native-core");
}

#[test]
fn bare_name_that_is_one_package_primary_and_anothers_secondary_resolves_the_same() {
    let dir = secondary_name_collides_with_a_primary_name();
    assert_same_packages(dir.path(), "shared");
}

#[test]
fn qualified_secondary_name_that_equals_another_packages_primary_resolves_the_same() {
    let dir = secondary_name_collides_with_a_primary_name();
    assert_same_packages(dir.path(), "npm/shared");
}

fn status_report(root: &Path) -> (Option<i32>, serde_json::Value) {
    let (exit, stdout, _) = run(root, &["status", "--check"]);
    (
        exit,
        serde_json::from_str(&stdout).expect("status prints a JSON report"),
    )
}

fn planned_packages(report: &serde_json::Value) -> BTreeSet<String> {
    report["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| !p["pendingSeverity"].is_null())
        .map(|p| p["package"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn write_changeset(root: &Path, selectors: &[(&str, &str)]) {
    let lines: Vec<String> = selectors.iter().map(|(s, b)| format!("\"{s}\": {b}\n")).collect();
    fs::write(
        root.join(".changeset/probe.md"),
        format!("---\n{}---\n\nChange.\n", lines.concat()),
    )
    .unwrap();
}

#[test]
fn status_check_passes_only_when_version_consumes_the_changeset() {
    let selectors = [
        "foo",
        "cargo/foo",
        "npm/foo",
        "pypi/foo",
        "@org/native-core-js",
        "npm/@org/native-core-js",
        "cargo/@org/native-core-js",
        "native-core",
        "npm/native-core",
        "shared",
        "npm/shared",
        "cargo/shared",
        "nonexistent",
    ];
    for dir in [
        unique(),
        dual(),
        promoted(),
        secondary_name_collides_with_a_primary_name(),
    ] {
        for selector in selectors {
            write_changeset(dir.path(), &[(selector, "minor")]);
            let (exit, report) = status_report(dir.path());
            let version = version_outcome(dir.path());
            let planned = planned_packages(&report);
            if exit == Some(0) {
                assert!(
                    !version.packages.is_empty(),
                    "`{selector}`: status passes but version consumes nothing"
                );
                assert_eq!(planned, version.packages, "`{selector}`");
            } else {
                assert!(
                    planned.is_empty(),
                    "`{selector}`: status reports {planned:?} planned while rejecting the entry"
                );
                assert!(
                    version.packages.is_empty(),
                    "`{selector}`: version bumps {:?}",
                    version.packages
                );
            }
        }
    }
}

#[test]
fn two_selectors_for_one_package_attach_it_once_with_the_higher_severity() {
    let dir = dual();
    write_changeset(
        dir.path(),
        &[
            ("native-core", "patch"),
            ("@org/native-core-js", "minor"),
            ("npm/@org/native-core-js", "patch"),
        ],
    );
    let (exit, report) = status_report(dir.path());
    assert_eq!(exit, Some(0), "{report}");
    let pkg = &report["packages"][0];
    assert_eq!(pkg["package"], "native-core");
    assert_eq!(pkg["pendingChangesets"], serde_json::json!(["probe"]));
    assert_eq!(pkg["pendingSeverity"], "minor");
    assert_eq!(
        version_outcome(dir.path()).packages,
        BTreeSet::from(["native-core".to_string()])
    );
}

#[test]
fn a_selector_naming_a_group_members_secondary_name_plans_the_whole_group() {
    let dir = workspace(&["crates/native-core", "crates/other"]);
    cargo_pkg(dir.path(), "crates/native-core", "native-core", "1.0.0");
    npm_pkg(dir.path(), "crates/native-core", "@org/native-core-js", "1.0.0");
    cargo_pkg(dir.path(), "crates/other", "other", "1.0.0");
    fs::write(
        dir.path().join("callisto.toml"),
        "[[fixed-group]]\nname = \"pair\"\nmembers = [\"@org/native-core-js\", \"other\"]\n",
    )
    .unwrap();
    commit(dir.path());
    for selector in ["@org/native-core-js", "npm/@org/native-core-js", "native-core"] {
        write_changeset(dir.path(), &[(selector, "minor")]);
        let (exit, report) = status_report(dir.path());
        let version = version_outcome(dir.path());
        assert_eq!(exit, Some(0), "`{selector}`: {report}");
        assert_eq!(
            planned_packages(&report),
            BTreeSet::from(["native-core".to_string(), "other".to_string()]),
            "`{selector}`"
        );
        assert_eq!(planned_packages(&report), version.packages, "`{selector}`");
    }
}

#[test]
fn a_group_naming_one_package_by_two_of_its_names_is_rejected() {
    let dir = workspace(&["crates/native-core"]);
    cargo_pkg(dir.path(), "crates/native-core", "native-core", "1.0.0");
    npm_pkg(dir.path(), "crates/native-core", "@org/native-core-js", "1.0.0");
    fs::write(
        dir.path().join("callisto.toml"),
        "[[fixed-group]]\nname = \"pair\"\nmembers = [\"native-core\", \"@org/native-core-js\"]\n",
    )
    .unwrap();
    commit(dir.path());
    let (exit, stdout, stderr) = run(dir.path(), &["status"]);
    assert_ne!(exit, Some(0), "{stdout}{stderr}");
    assert!(format!("{stdout}{stderr}").contains("E250"), "{stdout}{stderr}");
}

#[test]
fn selectors_resolve_the_same_in_a_prerelease_workspace() {
    let dir = dual();
    let (exit, _, stderr) = run(dir.path(), &["pre", "enter", "beta"]);
    assert_eq!(exit, Some(0), "{stderr}");
    commit(dir.path());
    for selector in [
        "@org/native-core-js",
        "npm/@org/native-core-js",
        "native-core",
        "npm/native-core",
    ] {
        assert_same_packages(dir.path(), selector);
    }
    let (status, version) = outcomes(dir.path(), "@org/native-core-js");
    assert_eq!(status.packages, BTreeSet::from(["native-core".to_string()]));
    assert_eq!(version.packages, status.packages);
}
