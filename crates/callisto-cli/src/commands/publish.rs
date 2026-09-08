use std::process::ExitCode;

use crate::cli::{GlobalArgs, OutputFormat, PublishArgs};
use crate::commands::plan_publish::build_plan;
use crate::error::CliError;
use crate::output::write_report_json;
use crate::render;

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

/// Compatibility preview for the retired mutable publish route.
///
/// This command intentionally never obtains an [`callisto_model::ApplyPermit`] or constructs
/// a registry client. Production publication moves to `release execute` once
/// its exact provider adapters are available.
pub fn handle(args: PublishArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let plan = build_plan(global, args.only)?;

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
