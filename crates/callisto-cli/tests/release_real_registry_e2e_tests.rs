#![cfg(unix)]

//! Real-registry e2e coverage (SPEC-DX-CORRECTNESS-E2E).
//!
//! `release_forge_publish_e2e_tests.rs` and `durable_release_e2e_tests.rs`
//! cover npm and PyPI publish only against fake providers. This file adds a
//! real local Verdaccio registry to the mix: `callisto release execute`'s
//! actual npm provider adapter, real `npm`, real HTTP over loopback. The
//! fake-provider suites are unchanged and remain the fast, hermetic default.
//!
//! Registry routing goes entirely through npm's own `.npmrc` (a project-root
//! file, gitignored so it never dirties the release-trust worktree check),
//! never a callisto registry binding: `registry_endpoint::canonical_registry_url`
//! rejects non-https URLs with no loopback exception, by design (see
//! SPEC-DX-CORRECTNESS-PARITY AC-4/AC-5), so a local `http://127.0.0.1` test
//! registry can never be configured as a `[registries]` entry. Leaving the
//! package unmatched by any `[registries]` override keeps
//! `PreparedRegistryBinding.endpoint` at `None`, which is also what makes
//! this reachable at all: both `npm view` (observation) and `npm publish`
//! omit `--registry` whenever `endpoint` is `None`, so npm resolves the
//! registry itself from `.npmrc` -- unlike PyPI's observation step, which
//! hardcodes the public simple index when `endpoint` is `None` (see the PyPI
//! note below).
//!
//! PyPI (AC-22) has no counterpart here. `registry_argv::pypi_publish_argv`
//! unconditionally appends `--skip-existing` to the twine upload argv; twine
//! 5 and newer's `verify_feature_capability` refuses to run at all when that
//! flag is set against any repository URL other than `https://upload.pypi.org/` or
//! `https://test.pypi.org/` (`UnsupportedConfiguration`). Confirmed live
//! against a real local `pypiserver` with real `python -m build` + `twine`:
//! the upload never reaches the wire. This blocks real-registry PyPI e2e
//! coverage -- and blocks any callisto user publishing to a private PyPI
//! index with a current twine -- until `--skip-existing` becomes conditional
//! on the target registry. Tracked as a follow-up rather than worked around
//! here.

#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use release_harness::*;

/// Finds `program` on `PATH`, the way a shell would. Never panics, so a test
/// can skip cleanly when the real tool isn't installed.
fn find_on_path(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

/// An ephemeral loopback port: bind to port 0, read back what the OS
/// assigned, release it immediately. The gap between release and the real
/// server binding it is a bounded TOCTOU risk, acceptable for test
/// infrastructure that owns the box.
fn free_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener must bind");
    listener
        .local_addr()
        .expect("listener must have a local address")
        .port()
}

/// A spawned server process, killed on drop so a panicking assertion never
/// leaks it past the test.
struct ServerGuard {
    child: Child,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

/// Blocks until `port` accepts a loopback TCP connection, or panics after `timeout`.
fn wait_for_port(port: u16, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "server on 127.0.0.1:{port} never became reachable within {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// GETs `url` with `curl` -- the same tool callisto's own PyPI observation
/// uses -- for an assertion independent of callisto's own HTTP client code.
fn curl_get(url: &str) -> String {
    let output = Command::new("curl")
        .args(["-sS", url])
        .output()
        .expect("curl must be runnable");
    assert!(
        output.status.success(),
        "curl {url} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Starts a real local Verdaccio on an ephemeral loopback port with
/// anonymous publish enabled (`access`/`publish`/`unpublish: $all`) and no
/// uplinks configured, so a lookup for an unknown package fails closed (404)
/// instead of reaching the real npm registry. Panics if it never becomes
/// reachable; callers check `find_on_path("verdaccio")` first.
fn start_verdaccio(external: &Path) -> (ServerGuard, u16) {
    let config_dir = external.join("verdaccio");
    fs::create_dir_all(config_dir.join("storage")).unwrap();
    fs::write(
        config_dir.join("config.yaml"),
        "storage: ./storage\n\
         self_path: ./storage\n\
         web:\n  enable: false\n\
         log: { type: stdout, format: pretty, level: warn }\n\
         packages:\n  '**':\n    access: $all\n    publish: $all\n    unpublish: $all\n",
    )
    .unwrap();
    let port = free_loopback_port();
    let child = Command::new("verdaccio")
        .args(["-c", "config.yaml", "-l", &format!("127.0.0.1:{port}")])
        .current_dir(&config_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("verdaccio must be spawnable");
    let guard = ServerGuard { child };
    wait_for_port(port, Duration::from_secs(20));
    (guard, port)
}

const NPM_PACKAGE: &str = "callisto-e2e-probe";
const NPM_ORIGIN: &str = "https://github.com/example/npm-registry-fixture.git";

/// A minimal npm-workspace release fixture: a private workspace root plus one
/// publishable member, `publish-to = ["npm"]` only -- no `github-release`, so
/// `release execute` never touches `gh` (confirmed via `release plan`'s
/// operation list: exactly a registry-publish and a tag operation, nothing
/// forge-shaped). `.npmrc` is gitignored from the very first commit, because
/// it's written after the release commit is sealed and `release execute`'s
/// clean-worktree gate (E051) would otherwise reject the untracked file.
fn npm_registry_release_fixture(package_name: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Callisto Test"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "tag.gpgsign", "false"]);
    git(root, &["remote", "add", "origin", NPM_ORIGIN]);

    fs::write(root.join(".gitignore"), ".npmrc\n").unwrap();
    fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"npm-registry-fixture-root\",\n  \"version\": \"0.0.0\",\n  \"private\": true,\n  \"workspaces\": [\"packages/probe\"]\n}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/probe")).unwrap();
    fs::write(
        root.join("packages/probe/package.json"),
        format!("{{\n  \"name\": \"{package_name}\",\n  \"version\": \"0.1.0\"\n}}\n"),
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial workspace"]);

    let init = callisto(root, &["init", "--yes"]);
    assert!(init.status.success(), "init failed: {}", stderr_of(&init));
    let config_path = root.join("callisto.toml");
    let config = fs::read_to_string(&config_path).unwrap();
    fs::write(
        &config_path,
        format!("{config}\n[[package]]\nmatch = \"npm/{package_name}\"\npublish-to = [\"npm\"]\n"),
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "configure callisto"]);

    let add = callisto(
        root,
        &[
            "add",
            "--package",
            &format!("{package_name}:minor"),
            "--summary",
            "Ship real-registry e2e coverage",
        ],
    );
    assert!(add.status.success(), "add failed: {}", stderr_of(&add));
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "add release changeset"]);

    let version = callisto(root, &["version", "--emit-decision", DECISION_PATH]);
    assert!(version.status.success(), "version failed: {}", stderr_of(&version));

    for entry in fs::read_dir(root.join(".changeset")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|name| name != "README.md")
            && path.extension().is_some_and(|extension| extension == "md")
        {
            fs::remove_file(path).unwrap();
        }
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "release npm package 0.2.0"]);
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &release_commit]);
    (dir, release_commit)
}

