//! The registry-publish role: one adapter per ecosystem, selected once.

use callisto_model::{
    normalize_pypi_project_name, CommandOutput, Ecosystem, ExactEvidence, ProviderEvidenceV1,
    ProviderIndeterminateCause, ProviderObservationV1, PublishOutcome, RegistryError, RegistryKey, Version,
};

use crate::commands::registry_argv;
use crate::error::{CommandFailure, ReleasePreconditionRequirement, RemoteConflict, UnsupportedReleaseFeature};
use crate::GraphError;

use super::http::parse_http_response;
use super::policy::{self, programs, require_registry_confirmation, run_observation, timeouts, Attempt};
use super::{
    preflight_from_observation, wrong_role, EffectAuthorization, PreparedOperation, ProviderCapabilities,
    ProviderContext, ProviderRequest, RegistryPublishOperation, ReleasePreflight, ReleaseProvider,
};

pub(crate) struct RegistryProvider;

impl ReleaseProvider for RegistryProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        // Every registry answers an observation. Whether that answer can be
        // exact is the adapter's `can_observe_versions`.
        ProviderCapabilities {
            can_observe: true,
            can_publish: true,
        }
    }

    fn observe(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
    ) -> Result<ProviderObservationV1, GraphError> {
        let operation = registry_operation(request)?;
        registry_observation(adapter_for(request.id.package.ecosystem())?, context, operation)
    }

    fn preflight_conflict(&self) -> RemoteConflict {
        RemoteConflict::RegistryVersionDiffers
    }

    fn preflight(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
    ) -> Result<ReleasePreflight, GraphError> {
        match self.observe(context, request)? {
            // A registry with no query API at all has nothing to observe
            // before the effect; the post-effect confirmation fails closed.
            ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::UnsupportedProvider,
            } => Ok(proceed()),
            observation => preflight_from_observation(observation, request.id, self.preflight_conflict()),
        }
    }

    fn publish(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
        _effect: &EffectAuthorization<'_>,
    ) -> Result<ExactEvidence, GraphError> {
        let operation = registry_operation(request)?;
        let adapter = adapter_for(request.id.package.ecosystem())?;
        let output = adapter.publish(context, operation)?;
        let outcome = adapter.classify(&output).map_err(|source| GraphError::Registry {
            package: operation.package_name.clone(),
            source,
        })?;
        // Neither a zero exit nor the client's "already exists" text is a
        // receipt: a yanked version is not the version this intent authorized,
        // so only an exact observation may satisfy the operation.
        let confirmed = if matches!(outcome, PublishOutcome::Published) && adapter.can_observe_versions() {
            confirmed_registry_observation(adapter, context, operation)?
        } else {
            registry_observation(adapter, context, operation)?
        };
        require_registry_confirmation(
            confirmed.is_terminal_success(),
            &operation.package_name,
            &operation.version,
        )?;
        confirmed
            .exact_evidence()
            .ok_or_else(|| GraphError::RegistryPublishUnconfirmed {
                package: operation.package_name.clone(),
                version: operation.version.clone(),
            })
    }
}

fn proceed() -> ReleasePreflight {
    ReleasePreflight::Proceed {
        proof: ProviderObservationV1::Absent
            .absent_proof()
            .expect("an absent observation mints an absent proof"),
    }
}

fn registry_operation<'a>(request: &ProviderRequest<'a>) -> Result<&'a RegistryPublishOperation, GraphError> {
    match request.operation {
        PreparedOperation::RegistryPublish(operation) => Ok(operation),
        _ => Err(wrong_role(request.id, "registry publish")),
    }
}

/// The one registry observation used by preflight and post-publish
/// confirmation, so the two can never disagree.
fn registry_observation(
    adapter: &'static dyn RegistryEcosystem,
    context: &ProviderContext<'_>,
    operation: &RegistryPublishOperation,
) -> Result<ProviderObservationV1, GraphError> {
    if !adapter.can_observe_versions() {
        return Ok(ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::UnsupportedProvider,
        });
    }
    policy::retry_observation(context.sleeper(), || adapter.observe_once(context, operation))
}

