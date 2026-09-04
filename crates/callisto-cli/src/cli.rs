use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Changesets-style version and release manager for Rust workspaces.
#[derive(Parser)]
#[command(name = "callisto", version)]
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
    Matrix(MatrixArgs),
    /// Consume pending changesets and bump package versions accordingly.
    Version(VersionArgs),
    /// Manage prerelease mode for the workspace.
    #[command(subcommand)]
    Pre(PreArgs),
    /// Check that changesets and the dependency graph are well-formed.
    Validate(ValidateArgs),
    /// Apply a temporary, non-persistent version bump for a snapshot release.
    Snapshot(SnapshotArgs),
    /// Scaffold Callisto configuration in the current workspace.
    Init(InitArgs),
    /// Compute which packages are ready to publish and print the publish plan.
    PlanPublish(PlanPublishArgs),
    /// Publish ready packages to their ecosystem registries via native CLI tools.
    Publish(PublishArgs),
    /// Generate a pull request body summarizing pending release changes.
    ComposePrBody(ComposePrBodyArgs),
    /// Create git tags for packages in a publish plan.
    Tag(TagArgs),
    /// Filter a publish plan down to what a publish report confirms
    /// actually succeeded, dropping anything that failed.
    FilterPlan(FilterPlanArgs),
    /// Create, inspect, reconcile, or execute durable release intents.
    #[command(subcommand)]
    Release(ReleaseArgs),
    /// Decide the next managed release-pull-request operation from a forge snapshot.
    #[command(subcommand)]
    ReleasePr(ReleasePrArgs),
    /// Generate shell completion scripts.
    Completions(CompletionsArgs),
    /// Print the JSON schema for a report type.
    Schema(SchemaArgs),
}

/// Arguments for the `schema` command.
#[derive(Args, Clone, Debug, Default)]
pub struct SchemaArgs {
    /// Report type to print the schema for (status, version, snapshot, validate, tag, init, plan-publish, changeset, pre, matrix); defaults to status.
    #[arg(long = "type", value_name = "TYPE")]
    pub target_type: Option<String>,
}

/// Arguments for the `add` command.
#[derive(Args, Clone, Debug)]
pub struct AddArgs {
    /// Package and severity to include, as `name:severity` (patch, minor, or major); repeatable. Omit to enter the interactive wizard.
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
    /// Treat dependency-graph warnings as errors.
    #[arg(long)]
    pub strict_graph: bool,
    /// Exit with a distinct status code indicating whether any changesets are pending.
    #[arg(long)]
    pub check: bool,
}

/// Arguments for the `matrix` command.
#[derive(Args, Clone, Debug, Default)]
pub struct MatrixArgs {
    /// Restrict output to one registered package's name (PackageId::name()).
    #[arg(long)]
    pub package: Option<String>,
}

/// Arguments for the `version` command.
#[derive(Args, Clone, Debug)]
pub struct VersionArgs {
    /// Regenerate lockfiles after applying the version bumps.
    #[arg(long)]
    pub refresh_lockfiles: bool,
    /// Treat warning-level diagnostics as errors.
    #[arg(long)]
    pub strict: bool,
    /// Treat dependency-graph warnings as errors.
    #[arg(long)]
    pub strict_graph: bool,
    /// Allow versioning to proceed even if no changesets are pending.
    #[arg(long)]
    pub allow_empty_changesets: bool,
}

/// Subcommands for managing prerelease mode.
#[derive(Subcommand, Clone, Debug)]
pub enum PreArgs {
    /// Enter prerelease mode, tagging subsequent version bumps with the given prerelease tag.
    Enter { tag: String },
    /// Exit prerelease mode, returning to normal versioning.
    Exit,
}

/// Arguments for the `validate` command.
#[derive(Args, Clone, Debug)]
pub struct ValidateArgs {
    /// Validate only changesets staged in git.
    #[arg(long)]
    pub staged: bool,
    /// Validate only changesets added since the given git ref.
    #[arg(long, value_name = "REF", conflicts_with = "staged")]
    pub since: Option<String>,
    /// Treat warning-level diagnostics as errors.
    #[arg(long)]
    pub strict: bool,
    /// Treat dependency-graph warnings as errors.
    #[arg(long)]
    pub strict_graph: bool,
}

/// Arguments for the `snapshot` command.
#[derive(Args, Clone, Debug)]
pub struct SnapshotArgs {
    /// Tag to append to the snapshot version (e.g. a commit SHA or branch name).
    #[arg(long)]
    pub tag: String,
    /// Abort if the workspace graph contains crosscheck failures or other
    /// error-severity diagnostics.
    #[arg(long)]
    pub strict: bool,
}

