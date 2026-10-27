//! The pypi registry adapter: how a pypi publish is observed, classified and run.

use super::*;

pub(super) fn run_pypi_upload_argv(
    context: &ProviderContext<'_>,
    argv: &registry_argv::Argv,
    timeout: Duration,
) -> Result<CommandOutput, GraphError> {
    let username = std::env::var("TWINE_USERNAME").ok();
    let password = std::env::var("TWINE_PASSWORD").ok();
    let env = pypi_publisher_environment(username.as_deref(), password.as_deref());
    let args: Vec<&str> = argv.args.iter().map(String::as_str).collect();
    Ok(context
        .runner()
        .run_with_timeout_publisher(&argv.program, &args, &argv.cwd, timeout, &env)?)
}

pub(super) fn pypi_publisher_environment<'a>(
    username: Option<&'a str>,
    password: Option<&'a str>,
) -> Vec<(&'static str, &'a str)> {
    let mut env = Vec::new();
    if let Some(username) = username {
        env.push(("TWINE_USERNAME", username));
    }
    if let Some(password) = password {
        env.push(("TWINE_PASSWORD", password));
    }
    env
}

pub(super) struct PypiRegistry;

/// PyPI's own PEP 691 JSON simple index, not `pip`: `pip index versions` is
/// experimental and hides yanked releases, and an unreachable index and a
/// missing project both look like "no versions found" to it. The index itself
/// answers plainly -- 200 with a `files[]` entry naming the version, or 404 --
/// so observation goes straight to `https://pypi.org/simple/<project>/`
/// (or the configured private index) over `curl`, never through `pip`.
pub(super) const PYPI_DEFAULT_SIMPLE_INDEX: &str = "https://pypi.org/simple";

/// PEP 691: the versioned JSON media type, so the index cannot fall back to
/// the legacy HTML page a bare GET would otherwise serve.
pub(super) const PYPI_SIMPLE_ACCEPT: &str = "application/vnd.pypi.simple.v1+json";

/// The project's simple-index URL: the configured private index if one is
/// bound (already https-validated in `binding.rs`), else the public default.
pub(super) fn pypi_simple_index_url(endpoint: Option<&str>, package_name: &str) -> String {
    let base = endpoint.unwrap_or(PYPI_DEFAULT_SIMPLE_INDEX);
    let project = normalize_pypi_project_name(package_name);
    format!("{}/{project}/", base.trim_end_matches('/'))
}

/// The version segment of a simple-index filename, or `None` for an extension
/// this parser does not recognize (a legacy installer, say).
pub(super) fn pypi_simple_file_version(filename: &str) -> Option<&str> {
    callisto_model::pypi_dist_filename_parts(filename).map(|(_, version)| version)
}

/// Whether a simple-index file's filename names exactly the requested version.
pub(super) fn pypi_simple_file_matches(file: &PypiSimpleFile, version: &Version) -> bool {
    pypi_simple_file_version(&file.filename).is_some_and(|candidate| {
        Version::parse(candidate, version.grammar())
            .is_ok_and(|candidate| candidate.compare(version) == Ok(std::cmp::Ordering::Equal))
    })
}

/// Classifies a parsed 200 body: the matching file's yank status decides the
/// answer, mirroring cargo's yanked-is-absent rationale above -- a yanked
/// release is not the version this intent authorized, so it fails closed.
pub(super) fn classify_pypi_simple_body(body: &str, version: &Version) -> Attempt<ProviderObservationV1> {
    let Ok(index) = serde_json::from_str::<PypiSimpleIndex>(body) else {
        return transient(ProviderIndeterminateCause::MalformedResponse, None);
    };
    let matching = index
        .files
        .iter()
        .filter(|file| pypi_simple_file_matches(file, version));
    let mut matched = false;
    for file in matching {
        if file.is_yanked() {
            return settled(ProviderObservationV1::Absent);
        }
        matched = true;
    }
    if !matched {
        return settled(ProviderObservationV1::Absent);
    }
    settled(exact_version(version))
}

/// The observation that a version is on the registry, unyanked.
pub(super) fn exact_version(version: &Version) -> ProviderObservationV1 {
    ProviderObservationV1::Exact {
        evidence: ProviderEvidenceV1::RegistryVersion {
            version: version.clone(),
            checksum: None,
            yanked: Some(false),
        },
    }
}

