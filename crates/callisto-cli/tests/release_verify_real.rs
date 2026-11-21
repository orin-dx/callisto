#![cfg(unix)]

//! Recovery rehearsals against real local registries, run by `just verify-real`.
//!
//! Every registry is bound through `[registries]` in `callisto.toml` over `https://`, so the production registry
//! binding and its https rule are exercised unchanged: the registries serve a self-signed certificate that the
//! package managers trust through their own CA environment variables. A missing tool fails the test.
//!
//! The Changesets scenarios run the real `@changesets/cli` 2.x and 3.x beside callisto and compare the versions
//! each produces.
//!
//! Ignored by default because they need the registries and CLIs `just verify-real-tools` installs.

#[path = "common/local_registries.rs"]
mod local_registries;
#[path = "common/release_harness.rs"]
mod release_harness;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

use local_registries::*;
use release_harness::*;

const DEPENDENCY: &str = "callisto-verify-dependency";
const DEPENDENT: &str = "callisto-verify-dependent";
const PYPI_PACKAGE: &str = "callisto-verify-py";
const PYPI_ORIGIN: &str = "https://github.com/example/pypi-registry-fixture.git";

/// A released workspace, the intent planned from it, and the rig that runs it against real local registries.
struct Release {
    dir: tempfile::TempDir,
    external: tempfile::TempDir,
    intent: PathBuf,
    receipt: PathBuf,
    bare: PathBuf,
    rig: Rig,
    tls: Tls,
    forge_tag: String,
}

