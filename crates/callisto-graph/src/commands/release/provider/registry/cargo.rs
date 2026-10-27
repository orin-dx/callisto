//! The cargo registry adapter: how a cargo publish is observed, classified and run.

use super::*;

pub(super) struct CargoRegistry;

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
pub(super) fn cargo_info_args<'a>(spec: &'a str, registry: &'a str) -> [&'a str; 6] {
    ["--config", "net.retry=0", "info", spec, "--registry", registry]
}

impl RegistryEcosystem for CargoRegistry {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Cargo
    }

    fn repository(&self, context: &ProviderContext<'_>, operation: &RegistryPublishOperation) -> Option<String> {
        let spec = format!("{}@{}", operation.package_name, operation.version.render());
        let registry = cargo_registry_name(&operation.registry.key);
        let args = cargo_info_args(&spec, registry);
        let cwd = registry_argv::cargo_workspace_dir(context.root(), &operation.package_dir).ok()?;
        let output = run_observation(
            context.runner(),
            programs::CARGO,
            &args,
            &cwd,
            timeouts::REGISTRY_QUERY,
            true,
        )
        .ok()?
        .ok()?;
        output
            .success()
            .then(|| cargo_info_repository(&output.stdout))
            .flatten()
    }

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
        run_argv(context, &argv, publish_timeout(operation))
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
pub(super) fn classify_cargo_info(
    output: &CommandOutput,
    spec: &str,
    version: &Version,
) -> Attempt<ProviderObservationV1> {
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

/// The value of the `repository:` line `cargo info` prints, if the crate has one.
pub(super) fn cargo_info_repository(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("repository:"))
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Whether cargo's `version: X ...` line names exactly the requested version.
pub(super) fn reports_version(stdout: &str, version: &Version) -> bool {
    stdout.lines().any(|line| {
        line.trim()
            .strip_prefix("version: ")
            .and_then(|rest| rest.strip_prefix(version.render()))
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    })
}
