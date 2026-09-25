//! Shared black-box release harness: real git fixtures plus fake `cargo`,
//! `gh` and `git` programs installed on PATH.
//!
//! Every fake is strict: it accepts only the invocation shapes in `TOOL_SHAPES`
//! and answers with responses templated from the captured provider fixtures
//! under `testing/fixtures/providers/`. The same table drives the real-tool
//! flag contract (`provider_flag_contract_tests.rs`) and
//! `assert_argv_within_allowlists`.
#![cfg(unix)]
#![allow(dead_code, unused_imports)]

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{Mutex, OnceLock},
};

use tempfile::TempDir;

#[path = "../../../../testing/loopback_http.rs"]
pub mod loopback;

pub use loopback::{fixtures, LoopbackRequest, LoopbackResponse, LoopbackServer};

/// The registry state one end-to-end fixture publishes into and observes from.
///
/// Observation goes through the ecosystem package manager, so there is no
/// server here: the fake `cargo publish` records the published version in a
/// marker file and the fake `cargo info` answers from exactly that file, which
/// makes "publish then observe" one mechanism rather than two fakes agreeing by
/// accident. The knobs beside it are files too, because the fakes are separate
/// processes.
pub mod test_registry {
    use super::{BTreeMap, Mutex, OnceLock, Path, PathBuf};

    struct Registry {
        markers: Mutex<BTreeMap<String, PathBuf>>,
        tokens: Mutex<BTreeMap<PathBuf, String>>,
        state: PathBuf,
    }

    fn registry() -> &'static Registry {
        static REGISTRY: OnceLock<Registry> = OnceLock::new();
        REGISTRY.get_or_init(|| {
            let state = tempfile::Builder::new()
                .prefix("callisto-test-registry-")
                .tempdir()
                .expect("registry state directory")
                .keep();
            Registry {
                markers: Mutex::new(BTreeMap::new()),
                tokens: Mutex::new(BTreeMap::new()),
                state,
            }
        })
    }

    fn token_for(root: &Path) -> String {
        let registry = registry();
        let mut tokens = registry.tokens.lock().unwrap();
        if let Some(token) = tokens.get(root) {
            return token.clone();
        }
        let token = format!("r{}", tokens.len());
        let directory = registry.state.join(&token);
        std::fs::create_dir_all(&directory).expect("marker directory");
        registry.markers.lock().unwrap().insert(token.clone(), directory);
        tokens.insert(root.to_path_buf(), token.clone());
        token
    }

    fn directory(root: &Path) -> PathBuf {
        let token = token_for(root);
        registry().markers.lock().unwrap()[&token].clone()
    }

    /// The prefix the fake `cargo publish` writes `<prefix>.<crate>` under, and
    /// the fake `cargo info` reads back.
    pub fn registry_marker(root: &Path) -> PathBuf {
        directory(root).join("published")
    }

    /// Arms index-propagation lag for `root`'s registry: the first
    /// `honest_serves` observations of a published version answer normally, the
    /// next `absent_responses` answer as if the version were absent, and the
    /// honest answers then resume.
    ///
    /// `honest_serves` is what lets a test place the lag at a chosen stage --
    /// `0` puts the absences on the post-publish confirmation, `1` on any read
    /// after it.
    pub fn set_registry_flap(root: &Path, honest_serves: usize, absent_responses: usize) {
        std::fs::write(
            directory(root).join("flap"),
            format!("{honest_serves} {absent_responses}\n"),
        )
        .expect("flap state");
    }

    /// Makes every observation for `root`'s registry fail the way an
    /// unreachable index does, or restores the marker-backed answers.
    pub fn set_registry_unreachable(root: &Path, unreachable: bool) {
        let path = directory(root).join("unreachable");
        if unreachable {
            std::fs::write(&path, "1\n").expect("unreachable state");
        } else {
            drop(std::fs::remove_file(&path));
        }
    }
}

pub use test_registry::{registry_marker, set_registry_flap, set_registry_unreachable};

pub fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git").args(args).current_dir(root).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Skips real backoff waits in the spawned binary.
const NO_BACKOFF_SLEEP: (&str, &str) = ("__CALLISTO_TEST_SLEEP_SCALE", "0");

