//! The Python release flow against a fake index, build and twine: a release is complete only when every file the
//! build declares is on the index, and a rerun uploads only what is missing.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use callisto_model::{
    CommandError, CommandOutput, CommandRunner, RegistryBindingDigest, RegistryBindingId, RegistryKey,
    ReleaseOperationId, ReleasePackageId, Version, VersionGrammar,
};
use sha2::{Digest, Sha256};

use super::super::policy::tests::RecordingSleeper;
use super::super::PypiFileSets;
use super::*;
use crate::commands::release::binding::PreparedRegistryBinding;

const WHEEL: &str = "my_pkg-1.0.0-py3-none-any.whl";
const SDIST: &str = "my_pkg-1.0.0.tar.gz";

fn sha256(content: &str) -> String {
    Sha256::digest(content.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

/// One file on the fake index.
#[derive(Clone)]
struct Held {
    filename: String,
    sha256: String,
    yanked: bool,
}

#[derive(Default)]
struct World {
    /// Files the index serves now.
    visible: Vec<Held>,
    /// Uploaded files the index shows after `lag` more reads.
    pending: Vec<Held>,
    lag: usize,
    /// What `python -m build` writes: filename and content.
    build_output: Vec<(String, String)>,
    /// Uploads that fail after this many files, with an HTTP error.
    fail_upload_after: Option<usize>,
    /// Uploads whose stored digest differs from the sent bytes.
    corrupt_uploads: bool,
    calls: Vec<String>,
    uploads: Vec<Vec<String>>,
    /// The `SOURCE_DATE_EPOCH` each build ran with.
    epochs: Vec<String>,
}

struct PypiRunner(Mutex<World>);

impl PypiRunner {
    fn new(build: &[(&str, &str)]) -> Self {
        Self(Mutex::new(World {
            build_output: build.iter().map(|(n, c)| (n.to_string(), c.to_string())).collect(),
            ..World::default()
        }))
    }

    fn with(self, edit: impl FnOnce(&mut World)) -> Self {
        edit(&mut self.0.lock().unwrap());
        self
    }

    fn count(&self, program: &str) -> usize {
        let world = self.0.lock().unwrap();
        world
            .calls
            .iter()
            .filter(|call| call.split(' ').next() == Some(program))
            .count()
    }

    fn calls(&self, program: &str) -> Vec<String> {
        let world = self.0.lock().unwrap();
        world
            .calls
            .iter()
            .filter(|call| call.split(' ').next() == Some(program))
            .cloned()
            .collect()
    }

    fn epochs(&self) -> Vec<String> {
        self.0.lock().unwrap().epochs.clone()
    }

    fn uploads(&self) -> Vec<Vec<String>> {
        self.0.lock().unwrap().uploads.clone()
    }

    /// The index shows every upload that has reached it.
    fn settle(&self) {
        let mut world = self.0.lock().unwrap();
        let arrived = std::mem::take(&mut world.pending);
        world.visible.extend(arrived);
    }

    fn held(&self) -> Vec<String> {
        let world = self.0.lock().unwrap();
        world.visible.iter().map(|file| file.filename.clone()).collect()
    }
}

fn ok(stdout: &str) -> Result<CommandOutput, CommandError> {
    Ok(CommandOutput {
        exit_code: Some(0),
        stdout: stdout.to_string(),
        stderr: String::new(),
    })
}

impl PypiRunner {
    fn handle(&self, program: &str, args: &[&str]) -> Result<CommandOutput, CommandError> {
        if program == "env" {
            let epoch = args[0]
                .strip_prefix("SOURCE_DATE_EPOCH=")
                .expect("the build pins its timestamps");
            self.0.lock().unwrap().epochs.push(epoch.to_string());
            return self.handle(args[1], &args[2..]);
        }
        let mut world = self.0.lock().unwrap();
        world.calls.push(format!("{program} {}", args.join(" ")));
        match program {
            "git" => ok("1700000000\n"),
            "curl" => {
                if world.lag > 0 {
                    world.lag -= 1;
                } else {
                    let arrived = std::mem::take(&mut world.pending);
                    world.visible.extend(arrived);
                }
                if world.visible.is_empty() {
                    return ok("HTTP/2 404 Not Found\r\n\r\n");
                }
                let files: Vec<serde_json::Value> = world
                    .visible
                    .iter()
                    .map(|file| {
                        serde_json::json!({
                            "filename": file.filename,
                            "url": "https://files.example.invalid/x",
                            "hashes": if file.sha256.is_empty() {
                                serde_json::json!({})
                            } else {
                                serde_json::json!({ "sha256": file.sha256 })
                            },
                            "yanked": file.yanked,
                        })
                    })
                    .collect();
                let body = serde_json::json!({ "meta": { "api-version": "1.0" }, "name": "my-pkg", "files": files });
                ok(&format!(
                    "HTTP/2 200 OK\r\ncontent-type: application/vnd.pypi.simple.v1+json\r\n\r\n{body}"
                ))
            }
            "python3" | "python" => {
                let outdir = PathBuf::from(args[args.iter().position(|arg| *arg == "--outdir").unwrap() + 1]);
                for (name, content) in &world.build_output {
                    std::fs::write(outdir.join(name), content).unwrap();
                }
                ok("")
            }
            "twine" => {
                let files: Vec<String> = args
                    .iter()
                    .skip(1)
                    .filter(|arg| !arg.starts_with("--") && !arg.starts_with("http"))
                    .map(|arg| arg.to_string())
                    .collect();
                world.uploads.push(
                    files
                        .iter()
                        .map(|file| Path::new(file).file_name().unwrap().to_string_lossy().into_owned())
                        .collect(),
                );
                for (sent, file) in files.iter().enumerate() {
                    if world.fail_upload_after == Some(sent) {
                        return Ok(CommandOutput {
                            exit_code: Some(1),
                            stdout: String::new(),
                            stderr: "HTTPError: 503 Service Unavailable".to_string(),
                        });
                    }
                    let bytes = std::fs::read_to_string(file).unwrap();
                    let digest = if world.corrupt_uploads {
                        "0".repeat(64)
                    } else {
                        sha256(&bytes)
                    };
                    let held = Held {
                        filename: Path::new(file).file_name().unwrap().to_string_lossy().into_owned(),
                        sha256: digest,
                        yanked: false,
                    };
                    world.pending.push(held);
                }
                ok("Uploading\n")
            }
            other => panic!("unexpected program {other}"),
        }
    }
}

impl CommandRunner for PypiRunner {
    fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
        self.handle(program, args)
    }

    fn run_with_timeout_env_filtered(
        &self,
        program: &str,
        args: &[&str],
        _cwd: &Path,
        _timeout: std::time::Duration,
        _removed: &[&str],
    ) -> Result<CommandOutput, CommandError> {
        self.handle(program, args)
    }

    fn run_with_timeout_publisher(
        &self,
        program: &str,
        args: &[&str],
        _cwd: &Path,
        _timeout: std::time::Duration,
        _env: &[(&str, &str)],
    ) -> Result<CommandOutput, CommandError> {
        self.handle(program, args)
    }
}

fn pep440() -> Version {
    Version::parse("1.0.0", VersionGrammar::Pep440).unwrap()
}

fn operation() -> RegistryPublishOperation {
    RegistryPublishOperation {
        package_dir: PathBuf::from("."),
        package_name: "my-pkg".to_owned(),
        version: pep440(),
        registry: PreparedRegistryBinding {
            key: RegistryKey("pypi".to_owned()),
            endpoint: None,
            publish_endpoint: None,
            identity: RegistryBindingDigest::from_normalized_binding(b"pypi"),
        },
        npm_access: None,
        npm_tag: None,
        by_directory: false,
    }
}

fn id(operation: &RegistryPublishOperation) -> ReleaseOperationId {
    ReleaseOperationId::registry_publish(
        ReleasePackageId::new(Ecosystem::Pypi, &operation.package_name).unwrap(),
        operation.version.clone(),
        RegistryBindingId::new("pypi", operation.registry.identity.clone()).unwrap(),
    )
}

fn held(filename: &str, content: &str) -> Held {
    Held {
        filename: filename.to_string(),
        sha256: sha256(content),
        yanked: false,
    }
}

const BUILD: [(&str, &str); 2] = [(WHEEL, "wheel bytes"), (SDIST, "sdist bytes")];

/// Everything one run shares: the root, the sleeper and the built-set cache.
struct Run {
    root: tempfile::TempDir,
    sleeper: RecordingSleeper,
    sets: PypiFileSets,
}

impl Run {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
            sleeper: RecordingSleeper::new(),
            sets: PypiFileSets::default(),
        }
    }

    fn context<'a>(&'a self, runner: &'a PypiRunner) -> ProviderContext<'a> {
        ProviderContext::new(self.root.path(), runner, None)
            .with_sleeper(&self.sleeper)
            .with_file_sets(&self.sets)
    }
}

