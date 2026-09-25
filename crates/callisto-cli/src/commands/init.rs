//! `callisto init`: report detected facts, ask only for intent, preview the first release, then write.

use std::io::Write;
use std::process::ExitCode;

use callisto_graph::commands::init::{
    self as scaffold, BinaryRelease, InitAnswers, InitFacts, InitPackage, Versioning,
};
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::GraphError;
use callisto_model::{ApplyPermit, CommandRunner, InitReport, SCHEMA_VERSION};
use dialoguer::{Confirm, Input, Select};

use crate::cli::{GlobalArgs, InitArgs, InitVersioning, OutputFormat};
use crate::error::CliError;
use crate::output::write_json;
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

fn prompt_failed(error: dialoguer::Error) -> CliError {
    CliError::Other(format!("Interactive prompt failed: {error}"))
}

impl Prompter for TerminalPrompter {
    fn select(&mut self, prompt: &str, items: &[String]) -> Result<usize, CliError> {
        Select::new()
            .with_prompt(prompt)
            .items(items)
            .default(0)
            .interact()
            .map_err(prompt_failed)
    }

    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool, CliError> {
        Confirm::new()
            .with_prompt(prompt)
            .default(default)
            .interact()
            .map_err(prompt_failed)
    }

    fn input(&mut self, prompt: &str, default: Option<&str>) -> Result<String, CliError> {
        let mut input = Input::<String>::new().with_prompt(prompt);
        if let Some(default) = default {
            input = input.default(default.to_owned());
        }
        input.interact_text().map_err(prompt_failed)
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
    let root = workspace_root(global, runner)?;
    let locator = IgnoreWalkLocator::new(&root);
    let facts = scaffold::detect(&root, &locator, runner)?;
    let json = global.format == OutputFormat::Json;
    // JSON keeps stdout for the report.
    let human: &mut dyn Write = if json { &mut *err } else { &mut *out };
    write_facts(&facts, human)?;

    if args.workflow && args.no_workflow {
        return Err(CliError::InitWorkflowFlagsConflict);
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
    let answers = collect_answers(&args, &facts, prompting, prompter, runner)?;

    let mut diagnostics = Vec::new();
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
        Some(_) if prompting => prompter.confirm(WORKFLOW_PROMPT, false)?,
        Some(_) => false,
    };
    let workflow = match shape.filter(|_| want_workflow) {
        Some(shape) => {
            scaffold::ensure_workflow_absent(&facts.root)?;
            let branch = scaffold::default_branch(runner, &facts.root);
            let version = env!("CARGO_PKG_VERSION");
            let commit = scaffold::resolve_release_commit(runner, &facts.root, version)?;
            let content = scaffold::render_workflow(&facts, shape, &branch, &commit, version);
            Some((content, branch))
        }
        None => None,
    };

    let config = scaffold::render_config(&facts, &answers);
    let plan = if facts.has_commit {
        Some(scaffold::preview(&facts, &config, &locator, runner)?)
    } else {
        None
    };
    writeln!(human, "\ncallisto.toml:\n{config}")?;
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

    let mut report = if global.dry_run {
        InitReport {
            schema_version: SCHEMA_VERSION,
            initialized: false,
            config_path: facts.root.join("callisto.toml"),
            config,
            diagnostics: Vec::new(),
        }
    } else {
        if prompting && !prompter.confirm(WRITE_PROMPT, true)? {
            writeln!(human, "Initialization cancelled; nothing was written.")?;
            return Ok(ExitCode::SUCCESS);
        }
        let permit = ApplyPermit::granted_unless_dry_run(false).expect("non-dry-run permits writes");
        let report = scaffold::write(&facts.root, &config, &permit)?;
        if let Some((workflow, branch)) = &workflow {
            scaffold::write_workflow(&facts.root, workflow, &permit)?;
            let note = merge_publishes_note(branch);
            writeln!(human, "{note}")?;
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
        report
    };
    report.diagnostics.extend(diagnostics);
    if json {
        write_json(&mut &mut *out, &report)?;
    } else {
        render::render_init(&report, &mut &mut *out)?;
    }
    Ok(ExitCode::SUCCESS)
}

fn write_facts(facts: &InitFacts, out: &mut dyn Write) -> Result<(), CliError> {
    let ecosystems: Vec<&str> = facts.ecosystems.iter().map(|ecosystem| ecosystem.prefix()).collect();
    writeln!(out, "Detected:")?;
    writeln!(out, "  ecosystems: {}", ecosystems.join(", "))?;
    writeln!(out, "  origin: {}", facts.origin)?;
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
    if args.versioning.is_none() {
        missing.push("--versioning");
    }
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

fn collect_answers<R: CommandRunner>(
    args: &InitArgs,
    facts: &InitFacts,
    prompting: bool,
    prompter: &mut dyn Prompter,
    runner: &R,
) -> Result<InitAnswers, CliError> {
    let versioning = match args.versioning {
        Some(InitVersioning::Fixed) => Versioning::Fixed,
        Some(InitVersioning::Independent) => Versioning::Independent,
        None if prompting => {
            let choices: Vec<String> = VERSIONING_CHOICES.iter().map(|choice| (*choice).to_owned()).collect();
            match prompter.select(VERSIONING_PROMPT, &choices)? {
                0 => Versioning::Independent,
                _ => Versioning::Fixed,
            }
        }
        None => return Err(missing("--versioning")),
    };
    let no_binaries = InitAnswers {
        versioning,
        binaries: None,
    };

    let binaries: Vec<&InitPackage> = facts.binary_packages().collect();
    if binaries.is_empty() && !args.artifact_targets.is_empty() {
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
    let ship = !args.artifact_targets.is_empty() || (prompting && prompter.confirm(SHIP_PROMPT, false)?);
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
            let targets: Vec<String> = answer
                .split(',')
                .map(str::trim)
                .filter(|target| !target.is_empty())
                .map(str::to_owned)
                .collect();
            if !targets.is_empty() {
                break targets;
            }
        }
    } else {
        args.artifact_targets.clone()
    };
    let known = scaffold::known_target_triples(runner, &facts.root);
    let targets = scaffold::validate_targets(&targets, known.as_ref())?;

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
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::path::Path;
    use std::rc::Rc;

    use callisto_model::{CommandError, CommandOutput};

    use super::*;

    #[derive(Clone, Default)]
    struct Shared(Rc<RefCell<Vec<u8>>>);

    impl Shared {
        fn text(&self) -> String {
            String::from_utf8(self.0.borrow().clone()).unwrap()
        }
    }

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    enum Answer {
        Select(usize),
        Confirm(bool),
        Input(&'static str),
    }

    /// Answers in order; records each prompt with how much output preceded it.
    struct Scripted {
        answers: VecDeque<Answer>,
        asked: Vec<(String, Option<String>)>,
        output: Shared,
        output_at_first_prompt: Option<String>,
    }

    impl Scripted {
        fn new(answers: Vec<Answer>, output: &Shared) -> Self {
            Scripted {
                answers: answers.into(),
                asked: Vec::new(),
                output: output.clone(),
                output_at_first_prompt: None,
            }
        }

        fn next(&mut self, prompt: &str, default: Option<&str>) -> Answer {
            self.output_at_first_prompt.get_or_insert_with(|| self.output.text());
            self.asked.push((prompt.to_owned(), default.map(str::to_owned)));
            self.answers
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected prompt `{prompt}`"))
        }

        fn prompts(&self) -> Vec<&str> {
            self.asked.iter().map(|(prompt, _)| prompt.as_str()).collect()
        }
    }

    impl Prompter for Scripted {
        fn select(&mut self, prompt: &str, _: &[String]) -> Result<usize, CliError> {
            match self.next(prompt, None) {
                Answer::Select(index) => Ok(index),
                _ => panic!("`{prompt}` is not a select"),
            }
        }
        fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool, CliError> {
            match self.next(prompt, Some(&default.to_string())) {
                Answer::Confirm(answer) => Ok(answer),
                _ => panic!("`{prompt}` is not a confirm"),
            }
        }
        fn input(&mut self, prompt: &str, default: Option<&str>) -> Result<String, CliError> {
            match self.next(prompt, default) {
                Answer::Input(answer) => Ok(answer.to_owned()),
                _ => panic!("`{prompt}` is not an input"),
            }
        }
    }

    fn git(root: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A Cargo workspace: `bins` binary crates (`app`, `tool`) plus library `core`.
    fn workspace(bins: usize) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut members = vec!["core"];
        members.extend(["app", "tool"].into_iter().take(bins));
        let list: Vec<String> = members.iter().map(|member| format!("\"{member}\"")).collect();
        std::fs::write(
            root.join("Cargo.toml"),
            format!("[workspace]\nmembers = [{}]\nresolver = \"2\"\n", list.join(", ")),
        )
        .unwrap();
        for member in members {
            std::fs::create_dir_all(root.join(member).join("src")).unwrap();
            std::fs::write(
                root.join(member).join("Cargo.toml"),
                format!("[package]\nname = \"{member}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n"),
            )
            .unwrap();
            let source = if member == "core" { "lib.rs" } else { "main.rs" };
            std::fs::write(root.join(member).join("src").join(source), "\n").unwrap();
        }
        callisto_fixtures::git::init_repo(root);
        git(
            root,
            &["remote", "add", "origin", "https://github.com/example/tools.git"],
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "fixture"]);
        dir
    }

    fn global(root: &Path, format: OutputFormat, dry_run: bool) -> GlobalArgs {
        GlobalArgs {
            format,
            cwd: root.to_path_buf(),
            dry_run,
        }
    }

    struct Run {
        result: Result<ExitCode, CliError>,
        out: String,
        prompter: Scripted,
    }

    fn run_init(root: &Path, args: InitArgs, interactive: bool, answers: Vec<Answer>, dry_run: bool) -> Run {
        run_init_with_runner(root, args, interactive, answers, dry_run, &CliCommandRunner)
    }

    fn run_init_with_runner<R: CommandRunner>(
        root: &Path,
        args: InitArgs,
        interactive: bool,
        answers: Vec<Answer>,
        dry_run: bool,
        runner: &R,
    ) -> Run {
        let out = Shared::default();
        let err = Shared::default();
        let mut prompter = Scripted::new(answers, &out);
        let result = run(
            args,
            &global(root, OutputFormat::Text, dry_run),
            interactive,
            &mut prompter,
            &mut out.clone(),
            &mut err.clone(),
            runner,
        );
        Run {
            result,
            out: out.text(),
            prompter,
        }
    }

    fn yes(versioning: InitVersioning) -> InitArgs {
        InitArgs {
            yes: true,
            versioning: Some(versioning),
            ..Default::default()
        }
    }

    fn config(root: &Path) -> String {
        std::fs::read_to_string(root.join("callisto.toml")).unwrap()
    }

    fn nothing_written(root: &Path) -> bool {
        !root.join("callisto.toml").exists()
            && !root.join(".changeset").exists()
            && !root.join(".github/workflows/callisto-release.yml").exists()
    }

    // AC-001, AC-002, AC-004, AC-021a: facts precede the only questions: versioning and the write confirm.
    #[test]
    fn interactive_run_without_binaries_asks_versioning_then_confirms() {
        let dir = workspace(0);
        let run = run_init(
            dir.path(),
            InitArgs::default(),
            true,
            vec![Answer::Select(1), Answer::Confirm(false), Answer::Confirm(true)],
            false,
        );
        assert_eq!(run.result.unwrap(), ExitCode::SUCCESS);
        assert_eq!(
            run.prompter.prompts(),
            [VERSIONING_PROMPT, WORKFLOW_PROMPT, WRITE_PROMPT]
        );
        let before = run.prompter.output_at_first_prompt.clone().unwrap();
        for fact in [
            "ecosystems: cargo",
            "origin: https://github.com/example/tools.git",
            "cargo/core 1.0.0 (last tag: none)",
        ] {
            assert!(before.contains(fact), "`{fact}` before the first question:\n{before}");
        }
        assert!(config(dir.path()).contains("[[fixed-group]]\nname = \"all\"\nmembers = [\"cargo/core\"]"));
        assert!(dir.path().join(".changeset/README.md").exists());
        assert!(run.out.contains("Initialized callisto configuration"));
    }

    // AC-002a: a supplied flag pre-answers its question on a TTY.
    #[test]
    fn a_supplied_flag_skips_its_question() {
        let dir = workspace(0);
        let run = run_init(
            dir.path(),
            InitArgs {
                versioning: Some(InitVersioning::Independent),
                ..Default::default()
            },
            true,
            vec![Answer::Confirm(false), Answer::Confirm(true)],
            false,
        );
        run.result.unwrap();
        assert_eq!(run.prompter.prompts(), [WORKFLOW_PROMPT, WRITE_PROMPT]);
        assert_eq!(config(dir.path()), "# callisto configuration\n");
    }

    // AC-003: the ship question appears only with binaries; declining writes no [release].
    #[test]
    fn declining_to_ship_binaries_writes_no_release() {
        let dir = workspace(1);
        let run = run_init(
            dir.path(),
            InitArgs::default(),
            true,
            vec![
                Answer::Select(0),
                Answer::Confirm(false),
                Answer::Confirm(false),
                Answer::Confirm(true),
            ],
            false,
        );
        run.result.unwrap();
        assert_eq!(
            run.prompter.prompts(),
            [VERSIONING_PROMPT, SHIP_PROMPT, WORKFLOW_PROMPT, WRITE_PROMPT]
        );
        assert_eq!(run.prompter.asked[1].1.as_deref(), Some("false"), "defaults to N");
        assert!(!config(dir.path()).contains("[release]"));
    }

    // AC-003a, AC-003b, AC-003c, AC-009: product, pre-filled forge, and targets (empty re-asks).
    #[test]
    fn shipping_binaries_asks_product_forge_and_targets() {
        let dir = workspace(2);
        let run = run_init(
            dir.path(),
            InitArgs::default(),
            true,
            vec![
                Answer::Select(0),
                Answer::Confirm(true),
                Answer::Select(1),
                Answer::Input("example/tools"),
                Answer::Input(" , "),
                Answer::Input("x86_64-unknown-linux-gnu, x86_64-pc-windows-msvc"),
                Answer::Confirm(false),
                Answer::Confirm(true),
            ],
            false,
        );
        run.result.unwrap();
        assert_eq!(
            run.prompter.prompts(),
            [
                VERSIONING_PROMPT,
                SHIP_PROMPT,
                PRODUCT_PROMPT,
                FORGE_PROMPT,
                TARGETS_PROMPT,
                TARGETS_PROMPT,
                WORKFLOW_PROMPT,
                WRITE_PROMPT
            ]
        );
        assert_eq!(run.prompter.asked[3].1.as_deref(), Some("example/tools"));
        let written = config(dir.path());
        assert!(written.contains("[release]\nproduct-package = \"cargo/tool\"\nforge-repository = \"example/tools\""));
        assert!(written.contains("asset-name = \"tool-x86_64-pc-windows-msvc.zip\""));
        assert!(
            run.out.contains("upload tool-x86_64-unknown-linux-gnu.tar.gz"),
            "{}",
            run.out
        );
    }

    // AC-003b: a forge repository other than origin's, or an invalid one, errors up front without writing.
    #[test]
    fn forge_repository_mismatch_and_invalid_error() {
        let dir = workspace(1);
        let run = run_init(
            dir.path(),
            InitArgs {
                yes: true,
                versioning: Some(InitVersioning::Independent),
                artifact_targets: vec!["x86_64-unknown-linux-gnu".to_owned()],
                forge_repository: Some("other/tools".to_owned()),
                ..Default::default()
            },
            false,
            vec![],
            false,
        );
        let error = run.result.unwrap_err();
        assert!(
            matches!(&error, CliError::Graph(GraphError::InitForgeRepositoryMismatch { .. })),
            "{error}"
        );
        assert_eq!(
            error.to_string(),
            "forge repository `other/tools` does not match the origin remote `https://github.com/example/tools.git`"
        );
        assert!(!run.out.contains("First release preview"), "{}", run.out);
        assert!(nothing_written(dir.path()));

        let run = run_init(
            dir.path(),
            InitArgs::default(),
            true,
            vec![Answer::Select(0), Answer::Confirm(true), Answer::Input("no slash")],
            false,
        );
        let error = run.result.unwrap_err();
        assert!(error.to_string().contains("`no slash`"), "{error}");
        assert!(nothing_written(dir.path()));
    }

    // AC-003c: an unrecognized triple errors without writing.
    #[test]
    fn unknown_interactive_target_errors() {
        let dir = workspace(1);
        let run = run_init(
            dir.path(),
            InitArgs::default(),
            true,
            vec![
                Answer::Select(0),
                Answer::Confirm(true),
                Answer::Input("example/tools"),
                Answer::Input("not-a-triple"),
            ],
            false,
        );
        let error = run.result.unwrap_err();
        assert!(error.to_string().contains("`not-a-triple`"), "{error}");
        assert!(nothing_written(dir.path()));
    }

    // AC-006
    #[test]
    fn declining_the_preview_writes_nothing() {
        let dir = workspace(0);
        let run = run_init(
            dir.path(),
            InitArgs::default(),
            true,
            vec![Answer::Select(0), Answer::Confirm(false), Answer::Confirm(false)],
            false,
        );
        assert_eq!(run.result.unwrap(), ExitCode::SUCCESS);
        assert!(run.out.contains("nothing was written"));
        assert!(nothing_written(dir.path()));
    }

    // AC-008
    #[test]
    fn an_existing_config_errors() {
        let dir = workspace(0);
        std::fs::write(dir.path().join("callisto.toml"), "[init]\necosystems = []\n").unwrap();
        let error = run_init(dir.path(), yes(InitVersioning::Fixed), false, vec![], false)
            .result
            .unwrap_err();
        assert!(error.to_string().contains("already initialized"), "{error}");
        assert!(!dir.path().join(".changeset").exists());
    }

    // AC-013
    #[test]
    fn non_tty_without_yes_names_yes_and_missing_flags() {
        let dir = workspace(0);
        let error = run_init(dir.path(), InitArgs::default(), false, vec![], false)
            .result
            .unwrap_err();
        assert!(matches!(&error, CliError::InitRequiresYes { missing } if missing == &["--yes", "--versioning"]));
        assert!(error.to_string().contains("--yes, --versioning"), "{error}");
        assert!(nothing_written(dir.path()));
    }

    // AC-013a, AC-014a
    #[test]
    fn yes_requires_versioning_forge_and_product() {
        let dir = workspace(2);
        let missing = |args| match run_init(dir.path(), args, false, vec![], false).result.unwrap_err() {
            CliError::InitMissingFlags { missing } => missing,
            other => panic!("{other}"),
        };
        assert_eq!(
            missing(InitArgs {
                yes: true,
                ..Default::default()
            }),
            ["--versioning"]
        );
        assert_eq!(
            missing(InitArgs {
                artifact_targets: vec!["x86_64-unknown-linux-gnu".to_owned()],
                ..yes(InitVersioning::Fixed)
            }),
            ["--forge-repository", "--product-package"]
        );
        assert!(nothing_written(dir.path()));
    }

    // AC-014: --yes without --artifact-target opts out of shipping.
    #[test]
    fn yes_without_targets_ships_nothing() {
        let dir = workspace(1);
        run_init(dir.path(), yes(InitVersioning::Independent), false, vec![], false)
            .result
            .unwrap();
        assert!(!config(dir.path()).contains("[release]"));
    }

    fn with_targets(targets: &[&str]) -> InitArgs {
        InitArgs {
            artifact_targets: targets.iter().map(|target| (*target).to_owned()).collect(),
            forge_repository: Some("example/tools".to_owned()),
            ..yes(InitVersioning::Independent)
        }
    }

    // AC-014b, AC-014c, AC-014d
    #[test]
    fn invalid_binary_flags_error_without_writing() {
        let one = workspace(1);
        for (args, expected) in [
            (
                with_targets(&["x86_64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"]),
                "`x86_64-unknown-linux-gnu`",
            ),
            (with_targets(&[""]), "target `` is invalid"),
            (with_targets(&["bogus-triple"]), "`bogus-triple`"),
            (
                InitArgs {
                    product_package: Some("core".to_owned()),
                    ..with_targets(&["x86_64-unknown-linux-gnu"])
                },
                "product package `core` is invalid",
            ),
            (
                InitArgs {
                    product_package: Some("missing".to_owned()),
                    ..with_targets(&["x86_64-unknown-linux-gnu"])
                },
                "product package `missing` is invalid",
            ),
        ] {
            let error = run_init(one.path(), args, false, vec![], false).result.unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
            assert!(nothing_written(one.path()));
        }
        let none = workspace(0);
        let error = run_init(
            none.path(),
            with_targets(&["x86_64-unknown-linux-gnu"]),
            false,
            vec![],
            false,
        )
        .result
        .unwrap_err();
        assert!(
            error.to_string().contains("no package produces a binary artifact"),
            "{error}"
        );
        assert!(nothing_written(none.path()));
    }

    // AC-015: flags produce the interactive run's config and preview, without prompting.
    #[test]
    fn non_interactive_matches_interactive() {
        let interactive_dir = workspace(1);
        let interactive = run_init(
            interactive_dir.path(),
            InitArgs::default(),
            true,
            vec![
                Answer::Select(1),
                Answer::Confirm(true),
                Answer::Input("example/tools"),
                Answer::Input("x86_64-unknown-linux-gnu"),
                Answer::Confirm(false),
                Answer::Confirm(true),
            ],
            false,
        );
        interactive.result.unwrap();
        let flags_dir = workspace(1);
        let flags = run_init(
            flags_dir.path(),
            InitArgs {
                versioning: Some(InitVersioning::Fixed),
                ..with_targets(&["x86_64-unknown-linux-gnu"])
            },
            false,
            vec![],
            false,
        );
        flags.result.unwrap();
        assert!(flags.prompter.asked.is_empty());
        assert_eq!(config(interactive_dir.path()), config(flags_dir.path()));
        let preview = |out: &str| {
            let start = out.find("First release preview").unwrap();
            let end = out.find("Initialized").unwrap();
            let text = out[start..end].to_owned();
            text.replace(interactive_dir.path().to_str().unwrap(), "")
        };
        assert_eq!(preview(&interactive.out), preview(&flags.out));
    }

    // A repository with no commit gets the config and a note instead of a preview.
    #[test]
    fn no_commits_skips_the_preview_and_still_writes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        callisto_fixtures::git::init_repo(root);
        git(root, &["remote", "add", "origin", "https://github.com/example/app.git"]);
        let run = run_init(root, yes(InitVersioning::Independent), false, vec![], false);
        assert_eq!(run.result.unwrap(), ExitCode::SUCCESS);
        assert!(run.out.contains(NO_COMMITS), "{}", run.out);
        assert!(!run.out.contains("First release preview"), "{}", run.out);
        assert_eq!(config(root), "# callisto configuration\n");
        assert!(root.join(".changeset/README.md").exists());
    }

    // AC-021: --dry-run detects, asks, and previews, then writes nothing.
    #[test]
    fn dry_run_previews_and_writes_nothing() {
        let dir = workspace(0);
        let run = run_init(
            dir.path(),
            InitArgs::default(),
            true,
            vec![Answer::Select(0), Answer::Confirm(false)],
            true,
        );
        run.result.unwrap();
        assert_eq!(run.prompter.prompts(), [VERSIONING_PROMPT, WORKFLOW_PROMPT]);
        assert!(run.out.contains("Release plan:"), "{}", run.out);
        assert!(run.out.contains("[DRY-RUN]"));
        assert!(nothing_written(dir.path()));
    }

    // JSON keeps stdout for the report; facts and preview go to stderr.
    #[test]
    fn json_reports_on_stdout() {
        let dir = workspace(0);
        let out = Shared::default();
        let err = Shared::default();
        run(
            yes(InitVersioning::Independent),
            &global(dir.path(), OutputFormat::Json, false),
            false,
            &mut Scripted::new(vec![], &out),
            &mut out.clone(),
            &mut err.clone(),
            &CliCommandRunner,
        )
        .unwrap();
        let report: serde_json::Value = serde_json::from_str(&out.text()).unwrap();
        assert_eq!(report["initialized"], true);
        assert_eq!(report["config"], "# callisto configuration\n");
        assert!(err.text().contains("Detected:"));
    }

    fn workflow_file(root: &Path) -> String {
        std::fs::read_to_string(root.join(".github/workflows/callisto-release.yml")).unwrap()
    }

    /// Delegates every call to the real `CliCommandRunner`, except `git
    /// ls-remote` (used only to resolve `callisto@<version>`'s commit),
    /// answered from canned output -- keeps these tests network-free without
    /// faking the many other git calls `run()` legitimately makes against
    /// each test's own temp repository.
    struct FakeLsRemote {
        stdout: &'static str,
    }

    impl CommandRunner for FakeLsRemote {
        fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
            CliCommandRunner.run(program, args, cwd)
        }
        fn run_with_timeout(
            &self,
            program: &str,
            args: &[&str],
            cwd: &Path,
            timeout: std::time::Duration,
        ) -> Result<CommandOutput, CommandError> {
            CliCommandRunner.run_with_timeout(program, args, cwd, timeout)
        }
        fn run_quiet(
            &self,
            program: &str,
            args: &[&str],
            cwd: &Path,
            timeout: std::time::Duration,
        ) -> Result<CommandOutput, CommandError> {
            if program == "git" && args.first() == Some(&"ls-remote") {
                return Ok(CommandOutput {
                    exit_code: Some(0),
                    stdout: self.stdout.to_owned(),
                    stderr: String::new(),
                });
            }
            CliCommandRunner.run_quiet(program, args, cwd, timeout)
        }
        fn run_with_stdin(
            &self,
            program: &str,
            args: &[&str],
            cwd: &Path,
            stdin: &[u8],
        ) -> Result<CommandOutput, CommandError> {
            CliCommandRunner.run_with_stdin(program, args, cwd, stdin)
        }
    }

