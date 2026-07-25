use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "callisto", version)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args, Clone, Debug)]
pub struct GlobalArgs {
    #[arg(long, global = true, value_enum, default_value = "text")]
    pub format: OutputFormat,

    #[arg(long, global = true, default_value = ".")]
    pub cwd: PathBuf,

    #[arg(
        long,
        global = true,
        help = "Preview manifest and file changes without writing to disk"
    )]
    pub dry_run: bool,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Subcommand, Clone, Debug)]
pub enum Command {
    Add(AddArgs),
    Status(StatusArgs),
    Version(VersionArgs),
    #[command(subcommand)]
    Pre(PreArgs),
    Validate(ValidateArgs),
    Snapshot(SnapshotArgs),
    Init(InitArgs),
    PlanPublish(PlanPublishArgs),
    ComposePrBody(ComposePrBodyArgs),
    Tag(TagArgs),
    Completions(CompletionsArgs),
    Schema(SchemaArgs),
}

#[derive(Args, Clone, Debug, Default)]
pub struct SchemaArgs {
    #[arg(long = "type", value_name = "TYPE")]
    pub target_type: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct AddArgs {
    #[arg(long = "package", value_name = "NAME:SEVERITY")]
    pub packages: Vec<String>,
    #[arg(long)]
    pub summary: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct StatusArgs {
    #[arg(long)]
    pub strict: bool,
    #[arg(long)]
    pub strict_graph: bool,
}

#[derive(Args, Clone, Debug)]
pub struct VersionArgs {
    #[arg(long)]
    pub refresh_lockfiles: bool,
    #[arg(long)]
    pub strict: bool,
    #[arg(long)]
    pub strict_graph: bool,
    #[arg(long)]
    pub allow_empty_changesets: bool,
}

#[derive(Subcommand, Clone, Debug)]
pub enum PreArgs {
    Enter { tag: String },
    Exit,
}

#[derive(Args, Clone, Debug)]
pub struct ValidateArgs {
    #[arg(long)]
    pub staged: bool,
    #[arg(long, value_name = "REF")]
    pub since: Option<String>,
    #[arg(long)]
    pub strict: bool,
    #[arg(long)]
    pub strict_graph: bool,
}

#[derive(Args, Clone, Debug)]
pub struct SnapshotArgs {
    #[arg(long)]
    pub tag: String,
}

#[derive(Args, Clone, Debug)]
pub struct InitArgs {
    #[arg(long)]
    pub yes: bool,
}

#[derive(Args, Clone, Debug, Default)]
pub struct PlanPublishArgs {}

#[derive(Args, Clone, Debug)]
pub struct ComposePrBodyArgs {
    #[arg(long, value_name = "TEXT|-")]
    pub existing_body: Option<String>,
    #[arg(long = "label")]
    pub labels: Vec<String>,
}

#[derive(Args, Clone, Debug)]
pub struct TagArgs {
    #[arg(long, value_name = "FILE|-")]
    pub plan: String,
}

#[derive(Args, Clone, Debug)]
pub struct CompletionsArgs {
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}