fn preflight(run: &Run, runner: &PypiRunner) -> Result<ReleasePreflight, GraphError> {
    preflight_with(run, runner, None)
}

fn preflight_with(
    run: &Run,
    runner: &PypiRunner,
    artifacts: Option<&VerifiedArtifactManifest<'_>>,
) -> Result<ReleasePreflight, GraphError> {
    let operation = operation();
    PypiRegistry.preflight(&run.context(runner), &id(&operation), &operation, artifacts)
}

/// Publishes whatever the index lacks and confirms it, as the provider does after an absent preflight.
fn publish_and_confirm(run: &Run, runner: &PypiRunner) -> Result<ProviderObservationV1, GraphError> {
    publish_and_confirm_with(run, runner, None)
}

fn publish_and_confirm_with(
    run: &Run,
    runner: &PypiRunner,
    artifacts: Option<&VerifiedArtifactManifest<'_>>,
) -> Result<ProviderObservationV1, GraphError> {
    let operation = operation();
    let context = run.context(runner);
    let output = PypiRegistry.publish(&context, &id(&operation), &operation, artifacts)?;
    let outcome = PypiRegistry.classify(&output).map_err(|source| GraphError::Registry {
        package: operation.package_name.clone(),
        source,
    })?;
    PypiRegistry.confirm(
        &context,
        &id(&operation),
        &operation,
        matches!(outcome, PublishOutcome::Published),
        artifacts,
    )
}