impl Release {
    /// `fixture` is the sealed release commit; `tls` was generated before it so its ports could be written into it.
    fn new(
        fixture: (tempfile::TempDir, String),
        external: tempfile::TempDir,
        tls: Tls,
        origin: &str,
        tag: &str,
    ) -> Self {
        let (dir, release_commit) = fixture;
        let intent = external.path().join("release-intent.json");
        let plan = callisto(
            dir.path(),
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

        let bare = bare_remote(external.path());
        let mut rig = Rig::new(external.path(), &release_commit);
        rig.real_git_push(&bare, origin);
        for (key, value) in tls.trust_env() {
            rig.set(key, &value);
        }
        // The rig's fake `cargo` must not stand in for the real one on PATH.
        fs::remove_file(rig.bin.join("cargo")).unwrap();
        let receipt = external.path().join("release-receipt.json");
        Self {
            dir,
            external,
            intent,
            receipt,
            bare,
            rig,
            tls,
            forge_tag: tag.to_owned(),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Packs and seals the npm and Python artifacts once, without publisher credentials, as the generated workflow's
    /// build jobs do. Reruns reuse the same sealed files.
    fn sealed_artifacts(&self) -> Option<(PathBuf, PathBuf)> {
        let intent: callisto_model::ReleaseIntentV1 = serde_json::from_slice(&fs::read(&self.intent).unwrap()).unwrap();
        let npm = !callisto_model::npm_publishes(&intent).is_empty();
        let python = !callisto_model::python_publishes(&intent).is_empty();
        if !npm && !python {
            return None;
        }
        let directory = self.external.path().join("sealed-artifacts");
        let manifest = self.external.path().join("artifact-manifest.json");
        if !manifest.exists() {
            fs::create_dir_all(&directory).unwrap();
            let run = |args: &[&str]| {
                let mut command = rig_command(self.root(), self.root(), &self.rig, &self.forge_tag);
                for credential in [
                    "CARGO_REGISTRY_TOKEN",
                    "GH_TOKEN",
                    "GITHUB_TOKEN",
                    "NPM_TOKEN",
                    "NODE_AUTH_TOKEN",
                    "NPM_CONFIG_TOKEN",
                    "PYPI_TOKEN",
                    "TWINE_USERNAME",
                    "TWINE_PASSWORD",
                ] {
                    command.env_remove(credential);
                }
                let output = command.args(args).output().expect("artifact preparation must run");
                assert!(output.status.success(), "{args:?}: {}", stderr_of(&output));
            };
            let intent = self.intent.to_str().unwrap();
            let dir = directory.to_str().unwrap();
            if npm {
                run(&["release", "pack-npm", "--intent", intent, "--artifact-dir", dir]);
            }
            if python {
                run(&["release", "build-python", "--intent", intent, "--artifact-dir", dir]);
            }
            let mut seal = vec![
                "release",
                "artifact-manifest",
                "--intent",
                intent,
                "--artifact-dir",
                dir,
                "--out",
                manifest.to_str().unwrap(),
            ];
            if npm {
                seal.push("--seal-npm");
            }
            if python {
                seal.push("--seal-python");
            }
            run(&seal);
        }
        Some((directory, manifest))
    }

    fn execute(&self) -> Output {
        let sealed = self.sealed_artifacts();
        let extra: Vec<&str> = sealed.as_ref().map_or_else(Vec::new, |(directory, manifest)| {
            vec![
                "--artifact-manifest",
                manifest.to_str().unwrap(),
                "--artifact-dir",
                directory.to_str().unwrap(),
            ]
        });
        execute_rig(
            self.root(),
            &self.intent,
            &self.receipt,
            &self.rig,
            &self.forge_tag,
            &extra,
            None,
        )
    }

    fn remote_has_tag(&self, tag: &str) -> bool {
        git(&self.bare, &["tag", "--list", tag]).contains(tag)
    }

    fn failure_context(&self, output: &Output) -> String {
        format!(
            "stderr:\n{}\neffect log:\n{}",
            stderr_of(output),
            fs::read_to_string(&self.rig.log).unwrap_or_default()
        )
    }

    fn assert_failed_with(&self, output: &Output, code: &str) {
        assert!(
            !output.status.success(),
            "the run must fail\n{}",
            self.failure_context(output)
        );
        assert!(
            stderr_of(output).contains(&format!("\"code\": \"{code}\"")),
            "expected {code}\n{}",
            self.failure_context(output)
        );
        assert!(!self.receipt.exists(), "a failed run must leave no terminal receipt");
    }

    fn assert_completed(&self, output: &Output) {
        assert!(
            output.status.success(),
            "the run must complete\n{}",
            self.failure_context(output)
        );
        assert!(self.receipt.exists(), "a completed run must write a receipt");
    }
}

// ----------------------------------------------------------------------- npm

/// A released npm workspace bound to one TLS Verdaccio.
struct NpmRelease {
    release: Release,
    port: u16,
}

impl NpmRelease {
    fn new() -> Self {
        let external = tempfile::tempdir().unwrap();
        let tls = Tls::generate(external.path());
        let port = free_loopback_port();
        let fixture = npm_release_fixture(
            &[(DEPENDENCY, &[]), (DEPENDENT, &[DEPENDENCY])],
            &format!("\n[registries.npm]\nurl = \"https://127.0.0.1:{port}/\"\nconfirm-timeout = \"20s\"\n"),
        );
        fs::write(
            fixture.0.path().join(".npmrc"),
            format!("//127.0.0.1:{port}/:_authToken=callisto-verify-token\n"),
        )
        .unwrap();
        let release = Release::new(fixture, external, tls, NPM_ORIGIN, &format!("{DEPENDENT}@0.2.0"));
        Self { release, port }
    }

    fn registry(&self, packages: &str) -> ServerGuard {
        start_verdaccio_with(
            self.release.external.path(),
            "npm",
            self.port,
            Some(&self.release.tls),
            packages,
        )
    }

    /// The versions the registry holds for `package`.
    fn versions(&self, package: &str) -> Vec<String> {
        let url = format!("https://127.0.0.1:{}/{package}", self.port);
        let raw = std::process::Command::new("curl")
            .args(["-sS", "-w", "\n%{http_code}", &url])
            .envs(self.release.tls.trust_env())
            .output()
            .expect("curl must be runnable");
        let text = String::from_utf8_lossy(&raw.stdout).into_owned();
        let (body, status) = text.rsplit_once('\n').unwrap();
        if status == "404" {
            return Vec::new();
        }
        let metadata: serde_json::Value = serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {text}"));
        metadata["versions"].as_object().unwrap().keys().cloned().collect()
    }
}

/// Verdaccio refusing `DEPENDENT` stands for a registry that dies after the first package is published.
fn refusing_the_dependent() -> String {
    format!(
        "packages:\n  '{DEPENDENT}':\n    access: $all\n    publish: nobody\n  '**':\n    access: $all\n    publish: $all\n    unpublish: $all\n"
    )
}

#[test]
#[ignore = "needs local registries; run via `just verify-real`"]
fn a_partial_npm_publish_is_adopted_by_the_rerun_without_republishing() {
    let npm = NpmRelease::new();
    let release = &npm.release;

    {
        let _registry = npm.registry(&refusing_the_dependent());
        let output = release.execute();
        assert!(
            !output.status.success(),
            "the registry refuses {DEPENDENT}, so the run must fail\n{}",
            release.failure_context(&output)
        );
        assert!(!release.receipt.exists(), "a failed run must leave no terminal receipt");
        assert_eq!(
            npm.versions(DEPENDENCY),
            ["0.2.0"],
            "the dependency published before the failure"
        );
        assert!(npm.versions(DEPENDENT).is_empty(), "the refused package must not exist");
    }

    let _registry = npm.registry(VERDACCIO_OPEN_ACCESS);
    let second = release.execute();
    release.assert_completed(&second);
    assert!(
        stderr_of(&second).contains("already published"),
        "the rerun must adopt the dependency instead of publishing it again:\n{}",
        stderr_of(&second)
    );
    assert_eq!(npm.versions(DEPENDENCY), ["0.2.0"]);
    assert_eq!(npm.versions(DEPENDENT), ["0.2.0"]);
    for package in [DEPENDENCY, DEPENDENT] {
        assert!(
            release.remote_has_tag(&format!("{package}@0.2.0")),
            "{package} tag must reach the remote"
        );
    }
}

#[test]
#[ignore = "needs local registries; run via `just verify-real`"]
fn a_remote_tag_on_another_commit_stops_the_release_with_a_conflict() {
    let npm = NpmRelease::new();
    let release = &npm.release;
    let _registry = npm.registry(VERDACCIO_OPEN_ACCESS);
    let other = git(release.root(), &["rev-parse", "HEAD~1"]);
    let tag = format!("{DEPENDENT}@0.2.0");
    git(
        release.root(),
        &[
            "push",
            &format!("file://{}", release.bare.display()),
            &format!("{other}:refs/tags/{tag}"),
        ],
    );

    let output = release.execute();
    release.assert_failed_with(&output, "E167");
    assert_eq!(
        git(&release.bare, &["rev-parse", &format!("refs/tags/{tag}^{{commit}}")]),
        other,
        "the foreign tag must be left as it was"
    );
}

// ---------------------------------------------------------------------- pypi

/// A released Python package whose index (a JSON simple index) and upload endpoint (pypiserver) serve two directories
/// over TLS.
struct PypiRelease {
    release: Release,
    index_port: u16,
    upload_port: u16,
}

impl PypiRelease {
    fn new(confirm_timeout: &str) -> Self {
        let external = tempfile::tempdir().unwrap();
        let tls = Tls::generate(external.path());
        let index_port = free_loopback_port();
        let upload_port = loop {
            let port = free_loopback_port();
            if port != index_port {
                break port;
            }
        };
        let fixture = pypi_release_fixture(&format!(
            "\n[registries.pypi]\nurl = \"https://127.0.0.1:{index_port}/simple/\"\n\
             publish-url = \"https://127.0.0.1:{upload_port}/\"\nconfirm-timeout = \"{confirm_timeout}\"\n"
        ));
        let mut release = Release::new(fixture, external, tls, PYPI_ORIGIN, &format!("{PYPI_PACKAGE}@0.2.0"));
        let wheels = std::env::var("VERIFY_REAL_WHEELS").expect("`just verify-real` provides VERIFY_REAL_WHEELS");
        release
            .rig
            .set("TWINE_USERNAME", "callisto-verify")
            .set("TWINE_PASSWORD", "callisto-verify")
            // The isolated build installs its backend from this directory, never from the real index.
            .set("PIP_NO_INDEX", "1")
            .set("PIP_FIND_LINKS", &wheels)
            // Looks at 1.5 s, 6 s, ... instead of 15 s, 45 s, ...
            .set("__CALLISTO_TEST_SLEEP_SCALE", "0.1");
        Self {
            release,
            index_port,
            upload_port,
        }
    }

    fn index_dir(&self) -> PathBuf {
        self.release.external.path().join("pypi-index")
    }

    fn upload_dir(&self) -> PathBuf {
        self.release.external.path().join("pypi-upload")
    }

    fn servers(&self) -> (ServerGuard, ServerGuard) {
        let external = self.release.external.path();
        let tls = Some(&self.release.tls);
        (
            start_json_simple_index(external, "index", self.index_port, &self.index_dir(), &self.release.tls),
            start_pypiserver(external, "upload", self.upload_port, &self.upload_dir(), tls),
        )
    }
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Copies every file of `from` that `to` lacks, as a mirror catching up.
fn copy_missing(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if !target.exists() {
            let partial = to.join(format!(".{}.partial", path.file_name().unwrap().to_string_lossy()));
            fs::copy(&path, &partial).unwrap();
            fs::rename(&partial, &target).unwrap();
        }
    }
}

/// Polls until `ready` holds, then waits `then` before running `act`. Used from a mirror thread.
fn after(ready: impl Fn() -> bool, then: Duration, act: impl FnOnce()) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !ready() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(then);
    act();
}

/// A released single-package Python workspace publishing to PyPI only.
fn pypi_release_fixture(registries: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    git(root, &["remote", "add", "origin", PYPI_ORIGIN]);
    fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
    fs::write(root.join(".gitignore"), "/pkg/dist/\n/pkg/build/\n*.egg-info/\n").unwrap();
    fs::create_dir_all(root.join("pkg/callisto_verify_py")).unwrap();
    fs::write(root.join("pkg/callisto_verify_py/__init__.py"), "\n").unwrap();
    fs::write(
        root.join("pkg/pyproject.toml"),
        format!(
            "[build-system]\nrequires = [\"setuptools>=61.0\"]\nbuild-backend = \"setuptools.build_meta\"\n\n\
             [project]\nname = \"{PYPI_PACKAGE}\"\nversion = \"0.1.0\"\ndescription = \"verify-real probe\"\n\
             requires-python = \">=3.8\"\n"
        ),
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial Python workspace"]);

    let init = callisto(root, &["init", "--yes", "--versioning", "independent"]);
    assert!(init.status.success(), "init failed: {}", stderr_of(&init));
    let config_path = root.join("callisto.toml");
    let mut config = fs::read_to_string(&config_path).unwrap();
    config.push_str(&format!(
        "\n[[package]]\nmatch = \"pypi/{PYPI_PACKAGE}\"\npublish-to = [\"pypi\"]\n"
    ));
    config.push_str(registries);
    fs::write(&config_path, config).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "configure callisto"]);

    let add = callisto(
        root,
        &[
            "add",
            "--package",
            &format!("pypi/{PYPI_PACKAGE}:minor"),
            "--summary",
            "Ship the verify-real probe",
        ],
    );
    assert!(add.status.success(), "add failed: {}", stderr_of(&add));
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "add release changeset"]);

    let version = callisto(
        root,
        &["version", "--no-refresh-lockfiles", "--emit-decision", DECISION_PATH],
    );
    assert!(version.status.success(), "version failed: {}", stderr_of(&version));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "release the probe 0.2.0"]);
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &release_commit]);
    (dir, release_commit)
}

