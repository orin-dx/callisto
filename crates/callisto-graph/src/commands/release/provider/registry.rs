//! The registry-publish role: one adapter per ecosystem, selected once.

use std::sync::Arc;

use callisto_model::{
    normalize_pypi_project_name, CommandOutput, Ecosystem, ExactEvidence, ProviderConflictReason, ProviderEvidenceV1,
    ProviderIndeterminateCause, ProviderObservationV1, PublishOutcome, RegistryError, RegistryKey, ReleaseOperationId,
    Version,
};

use crate::commands::registry_argv;
use crate::error::{CommandFailure, ReleasePreconditionRequirement, RemoteConflict, UnsupportedReleaseFeature};
use crate::GraphError;

use super::http::parse_http_response;
use super::policy::{self, programs, require_registry_confirmation, run_observation, timeouts, Attempt};
use super::pypi_files::{self, BuiltFile, BuiltSet, PypiSimpleFile, PypiSimpleIndex};
use super::{
    preflight_from_observation, wrong_role, EffectAuthorization, PreparedOperation, ProviderCapabilities,
    ProviderContext, ProviderRequest, RegistryPublishOperation, ReleasePreflight, ReleaseProvider,
    VerifiedArtifactManifest,
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
        adapter_for(request.id.package.ecosystem())?.preflight(
            context,
            request.id,
            registry_operation(request)?,
            request.artifacts,
        )
    }

    fn publish(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
        _effect: &EffectAuthorization<'_>,
    ) -> Result<ExactEvidence, GraphError> {
        let operation = registry_operation(request)?;
        let adapter = adapter_for(request.id.package.ecosystem())?;
        let output = adapter.publish(context, request.id, operation, request.artifacts)?;
        let outcome = adapter.classify(&output).map_err(|source| GraphError::Registry {
            package: operation.package_name.clone(),
            source,
        })?;
        // Neither a zero exit nor the client's "already exists" text is a
        // receipt: a yanked version is not the version this intent authorized,
        // so only an exact observation may satisfy the operation.
        let confirmed = adapter.confirm(
            context,
            request.id,
            operation,
            matches!(outcome, PublishOutcome::Published),
            request.artifacts,
        )?;
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
fn registry_observation<A: RegistryEcosystem + ?Sized>(
    adapter: &A,
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
fn confirmed_registry_observation<A: RegistryEcosystem + ?Sized>(
    adapter: &A,
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
        id: &ReleaseOperationId,
        operation: &RegistryPublishOperation,
        artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<CommandOutput, GraphError>;

    fn classify(&self, output: &CommandOutput) -> Result<PublishOutcome, RegistryError>;

    /// What the one fresh pre-effect observation authorizes.
    fn preflight(
        &self,
        context: &ProviderContext<'_>,
        id: &ReleaseOperationId,
        operation: &RegistryPublishOperation,
        _artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<ReleasePreflight, GraphError> {
        match registry_observation(self, context, operation)? {
            // A registry with no query API at all has nothing to observe
            // before the effect; the post-effect confirmation fails closed.
            ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::UnsupportedProvider,
            } => Ok(proceed()),
            observation => preflight_from_observation(observation, id, RemoteConflict::RegistryVersionDiffers),
        }
    }

    /// The observation that lets a finished publish be recorded; `published` means the index may still lag.
    fn confirm(
        &self,
        context: &ProviderContext<'_>,
        _id: &ReleaseOperationId,
        operation: &RegistryPublishOperation,
        published: bool,
        _artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<ProviderObservationV1, GraphError> {
        if published && self.can_observe_versions() {
            confirmed_registry_observation(self, context, operation)
        } else {
            registry_observation(self, context, operation)
        }
    }
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

fn run_npm_publish_argv(
    context: &ProviderContext<'_>,
    argv: &registry_argv::Argv,
) -> Result<CommandOutput, GraphError> {
    let token = std::env::var("NPM_TOKEN").ok().filter(|value| !value.is_empty());
    let config = std::env::var("CALLISTO_PUBLISH_NPM_CONFIG")
        .ok()
        .filter(|value| !value.is_empty());
    let env = npm_publisher_environment(&argv.program, token.as_deref(), config.as_deref());
    let args: Vec<&str> = argv.args.iter().map(String::as_str).collect();
    Ok(context
        .runner()
        .run_with_timeout_publisher(&argv.program, &args, &argv.cwd, timeouts::PUBLISH, &env)?)
}

fn npm_publisher_environment<'a>(
    program: &str,
    token: Option<&'a str>,
    config: Option<&'a str>,
) -> Vec<(&'static str, &'a str)> {
    let mut env = Vec::new();
    if let Some(token) = token {
        env.push(("NPM_TOKEN", token));
        if program == "bun" {
            env.push(("NPM_CONFIG_TOKEN", token));
        }
    }
    if let Some(config) = config {
        env.push(("NPM_CONFIG_USERCONFIG", config));
    }
    env
}

fn run_pypi_upload_argv(
    context: &ProviderContext<'_>,
    argv: &registry_argv::Argv,
) -> Result<CommandOutput, GraphError> {
    let username = std::env::var("TWINE_USERNAME").ok();
    let password = std::env::var("TWINE_PASSWORD").ok();
    let env = pypi_publisher_environment(username.as_deref(), password.as_deref());
    let args: Vec<&str> = argv.args.iter().map(String::as_str).collect();
    Ok(context
        .runner()
        .run_with_timeout_publisher(&argv.program, &args, &argv.cwd, timeouts::PUBLISH, &env)?)
}

fn pypi_publisher_environment<'a>(
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

/// Withheld from a build backend, which runs project code and needs none of them.
const RELEASE_CREDENTIALS: &[&str] = &[
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
];

fn run_build_argv(context: &ProviderContext<'_>, argv: &registry_argv::Argv) -> Result<CommandOutput, GraphError> {
    let args: Vec<&str> = argv.args.iter().map(String::as_str).collect();
    Ok(context.runner().run_with_timeout_env_filtered(
        &argv.program,
        &args,
        &argv.cwd,
        timeouts::PUBLISH,
        RELEASE_CREDENTIALS,
    )?)
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

    /// Runs from the package's Cargo workspace directory, so the same `.cargo/config.toml`
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
        let cwd = registry_argv::cargo_workspace_dir(context.root(), &operation.package_dir)?;
        let output = match run_observation(
            context.runner(),
            programs::CARGO,
            &args,
            &cwd,
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
        _id: &ReleaseOperationId,
        operation: &RegistryPublishOperation,
        _artifacts: Option<&VerifiedArtifactManifest<'_>>,
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

/// The code npm gives a registry 404 for a package or a version.
const NPM_NOT_FOUND_CODE: &str = "E404";

/// Only npm's own `E404` code proves absence, never text around it; any other failure is a retried `CommandFailed`.
fn classify_npm_view(output: &CommandOutput, version: &Version) -> Attempt<ProviderObservationV1> {
    if output.success() {
        return if output.stdout_trimmed().is_empty() {
            transient(ProviderIndeterminateCause::MalformedResponse, None)
        } else {
            settled(ProviderObservationV1::Exact {
                evidence: ProviderEvidenceV1::RegistryVersion {
                    version: version.clone(),
                    checksum: None,
                    yanked: None,
                },
            })
        };
    }
    let code = if output.stdout_trimmed().is_empty() {
        npm_stderr_code(&output.stderr)
    } else {
        npm_error_code(&output.stdout)
    };
    if code.as_deref() == Some(NPM_NOT_FOUND_CODE) {
        return settled(ProviderObservationV1::Absent);
    }
    transient(ProviderIndeterminateCause::CommandFailed, None)
}

/// The `error.code` of the JSON error body `npm --json` prints to stdout on failure.
fn npm_error_code(stdout: &str) -> Option<String> {
    let body: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    body.get("error")?.get("code")?.as_str().map(str::to_owned)
}

/// The code on npm's `npm ERR! code X` or `npm error code X` stderr line; npm 8 prints no JSON body.
fn npm_stderr_code(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|line| {
        let rest = line
            .strip_prefix("npm ERR! ")
            .or_else(|| line.strip_prefix("npm error "))?;
        rest.strip_prefix("code ").map(|code| code.trim().to_owned())
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
        Ok(classify_npm_view(&output, &operation.version))
    }

    fn publish(
        &self,
        context: &ProviderContext<'_>,
        _id: &ReleaseOperationId,
        operation: &RegistryPublishOperation,
        _artifacts: Option<&VerifiedArtifactManifest<'_>>,
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
            return run_npm_publish_argv(context, &argv);
        }
        // The package manager and the directory it runs in belong to the workspace that holds the package.
        let workspace_dir = registry_argv::npm_workspace_dir(context.root(), &operation.package_dir)?;
        let workspace_root = context.root().join(&workspace_dir);
        let package_manager = registry_argv::detect_npm_package_manager(&workspace_root)?;
        let within_workspace = operation
            .package_dir
            .strip_prefix(&workspace_dir)
            .unwrap_or(&operation.package_dir);
        let argv = registry_argv::npm_publish_argv(
            &workspace_root,
            within_workspace,
            &operation.package_name,
            package_manager,
            operation.npm_tag.as_deref(),
            operation.npm_access,
            operation.registry.endpoint.as_deref(),
        );
        run_npm_publish_argv(context, &argv)
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

/// The version segment of a simple-index filename, or `None` for an extension
/// this parser does not recognize (a legacy installer, say).
fn pypi_simple_file_version(filename: &str) -> Option<&str> {
    callisto_model::pypi_dist_filename_parts(filename).map(|(_, version)| version)
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
    settled(exact_version(version))
}

/// The observation that a version is on the registry, unyanked.
fn exact_version(version: &Version) -> ProviderObservationV1 {
    ProviderObservationV1::Exact {
        evidence: ProviderEvidenceV1::RegistryVersion {
            version: version.clone(),
            checksum: None,
            yanked: Some(false),
        },
    }
}

/// What the index holds for one release.
enum PypiRemote {
    Absent,
    Files(Vec<PypiSimpleFile>),
    Indeterminate(ProviderIndeterminateCause),
}

/// The release's own files from a parsed 200 body.
fn classify_pypi_files_body(body: &str, version: &Version) -> Attempt<PypiRemote> {
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
fn classify_pypi_status<T>(
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

fn classify_pypi_simple(output: &CommandOutput, version: &Version) -> Attempt<ProviderObservationV1> {
    classify_pypi_status(
        output,
        |body| classify_pypi_simple_body(body, version),
        ProviderObservationV1::Absent,
        |cause| ProviderObservationV1::Indeterminate { cause },
    )
}

fn classify_pypi_files(output: &CommandOutput, version: &Version) -> Attempt<PypiRemote> {
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
fn warn_about_extra_files(operation: &RegistryPublishOperation, extras: &[&str]) {
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
            operation.registry.endpoint.as_deref(),
            &paths,
        );
        let output = run_pypi_upload_argv(context, &upload)?;
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
        published: bool,
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
        if published {
            policy::observe_until_visible(context.sleeper(), observe)
        } else {
            observe()
        }
    }
}

/// Builds the sdist and wheel of a Python publish into `outdir`, for a build job that seals them.
pub(crate) fn build_python_into(
    context: &ProviderContext<'_>,
    operation: &RegistryPublishOperation,
    outdir: &std::path::Path,
) -> Result<(), GraphError> {
    PypiRegistry::build_into(context, operation, outdir)
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
mod pypi_file_set_tests;
#[cfg(test)]
mod tests;
