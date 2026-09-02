use std::process::ExitCode;

use callisto_graph::commands::{plan_publish, PublishOptions};

use crate::cli::{GlobalArgs, OutputFormat, PublishArgs};
use crate::error::CliError;
use crate::output::write_report_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

/// Writes the dry-run text notice for the publish command to the given
/// writer. When the plan contains no publishable packages, emits a clear
/// "nothing to publish (dry run)" message; otherwise previews the plan.
pub(crate) fn write_dry_run_text<W: std::io::Write>(
    plan: &callisto_model::PublishPlan,
    w: &mut W,
) -> std::io::Result<()> {
    let is_empty = plan.rust_crates.is_empty()
        && plan.npm_main_packages.is_empty()
        && plan.npm_platform_packages.is_empty()
        && plan.pypi_packages.is_empty()
        && plan.releases.is_empty();
    if is_empty {
        writeln!(w, "Nothing to publish (dry run).")?;
    } else {
        writeln!(
            w,
            "Dry run: about to publish the following plan (nothing will be published):"
        )?;
        render::render_publish(plan, w)?;
    }
    Ok(())
}

/// Checks whether the environment has credentials for each ecosystem in
/// the plan. Returns human-readable warnings for any missing credentials
/// so the operator can spot auth problems before publishing; empty when
/// all required credentials are present.
///
/// `env` is injectable (pass [`std::env::var`] in production; a closure
/// in tests to avoid mutating the process environment).
///
/// Soft pre-flight check only -- doesn't fail hard. Missing credentials
/// may still let publish succeed via an alternative auth mechanism (a
/// pre-configured `.npmrc` or `~/.cargo/credentials`).
#[cfg(test)]
fn check_credentials(
    plan: &callisto_model::PublishPlan,
    env: impl Fn(&str) -> Result<String, std::env::VarError>,
) -> Vec<String> {
    let mut warnings = Vec::new();

    // Cargo: crates targeting the default crates.io registry need
    // CARGO_REGISTRY_TOKEN; crates targeting a named private registry need
    // CARGO_REGISTRIES_<UPPERCASE_NAME>_TOKEN. Collect the distinct variable
    // names required and warn once per missing variable.
    let mut cargo_vars_checked = std::collections::BTreeSet::new();
    for pkg in &plan.rust_crates {
        let var = match &pkg.registry {
            None => "CARGO_REGISTRY_TOKEN".to_string(),
            Some(name) => format!("CARGO_REGISTRIES_{}_TOKEN", name.to_uppercase().replace('-', "_")),
        };
        if cargo_vars_checked.insert(var.clone()) && env(&var).is_err() {
            warnings.push(format!(
                "warning: {var} is not set; cargo publish may fail authentication"
            ));
        }
    }

    if (!plan.npm_main_packages.is_empty() || !plan.npm_platform_packages.is_empty()) && env("NPM_TOKEN").is_err() {
        warnings.push("warning: NPM_TOKEN is not set; npm publish may fail authentication".to_string());
    }

    if !plan.pypi_packages.is_empty() && env("TWINE_PASSWORD").is_err() {
        warnings.push("warning: TWINE_PASSWORD is not set; twine upload may fail authentication".to_string());
    }

    warnings
}