#[test]
#[ignore = "needs local registries; run via `just verify-real`"]
fn a_python_release_waits_for_a_delayed_index() {
    let pypi = PypiRelease::new("30s");
    let release = &pypi.release;
    let (index, _upload) = pypi.servers();

    // The upload endpoint takes the files at once; the index shows them only after the first looks.
    let mirror = std::thread::spawn({
        let (upload, index) = (pypi.upload_dir(), pypi.index_dir());
        move || {
            after(
                || files_in(&upload).len() == 2,
                Duration::from_millis(2500),
                || copy_missing(&upload, &index),
            )
        }
    });
    let started = Instant::now();
    let output = release.execute();
    mirror.join().unwrap();
    release.assert_completed(&output);
    assert!(
        started.elapsed() > Duration::from_millis(2500) && index.log().contains("\" 404 "),
        "the run must have looked at the index while it was empty:\n{}",
        index.log()
    );
    assert_eq!(files_in(&pypi.index_dir()), files_in(&pypi.upload_dir()));
}

#[test]
#[ignore = "needs local registries; run via `just verify-real`"]
fn a_python_index_that_never_shows_the_upload_fails_and_the_rerun_adopts_it() {
    let pypi = PypiRelease::new("5s");
    let release = &pypi.release;
    let _servers = pypi.servers();

    let first = release.execute();
    release.assert_failed_with(&first, "E157");
    assert_eq!(files_in(&pypi.upload_dir()).len(), 2, "both files were uploaded");
    assert!(files_in(&pypi.index_dir()).is_empty());
    let uploaded = files_in(&pypi.upload_dir());

    copy_missing(&pypi.upload_dir(), &pypi.index_dir());
    let second = release.execute();
    release.assert_completed(&second);
    assert_eq!(
        files_in(&pypi.upload_dir()),
        uploaded,
        "the rerun must not upload again"
    );
}

