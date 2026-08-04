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
    /// Generate shell completion scripts.
    Completions(CompletionsArgs),
    /// Print the JSON schema for a report type.
    Schema(SchemaArgs),
}

/// Arguments for the `schema` command.
#[derive(Args, Clone, Debug, Default)]
pub struct SchemaArgs {
    /// Report type to print the schema for (status, version, snapshot, validate, tag, init, plan-publish, changeset, pre); defaults to status.
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
    /// Treat warning-level diagnostics as errors.
    #[arg(long)]
    pub strict: bool,
    /// Treat dependency-graph warnings as errors.
    #[arg(long)]
    pub strict_graph: bool,
    /// Exit with a distinct status code indicating whether any changesets are pending.
    #[arg(long)]
    pub check: bool,
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

/// Arguments for the `plan-publish` command (currently none).
#[derive(Args, Clone, Debug, Default)]
pub struct PlanPublishArgs {}

/// Arguments for the `publish` command (currently none — use the global
/// `--dry-run` flag to preview the plan without publishing anything).
#[derive(Args, Clone, Debug, Default)]
pub struct PublishArgs {}

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

/// Arguments for the `completions` command.
#[derive(Args, Clone, Debug)]
pub struct CompletionsArgs {
    /// Shell to generate a completion script for.
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}