pub fn callisto(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--format", "json", "--cwd", root.to_str().unwrap()])
        .args(args)
        .env(NO_BACKOFF_SLEEP.0, NO_BACKOFF_SLEEP.1)
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
        "\n[release]\nproduct-package = \"cargo/core-crate\"\nforge-repository = \"example/core-crate\"\n\n[[release.artifact]]\npackage = \"cargo/core-crate\"\ntarget = \"aarch64-apple-darwin\"\nasset-name = \"callisto-aarch64-apple-darwin.tar.gz\"\n\n[[release.artifact]]\npackage = \"cargo/core-crate\"\ntarget = \"x86_64-unknown-linux-gnu\"\nasset-name = \"callisto-x86_64-unknown-linux-gnu.tar.gz\"\n\n[[release.artifact]]\npackage = \"cargo/core-crate\"\ntarget = \"x86_64-unknown-linux-musl\"\nasset-name = \"callisto-x86_64-unknown-linux-musl.tar.gz\"\n\n[[release.artifact]]\npackage = \"cargo/core-crate\"\ntarget = \"wasm32-wasip1\"\nasset-name = \"callisto-moon.wasm\"\n",
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
             [[fixed-group]]\nname = \"demo\"\nmembers = [\"crate-a\", \"crate-b\"]\n",
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
    // One cargo fake for both harnesses; a forced failure is just the same
    // publish-exit knob the rig uses, preset in the script.
    let preset = if fail_cargo_publish {
        "CALLISTO_TEST_CARGO_PUBLISH_EXIT=23\n"
    } else {
        ""
    };
    fs::write(
        bin.join("cargo"),
        rig_cargo().replacen("#!/bin/sh\n", &format!("#!/bin/sh\n{preset}"), 1),
    )
    .unwrap();
    fs::write(
        bin.join("git"),
        format!(
            "#!/bin/sh\nprintf 'git %s\\n' \"$*\" >> \"$CALLISTO_TEST_GIT_TRACE\"\n{FAKE_GIT_REMOTE_TAGS}\nif [ \"$1\" = push ]; then\n  no_flags_allowed \"$@\"\n  printf 'git %s\\n' \"$*\" >> \"$CALLISTO_TEST_LOG\"\n  record_pushed_tag \"$3\"\n  exit 0\nfi\nexec \"$CALLISTO_TEST_REAL_GIT\" \"$@\"\n"
        ),
    )
    .unwrap();
    fs::write(
        bin.join("gh"),
        rig_gh().replacen(
            "#!/bin/sh\n",
            &format!("#!/bin/sh\nCALLISTO_TEST_FORGE_COMMITISH=${{CALLISTO_TEST_FORGE_COMMITISH:-{release_commit}}}\n"),
            1,
        ),
    )
    .unwrap();
    write_fixture_templates(&bin);
    for program in [bin.join("cargo"), bin.join("git"), bin.join("gh")] {
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    }
    (bin, log, forge_marker, git_trace)
}

#[derive(Clone, Copy)]
pub struct FakePublishers<'a> {
    pub bin: &'a Path,
    pub log: &'a Path,
    pub forge_marker: &'a Path,
    pub git_trace: &'a Path,
}

pub fn execute(root: &Path, intent: &Path, receipt: &Path, publishers: FakePublishers<'_>) -> Output {
    let path = format!("{}:{}", publishers.bin.display(), std::env::var("PATH").unwrap());
    let mut command = Command::new(env!("CARGO_BIN_EXE_callisto"));
    command
        .args(["--format", "json", "--cwd", root.to_str().unwrap()])
        .args([
            "release",
            "execute",
            "--intent",
            intent.to_str().unwrap(),
            "--receipt",
            receipt.to_str().unwrap(),
            "--orchestration-revision",
            &git(root, &["rev-parse", "HEAD"]),
        ]);
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
        .env("CALLISTO_TEST_CARGO_MARKER", registry_marker(root))
        .env("CALLISTO_TEST_REAL_GIT", system_git())
        .env(NO_BACKOFF_SLEEP.0, NO_BACKOFF_SLEEP.1)
        .output()
        .expect("release execute should run")
}

/// Credential variables a local `callisto release` reads; cleared so the host cannot leak one in.
pub const CREDENTIAL_VARS: &[&str] = &[
    "CARGO_REGISTRY_TOKEN",
    "CARGO_HOME",
    "NODE_AUTH_TOKEN",
    "NPM_TOKEN",
    "TWINE_PASSWORD",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_URL",
    "XDG_STATE_HOME",
];

