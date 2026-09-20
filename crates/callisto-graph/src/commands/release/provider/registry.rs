//! The registry-publish role: one adapter per ecosystem, selected once.

use callisto_model::{
    normalize_pypi_project_name, ArtifactDigest, CommandOutput, Ecosystem, ExactEvidence, ProviderConflictReason,
    ProviderEvidenceV1, ProviderIndeterminateCause, ProviderObservationV1, PublishOutcome, RegistryError, RegistryKey,
};

use crate::commands::registry_argv;
use crate::error::{CommandFailure, RemoteConflict, UnsupportedReleaseFeature};
use crate::registry_endpoint::builtin_registry_url;
use crate::GraphError;

use super::super::binding::RegistryProtocol;
use super::http::{http_get, HttpOutcome, HttpResponse, TransportFailure};
use super::policy::{self, programs, require_registry_confirmation, timeouts, Attempt};
use super::{
    wrong_role, EffectAuthorization, PreparedOperation, ProviderCapabilities, ProviderContext, ProviderRequest,
    RegistryPublishOperation, ReleasePreflight, ReleaseProvider,
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
        let operation = registry_operation(request)?;
        // A registry version that already exists is a hard conflict, not an
        // adopted success: only the recovery path may converge on an exact
        // remote observation.
        match self.observe(context, request)? {
            ProviderObservationV1::Absent => Ok(proceed()),
            ProviderObservationV1::Exact { .. } => Err(GraphError::ReleaseRegistryVersionExists {
                package: operation.package_name.clone(),
                version: operation.version.clone(),
            }),
            // A registry with no query API at all has nothing to observe
            // before the effect; the post-effect confirmation fails closed.
            ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::UnsupportedProvider,
            } => Ok(proceed()),
            ProviderObservationV1::Indeterminate { .. } => Err(GraphError::ReleaseProviderIndeterminate {
                operation: Box::new(request.id.clone()),
            }),
            ProviderObservationV1::Conflict { .. } => Err(GraphError::ReleaseRemoteConflict {
                conflict: self.preflight_conflict(),
            }),
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

/// The one registry observation used by preflight, recovery, and post-publish
/// confirmation, so those three can never disagree.
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

/// The profile-bound base URL for an observation: the configured endpoint, or
/// the built-in URL the registry key stands for.
fn observation_endpoint(operation: &RegistryPublishOperation) -> Result<String, GraphError> {
    let base = operation
        .registry
        .endpoint
        .clone()
        .or_else(|| builtin_registry_url(operation.registry.key.as_str()).map(ToOwned::to_owned))
        .ok_or_else(|| GraphError::ReleaseInvariant {
            detail: format!(
                "registry `{}` has no observable endpoint",
                operation.registry.key.as_str()
            ),
        })?;
    Ok(base.trim_end_matches('/').to_owned())
}

/// Maps one HTTP outcome to the attempt shape, leaving only the statuses that
/// actually prove something for the caller to classify.
///
/// A 429, any 5xx, and a rate-limited 403 are transient; every other status is
/// a settled answer. No status is ever reported as a conflicting remote object.
fn http_attempt(outcome: HttpOutcome) -> Result<HttpResponse, Attempt<ProviderObservationV1>> {
    let response = match outcome {
        HttpOutcome::Transport(TransportFailure::Timeout) => {
            return Err(transient(ProviderIndeterminateCause::Timeout, None))
        }
        HttpOutcome::Transport(TransportFailure::Unreachable) => {
            return Err(transient(ProviderIndeterminateCause::CommandFailed, None))
        }
        HttpOutcome::Response(response) => response,
    };
    let transient_status =
        matches!(response.status, 429 | 500..=599) || (response.status == 403 && response.is_rate_limited());
    if transient_status {
        return Err(transient(
            ProviderIndeterminateCause::ProviderStatus {
                status: response.status,
            },
            response.retry_after(),
        ));
    }
    Ok(response)
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

fn indeterminate(cause: ProviderIndeterminateCause) -> Attempt<ProviderObservationV1> {
    settled(ProviderObservationV1::Indeterminate { cause })
}

fn yanked_conflict() -> Attempt<ProviderObservationV1> {
    settled(ProviderObservationV1::Conflict {
        reason: ProviderConflictReason::RegistryVersionYanked,
    })
}

struct CargoRegistry;

/// The sparse index path layout: `1/a`, `2/ab`, `3/a/abc`, else `ab/cd/name`,
/// always lowercased.
pub(crate) fn sparse_index_path(package_name: &str) -> String {
    let name = package_name.to_ascii_lowercase();
    let mut characters = name.chars();
    match name.chars().count() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", characters.next().expect("three characters")),
        _ => {
            let prefix: String = characters.by_ref().take(2).collect();
            let infix: String = characters.take(2).collect();
            format!("{prefix}/{infix}/{name}")
        }
    }
}

impl RegistryEcosystem for CargoRegistry {
    fn can_observe_versions(&self) -> bool {
        true
    }

    fn observe_once(
        &self,
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
    ) -> Result<Attempt<ProviderObservationV1>, GraphError> {
        if operation.registry.protocol == RegistryProtocol::CargoGitIndex {
            return Ok(indeterminate(ProviderIndeterminateCause::UnsupportedProtocol));
        }
        let url = format!(
            "{}/{}",
            observation_endpoint(operation)?,
            sparse_index_path(&operation.package_name)
        );
        let response = match http_attempt(http_get(context.runner(), context.root(), &url)?) {
            Ok(response) => response,
            Err(attempt) => return Ok(attempt),
        };
        Ok(match response.status {
            404 => settled(ProviderObservationV1::Absent),
            200 => classify_sparse_index(&response.body, operation),
            status => indeterminate(ProviderIndeterminateCause::ProviderStatus { status }),
        })
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

/// One line per version, newest last. A version the index does not carry is
/// absent; a yanked one exists but is not what this intent authorized.
fn classify_sparse_index(body: &str, operation: &RegistryPublishOperation) -> Attempt<ProviderObservationV1> {
    let mut found = None;
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            return indeterminate(ProviderIndeterminateCause::MalformedResponse);
        };
        if entry.get("vers").and_then(serde_json::Value::as_str) == Some(operation.version.render()) {
            found = Some(entry);
        }
    }
    let Some(entry) = found else {
        return settled(ProviderObservationV1::Absent);
    };
    if entry.get("yanked").and_then(serde_json::Value::as_bool) == Some(true) {
        return yanked_conflict();
    }
    let Some(checksum) = entry
        .get("cksum")
        .and_then(serde_json::Value::as_str)
        .map(ArtifactDigest::parse)
    else {
        return indeterminate(ProviderIndeterminateCause::MalformedResponse);
    };
    let Ok(checksum) = checksum else {
        return indeterminate(ProviderIndeterminateCause::MalformedResponse);
    };
    settled(ProviderObservationV1::Exact {
        evidence: ProviderEvidenceV1::RegistryVersion {
            version: operation.version.clone(),
            checksum: Some(checksum),
            yanked: Some(false),
        },
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
        let mut args = vec!["view", spec.as_str(), "--json"];
        if let Some(registry) = operation.registry.endpoint.as_deref() {
            args.extend(["--registry", registry]);
        }
        let output = context
            .runner()
            .run_quiet(programs::NPM, &args, context.root(), timeouts::REGISTRY_QUERY)?;
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

impl RegistryEcosystem for PypiRegistry {
    fn can_observe_versions(&self) -> bool {
        true
    }

    fn observe_once(
        &self,
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
    ) -> Result<Attempt<ProviderObservationV1>, GraphError> {
        let url = format!(
            "{}/pypi/{}/{}/json",
            observation_endpoint(operation)?,
            normalize_pypi_project_name(&operation.package_name),
            operation.version.render()
        );
        let response = match http_attempt(http_get(context.runner(), context.root(), &url)?) {
            Ok(response) => response,
            Err(attempt) => return Ok(attempt),
        };
        Ok(match response.status {
            404 => settled(ProviderObservationV1::Absent),
            200 => classify_pypi_version(&response.body, operation),
            status => indeterminate(ProviderIndeterminateCause::ProviderStatus { status }),
        })
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
        );
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

/// A version's file list. A release with every file yanked is a conflict; one
/// with no files at all exists but proves no identity.
fn classify_pypi_version(body: &str, operation: &RegistryPublishOperation) -> Attempt<ProviderObservationV1> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(body) else {
        return indeterminate(ProviderIndeterminateCause::MalformedResponse);
    };
    let Some(files) = document.get("urls").and_then(serde_json::Value::as_array) else {
        return indeterminate(ProviderIndeterminateCause::MalformedResponse);
    };
    let Some(first) = files.first() else {
        return indeterminate(ProviderIndeterminateCause::RegistryVersionUnverified);
    };
    if files
        .iter()
        .any(|file| file.get("yanked").and_then(serde_json::Value::as_bool) == Some(true))
    {
        return yanked_conflict();
    }
    let Some(Ok(checksum)) = first
        .get("digests")
        .and_then(|digests| digests.get("sha256"))
        .and_then(serde_json::Value::as_str)
        .map(ArtifactDigest::parse)
    else {
        return indeterminate(ProviderIndeterminateCause::MalformedResponse);
    };
    settled(ProviderObservationV1::Exact {
        evidence: ProviderEvidenceV1::RegistryVersion {
            version: operation.version.clone(),
            checksum: Some(checksum),
            yanked: Some(false),
        },
    })
}

#[cfg(test)]
mod tests;
