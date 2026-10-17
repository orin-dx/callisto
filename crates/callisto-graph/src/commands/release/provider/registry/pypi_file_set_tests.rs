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
    format!("{:x}", Sha256::digest(content.as_bytes()))
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
        let mut world = self.0.lock().unwrap();
        world.calls.push(format!("{program} {}", args.join(" ")));
        match program {
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
                            "hashes": { "sha256": file.sha256 },
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
    let operation = operation();
    PypiRegistry.preflight(&run.context(runner), &id(&operation), &operation)
}

/// Publishes whatever the index lacks and confirms it, as the provider does after an absent preflight.
fn publish_and_confirm(run: &Run, runner: &PypiRunner) -> Result<ProviderObservationV1, GraphError> {
    let operation = operation();
    let context = run.context(runner);
    let output = PypiRegistry.publish(&context, &id(&operation), &operation)?;
    let outcome = PypiRegistry.classify(&output).map_err(|source| GraphError::Registry {
        package: operation.package_name.clone(),
        source,
    })?;
    PypiRegistry.confirm(&context, &operation, matches!(outcome, PublishOutcome::Published))
}

/// A confirm that rebuilt would compare an upload with fresh bytes, so a run without the cache fails loudly.
#[test]
fn a_python_release_without_the_per_run_cache_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    let runner = PypiRunner::new(&BUILD).with(|world| world.visible = vec![held(WHEEL, "wheel bytes")]);
    let context = ProviderContext::new(root.path(), &runner, None);
    let operation = operation();
    let error = PypiRegistry
        .preflight(&context, &id(&operation), &operation)
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
    assert!(matches!(error, GraphError::PypiUploadedFileDiffers { .. }), "{error:?}");
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
