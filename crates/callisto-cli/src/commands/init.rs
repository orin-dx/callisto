//! `callisto init`: report detected facts, ask only for intent, preview the first release, then write.

use std::io::Write;
use std::process::ExitCode;

use callisto_graph::commands::init::{
    self as scaffold, BinaryRelease, InitAnswers, InitFacts, InitPackage, Versioning,
};
use callisto_graph::locate::{IgnoreWalkLocator, LocateError};
use callisto_graph::GraphError;
use callisto_model::{ApplyPermit, CommandRunner, InitReport, SCHEMA_VERSION};
use dialoguer::{Confirm, Input, Select};

use crate::cli::{GlobalArgs, InitArgs, InitVersioning, OutputFormat};
use crate::error::CliError;
use crate::output::emit_report;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::tty;
use crate::workspace::workspace_root;

pub const VERSIONING_PROMPT: &str = "How should packages be versioned?";
pub const SHIP_PROMPT: &str = "Ship binaries as GitHub release assets?";
pub const PRODUCT_PROMPT: &str = "Which package is the product whose binaries ship?";
pub const FORGE_PROMPT: &str = "GitHub repository to release to (owner/repo)";
pub const TARGETS_PROMPT: &str = "Artifact target triples (comma-separated)";
pub const WRITE_PROMPT: &str = "Write callisto.toml?";
pub const WORKFLOW_PROMPT: &str = "Generate a GitHub Actions release workflow?";
/// Printed instead of the preview when the repository has no commit to plan from.
pub const NO_COMMITS: &str =
    "No commits yet: commit, then run `callisto release --dry-run` to preview the first release.";
/// Printed instead of asking `WORKFLOW_PROMPT` for a workspace no generated workflow covers.
fn workflow_unsupported_note(reason: scaffold::WorkflowUnsupported) -> String {
    format!("Skipping GitHub Actions workflow generation: {reason}.")
}
/// Printed once a generated workflow is written: a merge to `branch` is the release approval.
fn merge_publishes_note(branch: &str) -> String {
    format!(
        "Merging to `{branch}` publishes: require pull requests and reviews on `{branch}` (GitHub → Settings → Rules)."
    )
}
/// Printed once a generated workflow is written: what to do before the first release.
fn next_steps_note(secrets: &[&str]) -> String {
    let secrets = if secrets.is_empty() {
        String::new()
    } else {
        format!(
            "\n  - Add the repository secrets {} (GitHub → Settings → Secrets and variables → Actions).",
            secrets.join(", ")
        )
    };
    format!(
        "\nNext steps:{secrets}\n  - Describe a change with `callisto add`, then commit.\n  - Preview the first release with `callisto release --dry-run`.\n  - A new package's first publish needs a token that can create it: see {FIRST_PUBLISH_URL}."
    )
}
/// Where the registry-by-registry first-publish guidance lives.
const FIRST_PUBLISH_URL: &str = "https://github.com/orin-dx/callisto/blob/main/docs/publishing.md#first-publish";
const VERSIONING_CHOICES: [&str; 2] = [
    "independent: each package has its own version",
    "fixed: every package shares one version",
];

/// Asks the operator questions; a scripted implementation drives tests.
pub trait Prompter {
    fn select(&mut self, prompt: &str, items: &[String]) -> Result<usize, CliError>;
    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool, CliError>;
    fn input(&mut self, prompt: &str, default: Option<&str>) -> Result<String, CliError>;
}

struct TerminalPrompter;

impl Prompter for TerminalPrompter {
    fn select(&mut self, prompt: &str, items: &[String]) -> Result<usize, CliError> {
        Ok(Select::new().with_prompt(prompt).items(items).default(0).interact()?)
    }

    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool, CliError> {
        Ok(Confirm::new().with_prompt(prompt).default(default).interact()?)
    }

    fn input(&mut self, prompt: &str, default: Option<&str>) -> Result<String, CliError> {
        let mut input = Input::<String>::new().with_prompt(prompt);
        if let Some(default) = default {
            input = input.default(default.to_owned());
        }
        Ok(input.interact_text()?)
    }
}

pub fn handle(args: InitArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    run(
        args,
        global,
        tty::is_interactive(),
        &mut TerminalPrompter,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
        &CliCommandRunner,
    )
}