/// Publishes and confirms `operation`, which may bind its own endpoints.
fn release_on(run: &Run, runner: &PypiRunner, operation: &RegistryPublishOperation) -> ProviderObservationV1 {
    let context = run.context(runner);
    PypiRegistry
        .preflight(&context, &id(operation), operation, None)
        .unwrap();
    PypiRegistry.publish(&context, &id(operation), operation, None).unwrap();
    PypiRegistry
        .confirm(&context, &id(operation), operation, true, None)
        .unwrap()
}

fn on_routes(endpoint: Option<&str>, publish_endpoint: Option<&str>) -> RegistryPublishOperation {
    let mut operation = operation();
    operation.registry.endpoint = endpoint.map(str::to_owned);
    operation.registry.publish_endpoint = publish_endpoint.map(str::to_owned);
    operation
}

#[test]
fn a_split_route_reads_the_index_and_uploads_to_the_upload_endpoint() {
    let run = Run::new();
    let runner = PypiRunner::new(&BUILD);
    let operation = on_routes(
        Some("https://index.example.test/simple"),
        Some("https://upload.example.test/legacy/"),
    );
    assert!(release_on(&run, &runner, &operation).is_terminal_success());
    let reads = runner.calls("curl");
    assert!(!reads.is_empty());
    for read in &reads {
        assert!(read.ends_with("https://index.example.test/simple/my-pkg/"), "{read}");
    }
    let uploads = runner.calls("twine");
    assert_eq!(uploads.len(), 1, "{uploads:?}");
    assert!(
        uploads[0].starts_with("twine upload --repository-url https://upload.example.test/legacy/ "),
        "{}",
        uploads[0]
    );
}

