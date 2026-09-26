use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Changesets-style version and release manager for Rust workspaces.
#[derive(Parser)]
#[command(name = "callisto", version, disable_help_subcommand = true)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Command,
}

/// Flags shared by every subcommand.
#[derive(Args, Clone, Debug)]
pub struct GlobalArgs {
    /// Output format for command results.
    #[arg(long, global = true, value_enum, default_value = "text")]
    pub format: OutputFormat,

    /// Workspace directory to operate in (defaults to the current directory).
    #[arg(long, global = true, default_value = ".")]
    pub cwd: PathBuf,

    #[arg(
        long,
        global = true,
        help = "Preview manifest and file changes without writing to disk"
    )]
    pub dry_run: bool,
}

/// Output rendering mode: human-readable text or machine-readable JSON.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

/// A callisto subcommand.
#[derive(Subcommand, Clone, Debug)]
pub enum Command {
    /// Create a new changeset describing pending package changes.
    Add(AddArgs),
    /// Show the workspace's pending changesets and diagnostics.
    Status(StatusArgs),
    /// Print the workspace's native-build target matrix as JSON or a table.
    #[command(hide = true)]
    Matrix(MatrixArgs),
    /// Consume pending changesets and bump package versions accordingly.
    Version(VersionArgs),
    /// Manage prerelease mode for the workspace.
    #[command(subcommand)]
    Pre(PreArgs),
    /// Apply a temporary, non-persistent version bump for a snapshot release.
    Snapshot(SnapshotArgs),
    /// Scaffold Callisto configuration in the current workspace.
    Init(InitArgs),
    /// Generate a pull request body summarizing pending release changes.
    #[command(hide = true)]
    ComposePrBody(ComposePrBodyArgs),
    /// Publish every package whose current version has not been released yet.
    Release(ReleaseCommandArgs),
    /// Decide the next managed release-pull-request operation from a forge snapshot.
    #[command(subcommand, hide = true)]
    ReleasePr(ReleasePrArgs),
    /// Generate shell completion scripts.
    Completions(CompletionsArgs),
    /// Print the JSON schema for a report type.
    #[command(hide = true)]
    Schema(SchemaArgs),
}

/// Arguments for the `schema` command.
#[derive(Args, Clone, Debug, Default)]
pub struct SchemaArgs {
    /// Report type to print the schema for (status, version, snapshot, validate, init, changeset, pre, matrix, release-receipt); defaults to status.
    #[arg(long = "type", value_name = "TYPE")]
    pub target_type: Option<String>,
}

/// Arguments for the `add` command.
#[derive(Args, Clone, Debug)]
pub struct AddArgs {
    /// Package and severity to include, as `name:severity` (none, patch, minor, or major); repeatable. Omit to enter the interactive wizard.
    #[arg(long = "package", value_name = "NAME:SEVERITY")]
    pub packages: Vec<String>,
    /// Human-readable summary of the change to record in the changeset.
    #[arg(long)]
    pub summary: Option<String>,
}

/// Arguments for the `status` command.
#[derive(Args, Clone, Debug)]
pub struct StatusArgs {
    /// Enable strict mode: promote warning-level diagnostics to errors, causing a non-zero exit.
    #[arg(long)]
    pub strict: bool,
    /// Exit with a distinct status code indicating whether any changesets are pending.
    #[arg(long)]
    pub check: bool,
}

/// Arguments for the `matrix` command.
#[derive(Args, Clone, Debug, Default)]
pub struct MatrixArgs {
    /// Restrict output to one package: a bare name (only if it names exactly
    /// one package) or an ecosystem-qualified id, for example `cargo/foo`.
    #[arg(long)]
    pub package: Option<String>,
}