/// `init` against explicit terminal state, prompter, output streams, and command runner.
pub fn run<R: CommandRunner>(
    args: InitArgs,
    global: &GlobalArgs,
    interactive: bool,
    prompter: &mut dyn Prompter,
    out: &mut dyn Write,
    err: &mut dyn Write,
    runner: &R,
) -> Result<ExitCode, CliError> {
    validate_flags(&args)?;
    // A repository whose manifests all sit in subdirectories has no root yet: init makes the Git toplevel one.
    let (root, no_root) = match workspace_root(global, runner) {
        Ok(root) => (root, None),
        Err(CliError::Locate(LocateError::WorkspaceRootNotFound { start, toplevel })) => (
            toplevel.clone(),
            Some(LocateError::WorkspaceRootNotFound { start, toplevel }),
        ),
        Err(error) => return Err(error),
    };
    let locator = IgnoreWalkLocator::new(&root);
    let facts = scaffold::detect(&root, &locator, runner)?;
    if let Some(error) = no_root.filter(|_| facts.packages.is_empty()) {
        return Err(error.into());
    }
    let json = global.format == OutputFormat::Json;
    // JSON keeps stdout for the report.
    let human: &mut dyn Write = if json { &mut *err } else { &mut *out };
    write_facts(&facts, human)?;

    if facts.origin.is_none() {
        if args.workflow {
            return Err(CliError::InitFlagRequiresOrigin { flag: "workflow" });
        }
        if !args.artifact_targets.is_empty() {
            return Err(CliError::InitFlagRequiresOrigin {
                flag: "artifact-target",
            });
        }
    }
    if !interactive && !args.yes {
        let mut missing = vec!["--yes"];
        missing.extend(missing_flags(&args, &facts));
        return Err(CliError::InitRequiresYes { missing });
    }
    if args.yes {
        let missing = missing_flags(&args, &facts);
        if !missing.is_empty() {
            return Err(CliError::InitMissingFlags { missing });
        }
    }
    let prompting = interactive && !args.yes;
    let answers = collect_answers(&args, &facts, prompting, prompter)?;

    let mut diagnostics = scaffold::changesets_config_diagnostics(&facts.root);
    if facts.origin.is_none() {
        writeln!(human, "{ORIGIN_MISSING_NOTE}")?;
        diagnostics.push(callisto_model::Diagnostic {
            code: callisto_model::DiagnosticCode::InitOriginMissing,
            severity: callisto_model::DiagnosticSeverity::Warning,
            message: ORIGIN_MISSING_NOTE.to_owned(),
            package: None,
            path: None,
            escalated_by: None,
            governed_by: None,
        });
    }
    let shape = match scaffold::workflow_shape(&facts, &answers) {
        Err(reason) if args.workflow => {
            return Err(GraphError::InitWorkflowUnsupported {
                reason: reason.to_string(),
            }
            .into());
        }
        Err(reason) => {
            let note = workflow_unsupported_note(reason);
            writeln!(human, "{note}")?;
            diagnostics.push(callisto_model::Diagnostic {
                code: callisto_model::DiagnosticCode::WorkflowGenerationUnsupported,
                severity: callisto_model::DiagnosticSeverity::Warning,
                message: note,
                package: None,
                path: None,
                escalated_by: None,
                governed_by: None,
            });
            None
        }
        Ok(shape) => Some(shape),
    };
    let want_workflow = match shape {
        None => false,
        Some(_) if args.workflow => true,
        Some(_) if args.no_workflow => false,
        Some(_) if prompting && facts.origin.is_some() => prompter.confirm(WORKFLOW_PROMPT, true)?,
        // The workflow is GitHub Actions, so only a GitHub origin gets it unasked.
        Some(_) => facts.origin_repository.is_some(),
    };
    // A workflow nobody asked for (the default) never fails `init`: if it cannot be built, say why and go on.
    let asked_for_workflow = args.workflow || prompting;
    let workflow = match shape.filter(|_| want_workflow) {
        Some(shape) => match build_workflow(&facts, shape, runner, human) {
            Ok(built) => Some(built),
            Err(error) if !asked_for_workflow => {
                let note =
                    format!("Skipping the GitHub Actions workflow: {error}. Run `callisto init --workflow` to retry.");
                writeln!(human, "{note}")?;
                diagnostics.push(callisto_model::Diagnostic {
                    code: callisto_model::DiagnosticCode::WorkflowGenerationUnsupported,
                    severity: callisto_model::DiagnosticSeverity::Warning,
                    message: note,
                    package: None,
                    path: None,
                    escalated_by: None,
                    governed_by: None,
                });
                None
            }
            Err(error) => return Err(error),
        },
        None => None,
    };

    let config = scaffold::render_config(&facts, &answers);
    let plan = if facts.has_commit {
        Some(scaffold::preview(&facts, &config, &locator, runner)?)
    } else {
        None
    };
    if config
        .lines()
        .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
    {
        writeln!(human, "\ncallisto.toml: no settings needed, the defaults apply.")?;
    } else {
        writeln!(human, "\ncallisto.toml:\n{config}")?;
    }
    match plan {
        Some(plan) => {
            writeln!(human, "First release preview (`callisto release --dry-run`):")?;
            super::release::write_release_preview(plan.as_ref(), OutputFormat::Text, human)?;
        }
        None => writeln!(human, "{NO_COMMITS}")?,
    }
    if let Some((workflow, _)) = &workflow {
        writeln!(
            human,
            "\n{}:\n{workflow}",
            scaffold::workflow_path(&facts.root).display()
        )?;
    }

    // Computed before any write, so the `.changeset/README.md` existence check sees disk state as-is.
    let files = scaffold::init_files(&facts.root, workflow.is_some())?;
    let mut report = if global.dry_run {
        InitReport {
            schema_version: SCHEMA_VERSION,
            initialized: false,
            config_path: facts.root.join("callisto.toml"),
            config,
            files,
            diagnostics: Vec::new(),
        }
    } else {
        if prompting && !prompter.confirm(WRITE_PROMPT, true)? {
            writeln!(human, "Initialization cancelled; nothing was written.")?;
            return Ok(ExitCode::SUCCESS);
        }
        let permit = ApplyPermit::granted_unless_dry_run(global.dry_run).expect("non-dry-run permits writes");
        let mut report = scaffold::write(&facts.root, &config, &permit)?;
        if let Some((workflow, branch)) = &workflow {
            scaffold::write_workflow(&facts.root, workflow, &permit)?;
            let note = merge_publishes_note(branch);
            writeln!(human, "{note}")?;
            writeln!(human, "{}", next_steps_note(&scaffold::registry_secret_names(&facts)))?;
            diagnostics.push(callisto_model::Diagnostic {
                code: callisto_model::DiagnosticCode::WorkflowMergePublishes,
                severity: callisto_model::DiagnosticSeverity::Info,
                message: note,
                package: None,
                path: None,
                escalated_by: None,
                governed_by: None,
            });
        }
        report.files = files;
        report
    };
    report.diagnostics.extend(diagnostics);
    if json {
        emit_report(&mut &mut *out, &report, global.dry_run)?;
    } else {
        render::render_init(&report, &mut &mut *out)?;
    }
    Ok(ExitCode::SUCCESS)
}

