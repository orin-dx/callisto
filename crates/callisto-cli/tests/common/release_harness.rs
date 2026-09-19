//! Shared black-box release harness: real git fixtures plus fake `cargo`,
//! `gh` and `git` programs installed on PATH.
//!
//! The legacy fakes (`fake_publishers`) are deliberately permissive and are
//! kept for the durable-release tests that depend on them. The `Rig` fakes
//! model the real tools (see `Realism`) and are opt-in per defect.
#![cfg(unix)]
#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use tempfile::TempDir;

pub fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git").args(args).current_dir(root).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

pub fn callisto(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--format", "json", "--cwd", root.to_str().unwrap()])
        .args(args)
        .output()
        .expect("callisto binary should be invocable")
}

pub const DECISION_PATH: &str = ".callisto/release-decision.json";

pub fn system_git() -> PathBuf {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join("git"))
        .find(|candidate| {
            candidate.is_file()
                && Command::new(candidate)
                    .arg("--version")
                    .output()
                    .is_ok_and(|output| output.status.success())
        })
        .expect("a real Git executable must be available before installing the fixture PATH")
}

/// Builds the exact shape a merged release PR must have: its parent contains
/// a pending changeset, while its head changes the manifest and changelog and
/// removes that changeset.
pub fn release_commit_fixture() -> (TempDir, String) {
    release_commit_fixture_with_product_release(false)
}

pub fn product_release_commit_fixture() -> (TempDir, String) {
    release_commit_fixture_with_product_release(true)
}

pub fn release_commit_fixture_with_product_release(product_release: bool) -> (TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Callisto Test"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "tag.gpgsign", "false"]);
    git(
        root,
        &["remote", "add", "origin", "https://github.com/example/core-crate.git"],
    );

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
    fs::write(root.join("crates/core/src/lib.rs"), "pub fn core() {}\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial workspace"]);

    let init = callisto(root, &["init", "--yes"]);
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let config_path = root.join("callisto.toml");
    let config = fs::read_to_string(&config_path).unwrap();
    let product_config = product_release.then_some(
        "\n[release]\nproduct-package = \"cargo/core-crate\"\nartifact-targets = [\n  \"aarch64-apple-darwin\",\n  \"x86_64-unknown-linux-gnu\",\n  \"x86_64-unknown-linux-musl\",\n  \"wasm32-wasip1\",\n]\n\n[release.profiles.production]\nforge-repository = \"example/core-crate\"\nregistry-routes = { cratesIo = \"cratesIo\" }\n",
    );
    let tag_template = product_release.then_some("tag-template = \"callisto@{version}\"\n");
    fs::write(
        config_path,
        format!(
            "{config}{product_config}\n[[package]]\nmatch = \"cargo/core-crate\"\npublish-to = [\"crates-io\", \"github-release\"]\n{tag_template}",
            product_config = product_config.unwrap_or_default(),
            tag_template = tag_template.unwrap_or_default(),
        ),
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "configure callisto"]);

    let add = callisto(
        root,
        &[
            "add",
            "--package",
            "core-crate:minor",
            "--summary",
            "Ship durable release execution",
        ],
    );
    assert!(
        add.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "add release changeset"]);

    let version = callisto(root, &["version", "--emit-decision", DECISION_PATH]);
    assert!(
        version.status.success(),
        "version failed: {}",
        String::from_utf8_lossy(&version.stderr)
    );

    // `version` writes the reviewed manifest and changelog edits.  The release
    // PR, rather than an intermediate local command, consumes its exact
    // changeset when that PR is merged.  Model that merge boundary explicitly:
    // the parent contains the authority and the merge commit deletes it.
    for entry in fs::read_dir(root.join(".changeset")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|name| name != "README.md")
            && path.extension().is_some_and(|extension| extension == "md")
        {
            fs::remove_file(path).unwrap();
        }
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "release core-crate 0.2.0"]);
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    // GitHub Actions checks out the merge commit detached.  Git-commit trust
    // deliberately requires that topology, rather than accepting a mutable
    // branch ref as release authority.
    git(root, &["checkout", "--detach", &release_commit]);
    (dir, release_commit)
}

