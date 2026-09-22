//! Registry observation tests.
//!
//! The cargo cases run the *real* `cargo info` through the production
//! observation code, against a hermetic local git registry served over
//! `file://`. Nothing here reaches a network. The remaining cases feed the
//! captured bytes of real `cargo info` and `npm view` runs (see
//! `testing/fixtures/providers/*/PROVENANCE.md`) to the same classifier.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use callisto_model::{ArtifactDigest, RegistryBindingDigest, RegistryKey, Version, VersionGrammar};

use super::super::loopback::fixtures;
use super::super::policy::tests::RecordingSleeper;
use super::*;
use crate::commands::release::binding::PreparedRegistryBinding;
use crate::commands::release::tests::RealGitRunner;

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
    run("git", &["init", "-q", "-b", "master", "."], &index);
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

fn operation(key: &str, package_name: &str, version: &str) -> RegistryPublishOperation {
    RegistryPublishOperation {
        package_dir: PathBuf::from("."),
        package_name: package_name.to_owned(),
        version: Version::parse(version, VersionGrammar::SemVer).unwrap(),
        registry: PreparedRegistryBinding {
            key: RegistryKey(key.to_owned()),
            endpoint: None,
            identity: RegistryBindingDigest::from_normalized_binding(key.as_bytes()),
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
    let runner = RealGitRunner;
    let sleeper = RecordingSleeper::new();
    let workspace = fixture().workspace.clone();
    let context = ProviderContext::new(&workspace, &runner, None).with_sleeper(&sleeper);
    let observation = registry_observation(adapter_for(Ecosystem::Cargo).unwrap(), &context, operation).unwrap();
    (observation, sleeper.waits())
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
        ["info", "core-crate@0.2.0", "--registry", "crates-io"]
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
fn an_unexpected_status_is_transient_malformed_response() {
    let output = curl_output("HTTP/1.1 500 Internal Server Error\r\ncontent-type: text/plain\r\n\r\noops");
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
        .publish(&context, &platform)
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
    adapter_for(Ecosystem::Npm).unwrap().publish(&context, &owner).unwrap();
    assert_eq!(runner.0.lock().unwrap()[0].0, "pnpm");
    assert!(runner.0.lock().unwrap()[0].1.contains(&"--filter=@s/cli".to_owned()));
}