/// Runs bare `callisto release` (local route) against the fake publishers.
/// `home` isolates `~/.cargo`, `~/.npmrc` and the state directory; `env` sets credentials.
pub fn release_local(
    root: &Path,
    args: &[&str],
    home: &Path,
    env: &[(&str, &str)],
    publishers: FakePublishers<'_>,
) -> Output {
    let path = format!("{}:{}", publishers.bin.display(), std::env::var("PATH").unwrap());
    let mut command = Command::new(env!("CARGO_BIN_EXE_callisto"));
    command.args(["--cwd", root.to_str().unwrap()]).args(args);
    for name in CREDENTIAL_VARS {
        command.env_remove(name);
    }
    command
        .env("HOME", home)
        .envs(env.iter().copied())
        .env("PATH", path)
        .env("CALLISTO_TEST_LOG", publishers.log)
        .env("CALLISTO_TEST_GIT_TRACE", publishers.git_trace)
        .env("CALLISTO_TEST_FORGE_MARKER", publishers.forge_marker)
        .env(
            "CALLISTO_TEST_ARTIFACT_MARKER",
            publishers.log.with_extension("artifact-marker"),
        )
        .env("CALLISTO_TEST_FORGE_TAG", "core-crate@0.2.0")
        .env("CALLISTO_TEST_CARGO_MARKER", registry_marker(root))
        .env("CALLISTO_TEST_REAL_GIT", system_git())
        .env(NO_BACKOFF_SLEEP.0, NO_BACKOFF_SLEEP.1)
        .output()
        .expect("callisto release should run")
}

pub fn execute_from_coordinator(
    coordinator: &Path,
    source: &Path,
    intent: &Path,
    receipt: &Path,
    publishers: FakePublishers<'_>,
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
            "--receipt",
            receipt.to_str().unwrap(),
            "--orchestration-revision",
            &git(coordinator, &["rev-parse", "HEAD"]),
        ]);
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
        .env("CALLISTO_TEST_CARGO_MARKER", registry_marker(source))
        .env("CALLISTO_TEST_REAL_GIT", system_git())
        .env(NO_BACKOFF_SLEEP.0, NO_BACKOFF_SLEEP.1)
        .output()
        .expect("cross-worktree release execute should run")
}

pub fn execute_product(
    root: &Path,
    intent: &Path,
    manifest: &Path,
    artifacts: &Path,
    receipt: &Path,
    publishers: FakePublishers<'_>,
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
            "--receipt",
            receipt.to_str().unwrap(),
            "--orchestration-revision",
            &git(root, &["rev-parse", "HEAD"]),
        ]);
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
        .env("CALLISTO_TEST_CARGO_MARKER", registry_marker(root))
        .env("CALLISTO_TEST_REAL_GIT", system_git())
        .env(NO_BACKOFF_SLEEP.0, NO_BACKOFF_SLEEP.1)
        .output()
        .expect("product release execute should run")
}

// ---------------------------------------------------------------------
// Rig: fake providers that model the real tools, one opt-in knob per
// audited defect. Every knob defaults to the permissive legacy behaviour so a
// red test isolates exactly the realism it needs.
// ---------------------------------------------------------------------

/// One external command shape the release path may emit, and the flags it may
/// pass. Anything outside this table is rejected by the fakes, flagged by
/// `assert_argv_within_allowlists`, and checked against the real tool's own help
/// by the flag contract tests.
pub struct ToolShape {
    pub tool: &'static str,
    pub subcommand: &'static [&'static str],
    pub flags: &'static [&'static str],
}