/// Two packages in a `[[fixed-group]]`, one bumped by a changeset naming only
/// itself. This is the exact shape the prior re-derivation approach rejected:
/// its sibling's version changes with no changeset of its own naming it.
/// Both packages are seeded with a prior tag so the fixed-group target isn't
/// the `0.0.0` fallback a group with no prior release ever hits in practice.
pub fn fixed_group_release_commit_fixture() -> (TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Callisto Test"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "tag.gpgsign", "false"]);
    git(
        root,
        &["remote", "add", "origin", "https://github.com/example/fixed-group.git"],
    );

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/crate-a\", \"crates/crate-b\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    for name in ["crate-a", "crate-b"] {
        fs::create_dir_all(root.join(format!("crates/{name}/src"))).unwrap();
        fs::write(
            root.join(format!("crates/{name}/Cargo.toml")),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        fs::write(root.join(format!("crates/{name}/src/lib.rs")), "\n").unwrap();
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial workspace"]);
    git(root, &["tag", "crate-a@0.1.0"]);
    git(root, &["tag", "crate-b@0.1.0"]);

    let init = callisto(root, &["init", "--yes"]);
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let config_path = root.join("callisto.toml");
    let config = fs::read_to_string(&config_path).unwrap();
    fs::write(
        config_path,
        format!(
            "{config}\n\
             [[package]]\nmatch = \"cargo/crate-a\"\npublish-to = [\"crates-io\"]\n\n\
             [[package]]\nmatch = \"cargo/crate-b\"\npublish-to = [\"crates-io\"]\n\n\
             [[fixed-group]]\nname = \"demo\"\nmembers = [\"crate-a\", \"crate-b\"]\n"
        ),
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "configure callisto"]);

    let add = callisto(
        root,
        &["add", "--package", "crate-a:patch", "--summary", "Fix crate-a only"],
    );
    assert!(
        add.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "add release changeset"]);

    let version = callisto(root, &["version", "--emit-decision", DECISION_PATH]);
    assert!(
        version.status.success(),
        "version failed: {}",
        String::from_utf8_lossy(&version.stderr)
    );
    let crate_b_manifest = fs::read_to_string(root.join("crates/crate-b/Cargo.toml")).unwrap();
    assert!(
        crate_b_manifest.contains("0.1.1"),
        "crate-b must cascade to the fixed group's target version even though only \
         crate-a's changeset named it directly; manifest was:\n{crate_b_manifest}"
    );

    for entry in fs::read_dir(root.join(".changeset")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|name| name != "README.md")
            && path.extension().is_some_and(|extension| extension == "md")
        {
            fs::remove_file(path).unwrap();
        }
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "release fixed group 0.1.1"]);
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &release_commit]);
    (dir, release_commit)
}

pub fn plan_intent(root: &Path, external: &Path, release_commit: &str) -> std::path::PathBuf {
    let intent = external.join("release-intent.json");
    let plan = callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            release_commit,
            "--decision",
            DECISION_PATH,
            "--out",
            intent.to_str().unwrap(),
        ],
    );
    assert!(
        plan.status.success(),
        "release plan failed: {}\nrelease-commit delta:\n{}",
        String::from_utf8_lossy(&plan.stderr),
        git(
            root,
            &["diff-tree", "--no-commit-id", "--name-status", "-r", "HEAD^", "HEAD",],
        )
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&intent).unwrap()).unwrap();
    assert_eq!(value["decision"]["entries"][0]["package"], "cargo/core-crate");
    assert_eq!(value["decision"]["entries"][0]["targetVersion"], "0.2.0");
    assert!(value["operations"]
        .as_array()
        .is_some_and(|operations| operations.len() >= 3));
    intent
}

pub fn coordinator_checkout(source: &Path, release_commit: &str) -> (TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let coordinator = dir.path();
    git(
        source,
        &[
            "clone",
            "--no-checkout",
            source.to_str().unwrap(),
            coordinator.to_str().unwrap(),
        ],
    );
    git(coordinator, &["config", "user.name", "Callisto Coordinator"]);
    git(coordinator, &["config", "user.email", "coordinator@example.invalid"]);
    git(coordinator, &["config", "commit.gpgsign", "false"]);
    git(coordinator, &["checkout", "--detach", release_commit]);
    fs::write(coordinator.join("COORDINATOR-REVISION"), "new coordinator\n").unwrap();
    git(coordinator, &["add", "COORDINATOR-REVISION"]);
    git(coordinator, &["commit", "-m", "advance coordinator"]);
    let revision = git(coordinator, &["rev-parse", "HEAD"]);
    assert_ne!(
        revision, release_commit,
        "the coordinator must differ from release source"
    );
    (dir, revision)
}

