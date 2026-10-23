//! Registry observation tests.
//!
//! The cargo cases run the *real* `cargo info` through the production
//! observation code, against a hermetic local git registry served over
//! `file://`. Nothing here reaches a network. The remaining cases feed the
//! captured bytes of real `cargo info` and `npm view` runs (see
//! `testing/fixtures/providers/*/PROVENANCE.md`) to the same classifier.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use callisto_model::{
    ArtifactDigest, CommandOutput, CommandRunner, RegistryBindingDigest, RegistryBindingId, RegistryKey,
    ReleaseOperationId, ReleasePackageId, Version, VersionGrammar,
};

use super::super::loopback::fixtures;
use super::super::policy::tests::RecordingSleeper;
use super::*;
use crate::commands::release::binding::PreparedRegistryBinding;

// --------------------------------------------------------------- the fixture

/// The crate the hermetic registry serves, and the one the temporary workspace
/// declares as its own unpublished member.
const CRATE: &str = "core-crate";
/// Served by the registry, not yanked.
const PUBLISHED: &str = "0.1.0";
/// Served by the registry, yanked.
const YANKED: &str = "0.2.0";
/// The temporary workspace member's on-disk version, absent from the registry.
/// Observing it is the D01 regression: without `--registry`, `cargo info`
/// answers from this manifest instead of from the registry.
const LOCAL_ONLY: &str = "0.3.0";

struct CargoFixture {
    /// Holds the index, the downloads and the workspace for the process's life.
    _root: tempfile::TempDir,
    workspace: PathBuf,
}

/// Builds the registry and workspace once per test binary: every `cargo info`
/// against a distinct index URL costs cargo a fresh cached clone, so the cases
/// share one.
fn fixture() -> &'static CargoFixture {
    static FIXTURE: OnceLock<CargoFixture> = OnceLock::new();
    FIXTURE.get_or_init(build_fixture)
}