    const FAKE_COMMIT: &str = "cccccccccccccccccccccccccccccccccccccccc";

    fn fake_ls_remote(version: &str) -> FakeLsRemote {
        FakeLsRemote {
            stdout: Box::leak(
                format!(
                    "{FAKE_COMMIT}\trefs/tags/callisto@{version}\n{FAKE_COMMIT}\trefs/tags/callisto@{version}^{{}}\n"
                )
                .into_boxed_str(),
            ),
        }
    }

    // AC-001, AC-002: answering yes to the interactive question writes the generated workflow.
    #[test]
    fn interactive_workflow_confirm_writes_the_generated_file() {
        let dir = workspace(0);
        let run = run_init_with_runner(
            dir.path(),
            InitArgs::default(),
            true,
            vec![Answer::Select(0), Answer::Confirm(true), Answer::Confirm(true)],
            false,
            &fake_ls_remote(env!("CARGO_PKG_VERSION")),
        );
        run.result.unwrap();
        assert_eq!(
            run.prompter.prompts(),
            [VERSIONING_PROMPT, WORKFLOW_PROMPT, WRITE_PROMPT]
        );
        assert_eq!(run.prompter.asked[1].1.as_deref(), Some("false"), "defaults to N");
        let workflow = workflow_file(dir.path());
        assert!(
            workflow.contains(&format!(
                "uses: orin-dx/callisto/.github/actions/callisto-action@{FAKE_COMMIT} # callisto@{}",
                env!("CARGO_PKG_VERSION")
            )),
            "{workflow}"
        );
        assert!(workflow.contains("with: {mode: version-pr}"));
        assert!(workflow.contains("with: {mode: release}"));
        assert!(
            run.out.contains(".github/workflows/callisto-release.yml:"),
            "{}",
            run.out
        );
    }

