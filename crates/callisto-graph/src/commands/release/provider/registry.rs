//! The registry-publish role: one adapter per ecosystem, selected once.

use callisto_model::{
    CommandOutput, Ecosystem, ExactEvidence, ProviderEvidenceV1, ProviderIndeterminateCause, ProviderObservationV1,
    PublishOutcome, RegistryError, RegistryKey,
};

use crate::commands::registry_argv;
use crate::error::{CommandFailure, RemoteConflict, UnsupportedReleaseFeature};
use crate::GraphError;

use super::policy::{
    self, poll_until_published, require_registry_confirmation, timeouts, REGISTRY_CONFIRMATION_MAX_RETRIES,
};
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
            // PyPI's upload endpoint is not a query API, so there is nothing
            // to observe before the effect; the post-effect confirmation is
            // where it fails closed.
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
        if matches!(outcome, PublishOutcome::Published) && adapter.can_observe_versions() {
            let confirmed = poll_until_published(
                || adapter.version_is_published(context, operation),
                REGISTRY_CONFIRMATION_MAX_RETRIES,
                policy::registry_confirmation_backoff,
                std::thread::sleep,
            )?;
            require_registry_confirmation(confirmed, &operation.package_name, &operation.version)?;
        }
        // Neither a zero exit nor the client's "already exists" text is a
        // receipt: a yanked version reads as absent to the registry, so only
        // an exact observation of the version itself may satisfy the
        // operation, whatever the publish client reported.
        self.observe(context, request)?
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
    Ok(if adapter.version_is_published(context, operation)? {
        ProviderObservationV1::Exact {
            evidence: ProviderEvidenceV1::RegistryVersion {
                version: operation.version.clone(),
                checksum: None,
                yanked: None,
            },
        }
    } else {
        ProviderObservationV1::Absent
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
    ///
    /// PyPI's identical propagation-lag gap is deliberately deferred -- no
    /// PyPI package or credentials in this workspace to test a fix against.
    fn can_observe_versions(&self) -> bool;

    /// Only called when [`Self::can_observe_versions`].
    fn version_is_published(
        &self,
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
    ) -> Result<bool, GraphError>;

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

struct CargoRegistry;

/// Cargo's own name for crates.io; the workspace's logical registry key for it
/// is `cratesIo`. Every other key is already a cargo registry name.
fn cargo_registry_name(registry_key: &str) -> &str {
    if registry_key == RegistryKey::CRATES_IO {
        "crates-io"
    } else {
        registry_key
    }
}

/// An empty directory with no `Cargo.toml` ancestor. `cargo info` run inside
/// the workspace resolves the local manifest and reports the unpublished
/// version being released as already published.
fn neutral_observation_dir() -> Result<tempfile::TempDir, GraphError> {
    tempfile::Builder::new()
        .prefix("callisto-registry-observation-")
        .tempdir()
        .map_err(|error| GraphError::ReleaseInputRead {
            path: std::env::temp_dir(),
            message: error.to_string(),
        })
}

impl RegistryEcosystem for CargoRegistry {
    fn can_observe_versions(&self) -> bool {
        true
    }

    fn version_is_published(
        &self,
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
    ) -> Result<bool, GraphError> {
        let spec = format!("{}@{}", operation.package_name, operation.version.render());
        let args = vec![
            "info",
            spec.as_str(),
            "--registry",
            cargo_registry_name(operation.registry.key.as_str()),
        ];
        let neutral = neutral_observation_dir()?;
        let output = context
            .runner()
            .run_quiet("cargo", &args, neutral.path(), timeouts::REGISTRY_QUERY)?;
        registry_argv::classify_cargo_info_output(&output).map_err(|source| GraphError::Registry {
            package: operation.package_name.clone(),
            source,
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

struct NpmRegistry;

impl RegistryEcosystem for NpmRegistry {
    fn can_observe_versions(&self) -> bool {
        true
    }

    /// Unlike `cargo info`, `npm view` never resolves the local manifest, so
    /// this keeps the workspace cwd: it is only where the project `.npmrc`
    /// supplying the registry and its credentials is read from.
    fn version_is_published(
        &self,
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
    ) -> Result<bool, GraphError> {
        let spec = format!("{}@{}", operation.package_name, operation.version.render());
        let mut args = vec!["view", spec.as_str(), "--json"];
        if let Some(registry) = operation.registry.endpoint.as_deref() {
            args.extend(["--registry", registry]);
        }
        let output = context
            .runner()
            .run_quiet("npm", &args, context.root(), timeouts::REGISTRY_QUERY)?;
        if output.success() {
            return Ok(!output.stdout_trimmed().is_empty());
        }
        let details = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
        if details.contains("e404")
            || details.contains("etarget")
            || details.contains("no matching version")
            || details.contains("is not in this registry")
        {
            return Ok(false);
        }
        Err(GraphError::ReleaseCommand {
            program: "npm".to_string(),
            args: args.iter().map(ToString::to_string).collect(),
            failure: CommandFailure::NonZeroExit {
                exit_code: output.exit_code,
                stderr: output.stderr,
            },
        })
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
        false
    }

    fn version_is_published(
        &self,
        _context: &ProviderContext<'_>,
        _operation: &RegistryPublishOperation,
    ) -> Result<bool, GraphError> {
        Err(GraphError::ReleaseInvariant {
            detail: "PyPI declares no version query; its observation is Indeterminate".to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression coverage for the confirmed bug: a Cargo publish was never
    /// confirmed against the registry at all, unlike npm's existing check.
    /// `can_observe_versions` is the exact declaration `RegistryProvider::publish`
    /// gates confirmation on, so this fails if Cargo's confirmation is ever
    /// silently dropped again.
    #[test]
    fn only_npm_and_cargo_declare_an_observable_registry_version() {
        assert!(adapter_for(Ecosystem::Cargo).unwrap().can_observe_versions());
        assert!(adapter_for(Ecosystem::Npm).unwrap().can_observe_versions());
        assert!(
            !adapter_for(Ecosystem::Pypi).unwrap().can_observe_versions(),
            "PyPI's identical gap is deliberately deferred, not silently fixed"
        );
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
}