/// The same observation, but an absence is treated as index-propagation lag
/// rather than an answer: this only runs after a publish client reported the
/// version as newly published.
fn confirmed_registry_observation(
    adapter: &'static dyn RegistryEcosystem,
    context: &ProviderContext<'_>,
    operation: &RegistryPublishOperation,
) -> Result<ProviderObservationV1, GraphError> {
    policy::retry_observation(context.sleeper(), || {
        Ok(match adapter.observe_once(context, operation)? {
            Attempt::Settled(ProviderObservationV1::Absent) => Attempt::Transient {
                value: ProviderObservationV1::Absent,
                retry_after: None,
            },
            settled_or_transient => settled_or_transient,
        })
    })
}

/// One ecosystem's registry client. This is the single place the release path
/// branches on [`Ecosystem`]: adding a registry means adding an adapter here,
/// not another match in preflight, dispatch, or confirmation.
pub(crate) trait RegistryEcosystem {
    /// Whether this registry exposes a query that proves an exact published
    /// version. `false` makes every observation `Indeterminate`, so a publish
    /// is never confirmed here and instead fails closed at the mandatory
    /// post-effect exact observation.
    fn can_observe_versions(&self) -> bool;

    /// One observation attempt. Only called when [`Self::can_observe_versions`].
    fn observe_once(
        &self,
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
    ) -> Result<Attempt<ProviderObservationV1>, GraphError>;

    /// Runs the publish client and returns the output that decides the outcome.
    fn publish(
        &self,
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
    ) -> Result<CommandOutput, GraphError>;

    fn classify(&self, output: &CommandOutput) -> Result<PublishOutcome, RegistryError>;
}

/// The production adapter set, keyed by ecosystem.
pub(crate) fn adapter_for(ecosystem: Ecosystem) -> Result<&'static dyn RegistryEcosystem, GraphError> {
    match ecosystem {
        Ecosystem::Cargo => Ok(&CargoRegistry),
        Ecosystem::Npm => Ok(&NpmRegistry),
        Ecosystem::Pypi => Ok(&PypiRegistry),
        _ => Err(GraphError::UnsupportedRelease {
            feature: UnsupportedReleaseFeature::Ecosystem,
        }),
    }
}

/// Runs an [`registry_argv::Argv`] built by the pure argv layer. This is the
/// only place an adapter touches [`callisto_model::CommandRunner`] --
/// everything about *what* to run was already decided by `registry_argv`.
fn run_argv(context: &ProviderContext<'_>, argv: &registry_argv::Argv) -> Result<CommandOutput, GraphError> {
    let args: Vec<&str> = argv.args.iter().map(String::as_str).collect();
    Ok(context
        .runner()
        .run_with_timeout(&argv.program, &args, &argv.cwd, timeouts::PUBLISH)?)
}

fn transient(
    cause: ProviderIndeterminateCause,
    retry_after: Option<std::time::Duration>,
) -> Attempt<ProviderObservationV1> {
    Attempt::Transient {
        value: ProviderObservationV1::Indeterminate { cause },
        retry_after,
    }
}

fn settled(observation: ProviderObservationV1) -> Attempt<ProviderObservationV1> {
    Attempt::Settled(observation)
}

struct CargoRegistry;

/// The name the *cargo* client knows this registry by. Callisto's logical
/// `cratesIo` key is cargo's built-in `crates-io`; every other key is spelled
/// the same in `.cargo/config.toml` as it is in `callisto.toml`.
/// The cargo registry name for a logical registry key (`cratesIo` is cargo's `crates-io`).
pub fn cargo_registry_name(key: &RegistryKey) -> &str {
    if key.as_str() == RegistryKey::CRATES_IO {
        "crates-io"
    } else {
        key.as_str()
    }
}

/// The observation argv: `cargo info NAME@VERSION --registry REGISTRY`.
///
/// `--registry` is never omitted. Without it `cargo info` resolves the local
/// workspace and reports an unpublished member's on-disk version as published
/// (`version: X (from ./)`), which is the defect this observation exists to
/// avoid; with it, cargo reads only the named registry.
///
/// `net.retry=0` must precede `info` (cargo ignores it after the subcommand):
/// callisto's bounded retry owns retrying, and cargo's own backoff inside each
/// attempt made an unreachable registry take ~11s per attempt.
fn cargo_info_args<'a>(spec: &'a str, registry: &'a str) -> [&'a str; 6] {
    ["--config", "net.retry=0", "info", spec, "--registry", registry]
}

impl RegistryEcosystem for CargoRegistry {
    fn can_observe_versions(&self) -> bool {
        true
    }

