//! Black-box protocol verification for the callisto moon extension.
//!
//! Unlike the type-level `cargo check --target wasm32-wasip1 --features pdk`
//! gate, this drives the *compiled* `callisto-moon.wasm` module through a
//! real wasmtime-backed Extism plugin container (via `moon_pdk_test_utils` /
//! `warpgate`), exercising the actual `register_extension` and
//! `execute_extension` wire calls moon itself would make.
//!
//! `moon_pdk_test_utils::find_wasm_file()` looks for a file literally named
//! `{CARGO_PKG_NAME}.wasm` (i.e. `callisto-moon.wasm`, hyphenated), but
//! cargo's own wasm32 cdylib output is `callisto_moon.wasm` (underscored).
//! `resolve_wasm_file` below builds the module (if needed) and stages a
//! correctly-named copy under `WARPGATE_PLUGINS_DIR` so discovery succeeds.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

use moon_pdk_test_utils::{
    create_empty_moon_sandbox, ExecuteExtensionInput, RegisterExtensionInput,
};

static BUILD_WASM: Once = Once::new();

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("callisto-moon manifest dir has no workspace-root ancestor")
        .to_path_buf()
}

/// Ensures `callisto-moon.wasm` (hyphenated, as `find_wasm_file()` expects)
/// exists in a directory pointed to by `WARPGATE_PLUGINS_DIR`, building the
/// real wasm32-wasip1 cdylib artifact first if it isn't already present.
fn resolve_wasm_file() {
    BUILD_WASM.call_once(|| {
        let root = workspace_root();
        let built = root.join("target/wasm32-wasip1/debug/callisto_moon.wasm");

        if !built.exists() {
            // `--crate-type cdylib` is passed explicitly here rather than
            // declared in Cargo.toml's [lib] section: a manifest-level
            // cdylib crate-type would make cargo also attempt (and fail) a
            // native cdylib build whenever the "pdk" feature is enabled
            // natively (e.g. `cargo llvm-cov --all-features`), since pdk's
            // code references extism-pdk's WASM-host-only import symbols.
            let status = Command::new("cargo")
                .args([
                    "rustc",
                    "-p",
                    "callisto-moon",
                    "--lib",
                    "--target",
                    "wasm32-wasip1",
                    "--features",
                    "pdk",
                    "--crate-type",
                    "cdylib",
                ])
                .current_dir(&root)
                .status()
                .expect("failed to invoke cargo rustc for callisto-moon.wasm fixture");

            assert!(
                status.success(),
                "cargo rustc --target wasm32-wasip1 --features pdk --crate-type cdylib failed"
            );
        }

        assert!(
            built.exists(),
            "expected wasm artifact at {} after build",
            built.display()
        );

        // `find_wasm_file_with_name` treats `WARPGATE_PLUGINS_DIR` as a
        // pseudo workspace root and appends `wasm32-wasip1/<profile>/` (or
        // `target/wasm32-wasip1/<profile>/`) itself, so the staged file
        // must live a further two directories down, not directly inside it.
        let plugins_root = root.join("target/.callisto-moon-plugin-fixture");
        let staged_dir = plugins_root.join("wasm32-wasip1/debug");
        fs::create_dir_all(&staged_dir).expect("failed to create staged plugin dir");

        let staged_file = staged_dir.join("callisto-moon.wasm");
        fs::copy(&built, &staged_file).expect("failed to stage callisto-moon.wasm fixture");

        // SAFETY: single-threaded `Once` setup that runs before any sandbox
        // is created; no other code in this test binary reads/writes env
        // vars concurrently with this call.
        unsafe {
            std::env::set_var("WARPGATE_PLUGINS_DIR", &plugins_root);
        }
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn register_extension_matches_moon_wire_protocol() {
    resolve_wasm_file();

    let sandbox = create_empty_moon_sandbox();
    let ext = sandbox.create_extension("callisto").await;

    let output = ext
        .register_extension(RegisterExtensionInput {
            id: moon_pdk_test_utils::Id::raw("callisto"),
        })
        .await;

    assert_eq!(output.name, "callisto");
    assert_eq!(output.plugin_version, env!("CARGO_PKG_VERSION"));
}

/// `define_extension_config` has no `ExtensionTestWrapper` convenience
/// method (unlike `register_extension`/`execute_extension`), so this drives
/// `ext.plugin.call_func_with` directly, mirroring the wire-boundary tests
/// further down this file (e.g. `register_extension_malformed_json_fails_cleanly_not_panics`).
/// Asserts the real moon wire type comes back well-formed: an empty-struct
/// `schematic::SchemaType::Struct` with no fields, matching
/// `define_extension_config`'s documented intent (callisto has no moon-config
/// keys to declare -- real settings live in `callisto.toml`).
#[tokio::test(flavor = "multi_thread")]
async fn define_extension_config_matches_moon_wire_protocol() {
    resolve_wasm_file();

    let sandbox = create_empty_moon_sandbox();
    let ext = sandbox.create_extension("callisto").await;

    let output: moon_pdk_test_utils::DefineExtensionConfigOutput = ext
        .plugin
        .call_func_with("define_extension_config", ())
        .await
        .expect("define_extension_config wire call must succeed");

    assert!(
        matches!(
            output.schema.ty,
            schematic_types::SchemaType::Struct(ref s) if s.fields.is_empty()
        ),
        "expected an empty struct schema, got: {:?}",
        output.schema.ty
    );
}

/// Pure wire-survival smoke test: proves the Extism host<->guest call for
/// `execute_extension` marshals successfully (no panic/trap) for the most
/// basic possible input. This asserts NOTHING about the response's
/// *content* -- `execute_extension` is infallible at the wire level
/// (`ExecuteExtensionOutput` always returns some value, with `exit_code`
/// carrying failure rather than the call itself erroring), and
/// `call_func_without_output` discards the output entirely. It would pass
/// identically whether the underlying command succeeded, failed cleanly, or
/// returned garbage.
///
/// For a test that drives this same "basic args" (`args: ["status"]`) case
/// and actually asserts something about the resulting report, see
/// `execute_extension_no_git_repo_succeeds_with_no_last_tag_not_panic` below
/// (same args, but against a fixture with no `.git` directory, and it
/// asserts on `exit_code`/`report`) and
/// `execute_extension_status_succeeds_against_real_repo_fixture` (asserts a
/// real success shape).
#[tokio::test(flavor = "multi_thread")]
async fn execute_extension_wire_call_survives_basic_args_smoke_test() {
    resolve_wasm_file();

    let sandbox = create_empty_moon_sandbox();
    let ext = sandbox.create_extension("callisto").await;

    ext.execute_extension(ExecuteExtensionInput {
        args: vec!["status".to_string()],
        ..Default::default()
    })
    .await;
}

/// Restores a previous `PATH` value on drop, so a test that temporarily
/// prepends a fake `moon` shim to `PATH` (see
/// `execute_extension_status_succeeds_against_real_repo_fixture` below)
/// can't leak that mutation into other tests in this binary even on panic.
///
/// `PATH` is global, process-wide mutable state, and `cargo test` runs
/// `#[tokio::test]` functions from the same test binary concurrently by
/// default (separate OS threads sharing one process) -- so every
/// PATH-mutating test in this file must also serialize against every other
/// one, or they race and corrupt each other's `PATH` mid-test (observed
/// empirically: running this suite without `--test-threads=1` produced
/// spurious failures from one test's fixture directory bleeding into
/// another's assertions). `PATH_MUTEX` provides that serialization;
/// `PathGuard` holds the lock for its entire lifetime, only releasing it
/// (in `Drop`, after `PATH` has been restored) once the owning test is
/// done with it.
static PATH_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct PathGuard {
    previous: Option<std::ffi::OsString>,
    _lock: tokio::sync::MutexGuard<'static, ()>,
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        // SAFETY: serialized by `PATH_MUTEX` (held by `self._lock` for this
        // guard's entire lifetime, released only after this restore runs),
        // so no other code in this binary can be concurrently
        // reading/writing `PATH` while this executes.
        unsafe {
            match &self.previous {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
    }
}

/// Prepends `dir` to `PATH` and returns a guard that restores the previous
/// value when dropped.
async fn prepend_to_path(dir: &Path) -> PathGuard {
    let lock = PATH_MUTEX.lock().await;
    let previous = std::env::var_os("PATH");
    let mut new_path = std::ffi::OsString::from(dir);
    if let Some(existing) = &previous {
        new_path.push(":");
        new_path.push(existing);
    }
    // SAFETY: see `PathGuard::drop` -- serialized via `PATH_MUTEX`, held
    // from here until the returned guard is dropped.
    unsafe {
        std::env::set_var("PATH", &new_path);
    }
    PathGuard {
        previous,
        _lock: lock,
    }
}

/// Replaces `PATH` entirely with exactly `dirs` (no inheriting the previous
/// value) and returns a guard that restores the previous value when
/// dropped. Unlike `prepend_to_path`, this is for tests that need to
/// guarantee a binary is *absent* -- prepending can't do that, since
/// anything already on the ambient test-runner `PATH` (e.g. a real `moon`
/// or `git` install) would still be found.
async fn set_path_to(dirs: &[&Path]) -> PathGuard {
    let lock = PATH_MUTEX.lock().await;
    let previous = std::env::var_os("PATH");
    let joined =
        std::env::join_paths(dirs.iter().map(|d| d.as_os_str())).expect("failed to join PATH");
    // SAFETY: see `PathGuard::drop` -- serialized via `PATH_MUTEX`, held
    // from here until the returned guard is dropped.
    unsafe {
        std::env::set_var("PATH", &joined);
    }
    PathGuard {
        previous,
        _lock: lock,
    }
}

/// Locates the real `which` binary on the ambient (unrestricted) test-runner
/// `PATH`, before any test narrows `PATH` down.
///
/// This crate's `MoonCommandRunner` (the `pdk`-feature impl) pre-checks
/// whether a program exists via `warpgate_pdk::command_exists`, which
/// itself shells out to `which` (see `runner.rs`'s doc comment on why: a
/// program genuinely missing from `PATH` makes warpgate's real
/// `exec_command` host function abort the entire plugin call rather than
/// returning a catchable error, so `command_exists`/`which` is used to
/// sidestep ever calling `exec` for a program known to be absent). Tests
/// that restrict `PATH` to prove some *other* program is absent must still
/// leave a working `which` on `PATH`, or they'd instead be proving `which`
/// is absent -- a different, uninteresting case.
fn locate_which_binary() -> PathBuf {
    let output = Command::new("which").arg("which").output().expect(
        "failed to invoke `which` to locate itself; `which` must be installed to run this test",
    );
    assert!(
        output.status.success(),
        "`which which` must succeed for this test to isolate a `which`-only PATH entry"
    );
    PathBuf::from(
        String::from_utf8(output.stdout)
            .expect("`which which` output must be valid UTF-8")
            .trim()
            .to_string(),
    )
}

/// Creates a tempdir containing nothing but a symlink to the real `which`
/// binary, for use with `set_path_to` in tests that need `command_exists`
/// (see `locate_which_binary`'s doc comment) to keep working while some
/// other specific program is made unreachable.
fn create_which_only_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("failed to create which-only tempdir");
    let which_path = locate_which_binary();
    std::os::unix::fs::symlink(&which_path, dir.path().join("which"))
        .expect("failed to symlink `which` into the which-only tempdir");
    dir
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git must be installed to run this test");
    assert!(status.success(), "git {args:?} failed in {dir:?}");
}

/// Writes an executable fake `moon` binary to `dir/moon` that answers
/// `moon project-graph --json` with a single project rooted at the
/// workspace root (`.`) and no dependencies -- just enough for
/// `MoonProjectLocator` to discover the fixture package without requiring a
/// real, fully-configured moon workspace.
fn write_fake_moon_binary(dir: &Path) -> PathBuf {
    let script = dir.join("moon");
    fs::write(
        &script,
        "#!/bin/sh\n\
         if [ \"$1\" = \"project-graph\" ]; then\n\
         echo '{\"projects\":[{\"root\":\".\",\"depends_on\":[]}]}'\n\
         exit 0\n\
         fi\n\
         exit 1\n",
    )
    .expect("failed to write fake moon script");

    let mut perms = fs::metadata(&script).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    fs::set_permissions(&script, perms).unwrap();

    script
}

/// Writes an executable fake `moon` binary to `dir/moon` that answers `moon
/// project-graph --json` with exit code 0 but unparseable garbage on
/// stdout, simulating a `moon` install that is present and "succeeds" (exit
/// 0) but whose output `MoonProjectLocator` cannot make sense of --
/// distinct from `moon` being absent/erroring outright.
fn write_fake_moon_binary_malformed_output(dir: &Path) -> PathBuf {
    let script = dir.join("moon");
    fs::write(
        &script,
        "#!/bin/sh\n\
         if [ \"$1\" = \"project-graph\" ]; then\n\
         echo 'this is not valid json {{{'\n\
         exit 0\n\
         fi\n\
         exit 1\n",
    )
    .expect("failed to write fake moon script");

    let mut perms = fs::metadata(&script).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    fs::set_permissions(&script, perms).unwrap();

    script
}

/// Regression test for the bug this task fixes: `TagIndex::build` (via
/// `callisto_graph::tags::last_tag_for`) used to call
/// `callisto_vcs::GitRepository::discover(root)?` unconditionally, which
/// always returns `Err` on `wasm32` (gix is excluded from that target's
/// dependency set). That made `Workspace::load` -- and therefore every
/// moon-invoked subcommand, including `status` -- fail on the very first
/// package for any real wasm32/moon invocation.
///
/// Unlike `execute_extension_runs_without_erroring_on_basic_args` above,
/// which only proves the Extism wire call itself didn't trap, this drives
/// `execute_extension` against a real fixture repo (a real `git init` +
/// tagged commit, staged inside the sandbox's preopened workspace
/// directory, with a fake `moon` binary shimmed onto `PATH` so
/// `MoonProjectLocator` can resolve the one fixture package without a full
/// moon workspace) and asserts the *response* -- exit code, report shape --
/// actually indicates success, not just "didn't trap". Before the fix, this
/// fails with `exit_code == 1` and an `E_GRAPH` error carrying the
/// `RepoNotFound` message; after the fix it succeeds and reports the tag
/// this test creates.
#[tokio::test(flavor = "multi_thread")]
async fn execute_extension_status_succeeds_against_real_repo_fixture() {
    resolve_wasm_file();

    let sandbox = create_empty_moon_sandbox();
    let root = sandbox.root.clone();

    run_git(&root, &["init", "-q"]);
    run_git(&root, &["config", "user.email", "test@example.com"]);
    run_git(&root, &["config", "user.name", "Test"]);

    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .expect("failed to write fixture Cargo.toml");

    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-q", "-m", "initial commit"]);
    run_git(
        &root,
        &[
            "-c",
            "tag.gpgSign=false",
            "tag",
            "-m",
            "release",
            "pkg-a@1.0.0",
        ],
    );

    let fake_bin_dir = tempfile::tempdir().expect("failed to create fake-bin tempdir");
    write_fake_moon_binary(fake_bin_dir.path());
    let _path_guard = prepend_to_path(fake_bin_dir.path()).await;

    let ext = sandbox.create_extension("callisto").await;

    let input = moon_pdk_test_utils::ExecuteExtensionInput {
        args: vec!["status".to_string()],
        context: ext.create_context(),
    };

    let output: callisto_moon::ExecuteExtensionOutput = ext
        .plugin
        .call_func_with("execute_extension", input)
        .await
        .expect("execute_extension wire call must succeed");

    assert_eq!(
        output.exit_code, 0,
        "expected status to succeed (exit_code 0), got report: {:#}",
        output.report
    );
    assert!(
        output.report.get("error").is_none(),
        "expected no error field in report, got: {:#}",
        output.report
    );

    let packages = output.report["packages"]
        .as_array()
        .expect("report.packages must be an array");
    assert_eq!(packages.len(), 1, "expected exactly one package in report");
    assert_eq!(
        packages[0]["lastTag"], "pkg-a@1.0.0",
        "status must resolve the last tag via the CommandRunner fallback \
         since gix is unavailable on wasm32; report: {:#}",
        output.report
    );
}

// ---------------------------------------------------------------------
// Failure-path black-box tests.
//
// Each of these drives `execute_extension` through the real compiled wasm
// module against a deliberately broken environment and asserts the
// *response* is a clean, structured failure (non-zero `exit_code`, a
// populated `report.error`) -- not merely that the Extism wire call itself
// didn't trap. `call_func_with` returning `Ok(ExecuteExtensionOutput { .. })`
// at all, for every scenario below, is itself already evidence supporting
// this crate's "execute_extension never panics; it catches everything into
// the report" claim: a real Rust panic inside the wasm guest surfaces to
// `call_func_with` as an `Err` (a wasmtime trap), not a successfully
// decoded `ExecuteExtensionOutput`. If any of these scenarios actually
// panicked, the `.expect(...)` on the wire call itself would fail, not the
// content assertions after it.
// ---------------------------------------------------------------------

/// Part A #1 (and the strengthened replacement for the old
/// "runs_without_erroring_on_basic_args" test's `args: ["status"]` case):
/// `execute_extension` against a workspace with a working `moon` (so
/// project discovery succeeds) but no `.git` directory at all.
///
/// IMPORTANT finding from writing this test: a missing `.git` directory is
/// NOT actually a failure condition here, contrary to this task's initial
/// assumption. `callisto_graph::tags::fetch_all_tags` (the CommandRunner
/// fallback `TagIndex::build` always takes on `wasm32`, per the doc comment
/// on `execute_extension_status_succeeds_against_real_repo_fixture` above)
/// calls `runner.run("git", ["tag", "--list"], root)?` -- but `?` only
/// short-circuits on the *process itself* failing to spawn, not on a
/// non-zero exit code. When `root` has no `.git` anywhere in its ancestry,
/// real `git tag --list` there exits non-zero ("not a git repository") with
/// empty stdout, which `runner.run` reports as a perfectly normal
/// `Ok(CommandOutput { exit_code: Some(128), stdout: "", .. })` -- so
/// `fetch_all_tags` returns `Ok(vec![])` (no tags), and `status` proceeds
/// to report the package with no `lastTag` and `changedSinceLastTag: true`
/// (unreleased), not an error. This is arguably the *correct*, more robust
/// behavior for a brand new/pre-git package -- so this test asserts that
/// real, graceful-success behavior rather than forcing a fabricated
/// "failure" assertion that doesn't match reality.
#[tokio::test(flavor = "multi_thread")]
async fn execute_extension_no_git_repo_succeeds_with_no_last_tag_not_panic() {
    resolve_wasm_file();

    let sandbox = create_empty_moon_sandbox();
    let root = sandbox.root.clone();

    // Deliberately no `git init` here -- that's the point of this test.
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .expect("failed to write fixture Cargo.toml");

    let fake_bin_dir = tempfile::tempdir().expect("failed to create fake-bin tempdir");
    write_fake_moon_binary(fake_bin_dir.path());
    let _path_guard = prepend_to_path(fake_bin_dir.path()).await;

    let ext = sandbox.create_extension("callisto").await;

    let input = moon_pdk_test_utils::ExecuteExtensionInput {
        args: vec!["status".to_string()],
        context: ext.create_context(),
    };

    let output: callisto_moon::ExecuteExtensionOutput = ext
        .plugin
        .call_func_with("execute_extension", input)
        .await
        .expect("execute_extension wire call must survive a missing .git directory, not trap");

    assert_eq!(
        output.exit_code, 0,
        "a missing .git directory is not a hard failure -- expected success \
         with no last tag, got report: {:#}",
        output.report
    );
    assert!(
        output.report.get("error").is_none(),
        "expected no report.error for a missing .git directory, got: {:#}",
        output.report
    );

    let packages = output.report["packages"]
        .as_array()
        .expect("report.packages must be an array");
    assert_eq!(packages.len(), 1, "expected exactly one package in report");
    assert!(
        packages[0].get("lastTag").is_none() || packages[0]["lastTag"].is_null(),
        "expected no lastTag when there is no .git repo at all, got report: {:#}",
        output.report
    );
    assert_eq!(
        packages[0]["changedSinceLastTag"], true,
        "a package with no discoverable tags should be reported as changed \
         (i.e. unreleased), got report: {:#}",
        output.report
    );
}

/// Part A #2: `execute_extension` when the `moon` host tool is present and
/// exits 0, but its `project-graph --json` output is unparseable garbage
/// rather than valid JSON.
#[tokio::test(flavor = "multi_thread")]
async fn execute_extension_malformed_moon_project_graph_json_fails_cleanly() {
    resolve_wasm_file();

    let sandbox = create_empty_moon_sandbox();

    let fake_bin_dir = tempfile::tempdir().expect("failed to create fake-bin tempdir");
    write_fake_moon_binary_malformed_output(fake_bin_dir.path());
    let _path_guard = prepend_to_path(fake_bin_dir.path()).await;

    let ext = sandbox.create_extension("callisto").await;

    let input = moon_pdk_test_utils::ExecuteExtensionInput {
        args: vec!["status".to_string()],
        context: ext.create_context(),
    };

    let output: callisto_moon::ExecuteExtensionOutput = ext
        .plugin
        .call_func_with("execute_extension", input)
        .await
        .expect(
            "execute_extension wire call must survive malformed `moon project-graph` JSON, not trap",
        );

    assert_ne!(
        output.exit_code, 0,
        "expected a non-zero exit_code when moon's project-graph output is unparseable, got report: {:#}",
        output.report
    );
    assert!(
        output.report.get("error").is_some(),
        "expected a populated report.error when moon's project-graph output is unparseable, got: {:#}",
        output.report
    );
    let message = output.report["error"]["message"]
        .as_str()
        .expect("report.error.message must be a string");
    assert!(
        !message.trim().is_empty(),
        "report.error.message must not be empty, got report: {:#}",
        output.report
    );
}

/// Part A #3: `execute_extension` when the `moon` host tool itself is
/// entirely absent from `PATH` (distinct from present-but-failing).
#[tokio::test(flavor = "multi_thread")]
async fn execute_extension_moon_binary_absent_from_path_fails_cleanly() {
    resolve_wasm_file();

    let sandbox = create_empty_moon_sandbox();

    // Replace PATH outright (not prepend) with a directory that has `which`
    // (needed by `command_exists`'s pre-check, see `locate_which_binary`'s
    // doc comment) but is otherwise guaranteed to contain nothing, so there
    // is no possibility of a real `moon` install elsewhere on the ambient
    // test-runner PATH being found.
    let which_dir = create_which_only_dir();
    let _path_guard = set_path_to(&[which_dir.path()]).await;

    let ext = sandbox.create_extension("callisto").await;

    let input = moon_pdk_test_utils::ExecuteExtensionInput {
        args: vec!["status".to_string()],
        context: ext.create_context(),
    };

    let output: callisto_moon::ExecuteExtensionOutput = ext
        .plugin
        .call_func_with("execute_extension", input)
        .await
        .expect("execute_extension wire call must survive moon being absent from PATH, not trap");

    assert_ne!(
        output.exit_code, 0,
        "expected a non-zero exit_code when moon is absent from PATH, got report: {:#}",
        output.report
    );
    assert!(
        output.report.get("error").is_some(),
        "expected a populated report.error when moon is absent from PATH, got: {:#}",
        output.report
    );
    let message = output.report["error"]["message"]
        .as_str()
        .expect("report.error.message must be a string");
    assert!(
        !message.trim().is_empty(),
        "report.error.message must not be empty, got report: {:#}",
        output.report
    );
}

/// Documents (Part B #2's "unrecognized subcommand" question) what
/// `execute_extension` actually does with a subcommand name it doesn't
/// specifically recognize: falls back to running `status`, exactly like no
/// subcommand at all, rather than producing a distinct error. See
/// `resolve_subcommand`'s doc comment in `src/extension.rs` for the source
/// reading backing this. This drives the real fixture repo (so a
/// non-error, populated report is actually reachable) with a nonsense
/// subcommand name and asserts the response is byte-for-byte the same
/// shape `"status"` would produce, i.e. no panic and no distinct
/// unrecognized-subcommand error path exists today.
#[tokio::test(flavor = "multi_thread")]
async fn execute_extension_unrecognized_subcommand_falls_back_to_status_not_error() {
    resolve_wasm_file();

    let sandbox = create_empty_moon_sandbox();
    let root = sandbox.root.clone();

    run_git(&root, &["init", "-q"]);
    run_git(&root, &["config", "user.email", "test@example.com"]);
    run_git(&root, &["config", "user.name", "Test"]);

    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .expect("failed to write fixture Cargo.toml");

    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-q", "-m", "initial commit"]);
    run_git(
        &root,
        &[
            "-c",
            "tag.gpgSign=false",
            "tag",
            "-m",
            "release",
            "pkg-a@1.0.0",
        ],
    );

    let fake_bin_dir = tempfile::tempdir().expect("failed to create fake-bin tempdir");
    write_fake_moon_binary(fake_bin_dir.path());
    let _path_guard = prepend_to_path(fake_bin_dir.path()).await;

    let ext = sandbox.create_extension("callisto").await;

    let input = moon_pdk_test_utils::ExecuteExtensionInput {
        args: vec!["totally-bogus-subcommand".to_string()],
        context: ext.create_context(),
    };

    let output: callisto_moon::ExecuteExtensionOutput = ext
        .plugin
        .call_func_with("execute_extension", input)
        .await
        .expect("execute_extension wire call must survive an unrecognized subcommand, not trap");

    assert_eq!(
        output.exit_code, 0,
        "unrecognized subcommand should fall back to `status` and succeed, got report: {:#}",
        output.report
    );
    assert!(
        output.report.get("error").is_none(),
        "unrecognized subcommand should not produce a distinct error, got: {:#}",
        output.report
    );
    let packages = output.report["packages"]
        .as_array()
        .expect("report.packages must be an array, same shape `status` produces");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0]["lastTag"], "pkg-a@1.0.0");
}

/// Part C: drives `MoonCommandRunner::run` through the REAL
/// `warpgate_pdk` seam (not a hand-authored string fed to
/// `classify_host_failure` -- see `runner.rs`'s unit tests for that), by
/// making the CommandRunner-fallback `git tag --list` call
/// (`callisto_graph::tags::fetch_all_tags`, always taken on wasm32 since
/// `gix` is excluded from that target) resolve a genuinely absent `git`,
/// and asserts a real `CommandError::NotFound` for `git` surfaces in the
/// report -- which is only observable end-to-end via `GraphError::Command`'s
/// Display text ("`{program}` was not found; ..."), since `locator.rs`'s
/// moon-unavailable path collapses the NotFound/Io distinction into a
/// single `LocateError::MoonUnavailable` variant and can't be used to
/// observe it.
///
/// NOTE on what this actually exercises after the bug fix documented in
/// `runner.rs`: `MoonCommandRunner::run` now resolves `NotFound` via a
/// `warpgate_pdk::command_exists` pre-check (real `which git` call through
/// the real host) *before* ever calling `warpgate_pdk::exec` --
/// `classify_host_failure`'s string-matching is deliberately bypassed for
/// this case (see `runner.rs`'s doc comment for why calling `exec` directly
/// for a program already known to be missing is unsafe: the real
/// `exec_command` host function aborts the entire plugin call rather than
/// returning a catchable error for a missing program). This test still
/// proves the real, end-to-end exec seam produces the right
/// `CommandError::NotFound` outcome for a genuinely-missing `git`; it just
/// does so via `command_exists` rather than `classify_host_failure`. See
/// `execute_extension_moon_binary_absent_from_path_fails_cleanly` above for
/// the empirical discovery of why `classify_host_failure`'s NotFound branch
/// can no longer be reached this way (and never reliably could, even
/// before the fix, for the same host-abort reason).
#[tokio::test(flavor = "multi_thread")]
async fn execute_extension_git_binary_absent_reports_notfound_via_real_exec_seam() {
    resolve_wasm_file();

    let sandbox = create_empty_moon_sandbox();
    let root = sandbox.root.clone();

    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .expect("failed to write fixture Cargo.toml");

    // `moon` and `which` must resolve (so the failure below is specifically
    // about `git`), but PATH must NOT contain a real `git` binary --
    // replace PATH outright with only the fake-moon directory plus a
    // `which`-only directory (see `create_which_only_dir`'s doc comment).
    let fake_bin_dir = tempfile::tempdir().expect("failed to create fake-bin tempdir");
    write_fake_moon_binary(fake_bin_dir.path());
    let which_dir = create_which_only_dir();
    let _path_guard = set_path_to(&[fake_bin_dir.path(), which_dir.path()]).await;

    let ext = sandbox.create_extension("callisto").await;

    let input = moon_pdk_test_utils::ExecuteExtensionInput {
        args: vec!["status".to_string()],
        context: ext.create_context(),
    };

    let output: callisto_moon::ExecuteExtensionOutput = ext
        .plugin
        .call_func_with("execute_extension", input)
        .await
        .expect("execute_extension wire call must survive git being absent from PATH, not trap");

    assert_ne!(
        output.exit_code, 0,
        "expected a non-zero exit_code when git is absent from PATH, got report: {:#}",
        output.report
    );
    let message = output.report["error"]["message"]
        .as_str()
        .expect("report.error.message must be a string");
    assert!(
        message.contains("git") && message.to_lowercase().contains("not found"),
        "expected classify_host_failure (via the real exec_command host \
         function) to classify a missing `git` binary as NotFound, with \
         `git`'s CommandError::NotFound message surfacing in the report; \
         got message: {message:?}, full report: {:#}",
        output.report
    );
}

/// Part B #1: the `register_extension` wire boundary
/// (`Json<RegisterExtensionInput>` deserialization inside the
/// `#[plugin_fn]`-generated guest export, per `extism-pdk-derive`'s macro
/// expansion) when fed malformed/missing-field JSON. `RegisterExtensionInput`
/// requires an `id` field; an empty object is missing it.
#[tokio::test(flavor = "multi_thread")]
async fn register_extension_malformed_json_fails_cleanly_not_panics() {
    resolve_wasm_file();

    let sandbox = create_empty_moon_sandbox();
    let ext = sandbox.create_extension("callisto").await;

    let result: Result<serde_json::Value, _> = ext
        .plugin
        .call_func_with("register_extension", serde_json::json!({}))
        .await;

    assert!(
        result.is_err(),
        "expected malformed/missing-field register_extension input to fail \
         cleanly (Err), got: {result:?}"
    );
}

/// Part B #3: `initialize_extension` well-formed-input success case,
/// against a real fixture (fake `moon` shim + a Cargo.toml package). Asserts
/// real side effects (`callisto.toml` and `.changeset/README.md` actually
/// scaffolded onto disk inside the sandbox), not just that the call
/// returned `Ok`.
#[tokio::test(flavor = "multi_thread")]
async fn initialize_extension_succeeds_against_real_repo_fixture() {
    resolve_wasm_file();

    let sandbox = create_empty_moon_sandbox();
    let root = sandbox.root.clone();

    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .expect("failed to write fixture Cargo.toml");

    let fake_bin_dir = tempfile::tempdir().expect("failed to create fake-bin tempdir");
    write_fake_moon_binary(fake_bin_dir.path());
    let _path_guard = prepend_to_path(fake_bin_dir.path()).await;

    let ext = sandbox.create_extension("callisto").await;

    let input = moon_pdk_test_utils::InitializeExtensionInput {
        context: ext.create_context(),
    };

    let output: moon_pdk_test_utils::InitializeExtensionOutput = ext
        .plugin
        .call_func_with("initialize_extension", input)
        .await
        .expect("initialize_extension wire call must succeed against a valid fixture");

    assert_eq!(output.config_url, None);
    assert_eq!(output.docs_url, None);
    assert!(output.prompts.is_empty());

    assert!(
        root.join("callisto.toml").exists(),
        "initialize_extension must scaffold callisto.toml"
    );
    assert!(
        root.join(".changeset/README.md").exists(),
        "initialize_extension must scaffold .changeset/README.md"
    );
}

/// Part B #3: `initialize_extension`'s wire boundary
/// (`Json<InitializeExtensionInput>` deserialization) when fed
/// malformed/missing-field JSON. `InitializeExtensionInput`'s only field
/// (`context: MoonContext`) is required, and `MoonContext.workspace_root`
/// is itself required -- an empty object is missing both.
#[tokio::test(flavor = "multi_thread")]
async fn initialize_extension_malformed_json_fails_cleanly_not_panics() {
    resolve_wasm_file();

    let sandbox = create_empty_moon_sandbox();
    let ext = sandbox.create_extension("callisto").await;

    let result: Result<serde_json::Value, _> = ext
        .plugin
        .call_func_with("initialize_extension", serde_json::json!({}))
        .await;

    assert!(
        result.is_err(),
        "expected malformed/missing-field initialize_extension input to \
         fail cleanly (Err), got: {result:?}"
    );
}

/// Part B #3: `initialize_extension` with a well-formed `MoonContext` (valid
/// JSON, correct shape) but pointing at a workspace where `moon` itself is
/// unavailable -- exercises `initialize_extension`'s `Result<..., LocateError>`
/// error path end-to-end (`extension::initialize_extension`'s `?` on
/// `MoonProjectLocator::new`, through `lib.rs`'s
/// `.map_err(|e| WithReturnCode::new(...))`), proving it surfaces as a
/// clean `FnResult` `Err` rather than a panic.
#[tokio::test(flavor = "multi_thread")]
async fn initialize_extension_moon_unavailable_reports_clean_error_not_panic() {
    resolve_wasm_file();

    let sandbox = create_empty_moon_sandbox();

    // Must leave a working `which` on PATH (see `create_which_only_dir`'s
    // doc comment) -- an empty PATH would instead make `command_exists`'s
    // own `which` lookup hit the same host-abort this test is trying to
    // rule out for `moon` specifically, which `Result::is_err()` alone
    // can't tell apart from the clean error path this test actually wants
    // to prove.
    let which_dir = create_which_only_dir();
    let _path_guard = set_path_to(&[which_dir.path()]).await;

    let ext = sandbox.create_extension("callisto").await;

    let input = moon_pdk_test_utils::InitializeExtensionInput {
        context: ext.create_context(),
    };

    let result: Result<moon_pdk_test_utils::InitializeExtensionOutput, _> = ext
        .plugin
        .call_func_with("initialize_extension", input)
        .await;

    let err = result.expect_err(
        "expected initialize_extension to fail cleanly (Err) when moon is \
         unavailable, not panic and not silently succeed",
    );
    let message = err.to_string();
    assert!(
        message.to_lowercase().contains("moon"),
        "expected the clean LocateError/CommandError path (mentioning \
         `moon`), not an opaque host-level plugin-call failure; got: {message:?}"
    );
}