#[test]
fn a_single_configured_endpoint_serves_both_reads_and_uploads() {
    let run = Run::new();
    let runner = PypiRunner::new(&BUILD);
    let operation = on_routes(Some("https://index.example.test/simple"), None);
    assert!(release_on(&run, &runner, &operation).is_terminal_success());
    assert!(runner.calls("curl")[0].ends_with("https://index.example.test/simple/my-pkg/"));
    assert!(runner.calls("twine")[0].starts_with("twine upload --repository-url https://index.example.test/simple "));
}

#[test]
fn an_upload_route_alone_leaves_reads_on_the_public_index() {
    let run = Run::new();
    let runner = PypiRunner::new(&BUILD);
    let operation = on_routes(None, Some("https://upload.example.test/legacy/"));
    assert!(release_on(&run, &runner, &operation).is_terminal_success());
    assert!(runner.calls("curl")[0].ends_with("https://pypi.org/simple/my-pkg/"));
    assert!(runner.calls("twine")[0].starts_with("twine upload --repository-url https://upload.example.test/legacy/ "));
}

/// A confirm that rebuilt would compare an upload with fresh bytes, so a run without the cache fails loudly.
#[test]
fn a_python_release_without_the_per_run_cache_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    let runner = PypiRunner::new(&BUILD).with(|world| world.visible = vec![held(WHEEL, "wheel bytes")]);
    let context = ProviderContext::new(root.path(), &runner, None);
    let operation = operation();
    let error = PypiRegistry
        .preflight(&context, &id(&operation), &operation, None)
        .unwrap_err();
    assert!(matches!(error, GraphError::ReleaseInvariant { .. }), "{error:?}");
    assert_eq!(runner.count("python3") + runner.count("python"), 0);
}

#[test]
fn an_index_with_none_of_the_files_proceeds_without_a_build() {
    let run = Run::new();
    let runner = PypiRunner::new(&BUILD);
    assert!(matches!(
        preflight(&run, &runner).unwrap(),
        ReleasePreflight::Proceed { .. }
    ));
    assert_eq!(runner.count("python3") + runner.count("python"), 0);
}

#[test]
fn a_first_release_uploads_every_built_file_and_confirms_it() {
    let run = Run::new();
    let runner = PypiRunner::new(&BUILD);
    assert!(matches!(
        preflight(&run, &runner).unwrap(),
        ReleasePreflight::Proceed { .. }
    ));
    let confirmed = publish_and_confirm(&run, &runner).unwrap();
    assert!(confirmed.is_terminal_success(), "{confirmed:?}");
    let uploads = runner.uploads();
    assert_eq!(uploads.len(), 1, "one upload command: {uploads:?}");
    assert_eq!(uploads[0], [WHEEL, SDIST]);
    assert_eq!(runner.held().len(), 2);
}

#[test]
fn an_index_with_every_file_is_already_satisfied_and_uploads_nothing() {
    let run = Run::new();
    let runner = PypiRunner::new(&BUILD).with(|world| {
        world.visible = vec![held(WHEEL, "wheel bytes"), held(SDIST, "sdist bytes")];
    });
    assert!(matches!(
        preflight(&run, &runner).unwrap(),
        ReleasePreflight::AlreadySatisfied { .. }
    ));
    assert_eq!(runner.count("twine"), 0);
}

/// A rebuild need not match an earlier attempt, so a file already on the index is trusted by name.
#[test]
fn a_file_already_on_the_index_is_trusted_by_name_not_by_bytes() {
    let run = Run::new();
    let runner = PypiRunner::new(&BUILD).with(|world| {
        world.visible = vec![held(WHEEL, "an earlier build"), held(SDIST, "an earlier build")];
    });
    assert!(matches!(
        preflight(&run, &runner).unwrap(),
        ReleasePreflight::AlreadySatisfied { .. }
    ));
}