pub const TOOL_SHAPES: &[ToolShape] = &[
    ToolShape {
        tool: "gh",
        subcommand: &["api"],
        flags: &["--include", "--method"],
    },
    ToolShape {
        tool: "gh",
        subcommand: &["release", "create"],
        flags: &[
            "--repo",
            "--verify-tag",
            "--draft",
            "--generate-notes",
            "--notes-file",
            "--prerelease",
        ],
    },
    ToolShape {
        tool: "gh",
        subcommand: &["release", "edit"],
        flags: &["--repo", "--draft"],
    },
    ToolShape {
        tool: "gh",
        subcommand: &["release", "upload"],
        flags: &["--repo"],
    },
    ToolShape {
        tool: "gh",
        subcommand: &["attestation", "verify"],
        flags: &[
            "--repo",
            "--signer-workflow",
            "--signer-digest",
            "--source-digest",
            "--deny-self-hosted-runners",
        ],
    },
    ToolShape {
        tool: "git",
        subcommand: &["ls-remote"],
        flags: &[],
    },
    ToolShape {
        tool: "git",
        subcommand: &["push"],
        flags: &[],
    },
    ToolShape {
        tool: "git",
        subcommand: &["tag"],
        flags: &["-a", "-m", "--no-sign", "--"],
    },
    ToolShape {
        tool: "git",
        subcommand: &["rev-parse"],
        flags: &[
            "--verify",
            "--quiet",
            "--show-toplevel",
            "--show-object-format",
            "--is-shallow-repository",
        ],
    },
    ToolShape {
        tool: "git",
        subcommand: &["for-each-ref"],
        flags: &["--format"],
    },
    ToolShape {
        tool: "git",
        subcommand: &["--version"],
        flags: &[],
    },
    ToolShape {
        tool: "git",
        subcommand: &["remote", "get-url"],
        flags: &["--push"],
    },
    ToolShape {
        tool: "git",
        subcommand: &["status"],
        flags: &["--porcelain", "-z", "--untracked-files"],
    },
    ToolShape {
        tool: "git",
        subcommand: &["symbolic-ref"],
        flags: &["--quiet"],
    },
    ToolShape {
        tool: "cargo",
        subcommand: &["publish"],
        flags: &["--manifest-path", "--locked", "--registry"],
    },
    ToolShape {
        tool: "cargo",
        subcommand: &["info"],
        flags: &["--registry", "--config"],
    },
    ToolShape {
        tool: "npm",
        subcommand: &["view"],
        flags: &["--json", "--registry", "--fetch-retries"],
    },
    ToolShape {
        tool: "npm",
        subcommand: &["publish"],
        flags: &["--workspace", "--tag", "--access", "--registry"],
    },
];

pub fn tool_shape(tool: &str, subcommand: &[&str]) -> Option<&'static ToolShape> {
    TOOL_SHAPES
        .iter()
        .find(|shape| shape.tool == tool && shape.subcommand == subcommand)
}

/// A shell `case` pattern (`key:--flag|key:--flag`) for every flag of `tool`'s shapes.
fn flag_patterns(tool: &str) -> String {
    let patterns: Vec<String> = TOOL_SHAPES
        .iter()
        .filter(|shape| shape.tool == tool)
        .flat_map(|shape| {
            let key = shape.subcommand.join("-");
            shape.flags.iter().map(move |flag| format!("{key}:{flag}"))
        })
        .collect();
    if patterns.is_empty() {
        "__none__".to_owned()
    } else {
        patterns.join("|")
    }
}

/// The one fake `cargo`, modelling the two subcommands the release path emits.
///
/// `publish` records the published version under
/// `$CALLISTO_TEST_CARGO_MARKER.<crate>`; `info` answers from exactly that
/// file, in the real `cargo info` output shapes (exit 0 with a `version:` line,
/// or exit 101 with cargo's "could not find" or "failed to load source" text).
///
/// Invoked without `--registry` it does what real cargo does -- answers from
/// the local manifest, the D01 defect -- rather than refusing, so dropping
/// `--registry` from the production argv breaks the end-to-end suite instead of
/// being absorbed by a lenient fake.
///
/// `$CALLISTO_TEST_CARGO_MARKER`'s directory also holds the two knobs the
/// fakes cannot receive in memory: `unreachable` forces the transport-failure
/// shape, and `flap` (`<honest> <absent>` counters) makes a published version
/// read as absent for a while, which is index-propagation lag.
fn rig_cargo() -> String {
    let patterns = flag_patterns("cargo");
    format!(
        r#"#!/bin/sh
printf 'cargo %s\n' "$*" >> "$CALLISTO_TEST_LOG"
if [ -n "$CALLISTO_TEST_CARGO_CALLS" ]; then
  printf '%s|%s\n' "$PWD" "$*" >> "$CALLISTO_TEST_CARGO_CALLS"
fi
# Real cargo takes global `--config KEY=VALUE` before the subcommand.
while [ "$1" = "--config" ]; do shift 2; done
case "$1" in
  publish|info) key=$1 ;;
  *) printf 'error: no such command: `%s`\n' "$1" >&2; exit 1 ;;