    // AC-001: `--workflow` generates without asking, even non-interactively.
    #[test]
    fn workflow_flag_generates_without_asking() {
        let dir = workspace(0);
        let args = InitArgs {
            workflow: true,
            ..yes(InitVersioning::Independent)
        };
        run_init_with_runner(
            dir.path(),
            args,
            false,
            vec![],
            false,
            &fake_ls_remote(env!("CARGO_PKG_VERSION")),
        )
        .result
        .unwrap();
        assert!(dir.path().join(".github/workflows/callisto-release.yml").exists());
    }

    // Writing a workflow prints that a merge to the default branch publishes, and records it
    // as an info diagnostic; a dry run writes nothing and says nothing.
    #[test]
    fn written_workflow_notes_that_merging_publishes() {
        let note =
            "Merging to `main` publishes: require pull requests and reviews on `main` (GitHub → Settings → Rules).";
        let args = || InitArgs {
            workflow: true,
            ..yes(InitVersioning::Independent)
        };
        let text = workspace(0);
        let run = run_init_with_runner(
            text.path(),
            args(),
            false,
            vec![],
            false,
            &fake_ls_remote(env!("CARGO_PKG_VERSION")),
        );
        run.result.unwrap();
        assert!(run.out.contains(&format!("{note}\n")), "{}", run.out);

        let dir = workspace(0);
        let out = Shared::default();
        let err = Shared::default();
        run_with_json(dir.path(), args(), false, &out, &err);
        assert!(err.text().contains(note), "{}", err.text());
        let report: serde_json::Value = serde_json::from_str(&out.text()).unwrap();
        let diagnostics = report["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics.len(), 1, "{report}");
        assert_eq!(diagnostics[0]["code"], "workflow-merge-publishes");
        assert_eq!(diagnostics[0]["severity"], "info");
        assert_eq!(diagnostics[0]["message"], note);

        let dry = workspace(0);
        let out = Shared::default();
        let err = Shared::default();
        run_with_json(dry.path(), args(), true, &out, &err);
        assert!(!err.text().contains("Merging to"), "{}", err.text());
        let report: serde_json::Value = serde_json::from_str(&out.text()).unwrap();
        assert!(
            report
                .get("diagnostics")
                .is_none_or(|d| d.as_array().unwrap().is_empty()),
            "{report}"
        );
    }

    fn run_with_json(root: &Path, args: InitArgs, dry_run: bool, out: &Shared, err: &Shared) {
        run(
            args,
            &global(root, OutputFormat::Json, dry_run),
            false,
            &mut Scripted::new(vec![], out),
            &mut out.clone(),
            &mut err.clone(),
            &fake_ls_remote(env!("CARGO_PKG_VERSION")),
        )
        .unwrap();
    }

    // AC-004: an unresolvable tag (offline, or an unreleased version) errors clearly, naming the fix.
    #[test]
    fn unresolvable_version_errors_naming_the_fix() {
        let dir = workspace(0);
        let empty = FakeLsRemote { stdout: "" };
        let args = InitArgs {
            workflow: true,
            ..yes(InitVersioning::Independent)
        };
        let run = run_init_with_runner(dir.path(), args, false, vec![], false, &empty);
        let error = run.result.unwrap_err();
        assert!(
            matches!(
                &error,
                CliError::Graph(GraphError::InitWorkflowVersionUnresolved { .. })
            ),
            "{error}"
        );
        assert!(error.to_string().contains("couldn't resolve"), "{error}");
        assert!(error.to_string().contains(env!("CARGO_PKG_VERSION")), "{error}");
        assert!(!dir.path().join(".github/workflows/callisto-release.yml").exists());
    }

    // AC-001a: declining interactively, `--no-workflow`, and `--yes` with neither flag all write nothing.
    #[test]
    fn workflow_not_requested_writes_nothing() {
        let declined = workspace(0);
        let run = run_init(
            declined.path(),
            InitArgs::default(),
            true,
            vec![Answer::Select(0), Answer::Confirm(false), Answer::Confirm(true)],
            false,
        );
        run.result.unwrap();
        assert!(!declined.path().join(".github/workflows/callisto-release.yml").exists());

        let no_flag = workspace(0);
        let args = InitArgs {
            no_workflow: true,
            ..yes(InitVersioning::Independent)
        };
        let run = run_init(no_flag.path(), args, false, vec![], false);
        run.result.unwrap();
        assert!(run.prompter.prompts().is_empty());
        assert!(!no_flag.path().join(".github/workflows/callisto-release.yml").exists());

        let default_yes = workspace(0);
        run_init(
            default_yes.path(),
            yes(InitVersioning::Independent),
            false,
            vec![],
            false,
        )
        .result
        .unwrap();
        assert!(!default_yes
            .path()
            .join(".github/workflows/callisto-release.yml")
            .exists());
    }

    // AC-002a: an existing workflow file errors before the preview, in both normal and --dry-run runs.
    #[test]
    fn existing_workflow_file_errors_before_the_preview_and_writes_nothing() {
        for dry_run in [false, true] {
            let dir = workspace(0);
            std::fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
            std::fs::write(dir.path().join(".github/workflows/callisto-release.yml"), "mine\n").unwrap();
            let args = InitArgs {
                workflow: true,
                ..yes(InitVersioning::Independent)
            };
            let run = run_init(dir.path(), args, false, vec![], dry_run);
            let error = run.result.unwrap_err();
            assert!(
                matches!(&error, CliError::Graph(GraphError::InitWorkflowExists { .. })),
                "{error}"
            );
            assert!(error.to_string().contains("release.yml"), "{error}");
            assert!(!run.out.contains("First release preview"), "{}", run.out);
            assert!(!run.out.contains("callisto.toml:"), "{}", run.out);
            assert_eq!(
                std::fs::read_to_string(dir.path().join(".github/workflows/callisto-release.yml")).unwrap(),
                "mine\n"
            );
            assert!(!dir.path().join("callisto.toml").exists());
        }
    }

    // AC-002b: the two flags are mutually exclusive; the question is never asked.
    #[test]
    fn workflow_and_no_workflow_together_error_without_asking() {
        let dir = workspace(0);
        let args = InitArgs {
            workflow: true,
            no_workflow: true,
            ..InitArgs::default()
        };
        let run = run_init(dir.path(), args, true, vec![], false);
        let error = run.result.unwrap_err();
        assert!(matches!(&error, CliError::InitWorkflowFlagsConflict), "{error}");
        assert!(run.prompter.prompts().is_empty());
        assert!(nothing_written(dir.path()));
    }

    /// An npm package with an attached platform package but no `napi.targets`.
    fn platform_workspace_without_napi_targets() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (path, content) in [
            (
                "package.json",
                r#"{"name":"root","version":"0.0.0","private":true,"workspaces":["web","web-linux-x64-gnu"]}"#,
            ),
            (
                "web/package.json",
                r#"{"name":"web","version":"1.0.0","optionalDependencies":{"web-linux-x64-gnu":"1.0.0"}}"#,
            ),
            (
                "web-linux-x64-gnu/package.json",
                r#"{"name":"web-linux-x64-gnu","version":"1.0.0","os":["linux"],"cpu":["x64"]}"#,
            ),
        ] {
            std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
            std::fs::write(root.join(path), content).unwrap();
        }
        callisto_fixtures::git::init_repo(root);
        git(
            root,
            &["remote", "add", "origin", "https://github.com/example/tools.git"],
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "fixture"]);
        dir
    }

    // SPEC-DX-SETUP-WORKFLOW-MATRIX: shipping binaries generates plan -> build -> execute.
    #[test]
    fn shipping_binaries_generates_the_build_matrix_workflow() {
        let dir = workspace(1);
        let args = InitArgs {
            workflow: true,
            ..with_targets(&["x86_64-unknown-linux-gnu"])
        };
        run_init_with_runner(
            dir.path(),
            args,
            false,
            vec![],
            false,
            &fake_ls_remote(env!("CARGO_PKG_VERSION")),
        )
        .result
        .unwrap();
        let workflow = workflow_file(dir.path());
        for expected in [
            "callisto release plan --from-release-commit",
            "include: ${{ fromJSON(needs.plan.outputs.matrix) }}",
            "callisto release execute",
            "with: {mode: version-pr}",
        ] {
            assert!(workflow.contains(expected), "`{expected}` missing:\n{workflow}");
        }
    }

    // A shape no generated workflow covers skips the question with a note that
    // also lands in InitReport.diagnostics for a --format json caller.
    #[test]
    fn unsupported_workspace_skips_workflow_generation_with_a_json_diagnostic() {
        let dir = platform_workspace_without_napi_targets();
        let out = Shared::default();
        let err = Shared::default();
        run(
            InitArgs::default(),
            &global(dir.path(), OutputFormat::Json, false),
            true,
            &mut Scripted::new(vec![Answer::Select(0), Answer::Confirm(true)], &out),
            &mut out.clone(),
            &mut err.clone(),
            &CliCommandRunner,
        )
        .unwrap();
        assert!(
            err.text().contains("Skipping GitHub Actions workflow generation"),
            "{}",
            err.text()
        );
        let report: serde_json::Value = serde_json::from_str(&out.text()).unwrap();
        let diagnostics = report["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics.len(), 1, "{report}");
        assert_eq!(diagnostics[0]["code"], "workflow-generation-unsupported");
        assert_eq!(diagnostics[0]["severity"], "warning");
        assert!(
            diagnostics[0]["message"].as_str().unwrap().contains("napi.targets"),
            "{report}"
        );
        assert!(!dir.path().join(".github/workflows/callisto-release.yml").exists());
    }

    // Passing --workflow for a shape no generated workflow covers errors, naming why.
    #[test]
    fn explicit_workflow_flag_errors_for_an_unsupported_workspace() {
        let dir = platform_workspace_without_napi_targets();
        let args = InitArgs {
            workflow: true,
            ..yes(InitVersioning::Independent)
        };
        let run = run_init(dir.path(), args, false, vec![], false);
        let error = run.result.unwrap_err();
        assert!(
            matches!(&error, CliError::Graph(GraphError::InitWorkflowUnsupported { .. })),
            "{error}"
        );
        assert!(error.to_string().contains("napi.targets"), "{error}");
        assert!(nothing_written(dir.path()));
    }
}
