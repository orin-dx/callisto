//! The registry-publish role: one adapter per ecosystem, selected once.

use std::sync::Arc;
use std::time::Duration;

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
use super::repository;
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
        let adapter = adapter_for(request.id.package.ecosystem())?;
        let operation = registry_operation(request)?;
        let preflight = adapter.preflight(context, request.id, operation, request.artifacts)?;
        if matches!(preflight, ReleasePreflight::AlreadySatisfied { .. }) {
            warn_if_foreign_repository(adapter, context, request.id, operation);
        }
        Ok(preflight)
    }

    fn defers_confirmation(&self, request: &ProviderRequest<'_>) -> bool {
        adapter_for(request.id.package.ecosystem()).is_ok_and(|adapter| adapter.defers_confirmation())
    }

    fn issue(
        &self,
        context: &ProviderContext<'_>,
        request: &ProviderRequest<'_>,
        _effect: &EffectAuthorization<'_>,
    ) -> Result<(), GraphError> {
        let operation = registry_operation(request)?;
        let adapter = adapter_for(request.id.package.ecosystem())?;
        if !adapter.defers_confirmation() {
            return Err(GraphError::ReleaseInvariant {
                detail: format!("`{}` is confirmed as it is published", request.id),
            });
        }
        run_publish_client(adapter, context, request, operation)
    }

    fn confirmation_window(&self, request: &ProviderRequest<'_>) -> Duration {
        match (registry_operation(request), adapter_for(request.id.package.ecosystem())) {
            (Ok(operation), Ok(adapter)) => visibility_window(adapter, operation),
            _ => Duration::ZERO,
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
        run_publish_client(adapter, context, request, operation)?;
        // Neither a zero exit nor the client's "already exists" text is a
        // receipt: a yanked version is not the version this intent authorized,
        // so only an exact observation may satisfy the operation.
        let confirmed = adapter.confirm(context, request.id, operation, request.artifacts)?;
        let waited = visibility_window(adapter, operation);
        require_registry_confirmation(
            confirmed.is_terminal_success(),
            &operation.package_name,
            &operation.version,
            waited,
        )?;
        confirmed
            .exact_evidence()
            .ok_or_else(|| GraphError::RegistryPublishUnconfirmed {
                package: operation.package_name.clone(),
                version: operation.version.clone(),
                waited,
            })
    }
}

/// Adopting a version that is already published is by design (a rerun finishes a half-done release), but a version
/// whose registry entry names another repository is more likely someone else's package. Warns, never blocks.
fn warn_if_foreign_repository(
    adapter: &dyn RegistryEcosystem,
    context: &ProviderContext<'_>,
    id: &ReleaseOperationId,
    operation: &RegistryPublishOperation,
) {
    let Some(origin) = context.origin_endpoint() else {
        return;
    };
    let registry_repository = adapter.repository(context, operation);
    if let Some(warning) = repository::foreign_repository_warning(
        &id.package.to_string(),
        operation.version.render(),
        registry_repository.as_deref(),
        origin,
    ) {
        eprintln!("{warning}");
    }
}

/// Runs the ecosystem's publish client. A failed publish stops here; a published and an already-existing version are
/// both left for the confirmation.
fn run_publish_client(
    adapter: &dyn RegistryEcosystem,
    context: &ProviderContext<'_>,
    request: &ProviderRequest<'_>,
    operation: &RegistryPublishOperation,
) -> Result<(), GraphError> {
    let output = adapter.publish(context, request.id, operation, request.artifacts)?;
    adapter
        .classify(&output)
        .map(|_outcome| ())
        .map_err(|source| GraphError::Registry {
            package: operation.package_name.clone(),
            source,
        })
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
    policy::retry_observation_within(context.sleeper(), context.patience(), || {
        adapter.observe_once(context, operation)
    })
}

/// The longest one publish command may run: the registry's configured `publish-timeout`, else the default.
fn publish_timeout(operation: &RegistryPublishOperation) -> Duration {
    operation.registry.publish_timeout.unwrap_or(timeouts::PUBLISH)
}

/// How long this publish's version may take to show up: the registry's configured `confirm-timeout`, else the default.
fn visibility_window<A: RegistryEcosystem + ?Sized>(adapter: &A, operation: &RegistryPublishOperation) -> Duration {
    operation
        .registry
        .confirm_timeout
        .unwrap_or_else(|| policy::default_visibility_window(adapter.ecosystem()))
}

/// The same observation, but an absence is treated as index-propagation lag
/// rather than an answer, for up to the registry's visibility window: this only
/// runs after a publish client reported the version as published or already there.
fn confirmed_registry_observation<A: RegistryEcosystem + ?Sized>(
    adapter: &A,
    context: &ProviderContext<'_>,
    operation: &RegistryPublishOperation,
) -> Result<ProviderObservationV1, GraphError> {
    policy::retry_within_window(context.sleeper(), visibility_window(adapter, operation), || {
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
    fn ecosystem(&self) -> Ecosystem;

    /// Whether a publish here is complete once the version is visible, so the run can issue the next publish while
    /// this one propagates. Not true where completion is more than visibility: PyPI compares a whole file set.
    fn defers_confirmation(&self) -> bool {
        false
    }

    /// The repository the registry says this published version belongs to. Best effort: a registry that records
    /// none, or a read that fails, is `None`.
    fn repository(&self, _context: &ProviderContext<'_>, _operation: &RegistryPublishOperation) -> Option<String> {
        None
    }

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

    /// The observation that lets a finished publish be recorded. The client reported the version published or already
    /// there, so a registry that does not show it yet is lagging, and is waited on.
    fn confirm(
        &self,
        context: &ProviderContext<'_>,
        _id: &ReleaseOperationId,
        operation: &RegistryPublishOperation,
        _artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<ProviderObservationV1, GraphError> {
        if self.can_observe_versions() {
            confirmed_registry_observation(self, context, operation)
        } else {
            registry_observation(self, context, operation)
        }
    }
}

/// The repository the registry records for a published version of this package, if it records one and can be read.
pub(crate) fn recorded_repository(
    context: &ProviderContext<'_>,
    ecosystem: Ecosystem,
    operation: &RegistryPublishOperation,
) -> Option<String> {
    adapter_for(ecosystem).ok()?.repository(context, operation)
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
fn run_argv(
    context: &ProviderContext<'_>,
    argv: &registry_argv::Argv,
    timeout: Duration,
) -> Result<CommandOutput, GraphError> {
    let args: Vec<&str> = argv.args.iter().map(String::as_str).collect();
    Ok(context
        .runner()
        .run_with_timeout(&argv.program, &args, &argv.cwd, timeout)?)
}

/// Withheld from a build backend, which runs project code and needs none of them.
const RELEASE_CREDENTIALS: &[&str] = &[
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_URL",
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

mod cargo;
mod npm;
mod pypi;

use cargo::*;
use npm::*;
use pypi::*;

#[cfg(test)]
mod pypi_file_set_tests;
#[cfg(test)]
mod tests;