esac
for a in "$@"; do
  case "$a" in
    -*)
      name=${{a%%=*}}
      case "$key:$name" in
        {patterns}) ;;
        *) printf "error: unexpected argument '%s' found\n" "$a" >&2; exit 1 ;;
      esac
      ;;
  esac
done
state=$(dirname "$CALLISTO_TEST_CARGO_MARKER")
registry=''
prev=''
for a in "$@"; do
  if [ "$prev" = --registry ]; then registry=$a; fi
  prev=$a
done

if [ "$key" = info ]; then
  spec=$2
  crate=${{spec%@*}}
  want=${{spec#*@}}
  if [ -z "$registry" ]; then
    printf '%s\nversion: %s (from ./)\nlicense: unknown\n' "$crate" "$want"
    exit 0
  fi
  if [ -f "$state/unreachable" ]; then
    printf '    Updating `%s` index\n' "$registry" >&2
    printf 'error: failed to load source for dependency `%s`\n\nCaused by:\n  unable to update registry `%s`\n' "$crate" "$registry" >&2
    exit 101
  fi
  published=$(cat "$CALLISTO_TEST_CARGO_MARKER.$crate" 2>/dev/null)
  serve=0
  if [ "$published" = "$want" ]; then serve=1; fi
  if [ "$serve" = 1 ] && [ -f "$state/flap" ]; then
    read -r honest absent < "$state/flap"
    if [ "${{honest:-0}}" -gt 0 ]; then
      printf '%s %s\n' "$((honest - 1))" "${{absent:-0}}" > "$state/flap"
    elif [ "${{absent:-0}}" -gt 0 ]; then
      printf '0 %s\n' "$((absent - 1))" > "$state/flap"
      serve=0
    fi
  fi
  if [ "$serve" = 1 ]; then
    printf '%s\nversion: %s (from registry `%s`)\nlicense: unknown\nrust-version: unknown\n' "$crate" "$want" "$registry"
    exit 0
  fi
  printf '    Updating `%s` index\n' "$registry" >&2
  printf 'error: could not find `%s` in registry `%s`\n' "$spec" "$registry" >&2
  exit 101
fi

manifest=''
prev=''
for a in "$@"; do
  if [ "$prev" = --manifest-path ]; then manifest=$a; fi
  prev=$a
done
name=$(grep -m1 '^name = ' "$manifest" | cut -d'"' -f2)
version=$(grep -m1 '^version = ' "$manifest" | cut -d'"' -f2)
if [ "$CALLISTO_TEST_CARGO_TARGET" = 1 ]; then
  mkdir -p "$PWD/target/package"
  : > "$PWD/target/package/x"
fi
if [ -n "$CALLISTO_TEST_CARGO_PUBLISH_EXIT" ]; then
  printf '%s\n' "$CALLISTO_TEST_CARGO_PUBLISH_STDERR" >&2
  exit "$CALLISTO_TEST_CARGO_PUBLISH_EXIT"
fi
if [ -n "$CALLISTO_TEST_CARGO_PUBLISH_SLEEP" ]; then
  sleep "$CALLISTO_TEST_CARGO_PUBLISH_SLEEP"
fi
printf '%s\n' "$version" > "$CALLISTO_TEST_CARGO_MARKER.$name"
exit 0
"#
    )
}

/// `gh` accepting only the shapes in `TOOL_SHAPES`, with cobra-style errors for the rest.
fn rig_gh() -> String {
    let patterns = flag_patterns("gh");
    format!(
        r#"#!/bin/sh
fx=$(dirname "$0")/fixtures
printf 'gh %s\n' "$*" >> "$CALLISTO_TEST_LOG"
if [ -n "$CALLISTO_TEST_GH_CALLS" ]; then printf '%s\n' "$*" >> "$CALLISTO_TEST_GH_CALLS"; fi
{FAKE_GH_FORGE}
case "$1" in
  api) key=api ;;
  release)
    case "$2" in
      create|edit|upload) key=release-$2 ;;
      *) printf 'unknown command "%s" for "gh release"\n' "$2" >&2; exit 1 ;;
    esac
    ;;
  attestation)
    case "$2" in
      verify) key=attestation-verify ;;
      *) printf 'unknown command "%s" for "gh attestation"\n' "$2" >&2; exit 1 ;;
    esac
    ;;
  *) printf 'unknown command "%s" for "gh"\n' "$1" >&2; exit 1 ;;
