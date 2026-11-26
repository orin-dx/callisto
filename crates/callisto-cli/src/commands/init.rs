//! `callisto init`: report detected facts, ask only for intent, preview the first release, then write.

use std::io::Write;
use std::process::ExitCode;

use callisto_graph::commands::init::{
    self as scaffold, changesets, BinaryRelease, InitAnswers, InitFacts, InitPackage, Versioning,
};
use callisto_graph::locate::{IgnoreWalkLocator, LocateError};
use callisto_graph::{GraphError, HOSTED_ORIGIN_FORMS};
use callisto_model::{ApplyPermit, BranchName, CommandRunner, InitReport, SCHEMA_VERSION};
use inquire::{Confirm, Select, Text};

use crate::cli::{GlobalArgs, InitArgs, InitVersioning, OutputFormat};
use crate::error::CliError;
use crate::output::{advise, emit_report};
use crate::render;
use crate::runner::CliCommandRunner;
use crate::tty;
use crate::workspace::workspace_root;

pub const VERSIONING_PROMPT: &str = "How should packages be versioned?";
pub const SHIP_PROMPT: &str = "Ship binaries as GitHub release assets?";
pub const PRODUCT_PROMPT: &str = "Which package is the product whose binaries ship?";
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
fn merge_publishes_note(branch: &BranchName) -> String {
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
    /// One of a few fixed `items`.
    fn select(&mut self, prompt: &str, items: &[String]) -> Result<usize, CliError>;
    /// One of a list of any length, such as the workspace's packages; a typed name is resolved by `by_name`.
    fn pick(&mut self, prompt: &str, items: &[String], by_name: tty::NameLookup<'_>) -> Result<usize, CliError>;
    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool, CliError>;
    fn input(&mut self, prompt: &str, default: Option<&str>) -> Result<String, CliError>;
}

struct TerminalPrompter;

impl Prompter for TerminalPrompter {
    fn select(&mut self, prompt: &str, items: &[String]) -> Result<usize, CliError> {
        Ok(Select::new(prompt, items.to_vec())
            .with_help_message("Up/Down move, Enter selects, type to filter")
            .raw_prompt()?
            .index)
    }

    fn pick(&mut self, prompt: &str, items: &[String], by_name: tty::NameLookup<'_>) -> Result<usize, CliError> {
        tty::pick_one(prompt, items, by_name)
    }

    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool, CliError> {
        Ok(Confirm::new(prompt).with_default(default).prompt()?)
    }

    fn input(&mut self, prompt: &str, default: Option<&str>) -> Result<String, CliError> {
        let mut input = Text::new(prompt);
        if let Some(default) = default {
            input = input.with_default(default);
        }
        Ok(input.prompt()?)
    }
}

pub fn handle(args: InitArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    run(
        args,
        global,
        tty::Prompts::detect(global.format),
        &mut TerminalPrompter,
        &mut crate::output::stdout(),
        &mut std::io::stderr(),
        &CliCommandRunner,
    )
}

/// Diagnostics queued while `init` runs. `run` emits whatever is still unshown when `init` fails, so an error never
/// swallows a warning that explains it.
#[derive(Default)]
struct Notes {
    /// Queued and not yet on stderr.
    items: Vec<callisto_model::Diagnostic>,
    shown: Vec<callisto_model::Diagnostic>,
}

impl Notes {
    /// Says the queued notes on stderr, best effort, and moves them to the shown ones; the JSON report carries them.
    fn emit(&mut self, err: &mut dyn Write) {
        advise_diagnostics(&self.items, err);
        self.shown.append(&mut self.items);
    }

    /// Every note, shown or not, for the JSON report; call `emit` first so each reaches stderr once.
    fn take_all(&mut self) -> Vec<callisto_model::Diagnostic> {
        let mut all = std::mem::take(&mut self.shown);
        all.append(&mut self.items);
        all
    }
}

/// Says `diagnostics` on stderr, best effort: the JSON report carries them too.
fn advise_diagnostics(diagnostics: &[callisto_model::Diagnostic], err: &mut dyn Write) {
    let mut text = Vec::new();
    render::render_diagnostics(diagnostics, None, &mut text).expect("rendering to memory cannot fail");
    advise(err, String::from_utf8_lossy(&text));
}

/// `init` against explicit terminal state (`prompts` is `Some` when it may ask questions), prompter, output streams, and
/// command runner.
pub fn run<R: CommandRunner>(
    args: InitArgs,
    global: &GlobalArgs,
    prompts: Option<tty::Prompts>,
    prompter: &mut dyn Prompter,
    out: &mut dyn Write,
    err: &mut dyn Write,
    runner: &R,
) -> Result<ExitCode, CliError> {
    let mut notes = Notes::default();
    let result = run_queued(
        args, global, prompts, prompter, &mut *out, &mut *err, runner, &mut notes,
    );
    if result.is_err() {
        notes.emit(err);
    }
    result
}

#[expect(clippy::too_many_arguments, reason = "init's explicit streams plus the note queue")]
fn run_queued<'a, R: CommandRunner>(
    args: InitArgs,
    global: &GlobalArgs,
    prompts: Option<tty::Prompts>,
    prompter: &mut dyn Prompter,
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
    runner: &R,
    notes: &mut Notes,
) -> Result<ExitCode, CliError> {
    validate_flags(&args)?;
    let interactive = prompts.is_some();
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
    if args.workflow && scaffold::is_initialized(&root)? {
        // Answers feed callisto.toml, which stays as it is: refuse them rather than drop them.
        // The forge and product flags already require an artifact target.
        if args.versioning.is_some() || !args.artifact_targets.is_empty() {
            return Err(GraphError::InitAlreadyInitialized {
                path: root.join("callisto.toml"),
            }
            .into());
        }
        return run_workflow_only(&root, &locator, global, out, err, runner);
    }
    let facts = scaffold::detect(&root, &locator, runner)?;
    if let Some(error) = no_root.filter(|_| facts.packages.is_empty()) {
        return Err(error.into());
    }
    let json = global.format == OutputFormat::Json;
    // JSON keeps stdout for the report.
    macro_rules! human {
        () => {
            if json {
                &mut *err
            } else {
                &mut *out
            }
        };
    }
    write_facts(&facts, human!())?;

    if facts.origin.is_none() {
        if args.workflow {
            return Err(CliError::InitFlagRequiresOrigin {
                flag: "workflow",
                not_hosted: facts.origin_not_hosted,
            });
        }
        if !args.artifact_targets.is_empty() {
            return Err(CliError::InitFlagRequiresOrigin {
                flag: "artifact-target",
                not_hosted: facts.origin_not_hosted,
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
    let requested = requested_release(&args, &facts)?;
    let prompting = interactive && !args.yes;
    let migration = changesets::migrate(&facts.root, &facts);
    // A dry run writes nothing, so it previews with the warning instead of refusing.
    let refusing = !migration.public_scoped.is_empty() && !args.yes && !global.dry_run;
    // Queued first so a refusal below still shows them; the refusal itself names the access hazard.
    notes.items.extend(migration.diagnostics_unless_refused(refusing));
    if refusing {
        return Err(CliError::InitChangesetsAccess {
            packages: migration.public_scoped,
        });
    }
    let mut answers = collect_answers(&args, &facts, &requested, prompting, prompter)?;

    notes.items.extend(migration.apply_fixed_groups(&mut answers));
    if facts.origin.is_none() {
        notes.items.push(callisto_model::Diagnostic {
            code: callisto_model::DiagnosticCode::InitOriginMissing,
            severity: callisto_model::DiagnosticSeverity::Warning,
            message: match facts.origin_not_hosted {
                Some(reason) => origin_not_hosted_note(reason),
                None => ORIGIN_MISSING_NOTE.to_owned(),
            },
            package: None,
            path: None,
            escalated_by: None,
            governed_by: None,
        });
    }
    let shape = match scaffold::workflow_shape(&facts, answers.binaries.is_some()) {
        Err(reason) if args.workflow => {
            return Err(GraphError::InitWorkflowUnsupported {
                reason: reason.to_string(),
            }
            .into());
        }
        Err(reason) => {
            let note = workflow_unsupported_note(reason);
            notes.items.push(callisto_model::Diagnostic {
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
    // An existing workflow is never overwritten, so it is not offered: `build_workflow` reports the same error.
    // Only an existing workflow skips the question; any other failure surfaces when the workflow is built.
    let workflow_blocked = matches!(
        scaffold::ensure_workflow_absent(&facts.root),
        Err(callisto_graph::GraphError::InitWorkflowExists { .. })
    );
    let want_workflow = match shape {
        None => false,
        Some(_) if args.workflow => true,
        Some(_) if args.no_workflow => false,
        Some(_) if prompting && facts.origin.is_some() && !workflow_blocked => {
            prompter.confirm(WORKFLOW_PROMPT, true)?
        }
        // The workflow is GitHub Actions, so only a GitHub origin gets it unasked.
        Some(_) => facts.origin_repository.is_some(),
    };
    // A workflow nobody asked for (the default) never fails `init`: if it cannot be built, say why and go on.
    let asked_for_workflow = args.workflow || (prompting && !workflow_blocked);
    let workflow = match shape.filter(|_| want_workflow) {
        Some(shape) => match build_workflow(&facts, shape, migration.base_branch.as_ref(), runner, human!()) {
            Ok(built) => Some(built),
            Err(error) if !asked_for_workflow => {
                let note = match &error {
                    CliError::Graph(callisto_graph::GraphError::InitWorkflowExists { path }) => format!(
                        "Keeping the existing GitHub Actions workflow at `{}`; to regenerate it, move it aside and run \
                         `callisto init --workflow`.",
                        path.display()
                    ),
                    _ => format!(
                        "Skipping the GitHub Actions workflow: {error}. Run `callisto init --workflow` to retry."
                    ),
                };
                notes.items.push(callisto_model::Diagnostic {
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
    if workflow.is_none() {
        notes.items.extend(migration.base_branch_unused());
    }

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
        writeln!(human!(), "\ncallisto.toml: no settings needed, the defaults apply.")?;
    } else {
        writeln!(human!(), "\ncallisto.toml:\n{config}")?;
    }
    match plan {
        Some((plan, pending)) => {
            writeln!(human!(), "First release preview (`callisto release --dry-run`):")?;
            super::release::write_release_preview(plan.as_ref(), OutputFormat::Text, None, human!())?;
            // `origin` already has its own note above, so an unbound tag adds nothing.
            if let Some(plan) = &plan {
                notes.items.extend(plan.diagnostics.clone());
            }
            let pending = super::release::PendingWork::of(pending);
            notes.items.extend(pending.diagnostic());
            pending.write_unchecked_note(&mut *err);
        }
        None => writeln!(human!(), "{NO_COMMITS}")?,
    }
    if let Some((_, branch)) = &workflow {
        writeln!(human!(), "\n{}", workflow_summary(&facts.root, branch))?;
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
        // Said before the write question so a decline keeps them.
        notes.emit(&mut *err);
        if prompting && !prompter.confirm(WRITE_PROMPT, true)? {
            // JSON never prompts, so a decline is always text.
            writeln!(human!(), "{}", tty::CANCELLED)?;
            return Ok(ExitCode::SUCCESS);
        }
        let permit = ApplyPermit::granted_unless_dry_run(global.dry_run).expect("non-dry-run permits writes");
        let mut report = scaffold::write(&facts.root, &config, &permit)?;
        if let Some((workflow, branch)) = &workflow {
            scaffold::write_workflow(&facts.root, workflow, &permit)?;
            notes.items.push(merge_publishes_diagnostic(branch));
            // The merge note and its advice come before the next steps.
            notes.emit(&mut *err);
            writeln!(
                human!(),
                "{}",
                next_steps_note(&scaffold::registry_secret_names(&facts))
            )?;
        }
        report.files = files;
        report
    };
    notes.emit(&mut *err);
    report.diagnostics.extend(notes.take_all());
    finish(&report, global, out)
}

/// Writes the report to stdout; its diagnostics already reached stderr.
fn finish(report: &InitReport, global: &GlobalArgs, out: &mut dyn Write) -> Result<ExitCode, CliError> {
    if global.format == OutputFormat::Json {
        emit_report(&mut &mut *out, report, global.dry_run)?;
    } else {
        render::render_init(report, &mut &mut *out)?;
    }
    Ok(ExitCode::SUCCESS)
}

/// The workflow's path and what it does, in place of its text.
fn workflow_summary(root: &std::path::Path, branch: &BranchName) -> String {
    format!(
        "{}: on each push to `{branch}`, opens a release PR while version work is pending and publishes once it merges.",
        scaffold::workflow_path(root).display()
    )
}

/// What a written workflow means for the branch it runs on, recorded as a diagnostic and said on stderr.
fn merge_publishes_diagnostic(branch: &BranchName) -> callisto_model::Diagnostic {
    callisto_model::Diagnostic {
        code: callisto_model::DiagnosticCode::WorkflowMergePublishes,
        severity: callisto_model::DiagnosticSeverity::Info,
        message: merge_publishes_note(branch),
        package: None,
        path: None,
        escalated_by: None,
        governed_by: None,
    }
}

/// `init --workflow` in a workspace that already has `callisto.toml`: writes only the workflow.
fn run_workflow_only<R: CommandRunner>(
    root: &std::path::Path,
    locator: &IgnoreWalkLocator,
    global: &GlobalArgs,
    out: &mut dyn Write,
    err: &mut dyn Write,
    runner: &R,
) -> Result<ExitCode, CliError> {
    let facts = scaffold::detect_initialized(root, locator, runner)?;
    let json = global.format == OutputFormat::Json;
    let human: &mut dyn Write = if json { &mut *err } else { &mut *out };
    write_facts(&facts, human)?;
    if facts.origin.is_none() {
        return Err(CliError::InitFlagRequiresOrigin {
            flag: "workflow",
            not_hosted: facts.origin_not_hosted,
        });
    }
    let shape =
        scaffold::workflow_shape(&facts, scaffold::existing_config_ships_binaries(&facts.root)?).map_err(|reason| {
            GraphError::InitWorkflowUnsupported {
                reason: reason.to_string(),
            }
        })?;
    let base_branch = changesets::base_branch(root);
    let (workflow, branch) = build_workflow(&facts, shape, base_branch.as_ref(), runner, human)?;
    writeln!(human, "\n{}", workflow_summary(&facts.root, &branch))?;
    let mut diagnostics = Vec::new();
    if !global.dry_run {
        let permit = ApplyPermit::granted_unless_dry_run(global.dry_run).expect("non-dry-run permits writes");
        scaffold::write_workflow(&facts.root, &workflow, &permit)?;
        let note = merge_publishes_diagnostic(&branch);
        advise_diagnostics(std::slice::from_ref(&note), err);
        let human: &mut dyn Write = if json { &mut *err } else { &mut *out };
        writeln!(human, "{}", next_steps_note(&scaffold::registry_secret_names(&facts)))?;
        diagnostics.push(note);
    }
    let config_path = facts.root.join("callisto.toml");
    let config = callisto_model::strict_fs::read_optional(&config_path)
        .map_err(|error| GraphError::InitIo {
            path: config_path.clone(),
            message: error.to_string(),
        })?
        .unwrap_or_default();
    let report = InitReport {
        schema_version: SCHEMA_VERSION,
        initialized: !global.dry_run,
        config_path,
        config,
        files: vec![scaffold::workflow_path(&facts.root)],
        diagnostics,
    };
    finish(&report, global, out)
}

/// The workflow text and the default branch it triggers on.
fn build_workflow<R: CommandRunner>(
    facts: &InitFacts,
    shape: scaffold::WorkflowShape,
    base_branch: Option<&BranchName>,
    runner: &R,
    human: &mut dyn Write,
) -> Result<(String, BranchName), CliError> {
    scaffold::ensure_workflow_absent(&facts.root)?;
    let branch = match base_branch {
        Some(name) => scaffold::DefaultBranch {
            name: name.clone(),
            source: scaffold::DefaultBranchSource::ChangesetsBaseBranch,
        },
        None => scaffold::default_branch(runner, &facts.root),
    };
    writeln!(human, "  default branch: {} (from {})", branch.name, branch.source)?;
    let version = semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo package versions are semver");
    let commit = scaffold::resolve_release_commit(runner, &facts.root, &version)?;
    let content = scaffold::render_workflow(facts, shape, &branch.name, &commit, &version);
    Ok((content, branch.name))
}

/// Printed, and recorded as a warning diagnostic, when no `origin` remote is configured.
const ORIGIN_MISSING_NOTE: &str = "No `origin` remote is configured: skipping the release workflow and \
shipping-binaries questions; `callisto release` will refuse until one is added.";

/// Printed, and recorded as a warning diagnostic, when `origin` is not a hosted remote; the value is never shown.
fn origin_not_hosted_note(reason: &str) -> String {
    format!(
        "`origin` is not a hosted remote: {reason} (use {HOSTED_ORIGIN_FORMS}); skipping the release workflow and \
shipping-binaries questions; `callisto release` will refuse until one is set."
    )
}

/// Rejects flag combinations invalid regardless of workspace facts.
fn validate_flags(args: &InitArgs) -> Result<(), CliError> {
    if args.workflow && args.no_workflow {
        return Err(CliError::InitWorkflowFlagsConflict);
    }
    if args.artifact_targets.is_empty() && args.product_package.is_some() {
        return Err(CliError::InitFlagRequiresArtifactTarget {
            flag: "product-package",
        });
    }
    Ok(())
}

fn write_facts(facts: &InitFacts, out: &mut dyn Write) -> Result<(), CliError> {
    let ecosystems: Vec<&str> = facts.ecosystems.iter().map(|ecosystem| ecosystem.prefix()).collect();
    writeln!(out, "Detected:")?;
    writeln!(out, "  ecosystems: {}", ecosystems.join(", "))?;
    let origin = match (&facts.origin, facts.origin_not_hosted.is_some()) {
        (Some(origin), _) => origin.as_str(),
        (None, true) => "not hosted",
        (None, false) => "none",
    };
    writeln!(out, "  origin: {origin}")?;
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
    if !args.artifact_targets.is_empty()
        && facts.binary_packages().next().is_some()
        && args.product_package.is_none()
        && facts.binary_packages().nth(1).is_some()
    {
        missing.push("--product-package");
    }
    missing
}

fn missing(flag: &'static str) -> CliError {
    CliError::InitMissingFlags { missing: vec![flag] }
}

/// What `--product-package` and `--artifact-target` ask for, checked against the workspace before any question.
struct RequestedRelease<'a> {
    product: Option<&'a InitPackage>,
    /// The validated targets; `None` when the flag was not given.
    targets: Option<Vec<String>>,
}

fn requested_release<'a>(args: &InitArgs, facts: &'a InitFacts) -> Result<RequestedRelease<'a>, CliError> {
    if args.artifact_targets.is_empty() {
        return Ok(RequestedRelease {
            product: None,
            targets: None,
        });
    }
    if facts.binary_packages().next().is_none() {
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
    let product = args
        .product_package
        .as_deref()
        .map(|name| facts.product_package(name))
        .transpose()?;
    // A release goes to origin's GitHub repository, so shipping needs one.
    if facts.origin_repository.is_none() {
        return Err(GraphError::ReleaseForgeRepositoryMissing.into());
    }
    let targets = scaffold::validate_targets(&args.artifact_targets)?;
    Ok(RequestedRelease {
        product,
        targets: Some(targets),
    })
}

fn collect_answers(
    args: &InitArgs,
    facts: &InitFacts,
    requested: &RequestedRelease<'_>,
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
        fixed_groups: Vec::new(),
    };

    let binaries: Vec<&InitPackage> = facts.binary_packages().collect();
    if binaries.is_empty() {
        return Ok(no_binaries);
    }
    // No `origin`: the forge question below has nothing to validate a repository against, so shipping is never
    // offered; `--artifact-target` without `origin` is already rejected up front in `run`.
    let ship =
        requested.targets.is_some() || (facts.origin.is_some() && prompting && prompter.confirm(SHIP_PROMPT, false)?);
    if !ship {
        return Ok(no_binaries);
    }
    // A release goes to origin's GitHub repository, so shipping needs one.
    if facts.origin_repository.is_none() {
        return Err(GraphError::ReleaseForgeRepositoryMissing.into());
    }

    let product = match (requested.product, binaries.as_slice()) {
        (Some(product), _) => product,
        (None, [only]) => only,
        (None, _) if prompting => {
            let ids: Vec<String> = binaries.iter().map(|package| package.id.clone()).collect();
            binaries[prompter.pick(PRODUCT_PROMPT, &ids, &|word| product_named(facts, &binaries, word))?]
        }
        (None, _) => return Err(missing("--product-package")),
    };

    let targets = match &requested.targets {
        Some(targets) => targets.clone(),
        None => {
            let answer = loop {
                let answer = prompter.input(TARGETS_PROMPT, None)?;
                let targets: Vec<String> = answer.split(',').map(str::trim).map(str::to_owned).collect();
                // Nothing meaningful submitted: re-ask rather than error.
                if !targets.iter().all(|target| target.is_empty()) {
                    break targets;
                }
            };
            scaffold::validate_targets(&answer)?
        }
    };

    Ok(InitAnswers {
        versioning,
        binaries: Some(BinaryRelease {
            product: product.id.clone(),
            targets,
        }),
        fixed_groups: Vec::new(),
    })
}

/// The index in `binaries` of the product a typed word names, resolved as `--product-package` resolves it, or why it
/// cannot be the product; `None` when it names no workspace package, so it may be a row number.
fn product_named(facts: &InitFacts, binaries: &[&InitPackage], word: &str) -> Option<Result<usize, String>> {
    if facts.named(word).is_empty() {
        return None;
    }
    Some(
        facts
            .product_package(word)
            .map(|product| {
                binaries
                    .iter()
                    .position(|package| package.id == product.id)
                    .expect("product_package accepts only a package binary_packages lists")
            })
            .map_err(|error| error.to_string()),
    )
}

#[cfg(test)]
mod tests;