/// What the index holds for one release.
pub(super) enum PypiRemote {
    Absent,
    Files(Vec<PypiSimpleFile>),
    Indeterminate(ProviderIndeterminateCause),
}

/// The release's own files from a parsed 200 body.
pub(super) fn classify_pypi_files_body(body: &str, version: &Version) -> Attempt<PypiRemote> {
    let Ok(index) = serde_json::from_str::<PypiSimpleIndex>(body) else {
        return Attempt::Transient {
            value: PypiRemote::Indeterminate(ProviderIndeterminateCause::MalformedResponse),
            retry_after: None,
        };
    };
    let files: Vec<_> = index
        .files
        .into_iter()
        .filter(|file| pypi_simple_file_matches(file, version))
        .collect();
    Attempt::Settled(if files.is_empty() {
        PypiRemote::Absent
    } else {
        PypiRemote::Files(files)
    })
}

/// Classifies one `curl -sS -i` run against the simple index. A private index
/// that ignores `Accept` and serves its legacy HTML page fails the JSON parse
/// inside `on_ok` and reads as `MalformedResponse`, same as any other body PEP
/// 691 JSON parsing cannot make sense of.
///
/// Any other status shares [`HttpResponse::is_transient`] with GitHub's `gh api` reads, so it retries on
/// `Retry-After` instead of settling immediately, and either way reports the status as `ProviderStatus`.
pub(super) fn classify_pypi_status<T>(
    output: &CommandOutput,
    on_ok: impl FnOnce(&str) -> Attempt<T>,
    absent: T,
    indeterminate: impl Fn(ProviderIndeterminateCause) -> T,
) -> Attempt<T> {
    let response = match parse_http_response(&output.stdout) {
        Ok(response) => response,
        Err(_) if !output.success() => {
            return Attempt::Transient {
                value: indeterminate(ProviderIndeterminateCause::CommandFailed),
                retry_after: None,
            }
        }
        Err(_) => {
            return Attempt::Transient {
                value: indeterminate(ProviderIndeterminateCause::MalformedResponse),
                retry_after: None,
            }
        }
    };
    match response.status {
        200 => on_ok(&response.body),
        404 => Attempt::Settled(absent),
        status if response.is_transient() => Attempt::Transient {
            value: indeterminate(ProviderIndeterminateCause::ProviderStatus { status }),
            retry_after: response.retry_after(),
        },
        status => Attempt::Settled(indeterminate(ProviderIndeterminateCause::ProviderStatus { status })),
    }
}

pub(super) fn classify_pypi_simple(output: &CommandOutput, version: &Version) -> Attempt<ProviderObservationV1> {
    classify_pypi_status(
        output,
        |body| classify_pypi_simple_body(body, version),
        ProviderObservationV1::Absent,
        |cause| ProviderObservationV1::Indeterminate { cause },
    )
}

pub(super) fn classify_pypi_files(output: &CommandOutput, version: &Version) -> Attempt<PypiRemote> {
    classify_pypi_status(
        output,
        |body| classify_pypi_files_body(body, version),
        PypiRemote::Absent,
        PypiRemote::Indeterminate,
    )
}

impl PypiRegistry {
    /// One `curl` of the project's simple index; `Err` when the command could not run at all.
    fn fetch_once(
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
    ) -> Result<Result<CommandOutput, policy::CommandUnavailable>, GraphError> {
        let url = pypi_simple_index_url(operation.registry.endpoint.as_deref(), &operation.package_name);
        let accept_header = format!("Accept: {PYPI_SIMPLE_ACCEPT}");
        let args = ["-sS", "-i", "-H", accept_header.as_str(), url.as_str()];
        run_observation(
            context.runner(),
            programs::CURL,
            &args,
            context.root(),
            timeouts::REGISTRY_QUERY,
            false,
        )
    }

    /// The release's files on the index, retried on a transient failure.
    fn remote(context: &ProviderContext<'_>, operation: &RegistryPublishOperation) -> Result<PypiRemote, GraphError> {
        policy::retry_observation(context.sleeper(), || {
            Ok(match Self::fetch_once(context, operation)? {
                Ok(output) => classify_pypi_files(&output, &operation.version),
                Err(_unavailable) => Attempt::Transient {
                    value: PypiRemote::Indeterminate(ProviderIndeterminateCause::CommandFailed),
                    retry_after: None,
                },
            })
        })
    }

