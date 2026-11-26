//! `callisto add --package` writes a changeset that `status` reads back as the same package, or refuses the name
//! before writing anything.

use std::path::Path;
use std::process::{Command, Output, Stdio};

/// A single npm package named `name`, with nothing released.
fn workspace(name: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    let manifest = serde_json::json!({ "name": name, "version": "1.0.0" });
    std::fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    std::fs::write(root.join("callisto.toml"), "").unwrap();
    dir
}

fn callisto(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--cwd", root.to_str().unwrap()])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn add(root: &Path, name: &str, dry_run: bool) -> Output {
    let package = format!("npm/{name}:patch");
    let mut args = vec!["add", "--package", &package, "--summary", "Fix"];
    if dry_run {
        args.push("--dry-run");
    }
    callisto(root, &args)
}

fn written(root: &Path) -> Vec<String> {
    std::fs::read_dir(root.join(".changeset"))
        .map(|entries| {
            entries
                .map(|e| e.unwrap().path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
                .map(|path| std::fs::read_to_string(path).unwrap())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_name_a_changeset_cannot_hold_is_refused_before_anything_is_written() {
    for name in ["bad\nname", "bad\rname", "bad\"name"] {
        for dry_run in [false, true] {
            let dir = workspace(name);
            let out = add(dir.path(), name, dry_run);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(!out.status.success(), "{name:?} dry_run={dry_run}: {stderr}");
            assert!(stderr.contains("E268"), "{name:?} dry_run={dry_run}: {stderr}");
            assert!(
                !String::from_utf8_lossy(&out.stdout).contains("---"),
                "{name:?}: nothing is previewed"
            );
            assert!(
                written(dir.path()).is_empty(),
                "{name:?} dry_run={dry_run} must write nothing"
            );
        }
    }
}

#[test]
fn every_other_name_is_pending_for_the_same_package_after_add() {
    for name in [
        " lead",
        "trail ",
        "\tlead",
        "trail\t",
        "\u{a0}lead",
        "trail\u{a0}",
        "in\tside",
        "a:b",
        "a#b",
        "@scope/pkg",
        "plain",
    ] {
        let dir = workspace(name);
        let root = dir.path();
        let preview = add(root, name, true);
        assert!(
            preview.status.success(),
            "{name:?}: {}",
            String::from_utf8_lossy(&preview.stderr)
        );
        assert!(written(root).is_empty(), "{name:?}: a dry run writes nothing");

        let out = add(root, name, false);
        assert!(
            out.status.success(),
            "{name:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let status = callisto(root, &["status", "--format", "json"]);
        let report: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
        assert_eq!(report["pending"], 1, "{name:?} wrote {:?}: {report}", written(root));
        assert!(report.get("diagnostics").is_none(), "{name:?}: {report}");
        assert_eq!(report["packages"][0]["pendingSeverity"], "patch", "{name:?}: {report}");
    }
}
