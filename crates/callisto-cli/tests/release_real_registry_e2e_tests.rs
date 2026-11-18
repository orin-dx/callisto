#![cfg(target_os = "linux")]

//! Real-registry e2e coverage. Linux only: CI installs the registries there.
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
//! rejects non-https URLs with no loopback exception, by design, so a local `http://127.0.0.1` test
//! registry cannot be configured as a `[registries]` entry. `release_verify_real.rs` binds local `https://`
//! registries through `[registries]` instead. Leaving the
//! package unmatched by any `[registries]` override keeps
//! `PreparedRegistryBinding.endpoint` at `None`, which is also what makes
//! this reachable at all: both `npm view` (observation) and `npm publish`
//! omit `--registry` whenever `endpoint` is `None`, so npm resolves the
//! registry itself from `.npmrc` -- unlike PyPI's observation step, which
//! hardcodes the public simple index when `endpoint` is `None` (see the PyPI
//! note below).
//!
//! PyPI drives `registry_argv::pypi_build_argv` and `pypi_upload_argv` directly (the exact
//! production build and upload argv, including `--repository-url` for a
//! private index) rather than through `callisto release execute`: that CLI
//! path routes a registry through `PreparedRegistryBinding`, whose
//! `canonical_registry_url` rejects non-https with no loopback exception --
//! by design -- so a callisto
//! registry binding can never target a local `http://127.0.0.1` test index.
//! Calling those directly stays on the same side of that line
//! as the npm test above: real registry, real package-manager config
//! (`--repository-url`, not `.pypirc`), zero callisto-side registry binding.
//! The upload argv never carries `--skip-existing`, which twine 5 and newer refuses to run
//! at all against a non-warehouse repository URL (`UnsupportedConfiguration`).
//!
//! Cargo drives `registry_argv::cargo_publish_argv` directly against a real
//! local `cargo-http-registry`, for a third, different reason than PyPI's:
//! `cargo publish --registry <key>` and `cargo info --registry <key>` never
//! carry a URL at all, only the registry's *name* -- the URL lives entirely
//! in `.cargo/config.toml`, which callisto never writes. So the one
//! `PreparedRegistryBinding` requirement that would block this (an `https`
//! callisto-side binding) never applies: a `[registries]` entry with a
//! `kind` and no `url` leaves `endpoint` at `None` and the key still reaches
//! `cargo` as `--registry local-e2e`. Driving `cargo_publish_argv` directly
//! here (rather than through `callisto release execute`) keeps this test
//! independent of the git-remote scheme check (`https`/`ssh` only), which is
//! an orthogonal gate on the *release commit's* origin, not on the registry.
//! `cargo-http-registry` serves a git-protocol index (not sparse), so
//! `.cargo/config.toml` also needs `net.git-fetch-with-cli = true` --
//! libgit2's HTTP client rejects the server's dumb-protocol content type.

#[path = "common/local_registries.rs"]
mod local_registries;
#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use callisto_graph::commands::registry_argv::{
    cargo_publish_argv, pypi_build_argv, pypi_upload_argv, Argv, CargoVerification,
};
use callisto_model::{Version, VersionGrammar};
use local_registries::*;
use release_harness::*;

const NPM_PACKAGE: &str = "callisto-e2e-probe";

/// The credential-free pack job's two steps: pack every npm package of the intent, then seal the tarballs. Returns the
/// manifest and directory arguments `release execute` takes.
fn pack_and_seal(root: &Path, intent: &Path, external: &Path) -> Vec<String> {
    let artifacts = external.join("npm-dist");
    fs::create_dir_all(&artifacts).unwrap();
    let manifest = external.join("npm-manifest.json");
    let packed = callisto(
        root,
        &[
            "release",
            "pack-npm",
            "--intent",
            intent.to_str().unwrap(),
            "--artifact-dir",
            artifacts.to_str().unwrap(),
        ],
    );
    assert!(packed.status.success(), "pack-npm failed: {}", stderr_of(&packed));
    let sealed = callisto(
        root,
        &[
            "release",
            "artifact-manifest",
            "--intent",
            intent.to_str().unwrap(),
            "--artifact-dir",
            artifacts.to_str().unwrap(),
            "--seal-npm",
            "--out",
            manifest.to_str().unwrap(),
        ],
    );
    assert!(
        sealed.status.success(),
        "artifact-manifest failed: {}",
        stderr_of(&sealed)
    );
    vec![
        "--artifact-manifest".to_owned(),
        manifest.to_str().unwrap().to_owned(),
        "--artifact-dir".to_owned(),
        artifacts.to_str().unwrap().to_owned(),
    ]
}