fn run(program: &str, args: &[&str], cwd: &Path) {
    let output = std::process::Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|error| panic!("{program} must be runnable: {error}"));
    assert!(
        output.status.success(),
        "{program} {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// One `.crate` file: cargo downloads and checksums it before printing an
/// answer, so it must be a real gzipped tar of a real package directory.
fn crate_file(work: &Path, downloads: &Path, version: &str) -> ArtifactDigest {
    let staging = work.join(format!("stage-{version}"));
    let package = staging.join(format!("{CRATE}-{version}"));
    std::fs::create_dir_all(package.join("src")).unwrap();
    std::fs::write(
        package.join("Cargo.toml"),
        format!("[package]\nname = \"{CRATE}\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
    )
    .unwrap();
    std::fs::write(package.join("src/lib.rs"), "\n").unwrap();
    let target = downloads.join(CRATE).join(version);
    std::fs::create_dir_all(&target).unwrap();
    let archive = target.join("download");
    run(
        "tar",
        &["czf", archive.to_str().unwrap(), &format!("{CRATE}-{version}")],
        &staging,
    );
    ArtifactDigest::from_bytes(std::fs::read(&archive).unwrap())
}

fn build_fixture() -> CargoFixture {
    let root = tempfile::Builder::new()
        .prefix("callisto-cargo-registry-")
        .tempdir()
        .unwrap();
    let base = root.path();
    let index = base.join("index");
    let downloads = base.join("downloads");
    let workspace = base.join("workspace");
    std::fs::create_dir_all(index.join("co/re")).unwrap();
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::create_dir_all(workspace.join(".cargo")).unwrap();

    let published = crate_file(base, &downloads, PUBLISHED);
    let yanked = crate_file(base, &downloads, YANKED);
    std::fs::write(
        index.join("config.json"),
        format!(
            "{{\"dl\":\"file://{downloads}\",\"api\":\"file://{downloads}\"}}\n",
            downloads = downloads.display()
        ),
    )
    .unwrap();
    let entry = |version: &str, cksum: &ArtifactDigest, yanked: bool| {
        format!(
            "{{\"name\":\"{CRATE}\",\"vers\":\"{version}\",\"deps\":[],\"cksum\":\"{}\",\"features\":{{}},\"yanked\":{yanked}}}\n",
            cksum.as_str()
        )
    };
    std::fs::write(
        index.join("co/re").join(CRATE),
        format!(
            "{}{}",
            entry(PUBLISHED, &published, false),
            entry(YANKED, &yanked, true)
        ),
    )
    .unwrap();
    // Real crates.io indexes use `master`, not `init_repo`'s `main`, so this fixture
    // stays hand-rolled; background auto-gc/maintenance is still disabled to match.
    run("git", &["init", "-q", "-b", "master", "."], &index);
    run("git", &["config", "gc.auto", "0"], &index);
    run("git", &["config", "maintenance.auto", "false"], &index);
    run("git", &["add", "-A"], &index);
    run(
        "git",
        &[
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "user.name=fixture",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "index",
        ],
        &index,
    );

    // A workspace member with the registry's crate name and a version the
    // registry does not serve.
    std::fs::write(
        workspace.join("Cargo.toml"),
        format!("[package]\nname = \"{CRATE}\"\nversion = \"{LOCAL_ONLY}\"\nedition = \"2021\"\n"),
    )
    .unwrap();
    std::fs::write(workspace.join("src/lib.rs"), "\n").unwrap();
    std::fs::write(
        workspace.join(".cargo/config.toml"),
        format!(
            "[registries.testreg]\nindex = \"file://{index}\"\n\n[registries.unreachable]\nindex = \"file://{missing}\"\n",
            index = index.display(),
            missing = base.join("no-such-index").display(),
        ),
    )
    .unwrap();

    CargoFixture { _root: root, workspace }
}

/// The id of the registry publish for `operation`.
fn operation_id(ecosystem: Ecosystem, operation: &RegistryPublishOperation) -> ReleaseOperationId {
    ReleaseOperationId::registry_publish(
        ReleasePackageId::new(ecosystem, &operation.package_name).unwrap(),
        operation.version.clone(),
        RegistryBindingId::new(operation.registry.key.as_str(), operation.registry.identity.clone()).unwrap(),
    )
}

fn operation(key: &str, package_name: &str, version: &str) -> RegistryPublishOperation {
    RegistryPublishOperation {
        package_dir: PathBuf::from("."),
        package_name: package_name.to_owned(),
        version: Version::parse(version, VersionGrammar::SemVer).unwrap(),
        registry: PreparedRegistryBinding {
            key: RegistryKey(key.to_owned()),
            endpoint: None,
            publish_endpoint: None,
            identity: RegistryBindingDigest::from_normalized_binding(key.as_bytes()),
            confirm_timeout: None,
            publish_timeout: None,
        },
        npm_access: None,
        npm_tag: None,
        by_directory: false,
    }
}

/// Runs one full observation -- real `cargo info` under the production retry
/// policy -- from the fixture workspace root, and reports the waits the policy
/// would have spent.
fn observe_cargo(operation: &RegistryPublishOperation) -> (ProviderObservationV1, Vec<std::time::Duration>) {
    let fixture = fixture();
    let cargo_home = fixture._root.path().join("cargo-home");
    std::fs::create_dir_all(&cargo_home).unwrap();
    let runner = CargoFixtureRunner { cargo_home };
    let sleeper = RecordingSleeper::new();
    let workspace = fixture.workspace.clone();
    let context = ProviderContext::new(&workspace, &runner, None).with_sleeper(&sleeper);
    let observation = registry_observation(adapter_for(Ecosystem::Cargo).unwrap(), &context, operation).unwrap();
    (observation, sleeper.waits())
}

struct CargoFixtureRunner {
    cargo_home: PathBuf,
}

impl CommandRunner for CargoFixtureRunner {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, callisto_model::CommandError> {
        let mut command = std::process::Command::new(program);
        command.args(args).current_dir(cwd);
        if program == "cargo" {
            command.env("CARGO_HOME", &self.cargo_home);
        }
        let output = command.output().map_err(|error| callisto_model::CommandError::Io {
            program: program.to_owned(),
            message: error.to_string(),
        })?;
        Ok(CommandOutput {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn exact_without_checksum(version: &str) -> ProviderObservationV1 {
    ProviderObservationV1::Exact {
        evidence: ProviderEvidenceV1::RegistryVersion {
            version: Version::parse(version, VersionGrammar::SemVer).unwrap(),
            checksum: None,
            yanked: None,
        },
    }
}

// ------------------------------------------- real cargo against a real index

#[test]
fn a_version_the_registry_serves_is_exact_with_no_checksum_or_yank_claim() {
    let (observation, waits) = observe_cargo(&operation("testreg", CRATE, PUBLISHED));
    assert_eq!(
        observation,
        exact_without_checksum(PUBLISHED),
        "`cargo info` reports neither the index checksum nor the yank flag, so the evidence must claim neither"
    );
    assert!(waits.is_empty(), "a settled answer must not be retried");
}

#[test]
fn a_version_the_registry_does_not_serve_is_absent() {
    let (observation, _) = observe_cargo(&operation("testreg", CRATE, "9.9.9"));
    assert_eq!(observation, ProviderObservationV1::Absent);
}

#[test]
fn a_crate_the_registry_has_never_heard_of_is_absent() {
    let (observation, _) = observe_cargo(&operation("testreg", "core-crate-no-such-crate-zz", "1.0.0"));
    assert_eq!(observation, ProviderObservationV1::Absent);
}

/// Real `cargo info` answers "could not find" for a yanked version exactly as
/// it does for one that never existed, so the observation is `Absent`. That
/// fails closed, not open: the publish that follows is refused by the registry
/// and, since only an `Exact` observation may satisfy an operation, the run
/// ends in `RegistryPublishUnconfirmed` rather than in a receipt.
#[test]
fn a_yanked_version_reads_as_absent_because_cargo_info_cannot_see_yanks() {
    let (observation, _) = observe_cargo(&operation("testreg", CRATE, YANKED));
    assert_eq!(observation, ProviderObservationV1::Absent);
}

/// D01: `cargo info` without `--registry` resolves the local workspace and
/// reports its on-disk version as published. The observation always passes
/// `--registry`, so the member's own version is still absent from the registry.
#[test]
fn a_local_workspace_member_of_the_same_name_is_not_read_as_a_published_version() {
    let (observation, _) = observe_cargo(&operation("testreg", CRATE, LOCAL_ONLY));
    assert_eq!(
        observation,
        ProviderObservationV1::Absent,
        "the local manifest must never decide what the registry holds"
    );
}

#[test]
fn an_unreachable_registry_is_indeterminate_and_never_absent() {
    let (observation, waits) = observe_cargo(&operation("unreachable", CRATE, PUBLISHED));
    assert_eq!(
        observation,
        ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::CommandFailed
        },
        "a registry that cannot be reached proves nothing; reading it as an absence would authorize a publish"
    );
    assert_eq!(waits.len(), 4, "the transient answer must exhaust the bounded retry");
}

/// The argv is the whole defence against the local-manifest read, so it is
/// asserted on its own rather than only through its effects.
#[test]
fn the_observation_argv_always_names_a_registry() {
    assert_eq!(
        cargo_info_args("core-crate@0.2.0", "crates-io"),
        [
            "--config",
            "net.retry=0",
            "info",
            "core-crate@0.2.0",
            "--registry",
            "crates-io"
        ]
    );
    assert_eq!(
        cargo_registry_name(&RegistryKey(RegistryKey::CRATES_IO.to_owned())),
        "crates-io",
        "callisto's logical cratesIo key is cargo's built-in crates-io registry"
    );
    assert_eq!(
        cargo_registry_name(&RegistryKey("private-cargo".to_owned())),
        "private-cargo"
    );
}

// ------------------------------- captured real bytes fed to the classifier

fn classify(raw: &str, spec: &str, version: &str) -> Attempt<ProviderObservationV1> {
    let captured = fixtures::captured_command(raw);
    let output = CommandOutput {
        exit_code: Some(captured.exit_code),
        stdout: captured.stdout,
        stderr: captured.stderr,
    };
    classify_cargo_info(&output, spec, &Version::parse(version, VersionGrammar::SemVer).unwrap())
}

fn settled_value(attempt: Attempt<ProviderObservationV1>) -> ProviderObservationV1 {
    match attempt {
        Attempt::Settled(observation) => observation,
        other => panic!("expected a settled observation, got {other:?}"),
    }
}

#[test]
fn the_captured_found_run_is_exact_for_the_version_it_names() {
    assert_eq!(
        settled_value(classify(fixtures::CARGO_INFO_FOUND, "serde@1.0.0", "1.0.0")),
        exact_without_checksum("1.0.0")
    );
}

/// The captured found run prints `version: 1.0.0 (latest ...)`; a different
/// requested version must not match that line.
#[test]
fn the_captured_found_run_does_not_satisfy_another_version() {
    assert_eq!(
        classify(fixtures::CARGO_INFO_FOUND, "serde@1.0.229", "1.0.229"),
        Attempt::Transient {
            value: ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::MalformedResponse
            },
            retry_after: None
        }
    );
}

#[test]
fn the_captured_absent_unknown_and_yanked_runs_are_all_absent() {
    for (raw, spec, version) in [
        (
            fixtures::CARGO_INFO_ABSENT_VERSION,
            "serde@0.0.0-definitely-not",
            "0.0.0-definitely-not",
        ),
        (
            fixtures::CARGO_INFO_UNKNOWN_CRATE,
            "callisto-model-no-such-crate-zz@1.0.0",
            "1.0.0",
        ),
        (fixtures::CARGO_INFO_YANKED, "chacha20@0.10.1", "0.10.1"),
    ] {
        assert_eq!(
            settled_value(classify(raw, spec, version)),
            ProviderObservationV1::Absent,
            "captured run for {spec} must be absent"
        );
    }
}

/// The captured network failure exits 101 like the three above, so only the
/// exact "could not find" phrase may be read as an absence.
#[test]
fn the_captured_network_failure_is_transient_not_absent() {
    assert_eq!(
        classify(fixtures::CARGO_INFO_NETWORK_FAILURE, "serde@1.0.0", "1.0.0"),
        Attempt::Transient {
            value: ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::CommandFailed
            },
            retry_after: None
        }
    );
}

#[test]
fn the_captured_absent_run_of_one_crate_does_not_answer_for_another() {
    assert_eq!(
        classify(fixtures::CARGO_INFO_ABSENT_VERSION, "other-crate@1.0.0", "1.0.0"),
        Attempt::Transient {
            value: ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::CommandFailed
            },
            retry_after: None
        },
        "an answer about a different spec proves nothing about this one"
    );
}

/// `npm view` is unchanged, and the captured runs are what its classification
/// is held to.
#[test]
fn the_captured_npm_view_runs_are_exact_and_absent() {
    let found = fixtures::captured_command(fixtures::NPM_VIEW_FOUND);
    assert_eq!(found.exit_code, 0);
    assert_eq!(found.stdout.trim(), "\"1.3.0\"");
    let missing = fixtures::captured_command(fixtures::NPM_VIEW_MISSING);
    assert_eq!(missing.exit_code, 1);
    assert!(
        format!("{}{}", missing.stdout, missing.stderr)
            .to_ascii_lowercase()
            .contains("e404"),
        "the npm absence classifier keys on E404: {missing:?}",
        missing = missing.stderr
    );
}

// ------------------------------------------------ npm view classification

/// Answers every `npm view` with one scripted output.
struct ScriptedNpm(CommandOutput);

impl CommandRunner for ScriptedNpm {
    fn run(&self, program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, callisto_model::CommandError> {
        assert_eq!(program, "npm");
        Ok(self.0.clone())
    }
}

fn npm_output(exit_code: Option<i32>, stdout: &str, stderr: &str) -> CommandOutput {
    CommandOutput {
        exit_code,
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
    }
}

fn captured_npm_output(raw: &str) -> CommandOutput {
    let captured = fixtures::captured_command(raw);
    npm_output(Some(captured.exit_code), &captured.stdout, &captured.stderr)
}

fn observe_npm(output: CommandOutput) -> ProviderObservationV1 {
    let runner = ScriptedNpm(output);
    let sleeper = RecordingSleeper::new();
    let root = std::env::temp_dir();
    let context = ProviderContext::new(&root, &runner, None).with_sleeper(&sleeper);
    let operation = operation("npm", "callisto-no-such-package-zz", "1.0.0");
    registry_observation(adapter_for(Ecosystem::Npm).unwrap(), &context, &operation).unwrap()
}

fn npm_json_error(code: &str, summary: &str) -> String {
    serde_json::json!({ "error": { "code": code, "summary": summary, "detail": "" } }).to_string()
}

fn npm_command_failed() -> ProviderObservationV1 {
    ProviderObservationV1::Indeterminate {
        cause: ProviderIndeterminateCause::CommandFailed,
    }
}

#[test]
fn the_captured_npm_runs_classify_as_exact_and_absent() {
    assert_eq!(
        observe_npm(captured_npm_output(fixtures::NPM_VIEW_FOUND)),
        ProviderObservationV1::Exact {
            evidence: ProviderEvidenceV1::RegistryVersion {
                version: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
                checksum: None,
                yanked: None,
            },
        }
    );
    assert_eq!(
        observe_npm(captured_npm_output(fixtures::NPM_VIEW_MISSING)),
        ProviderObservationV1::Absent
    );
}

/// Answers each `npm view` with the next scripted output, repeating the last one.
struct SequencedNpm(std::sync::Mutex<std::collections::VecDeque<CommandOutput>>);

impl CommandRunner for SequencedNpm {
    fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, callisto_model::CommandError> {
        let mut outputs = self.0.lock().unwrap();
        Ok(if outputs.len() > 1 {
            outputs.pop_front().expect("checked non-empty")
        } else {
            outputs.front().expect("a scripted output").clone()
        })
    }
}

/// The waits spent confirming an npm publish whose `npm view` answers are `outputs`, and what the confirmation saw.
fn confirm_npm(
    outputs: Vec<CommandOutput>,
    confirm_timeout: Option<std::time::Duration>,
) -> (ProviderObservationV1, Vec<std::time::Duration>) {
    let runner = SequencedNpm(std::sync::Mutex::new(outputs.into()));
    let sleeper = RecordingSleeper::new();
    let root = std::env::temp_dir();
    let context = ProviderContext::new(&root, &runner, None).with_sleeper(&sleeper);
    let mut operation = operation("npm", "callisto-no-such-package-zz", "1.0.0");
    operation.registry.confirm_timeout = confirm_timeout;
    let observation =
        confirmed_registry_observation(adapter_for(Ecosystem::Npm).unwrap(), &context, &operation).unwrap();
    (observation, sleeper.waits())
}

/// npm serves a package's document from a cache that can answer stale for minutes after a publish.
#[test]
fn an_npm_version_that_appears_minutes_late_is_confirmed_without_failing_the_run() {
    let missing = captured_npm_output(fixtures::NPM_VIEW_MISSING);
    let found = captured_npm_output(fixtures::NPM_VIEW_FOUND);
    let mut outputs = vec![missing; 3];
    outputs.push(found);
    let (observation, waits) = confirm_npm(outputs, None);
    assert!(
        matches!(observation, ProviderObservationV1::Exact { .. }),
        "{observation:?}"
    );
    let waited: std::time::Duration = waits.iter().sum();
    assert!(waited > std::time::Duration::from_secs(60), "waited {waited:?}");
    assert!(waited < std::time::Duration::from_secs(360), "waited {waited:?}");
}

#[test]
fn an_npm_version_that_never_appears_is_absent_after_the_default_window() {
    let (observation, waits) = confirm_npm(vec![captured_npm_output(fixtures::NPM_VIEW_MISSING)], None);
    assert_eq!(observation, ProviderObservationV1::Absent);
    assert_eq!(
        waits.iter().sum::<std::time::Duration>(),
        std::time::Duration::from_secs(360)
    );
}

#[test]
fn the_repository_a_registry_records_is_read_from_cargo_and_npm_output() {
    let info = "core-crate #lib\nversion: 0.1.0\nlicense: MIT\nrepository: https://github.com/o/r\n";
    assert_eq!(cargo_info_repository(info).as_deref(), Some("https://github.com/o/r"));
    assert_eq!(cargo_info_repository("version: 0.1.0\n"), None);
    assert_eq!(cargo_info_repository("repository:\n"), None);

    assert_eq!(
        npm_view_repository(r#"{"type":"git","url":"git+https://github.com/o/r.git"}"#).as_deref(),
        Some("git+https://github.com/o/r.git")
    );
    assert_eq!(npm_view_repository(r#""o/r""#).as_deref(), Some("o/r"));
    for none in ["", "null", "[]", r#"{"type":"git"}"#, r#""""#] {
        assert_eq!(npm_view_repository(none), None, "{none:?}");
    }
}

/// The query the foreign-repository warning relies on: npm is asked for the version's `repository` and a failed or
/// empty answer reads as "none", never as an error.
#[test]
fn the_npm_adapter_reads_the_repository_a_published_version_names() {
    let read = |output: CommandOutput| {
        let runner = ScriptedNpm(output);
        let root = std::env::temp_dir();
        let context = ProviderContext::new(&root, &runner, None);
        let operation = operation("npm", "callisto-no-such-package-zz", "1.0.0");
        adapter_for(Ecosystem::Npm).unwrap().repository(&context, &operation)
    };
    assert_eq!(
        read(npm_output(
            Some(0),
            r#"{"type":"git","url":"git+https://github.com/o/r.git"}"#,
            ""
        ))
        .as_deref(),
        Some("git+https://github.com/o/r.git")
    );
    assert_eq!(read(npm_output(Some(0), "", "")), None);
    assert_eq!(read(npm_output(Some(1), "", "npm ERR! code ENOTFOUND\n")), None);
}

#[test]
fn a_publish_command_runs_for_fifteen_minutes_unless_the_registry_sets_a_publish_timeout() {
    let mut op = operation("npm", "demo", "1.0.0");
    assert_eq!(publish_timeout(&op), std::time::Duration::from_secs(900));
    op.registry.publish_timeout = Some(std::time::Duration::from_secs(300));
    assert_eq!(publish_timeout(&op), std::time::Duration::from_secs(300));
}

#[test]
fn a_configured_confirm_timeout_replaces_the_default_window() {
    let window = std::time::Duration::from_secs(90);
    let (observation, waits) = confirm_npm(vec![captured_npm_output(fixtures::NPM_VIEW_MISSING)], Some(window));
    assert_eq!(observation, ProviderObservationV1::Absent);
    assert_eq!(waits.iter().sum::<std::time::Duration>(), window);
}

/// An unreachable registry is waited on within the window too, then reported as unknown rather than absent.
#[test]
fn an_npm_registry_that_cannot_be_reached_is_retried_within_the_window() {
    let (observation, waits) = confirm_npm(vec![npm_output(Some(1), "", "npm ERR! code ENOTFOUND\n")], None);
    assert_eq!(observation, npm_command_failed());
    assert_eq!(
        waits.iter().sum::<std::time::Duration>(),
        std::time::Duration::from_secs(360)
    );
}

#[test]
fn a_json_e404_error_is_absent() {
    assert_eq!(
        observe_npm(npm_output(
            Some(1),
            &npm_json_error("E404", "No match found for version 9.9.9"),
            ""
        )),
        ProviderObservationV1::Absent
    );
}

/// npm 8 prints no JSON body for `npm view --json` on failure; its error code is only the `code` line on stderr.
#[test]
fn an_npm_8_missing_version_is_absent_through_its_stderr_code_line() {
    let stderr = "npm ERR! code E404\nnpm ERR! 404 No match found for version 9.9.9\nnpm ERR! 404 \nnpm ERR! 404  'left-pad@9.9.9' is not in this registry.\n";
    assert_eq!(
        observe_npm(npm_output(Some(1), "", stderr)),
        ProviderObservationV1::Absent
    );
    let newer = "npm error code E404\nnpm error 404 No match found for version 9.9.9\n";
    assert_eq!(
        observe_npm(npm_output(Some(1), "", newer)),
        ProviderObservationV1::Absent
    );
}

/// The stderr fallback reads only the `code` line, never the free text around it.
#[test]
fn a_stderr_error_that_is_not_the_not_found_code_line_is_indeterminate() {
    for stderr in [
        "npm ERR! code E401\nnpm ERR! 404 This looks like a not found but is not the code\n",
        "npm error code E503\nnpm error E404 mentioned in a proxy message\n",
        "npm ERR! code E4040\n",
        "npm ERR! 404 No match found for version 9.9.9\n",
        "something failed with E404 somewhere\n",
        "",
    ] {
        assert_eq!(
            observe_npm(npm_output(Some(1), "", stderr)),
            npm_command_failed(),
            "stderr {stderr:?} must not read as absent"
        );
    }
}

/// A JSON error body on stdout is authoritative over stderr.
#[test]
fn a_json_error_code_wins_over_a_conflicting_stderr_code() {
    assert_eq!(
        observe_npm(npm_output(
            Some(1),
            &npm_json_error("E401", "unauthorized"),
            "npm error code E404\n"
        )),
        npm_command_failed()
    );
}

/// Only the registry's own "not found" proves absence: credentials, rate limits, outages and an unreachable host
/// prove nothing, and an `ETARGET` is `npm install`'s code, not an answer `npm view` gives.
#[test]
fn any_other_json_error_code_is_indeterminate() {
    for code in [
        "E401",
        "E403",
        "E429",
        "E500",
        "E503",
        "ETARGET",
        "ECONNREFUSED",
        "EAI_AGAIN",
        "ETIMEDOUT",
        "e404",
        "E4040",
        "",
    ] {
        assert_eq!(
            observe_npm(npm_output(Some(1), &npm_json_error(code, "failed"), "")),
            npm_command_failed(),
            "npm error code {code:?} must not read as absent"
        );
    }
}

/// The code field decides, not words inside the summary: an outage that quotes a 404 or a version message is not
/// an absence.
#[test]
fn free_text_that_mentions_a_not_found_answer_is_not_absent() {
    for summary in [
        "E404 returned by an upstream proxy",
        "No matching version found for callisto-no-such-package-zz@1.0.0",
        "'x@1.0.0' is not in this registry",
        "etarget",
    ] {
        assert_eq!(
            observe_npm(npm_output(Some(1), &npm_json_error("E503", summary), "")),
            npm_command_failed(),
            "summary {summary:?} must not read as absent"
        );
    }
}

#[test]
fn a_non_json_failure_is_indeterminate_even_when_it_mentions_e404() {
    for (stdout, stderr) in [
        (
            "",
            "npm error code ETARGET\nnpm error notarget No matching version found\n",
        ),
        ("<html>404 Not Found</html>", ""),
        ("", ""),
        ("{", "npm error code E404\n"),
        ("{\"error\":\"E404\"}", ""),
        ("{\"error\":{}}", ""),
        ("[]", ""),
        ("null", ""),
    ] {
        assert_eq!(
            observe_npm(npm_output(Some(1), stdout, stderr)),
            npm_command_failed(),
            "stdout {stdout:?} stderr {stderr:?} must not read as absent"
        );
    }
}

/// npm 11 answers a missing version with an E404 error and exit 1; an empty success proves nothing.
#[test]
fn a_successful_exit_with_no_output_is_indeterminate_not_absent() {
    assert_eq!(
        observe_npm(npm_output(Some(0), "", "")),
        ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::MalformedResponse
        }
    );
    assert_eq!(
        observe_npm(npm_output(Some(0), "  \n", "npm error code E404\n")),
        ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::MalformedResponse
        }
    );
}

// --------------------------------------- captured real pypi bytes: registry

fn pep440(version: &str) -> Version {
    Version::parse(version, VersionGrammar::Pep440).unwrap()
}

fn curl_output(raw: &str) -> CommandOutput {
    CommandOutput {
        exit_code: Some(0),
        stdout: raw.to_owned(),
        stderr: String::new(),
    }
}

fn exact_pypi(version: &str) -> ProviderObservationV1 {
    ProviderObservationV1::Exact {
        evidence: ProviderEvidenceV1::RegistryVersion {
            version: pep440(version),
            checksum: None,
            yanked: Some(false),
        },
    }
}

#[test]
fn the_captured_pypi_found_run_is_exact_for_the_version_it_names() {
    assert_eq!(
        settled_value(classify_pypi_simple(
            &curl_output(fixtures::PYPI_SIMPLE_FOUND),
            &pep440("2.3.0")
        )),
        exact_pypi("2.3.0")
    );
}

#[test]
fn the_captured_pypi_found_run_does_not_satisfy_an_unpublished_version() {
    assert_eq!(
        settled_value(classify_pypi_simple(
            &curl_output(fixtures::PYPI_SIMPLE_FOUND),
            &pep440("9.9.9")
        )),
        ProviderObservationV1::Absent
    );
}

/// PyPI yanks every file of a release together, but the classifier checks the
/// matched file(s) rather than assuming that -- either way this must fail
/// closed: a yanked version is not the version this intent authorized.
#[test]
fn the_captured_yanked_run_reads_as_absent_not_exact() {
    assert_eq!(
        settled_value(classify_pypi_simple(
            &curl_output(fixtures::PYPI_SIMPLE_YANKED),
            &pep440("1.1.0")
        )),
        ProviderObservationV1::Absent,
        "a yanked release must fail closed, not report the version as published"
    );
}

#[test]
fn the_captured_yanked_runs_index_still_answers_exact_for_an_unyanked_sibling_version() {
    assert_eq!(
        settled_value(classify_pypi_simple(
            &curl_output(fixtures::PYPI_SIMPLE_YANKED),
            &pep440("1.2.0")
        )),
        exact_pypi("1.2.0"),
        "one yanked release must not poison observation of another version in the same index"
    );
}

/// Also exercises a legacy `.zip` sdist and a PEP 440 dev-release filename
/// (`pluggy-0.4.0.zip`, `pluggy-1.0.0.dev0*`), both served by this fixture.
#[test]
fn the_captured_yanked_runs_index_reads_a_dev_release_and_a_legacy_zip_sdist() {
    assert_eq!(
        settled_value(classify_pypi_simple(
            &curl_output(fixtures::PYPI_SIMPLE_YANKED),
            &pep440("0.4.0")
        )),
        exact_pypi("0.4.0")
    );
    assert_eq!(
        settled_value(classify_pypi_simple(
            &curl_output(fixtures::PYPI_SIMPLE_YANKED),
            &pep440("1.0.0.dev0")
        )),
        exact_pypi("1.0.0.dev0")
    );
}

#[test]
fn the_captured_pypi_absent_run_is_absent() {
    assert_eq!(
        settled_value(classify_pypi_simple(
            &curl_output(fixtures::PYPI_SIMPLE_ABSENT),
            &pep440("1.0.0")
        )),
        ProviderObservationV1::Absent
    );
}

#[test]
fn a_curl_transport_failure_with_no_parseable_response_is_transient_command_failed() {
    let output = CommandOutput {
        exit_code: Some(6),
        stdout: String::new(),
        stderr: "curl: (6) Could not resolve host: pypi.org".to_owned(),
    };
    assert_eq!(
        classify_pypi_simple(&output, &pep440("1.0.0")),
        Attempt::Transient {
            value: ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::CommandFailed
            },
            retry_after: None
        }
    );
}

#[test]
fn an_unexpected_status_is_transient_and_names_the_status() {
    let output = curl_output("HTTP/1.1 500 Internal Server Error\r\ncontent-type: text/plain\r\n\r\noops");
    assert_eq!(
        classify_pypi_simple(&output, &pep440("1.0.0")),
        Attempt::Transient {
            value: ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::ProviderStatus { status: 500 }
            },
            retry_after: None
        }
    );
}

/// PyPI shares GitHub's status-based classifier: a 429 retries on the server's own `Retry-After`, not a fixed backoff.
#[test]
fn a_rate_limited_pypi_response_retries_on_its_own_retry_after() {
    let output = curl_output("HTTP/1.1 429 Too Many Requests\r\nRetry-After: 30\r\n\r\nslow down");
    assert_eq!(
        classify_pypi_simple(&output, &pep440("1.0.0")),
        Attempt::Transient {
            value: ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::ProviderStatus { status: 429 }
            },
            retry_after: Some(std::time::Duration::from_secs(30))
        }
    );
}

/// A plain 401 is not a rate limit and cannot resolve by retrying, so it settles immediately instead of retrying.
#[test]
fn a_permanent_401_settles_instead_of_retrying() {
    let output = curl_output("HTTP/1.1 401 Unauthorized\r\ncontent-type: text/plain\r\n\r\nno auth");
    assert_eq!(
        classify_pypi_simple(&output, &pep440("1.0.0")),
        Attempt::Settled(ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::ProviderStatus { status: 401 }
        })
    );
}

