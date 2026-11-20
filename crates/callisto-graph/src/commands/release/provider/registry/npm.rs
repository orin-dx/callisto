//! The npm registry adapter: how a npm publish is observed, classified and run.

use super::*;

pub(super) fn run_npm_publish_argv(
    context: &ProviderContext<'_>,
    argv: &registry_argv::Argv,
    timeout: Duration,
) -> Result<CommandOutput, GraphError> {
    let token = env_value("NPM_TOKEN");
    let config = publish_npm_config();
    let env = npm_publisher_environment(token.as_deref(), config.as_deref());
    let args: Vec<&str> = argv.args.iter().map(String::as_str).collect();
    Ok(context
        .runner()
        .run_with_timeout_publisher(&argv.program, &args, &argv.cwd, timeout, &env)?)
}

/// The temporary npm config the release action writes when it is given `NPM_TOKEN`, if any.
fn publish_npm_config() -> Option<String> {
    env_value("CALLISTO_PUBLISH_NPM_CONFIG")
}

/// `npm view` arguments for one package version. `userconfig` is the publish's temporary npm config: a restricted
/// package answers an anonymous `view` with 404.
pub(super) fn npm_view_args(
    spec: &str,
    repository_only: bool,
    registry: Option<&str>,
    userconfig: Option<&str>,
) -> Vec<String> {
    let mut args = vec!["view".to_owned(), spec.to_owned()];
    if repository_only {
        args.push("repository".to_owned());
    }
    args.extend(["--json".to_owned(), "--fetch-retries=0".to_owned()]);
    if let Some(registry) = registry {
        args.extend(["--registry".to_owned(), registry.to_owned()]);
    }
    if let Some(userconfig) = userconfig {
        args.extend(["--userconfig".to_owned(), userconfig.to_owned()]);
    }
    args
}