#[test]
#[ignore = "needs local registries; run via `just verify-real`"]
fn a_python_release_with_a_missing_wheel_uploads_only_the_wheel() {
    let pypi = PypiRelease::new("5s");
    let release = &pypi.release;
    let _servers = pypi.servers();

    let first = release.execute();
    release.assert_failed_with(&first, "E157");
    let files = files_in(&pypi.upload_dir());
    let sdist = files
        .iter()
        .find(|name| name.ends_with(".tar.gz"))
        .expect("an sdist")
        .clone();
    let wheel = files
        .iter()
        .find(|name| name.ends_with(".whl"))
        .expect("a wheel")
        .clone();

    // The index and the upload store hold the sdist; the wheel is gone from both.
    fs::remove_file(pypi.upload_dir().join(&wheel)).unwrap();
    fs::copy(pypi.upload_dir().join(&sdist), pypi.index_dir().join(&sdist)).unwrap();
    let sdist_before = fs::read(pypi.upload_dir().join(&sdist)).unwrap();

    let mirror = std::thread::spawn({
        let (upload, index, wheel) = (pypi.upload_dir(), pypi.index_dir(), wheel.clone());
        move || {
            after(
                || upload.join(&wheel).exists(),
                Duration::ZERO,
                || copy_missing(&upload, &index),
            )
        }
    });
    let second = release.execute();
    mirror.join().unwrap();
    release.assert_completed(&second);
    assert!(
        pypi.upload_dir().join(&wheel).exists(),
        "the missing wheel must be uploaded"
    );
    assert_eq!(
        fs::read(pypi.upload_dir().join(&sdist)).unwrap(),
        sdist_before,
        "the sdist the index already held must not be uploaded again"
    );
}