/// Arguments for the `version` command.
#[derive(Args, Clone, Debug)]
pub struct VersionArgs {
    /// Accepted for compatibility; lockfile refresh is on by default now, so this is a no-op.
    #[arg(long, hide = true)]
    pub refresh_lockfiles: bool,
    /// Skip regenerating lockfiles after applying the version bumps.
    #[arg(long)]
    pub no_refresh_lockfiles: bool,
    /// Treat warning-level diagnostics as errors.
    #[arg(long)]
    pub strict: bool,
    /// Allow versioning to proceed even if no changesets are pending.
    #[arg(long)]
    pub allow_empty_changesets: bool,
    /// Writes the exact release decision this version plan authorizes --
    /// package, target version, and inclusion reason (changeset, fixed
    /// group, linked group, dependency cascade, or pre-release policy) --
    /// to the given path, alongside the manifest and changelog edits.
    ///
    /// A merged release PR's commit carrying this file is later the sole
    /// authority `release plan --from-release-commit` verifies against: no
    /// re-derivation of cascade or group policy happens at that boundary,
    /// only confirmation that the committed diff matches exactly what this
    /// command already decided, once, here.
    #[arg(long, value_name = "PATH")]
    pub emit_decision: Option<std::path::PathBuf>,
}

/// Subcommands for managing prerelease mode.
#[derive(Subcommand, Clone, Debug)]
pub enum PreArgs {
    /// Enter prerelease mode, tagging subsequent version bumps with the given prerelease tag.
    Enter { tag: String },
    /// Exit prerelease mode, returning to normal versioning.
    Exit,
}

/// Arguments for the `snapshot` command.
#[derive(Args, Clone, Debug)]
pub struct SnapshotArgs {
    /// Tag to append to the snapshot version (e.g. a commit SHA or branch name).
    #[arg(long)]
    pub tag: String,
    /// Abort if the workspace graph contains error-severity diagnostics.
    #[arg(long)]
    pub strict: bool,
}

/// Arguments for the `init` command.
#[derive(Args, Clone, Debug, Default)]
pub struct InitArgs {
    /// Run without prompts; required when stdin is not a terminal.
    #[arg(long)]
    pub yes: bool,
    /// Whether packages share one version (fixed) or version independently.
    #[arg(long, value_enum)]
    pub versioning: Option<InitVersioning>,
    /// Ship the product's binaries as GitHub release assets for this Rust target triple (repeatable, comma-separated).
    #[arg(long = "artifact-target", value_name = "TRIPLE", value_delimiter = ',')]
    pub artifact_targets: Vec<String>,
    /// The package whose binaries ship, when several packages produce one.
    #[arg(long, value_name = "PACKAGE")]
    pub product_package: Option<String>,
    /// The GitHub repository (`owner/repo`) binaries are released to.
    #[arg(long, value_name = "OWNER/REPO")]
    pub forge_repository: Option<String>,
    /// Generate `.github/workflows/callisto-release.yml`.
    #[arg(long)]
    pub workflow: bool,
    /// Do not generate a GitHub Actions release workflow.
    #[arg(long)]
    pub no_workflow: bool,
}

/// `init --versioning` values.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitVersioning {
    Fixed,
    Independent,
}

/// Arguments for the `compose-pr-body` command.
#[derive(Args, Clone, Debug)]
pub struct ComposePrBodyArgs {
    /// Existing PR body text to merge with, or `-` to read it from stdin.
    #[arg(long, value_name = "TEXT|-")]
    pub existing_body: Option<String>,
    /// Branch name to reference in the generated PR body.
    #[arg(long)]
    pub branch: Option<String>,
}

/// Arguments for the `release` command.
#[derive(Args, Clone, Debug)]
#[command(args_conflicts_with_subcommands = true)]
pub struct ReleaseCommandArgs {
    #[command(subcommand)]
    pub command: Option<ReleaseArgs>,
    /// Release only this exact qualified package, for example `cargo/callisto-cli`. Repeatable.
    #[arg(long = "package", value_name = "ECOSYSTEM/NAME")]
    pub packages: Vec<String>,
    /// Also write the release receipt to this file.
    #[arg(long, value_name = "FILE")]
    pub receipt: Option<PathBuf>,
}

/// Durable release subcommands for the CI plan/build/execute route.
#[derive(Subcommand, Clone, Debug)]
pub enum ReleaseArgs {
    /// Create a read-only durable release intent from exact package selections.
    #[command(hide = true)]
    Plan(ReleasePlanArgs),
    /// Display a durable intent, manifest, or receipt without recomputing it.
    #[command(hide = true)]
    Inspect(ReleaseInspectArgs),
    /// Create the exact artifact manifest for a completed intent-bound build.
    #[command(hide = true)]
    ArtifactManifest(ReleaseArtifactManifestArgs),
    /// Execute a previously approved intent. This is the only durable mutation route.
    #[command(hide = true)]
    Execute(ReleaseExecuteArgs),
}