/// Arguments for the `init` command.
#[derive(Args, Clone, Debug)]
pub struct InitArgs {
    /// Skip the interactive confirmation prompt.
    #[arg(long)]
    pub yes: bool,
}

/// Arguments for the `plan-publish` command.
#[derive(Args, Clone, Debug, Default)]
pub struct PlanPublishArgs {
    /// Plan only the named package(s). Repeatable: `--package foo --package bar`.
    #[arg(long = "package", value_name = "NAME")]
    pub only: Vec<String>,
}

/// Arguments for the `publish` command.
#[derive(Args, Clone, Debug, Default)]
pub struct PublishArgs {
    /// Publish only the named package(s). Repeatable: `--package foo --package bar`.
    /// When omitted, all packages in the plan are published.
    #[arg(long = "package", value_name = "NAME")]
    pub only: Vec<String>,
    /// Skip the pre-publish "already published?" registry check and publish
    /// directly, relying on the registry's own already-published response.
    /// Saves one round-trip per package, at the cost of re-running local
    /// publish lifecycle scripts (e.g. npm's prepublishOnly) on a retried
    /// publish of an already-published version.
    #[arg(long)]
    pub skip_publish_precheck: bool,
}

/// Arguments for the `compose-pr-body` command.
#[derive(Args, Clone, Debug)]
pub struct ComposePrBodyArgs {
    /// Existing PR body text to merge with, or `-` to read it from stdin.
    #[arg(long, value_name = "TEXT|-")]
    pub existing_body: Option<String>,
    /// Label to attach to the PR body; repeatable.
    #[arg(long = "label")]
    pub labels: Vec<String>,
    /// Branch name to reference in the generated PR body.
    #[arg(long)]
    pub branch: Option<String>,
}

/// Arguments for the `tag` command.
#[derive(Args, Clone, Debug)]
pub struct TagArgs {
    /// Path to a publish plan JSON file, inline JSON, or `-` to read it from stdin.
    #[arg(long, value_name = "FILE|-")]
    pub plan: String,
    /// Also move a floating major-version tag (e.g. `v1`) to point at the new tag.
    #[arg(long)]
    pub floating_major: bool,
    /// Abort if the workspace graph contains crosscheck failures or other
    /// error-severity diagnostics.
    #[arg(long)]
    pub strict: bool,
}

/// Arguments for the `filter-plan` command.
#[derive(Args, Clone, Debug)]
pub struct FilterPlanArgs {
    /// Path to a publish plan JSON file, inline JSON, or `-` to read it from stdin.
    #[arg(long, value_name = "FILE|-")]
    pub plan: String,
    /// Path to a publish report JSON file, inline JSON, or `-` to read it from stdin.
    #[arg(long, value_name = "FILE|-")]
    pub report: String,
}

/// Durable release subcommands.
#[derive(Subcommand, Clone, Debug)]
pub enum ReleaseArgs {
    /// Create a read-only durable release intent from exact package selections.
    Plan(ReleasePlanArgs),
    /// Display a durable intent, manifest, state, or receipt without recomputing it.
    Inspect(ReleaseInspectArgs),
    /// Report which pending operations are eligible without changing release state.
    Reconcile(ReleaseReconcileArgs),
    /// Execute a previously approved intent. This is the only durable mutation route.
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
        required_unless_present = "packages"
    )]
    pub from_release_commit: Option<String>,
    /// Explicit path where the immutable intent JSON will be atomically written.
    #[arg(long, value_name = "FILE")]
    pub out: PathBuf,
}

#[derive(Args, Clone, Debug)]
pub struct ReleaseInspectArgs {
    /// Explicit path to an intent, artifact manifest, state, or receipt JSON document.
    #[arg(long, value_name = "FILE")]
    pub input: PathBuf,
}

#[derive(Args, Clone, Debug)]
pub struct ReleaseReconcileArgs {
    /// Explicit path to the durable release intent JSON document.
    #[arg(long, value_name = "FILE")]
    pub intent: PathBuf,
    /// Explicit state path. If omitted, reconciliation reports the initialized state.
    #[arg(long, value_name = "FILE")]
    pub state: Option<PathBuf>,
}

#[derive(Args, Clone, Debug)]
pub struct ReleaseExecuteArgs {
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
    /// Explicit durable state path. If omitted, state is stored outside the checkout.
    #[arg(long, value_name = "FILE")]
    pub state: Option<PathBuf>,
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
        .is_ok());
    }

    /// AC-006/AC-007 (parse slice): `callisto matrix --package foo` parses
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

    /// AC-003b: bare `callisto matrix` (no --package) parses with package: None.
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
}