pub(super) fn npm_publisher_environment<'a>(
    token: Option<&'a str>,
    config: Option<&'a str>,
) -> Vec<(&'static str, &'a str)> {
    let mut env = Vec::new();
    if let Some(token) = token {
        env.push(("NPM_TOKEN", token));
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
        let args = npm_view_args(
            &spec,
            true,
            operation.registry.endpoint.as_deref(),
            publish_npm_config().as_deref(),
        );
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
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
        let args = npm_view_args(
            &spec,
            false,
            operation.registry.endpoint.as_deref(),
            publish_npm_config().as_deref(),
        );
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
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

    /// Publishes one tarball with `npm`, scripts off. A manifest that seals the package supplies the tarball a
    /// credential-free job packed; one that does not is refused, so a package's `prepack` never runs beside the
    /// registry token. Without a manifest (the local route) the tarball is packed here with credentials withheld.
    fn publish(
        &self,
        context: &ProviderContext<'_>,
        _id: &ReleaseOperationId,
        operation: &RegistryPublishOperation,
        artifacts: Option<&VerifiedArtifactManifest<'_>>,
    ) -> Result<CommandOutput, GraphError> {
        let workspace_dir = crate::workspace_owner::npm_workspace_dir(context.root(), &operation.package_dir)?;
        let published = callisto_model::ReleasePackageId::new(Ecosystem::Npm, &operation.package_name)?;
        let packed_here;
        let tarball = match artifacts {
            Some(artifacts) => {
                let sealed = artifacts
                    .sealed_files(&published, &operation.version)
                    .and_then(|mut files| (files.len() == 1).then(|| files.remove(0)));
                let Some(sealed) = sealed else {
                    return Err(GraphError::BuildOutputIncomplete {
                        tool: "artifact manifest",
                        package: operation.package_name.clone(),
                        version: operation.version.clone(),
                        missing: "sealed npm tarball",
                    });
                };
                recheck_sealed_file(&sealed)?;
                sealed.path
            }
            None if context.route() == PublishRoute::Ci => {
                return Err(GraphError::BuildOutputIncomplete {
                    tool: "artifact manifest",
                    package: operation.package_name.clone(),
                    version: operation.version.clone(),
                    missing: "sealed npm tarball",
                });
            }
            None => {
                packed_here = tempfile::tempdir().map_err(|error| {
                    GraphError::Command(callisto_model::CommandError::Io {
                        program: "npm pack".to_string(),
                        message: error.to_string(),
                    })
                })?;
                Self::pack_into(context, operation, packed_here.path())?
            }
        };
        let argv = registry_argv::npm_publish_tarball_argv(
            &context.root().join(&workspace_dir),
            &tarball,
            &operation.package_name,
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

impl NpmRegistry {
    /// Packs the package into `outdir` with its workspace's tool and returns the tarball's path. The package's
    /// lifecycle scripts run, so credentials are withheld. A platform package is not a workspace member, so `npm`
    /// packs it by directory.
    pub(crate) fn pack_into(
        context: &ProviderContext<'_>,
        operation: &RegistryPublishOperation,
        outdir: &std::path::Path,
    ) -> Result<PathBuf, GraphError> {
        let io = |error: std::io::Error| {
            GraphError::Command(callisto_model::CommandError::Io {
                program: "npm pack".to_string(),
                message: error.to_string(),
            })
        };
        let manager = if operation.by_directory {
            registry_argv::NpmPackageManager::Npm
        } else {
            let workspace_dir = crate::workspace_owner::npm_workspace_dir(context.root(), &operation.package_dir)?;
            registry_argv::detect_npm_package_manager(&context.root().join(workspace_dir))?
        };
        // A fresh empty directory, so the tarball found afterwards is the one this pack wrote.
        let scratch = tempfile::tempdir_in(outdir).map_err(io)?;
        let argv = registry_argv::npm_pack_argv(&context.root().join(&operation.package_dir), manager, scratch.path());
        let output = run_build_argv(context, &argv)?;
        if output.exit_code != Some(0) {
            return Err(GraphError::ReleaseCommand {
                program: argv.program.clone(),
                args: argv.args.clone(),
                failure: CommandFailure::NonZeroExit {
                    exit_code: output.exit_code,
                    stderr: output.stderr,
                },
            });
        }
        let expected = callisto_model::npm_tarball_filename(&operation.package_name, &operation.version);
        let mut tarballs = Vec::new();
        for entry in std::fs::read_dir(scratch.path()).map_err(io)? {
            tarballs.push(entry.map_err(io)?.path());
        }
        let incomplete = |missing| GraphError::BuildOutputIncomplete {
            tool: "npm pack",
            package: operation.package_name.clone(),
            version: operation.version.clone(),
            missing,
        };
        let [packed] = tarballs.as_slice() else {
            return Err(incomplete("single tarball"));
        };
        // The manager's own file name is not trusted (yarn names a scoped package its own way): the manifest inside is.
        let packed_name = packed.to_string_lossy();
        let manifest = context.runner().run(
            "tar",
            &["-xOzf", packed_name.as_ref(), "package/package.json"],
            scratch.path(),
        )?;
        let manifest: serde_json::Value = serde_json::from_str(&manifest.stdout).unwrap_or_default();
        let named = manifest["name"].as_str() == Some(operation.package_name.as_str())
            && manifest["version"].as_str() == Some(operation.version.render());
        if !named {
            return Err(incomplete("tarball of this package and version"));
        }
        let target = outdir.join(&expected);
        std::fs::rename(packed, &target).map_err(io)?;
        Ok(target)
    }
}

/// Re-hashes a sealed file just before it is uploaded: a file changed after verification must not reach the registry.
fn recheck_sealed_file(file: &SealedFile) -> Result<(), GraphError> {
    let unchanged = std::fs::symlink_metadata(&file.path).is_ok_and(|metadata| metadata.is_file())
        && std::fs::File::open(&file.path)
            .and_then(callisto_model::ArtifactDigest::from_reader)
            .is_ok_and(|(digest, _)| digest.as_str() == file.sha256);
    if unchanged {
        Ok(())
    } else {
        Err(GraphError::ArtifactBytesMismatch {
            path: file.path.clone(),
        })
    }
}