    /// The files of this release, once per run: the build job's sealed set when the manifest has one, else a build.
    fn built_set(
        context: &ProviderContext<'_>,
        id: &ReleaseOperationId,
        operation: &RegistryPublishOperation,
        artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<Arc<BuiltSet>, GraphError> {
        let sets = context.file_sets().ok_or_else(|| GraphError::ReleaseInvariant {
            detail: "a Python release needs the per-run file set cache".to_string(),
        })?;
        sets.get_or_build(
            (operation.package_dir.clone(), operation.version.render().to_string()),
            || match artifacts.and_then(|artifacts| artifacts.sealed_files(&id.package, &operation.version)) {
                Some(files) => BuiltSet::sealed(
                    files
                        .into_iter()
                        .map(|file| BuiltFile {
                            filename: file.filename,
                            path: file.path,
                            sha256: file.sha256,
                        })
                        .collect(),
                    &operation.package_name,
                    &operation.version,
                ),
                None => {
                    if artifacts.is_some() {
                        eprintln!(
                            "warning: the artifact manifest does not seal {} {}; building it in this job",
                            operation.package_name,
                            operation.version.render()
                        );
                    }
                    Self::build_set(context, operation)
                }
            },
        )
    }

    /// Builds the sdist and wheel into a fresh directory, so a stale `dist/` can never reach the upload.
    fn build_set(context: &ProviderContext<'_>, operation: &RegistryPublishOperation) -> Result<BuiltSet, GraphError> {
        let dir = tempfile::tempdir().map_err(|error| {
            GraphError::Command(callisto_model::CommandError::Io {
                program: "python -m build".to_string(),
                message: error.to_string(),
            })
        })?;
        Self::build_into(context, operation, dir.path())?;
        BuiltSet::from_dir(dir, &operation.package_name, &operation.version)
    }

    /// Runs the build into `outdir`, with file timestamps pinned to the release commit.
    pub(crate) fn build_into(
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
        outdir: &std::path::Path,
    ) -> Result<(), GraphError> {
        let epoch = callisto_model::vcs::GitAccess::new(context.root(), context.runner()).commit_time("HEAD")?;
        let build = registry_argv::pypi_build_argv(context.root(), &operation.package_dir, outdir, epoch)?;
        let built = run_build_argv(context, &build)?;
        if built.exit_code != Some(0) {
            return Err(GraphError::ReleaseCommand {
                program: build.program.clone(),
                args: build.args.clone(),
                failure: CommandFailure::NonZeroExit {
                    exit_code: built.exit_code,
                    stderr: built.stderr,
                },
            });
        }
        Ok(())
    }
}

/// Files on the index that no build of this release declared are never checked, but they are worth a line.
pub(super) fn warn_about_extra_files(operation: &RegistryPublishOperation, extras: &[&str]) {
    if !extras.is_empty() {
        eprintln!(
            "warning: the index lists {} for {} {}, which this release did not build; it is not checked",
            extras.join(", "),
            operation.package_name,
            operation.version.render()
        );
    }
}

impl RegistryEcosystem for PypiRegistry {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Pypi
    }

    /// PyPI's own JSON simple index (PEP 691) answers exactly like every
    /// other registry: 200 with the version present or absent, 404 for an
    /// unknown project. `pip` cannot observe this; `curl` against the index
    /// directly can.
    fn can_observe_versions(&self) -> bool {
        true
    }

    fn observe_once(
        &self,
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
    ) -> Result<Attempt<ProviderObservationV1>, GraphError> {
        let output = match Self::fetch_once(context, operation)? {
            Ok(output) => output,
            Err(_unavailable) => return Ok(transient(ProviderIndeterminateCause::CommandFailed, None)),
        };
        Ok(classify_pypi_simple(&output, &operation.version))
    }