/// Regression: a `curl` call that times out once must retry, not hand `CommandError::TimedOut` straight to `?`.
#[test]
fn a_timed_out_curl_call_retries_and_settles() {
    struct FlakyCurl(std::sync::Mutex<std::collections::VecDeque<Result<CommandOutput, callisto_model::CommandError>>>);
    impl callisto_model::CommandRunner for FlakyCurl {
        fn run(
            &self,
            program: &str,
            _args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<CommandOutput, callisto_model::CommandError> {
            assert_eq!(program, "curl");
            self.0.lock().unwrap().pop_front().expect("script exhausted")
        }
    }
    let runner = FlakyCurl(std::sync::Mutex::new(
        vec![
            Err(callisto_model::CommandError::TimedOut {
                program: "curl".to_owned(),
                seconds: 300,
            }),
            Ok(curl_output(fixtures::PYPI_SIMPLE_ABSENT)),
        ]
        .into(),
    ));
    let sleeper = super::super::policy::tests::RecordingSleeper::new();
    let root = std::env::temp_dir();
    let context = ProviderContext::new(&root, &runner, None).with_sleeper(&sleeper);
    let operation = RegistryPublishOperation {
        package_dir: PathBuf::from("."),
        package_name: "left-pad".to_owned(),
        version: pep440("1.0.0"),
        registry: PreparedRegistryBinding {
            key: RegistryKey("pypi".to_owned()),
            endpoint: None,
            publish_endpoint: None,
            identity: RegistryBindingDigest::from_normalized_binding(b"pypi"),
            confirm_timeout: None,
            publish_timeout: None,
        },
        npm_access: None,
        npm_tag: None,
        by_directory: false,
    };
    let result = policy::retry_observation(context.sleeper(), || PypiRegistry.observe_once(&context, &operation));
    assert_eq!(result, Ok(ProviderObservationV1::Absent));
    assert_eq!(sleeper.waits(), vec![std::time::Duration::from_secs(2)]);
}

/// A private index that ignores `Accept` and serves its legacy HTML page
/// fails the JSON parse and must not be misread as an answer either way.
#[test]
fn a_private_index_serving_html_instead_of_json_is_transient_malformed_response() {
    let output = curl_output("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\n\r\n<html><body>not json</body></html>");
    assert_eq!(
        classify_pypi_simple(&output, &pep440("1.0.0")),
        Attempt::Transient {
            value: ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::MalformedResponse
            },
            retry_after: None
        }
    );
}

