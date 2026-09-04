//! Proves `release-pr commit-plan`'s output is byte-faithful to what Git
//! itself would commit: applying the plan's additions/deletions onto a
//! fresh checkout of `--base-commit` must reproduce the exact tree the
//! staged index actually held when the plan was built.

mod common;

use std::fs;
use std::process::Command;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use common::setup_polyglot_git_repo;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_callisto")
}

fn git(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn git {args:?}: {e}"))
}

fn git_ok(root: &std::path::Path, args: &[&str]) -> String {
    let out = git(root, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn commit_plan_reproduces_git_write_tree() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();
    let base_sha = git_ok(root, &["rev-parse", "HEAD"]);

    // Stage an addition, a modification, a deletion, and a CRLF file.
    fs::write(root.join("NEW_FILE.txt"), b"brand new\n").unwrap();
    fs::write(
        root.join("crates/core/src/lib.rs"),
        b"pub fn hello() {\n    println!(\"hi\");\n}\n",
    )
    .unwrap();
    fs::remove_file(root.join("packages/web/package.json")).unwrap();
    fs::write(root.join("crlf.txt"), b"line one\r\nline two\r\n").unwrap();
    let out = git(root, &["add", "-A"]);
    assert!(out.status.success());

    let expected_tree = git_ok(root, &["write-tree"]);

    let out = Command::new(bin())
        .args([
            "--cwd",
            root.to_str().unwrap(),
            "--format",
            "json",
            "release-pr",
            "commit-plan",
            "--base-commit",
            &base_sha,
            "--message",
            "test commit",
        ])
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn callisto release-pr commit-plan: {e}"));
    assert!(
        out.status.success(),
        "commit-plan failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();

    // Reconstruct the base tree into a standalone directory via `git
    // archive`, apply the plan's additions/deletions, and compare trees --
    // this never touches the original repo's own (already-staged) index.
    let recon = tempfile::tempdir().unwrap();
    let archive_out = git(root, &["archive", &base_sha]);
    assert!(archive_out.status.success());
    let tar_status = {
        let mut child = Command::new("tar")
            .args(["-x", "-C", recon.path().to_str().unwrap()])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child.stdin.take().unwrap().write_all(&archive_out.stdout).unwrap();
        child.wait().unwrap()
    };
    assert!(tar_status.success());
    git_ok(recon.path(), &["init", "-q", "-b", "main"]);
    git(recon.path(), &["config", "user.name", "Recon"]);
    git(recon.path(), &["config", "user.email", "recon@callisto.dev"]);

    for addition in plan["additions"].as_array().unwrap() {
        let path = addition["path"].as_str().unwrap();
        let contents = BASE64.decode(addition["contentsBase64"].as_str().unwrap()).unwrap();
        let full = recon.path().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, contents).unwrap();
    }
    for deletion in plan["deletions"].as_array().unwrap() {
        let path = deletion["path"].as_str().unwrap();
        let full = recon.path().join(path);
        if full.exists() {
            fs::remove_file(full).unwrap();
        }
    }
    git(recon.path(), &["add", "-A"]);
    let reconstructed_tree = git_ok(recon.path(), &["write-tree"]);

    assert_eq!(
        reconstructed_tree, expected_tree,
        "applying the commit plan onto base_commit must reproduce the exact staged tree"
    );

    let additions: std::collections::BTreeSet<&str> = plan["additions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["path"].as_str().unwrap())
        .collect();
    assert!(additions.contains("NEW_FILE.txt"));
    assert!(additions.contains("crates/core/src/lib.rs"));
    assert!(additions.contains("crlf.txt"));
    let deletions: std::collections::BTreeSet<&str> = plan["deletions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["path"].as_str().unwrap())
        .collect();
    assert!(deletions.contains("packages/web/package.json"));
}

#[test]
fn commit_plan_fails_closed_on_executable_bit() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();
    let base_sha = git_ok(root, &["rev-parse", "HEAD"]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = root.join("script.sh");
        fs::write(&script, b"#!/bin/sh\necho hi\n").unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
    }
    #[cfg(not(unix))]
    {
        fs::write(root.join("script.sh"), b"#!/bin/sh\necho hi\n").unwrap();
    }
    let out = git(root, &["add", "-A"]);
    assert!(out.status.success());

    let out = Command::new(bin())
        .args([
            "--cwd",
            root.to_str().unwrap(),
            "--format",
            "json",
            "release-pr",
            "commit-plan",
            "--base-commit",
            &base_sha,
            "--message",
            "test commit",
        ])
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn callisto release-pr commit-plan: {e}"));

    #[cfg(unix)]
    {
        assert!(
            !out.status.success(),
            "an executable-bit addition must be refused, not silently downgraded"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("E150") || stderr.to_lowercase().contains("mode"),
            "expected an UnsupportedFileMode-shaped error, got: {stderr}"
        );
    }
}