    /// Runs from the source workspace root, so the same `.cargo/config.toml`
    /// registry definitions and credentials the publish effect uses apply --
    /// private registries and their auth are cargo's business, not ours.
    fn observe_once(
        &self,
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
    ) -> Result<Attempt<ProviderObservationV1>, GraphError> {
        let spec = format!("{}@{}", operation.package_name, operation.version.render());
        let registry = cargo_registry_name(&operation.registry.key);
        let args = cargo_info_args(&spec, registry);
        let output = match run_observation(
            context.runner(),
            programs::CARGO,
            &args,
            context.root(),
            timeouts::REGISTRY_QUERY,
            true,
        )? {
            Ok(output) => output,
            Err(_unavailable) => return Ok(transient(ProviderIndeterminateCause::CommandFailed, None)),
        };
        Ok(classify_cargo_info(&output, &spec, &operation.version))
    }

    fn publish(
        &self,
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
    ) -> Result<CommandOutput, GraphError> {
        let registry_key =
            (operation.registry.key.as_str() != RegistryKey::CRATES_IO).then(|| operation.registry.key.as_str());
        let argv = registry_argv::cargo_publish_argv(
            context.root(),
            &operation.package_dir,
            &operation.package_name,
            &operation.version,
            registry_key,
        )?;
        run_argv(context, &argv)
    }

    fn classify(&self, output: &CommandOutput) -> Result<PublishOutcome, RegistryError> {
        registry_argv::classify_cargo_output(output)
    }
}

/// Classifies one real `cargo info` run. There is no `--json`, so the two
/// answers cargo actually proves are read from its exit code and its text:
///
/// - exit 0 with a `version: VERSION` line for the requested version: `Exact`.
///   `cargo info` reports neither the index checksum nor the yank flag, so the
///   evidence says `None` for both rather than inventing them.
/// - exit 101 with ``could not find `NAME@VERSION` ``: `Absent`.
/// - anything else -- another exit code, another message (a network failure
///   prints ``failed to load source for dependency``), or a zero exit with no
///   matching version line: transient `Indeterminate`, retried by the bounded
///   policy. An unreachable registry must never read as an absence.
///
/// A *yanked* version reads as `Absent`, because `cargo info` cannot see yanks
/// and answers "could not find" for one. That fails closed rather than open:
/// the publish attempt that follows is refused by the registry, and since only
/// an `Exact` observation may satisfy an operation, the run ends in the typed
/// `RegistryPublishUnconfirmed` error instead of a receipt.
fn classify_cargo_info(output: &CommandOutput, spec: &str, version: &Version) -> Attempt<ProviderObservationV1> {
    if output.success() {
        return if reports_version(&output.stdout, version) {
            settled(ProviderObservationV1::Exact {
                evidence: ProviderEvidenceV1::RegistryVersion {
                    version: version.clone(),
                    checksum: None,
                    yanked: None,
                },
            })
        } else {
            transient(ProviderIndeterminateCause::MalformedResponse, None)
        };
    }
    if output.exit_code == Some(101) && output.stderr.contains(&format!("could not find `{spec}`")) {
        return settled(ProviderObservationV1::Absent);
    }
    transient(ProviderIndeterminateCause::CommandFailed, None)
}

/// Whether cargo's `version: X ...` line names exactly the requested version.
fn reports_version(stdout: &str, version: &Version) -> bool {
    stdout.lines().any(|line| {
        line.trim()
            .strip_prefix("version: ")
            .and_then(|rest| rest.strip_prefix(version.render()))
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    })
}

struct NpmRegistry;

impl RegistryEcosystem for NpmRegistry {
    fn can_observe_versions(&self) -> bool {
        true
    }