#[test]
fn simple_file_version_reads_wheels_sdists_and_legacy_zips() {
    assert_eq!(
        pypi_simple_file_version("iniconfig-2.3.0-py3-none-any.whl"),
        Some("2.3.0")
    );
    assert_eq!(pypi_simple_file_version("iniconfig-2.3.0.tar.gz"), Some("2.3.0"));
    assert_eq!(pypi_simple_file_version("pluggy-0.4.0.zip"), Some("0.4.0"));
    assert_eq!(
        pypi_simple_file_version("pluggy-1.0.0.dev0-py2.py3-none-any.whl"),
        Some("1.0.0.dev0")
    );
    assert_eq!(
        pypi_simple_file_version("my-package-1.0.0.tar.gz"),
        Some("1.0.0"),
        "a PEP 440 version never contains a hyphen, so the last `-` is always the name/version boundary"
    );
    assert_eq!(
        pypi_simple_file_version("iniconfig-2.3.0.egg"),
        None,
        "an unrecognized extension must not be guessed at"
    );
}

#[test]
fn the_simple_index_url_uses_the_default_endpoint_and_the_normalized_project_name() {
    assert_eq!(
        pypi_simple_index_url(None, "My.Package"),
        "https://pypi.org/simple/my-package/"
    );
    assert_eq!(
        pypi_simple_index_url(Some("https://pypi.example.com/simple"), "My.Package"),
        "https://pypi.example.com/simple/my-package/"
    );
}