// --------------------------------------------------------------------- cargo

const CARGO_ORIGIN: &str = "https://github.com/example/cargo-registry-fixture.git";
const CARGO_REGISTRY: &str = "verify-reg";
const CARGO_CRATES: [&str; 2] = ["callisto-verify-crate-a", "callisto-verify-crate-b"];

/// A released two-crate workspace publishing to the `verify-reg` Cargo registry, whose URL only `CARGO_HOME` knows.
struct CargoRelease {
    release: Release,
    port: u16,
    registry_dir: PathBuf,
}

impl CargoRelease {
    fn new() -> Self {
        let external = tempfile::tempdir().unwrap();
        let tls = Tls::generate(external.path());
        let port = free_loopback_port();
        let fixture = cargo_release_fixture(&format!(
            "\n[registries.{CARGO_REGISTRY}]\nkind = \"cargo\"\nconfirm-timeout = \"30s\"\n"
        ));
        let cargo_home = external.path().join("cargo-home");
        fs::create_dir_all(&cargo_home).unwrap();
        fs::write(
            cargo_home.join("config.toml"),
            format!(
                "[registries.{CARGO_REGISTRY}]\nindex = \"sparse+http://127.0.0.1:{port}/index/\"\n\n\
                 [unstable]\npublish-timeout = true\n\n[publish]\ntimeout = 0\n"
            ),
        )
        .unwrap();
        let registry_dir = external.path().join("cargo-registry");
        let mut release = Release::new(
            fixture,
            external,
            tls,
            CARGO_ORIGIN,
            &format!("{}@0.2.0", CARGO_CRATES[0]),
        );
        release
            .rig
            .set("CARGO_HOME", cargo_home.to_str().unwrap())
            .set("CARGO_REGISTRIES_VERIFY_REG_TOKEN", "callisto-verify-token")
            // Stable cargo only reads `publish.timeout` (0: do not wait for the index) when it is allowed unstable keys.
            .set("RUSTC_BOOTSTRAP", "1")
            .set("__CALLISTO_TEST_SLEEP_SCALE", "0.1");
        Self {
            release,
            port,
            registry_dir,
        }
    }

    fn registry(&self, refuse_after: u32, index_delay: Duration) -> ServerGuard {
        start_sparse_cargo_registry(
            self.release.external.path(),
            "cargo",
            self.port,
            &self.registry_dir,
            refuse_after,
            index_delay,
        )
    }

    /// The crate files the registry holds.
    fn crates(&self) -> Vec<String> {
        files_in(&self.registry_dir.join("crates"))
    }
}