#[test]
fn a_partly_published_release_uploads_only_the_missing_file() {
    let run = Run::new();
    let runner = PypiRunner::new(&BUILD).with(|world| world.visible = vec![held(WHEEL, "wheel bytes")]);
    assert!(matches!(
        preflight(&run, &runner).unwrap(),
        ReleasePreflight::Proceed { .. }
    ));
    let confirmed = publish_and_confirm(&run, &runner).unwrap();
    assert!(confirmed.is_terminal_success(), "{confirmed:?}");
    assert_eq!(runner.uploads(), [vec![SDIST.to_string()]]);
    assert_eq!(
        runner.count("python3") + runner.count("python"),
        1,
        "one build serves both steps"
    );
}

/// The first attempt dies after one upload; the rerun finishes the set without offering the first file again.
#[test]
fn a_rerun_after_a_failed_upload_finishes_the_set() {
    let first = Run::new();
    let runner = PypiRunner::new(&BUILD).with(|world| world.fail_upload_after = Some(1));
    assert!(matches!(
        preflight(&first, &runner).unwrap(),
        ReleasePreflight::Proceed { .. }
    ));
    let error = publish_and_confirm(&first, &runner).unwrap_err();
    assert!(matches!(error, GraphError::Registry { .. }), "{error:?}");
    // The index shows what reached it.
    runner.settle();
    assert_eq!(runner.held().len(), 1);

    let second = Run::new();
    runner.0.lock().unwrap().fail_upload_after = None;
    assert!(matches!(
        preflight(&second, &runner).unwrap(),
        ReleasePreflight::Proceed { .. }
    ));
    let confirmed = publish_and_confirm(&second, &runner).unwrap();
    assert!(confirmed.is_terminal_success(), "{confirmed:?}");
    let uploads = runner.uploads();
    assert_eq!(uploads.len(), 2);
    assert_eq!(
        uploads[1].len(),
        1,
        "the rerun offered only the missing file: {uploads:?}"
    );
    assert!(!uploads[1].contains(&uploads[0][0]), "{uploads:?}");
    assert_eq!(runner.held().len(), 2);
}

#[test]
fn a_yanked_release_is_a_conflict() {
    let run = Run::new();
    let runner = PypiRunner::new(&BUILD).with(|world| {
        let mut wheel = held(WHEEL, "wheel bytes");
        wheel.yanked = true;
        world.visible = vec![wheel, held(SDIST, "sdist bytes")];
    });
    let error = preflight(&run, &runner).unwrap_err();
    assert!(matches!(error, GraphError::ReleaseRemoteConflict { .. }), "{error:?}");
}

#[test]
fn the_index_catching_up_after_an_upload_is_waited_for() {
    let run = Run::new();
    let runner = PypiRunner::new(&BUILD).with(|world| world.lag = 4);
    assert!(matches!(
        preflight(&run, &runner).unwrap(),
        ReleasePreflight::Proceed { .. }
    ));
    let confirmed = publish_and_confirm(&run, &runner).unwrap();
    assert!(confirmed.is_terminal_success(), "{confirmed:?}");
    assert!(!run.sleeper.waits().is_empty(), "the confirmation waited for the index");
}

#[test]
fn an_uploaded_file_stored_with_other_bytes_is_an_error() {
    let run = Run::new();
    let runner = PypiRunner::new(&BUILD).with(|world| world.corrupt_uploads = true);
    let error = publish_and_confirm(&run, &runner).unwrap_err();
    assert!(matches!(error, GraphError::PypiFileDiffers { .. }), "{error:?}");
}

#[test]
fn files_the_build_did_not_declare_do_not_block_a_complete_release() {
    let run = Run::new();
    let runner = PypiRunner::new(&BUILD).with(|world| {
        world.visible = vec![
            held(WHEEL, "wheel bytes"),
            held(SDIST, "sdist bytes"),
            held("my_pkg-1.0.0-cp312-cp312-manylinux_2_28_x86_64.whl", "another job"),
        ];
    });
    assert!(matches!(
        preflight(&run, &runner).unwrap(),
        ReleasePreflight::AlreadySatisfied { .. }
    ));
}

