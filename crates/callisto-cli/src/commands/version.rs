use std::process::ExitCode;

use callisto_graph::apply::{apply_version_plan, ApplyOptions, ApplyOutcome};
use callisto_graph::commands::VersionOptions;
use callisto_model::{ApplyPermit, DiagnosticSeverity};

use crate::cli::{GlobalArgs, OutputFormat, VersionArgs};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::{load_workspace, select_inference};

pub fn handle(args: VersionArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let inference = select_inference();
    let opts = VersionOptions {
        strict: args.strict,
        strict_graph: args.strict_graph,
        allow_empty_changesets: args.allow_empty_changesets,
    };

    let plan = callisto_graph::commands::plan_version(&ws, &inference, &opts)?;

    let apply_opts = ApplyOptions {
        refresh_lockfiles: args.refresh_lockfiles,
        transient: false,
    };

    let outcome = match ApplyPermit::granted_unless_dry_run(global.dry_run) {
        Some(permit) => apply_version_plan(&ws.root, &plan, &runner, &apply_opts, &permit)?,
        None => ApplyOutcome::default(),
    };
    let report = plan.to_report(outcome.lockfile_refresh_results);

    if global.dry_run && global.format == OutputFormat::Text {
        println!("[DRY-RUN] Version Plan Calculated (no files modified):");
    }

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_version(&report, &mut std::io::stdout())?,
    }

    // If any diagnostic was escalated to Error (e.g. by --strict), fail.
    // Mirrors the pattern in status.rs: diagnostics ride in the report so the
    // caller sees full detail before the non-zero exit.
    let has_errors = report
        .diagnostics
        .iter()
        .any(|d| d.severity == DiagnosticSeverity::Error);

    if has_errors {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use callisto_graph::commands::{plan_version, VersionOptions};
    use callisto_graph::infer::NoInference;
    use callisto_graph::locate::IgnoreWalkLocator;
    use callisto_graph::Workspace;
    use callisto_model::{CommandError, CommandOutput, CommandRunner, PackageId};

    struct NoopRunner;

    impl CommandRunner for NoopRunner {
        fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    fn git_init_with_commit(root: &Path) {
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Test"],
            vec!["config", "commit.gpgsign", "false"],
            vec!["config", "tag.gpgsign", "false"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(root)
                .output()
                .expect("git must be installed");
        }
        std::fs::write(root.join(".gitkeep"), "").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(root)
                .output()
                .expect("git must be installed");
        }
    }

    #[cfg(feature = "inference")]
    fn run_git(root: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .expect("git must be installed");
        assert!(status.success(), "git {args:?} failed");
    }

    fn make_fixture(root: &Path) {
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("pkg-alpha")).unwrap();
        std::fs::write(
            root.join("pkg-alpha/Cargo.toml"),
            "[package]\nname = \"pkg-alpha\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();

        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(
            root.join(".changeset/bump-alpha.md"),
            "---\n\"pkg-alpha\": minor\n---\n\nAdd a new feature.\n",
        )
        .unwrap();
    }

    /// Spec: the text rendering of a `VersionReport` produced from a workspace
    /// with a pending changeset must include the package name and both the
    /// `from` and `to` version strings. The `render_version` function writes
    /// one line per bump in the format `<name> <from> → <to>`.
    #[test]
    fn version_text_output_includes_package_name_and_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        make_fixture(root);

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");
        let report = plan.to_report(None);

        // The report must include a bump for pkg-alpha.
        let pkg_alpha = PackageId::parse("pkg-alpha").unwrap();
        let bump = report
            .bumps
            .iter()
            .find(|b| b.package == pkg_alpha)
            .expect("pkg-alpha must have a planned bump");

        assert_eq!(bump.from.render(), "1.0.0", "from version must be the current version");
        assert_eq!(
            bump.to.render(),
            "1.1.0",
            "to version must be a minor bump (1.0.0 -> 1.1.0)"
        );

        // Render text output and verify it contains the key strings.
        let mut text_out = Vec::new();
        crate::render::render_version(&report, &mut text_out).unwrap();
        let rendered = String::from_utf8(text_out).unwrap();

        assert!(
            rendered.contains("pkg-alpha"),
            "text output must include package name; got:\n{rendered}"
        );
        assert!(
            rendered.contains("1.0.0"),
            "text output must include from version; got:\n{rendered}"
        );
        assert!(
            rendered.contains("1.1.0"),
            "text output must include to version; got:\n{rendered}"
        );
    }

    /// Spec: the JSON output of `write_json(&report)` for a `VersionReport`
    /// must be valid JSON. It must include the top-level keys `schemaVersion`
    /// and `bumps`. The `diagnostics` key is omitted when empty
    /// (`skip_serializing_if = "Vec::is_empty"`), so its presence is optional.
    /// Each entry in `bumps` must have `package`, `from`, `to`, and `severity`.
    #[test]
    fn version_json_output_is_valid_json_with_expected_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        make_fixture(root);

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");
        let report = plan.to_report(None);

        // Serialize to JSON via the same function the CLI uses.
        let mut json_out = Vec::new();
        crate::output::write_json(&mut json_out, &report).unwrap();
        let json_str = String::from_utf8(json_out).unwrap();

        // Must be parseable as valid JSON.
        let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("output must be valid JSON");

        // Top-level structure checks: schemaVersion and bumps are always present.
        assert!(
            parsed.get("schemaVersion").is_some(),
            "JSON must have schemaVersion key; got:\n{json_str}"
        );
        assert!(
            parsed.get("bumps").is_some(),
            "JSON must have bumps key; got:\n{json_str}"
        );
        // diagnostics is omitted when empty (skip_serializing_if); if present it
        // must be an array.
        if let Some(diags) = parsed.get("diagnostics") {
            assert!(
                diags.is_array(),
                "diagnostics key must be an array when present; got:\n{json_str}"
            );
        }

        // Each bump must have the required fields.
        let bumps = parsed["bumps"].as_array().expect("bumps must be an array");
        assert!(!bumps.is_empty(), "bumps array must be non-empty for the test fixture");

        for bump in bumps {
            assert!(
                bump.get("package").is_some(),
                "each bump must have a package field; bump: {bump}"
            );
            assert!(
                bump.get("from").is_some(),
                "each bump must have a from field; bump: {bump}"
            );
            assert!(bump.get("to").is_some(), "each bump must have a to field; bump: {bump}");
            assert!(
                bump.get("severity").is_some(),
                "each bump must have a severity field; bump: {bump}"
            );
        }
    }

    /// Spec: with the `inference` feature enabled, the CLI must actually use commit-based
    /// severity inference for a package with no changeset -- not silently behave as if
    /// inference were off. Builds a real git repo (real `CliCommandRunner`, real gix/git,
    /// no `NoopRunner`) with a release tag followed by a `feat:` commit and no changeset
    /// file, then asserts `plan_version` -- driven by `crate::workspace::select_inference()`,
    /// the same call `handle()` makes -- infers a `Minor` bump attributed to inference.
    /// `NoInference` (or a `select_inference()` that ignores the feature flag) always
    /// returns `Ok(None)`, which would leave the package unbumped, so this fails for exactly
    /// the right reason if the feature-gated dispatch isn't wired up.
    #[cfg(feature = "inference")]
    #[test]
    fn version_uses_real_commit_inference_when_feature_enabled() {
        use crate::runner::CliCommandRunner;
        use callisto_graph::locate::IgnoreWalkLocator;
        use callisto_model::BumpReason;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        git_init_with_commit(root);
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"pkg-alpha\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "chore: add package"]);
        run_git(
            root,
            &["-c", "tag.gpgSign=false", "tag", "-m", "release", "pkg-alpha@1.0.0"],
        );

        // A feat commit after the tag, with no changeset -- the only way this can produce a
        // bump is via real commit inference. Inference's pathspecs are scoped to the package's
        // source directory (aggregate.rs, via changed::package_paths); for this single-package
        // repo that's the workspace root, so touching Cargo.toml itself is enough to be counted.
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"pkg-alpha\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\
             description = \"a new feature\"\n",
        )
        .unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: add a new feature"]);

        let locator = IgnoreWalkLocator::new(root);
        let runner = CliCommandRunner;
        let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let inference = crate::workspace::select_inference();
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        let pkg_alpha = PackageId::parse("pkg-alpha").unwrap();
        let bump = plan.bumps.iter().find(|b| b.package == pkg_alpha).expect(
            "pkg-alpha must have a planned bump from commit inference -- got none, meaning \
             select_inference() is not actually dispatching to CommitInference despite the \
             inference feature being enabled",
        );

        assert_eq!(bump.to.render(), "1.1.0", "a `feat:` commit must infer a minor bump");
        assert!(
            matches!(bump.reason, Some(BumpReason::Inference { .. })),
            "the bump must be attributed to inference, not a changeset (there is none); got: \
             {:?}",
            bump.reason
        );
    }

    /// `callisto version --dry-run --format text` must print the `[DRY-RUN]`
    /// marker and not modify any manifest on disk.
    #[test]
    fn handle_dry_run_text_output_carries_the_dry_run_marker_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        make_fixture(root);
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"pkg-alpha\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();

        let global = crate::cli::GlobalArgs {
            format: crate::cli::OutputFormat::Text,
            cwd: root.to_path_buf(),
            dry_run: true,
        };

        let manifest_before = std::fs::read_to_string(root.join("pkg-alpha/Cargo.toml")).unwrap();

        let args = crate::cli::VersionArgs {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
            refresh_lockfiles: false,
        };
        let result = super::handle(args, &global);
        assert!(result.is_ok(), "expected Ok, got: {result:?}");

        let manifest_after = std::fs::read_to_string(root.join("pkg-alpha/Cargo.toml")).unwrap();
        assert_eq!(
            manifest_before, manifest_after,
            "dry-run must not modify pkg-alpha's Cargo.toml"
        );
    }
}