/// Read-only managed release-pull-request subcommands.
#[derive(Subcommand, Clone, Debug)]
pub enum ReleasePrArgs {
    /// Derive create, update, or no-op from fresh workspace and forge facts.
    Decide(ReleasePrDecideArgs),
    /// Verify that a freshly collected forge snapshot still matches a prior decision.
    Verify(ReleasePrVerifyArgs),
    /// Build the exact forge commit-API file changes for the staged Git index.
    CommitPlan(ReleasePrCommitPlanArgs),
}

#[derive(Args, Clone, Debug)]
pub struct ReleasePrDecideArgs {
    /// Versioned credential-free forge snapshot JSON, inline JSON, file path, or `-` for stdin.
    #[arg(long, value_name = "FILE|JSON|-")]
    pub snapshot: String,
    /// Exact configured forge repository identity, for example `orin-dx/callisto`.
    #[arg(long, value_name = "OWNER/REPOSITORY")]
    pub repository: String,
    /// Configured base branch for the managed release PR.
    #[arg(long)]
    pub base_branch: String,
    /// Canonical branch managed for the release PR.
    #[arg(long)]
    pub release_branch: String,
}

#[derive(Args, Clone, Debug)]
pub struct ReleasePrVerifyArgs {
    /// Versioned decision JSON, inline JSON, file path, or `-` for stdin.
    #[arg(long, value_name = "FILE|JSON|-")]
    pub decision: String,
    /// Fresh versioned credential-free forge snapshot JSON, inline JSON, file path, or `-` for stdin.
    #[arg(long, value_name = "FILE|JSON|-")]
    pub snapshot: String,
}

#[derive(Args, Clone, Debug)]
pub struct ReleasePrCommitPlanArgs {
    /// Full commit SHA the staged changes are relative to (the staging branch's root).
    #[arg(long)]
    pub base_commit: String,
    /// Commit message to record in the plan for the forge commit API.
    #[arg(long)]
    pub message: String,
    /// Write the plan to this file instead of stdout.
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
}

#[derive(Args, Clone, Debug)]
pub struct ReleasePlanArgs {
    /// Read the release source from this checkout while the Callisto binary
    /// itself may come from a separate, current orchestration checkout.
    #[arg(long, value_name = "DIR")]
    pub source_root: Option<PathBuf>,
    /// Exact current coordinator revision whose workflow will attest product assets.
    #[arg(long, value_name = "SHA")]
    pub orchestration_revision: Option<String>,
    /// GitHub repository receiving the product release assets, for example
    /// `orin-dx/callisto`. Required when the workspace declares product assets.
    #[arg(long, value_name = "OWNER/REPOSITORY", requires = "orchestration_revision")]
    pub artifact_repository: Option<String>,
    /// Exact qualified package identity, for example `cargo/callisto-cli`. Repeat this flag to select multiple packages. This local/manual mode cannot be combined with --from-release-commit.
    #[arg(
        long = "package",
        value_name = "ECOSYSTEM/NAME",
        conflicts_with = "from_release_commit",
        required_unless_present = "from_release_commit"
    )]
    pub packages: Vec<String>,
    /// Exact merged release-PR commit checked out in this workspace. CI derives the roster from its versioned manifest delta and never reruns changeset planning.
    #[arg(
        long,
        value_name = "SHA",
        conflicts_with = "packages",
        required_unless_present = "packages",
        requires = "decision"
    )]
    pub from_release_commit: Option<String>,
    /// Exact release-decision JSON path committed alongside the manifest and
    /// changelog edits in the merge commit (written by `callisto version
    /// --emit-decision`). Required with --from-release-commit: this function
    /// verifies the committed diff matches this file exactly rather than
    /// re-deriving changeset, fixed-group, linked-group, cascade, or
    /// pre-release-policy inclusion from scratch.
    #[arg(long, value_name = "FILE")]
    pub decision: Option<PathBuf>,
    /// Explicit path where the immutable intent JSON will be atomically written.
    #[arg(long, value_name = "FILE")]
    pub out: PathBuf,
}