    /// No file on the index needs no build; any file means a partial release, so a build says which are missing.
    fn preflight(
        &self,
        context: &ProviderContext<'_>,
        id: &ReleaseOperationId,
        operation: &RegistryPublishOperation,
        artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<ReleasePreflight, GraphError> {
        let remote = match Self::remote(context, operation)? {
            PypiRemote::Absent => return Ok(proceed()),
            PypiRemote::Indeterminate(cause) => {
                return Err(GraphError::ReleaseProviderIndeterminate {
                    operation: Box::new(id.clone()),
                    cause,
                })
            }
            PypiRemote::Files(remote) => remote,
        };
        let set = Self::built_set(context, id, operation, artifacts)?;
        let comparison = pypi_files::compare(&set, &remote);
        warn_about_extra_files(operation, &comparison.extras);
        if let Some(filename) = comparison.differing.first() {
            return Err(GraphError::PypiFileDiffers {
                filename: (*filename).to_string(),
            });
        }
        if !comparison.yanked.is_empty() {
            return preflight_from_observation(
                ProviderObservationV1::Conflict {
                    reason: ProviderConflictReason::RegistryVersionYanked,
                },
                id,
                RemoteConflict::RegistryVersionDiffers,
            );
        }
        if comparison.is_complete() {
            return preflight_from_observation(
                exact_version(&operation.version),
                id,
                RemoteConflict::RegistryVersionDiffers,
            );
        }
        Ok(proceed())
    }

    /// Uploads only the files the index lacks, named one by one: a file already there is never offered again.
    fn publish(
        &self,
        context: &ProviderContext<'_>,
        id: &ReleaseOperationId,
        operation: &RegistryPublishOperation,
        artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<CommandOutput, GraphError> {
        let set = Self::built_set(context, id, operation, artifacts)?;
        let remote = match Self::remote(context, operation)? {
            PypiRemote::Absent => Vec::new(),
            PypiRemote::Files(remote) => remote,
            PypiRemote::Indeterminate(cause) => {
                return Err(GraphError::ReleaseProviderIndeterminate {
                    operation: Box::new(id.clone()),
                    cause,
                })
            }
        };
        let comparison = pypi_files::compare(&set, &remote);
        if let Some(filename) = comparison.differing.first() {
            return Err(GraphError::PypiFileDiffers {
                filename: (*filename).to_string(),
            });
        }
        if !comparison.yanked.is_empty() {
            return Err(GraphError::ReleaseRemoteConflict {
                conflict: RemoteConflict::RegistryVersionDiffers,
            });
        }
        let missing: Vec<&BuiltFile> = set
            .files
            .iter()
            .filter(|file| comparison.missing.contains(&file.filename.as_str()))
            .collect();
        if missing.is_empty() {
            return Ok(CommandOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            });
        }
        set.recheck(&missing)?;
        let paths: Vec<_> = missing.iter().map(|file| file.path.clone()).collect();
        let upload = registry_argv::pypi_upload_argv(
            context.root(),
            &operation.package_dir,
            operation.registry.upload_endpoint(),
            &paths,
        );
        let output = run_pypi_upload_argv(context, &upload, publish_timeout(operation))?;
        if output.success() {
            set.mark_uploaded(&missing);
        }
        Ok(output)
    }

    fn classify(&self, output: &CommandOutput) -> Result<PublishOutcome, RegistryError> {
        registry_argv::classify_twine_output(output)
    }

    /// Waits for every file to be on the index; a sealed or uploaded file must carry the bytes it was sealed or sent with.
    fn confirm(
        &self,
        context: &ProviderContext<'_>,
        id: &ReleaseOperationId,
        operation: &RegistryPublishOperation,
        artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<ProviderObservationV1, GraphError> {
        let set = Self::built_set(context, id, operation, artifacts)?;
        let observe = || -> Result<ProviderObservationV1, GraphError> {
            Ok(match Self::remote(context, operation)? {
                PypiRemote::Indeterminate(cause) => ProviderObservationV1::Indeterminate { cause },
                PypiRemote::Absent => ProviderObservationV1::Absent,
                PypiRemote::Files(remote) => {
                    let comparison = pypi_files::compare(&set, &remote);
                    if !comparison.yanked.is_empty() {
                        ProviderObservationV1::Conflict {
                            reason: ProviderConflictReason::RegistryVersionYanked,
                        }
                    } else if let Some(filename) = comparison.differing.first() {
                        return Err(GraphError::PypiFileDiffers {
                            filename: (*filename).to_string(),
                        });
                    } else if comparison.missing.is_empty() {
                        exact_version(&operation.version)
                    } else {
                        ProviderObservationV1::Absent
                    }
                }
            })
        };
        policy::observe_within_window(context.sleeper(), visibility_window(self, operation), observe)
    }
}