esac
for a in "$@"; do
  case "$a" in
    -*)
      name=${{a%%=*}}
      case "$key:$name" in
        {patterns}) ;;
        *) printf 'unknown flag: %s\n' "$a" >&2; exit 1 ;;
      esac
      ;;
  esac
done
case "$key" in
  attestation-verify) exit 0 ;;
  api)
    for a in "$@"; do endpoint=$a; done
    gh_api "$endpoint"
    exit $?
    ;;
  release-*)
    action=${{key#release-}}
    shift 2
    gh_release "$action" "$@"
    ;;
esac
exit 0
"#
    )
}

/// A `gh` that answers every API read with the real 401 shape. Used to prove an
/// unauthenticated forge fails closed; any non-`api` command is unknown to it.
pub fn unauthorized_gh() -> &'static str {
    "#!/bin/sh\nprintf 'gh %s\\n' \"$*\" >> \"$CALLISTO_TEST_LOG\"\nif [ \"$1\" = api ]; then\n  printf 'HTTP/2.0 401 Unauthorized\\n\\n{\"message\":\"Bad credentials\",\"status\":\"401\"}'\n  printf 'gh: Bad credentials (HTTP 401)\\n' >&2\n  exit 1\nfi\nprintf 'unknown command \"%s\" for \"gh\"\\n' \"$1\" >&2\nexit 1\n"
}

/// Writes the captured-response templates the fake `gh` renders from.
fn write_fixture_templates(bin: &Path) {
    let dir = bin.join("fixtures");
    fs::create_dir_all(&dir).unwrap();
    let head = |raw: &str| fixtures::split_raw_http(raw).0.to_owned();
    fs::write(dir.join("gh-200.head"), head(fixtures::GITHUB_RELEASE_PUBLISHED)).unwrap();
    fs::write(dir.join("gh-list.head"), head(fixtures::GITHUB_RELEASE_LIST)).unwrap();
    fs::write(dir.join("gh-404.head"), head(fixtures::GITHUB_RELEASE_404)).unwrap();
    fs::write(dir.join("gh-404.body"), fixtures::body_of(fixtures::GITHUB_RELEASE_404)).unwrap();
    let mut release = fixtures::github_release(
        "@@TAG@@",
        false,
        false,
        "@@COMMITISH@@",
        &[("@@NAME@@", 0, "@@DIGEST@@")],
    );
    let mut asset = release["assets"][0].clone();
    asset["size"] = "@@SIZE@@".into();
    release["draft"] = "@@DRAFT@@".into();
    release["prerelease"] = "@@PRERELEASE@@".into();
    release["assets"] = "@@ASSETS@@".into();
    let release = release
        .to_string()
        .replace("\"@@DRAFT@@\"", "@@DRAFT@@")
        .replace("\"@@PRERELEASE@@\"", "@@PRERELEASE@@")
        .replace("\"@@ASSETS@@\"", "[@@ASSETS@@]");
    fs::write(dir.join("release.tmpl"), release).unwrap();
    fs::write(
        dir.join("asset.tmpl"),
        asset.to_string().replace("\"@@SIZE@@\"", "@@SIZE@@"),
    )
    .unwrap();
}