/// A missing test tool skips on a developer machine but fails under CI, where the workflow installs every one of them:
/// a skip there would report as a pass and hide that nothing ran.
fn skip_missing(what: &str) {
    assert!(
        std::env::var_os("CI").is_none() || std::env::var_os("CALLISTO_ALLOW_MISSING_TOOLS").is_some(),
        "{what}; the CI workflow must install it"
    );
    eprintln!("SKIPPED real-registry e2e: {what}");
}

/// A real Verdaccio registry, npm routed to it purely via
/// `.npmrc`, `callisto release execute`'s real npm provider adapter, and an
/// assertion against the registry's own package metadata afterward -- not
/// just callisto's exit code.
#[test]
fn npm_publish_against_a_real_verdaccio_registry_is_retrievable_afterward() {
    let Some(_verdaccio) = find_on_path("verdaccio") else {
        skip_missing("npm: `verdaccio` is not installed");
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

    let port = free_loopback_port();
    let _verdaccio_guard = start_verdaccio(external.path(), "npm", port, None);

    // npm's own configuration, not a callisto registry binding: no `[registries]` entry exists, so neither
    // `npm view` nor `npm publish` receives a `--registry` flag from callisto and both fall through to this file.
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
    let sealed = pack_and_seal(root, &intent, external.path());
    let sealed: Vec<&str> = sealed.iter().map(String::as_str).collect();
    let output = execute_rig(root, &intent, &receipt, &rig, &tag, &sealed, None);
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

/// Two npm packages, the second depending on the first, released together against a real Verdaccio: both are
/// published before either is waited for, both reach the registry, and both tags reach the remote.
#[test]
fn a_dependent_npm_package_is_published_with_its_dependency_against_a_real_verdaccio_registry() {
    let Some(_verdaccio) = find_on_path("verdaccio") else {
        skip_missing("npm: `verdaccio` is not installed");
        return;
    };

    let (dir, release_commit) = npm_release_fixture(
        &[
            ("callisto-e2e-dependency", &[]),
            ("callisto-e2e-dependent", &["callisto-e2e-dependency"]),
        ],
        "",
    );
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

    let port = free_loopback_port();
    let _verdaccio_guard = start_verdaccio(external.path(), "npm", port, None);
    fs::write(
        root.join(".npmrc"),
        format!("registry=http://127.0.0.1:{port}/\n//127.0.0.1:{port}/:_authToken=callisto-e2e-test-token\n"),
    )
    .unwrap();

    let bare = bare_remote(external.path());
    let mut rig = Rig::new(external.path(), &release_commit);
    rig.real_git_push(&bare, NPM_ORIGIN);

    let receipt = external.path().join("release-receipt.json");
    let sealed = pack_and_seal(root, &intent, external.path());
    let sealed: Vec<&str> = sealed.iter().map(String::as_str).collect();
    let output = execute_rig(
        root,
        &intent,
        &receipt,
        &rig,
        "callisto-e2e-dependent@0.2.0",
        &sealed,
        None,
    );
    assert!(
        output.status.success(),
        "real-registry npm release execute failed: {}\neffect log:\n{}",
        stderr_of(&output),
        fs::read_to_string(&rig.log).unwrap_or_default(),
    );
    assert!(receipt.exists(), "a successful release must persist a terminal receipt");

    for package in ["callisto-e2e-dependency", "callisto-e2e-dependent"] {
        let metadata_raw = curl_get(&format!("http://127.0.0.1:{port}/{package}"));
        let metadata: serde_json::Value = serde_json::from_str(&metadata_raw)
            .unwrap_or_else(|error| panic!("registry metadata was not JSON: {error}\n{metadata_raw}"));
        assert!(
            metadata["versions"]["0.2.0"].is_object(),
            "{package} 0.2.0 must be retrievable from the registry's own metadata: {metadata}"
        );
        let tag = format!("{package}@0.2.0");
        assert!(
            git(&bare, &["tag", "--list", &tag]).contains(&tag),
            "the {tag} tag must reach the remote"
        );
    }
}

// --------------------------------------------------------------------- pypi

/// Whether `interpreter` can `import module` -- the actual precondition for
/// `pypi_build_argv`'s `python -m build` step, not just the interpreter
/// binary existing.
fn python_module_importable(interpreter: &str, module: &str) -> bool {
    Command::new(interpreter)
        .args(["-c", &format!("import {module}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Mirrors `pypi_build_argv`'s own interpreter resolution (`python3` first,
/// `python` as a fallback), so this test skips exactly when the production
/// code would fail to resolve one.
fn resolved_python() -> Option<&'static str> {
    if find_on_path("python3").is_some() {
        Some("python3")
    } else if find_on_path("python").is_some() {
        Some("python")
    } else {
        None
    }
}

const PYPI_PACKAGE: &str = "callisto-e2e-probe";
const PYPI_VERSION: &str = "0.1.0";

/// A minimal real Python source distribution: just enough for `python -m
/// build --sdist --wheel` to produce real, installable artifacts.
fn pypi_package_fixture(root: &Path, package_name: &str, version: &str) {
    fs::write(
        root.join("pyproject.toml"),
        format!(
            "[build-system]\nrequires = [\"setuptools>=61.0\"]\nbuild-backend = \"setuptools.build_meta\"\n\n\
             [project]\nname = \"{package_name}\"\nversion = \"{version}\"\ndescription = \"e2e probe\"\n\
             requires-python = \">=3.8\"\n"
        ),
    )
    .unwrap();
    let module_dir = root.join(package_name.replace('-', "_"));
    fs::create_dir_all(&module_dir).unwrap();
    fs::write(module_dir.join("__init__.py"), "\n").unwrap();
}

/// Runs an `Argv` exactly as the production `run_argv` does (see
/// `provider/registry.rs`), just without a `CommandRunner` in the way.
fn run_argv(argv: &Argv, extra_env: &[(&str, &str)]) -> std::process::Output {
    let mut command = Command::new(&argv.program);
    command.args(&argv.args).current_dir(&argv.cwd).stdin(Stdio::null());
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.output().unwrap_or_else(|error| {
        panic!(
            "{} {:?} (cwd {}) must be runnable: {error}",
            argv.program,
            argv.args,
            argv.cwd.display()
        )
    })
}

/// A real pypiserver registry, `registry_argv`'s exact production build and
/// upload argv (including `--repository-url`,
/// callisto's own package-manager-config routing, not a registry binding),
/// and an assertion against the registry's own simple index afterward -- not
/// just the upload's exit code.
#[test]
fn pypi_publish_against_a_real_pypiserver_registry_is_retrievable_afterward() {
    if find_on_path("pypi-server").is_none() {
        skip_missing("pypi: `pypi-server` is not installed");
        return;
    }
    let Some(interpreter) = resolved_python() else {
        skip_missing("pypi: neither `python3` nor `python` is on PATH");
        return;
    };
    if !python_module_importable(interpreter, "build") || find_on_path("twine").is_none() {
        skip_missing("pypi: the `build` module or `twine` is not installed");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    pypi_package_fixture(root, PYPI_PACKAGE, PYPI_VERSION);

    let external = tempfile::tempdir().unwrap();
    let port = free_loopback_port();
    let packages_dir = external.path().join("pypiserver-packages");
    let _pypiserver_guard = start_pypiserver(external.path(), "pypi", port, &packages_dir, None);
    let index = format!("http://127.0.0.1:{port}");

    let outdir = tempfile::tempdir().unwrap();
    let build = pypi_build_argv(root, Path::new(""), outdir.path(), 1_700_000_000).unwrap();

    // `--no-isolation`: production omits it, but an isolated `python -m build` would pip-install `setuptools` from
    // PyPI, which CI must not depend on. The CI venv pre-installs pinned `setuptools`/`wheel`; `PIP_NO_INDEX=1`
    // makes an isolation attempt a hard failure instead of a silent fetch.
    let offline_build = Argv {
        program: build.program.clone(),
        args: build
            .args
            .iter()
            .cloned()
            .chain(["--no-isolation".to_string()])
            .collect(),
        cwd: build.cwd.clone(),
    };
    let built = run_argv(&offline_build, &[("PIP_NO_INDEX", "1")]);
    assert!(
        built.status.success(),
        "python -m build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );

    let mut files: Vec<PathBuf> = fs::read_dir(outdir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 2, "the build must produce a wheel and an sdist: {files:?}");
    let upload = pypi_upload_argv(root, Path::new(""), Some(index.as_str()), &files);
    assert!(
        upload.args.contains(&"--repository-url".to_string()) && upload.args.contains(&index),
        "twine must be routed via --repository-url, not a callisto registry binding: {:?}",
        upload.args
    );

    // pypiserver's `-a . -P .` disables authentication, but twine still refuses to run with no credentials
    // configured; this is the package manager's own config, like npm's dummy `_authToken` above.
    let uploaded = run_argv(
        &upload,
        &[("TWINE_USERNAME", "callisto-e2e"), ("TWINE_PASSWORD", "callisto-e2e")],
    );
    assert!(
        uploaded.status.success(),
        "twine upload failed: {}",
        String::from_utf8_lossy(&uploaded.stderr)
    );

    // Independent verification against the registry itself: the published
    // version must be visible in pypiserver's own PEP 691 JSON simple index,
    // not just inferred from twine's exit code.
    let simple_index = curl_get(&format!("http://127.0.0.1:{port}/simple/{PYPI_PACKAGE}/"));
    assert!(
        simple_index.contains(&format!("{PYPI_PACKAGE}-{PYPI_VERSION}"))
            || simple_index.contains(&format!("{}-{PYPI_VERSION}", PYPI_PACKAGE.replace('-', "_"))),
        "published version must be retrievable from the registry's own simple index: {simple_index}"
    );
}

// -------------------------------------------------------------------- cargo

const CARGO_PACKAGE: &str = "callisto-e2e-cargo-probe";
const CARGO_VERSION: &str = "0.1.0";

/// A minimal real crate: just enough for `cargo publish` to package, verify
/// and upload it for real.
fn cargo_package_fixture(root: &Path, package_name: &str, version: &str) {
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{package_name}\"\nversion = \"{version}\"\nedition = \"2021\"\n\
             description = \"e2e probe\"\nlicense = \"MIT\"\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn probe() {}\n").unwrap();
}

/// A real local `cargo-http-registry`, `registry_argv::cargo_publish_argv`'s
/// exact production argv (`--manifest-path`, `--locked`, `--registry`), and
/// an assertion against the registry's own download endpoint afterward --
/// not just `cargo publish`'s exit code.
#[test]
fn cargo_publish_against_a_real_cargo_http_registry_is_retrievable_afterward() {
    if find_on_path("cargo-http-registry").is_none() {
        skip_missing("cargo: `cargo-http-registry` is not installed");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    cargo_package_fixture(root, CARGO_PACKAGE, CARGO_VERSION);

    let external = tempfile::tempdir().unwrap();
    let (_registry_guard, port) = start_cargo_http_registry(external.path(), &external.path().join("registry"));

    // No callisto-side registry binding: the key resolves through `.cargo/config.toml`. `net.git-fetch-with-cli`:
    // this server's index is dumb-HTTP git, which libgit2's client refuses (content-type mismatch).
    fs::create_dir_all(root.join(".cargo")).unwrap();
    fs::write(
        root.join(".cargo/config.toml"),
        format!(
            "[registries.local-e2e]\nindex = \"http://127.0.0.1:{port}/git\"\n\n[net]\ngit-fetch-with-cli = true\n"
        ),
    )
    .unwrap();

    let version = Version::parse(CARGO_VERSION, VersionGrammar::SemVer).unwrap();
    let argv = cargo_publish_argv(
        root,
        Path::new("."),
        CARGO_PACKAGE,
        &version,
        Some("local-e2e"),
        CargoVerification::Build,
    )
    .unwrap();
    assert!(
        argv.args.contains(&"--registry".to_string()) && argv.args.contains(&"local-e2e".to_string()),
        "cargo must be routed via --registry <key>, not a URL: {:?}",
        argv.args
    );

    // A fresh, scratch `CARGO_HOME`: the registry needs no real credential,
    // but `cargo publish` still refuses with none configured at all.
    let cargo_home = external.path().join("cargo-home");
    fs::create_dir_all(&cargo_home).unwrap();
    let published = run_argv(
        &argv,
        &[
            ("CARGO_HOME", cargo_home.to_str().unwrap()),
            ("CARGO_REGISTRIES_LOCAL_E2E_TOKEN", "callisto-e2e-test-token"),
        ],
    );
    assert!(
        published.status.success(),
        "real-registry cargo publish failed: {}",
        String::from_utf8_lossy(&published.stderr)
    );

    // Independent verification against the registry itself: the published
    // crate must be downloadable from the registry's own API, not just
    // inferred from `cargo publish`'s exit code.
    let downloaded = Command::new("curl")
        .args([
            "-sSL",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            &format!("http://127.0.0.1:{port}/api/v1/crates/{CARGO_PACKAGE}/{CARGO_VERSION}/download"),
        ])
        .output()
        .expect("curl must be runnable");
    assert_eq!(
        String::from_utf8_lossy(&downloaded.stdout),
        "200",
        "published crate must be downloadable from the registry's own API"
    );
}