#[derive(Args, Clone, Debug)]
pub struct ReleaseInspectArgs {
    /// Explicit path to an intent, artifact manifest, or receipt JSON document.
    #[arg(long, value_name = "FILE")]
    pub input: PathBuf,
}

#[derive(Args, Clone, Debug)]
pub struct ReleaseArtifactManifestArgs {
    /// Exact durable release intent that declares the expected artifact slots.
    #[arg(long, value_name = "FILE")]
    pub intent: PathBuf,
    /// Directory containing one regular file for every declared artifact slot.
    #[arg(long, value_name = "DIR")]
    pub artifact_dir: PathBuf,
    /// Explicit path where the artifact manifest will be atomically written.
    #[arg(long, value_name = "FILE")]
    pub out: PathBuf,
}

#[derive(Args, Clone, Debug)]
pub struct ReleaseExecuteArgs {
    /// Validate and execute against this exact release-source checkout. The
    /// current CLI/orchestration checkout is never inferred from this path.
    #[arg(long, value_name = "DIR")]
    pub source_root: Option<PathBuf>,
    /// Explicit path to the durable release intent JSON document.
    #[arg(long, value_name = "FILE")]
    pub intent: PathBuf,
    /// Explicit artifact manifest path required when the intent declares binary slots.
    #[arg(long, value_name = "FILE")]
    pub artifact_manifest: Option<PathBuf>,
    /// Directory containing the exact regular artifact files named by the manifest.
    /// Required with --artifact-manifest; it is never inferred from the checkout.
    #[arg(long, value_name = "DIR", requires = "artifact_manifest")]
    pub artifact_dir: Option<PathBuf>,
    /// Write the terminal, provider-observed receipt to this explicit path.
    /// A release is not reported successful until this receipt is written.
    #[arg(long, value_name = "FILE")]
    pub receipt: PathBuf,
    /// Exact current coordinator revision that executed this release.
    #[arg(long, value_name = "SHA")]
    pub orchestration_revision: String,
}

/// Arguments for the `completions` command.
#[derive(Args, Clone, Debug)]
pub struct CompletionsArgs {
    /// Shell to generate a completion script for.
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// QW-5: --strict flag on status subcommand must have a meaningful help string.
    #[test]
    fn strict_flag_help_text_is_meaningful() {
        let mut cmd = Cli::command();
        cmd.build();

        // Find the "status" subcommand.
        let status_sub = cmd
            .get_subcommands()
            .find(|s| s.get_name() == "status")
            .expect("status subcommand must exist");

        // Find the --strict argument.
        let strict_arg = status_sub
            .get_arguments()
            .find(|a| a.get_long() == Some("strict"))
            .expect("--strict argument must exist on status subcommand");

        let help = strict_arg
            .get_help()
            .map(|h| h.to_string())
            .unwrap_or_default()
            .to_lowercase();

        // Must contain "strict" and describe what it does.
        assert!(
            help.contains("strict"),
            "--strict help text must contain the word 'strict'; got: {help:?}"
        );
        assert!(
            help.contains("warning") || help.contains("error"),
            "--strict help text must mention 'warning' or 'error'; got: {help:?}"
        );
        // Must be longer than a placeholder.
        assert!(
            help.len() > 20,
            "--strict help text is too short to be meaningful: {help:?}"
        );
    }

    #[test]
    fn release_plan_requires_exactly_one_authority_mode() {
        use clap::Parser;

        assert!(Cli::try_parse_from(["callisto", "release", "plan", "--out", "intent.json"]).is_err());
        assert!(Cli::try_parse_from([
            "callisto",
            "release",
            "plan",
            "--package",
            "cargo/demo",
            "--from-release-commit",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--out",
            "intent.json",
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "callisto",
            "release",
            "plan",
            "--from-release-commit",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--out",
            "intent.json",
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "callisto",
            "release",
            "plan",
            "--from-release-commit",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--decision",
            "release-decision.json",
            "--out",
            "intent.json",
        ])
        .is_ok());
    }