pub fn plan_intent_from_source(
    coordinator: &Path,
    source: &Path,
    external: &Path,
    release_commit: &str,
) -> std::path::PathBuf {
    let intent = external.join("release-intent.json");
    let plan = callisto(
        coordinator,
        &[
            "release",
            "plan",
            "--source-root",
            source.to_str().unwrap(),
            "--from-release-commit",
            release_commit,
            "--decision",
            source.join(DECISION_PATH).to_str().unwrap(),
            "--out",
            intent.to_str().unwrap(),
        ],
    );
    assert!(
        plan.status.success(),
        "cross-worktree release plan failed: {}",
        String::from_utf8_lossy(&plan.stderr)
    );
    intent
}

pub fn plan_product_intent(root: &Path, external: &Path, release_commit: &str) -> std::path::PathBuf {
    let intent = external.join("release-intent.json");
    let plan = callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            release_commit,
            "--decision",
            DECISION_PATH,
            "--orchestration-revision",
            release_commit,
            "--artifact-repository",
            "example/core-crate",
            "--out",
            intent.to_str().unwrap(),
        ],
    );
    assert!(
        plan.status.success(),
        "product release plan failed: {}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&intent).unwrap()).unwrap();
    assert_eq!(value["artifactSlots"].as_array().map(Vec::len), Some(4));
    intent
}
pub const PRODUCT_ASSETS: [&str; 4] = [
    "callisto-aarch64-apple-darwin.tar.gz",
    "callisto-x86_64-unknown-linux-gnu.tar.gz",
    "callisto-x86_64-unknown-linux-musl.tar.gz",
    "callisto-moon.wasm",
];

pub fn create_product_artifacts(external: &Path) -> std::path::PathBuf {
    let artifacts = external.join("release-artifacts");
    fs::create_dir_all(&artifacts).unwrap();
    for asset in PRODUCT_ASSETS {
        fs::write(artifacts.join(asset), format!("fixture artifact: {asset}\n")).unwrap();
    }
    // Artifact downloads can contain framework metadata or nested archive
    // paths. Only the immutable manifest's direct asset names may reach the
    // provider adapter; these fixtures must remain inert.
    fs::write(artifacts.join(".hidden-metadata"), "must not upload\n").unwrap();
    fs::create_dir_all(artifacts.join("nested")).unwrap();
    fs::write(artifacts.join("nested/unlisted-artifact"), "must not upload\n").unwrap();
    artifacts
}