    /// `npm view` keeps the workspace cwd: unlike a manifest-resolving client
    /// it never reads the local package, and the cwd is only where the project
    /// `.npmrc` supplying the registry is read from.
    fn observe_once(
        &self,
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
    ) -> Result<Attempt<ProviderObservationV1>, GraphError> {
        let spec = format!("{}@{}", operation.package_name, operation.version.render());
        // Callisto's bounded retry owns retrying; npm's own fetch retries made an
        // unreachable registry take ~70s per attempt.
        let mut args = vec!["view", spec.as_str(), "--json", "--fetch-retries=0"];
        if let Some(registry) = operation.registry.endpoint.as_deref() {
            args.extend(["--registry", registry]);
        }
        let output = match run_observation(
            context.runner(),
            programs::NPM,
            &args,
            context.root(),
            timeouts::REGISTRY_QUERY,
            true,
        )? {
            Ok(output) => output,
            Err(_unavailable) => return Ok(transient(ProviderIndeterminateCause::CommandFailed, None)),
        };
        if output.success() {
            return Ok(if output.stdout_trimmed().is_empty() {
                settled(ProviderObservationV1::Absent)
            } else {
                // `npm view` reports no package checksum, so the evidence says
                // so rather than inventing one.
                settled(ProviderObservationV1::Exact {
                    evidence: ProviderEvidenceV1::RegistryVersion {
                        version: operation.version.clone(),
                        checksum: None,
                        yanked: None,
                    },
                })
            });
        }
        let details = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
        if details.contains("e404")
            || details.contains("etarget")
            || details.contains("no matching version")
            || details.contains("is not in this registry")
        {
            return Ok(settled(ProviderObservationV1::Absent));
        }
        Ok(transient(ProviderIndeterminateCause::CommandFailed, None))
    }

    fn publish(
        &self,
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
    ) -> Result<CommandOutput, GraphError> {
        if operation.by_directory {
            let argv = registry_argv::npm_publish_directory_argv(
                context.root(),
                &operation.package_dir,
                &operation.package_name,
                operation.npm_tag.as_deref(),
                operation.npm_access,
                operation.registry.endpoint.as_deref(),
            );
            return run_argv(context, &argv);
        }
        let package_manager = registry_argv::detect_npm_package_manager(context.root());
        let argv = registry_argv::npm_publish_argv(
            context.root(),
            &operation.package_dir,
            &operation.package_name,
            package_manager,
            operation.npm_tag.as_deref(),
            operation.npm_access,
            operation.registry.endpoint.as_deref(),
        );
        run_argv(context, &argv)
    }

    fn classify(&self, output: &CommandOutput) -> Result<PublishOutcome, RegistryError> {
        registry_argv::classify_npm_publish_output(output)
    }
}

struct PypiRegistry;

/// PyPI's own PEP 691 JSON simple index, not `pip`: `pip index versions` is
/// experimental and hides yanked releases, and an unreachable index and a
/// missing project both look like "no versions found" to it. The index itself
/// answers plainly -- 200 with a `files[]` entry naming the version, or 404 --
/// so observation goes straight to `https://pypi.org/simple/<project>/`
/// (or the configured private index) over `curl`, never through `pip`.
const PYPI_DEFAULT_SIMPLE_INDEX: &str = "https://pypi.org/simple";

/// PEP 691: the versioned JSON media type, so the index cannot fall back to
/// the legacy HTML page a bare GET would otherwise serve.
const PYPI_SIMPLE_ACCEPT: &str = "application/vnd.pypi.simple.v1+json";

/// The project's simple-index URL: the configured private index if one is
/// bound (already https-validated in `binding.rs`), else the public default.
fn pypi_simple_index_url(endpoint: Option<&str>, package_name: &str) -> String {
    let base = endpoint.unwrap_or(PYPI_DEFAULT_SIMPLE_INDEX);
    let project = normalize_pypi_project_name(package_name);
    format!("{}/{project}/", base.trim_end_matches('/'))
}

