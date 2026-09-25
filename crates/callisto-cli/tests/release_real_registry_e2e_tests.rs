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
//! registry can never be configured as a `[registries]` entry. Leaving the
//! package unmatched by any `[registries]` override keeps
//! `PreparedRegistryBinding.endpoint` at `None`, which is also what makes
//! this reachable at all: both `npm view` (observation) and `npm publish`
//! omit `--registry` whenever `endpoint` is `None`, so npm resolves the
//! registry itself from `.npmrc` -- unlike PyPI's observation step, which
//! hardcodes the public simple index when `endpoint` is `None` (see the PyPI
//! note below).
//!
//! PyPI drives `registry_argv::pypi_publish_argv` directly (the exact
//! production build-then-upload argv, including `--repository-url` for a
//! private index) rather than through `callisto release execute`: that CLI
//! path routes a registry through `PreparedRegistryBinding`, whose
//! `canonical_registry_url` rejects non-https with no loopback exception --
//! by design -- so a callisto
//! registry binding can never target a local `http://127.0.0.1` test index.
//! Calling `pypi_publish_argv` directly stays on the same side of that line
//! as the npm test above: real registry, real package-manager config
//! (`--repository-url`, not `.pypirc`), zero callisto-side registry binding.
//! `pypi_publish_argv` used to unconditionally append `--skip-existing`,
//! which twine 5 and newer refuses to run at all against a non-warehouse
//! repository URL (`UnsupportedConfiguration`) -- fixed to be conditional;
//! see the `fix(publish)` commit on this branch and the `pypi_publish_argv_*`
//! `--skip-existing` tests in `registry_argv.rs`.

#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use callisto_graph::commands::registry_argv::{pypi_publish_argv, Argv};
use callisto_model::{Version, VersionGrammar};
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
/// leaks it past the test. `log_path` is its combined stdout+stderr, written
/// to a file rather than piped so a chatty server can never fill an
/// undrained pipe and deadlock the test.
struct ServerGuard {
    child: Child,
    log_path: PathBuf,
}

