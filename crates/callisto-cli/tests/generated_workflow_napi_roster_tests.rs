#![cfg(unix)]

//! The generated build-matrix workflow's napi build and placement rosters come from the release
//! intent, not from every napi addon in the workspace. Runs the template's own shell steps with
//! the real `callisto` binary and the real `jq`.

use std::fs;
use std::path::Path;
use std::process::Command;

const TEMPLATE: &str = include_str!("../../callisto-graph/src/templates/matrix_workflow.tmpl");

/// The `run: |` script of the step named `name`, dedented.
fn step_script(name: &str) -> String {
    let lines: Vec<&str> = TEMPLATE.lines().collect();
    let header = format!("- name: {name}");
    let start = lines
        .iter()
        .position(|line| line.trim() == header)
        .unwrap_or_else(|| panic!("no step `{name}`"));
    let run = lines[start..]
        .iter()
        .position(|line| line.trim() == "run: |")
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("step `{name}` has no run block"));
    let indent = lines[run].len() - lines[run].trim_start().len() + 2;
    lines[run + 1..]
        .iter()
        .take_while(|line| line.trim().is_empty() || line.len() - line.trim_start().len() >= indent)
        .map(|line| line.get(indent..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn git(root: &Path, args: &[&str]) {
    callisto_fixtures::git::run_git(root, args);
}

/// A Cargo crate `crate_name` next to an npm package `npm_name` with napi targets, in `dir`.
/// A `broken` addon has no crate to build, which makes `callisto matrix` fail on it with E204.
fn addon_with(root: &Path, dir: &str, crate_name: &str, npm_name: &str, broken: bool) {
    let path = root.join(dir);
    fs::create_dir_all(path.join("src")).unwrap();
    if !broken {
        fs::write(path.join("src/lib.rs"), "\n").unwrap();
        fs::write(
            path.join("Cargo.toml"),
            format!("[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nnapi = \"3\"\n"),
        )
        .unwrap();
    }
    fs::write(
        path.join("package.json"),
        format!(
            r#"{{"name":"{npm_name}","version":"0.1.0","napi":{{"binaryName":"{crate_name}","targets":["aarch64-apple-darwin"]}}}}"#
        ),
    )
    .unwrap();
}

fn addon(root: &Path, dir: &str, crate_name: &str, npm_name: &str) {
    addon_with(root, dir, crate_name, npm_name, false);
}

/// A workspace releasing `lib` (and `wanted` when asked), plus an addon `unrelated` that was
/// released in an earlier commit.
fn workspace(release_wanted: bool) -> tempfile::TempDir {
    workspace_with(release_wanted, false)
}

/// As [`workspace`]; `broken_unrelated` makes the settled addon fail `callisto matrix`.
fn workspace_with(release_wanted: bool, broken_unrelated: bool) -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    callisto_fixtures::git::init_repo(root);
    let mut members = vec!["crates/lib"];
    if !broken_unrelated {
        members.push("unrelated");
    }
    if release_wanted {
        members.push("wanted");
    }
    let members = format!("{members:?}");
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nmembers = {members}\nresolver = \"2\"\n"),
    )
    .unwrap();
    fs::write(root.join("callisto.toml"), "").unwrap();
    fs::create_dir_all(root.join("crates/lib/src")).unwrap();
    fs::write(root.join("crates/lib/src/lib.rs"), "\n").unwrap();
    fs::write(
        root.join("crates/lib/Cargo.toml"),
        "[package]\nname = \"lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    addon_with(root, "unrelated", "unrelated", "unrelated-js", broken_unrelated);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "settled addon"]);
    git(root, &["tag", "unrelated@0.1.0"]);
    git(root, &["tag", "unrelated-js@0.1.0"]);
    if release_wanted {
        addon(root, "wanted", "wanted", "wanted-js");
    }
    fs::write(root.join("crates/lib/src/lib.rs"), "pub fn later() {}\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "later work"]);
    tmp
}

struct Run {
    github_output: String,
    placed: String,
}

/// Runs the plan step's matrix script and the execute job's napi placement script from a real
/// intent for `root`.
fn run_steps(root: &Path) -> Run {
    let callisto = env!("CARGO_BIN_EXE_callisto");
    let preview = Command::new(callisto)
        .args([
            "--cwd",
            root.to_str().unwrap(),
            "--format",
            "json",
            "--dry-run",
            "release",
        ])
        .output()
        .unwrap();
    assert!(
        preview.status.success(),
        "release preview: {}",
        String::from_utf8_lossy(&preview.stderr)
    );
    // The preview flattens the intent into the report envelope; `release plan` writes the bare intent.
    let mut intent: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    for envelope in ["schemaVersion", "command", "dryRun", "diagnostics"] {
        intent.as_object_mut().unwrap().remove(envelope);
    }

    let scratch = tempfile::TempDir::new().unwrap();
    let bin = scratch.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(callisto, bin.join("callisto")).unwrap();
    let placed_log = scratch.path().join("placed.log");
    let npx = bin.join("npx");
    fs::write(&npx, format!("#!/bin/sh\necho \"$*\" >> '{}'\n", placed_log.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&npx, fs::Permissions::from_mode(0o755)).unwrap();

    let runner_temp = scratch.path().join("runner");
    let intent_dir = runner_temp.join("release-intent");
    fs::create_dir_all(&intent_dir).unwrap();
    fs::write(
        intent_dir.join("release-intent.json"),
        serde_json::to_vec(&intent).unwrap(),
    )
    .unwrap();

    let github_output = scratch.path().join("github-output");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let bash = |script: String, napi_dirs: &str| {
        let output = Command::new("bash")
            .args(["-c", &script])
            .current_dir(root)
            .env("PATH", &path)
            .env("RUNNER_TEMP", &runner_temp)
            .env("GITHUB_OUTPUT", &github_output)
            .env("NAPI_DIRS", napi_dirs)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{script}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    bash(step_script("Build the matrix from napi targets and artifact slots"), "");
    let planned = Run {
        github_output: fs::read_to_string(&github_output).unwrap(),
        placed: String::new(),
    };
    // The placement step only runs when the plan found napi builds, and reads the plan job's output.
    if output(&planned, "has_napi") == "true" {
        bash(step_script("Place napi artifacts"), &output(&planned, "napi_dirs"));
    }
    Run {
        placed: fs::read_to_string(&placed_log).unwrap_or_default(),
        ..planned
    }
}

fn output(run: &Run, key: &str) -> String {
    run.github_output
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("no `{key}` output:\n{}", run.github_output))
        .to_owned()
}

fn matrix_packages(run: &Run) -> Vec<String> {
    let matrix: serde_json::Value = serde_json::from_str(&output(run, "matrix")).unwrap();
    matrix
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["packageName"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn an_unrelated_napi_addon_adds_no_build_and_no_placement() {
    let tmp = workspace(false);
    let run = run_steps(tmp.path());
    assert_eq!(output(&run, "matrix"), "[]", "{}", run.github_output);
    assert_eq!(output(&run, "has_napi"), "false");
    assert_eq!(run.placed, "", "no napi placement for a Cargo-only release");
}

#[test]
fn a_misconfigured_unrelated_addon_cannot_block_the_release_plan() {
    let tmp = workspace_with(false, true);
    let run = run_steps(tmp.path());
    assert_eq!(output(&run, "matrix"), "[]", "{}", run.github_output);
    assert_eq!(output(&run, "has_napi"), "false");
}

#[test]
fn only_the_addon_in_the_release_is_built_and_placed() {
    let tmp = workspace(true);
    let run = run_steps(tmp.path());
    let packages = matrix_packages(&run);
    assert!(!packages.is_empty(), "{}", run.github_output);
    assert!(
        packages.iter().all(|name| name.contains("wanted")),
        "built an unrelated addon: {packages:?}"
    );
    assert_eq!(output(&run, "has_napi"), "true");
    assert!(run.placed.contains("--cwd wanted"), "{}", run.placed);
    assert!(!run.placed.contains("unrelated"), "{}", run.placed);
}