/// A released workspace of two independent crates with a committed `Cargo.lock`.
fn cargo_release_fixture(registries: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    git(root, &["remote", "add", "origin", CARGO_ORIGIN]);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(root.join(".gitignore"), "/target/\n").unwrap();
    for name in CARGO_CRATES {
        let crate_dir = root.join("crates").join(name);
        fs::create_dir_all(crate_dir.join("src")).unwrap();
        fs::write(
            crate_dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
                 description = \"verify-real probe\"\nlicense = \"MIT\"\n"
            ),
        )
        .unwrap();
        fs::write(crate_dir.join("src/lib.rs"), "pub fn probe() {}\n").unwrap();
    }
    let lock = std::process::Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(root)
        .output()
        .expect("cargo must be runnable");
    assert!(
        lock.status.success(),
        "lockfile failed: {}",
        String::from_utf8_lossy(&lock.stderr)
    );
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial workspace"]);

    let init = callisto(root, &["init", "--yes", "--versioning", "independent"]);
    assert!(init.status.success(), "init failed: {}", stderr_of(&init));
    let config_path = root.join("callisto.toml");
    let mut config = fs::read_to_string(&config_path).unwrap();
    for name in CARGO_CRATES {
        config.push_str(&format!(
            "\n[[package]]\nmatch = \"cargo/{name}\"\npublish-to = [\"{CARGO_REGISTRY}\"]\n"
        ));
    }
    config.push_str(registries);
    fs::write(&config_path, config).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "configure callisto"]);

    let mut add_args = vec!["add".to_owned()];
    for name in CARGO_CRATES {
        add_args.extend(["--package".to_owned(), format!("cargo/{name}:minor")]);
    }
    add_args.extend(["--summary".to_owned(), "Ship the verify-real probes".to_owned()]);
    let add_args: Vec<&str> = add_args.iter().map(String::as_str).collect();
    let add = callisto(root, &add_args);
    assert!(add.status.success(), "add failed: {}", stderr_of(&add));
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "add release changeset"]);

    let version = callisto(root, &["version", "--emit-decision", DECISION_PATH]);
    assert!(version.status.success(), "version failed: {}", stderr_of(&version));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "release the probes 0.2.0"]);
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &release_commit]);
    (dir, release_commit)
}

#[test]
#[ignore = "needs local registries; run via `just verify-real`"]
fn a_partial_cargo_publish_is_adopted_by_the_rerun_without_republishing() {
    let cargo = CargoRelease::new();
    let release = &cargo.release;

    let first_published = {
        // The registry takes one crate, then refuses: the run dies between the two publishes.
        let _registry = cargo.registry(1, Duration::ZERO);
        let output = release.execute();
        assert!(
            !output.status.success(),
            "the registry refuses the second crate, so the run must fail\n{}",
            release.failure_context(&output)
        );
        assert!(!release.receipt.exists(), "a failed run must leave no terminal receipt");
        let published = cargo.crates();
        assert_eq!(
            published.len(),
            1,
            "exactly one crate published before the failure: {published:?}"
        );
        published
    };

    let _registry = cargo.registry(u32::MAX, Duration::ZERO);
    let second = release.execute();
    release.assert_completed(&second);
    assert!(
        stderr_of(&second).contains("already published"),
        "the rerun must adopt the first crate instead of publishing it again:\n{}",
        stderr_of(&second)
    );
    let all = cargo.crates();
    assert_eq!(all.len(), 2, "both crates must be on the registry: {all:?}");
    assert!(all.contains(&first_published[0]));
    for name in CARGO_CRATES {
        assert!(
            release.remote_has_tag(&format!("{name}@0.2.0")),
            "{name} tag must reach the remote"
        );
    }
}

#[test]
#[ignore = "needs local registries; run via `just verify-real`"]
fn a_cargo_release_waits_for_a_delayed_index() {
    let cargo = CargoRelease::new();
    let release = &cargo.release;
    let registry = cargo.registry(u32::MAX, Duration::from_millis(2500));

    let output = release.execute();
    release.assert_completed(&output);
    assert_eq!(cargo.crates().len(), 2);

    // `cargo info` also asks for the underscore spelling of the name and `cargo publish`'s own wait does not, so a 404
    // for it after the publish is callisto looking at the index while the entry is still hidden.
    let log = registry.log();
    let lines: Vec<&str> = log.lines().collect();
    let published = lines
        .iter()
        .position(|line| *line == format!("published {} 0.2.0", CARGO_CRATES[0]))
        .expect("the first crate must be published");
    let hidden_look = format!(
        "\"GET /index/{}/{}/{} HTTP/1.1\" 404 -",
        &CARGO_CRATES[0][..2],
        &CARGO_CRATES[0][2..4],
        CARGO_CRATES[0].replace('-', "_")
    );
    assert!(
        lines[published..].contains(&hidden_look.as_str()),
        "callisto must have looked at the index before the entry showed:\n{log}"
    );
}