/// The workflow text and the default branch it triggers on.
fn build_workflow<R: CommandRunner>(
    facts: &InitFacts,
    shape: scaffold::WorkflowShape,
    runner: &R,
    human: &mut dyn Write,
) -> Result<(String, String), CliError> {
    scaffold::ensure_workflow_absent(&facts.root)?;
    let branch = scaffold::default_branch(runner, &facts.root);
    writeln!(human, "  default branch: {} (from {})", branch.name, branch.source)?;
    let version = env!("CARGO_PKG_VERSION");
    let commit = scaffold::resolve_release_commit(runner, &facts.root, version)?;
    let content = scaffold::render_workflow(facts, shape, &branch.name, &commit, version);
    Ok((content, branch.name))
}

/// Printed, and recorded as a warning diagnostic, when no `origin` remote is configured.
const ORIGIN_MISSING_NOTE: &str = "No `origin` remote is configured: skipping the release workflow and \
shipping-binaries questions; `callisto release` will refuse until one is added.";

/// Rejects flag combinations invalid regardless of workspace facts.
fn validate_flags(args: &InitArgs) -> Result<(), CliError> {
    if args.workflow && args.no_workflow {
        return Err(CliError::InitWorkflowFlagsConflict);
    }
    if args.artifact_targets.is_empty() {
        if args.forge_repository.is_some() {
            return Err(CliError::InitFlagRequiresArtifactTarget {
                flag: "forge-repository",
            });
        }
        if args.product_package.is_some() {
            return Err(CliError::InitFlagRequiresArtifactTarget {
                flag: "product-package",
            });
        }
    }
    Ok(())
}

fn write_facts(facts: &InitFacts, out: &mut dyn Write) -> Result<(), CliError> {
    let ecosystems: Vec<&str> = facts.ecosystems.iter().map(|ecosystem| ecosystem.prefix()).collect();
    writeln!(out, "Detected:")?;
    writeln!(out, "  ecosystems: {}", ecosystems.join(", "))?;
    writeln!(out, "  origin: {}", facts.origin.as_deref().unwrap_or("none"))?;
    writeln!(out, "  packages:")?;
    for package in &facts.packages {
        let tag = package.last_tag.as_ref().map_or("none", |tag| tag.as_str());
        write!(out, "    {} {} (last tag: {tag}", package.id, package.version)?;
        if package.is_binary() {
            write!(out, "; binary: {}", package.bin_names.join(", "))?;
        }
        writeln!(out, ")")?;
    }
    Ok(())
}