// ----------------------------------------------------- adapter capabilities

/// PyPI's own JSON simple index answers like any other registry, so all three
/// implemented ecosystems pass the plan-time observability gate.
#[test]
fn cargo_npm_and_pypi_are_observable_and_pass_the_plan_time_gate() {
    for ecosystem in [Ecosystem::Cargo, Ecosystem::Npm, Ecosystem::Pypi] {
        assert!(adapter_for(ecosystem).unwrap().can_observe_versions());
        assert!(require_observable_registry(ecosystem).is_ok());
    }
}

#[test]
fn an_ecosystem_with_no_registry_adapter_is_unsupported() {
    assert!(matches!(
        adapter_for(Ecosystem::NuGet),
        Err(GraphError::UnsupportedRelease {
            feature: UnsupportedReleaseFeature::Ecosystem
        })
    ));
}

/// Records every command and answers success, so a publish argv can be read back.
struct RecordingRunner(std::sync::Mutex<Vec<(String, Vec<String>, PathBuf)>>);

impl callisto_model::CommandRunner for RecordingRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        cwd: &Path,
    ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
        self.0.lock().unwrap().push((
            program.to_owned(),
            args.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd.to_path_buf(),
        ));
        Ok(callisto_model::CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

struct EnvironmentFilterRunner(std::sync::Mutex<Vec<String>>);

impl callisto_model::CommandRunner for EnvironmentFilterRunner {
    fn run(
        &self,
        _program: &str,
        _args: &[&str],
        _cwd: &Path,
    ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
        Ok(callisto_model::CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    fn run_with_timeout_env_filtered(
        &self,
        _program: &str,
        _args: &[&str],
        _cwd: &Path,
        _timeout: std::time::Duration,
        removed: &[&str],
    ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
        *self.0.lock().unwrap() = removed.iter().map(|name| (*name).to_owned()).collect();
        self.run("", &[], Path::new(""))
    }
}

#[test]
fn python_build_removes_every_release_credential_from_its_environment() {
    let root = tempfile::tempdir().unwrap();
    let runner = EnvironmentFilterRunner(std::sync::Mutex::new(Vec::new()));
    let context = ProviderContext::new(root.path(), &runner, None);
    let argv = registry_argv::Argv {
        program: "python3".to_owned(),
        args: vec!["-m".to_owned(), "build".to_owned()],
        cwd: root.path().to_path_buf(),
    };

    assert!(run_build_argv(&context, &argv).unwrap().success());
    assert_eq!(
        runner.0.lock().unwrap().as_slice(),
        [
            "CARGO_REGISTRY_TOKEN",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "NPM_CONFIG_TOKEN",
            "NPM_CONFIG_USERCONFIG",
            "NPM_TOKEN",
            "NODE_AUTH_TOKEN",
            "PYPI_TOKEN",
            "TWINE_PASSWORD",
            "TWINE_USERNAME",
        ]
    );
}

#[test]
fn publisher_environment_maps_only_the_selected_native_tool() {
    let npm = npm_publisher_environment("npm", Some("npm-secret"), Some("/tmp/npmrc"));
    assert_eq!(
        npm,
        [("NPM_TOKEN", "npm-secret"), ("NPM_CONFIG_USERCONFIG", "/tmp/npmrc")]
    );
    let bun = npm_publisher_environment("bun", Some("npm-secret"), Some("/tmp/npmrc"));
    assert_eq!(
        bun,
        [
            ("NPM_TOKEN", "npm-secret"),
            ("NPM_CONFIG_TOKEN", "npm-secret"),
            ("NPM_CONFIG_USERCONFIG", "/tmp/npmrc")
        ]
    );
    assert_eq!(
        pypi_publisher_environment(Some("__token__"), Some("pypi-secret")),
        [("TWINE_USERNAME", "__token__"), ("TWINE_PASSWORD", "pypi-secret")]
    );
    assert!(npm_publisher_environment("npm", None, None).is_empty());
    assert!(pypi_publisher_environment(None, None).is_empty());
}

/// A platform package outside the workspace globs cannot be selected by name
/// (`pnpm --filter` answers "No projects matched"), so it is published by path
/// with npm, even in a pnpm workspace.
#[test]
fn a_platform_package_is_published_by_directory_with_npm() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("pnpm-lock.yaml"), "").unwrap();
    let runner = RecordingRunner(std::sync::Mutex::new(Vec::new()));
    let context = ProviderContext::new(root.path(), &runner, None);
    let mut platform = operation("npm", "@s/cli-linux-x64-gnu", "1.0.0");
    platform.package_dir = PathBuf::from("packages/cli/npm/linux-x64-gnu");
    platform.npm_access = Some(callisto_model::NpmAccess::Public);
    platform.by_directory = true;

    adapter_for(Ecosystem::Npm)
        .unwrap()
        .publish(&context, &operation_id(Ecosystem::Npm, &platform), &platform, None)
        .unwrap();
    let dir = root.path().join("packages/cli/npm/linux-x64-gnu");
    assert_eq!(
        runner.0.lock().unwrap().as_slice(),
        [(
            "npm".to_owned(),
            vec![
                "publish".to_owned(),
                dir.to_string_lossy().into_owned(),
                "--access".to_owned(),
                "public".to_owned()
            ],
            root.path().to_path_buf(),
        )]
    );

    // The owner is a workspace member and keeps its package manager's by-name form.
    runner.0.lock().unwrap().clear();
    let owner = operation("npm", "@s/cli", "1.0.0");
    adapter_for(Ecosystem::Npm)
        .unwrap()
        .publish(&context, &operation_id(Ecosystem::Npm, &owner), &owner, None)
        .unwrap();
    assert_eq!(runner.0.lock().unwrap()[0].0, "pnpm");
    assert!(runner.0.lock().unwrap()[0].1.contains(&"--filter=@s/cli".to_owned()));
}

/// Observing and publishing must read one `.npmrc`, or the version looked for and the version published could be on
/// different registries. Both resolve the workspace that owns the package, by its declaration and not by a lockfile.
#[test]
fn observing_and_publishing_a_nested_npm_package_all_run_in_the_workspace_that_owns_it() {
    let root = tempfile::tempdir().unwrap();
    let write = |path: &str, text: &str| {
        let path = root.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("package.json", r#"{"name":"marker","private":true}"#);
    write(
        "web/package.json",
        r#"{"name":"web","private":true,"workspaces":["packages/*"]}"#,
    );
    write("web/.npmrc", "registry=https://nested.example.test/\n");
    write("web/packages/a/package.json", r#"{"name":"a","version":"1.0.0"}"#);
    let runner = RecordingRunner(std::sync::Mutex::new(Vec::new()));
    let context = ProviderContext::new(root.path(), &runner, None);
    let adapter = adapter_for(Ecosystem::Npm).unwrap();

    let mut by_name = operation("npm", "a", "1.0.0");
    by_name.package_dir = PathBuf::from("web/packages/a");
    adapter.observe_once(&context, &by_name).unwrap();
    adapter
        .publish(&context, &operation_id(Ecosystem::Npm, &by_name), &by_name, None)
        .unwrap();
    let mut by_directory = operation("npm", "a", "1.0.0");
    by_directory.package_dir = PathBuf::from("web/packages/a");
    by_directory.by_directory = true;
    adapter
        .publish(
            &context,
            &operation_id(Ecosystem::Npm, &by_directory),
            &by_directory,
            None,
        )
        .unwrap();

    let calls = runner.0.lock().unwrap().clone();
    let programs: Vec<&str> = calls.iter().map(|(program, ..)| program.as_str()).collect();
    assert_eq!(programs, ["npm", "npm", "npm"], "{calls:?}");
    assert!(calls[0].1.contains(&"view".to_owned()), "{calls:?}");
    assert!(calls[1].1.contains(&"--workspace=a".to_owned()), "{calls:?}");
    for (_, _, cwd) in &calls {
        assert_eq!(cwd, &root.path().join("web"), "{calls:?}");
    }
}

/// A credential withheld from a build backend is either a secret the redactor knows or a name known to carry
/// no secret value, so a new secret cannot be added here and escape redaction in an error.
#[test]
fn every_credential_withheld_from_a_build_is_redacted_or_known_not_to_be_secret() {
    const NOT_SECRET: &[&str] = &["TWINE_USERNAME", "NPM_CONFIG_USERCONFIG"];
    for name in super::RELEASE_CREDENTIALS {
        assert!(
            callisto_model::SECRET_CREDENTIAL_ENV.contains(name) || NOT_SECRET.contains(name),
            "`{name}` is neither redacted from output nor listed as carrying no secret"
        );
    }
}