pub fn fake_publishers(
    external: &Path,
    release_commit: &str,
    fail_cargo_publish: bool,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    use std::os::unix::fs::PermissionsExt;

    let bin = external.join("fake-bin");
    fs::create_dir_all(&bin).unwrap();
    let log = external.join("external-effects.log");
    let git_trace = external.join("git-commands.log");
    let forge_marker = external.join("forge-release-created");
    let cargo_publish = if fail_cargo_publish {
        "exit 23"
    } else {
        ": > \"$CALLISTO_TEST_CARGO_MARKER\"\nexit 0"
    };
    fs::write(
        bin.join("cargo"),
        format!(
            "#!/bin/sh\nprintf 'cargo %s\\n' \"$*\" >> \"$CALLISTO_TEST_LOG\"\nif [ \"$1\" = info ]; then\n  if [ -f \"$CALLISTO_TEST_CARGO_MARKER\" ]; then\n    exit 0\n  fi\n  printf 'could not find crate\\n' >&2\n  exit 101\nfi\nif [ \"$1\" = publish ]; then\n  {cargo_publish}\nfi\nexit 0\n"
        ),
    )
    .unwrap();
    fs::write(
        bin.join("git"),
        format!(
            "#!/bin/sh\nprintf 'git %s\\n' \"$*\" >> \"$CALLISTO_TEST_GIT_TRACE\"\n{FAKE_GIT_REMOTE_TAGS}\nif [ \"$1\" = push ]; then\n  printf 'git %s\\n' \"$*\" >> \"$CALLISTO_TEST_LOG\"\n  record_pushed_tag \"$3\"\n  exit 0\nfi\nexec \"$CALLISTO_TEST_REAL_GIT\" \"$@\"\n"
        ),
    )
    .unwrap();
    fs::write(
        bin.join("gh"),
        format!(
            "#!/bin/sh\nprintf 'gh %s\\n' \"$*\" >> \"$CALLISTO_TEST_LOG\"\nif [ \"$1\" = attestation ] && [ \"$2\" = verify ]; then\n  exit 0\nfi\nif [ \"$1\" = api ]; then\n  if [ -f \"$CALLISTO_TEST_FORGE_MARKER\" ]; then\n    assets=''\n    comma=''\n    if [ -f \"$CALLISTO_TEST_ARTIFACT_MARKER\" ]; then\n      while IFS='|' read -r asset size digest; do\n        assets=\"${{assets}}${{comma}}{{\\\"name\\\":\\\"${{asset}}\\\",\\\"size\\\":${{size}},\\\"digest\\\":\\\"sha256:${{digest}}\\\"}}\"\n        comma=','\n      done < \"$CALLISTO_TEST_ARTIFACT_MARKER\"\n    fi\n    printf '%s\\n\\n%s\\n' 'HTTP/1.1 200 OK' \"{{\\\"tag_name\\\":\\\"$CALLISTO_TEST_FORGE_TAG\\\",\\\"target_commitish\\\":\\\"{release_commit}\\\",\\\"assets\\\":[${{assets}}]}}\"\n  else\n    printf '%s\\n\\n%s\\n' 'HTTP/1.1 404 Not Found' '{{}}'\n  fi\n  exit 0\nfi\nif [ \"$1\" = release ] && [ \"$2\" = create ]; then\n  : > \"$CALLISTO_TEST_FORGE_MARKER\"\nfi\nif [ \"$1\" = release ] && [ \"$2\" = upload ]; then\n  asset=$4\n  name=$(basename \"$asset\")\n  size=$(wc -c < \"$asset\" | tr -d ' ')\n  digest=$(shasum -a 256 \"$asset\" | awk '{{print $1}}')\n  printf '%s|%s|%s\\n' \"$name\" \"$size\" \"$digest\" >> \"$CALLISTO_TEST_ARTIFACT_MARKER\"\nfi\nexit 0\n"
        ),
    )
    .unwrap();
    for program in [bin.join("cargo"), bin.join("git"), bin.join("gh")] {
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    }
    (bin, log, forge_marker, git_trace)
}

pub fn execute(
    root: &Path,
    intent: &Path,
    state: &Path,
    bin: &Path,
    log: &Path,
    forge_marker: &Path,
    git_trace: &Path,
) -> Output {
    execute_with_recovery(
        root,
        intent,
        state,
        FakePublishers {
            bin,
            log,
            forge_marker,
            git_trace,
        },
        false,
    )
}

#[derive(Clone, Copy)]
pub struct FakePublishers<'a> {
    pub bin: &'a Path,
    pub log: &'a Path,
    pub forge_marker: &'a Path,
    pub git_trace: &'a Path,
}

pub fn execute_with_recovery(
    root: &Path,
    intent: &Path,
    state: &Path,
    publishers: FakePublishers<'_>,
    recovery: bool,
) -> Output {
    let path = format!("{}:{}", publishers.bin.display(), std::env::var("PATH").unwrap());
    let mut command = Command::new(env!("CARGO_BIN_EXE_callisto"));
    command
        .args(["--format", "json", "--cwd", root.to_str().unwrap()])
        .args([
            "release",
            "execute",
            "--intent",
            intent.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
            "--receipt",
            state.with_extension("receipt.json").to_str().unwrap(),
            "--orchestration-revision",
            &git(root, &["rev-parse", "HEAD"]),
        ]);
    if recovery {
        command.arg("--recovery");
    }
    command
        .env("PATH", path)
        .env("CALLISTO_TEST_LOG", publishers.log)
        .env("CALLISTO_TEST_GIT_TRACE", publishers.git_trace)
        .env("CALLISTO_TEST_FORGE_MARKER", publishers.forge_marker)
        .env(
            "CALLISTO_TEST_ARTIFACT_MARKER",
            publishers.log.with_extension("artifact-marker"),
        )
        .env("CALLISTO_TEST_FORGE_TAG", "core-crate@0.2.0")
        .env("CALLISTO_TEST_CARGO_MARKER", state.with_extension("cargo-marker"))
        .env("CALLISTO_TEST_REAL_GIT", system_git())
        .output()
        .expect("release execute should run")
}