/// AC-20 / AC-20b: a real Verdaccio registry, npm routed to it purely via
/// `.npmrc`, `callisto release execute`'s real npm provider adapter, and an
/// assertion against the registry's own package metadata afterward -- not
/// just callisto's exit code.
#[test]
fn npm_publish_against_a_real_verdaccio_registry_is_retrievable_afterward() {
    let Some(_verdaccio) = find_on_path("verdaccio") else {
        eprintln!("SKIPPED npm real-registry e2e: `verdaccio` is not installed");
        return;
    };

    let (dir, release_commit) = npm_registry_release_fixture(NPM_PACKAGE);
    let root = dir.path();
    let external = tempfile::tempdir().unwrap();

    let intent = external.path().join("release-intent.json");
    let plan = callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            &release_commit,
            "--decision",
            DECISION_PATH,
            "--out",
            intent.to_str().unwrap(),
        ],
    );
    assert!(plan.status.success(), "release plan failed: {}", stderr_of(&plan));

    let (_verdaccio_guard, port) = start_verdaccio(external.path());

    // npm's own configuration, not a callisto registry binding: no
    // `[registries]` entry exists, so `PreparedRegistryBinding.endpoint`
    // stays `None` and neither `npm view` nor `npm publish` ever receives a
    // `--registry` flag from callisto -- both fall through to this file.
    fs::write(
        root.join(".npmrc"),
        format!("registry=http://127.0.0.1:{port}/\n//127.0.0.1:{port}/:_authToken=callisto-e2e-test-token\n"),
    )
    .unwrap();
    assert_eq!(
        git(root, &["status", "--porcelain"]),
        "",
        ".npmrc must be gitignored, not dirty the sealed release commit"
    );

    let bare = bare_remote(external.path());
    let mut rig = Rig::new(external.path(), &release_commit);
    rig.real_git_push(&bare, NPM_ORIGIN);

    let receipt = external.path().join("release-receipt.json");
    let tag = format!("{NPM_PACKAGE}@0.2.0");
    let output = execute_rig(root, &intent, &receipt, &rig, &tag, &[], None);
    assert!(
        output.status.success(),
        "real-registry npm release execute failed: {}\neffect log:\n{}",
        stderr_of(&output),
        fs::read_to_string(&rig.log).unwrap_or_default(),
    );
    assert!(receipt.exists(), "a successful release must persist a terminal receipt");

    // Independent verification against the registry itself: the published
    // version must be visible in Verdaccio's own package metadata, not just
    // inferred from callisto's exit code.
    let metadata_raw = curl_get(&format!("http://127.0.0.1:{port}/{NPM_PACKAGE}"));
    let metadata: serde_json::Value = serde_json::from_str(&metadata_raw)
        .unwrap_or_else(|error| panic!("registry metadata was not JSON: {error}\n{metadata_raw}"));
    assert!(
        metadata["versions"]["0.2.0"].is_object(),
        "published version must be retrievable from the registry's own package metadata: {metadata}"
    );

    assert!(
        git(&bare, &["tag", "--list", &tag]).contains(&tag),
        "the release tag must reach the remote"
    );
}
