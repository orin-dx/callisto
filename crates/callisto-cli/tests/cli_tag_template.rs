//! Changing a package's `tag-template` must not silently orphan the tags the
//! previous template produced.

use std::{fs, process::Command};

use callisto_fixtures::git::run_git as git;

#[test]
fn red_c3_renamed_tag_template_still_finds_prior_tags() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/foo\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/foo/src")).unwrap();
    fs::write(
        root.join("crates/foo/Cargo.toml"),
        "[package]\nname = \"foo\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("crates/foo/src/lib.rs"), "\n").unwrap();
    fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"cargo/foo\"\ntag-template = \"bar@{version}\"\nprevious-tag-templates = [\"foo@{version}\"]\n",
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial"]);
    git(root, &["tag", "foo@1.0.0"]);

    let status = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--format", "json", "--cwd", root.to_str().unwrap(), "status"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&status.stdout).into_owned();
    assert!(
        status.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let text = value.to_string();
    let last_released = find_key(&value, "lastReleasedVersion");
    let diagnosed = text.contains("tag-template") || text.contains("tagTemplate");
    assert!(
        last_released.as_deref() == Some("1.0.0") || diagnosed,
        "lastReleasedVersion is {last_released:?} and no diagnostic reports the orphaned tags; status: {stdout}"
    );
}

fn find_key(value: &serde_json::Value, key: &str) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => map
            .get(key)
            .and_then(|found| found.as_str().map(str::to_owned))
            .or_else(|| map.values().find_map(|inner| find_key(inner, key))),
        serde_json::Value::Array(items) => items.iter().find_map(|item| find_key(item, key)),
        _ => None,
    }
}