#[test]
fn a_build_that_produces_no_sdist_is_an_error_before_any_upload() {
    let run = Run::new();
    let runner =
        PypiRunner::new(&[(WHEEL, "wheel bytes")]).with(|world| world.visible = vec![held(WHEEL, "wheel bytes")]);
    let error = preflight(&run, &runner).unwrap_err();
    assert!(
        matches!(error, GraphError::PypiBuildOutputIncomplete { missing: "sdist", .. }),
        "{error:?}"
    );
    assert_eq!(runner.count("twine"), 0);
}

/// A build job's output directory and the manifest sealing it.
struct Sealed {
    dir: tempfile::TempDir,
    manifest: callisto_model::ArtifactManifestV1,
}

impl Sealed {
    fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        for (name, content) in files {
            std::fs::write(dir.path().join(name), content).unwrap();
        }
        let sealed = files
            .iter()
            .map(|(name, content)| callisto_model::DistributionFileV1 {
                filename: name.to_string(),
                digest: callisto_model::ArtifactDigest::from_bytes(content.as_bytes()),
                byte_length: content.len() as u64,
            })
            .collect::<Vec<_>>();
        let distributions = if sealed.is_empty() {
            Vec::new()
        } else {
            vec![callisto_model::DistributionSetV1 {
                package: ReleasePackageId::new(Ecosystem::Pypi, "my-pkg").unwrap(),
                version: "1.0.0".to_string(),
                files: sealed,
            }]
        };
        let manifest = callisto_model::ArtifactManifestV1 {
            schema_version: 1,
            intent_digest: callisto_model::IntentDigest::parse(&"a".repeat(64)).unwrap(),
            source_commit: callisto_model::CommitSha::parse(&"b".repeat(40)).unwrap(),
            entries: Vec::new(),
            distributions,
        };
        Self { dir, manifest }
    }

    fn verified(&self) -> VerifiedArtifactManifest<'_> {
        VerifiedArtifactManifest::for_tests(&self.manifest, self.dir.path().to_path_buf())
    }
}

#[test]
fn a_sealed_set_is_uploaded_as_sealed_without_building() {
    let run = Run::new();
    let sealed = Sealed::new(&BUILD);
    let artifacts = sealed.verified();
    let runner = PypiRunner::new(&[("other-1.0.0.tar.gz", "a rebuild would differ")]);
    assert!(matches!(
        preflight_with(&run, &runner, Some(&artifacts)).unwrap(),
        ReleasePreflight::Proceed { .. }
    ));
    let confirmed = publish_and_confirm_with(&run, &runner, Some(&artifacts)).unwrap();
    assert!(confirmed.is_terminal_success(), "{confirmed:?}");
    assert_eq!(runner.count("python3") + runner.count("python"), 0, "nothing was built");
    assert_eq!(runner.uploads().concat().len(), 2);
}

#[test]
fn a_sealed_rerun_uploads_only_the_file_the_index_lacks() {
    let run = Run::new();
    let sealed = Sealed::new(&BUILD);
    let artifacts = sealed.verified();
    let runner = PypiRunner::new(&[]).with(|world| world.visible = vec![held(WHEEL, "wheel bytes")]);
    assert!(matches!(
        preflight_with(&run, &runner, Some(&artifacts)).unwrap(),
        ReleasePreflight::Proceed { .. }
    ));
    publish_and_confirm_with(&run, &runner, Some(&artifacts)).unwrap();
    assert_eq!(runner.uploads(), vec![vec![SDIST.to_string()]]);
}

#[test]
fn a_complete_sealed_release_is_satisfied_without_a_build_or_an_upload() {
    let run = Run::new();
    let sealed = Sealed::new(&BUILD);
    let artifacts = sealed.verified();
    let runner = PypiRunner::new(&[]).with(|world| {
        world.visible = vec![held(WHEEL, "wheel bytes"), held(SDIST, "sdist bytes")];
    });
    assert!(matches!(
        preflight_with(&run, &runner, Some(&artifacts)).unwrap(),
        ReleasePreflight::AlreadySatisfied { .. }
    ));
    assert_eq!(
        runner.count("twine") + runner.count("python3") + runner.count("python"),
        0
    );
}