pub fn execute_from_coordinator(
    coordinator: &Path,
    source: &Path,
    intent: &Path,
    state: &Path,
    publishers: FakePublishers<'_>,
    recovery: bool,
) -> Output {
    let path = format!("{}:{}", publishers.bin.display(), std::env::var("PATH").unwrap());
    let mut command = Command::new(env!("CARGO_BIN_EXE_callisto"));
    command
        .args(["--format", "json", "--cwd", coordinator.to_str().unwrap()])
        .args([
            "release",
            "execute",
            "--source-root",
            source.to_str().unwrap(),
            "--intent",
            intent.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
            "--receipt",
            state.with_extension("receipt.json").to_str().unwrap(),
            "--orchestration-revision",
            &git(coordinator, &["rev-parse", "HEAD"]),
        ]);
    if recovery {
        command.arg("--recovery");
    }
    command
        .env("PATH", path)
        .env("CALLISTO_TEST_LOG", publishers.log)
        .env("CALLISTO_TEST_GIT_TRACE", publishers.git_trace)
        .env("CALLISTO_TEST_FORGE_MARKER", publishers.forge_marker)
        .env(
            "CALLISTO_TEST_ARTIFACT_MARKER",
            publishers.log.with_extension("artifact-marker"),
        )
        .env("CALLISTO_TEST_FORGE_TAG", "core-crate@0.2.0")
        .env("CALLISTO_TEST_CARGO_MARKER", state.with_extension("cargo-marker"))
        .env("CALLISTO_TEST_REAL_GIT", system_git())
        .output()
        .expect("cross-worktree release execute should run")
}

pub fn execute_product(
    root: &Path,
    intent: &Path,
    manifest: &Path,
    artifacts: &Path,
    state: &Path,
    publishers: FakePublishers<'_>,
    recovery: bool,
) -> Output {
    let path = format!("{}:{}", publishers.bin.display(), std::env::var("PATH").unwrap());
    let mut command = Command::new(env!("CARGO_BIN_EXE_callisto"));
    command
        .args(["--format", "json", "--cwd", root.to_str().unwrap()])
        .args([
            "release",
            "execute",
            "--intent",
            intent.to_str().unwrap(),
            "--artifact-manifest",
            manifest.to_str().unwrap(),
            "--artifact-dir",
            artifacts.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
            "--receipt",
            state.with_extension("receipt.json").to_str().unwrap(),
            "--orchestration-revision",
            &git(root, &["rev-parse", "HEAD"]),
        ]);
    if recovery {
        command.arg("--recovery");
    }
    command
        .env("PATH", path)
        .env("CALLISTO_TEST_LOG", publishers.log)
        .env("CALLISTO_TEST_GIT_TRACE", publishers.git_trace)
        .env("CALLISTO_TEST_FORGE_MARKER", publishers.forge_marker)
        .env(
            "CALLISTO_TEST_ARTIFACT_MARKER",
            publishers.log.with_extension("artifact-marker"),
        )
        .env("CALLISTO_TEST_FORGE_TAG", "callisto@0.2.0")
        .env("CALLISTO_TEST_CARGO_MARKER", state.with_extension("cargo-marker"))
        .env("CALLISTO_TEST_REAL_GIT", system_git())
        .output()
        .expect("product release execute should run")
}

// ---------------------------------------------------------------------
// Rig: fake providers that model the real tools, one opt-in knob per
// audited defect. Every knob defaults to the permissive legacy behaviour so a
// red test isolates exactly the realism it needs.
// ---------------------------------------------------------------------

const RIG_CARGO: &str = r#"#!/bin/sh
printf 'cargo %s\n' "$*" >> "$CALLISTO_TEST_LOG"
printf '%s|%s\n' "$PWD" "$*" >> "$CALLISTO_TEST_CARGO_CALLS"
if [ "$1" = info ]; then
  if [ "$CALLISTO_TEST_CARGO_INFO_FAIL" = 1 ]; then
    printf 'error: network unreachable\n' >&2
    exit 1
  fi
  name=${2%@*}
  registry=''
  prev=''
  for a in "$@"; do
    if [ "$prev" = --registry ]; then registry=$a; fi
    prev=$a
  done
  if [ "$CALLISTO_TEST_CARGO_LOCAL" = 1 ] && [ -z "$registry" ]; then
    dir=$PWD
    while [ "$dir" != / ]; do
      if [ -f "$dir/Cargo.toml" ]; then
        if grep -rqs --include=Cargo.toml "^name = \"$name\"" "$dir"; then
          printf '%s v0.0.0 (from ./crates/%s)\n' "$name" "$name"
          exit 0
        fi
      fi
      dir=$(dirname "$dir")
    done
  fi
  if [ -f "$CALLISTO_TEST_CARGO_MARKER" ] || [ -f "$CALLISTO_TEST_CARGO_MARKER.$name" ]; then
    printf '%s\n' "$name"
    exit 0
  fi
  printf 'could not find crate\n' >&2
  exit 101
