//! The npm registry adapter: how a npm publish is observed, classified and run.

use super::*;

pub(super) fn run_npm_publish_argv(
    context: &ProviderContext<'_>,
    argv: &registry_argv::Argv,
    timeout: Duration,
) -> Result<CommandOutput, GraphError> {
    let token = std::env::var("NPM_TOKEN").ok().filter(|value| !value.is_empty());
    let config = std::env::var("CALLISTO_PUBLISH_NPM_CONFIG")
        .ok()
        .filter(|value| !value.is_empty());
    let env = npm_publisher_environment(&argv.program, token.as_deref(), config.as_deref());
    let args: Vec<&str> = argv.args.iter().map(String::as_str).collect();
    Ok(context
        .runner()
        .run_with_timeout_publisher(&argv.program, &args, &argv.cwd, timeout, &env)?)
}

pub(super) fn npm_publisher_environment<'a>(
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

/// The code npm gives a registry 404 for a package or a version.
pub(super) const NPM_NOT_FOUND_CODE: &str = "E404";

/// Only npm's own `E404` code proves absence, never text around it; any other failure is a retried `CommandFailed`.
pub(super) fn classify_npm_view(output: &CommandOutput, version: &Version) -> Attempt<ProviderObservationV1> {
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

/// The repository URL in `npm view <spec> repository --json`: a string, or an object with a `url`.
pub(super) fn npm_view_repository(stdout: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    match value {
        serde_json::Value::String(url) => Some(url),
        serde_json::Value::Object(map) => map.get("url")?.as_str().map(str::to_owned),
        _ => None,
    }
    .filter(|url| !url.is_empty())
}

/// The `error.code` of the JSON error body `npm --json` prints to stdout on failure.
pub(super) fn npm_error_code(stdout: &str) -> Option<String> {
    let body: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    body.get("error")?.get("code")?.as_str().map(str::to_owned)
}

/// The code on npm's `npm ERR! code X` or `npm error code X` stderr line; npm 8 prints no JSON body.
pub(super) fn npm_stderr_code(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|line| {
        let rest = line
            .strip_prefix("npm ERR! ")
            .or_else(|| line.strip_prefix("npm error "))?;
        rest.strip_prefix("code ").map(|code| code.trim().to_owned())
    })
}

pub(super) struct NpmRegistry;

impl RegistryEcosystem for NpmRegistry {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Npm
    }

    fn defers_confirmation(&self) -> bool {
        true
    }

    fn repository(&self, context: &ProviderContext<'_>, operation: &RegistryPublishOperation) -> Option<String> {
        let spec = format!("{}@{}", operation.package_name, operation.version.render());
        let mut args = vec!["view", spec.as_str(), "repository", "--json", "--fetch-retries=0"];
        if let Some(registry) = operation.registry.endpoint.as_deref() {
            args.extend(["--registry", registry]);
        }
        let workspace_dir = crate::workspace_owner::npm_workspace_dir(context.root(), &operation.package_dir).ok()?;
        let output = run_observation(
            context.runner(),
            programs::NPM,
            &args,
            &context.root().join(workspace_dir),
            timeouts::REGISTRY_QUERY,
            true,
        )
        .ok()?
        .ok()?;
        output.success().then(|| npm_view_repository(&output.stdout)).flatten()
    }

    fn can_observe_versions(&self) -> bool {
        true
    }

    /// `npm view` never reads the local package; its cwd is only where the project `.npmrc` supplying the registry is
    /// read from. That must be where publishing runs, or the two could address different registries, so it is the
    /// workspace that owns the package.
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
        let workspace_dir = crate::workspace_owner::npm_workspace_dir(context.root(), &operation.package_dir)?;
        let output = match run_observation(
            context.runner(),
            programs::NPM,
            &args,
            &context.root().join(workspace_dir),
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
        let workspace_dir = crate::workspace_owner::npm_workspace_dir(context.root(), &operation.package_dir)?;
        if operation.by_directory {
            let argv = registry_argv::npm_publish_directory_argv(
                context.root(),
                &workspace_dir,
                &operation.package_dir,
                &operation.package_name,
                operation.npm_tag.as_deref(),
                operation.npm_access,
                operation.registry.endpoint.as_deref(),
            );
            return run_npm_publish_argv(context, &argv, publish_timeout(operation));
        }
        // The package manager and the directory it runs in belong to the workspace that holds the package.
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
        run_npm_publish_argv(context, &argv, publish_timeout(operation))
    }

    fn classify(&self, output: &CommandOutput) -> Result<PublishOutcome, RegistryError> {
        registry_argv::classify_npm_publish_output(output)
    }
}