/// The GitHub Releases model the fake `gh` shares across harnesses.
///
/// It reproduces the three facts the provider depends on: a release starts as
/// a draft, `GET /releases/tags/{tag}` does not serve drafts, and the list
/// endpoint does (paginated). `$CALLISTO_TEST_FORGE_MARKER` holds `draft` or
/// `published`; an empty marker is a pre-existing published release, which is
/// how a test seeds one directly. Bodies are `$fx/release.tmpl` and
/// `$fx/asset.tmpl` (the captured release shape) with values substituted.
const FAKE_GH_FORGE: &str = r#"
forge_state() {
  if [ ! -f "$CALLISTO_TEST_FORGE_MARKER" ]; then printf 'absent'; return; fi
  case "$(cat "$CALLISTO_TEST_FORGE_MARKER")" in
    draft) printf 'draft' ;;
    *) printf 'published' ;;
  esac
}
render_release() {
  tag=$1; draft=$2; pre=$3; assets=$4
  sed -e "s|@@TAG@@|$tag|g" -e "s|@@COMMITISH@@|$CALLISTO_TEST_FORGE_COMMITISH|g" \
    -e "s|@@DRAFT@@|$draft|g" -e "s|@@PRERELEASE@@|$pre|g" -e "s|@@ASSETS@@|$assets|g" "$fx/release.tmpl"
}
release_json() {
  assets=''
  comma=''
  if [ -f "$CALLISTO_TEST_ARTIFACT_MARKER" ]; then
    while IFS='|' read -r asset size digest; do
      rendered=$(sed -e "s|@@NAME@@|$asset|g" -e "s|@@SIZE@@|$size|g" -e "s|@@DIGEST@@|$digest|g" "$fx/asset.tmpl")
      assets="${assets}${comma}${rendered}"
      comma=','
    done < "$CALLISTO_TEST_ARTIFACT_MARKER"
  fi
  draft=false
  if [ "$(forge_state)" = draft ]; then draft=true; fi
  pre=false
  if [ -f "$CALLISTO_TEST_FORGE_MARKER.prerelease" ]; then pre=true; fi
  render_release "$CALLISTO_TEST_FORGE_TAG" "$draft" "$pre" "$assets"
}
http_ok() { cat "$fx/$1"; printf '%s' "$2"; }
http_404() { cat "$fx/gh-404.head" "$fx/gh-404.body"; printf 'gh: Not Found (HTTP 404)\n' >&2; return 1; }
gh_api() {
  endpoint=$1
  state=$(forge_state)
  case "$endpoint" in
    */releases/tags/*)
      if [ "$state" = published ]; then http_ok gh-200.head "$(release_json)"; else http_404; fi
      ;;
    *'/releases?'*)
      page=${endpoint##*page=}
      if [ "$state" = absent ]; then http_ok gh-list.head '[]'; return; fi
      listed=${CALLISTO_TEST_FORGE_PAGE:-1}
      if [ "$page" = "$listed" ]; then
        http_ok gh-list.head "[$(release_json)]"
      elif [ "$page" -lt "$listed" ]; then
        http_ok gh-list.head "[$(render_release 'unrelated@0.0.1' false false '')]"
      else
        http_ok gh-list.head '[]'
      fi
      ;;
    *) http_404 ;;
  esac
}
gh_release() {
  action=$1
  shift
  case "$action" in
    create)
      state=draft
      for a in "$@"; do
        case "$a" in
          --draft) state=draft ;;
          --prerelease) : > "$CALLISTO_TEST_FORGE_MARKER.prerelease" ;;
        esac
      done
      printf '%s' "$state" > "$CALLISTO_TEST_FORGE_MARKER"
      ;;
    edit)
      for a in "$@"; do
        case "$a" in
          --draft=false) printf 'published' > "$CALLISTO_TEST_FORGE_MARKER" ;;
        esac
      done
      ;;
    upload)
      [ -f "$CALLISTO_TEST_FORGE_MARKER" ] || { printf 'release not found\n' >&2; exit 1; }
      # $1 is the tag, $2 the asset path.
      asset=$2
      name=$(basename "$asset")
      size=$(wc -c < "$asset" | tr -d ' ')
      digest=$(shasum -a 256 "$asset" | awk '{print $1}')
      printf '%s|%s|%s\n' "$name" "$size" "$digest" >> "$CALLISTO_TEST_ARTIFACT_MARKER"
      ;;
  esac
}
"#;

/// Fake remote tag storage shared by both fake `git` programs: `git push`
/// records the pushed tag exactly as `ls-remote` would report it (the tag
/// object plus its peeled commit, the captured `ls-remote` line shape), and
/// `ls-remote` answers from that record. Real git is used instead wherever the
/// rig points at a real bare remote. `push` and `ls-remote` take no flags.
const FAKE_GIT_REMOTE_TAGS: &str = r#"
store="$CALLISTO_TEST_GIT_TRACE.remote-tags"
tab=$(printf '\t')
no_flags_allowed() {
  first=$1
  shift
  for a in "$@"; do
    case "$a" in
      -*) printf "error: unknown option \`%s'\n" "${a#-}" >&2; printf 'usage: git %s <remote> <ref>...\n' "$first" >&2; exit 129 ;;
    esac
  done
}
record_pushed_tag() {
  obj=$("$CALLISTO_TEST_REAL_GIT" rev-parse "refs/tags/$1" 2>/dev/null) || return 0
  commit=$("$CALLISTO_TEST_REAL_GIT" rev-parse "refs/tags/$1^{commit}" 2>/dev/null) || return 0
  printf '%s%srefs/tags/%s\n' "$obj" "$tab" "$1" >> "$store"
  printf '%s%srefs/tags/%s^{}\n' "$commit" "$tab" "$1" >> "$store"
}
if [ "$1" = ls-remote ]; then
  no_flags_allowed "$@"
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
  no_flags_allowed "$@"
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

/// Fails unless every external command the run logged is a `TOOL_SHAPES`
/// invocation: a known subcommand carrying only flags its shape lists. `log` is
/// `$CALLISTO_TEST_LOG` (cargo, gh, git push) and `git_trace` every git call.
pub fn assert_argv_within_allowlists(log: &Path, git_trace: &Path) {
    let mut violations = Vec::new();
    for path in [log, git_trace] {
        for line in fs::read_to_string(path).unwrap_or_default().lines() {
            argv_violations(line, &mut violations);
        }
    }
    assert!(
        violations.is_empty(),
        "external commands outside the allow-lists:\n{}",
        violations.join("\n")
    );
}

pub fn argv_violations(line: &str, violations: &mut Vec<String>) {
    let mut words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() {
        return;
    }
    let tool = words.remove(0);
    if tool == "git" {
        while matches!(words.first(), Some(&"-c" | &"-C")) {
            words.drain(..2.min(words.len()));
        }
    }
    if tool == "cargo" {
        while words.first() == Some(&"--config") {
            words.drain(..2.min(words.len()));
        }
    }
    let matched = [2, 1]
        .into_iter()
        .filter(|len| words.len() >= *len)
        .find_map(|len| tool_shape(tool, &words[..len]).map(|shape| (shape, len)));
    let Some((shape, len)) = matched else {
        violations.push(format!("no allow-list entry for: {line}"));
        return;
    };
    for word in &words[len..] {
        if word.starts_with('-') && word.len() > 1 {
            let name = word.split('=').next().unwrap();
            if !shape.flags.contains(&name) {
                violations.push(format!(
                    "flag `{name}` is not allowed for `{tool} {}`: {line}",
                    words[..len].join(" ")
                ));
            }
        }
    }
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
        for (name, body) in [("cargo", rig_cargo()), ("gh", rig_gh()), ("git", rig_git())] {
            fs::write(bin.join(name), body).unwrap();
            fs::set_permissions(bin.join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
        write_fixture_templates(&bin);
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

    /// `cargo publish` leaves `target/package/` in its working directory.
    pub fn real_cargo_target_dir(&mut self) -> &mut Self {
        self.set("CALLISTO_TEST_CARGO_TARGET", "1")
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
    receipt: &Path,
    rig: &Rig,
    forge_tag: &str,
    extra: &[&str],
    orchestration: Option<&str>,
) -> Output {
    execute_rig_in(root, root, intent, receipt, rig, forge_tag, extra, orchestration)
}

/// The same run with `--cwd` chosen separately from the fixture root.
#[allow(clippy::too_many_arguments)]
pub fn execute_rig_in(
    root: &Path,
    cwd: &Path,
    intent: &Path,
    receipt: &Path,
    rig: &Rig,
    forge_tag: &str,
    extra: &[&str],
    orchestration: Option<&str>,
) -> Output {
    let path = format!("{}:{}", rig.bin.display(), std::env::var("PATH").unwrap());
    let head = git(root, &["rev-parse", "HEAD"]);
    let mut command = Command::new(env!("CARGO_BIN_EXE_callisto"));
    command
        .args(["--format", "json", "--cwd", cwd.to_str().unwrap()])
        .args([
            "release",
            "execute",
            "--intent",
            intent.to_str().unwrap(),
            "--receipt",
            receipt.to_str().unwrap(),
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
        .env("CALLISTO_TEST_CARGO_MARKER", registry_marker(root))
        .env("CALLISTO_TEST_REAL_GIT", system_git())
        .env(NO_BACKOFF_SLEEP.0, NO_BACKOFF_SLEEP.1);
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