impl ServerGuard {
    fn log(&self) -> String {
        fs::read_to_string(&self.log_path).unwrap_or_default()
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

/// Redirects a spawned command's stdout and stderr to one combined log file
/// under `dir`/`name`, so a startup crash is diagnosable instead of silent.
fn log_to_file(command: &mut Command, dir: &Path, name: &str) -> PathBuf {
    let log_path = dir.join(name);
    let file = fs::File::create(&log_path).expect("server log file must be creatable");
    let file_for_stderr = file.try_clone().expect("log file handle must be cloneable");
    command.stdin(Stdio::null()).stdout(file).stderr(file_for_stderr);
    log_path
}

/// Blocks until `port` accepts a loopback TCP connection, or panics after
/// `timeout` -- including the server's captured output in the panic message
/// either way, and its exit status if it already died. Checking `try_wait`
/// first means a server that crashes at startup fails fast with its real
/// error instead of spinning out the whole timeout with no evidence.
fn wait_for_port(guard: &mut ServerGuard, port: u16, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        if let Ok(Some(status)) = guard.child.try_wait() {
            panic!(
                "server exited with {status} before binding 127.0.0.1:{port}; output:\n{}",
                guard.log()
            );
        }
        if Instant::now() >= deadline {
            panic!(
                "server on 127.0.0.1:{port} never became reachable within {timeout:?}; output:\n{}",
                guard.log()
            );
        }
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
    let mut command = Command::new("verdaccio");
    command.args(["-c", "config.yaml", "-l", &format!("127.0.0.1:{port}")]);
    command.current_dir(&config_dir);
    let log_path = log_to_file(&mut command, external, "verdaccio.log");
    let child = command.spawn().expect("verdaccio must be spawnable");
    let mut guard = ServerGuard { child, log_path };
    wait_for_port(&mut guard, port, Duration::from_secs(20));
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
    callisto_fixtures::git::init_repo(root);
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

    let init = callisto(root, &["init", "--yes", "--versioning", "independent"]);
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

/// A real Verdaccio registry, npm routed to it purely via
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

// --------------------------------------------------------------------- pypi

/// Whether `interpreter` can `import module` -- the actual precondition for
/// `pypi_publish_argv`'s `python -m build` step, not just the interpreter
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

/// Mirrors `pypi_publish_argv`'s own interpreter resolution (`python3` first,
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

/// Starts a real local `pypiserver` on an ephemeral loopback port with
/// authentication fully disabled (`-a . -P .`, its own documented way to
/// allow anonymous browsing and uploads) and `--disable-fallback`, so a
/// lookup for an unknown project 404s instead of proxying to real PyPI.
///
/// `--server gunicorn`: pypiserver's default `auto` backend selection falls
/// through to `wsgiref` when nothing else is installed, and `wsgiref`
/// (like `paste` and gevent's `pywsgi`, also measured) resolves the bind
/// address via `socket.getfqdn()` before it starts listening -- a reverse
/// DNS lookup that measured 12-35s on a real machine exhibiting the same
/// symptom as the CI macOS runner, against sub-second on Linux. `gunicorn`
/// has no such call anywhere in its source (grepped) and measured
/// consistently under 1s across repeated runs here; it forks a worker and
/// terminates both master and worker cleanly on the one SIGTERM `ServerGuard`
/// sends the master PID.
fn start_pypiserver(external: &Path) -> (ServerGuard, u16) {
    let packages_dir = external.join("pypiserver-packages");
    fs::create_dir_all(&packages_dir).unwrap();
    let port = free_loopback_port();
    let mut command = Command::new("pypi-server");
    command.args([
        "run",
        "--server",
        "gunicorn",
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--authenticate",
        ".",
        "--passwords",
        ".",
        "--disable-fallback",
        packages_dir.to_str().unwrap(),
    ]);
    let log_path = log_to_file(&mut command, external, "pypiserver.log");
    let child = command.spawn().expect("pypi-server must be spawnable");
    let mut guard = ServerGuard { child, log_path };
    wait_for_port(&mut guard, port, Duration::from_secs(20));
    (guard, port)
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

/// A real pypiserver registry, `registry_argv::pypi_publish_argv`'s
/// exact production build-then-upload argv (including `--repository-url`,
/// callisto's own package-manager-config routing, not a registry binding),
/// and an assertion against the registry's own simple index afterward -- not
/// just the upload's exit code.
#[test]
fn pypi_publish_against_a_real_pypiserver_registry_is_retrievable_afterward() {
    if find_on_path("pypi-server").is_none() {
        eprintln!("SKIPPED pypi real-registry e2e: `pypi-server` is not installed");
        return;
    }
    let Some(interpreter) = resolved_python() else {
        eprintln!("SKIPPED pypi real-registry e2e: neither `python3` nor `python` is on PATH");
        return;
    };
    if !python_module_importable(interpreter, "build") || find_on_path("twine").is_none() {
        eprintln!("SKIPPED pypi real-registry e2e: the `build` module or `twine` is not installed");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    pypi_package_fixture(root, PYPI_PACKAGE, PYPI_VERSION);

    let external = tempfile::tempdir().unwrap();
    let (_pypiserver_guard, port) = start_pypiserver(external.path());
    let index = format!("http://127.0.0.1:{port}");

    let version = Version::parse(PYPI_VERSION, VersionGrammar::SemVer).unwrap();
    let steps = pypi_publish_argv(root, Path::new(""), PYPI_PACKAGE, &version, Some(index.as_str())).unwrap();
    let [build, upload] = steps.as_slice() else {
        panic!("pypi_publish_argv must return exactly a build and an upload step");
    };
    assert!(
        upload.args.contains(&"--repository-url".to_string()) && upload.args.contains(&index),
        "twine must be routed via --repository-url, not a callisto registry binding: {:?}",
        upload.args
    );

    // `--no-isolation`: production `pypi_publish_argv` deliberately omits it (a real release
    // wants a clean, isolated build), but a plain `python -m build` here would pip-install
    // `setuptools` from PyPI into a fresh isolated env -- network access CI must not depend on.
    // The CI venv (see callisto-ci.yml) pre-installs pinned `setuptools`/`wheel`, so building
    // against it directly, unisolated, needs nothing from the network; `PIP_NO_INDEX=1` makes
    // that a hard failure instead of a silent fetch if isolation is ever attempted anyway.
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

    // pypiserver's `-a . -P .` disables authentication entirely, but twine's
    // own client still refuses to run with no credentials configured at
    // all -- the package manager's own config, same as npm's dummy
    // `_authToken` above, not a callisto concern.
    let uploaded = run_argv(
        upload,
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