/// Flags `--yes` needs that were not supplied.
fn missing_flags(args: &InitArgs, facts: &InitFacts) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if !args.artifact_targets.is_empty() && facts.binary_packages().next().is_some() {
        if args.forge_repository.is_none() {
            missing.push("--forge-repository");
        }
        if args.product_package.is_none() && facts.binary_packages().nth(1).is_some() {
            missing.push("--product-package");
        }
    }
    missing
}

fn missing(flag: &'static str) -> CliError {
    CliError::InitMissingFlags { missing: vec![flag] }
}

fn collect_answers(
    args: &InitArgs,
    facts: &InitFacts,
    prompting: bool,
    prompter: &mut dyn Prompter,
) -> Result<InitAnswers, CliError> {
    let versioning = match args.versioning {
        Some(InitVersioning::Fixed) => Versioning::Fixed,
        Some(InitVersioning::Independent) => Versioning::Independent,
        // Fixed and independent mean the same for one package, so only several packages are asked.
        None if prompting && facts.packages.len() > 1 => {
            let choices: Vec<String> = VERSIONING_CHOICES.iter().map(|choice| (*choice).to_owned()).collect();
            match prompter.select(VERSIONING_PROMPT, &choices)? {
                0 => Versioning::Independent,
                _ => Versioning::Fixed,
            }
        }
        None => Versioning::Independent,
    };
    let no_binaries = InitAnswers {
        versioning,
        binaries: None,
    };

    let binaries: Vec<&InitPackage> = facts.binary_packages().collect();
    if binaries.is_empty() && !args.artifact_targets.is_empty() {
        if let Some(package) = facts.unsupported_artifact_packages().next() {
            return Err(GraphError::InitUnsupportedArtifactBuilder {
                package: package.id.clone(),
                ecosystem: package
                    .id
                    .split_once('/')
                    .map_or("unknown", |(ecosystem, _)| ecosystem)
                    .to_owned(),
            }
            .into());
        }
        return Err(GraphError::InitNoBinaryPackage.into());
    }
    let requested_product = args
        .product_package
        .as_deref()
        .map(|name| facts.product_package(name))
        .transpose()?;
    if binaries.is_empty() {
        return Ok(no_binaries);
    }
    // No `origin`: the forge question below has nothing to validate a repository against, so
    // shipping is never offered (skips straight to `no_binaries`); `--artifact-target` without
    // `origin` is already rejected up front in `run`.
    let ship = !args.artifact_targets.is_empty()
        || (facts.origin.is_some() && prompting && prompter.confirm(SHIP_PROMPT, false)?);
    if !ship {
        return Ok(no_binaries);
    }

    let product = match (requested_product, binaries.as_slice()) {
        (Some(product), _) => product,
        (None, [only]) => only,
        (None, _) if prompting => {
            let ids: Vec<String> = binaries.iter().map(|package| package.id.clone()).collect();
            binaries[prompter.select(PRODUCT_PROMPT, &ids)?]
        }
        (None, _) => return Err(missing("--product-package")),
    };

    let forge_repository = match &args.forge_repository {
        Some(value) => scaffold::forge_repository(facts, value)?,
        None if prompting => {
            let detected = facts.origin_repository.as_ref().map(|repository| repository.as_slug());
            scaffold::forge_repository(facts, &prompter.input(FORGE_PROMPT, detected.as_deref())?)?
        }
        None => return Err(missing("--forge-repository")),
    };

    let targets = if args.artifact_targets.is_empty() {
        loop {
            let answer = prompter.input(TARGETS_PROMPT, None)?;
            let targets: Vec<String> = answer.split(',').map(str::trim).map(str::to_owned).collect();
            if targets.iter().all(|target| target.is_empty()) {
                // Nothing meaningful submitted: re-ask rather than error.
                continue;
            }
            break targets;
        }
    } else {
        args.artifact_targets.clone()
    };
    let targets = scaffold::validate_targets(&targets)?;

    Ok(InitAnswers {
        versioning,
        binaries: Some(BinaryRelease {
            product: product.id.clone(),
            forge_repository,
            targets,
        }),
    })
}

#[cfg(test)]
mod tests;