/// Compatibility preview for the retired mutable publish route.
///
/// This command intentionally never obtains an [`ApplyPermit`] or constructs
/// a registry client. Production publication moves to `release execute` once
/// its exact provider adapters are available.
pub fn handle(args: PublishArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let _skip_publish_precheck = args.skip_publish_precheck;
    let opts = PublishOptions { only: args.only };
    let plan = plan_publish(&ws, &opts)?;

    match global.format {
        OutputFormat::Json => write_report_json(&mut std::io::stdout(), &plan)?,
        OutputFormat::Text => {
            write_dry_run_text(&plan, &mut std::io::stdout())?;
            eprintln!("`callisto publish` is a compatibility preview; use `callisto release plan` and `callisto release execute` for durable releases.");
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::SCHEMA_VERSION;

    fn empty_plan() -> callisto_model::PublishPlan {
        callisto_model::PublishPlan {
            schema_version: SCHEMA_VERSION,
            rust_crates: vec![],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        }
    }

    // Inject a closure that always returns NotPresent — no process-environment
    // mutation needed, so these tests are safe under parallel test execution.
    fn missing_env(_var: &str) -> Result<String, std::env::VarError> {
        Err(std::env::VarError::NotPresent)
    }

    fn present_env(_var: &str) -> Result<String, std::env::VarError> {
        Ok("token".to_string())
    }

    #[test]
    fn credential_check_warns_when_npm_token_missing() {
        use callisto_model::{NpmMainPublish, RegistryKey, Version, VersionGrammar};
        use std::path::PathBuf;

        let v1 = Version::parse("1.0.0", VersionGrammar::SemVer).unwrap();
        let plan = callisto_model::PublishPlan {
            schema_version: SCHEMA_VERSION,
            rust_crates: vec![],
            npm_main_packages: vec![NpmMainPublish {
                name: "@callisto/cli".to_string(),
                version: v1,
                publish_to: RegistryKey("npm".to_string()),
                registry: None,
                tag: None,
                access: None,
                depends_on_platforms: vec![],
                package_dir: PathBuf::from("packages/callisto-cli"),
            }],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };

        let warnings = check_credentials(&plan, missing_env);
        assert!(
            !warnings.is_empty(),
            "expected a warning for missing NPM_TOKEN but got none"
        );
        assert!(
            warnings.iter().any(|w| w.contains("NPM_TOKEN")),
            "warning must mention NPM_TOKEN, got: {:?}",
            warnings
        );
    }

    #[test]
    fn credential_check_no_warnings_when_all_tokens_present() {
        use callisto_model::{NpmMainPublish, RegistryKey, Version, VersionGrammar};
        use std::path::PathBuf;

        let v1 = Version::parse("1.0.0", VersionGrammar::SemVer).unwrap();
        let plan = callisto_model::PublishPlan {
            schema_version: SCHEMA_VERSION,
            rust_crates: vec![],
            npm_main_packages: vec![NpmMainPublish {
                name: "@callisto/cli".to_string(),
                version: v1,
                publish_to: RegistryKey("npm".to_string()),
                registry: None,
                tag: None,
                access: None,
                depends_on_platforms: vec![],
                package_dir: PathBuf::from("packages/callisto-cli"),
            }],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };

        let warnings = check_credentials(&plan, present_env);
        assert!(
            warnings.is_empty(),
            "expected no warnings when all tokens present, got: {:?}",
            warnings
        );
    }

    #[test]
    fn credential_check_default_crates_io_registry_warns_on_missing_token() {
        use callisto_model::{CratePublish, RegistryKey, Version, VersionGrammar};

        let v1 = Version::parse("1.0.0", VersionGrammar::SemVer).unwrap();
        let plan = callisto_model::PublishPlan {
            schema_version: SCHEMA_VERSION,
            rust_crates: vec![CratePublish {
                name: "my-crate".to_string(),
                version: v1,
                publish_to: RegistryKey("crates-io".to_string()),
                registry: None,
                package_dir: None,
            }],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };

        let warnings = check_credentials(&plan, missing_env);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("CARGO_REGISTRY_TOKEN") && !w.contains("REGISTRIES")),
            "a crate with no named registry (crates.io) must warn about bare CARGO_REGISTRY_TOKEN, got: {:?}",
            warnings
        );
    }

    #[test]
    fn credential_check_warns_when_pypi_password_missing() {
        use callisto_model::{PypiPublish, RegistryKey, Version, VersionGrammar};
        use std::path::PathBuf;

        let v1 = Version::parse("1.0.0", VersionGrammar::SemVer).unwrap();
        let plan = callisto_model::PublishPlan {
            schema_version: SCHEMA_VERSION,
            rust_crates: vec![],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![PypiPublish {
                name: "my-pkg".to_string(),
                version: v1,
                publish_to: RegistryKey("pypi".to_string()),
                package_dir: PathBuf::from("packages/my-pkg"),
                index: None,
            }],
            releases: vec![],
            diagnostics: vec![],
        };

        let warnings = check_credentials(&plan, missing_env);
        assert!(
            warnings.iter().any(|w| w.contains("TWINE_PASSWORD")),
            "expected a warning for missing TWINE_PASSWORD, got: {:?}",
            warnings
        );
    }

    #[test]
    fn handle_dry_run_text_format_succeeds_on_empty_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();

        let global = GlobalArgs {
            format: OutputFormat::Text,
            cwd: root.to_path_buf(),
            dry_run: true,
        };

        let result = handle(PublishArgs::default(), &global);
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }

    #[test]
    fn credential_check_private_cargo_registry_checks_correct_var() {
        use callisto_model::{CratePublish, RegistryKey, Version, VersionGrammar};

        let v1 = Version::parse("1.0.0", VersionGrammar::SemVer).unwrap();
        let plan = callisto_model::PublishPlan {
            schema_version: SCHEMA_VERSION,
            rust_crates: vec![CratePublish {
                name: "my-crate".to_string(),
                version: v1,
                publish_to: RegistryKey("cloudsmith".to_string()),
                registry: Some("cloudsmith".to_string()),
                package_dir: None,
            }],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };

        // Only CARGO_REGISTRIES_CLOUDSMITH_TOKEN should trigger a warning,
        // not CARGO_REGISTRY_TOKEN (which is for crates.io only).
        let warnings = check_credentials(&plan, missing_env);
        assert!(
            warnings.iter().any(|w| w.contains("CARGO_REGISTRIES_CLOUDSMITH_TOKEN")),
            "expected warning about CARGO_REGISTRIES_CLOUDSMITH_TOKEN, got: {:?}",
            warnings
        );
        assert!(
            !warnings
                .iter()
                .any(|w| w.contains("CARGO_REGISTRY_TOKEN") && !w.contains("REGISTRIES")),
            "should NOT warn about bare CARGO_REGISTRY_TOKEN for private-registry crates, got: {:?}",
            warnings
        );
    }

    /// Spec: when no packages would be published and --dry-run is active,
    /// the text output must contain "dry run" (case-insensitive) so the
    /// operator can see that nothing was published.
    #[test]
    fn dry_run_text_contains_dry_run_for_empty_plan() {
        let plan = empty_plan();
        let mut out = Vec::<u8>::new();
        write_dry_run_text(&plan, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.to_ascii_lowercase().contains("dry run"),
            "dry-run text output must contain 'dry run', got: {text:?}"
        );
    }

    /// Spec: the empty-plan dry-run message must use present tense ("nothing to
    /// publish") not past tense ("no packages published"), because nothing has
    /// actually happened yet during a dry run. Past-tense wording misleads
    /// operators into thinking the publish already occurred.
    #[test]
    fn dry_run_empty_plan_message_is_present_tense() {
        let plan = empty_plan();
        let mut out = Vec::<u8>::new();
        write_dry_run_text(&plan, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            !text.to_ascii_lowercase().contains("published"),
            "dry-run empty-plan message must NOT use past tense 'published'; got: {text:?}"
        );
        assert!(
            text.to_ascii_lowercase().contains("nothing"),
            "dry-run empty-plan message must say 'nothing'; got: {text:?}"
        );
    }

    /// PUB-010: JSON output for `plan-publish` must include a `"command"`
    /// discriminator field so consumers can distinguish it from `PublishReport`
    /// without inspecting the payload structure.
    #[test]
    fn plan_publish_json_includes_command_discriminator() {
        use crate::output::write_report_json;
        use callisto_model::Report;

        let plan = empty_plan();
        let mut out = Vec::<u8>::new();
        write_report_json(&mut out, &plan).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("\"command\""),
            "plan-publish JSON must contain 'command' field; got: {text}"
        );
        let expected_command = callisto_model::PublishPlan::COMMAND;
        assert!(
            text.contains(&format!("\"{}\"", expected_command)),
            "plan-publish JSON 'command' must be {:?}; got: {text}",
            expected_command
        );
    }

    /// A plan that has pending release entries (e.g. a GitHub release tag) but
    /// no registry packages must NOT print "Nothing to publish" in dry-run
    /// output — the release tag is real pending work that the operator needs
    /// to be aware of before approving the publish.
    #[test]
    fn dry_run_release_only_plan_is_not_empty() {
        use callisto_model::{CommitSha, PackageId, ReleaseEntry, TagName, SCHEMA_VERSION};

        let sha = CommitSha::parse("a".repeat(40).as_str()).unwrap();
        let plan = callisto_model::PublishPlan {
            schema_version: SCHEMA_VERSION,
            rust_crates: vec![],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![ReleaseEntry {
                package: PackageId::Bare("my-lib".to_string()),
                tag_name: TagName("my-lib@1.0.0".to_string()),
                sha,
                changelog_section: None,
                is_prerelease: false,
            }],
            diagnostics: vec![],
        };

        let mut out = Vec::<u8>::new();
        write_dry_run_text(&plan, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            !text.to_ascii_lowercase().contains("nothing to publish"),
            "dry-run output for a release-only plan must NOT say 'nothing to publish'; \
             the pending release tag must be visible; got: {text:?}"
        );
        assert!(
            text.contains("my-lib@1.0.0"),
            "dry-run output must mention the pending release tag; got: {text:?}"
        );
    }
}