/// One file entry of a PEP 691 JSON simple-index response -- only the fields
/// the observation reads.
#[derive(serde::Deserialize)]
struct PypiSimpleFile {
    filename: String,
    /// `false` when not yanked, or a (possibly empty) reason string when it is.
    #[serde(default)]
    yanked: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct PypiSimpleIndex {
    files: Vec<PypiSimpleFile>,
}

impl PypiSimpleFile {
    fn is_yanked(&self) -> bool {
        !matches!(self.yanked, None | Some(serde_json::Value::Bool(false)))
    }
}

/// The version segment of a simple-index filename, or `None` for an extension
/// this parser does not recognize (a legacy installer, say).
///
/// A wheel's distribution segment is PEP 427-escaped to underscores only, so
/// splitting on `-` unambiguously yields `version` second. A PEP 440 version
/// never itself contains a hyphen, so an sdist's *last* `-` is always the
/// name/version boundary regardless of how the distribution name is spelled.
fn pypi_simple_file_version(filename: &str) -> Option<&str> {
    if let Some(stem) = filename.strip_suffix(".whl") {
        return stem.split('-').nth(1);
    }
    [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.Z", ".zip"]
        .into_iter()
        .find_map(|ext| filename.strip_suffix(ext))
        .and_then(|stem| stem.rsplit_once('-'))
        .map(|(_, version)| version)
}

/// Whether a simple-index file's filename names exactly the requested version.
fn pypi_simple_file_matches(file: &PypiSimpleFile, version: &Version) -> bool {
    pypi_simple_file_version(&file.filename).is_some_and(|candidate| {
        Version::parse(candidate, version.grammar())
            .is_ok_and(|candidate| candidate.compare(version) == Ok(std::cmp::Ordering::Equal))
    })
}

/// Classifies a parsed 200 body: the matching file's yank status decides the
/// answer, mirroring cargo's yanked-is-absent rationale above -- a yanked
/// release is not the version this intent authorized, so it fails closed.
fn classify_pypi_simple_body(body: &str, version: &Version) -> Attempt<ProviderObservationV1> {
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
    settled(ProviderObservationV1::Exact {
        evidence: ProviderEvidenceV1::RegistryVersion {
            version: version.clone(),
            checksum: None,
            yanked: Some(false),
        },
    })
}

/// Classifies one `curl -sS -i` run against the simple index. A private index
/// that ignores `Accept` and serves its legacy HTML page fails the JSON parse
/// inside [`classify_pypi_simple_body`] and reads as `MalformedResponse`, same
/// as any other body PEP 691 JSON parsing cannot make sense of.
///
/// Any other status shares [`HttpResponse::is_transient`] with GitHub's `gh api` reads, so it retries on
/// `Retry-After` instead of settling immediately as `MalformedResponse`.
fn classify_pypi_simple(output: &CommandOutput, version: &Version) -> Attempt<ProviderObservationV1> {
    let response = match parse_http_response(&output.stdout) {
        Ok(response) => response,
        Err(_) if !output.success() => return transient(ProviderIndeterminateCause::CommandFailed, None),
        Err(_) => return transient(ProviderIndeterminateCause::MalformedResponse, None),
    };
    match response.status {
        200 => classify_pypi_simple_body(&response.body, version),
        404 => settled(ProviderObservationV1::Absent),
        _ if response.is_transient() => {
            transient(ProviderIndeterminateCause::MalformedResponse, response.retry_after())
        }
        _ => settled(ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::MalformedResponse,
        }),
    }
}

impl RegistryEcosystem for PypiRegistry {
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
        let url = pypi_simple_index_url(operation.registry.endpoint.as_deref(), &operation.package_name);
        let accept_header = format!("Accept: {PYPI_SIMPLE_ACCEPT}");
        let args = ["-sS", "-i", "-H", accept_header.as_str(), url.as_str()];
        let output = match run_observation(
            context.runner(),
            programs::CURL,
            &args,
            context.root(),
            timeouts::REGISTRY_QUERY,
            false,
        )? {
            Ok(output) => output,
            Err(_unavailable) => return Ok(transient(ProviderIndeterminateCause::CommandFailed, None)),
        };
        Ok(classify_pypi_simple(&output, &operation.version))
    }

    fn publish(
        &self,
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
    ) -> Result<CommandOutput, GraphError> {
        let steps = registry_argv::pypi_publish_argv(
            context.root(),
            &operation.package_dir,
            &operation.package_name,
            &operation.version,
            operation.registry.endpoint.as_deref(),
        )?;
        let [build, upload] = steps.as_slice() else {
            return Err(GraphError::ReleaseInvariant {
                detail: "pypi_publish_argv did not return exactly a build and an upload step".to_string(),
            });
        };
        let built = run_argv(context, build)?;
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
        run_argv(context, upload)
    }

    fn classify(&self, output: &CommandOutput) -> Result<PublishOutcome, RegistryError> {
        registry_argv::classify_twine_output(output)
    }
}

/// The plan-time gate: an ecosystem whose client cannot prove a version absent
/// could never mint the absence proof an effect needs, nor the exact evidence a
/// receipt needs, so the plan is refused here rather than at dispatch.
pub(crate) fn require_observable_registry(ecosystem: Ecosystem) -> Result<(), GraphError> {
    if !adapter_for(ecosystem)?.can_observe_versions() {
        return Err(GraphError::ReleasePreconditionUnmet {
            requirement: ReleasePreconditionRequirement::ObservableRegistryClient,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