/// A rebuild may legitimately differ, but a sealed set may not: the index holding other bytes under a sealed name
/// is an error before any upload.
#[test]
fn an_index_file_with_other_bytes_than_the_sealed_file_is_an_error_before_any_upload() {
    let run = Run::new();
    let sealed = Sealed::new(&BUILD);
    let artifacts = sealed.verified();
    let runner = PypiRunner::new(&[]).with(|world| world.visible = vec![held(WHEEL, "someone else's wheel")]);
    let error = preflight_with(&run, &runner, Some(&artifacts)).unwrap_err();
    assert!(matches!(error, GraphError::PypiFileDiffers { .. }), "{error:?}");
    let error = publish_and_confirm_with(&run, &runner, Some(&artifacts)).unwrap_err();
    assert!(matches!(error, GraphError::PypiFileDiffers { .. }), "{error:?}");
    assert_eq!(runner.count("twine"), 0);
}

#[test]
fn a_manifest_that_seals_no_python_files_leaves_the_build_to_this_job() {
    let run = Run::new();
    let sealed = Sealed::new(&[]);
    let artifacts = sealed.verified();
    let runner = PypiRunner::new(&BUILD);
    assert!(matches!(
        preflight_with(&run, &runner, Some(&artifacts)).unwrap(),
        ReleasePreflight::Proceed { .. }
    ));
    publish_and_confirm_with(&run, &runner, Some(&artifacts)).unwrap();
    assert_eq!(runner.count("python3") + runner.count("python"), 1);
}

#[test]
fn a_build_pins_source_date_epoch_to_the_release_commit_time() {
    let run = Run::new();
    let runner = PypiRunner::new(&BUILD).with(|world| world.visible = vec![held(WHEEL, "wheel bytes")]);
    preflight(&run, &runner).unwrap();
    assert_eq!(runner.epochs(), vec!["1700000000".to_string()]);
}

/// Verification happened when the run started; the upload must not trust it blindly.
#[test]
fn a_sealed_file_changed_after_verification_is_not_uploaded() {
    let run = Run::new();
    let sealed = Sealed::new(&BUILD);
    let artifacts = sealed.verified();
    let runner = PypiRunner::new(&[]);
    std::fs::write(sealed.dir.path().join(WHEEL), "swapped after verification").unwrap();
    let error = publish_and_confirm_with(&run, &runner, Some(&artifacts)).unwrap_err();
    assert!(matches!(error, GraphError::ArtifactBytesMismatch { .. }), "{error:?}");
    assert_eq!(runner.count("twine"), 0);
}

#[test]
fn an_index_that_serves_no_sha256_cannot_confirm_a_sealed_file() {
    let run = Run::new();
    let sealed = Sealed::new(&BUILD);
    let artifacts = sealed.verified();
    let mut unhashed = held(WHEEL, "wheel bytes");
    unhashed.sha256 = String::new();
    let runner = PypiRunner::new(&[]).with(|world| world.visible = vec![unhashed]);
    let error = preflight_with(&run, &runner, Some(&artifacts)).unwrap_err();
    assert!(matches!(error, GraphError::PypiFileDiffers { .. }), "{error:?}");
}

#[test]
fn a_sealed_file_that_is_not_a_distribution_of_the_release_is_an_error() {
    let run = Run::new();
    let sealed = Sealed::new(&[
        ("other_pkg-9.9.9-py3-none-any.whl", "wheel"),
        ("other_pkg-9.9.9.tar.gz", "sdist"),
    ]);
    let artifacts = sealed.verified();
    let runner = PypiRunner::new(&[]);
    let error = publish_and_confirm_with(&run, &runner, Some(&artifacts)).unwrap_err();
    assert!(matches!(error, GraphError::ReleaseInvariant { .. }), "{error:?}");
    assert_eq!(runner.count("twine"), 0);
}