#[test]
#[ignore = "needs local registries; run via `just verify-real`"]
fn a_cargo_remote_tag_on_another_commit_stops_the_release_with_a_conflict() {
    let cargo = CargoRelease::new();
    let release = &cargo.release;
    let _registry = cargo.registry(u32::MAX, Duration::ZERO);
    let other = git(release.root(), &["rev-parse", "HEAD~1"]);
    let tag = format!("{}@0.2.0", CARGO_CRATES[1]);
    git(
        release.root(),
        &[
            "push",
            &format!("file://{}", release.bare.display()),
            &format!("{other}:refs/tags/{tag}"),
        ],
    );

    let output = release.execute();
    release.assert_failed_with(&output, "E167");
    assert_eq!(
        git(&release.bare, &["rev-parse", &format!("refs/tags/{tag}^{{commit}}")]),
        other,
        "the foreign tag must be left as it was"
    );
}

// ----------------------------------------------------------------- Changesets

/// A real `@changesets/cli`. The 3.x and 2.x lines differ in `init` (3.x asks questions on a TTY) and in `pre.json`.
struct Changesets {
    bin: PathBuf,
    major: u32,
}

impl Changesets {
    /// The pinned 3.x on `PATH` (installed by `just verify-real-tools`) and the pinned 2.x named by `VERIFY_REAL_CHANGESETS_2`.
    fn pinned() -> [Self; 2] {
        let v3 = find_on_path("changeset").expect("changeset 3.x must be on PATH: run `just verify-real-tools`");
        let v2 = std::env::var_os("VERIFY_REAL_CHANGESETS_2")
            .expect("VERIFY_REAL_CHANGESETS_2 must name the changeset 2.x binary: run `just verify-real`");
        [Self::new(v3, 3), Self::new(PathBuf::from(v2), 2)]
    }

    fn new(bin: PathBuf, major: u32) -> Self {
        let output = std::process::Command::new(&bin)
            .arg("--version")
            .output()
            .unwrap_or_else(|error| panic!("{} must run: {error}", bin.display()));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let version = stdout.lines().last().unwrap_or_default().trim();
        assert!(
            version.starts_with(&format!("{major}.")),
            "expected changeset {major}.x, got {version:?}"
        );
        Self { bin, major }
    }

    fn run(&self, root: &Path, args: &[&str]) {
        let output = std::process::Command::new(&self.bin)
            .args(args)
            .current_dir(root)
            .output()
            .expect("changeset must be runnable");
        assert!(
            output.status.success(),
            "changeset {}.x {args:?} failed: {}{}",
            self.major,
            String::from_utf8_lossy(&output.stdout),
            stderr_of(&output)
        );
    }

    /// `changeset init`, then `fixed` set to `fixed`.
    fn init(&self, root: &Path, fixed: &[&[&str]]) {
        let path = root.join(".changeset/config.json");
        if self.major == 2 {
            self.run(root, &["init"]);
        } else {
            fs::create_dir_all(root.join(".changeset")).unwrap();
            let defaults = serde_json::json!({
                "changelog": "@changesets/cli/changelog",
                "commit": false,
                "fixed": [],
                "linked": [],
                "access": "restricted",
                "baseBranch": "main",
                "updateInternalDependencies": "patch",
                "ignore": []
            });
            write_json(&path, &defaults);
        }
        let mut config: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        config["fixed"] = serde_json::json!(fixed);
        write_json(&path, &config);
    }
}

fn write_json(path: &Path, value: &serde_json::Value) {
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(value).unwrap())).unwrap();
}

fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", message]);
}