fi
if [ "$1" = publish ]; then
  manifest=''
  prev=''
  for a in "$@"; do
    if [ "$prev" = --manifest-path ]; then manifest=$a; fi
    prev=$a
  done
  name=$(grep -m1 '^name = ' "$manifest" | cut -d'"' -f2)
  if [ "$CALLISTO_TEST_CARGO_TARGET" = 1 ]; then
    mkdir -p "$PWD/target/package"
    : > "$PWD/target/package/x"
  fi
  if [ -n "$CALLISTO_TEST_CARGO_PUBLISH_EXIT" ]; then
    printf '%s\n' "$CALLISTO_TEST_CARGO_PUBLISH_STDERR" >&2
    exit "$CALLISTO_TEST_CARGO_PUBLISH_EXIT"
  fi
  : > "$CALLISTO_TEST_CARGO_MARKER.$name"
  exit 0
fi
exit 0
"#;

const RIG_GH: &str = r#"#!/bin/sh
printf 'gh %s\n' "$*" >> "$CALLISTO_TEST_LOG"
printf '%s\n' "$*" >> "$CALLISTO_TEST_GH_CALLS"
reject() {
  printf 'unknown flag: %s\n' "$1" >&2
  exit 1
}
if [ "$CALLISTO_TEST_GH_STRICT" = 1 ]; then
  cmd=$1
  first=1
  for a in "$@"; do
    if [ "$first" = 1 ]; then first=0; continue; fi
    case "$a" in
      -*)
        case "$cmd:$a" in
          api:--include|api:-i|api:--method|api:-X|api:--header|api:-H|api:--field|api:-F|api:--raw-field|api:-f|api:--jq|api:-q|api:--paginate|api:--silent|api:--slurp|api:--hostname|api:--input|api:--cache|api:--template|api:-t|api:--verbose) ;;
          release:--repo|release:-R|release:--verify-tag|release:--generate-notes|release:-g|release:--target|release:--title|release:-t|release:--notes|release:-n|release:--notes-file|release:-F|release:--draft|release:-d|release:--prerelease|release:-p|release:--latest|release:--clobber|release:--discussion-category|release:--notes-start-tag|release:--fail-on-no-commits) ;;
          api:*|release:*) reject "$a" ;;
        esac
        ;;
    esac
  done
fi
if [ "$1" = attestation ] && [ "$2" = verify ]; then
  exit 0
fi
if [ "$1" = api ]; then
  if [ -f "$CALLISTO_TEST_FORGE_MARKER" ]; then
    assets=''
    comma=''
    if [ -f "$CALLISTO_TEST_ARTIFACT_MARKER" ]; then
      while IFS='|' read -r asset size digest; do
        assets="${assets}${comma}{\"name\":\"${asset}\",\"size\":${size},\"digest\":\"sha256:${digest}\"}"
        comma=','
      done < "$CALLISTO_TEST_ARTIFACT_MARKER"
    fi
    printf '%s\n\n%s\n' 'HTTP/1.1 200 OK' "{\"tag_name\":\"$CALLISTO_TEST_FORGE_TAG\",\"target_commitish\":\"$CALLISTO_TEST_FORGE_COMMITISH\",\"assets\":[${assets}]}"
  else
    printf '%s\n\n%s\n' 'HTTP/1.1 404 Not Found' '{}'
  fi
  exit 0
fi
if [ "$1" = release ] && [ "$2" = create ]; then
  : > "$CALLISTO_TEST_FORGE_MARKER"
fi
if [ "$1" = release ] && [ "$2" = upload ]; then
  asset=$4
  name=$(basename "$asset")
  size=$(wc -c < "$asset" | tr -d ' ')
  digest=$(shasum -a 256 "$asset" | awk '{print $1}')
  printf '%s|%s|%s\n' "$name" "$size" "$digest" >> "$CALLISTO_TEST_ARTIFACT_MARKER"
fi
exit 0
"#;