    #[test]
    fn release_artifact_manifest_requires_all_explicit_paths() {
        use clap::Parser;

        assert!(Cli::try_parse_from(["callisto", "release", "artifact-manifest"]).is_err());
        assert!(Cli::try_parse_from([
            "callisto",
            "release",
            "artifact-manifest",
            "--intent",
            "intent.json",
            "--artifact-dir",
            "artifacts",
            "--out",
            "manifest.json",
        ])
        .is_ok());
    }

    /// The legacy publish commands no longer parse.
    #[test]
    fn legacy_publish_commands_are_unrecognized() {
        use clap::Parser;

        for args in [
            vec!["callisto", "publish"],
            vec!["callisto", "plan-publish"],
            vec!["callisto", "tag", "--plan", "plan.json"],
            vec!["callisto", "filter-plan", "--plan", "p.json", "--report", "r.json"],
        ] {
            let error = Cli::try_parse_from(&args)
                .err()
                .unwrap_or_else(|| panic!("{args:?} parsed"));
            assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand, "{args:?}");
        }
    }

    /// Bare release takes repeatable `--package`, not `--from-release-commit`.
    #[test]
    fn bare_release_accepts_packages_and_rejects_release_commit() {
        use clap::Parser;

        let cli = Cli::try_parse_from([
            "callisto",
            "release",
            "--dry-run",
            "--package",
            "cargo/a",
            "--package",
            "npm/b",
            "--receipt",
            "/tmp/receipt.json",
        ])
        .unwrap();
        assert!(cli.global.dry_run);
        let Command::Release(args) = cli.command else {
            panic!("expected release");
        };
        assert!(args.command.is_none());
        assert_eq!(args.packages, ["cargo/a", "npm/b"]);
        assert_eq!(args.receipt, Some(PathBuf::from("/tmp/receipt.json")));

        let bare = Cli::try_parse_from(["callisto", "release"]).unwrap();
        assert!(matches!(
            bare.command,
            Command::Release(ReleaseCommandArgs { command: None, .. })
        ));

        assert!(Cli::try_parse_from(["callisto", "release", "--from-release-commit", &"a".repeat(40)]).is_err());
        assert!(
            Cli::try_parse_from(["callisto", "release", "--package", "cargo/a", "inspect", "--input", "x"]).is_err()
        );
    }

    /// `callisto matrix --package foo` parses
    /// into Command::Matrix with the package field populated; MatrixArgs
    /// declares no --format of its own (the global flag is used instead).
    #[test]
    fn test_cli_parse_matrix_command_with_package() {
        use clap::Parser;
        let cli = Cli::parse_from(["callisto", "matrix", "--package", "foo"]);
        if let Command::Matrix(args) = cli.command {
            assert_eq!(args.package, Some("foo".to_string()));
        } else {
            panic!("Expected Matrix command");
        }
    }

    /// Bare `callisto matrix` (no --package) parses with package: None.
    #[test]
    fn test_cli_parse_matrix_command_bare() {
        use clap::Parser;
        let cli = Cli::parse_from(["callisto", "matrix"]);
        if let Command::Matrix(args) = cli.command {
            assert_eq!(args.package, None);
        } else {
            panic!("Expected Matrix command");
        }
    }

    /// `--help` lists exactly the 8 user commands,
    /// each with one plain-language line free of internal jargon.
    #[test]
    fn help_lists_exactly_eight_commands_with_plain_help_text() {
        let mut cmd = Cli::command();
        cmd.build();

        let visible: Vec<&str> = cmd
            .get_subcommands()
            .filter(|s| !s.is_hide_set())
            .map(|s| s.get_name())
            .collect();
        let expected = [
            "add",
            "status",
            "version",
            "pre",
            "snapshot",
            "init",
            "release",
            "completions",
        ];
        assert_eq!(
            visible.len(),
            8,
            "expected exactly 8 visible commands, got: {visible:?}"
        );
        for name in expected {
            assert!(
                visible.contains(&name),
                "missing {name:?} from visible commands: {visible:?}"
            );
        }

        let banned = [
            "durable release intent",
            "forge snapshot",
            "coordinator revision",
            "schema v",
        ];
        for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()) {
            let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
            assert_eq!(
                about.lines().count().max(1),
                1,
                "{}'s help text must be exactly one line: {about:?}",
                sub.get_name()
            );
            assert!(!about.is_empty(), "{} has no help text", sub.get_name());
            let lower = about.to_lowercase();
            for phrase in banned {
                assert!(
                    !lower.contains(phrase),
                    "{}'s help text contains banned jargon {phrase:?}: {about:?}",
                    sub.get_name()
                );
            }
        }
    }

    /// The automatic `help` subcommand is disabled,
    /// but `-h`/`--help` still work on the root command and on subcommands.
    #[test]
    fn help_subcommand_is_disabled_but_flag_help_works() {
        use clap::Parser;

        let help_sub = Cli::try_parse_from(["callisto", "help"]);
        assert!(help_sub.is_err(), "`callisto help` must fail to parse");
        assert_eq!(
            help_sub.err().unwrap().kind(),
            clap::error::ErrorKind::InvalidSubcommand
        );

        let help_sub_command = Cli::try_parse_from(["callisto", "help", "status"]);
        assert!(help_sub_command.is_err(), "`callisto help status` must fail to parse");

        let root_flag = Cli::try_parse_from(["callisto", "--help"]);
        assert_eq!(
            root_flag.err().unwrap().kind(),
            clap::error::ErrorKind::DisplayHelp,
            "-h/--help must still work on the root command"
        );

        let sub_flag = Cli::try_parse_from(["callisto", "status", "--help"]);
        assert_eq!(
            sub_flag.err().unwrap().kind(),
            clap::error::ErrorKind::DisplayHelp,
            "-h/--help must still work on subcommands"
        );
    }

    /// Plumbing subcommands stay callable but hidden
    /// from `callisto --help` and `callisto release --help`.
    #[test]
    fn plumbing_subcommands_are_hidden_but_callable() {
        let mut cmd = Cli::command();
        cmd.build();

        for name in ["matrix", "schema", "compose-pr-body", "release-pr"] {
            let sub = cmd
                .get_subcommands()
                .find(|s| s.get_name() == name)
                .unwrap_or_else(|| panic!("{name} must still be a registered subcommand"));
            assert!(sub.is_hide_set(), "{name} must be hidden from top-level --help");
        }

        let release = cmd
            .get_subcommands()
            .find(|s| s.get_name() == "release")
            .expect("release subcommand must exist");
        for name in ["plan", "inspect", "artifact-manifest", "execute"] {
            let sub = release
                .get_subcommands()
                .find(|s| s.get_name() == name)
                .unwrap_or_else(|| panic!("release {name} must still be a registered subcommand"));
            assert!(sub.is_hide_set(), "release {name} must be hidden from `release --help`");
        }

        // Still callable: parsing succeeds for every hidden subcommand.
        use clap::Parser;
        assert!(Cli::try_parse_from(["callisto", "matrix"]).is_ok());
        assert!(Cli::try_parse_from(["callisto", "schema"]).is_ok());
        assert!(Cli::try_parse_from(["callisto", "compose-pr-body"]).is_ok());
        assert!(Cli::try_parse_from([
            "callisto",
            "release",
            "plan",
            "--package",
            "cargo/demo",
            "--out",
            "intent.json"
        ])
        .is_ok());
        assert!(Cli::try_parse_from(["callisto", "release", "inspect", "--input", "x"]).is_ok());
        assert!(Cli::try_parse_from([
            "callisto",
            "release-pr",
            "decide",
            "--snapshot",
            "-",
            "--repository",
            "o/r",
            "--base-branch",
            "main",
            "--release-branch",
            "release"
        ])
        .is_ok());
    }

    /// `validate` is removed, not merely hidden --
    /// parsing it must fail as an unrecognized subcommand.
    #[test]
    fn validate_subcommand_fails_to_parse() {
        use clap::Parser;
        assert!(
            Cli::try_parse_from(["callisto", "validate"]).is_err(),
            "`validate` must no longer parse as a subcommand"
        );
    }

    /// A comma-separated `--artifact-target` value keeps every
    /// entry, including empties, so `collect_answers` can error naming the empty one.
    #[test]
    fn artifact_target_flag_splits_on_comma() {
        use clap::Parser;
        let cli = Cli::parse_from(["callisto", "init", "--artifact-target", "a,,b"]);
        let Command::Init(args) = cli.command else {
            panic!("expected Init command");
        };
        assert_eq!(args.artifact_targets, vec!["a", "", "b"]);
    }
}