/// A committed npm workspace of `(name, version, dependency)` packages. Changesets 3.x only sees the workspace
/// when a lockfile is present.
fn npm_workspace(packages: &[(&str, &str, Option<&str>)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    let name = "changesets-differential";
    write_json(
        &root.join("package.json"),
        &serde_json::json!({"name": name, "private": true, "workspaces": ["packages/*"]}),
    );
    write_json(
        &root.join("package-lock.json"),
        &serde_json::json!({"name": name, "lockfileVersion": 3, "requires": true, "packages": {}}),
    );
    for (package, version, dependency) in packages {
        let mut manifest = serde_json::json!({"name": package, "version": version});
        if let Some(dependency) = dependency {
            manifest["dependencies"][*dependency] = serde_json::json!("^1.0.0");
        }
        let directory = root.join("packages").join(package);
        fs::create_dir_all(&directory).unwrap();
        write_json(&directory.join("package.json"), &manifest);
    }
    commit(root, "workspace");
    dir
}

fn note(root: &Path, id: &str, bumps: &[(&str, &str)]) {
    let frontmatter: String = bumps
        .iter()
        .map(|(package, bump)| format!("\"{package}\": {bump}\n"))
        .collect();
    fs::write(
        root.join(format!(".changeset/{id}.md")),
        format!("---\n{frontmatter}---\n\n{id}\n"),
    )
    .unwrap();
}

fn copy_workspace(root: &Path) -> tempfile::TempDir {
    let copy = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("cp")
        .arg("-R")
        .arg(format!("{}/.", root.display()))
        .arg(copy.path())
        .status()
        .unwrap();
    assert!(status.success(), "copying the workspace failed");
    copy
}

type Versions = BTreeMap<String, String>;

fn manifest_versions(root: &Path) -> Versions {
    fs::read_dir(root.join("packages"))
        .unwrap()
        .map(|entry| {
            let manifest = fs::read_to_string(entry.unwrap().path().join("package.json")).unwrap();
            let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
            (
                manifest["name"].as_str().unwrap().to_owned(),
                manifest["version"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The versions `callisto version --dry-run` would leave: the manifests' versions with each planned bump applied.
fn callisto_planned_versions(root: &Path) -> Versions {
    let output = callisto(root, &["version", "--dry-run"]);
    assert!(
        output.status.success(),
        "version --dry-run failed: {}",
        stderr_of(&output)
    );
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let mut versions = manifest_versions(root);
    for bump in plan["bumps"].as_array().unwrap() {
        let package = bump["package"].as_str().unwrap();
        let package = package
            .strip_prefix("npm/")
            .expect("every package in the fixture is npm");
        versions.insert(package.to_owned(), bump["to"].as_str().unwrap().to_owned());
    }
    versions
}

/// Changesets' own release plan for a fixed group, reproduced by callisto after `init` over the same workspace.
/// `init` carries the `fixed` group over into `callisto.toml`.
#[test]
#[ignore = "needs the real changeset CLI; run via `just verify-real`"]
fn a_changesets_fixed_group_release_plan_is_reproduced_after_migration() {
    for changesets in Changesets::pinned() {
        let major = changesets.major;
        let dir = npm_workspace(&[
            ("alpha", "1.0.0", None),
            ("beta", "1.2.0", None),
            ("gamma", "0.3.0", Some("alpha")),
        ]);
        let root = dir.path();
        changesets.init(root, &[&["alpha", "beta"]]);
        note(root, "one", &[("alpha", "minor")]);
        note(root, "two", &[("gamma", "patch")]);
        commit(root, "changesets");

        let init = callisto(root, &["init", "--yes"]);
        assert!(init.status.success(), "init failed: {}", stderr_of(&init));
        let config = fs::read_to_string(root.join("callisto.toml")).unwrap();
        assert!(
            config.contains("[[fixed-group]]") && config.contains("npm/alpha") && config.contains("npm/beta"),
            "init must write the fixed group: {config}"
        );
        commit(root, "callisto");

        let planned = callisto_planned_versions(root);
        changesets.run(root, &["version"]);
        let real = manifest_versions(root);
        assert_ne!(real["beta"], "1.2.0", "changeset {major}.x must bump beta with alpha");
        assert_eq!(planned, real, "callisto's plan differs from changeset {major}.x");
    }
}

/// callisto continues a pre cycle Changesets started, and exits it, with the versions Changesets itself would give.
#[test]
#[ignore = "needs the real changeset CLI; run via `just verify-real`"]
fn a_changesets_pre_cycle_is_continued_and_exited_by_callisto() {
    for changesets in Changesets::pinned() {
        let major = changesets.major;
        let dir = npm_workspace(&[("alpha", "1.0.0", None), ("beta", "2.0.0", None)]);
        let root = dir.path();
        changesets.init(root, &[]);
        commit(root, "changesets");
        changesets.run(root, &["pre", "enter", "beta"]);
        note(root, "one", &[("alpha", "minor")]);
        changesets.run(root, &["version"]);
        note(root, "two", &[("alpha", "patch"), ("beta", "minor")]);
        commit(root, "mid-cycle");

        let next = callisto_planned_versions(root);
        let reference = copy_workspace(root);
        changesets.run(reference.path(), &["version"]);
        assert_eq!(next["alpha"], "1.1.0-beta.1", "callisto must continue the beta counter");
        assert_eq!(
            next,
            manifest_versions(reference.path()),
            "the next beta differs from changeset {major}.x"
        );

        let reference = copy_workspace(root);
        changesets.run(reference.path(), &["pre", "exit"]);
        changesets.run(reference.path(), &["version"]);
        let exiting = copy_workspace(root);
        let exit = callisto(exiting.path(), &["pre", "exit"]);
        assert!(exit.status.success(), "pre exit failed: {}", stderr_of(&exit));
        assert_eq!(
            callisto_planned_versions(exiting.path()),
            manifest_versions(reference.path()),
            "the stable release after exit differs from changeset {major}.x"
        );
    }
}