/// Fake remote tag storage shared by both fake `git` programs: `git push`
/// records the pushed tag exactly as `ls-remote` would report it (the tag
/// object plus its peeled commit), and `ls-remote` answers from that record.
/// Real git is used instead wherever the rig points at a real bare remote.
const FAKE_GIT_REMOTE_TAGS: &str = r#"
store="$CALLISTO_TEST_GIT_TRACE.remote-tags"
tab=$(printf '\t')
record_pushed_tag() {
  obj=$("$CALLISTO_TEST_REAL_GIT" rev-parse "refs/tags/$1" 2>/dev/null) || return 0
  commit=$("$CALLISTO_TEST_REAL_GIT" rev-parse "refs/tags/$1^{commit}" 2>/dev/null) || return 0
  printf '%s%srefs/tags/%s\n' "$obj" "$tab" "$1" >> "$store"
  printf '%s%srefs/tags/%s^{}\n' "$commit" "$tab" "$1" >> "$store"
}
if [ "$1" = ls-remote ]; then
  shift
  shift
  [ -f "$store" ] || exit 0
  for ref in "$@"; do
    while IFS= read -r line; do
      case "$line" in *"$tab$ref") printf '%s\n' "$line";; esac
    done < "$store"
  done
  exit 0
fi
"#;

const RIG_GIT: &str = r#"#!/bin/sh
printf 'git %s\n' "$*" >> "$CALLISTO_TEST_GIT_TRACE"
if [ "$CALLISTO_TEST_PUSH" = real ]; then
  if [ "$1" = push ]; then printf 'git %s\n' "$*" >> "$CALLISTO_TEST_LOG"; fi
  n=$#
  i=0
  while [ "$i" -lt "$n" ]; do
    a=$1
    shift
    if [ "$a" = "$CALLISTO_TEST_REMOTE_URL" ]; then a=$CALLISTO_TEST_BARE_REMOTE; fi
    set -- "$@" "$a"
    i=$((i + 1))
  done
  exec "$CALLISTO_TEST_REAL_GIT" "$@"
fi
"#;

const RIG_GIT_TAIL: &str = r#"
if [ "$1" = push ]; then
  printf 'git %s\n' "$*" >> "$CALLISTO_TEST_LOG"
  if [ "$CALLISTO_TEST_PUSH" = fail ]; then
    printf 'fatal: unable to access remote\n' >&2
    exit 1
  fi
  record_pushed_tag "$3"
  exit 0
fi
exec "$CALLISTO_TEST_REAL_GIT" "$@"
"#;

fn rig_git() -> String {
    format!("{RIG_GIT}{FAKE_GIT_REMOTE_TAGS}{RIG_GIT_TAIL}")
}

/// Fake `cargo`/`gh`/`git` on a private PATH directory plus the env that
/// steers them.
pub struct Rig {
    pub bin: PathBuf,
    pub log: PathBuf,
    pub forge_marker: PathBuf,
    pub git_trace: PathBuf,
    pub cargo_calls: PathBuf,
    pub gh_calls: PathBuf,
    env: Vec<(String, String)>,
}

impl Rig {
    pub fn new(external: &Path, release_commit: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let bin = external.join("rig-bin");
        fs::create_dir_all(&bin).unwrap();
        for (name, body) in [
            ("cargo", RIG_CARGO.to_owned()),
            ("gh", RIG_GH.to_owned()),
            ("git", rig_git()),
        ] {
            fs::write(bin.join(name), body).unwrap();
            fs::set_permissions(bin.join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut rig = Self {
            log: external.join("external-effects.log"),
            forge_marker: external.join("forge-release-created"),
            git_trace: external.join("git-commands.log"),
            cargo_calls: external.join("cargo-calls.log"),
            gh_calls: external.join("gh-calls.log"),
            bin,
            env: Vec::new(),
        };
        rig.set("CALLISTO_TEST_FORGE_COMMITISH", release_commit);
        rig
    }

    pub fn set(&mut self, key: &str, value: &str) -> &mut Self {
        self.env.retain(|(existing, _)| existing != key);
        self.env.push((key.to_owned(), value.to_owned()));
        self
    }

    /// `cargo info` resolves a workspace-local manifest when run inside a workspace without `--registry`.
    pub fn real_cargo_info(&mut self) -> &mut Self {
        self.set("CALLISTO_TEST_CARGO_LOCAL", "1")
    }
    /// `cargo publish` leaves `target/package/` in its working directory.
    pub fn real_cargo_target_dir(&mut self) -> &mut Self {
        self.set("CALLISTO_TEST_CARGO_TARGET", "1")
    }
    /// `gh api` and `gh release` reject flags the real subcommands do not define.
    pub fn strict_gh(&mut self) -> &mut Self {
        self.set("CALLISTO_TEST_GH_STRICT", "1")
    }
    /// A release created without `--target` reports the default branch as `target_commitish`.
    pub fn forge_default_branch(&mut self, branch: &str) -> &mut Self {
        self.set("CALLISTO_TEST_FORGE_COMMITISH", branch)
    }
    /// `git push`/`ls-remote` are real git against a bare `file://` remote that replaces `remote_url` in their arguments.
    pub fn real_git_push(&mut self, bare: &Path, remote_url: &str) -> &mut Self {
        self.set("CALLISTO_TEST_PUSH", "real")
            .set("CALLISTO_TEST_BARE_REMOTE", &format!("file://{}", bare.display()))
            .set("CALLISTO_TEST_REMOTE_URL", remote_url)
    }
    pub fn failing_git_push(&mut self) -> &mut Self {
        self.set("CALLISTO_TEST_PUSH", "fail")
    }

    pub fn log_count(&self, needle: &str) -> usize {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .matches(needle)
            .count()
    }
    pub fn cargo_calls(&self) -> Vec<(String, String)> {
        fs::read_to_string(&self.cargo_calls)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.split_once('|'))
            .map(|(cwd, argv)| (cwd.to_owned(), argv.to_owned()))
            .collect()
    }
    pub fn gh_calls(&self) -> Vec<String> {
        fs::read_to_string(&self.gh_calls)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

pub fn bare_remote(dir: &Path) -> PathBuf {
    let bare = dir.join("remote.git");
    let output = Command::new(system_git())
        .args(["init", "--bare", "-b", "main"])
        .arg(&bare)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    bare
}

/// Runs `callisto release execute` under the rig; `extra` are appended CLI arguments.
pub fn execute_rig(
    root: &Path,
    intent: &Path,
    state: &Path,
    rig: &Rig,
    forge_tag: &str,
    extra: &[&str],
    orchestration: Option<&str>,
) -> Output {
    let path = format!("{}:{}", rig.bin.display(), std::env::var("PATH").unwrap());
    let head = git(root, &["rev-parse", "HEAD"]);
    let mut command = Command::new(env!("CARGO_BIN_EXE_callisto"));
    command
        .args(["--format", "json", "--cwd", root.to_str().unwrap()])
        .args([
            "release",
            "execute",
            "--intent",
            intent.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
            "--receipt",
            state.with_extension("receipt.json").to_str().unwrap(),
            "--orchestration-revision",
            orchestration.unwrap_or(&head),
        ])
        .args(extra)
        .env("PATH", path)
        .env("CALLISTO_TEST_LOG", &rig.log)
        .env("CALLISTO_TEST_GIT_TRACE", &rig.git_trace)
        .env("CALLISTO_TEST_CARGO_CALLS", &rig.cargo_calls)
        .env("CALLISTO_TEST_GH_CALLS", &rig.gh_calls)
        .env("CALLISTO_TEST_FORGE_MARKER", &rig.forge_marker)
        .env(
            "CALLISTO_TEST_ARTIFACT_MARKER",
            rig.log.with_extension("artifact-marker"),
        )
        .env("CALLISTO_TEST_FORGE_TAG", forge_tag)
        .env("CALLISTO_TEST_CARGO_MARKER", state.with_extension("cargo-marker"))
        .env("CALLISTO_TEST_REAL_GIT", system_git());
    for (key, value) in &rig.env {
        command.env(key, value);
    }
    command.output().expect("release execute should run")
}

pub fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Every diagnostic `code` found in the JSON documents callisto printed on stdout or stderr.
pub fn diagnostic_codes(output: &Output) -> Vec<String> {
    let mut codes = Vec::new();
    for stream in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(stream);
        for line in text.lines() {
            collect_codes(line, &mut codes);
        }
        collect_codes(&text, &mut codes);
    }
    codes.sort();
    codes.dedup();
    codes
}

fn collect_codes(text: &str, codes: &mut Vec<String>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    fn walk(value: &serde_json::Value, codes: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, inner) in map {
                    if key == "code" {
                        if let Some(code) = inner.as_str() {
                            codes.push(code.to_owned());
                        }
                    }
                    walk(inner, codes);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| walk(item, codes)),
            _ => {}
        }
    }
    walk(&value, codes);
}
