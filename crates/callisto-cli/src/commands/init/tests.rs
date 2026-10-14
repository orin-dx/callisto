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

/// A Cargo workspace: `bins` binary crates (`app`, `tool`) plus library `core`, with `origin`.
fn workspace(bins: usize) -> tempfile::TempDir {
    workspace_with_origin(bins, true)
}

/// Same workspace shape as [`workspace`], with no `origin` remote configured.
fn workspace_without_origin(bins: usize) -> tempfile::TempDir {
    workspace_with_origin(bins, false)
}

fn workspace_with_origin(bins: usize, origin: bool) -> tempfile::TempDir {
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
    if origin {
        git(
            root,
            &["remote", "add", "origin", "https://github.com/example/tools.git"],
        );
    }
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

// Facts precede the only questions: versioning and the write confirm.
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

// A supplied flag pre-answers its question on a TTY.
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

// The ship question appears only with binaries; declining writes no [release].
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

// Product, pre-filled forge, and targets (empty re-asks).
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

// A comma-separated answer with an empty entry errors naming it, rather than
// silently dropping it, and re-asking only kicks in when the whole answer is blank.
#[test]
fn interactive_target_with_empty_entry_errors_without_writing() {
    let dir = workspace(1);
    let run = run_init(
        dir.path(),
        InitArgs::default(),
        true,
        vec![
            Answer::Select(0),
            Answer::Confirm(true),
            Answer::Input("example/tools"),
            Answer::Input("x86_64-unknown-linux-gnu,,x86_64-pc-windows-msvc"),
        ],
        false,
    );
    let error = run.result.unwrap_err();
    assert!(error.to_string().contains("target `` is invalid"), "{error}");
    assert!(nothing_written(dir.path()));
}

// A forge repository other than origin's, or an invalid one, errors up front without writing.
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

// A repeated triple errors without writing.
#[test]
fn repeated_interactive_target_errors() {
    let dir = workspace(1);
    let run = run_init(
        dir.path(),
        InitArgs::default(),
        true,
        vec![
            Answer::Select(0),
            Answer::Confirm(true),
            Answer::Input("example/tools"),
            Answer::Input("x86_64-unknown-linux-gnu, x86_64-unknown-linux-gnu"),
        ],
        false,
    );
    let error = run.result.unwrap_err();
    assert!(error.to_string().contains("`x86_64-unknown-linux-gnu`"), "{error}");
    assert!(nothing_written(dir.path()));
}

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

// --yes without --artifact-target opts out of shipping.
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

#[test]
fn invalid_binary_flags_error_without_writing() {
    let one = workspace(1);
    for (args, expected) in [
        (
            with_targets(&["x86_64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"]),
            "`x86_64-unknown-linux-gnu`",
        ),
        (with_targets(&[""]), "target `` is invalid"),
        (
            with_targets(&["x86_64-unknown-linux-gnu", "", "x86_64-pc-windows-msvc"]),
            "target `` is invalid",
        ),
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

#[test]
fn unsupported_registry_executable_artifact_fails_before_writing() {
    let dir = workspace(0);
    std::fs::create_dir_all(dir.path().join("tool")).unwrap();
    std::fs::write(
        dir.path().join("tool/package.json"),
        r#"{"name":"tool","version":"1.0.0","bin":{"tool":"cli.js"}}"#,
    )
    .unwrap();
    let args = InitArgs {
        product_package: Some("npm/tool".to_owned()),
        ..with_targets(&["x86_64-unknown-linux-gnu"])
    };
    let error = run_init(dir.path(), args, false, vec![], false).result.unwrap_err();
    assert!(
        error.to_string().contains("npm/tool") && error.to_string().contains("npm"),
        "{error}"
    );
    assert!(nothing_written(dir.path()));
}

#[test]
fn registry_executable_can_be_initialized_without_artifact_generation() {
    let dir = workspace(0);
    std::fs::create_dir_all(dir.path().join("tool")).unwrap();
    std::fs::write(
        dir.path().join("tool/package.json"),
        r#"{"name":"tool","version":"1.0.0","bin":{"tool":"cli.js"}}"#,
    )
    .unwrap();
    let run = run_init(dir.path(), yes(InitVersioning::Independent), false, vec![], false);
    assert!(run.result.is_ok(), "{}", run.out);
    assert!(!config(dir.path()).contains("[release]"));
}

// --forge-repository/--product-package are validated up front, before facts are even
// detected: dropped silently without --artifact-target is the bug this rejects.
#[test]
fn forge_or_product_without_artifact_target_errors_up_front() {
    let dir = workspace(2);
    for args in [
        InitArgs {
            forge_repository: Some("example/tools".to_owned()),
            ..yes(InitVersioning::Independent)
        },
        InitArgs {
            product_package: Some("app".to_owned()),
            ..yes(InitVersioning::Independent)
        },
    ] {
        let error = run_init(dir.path(), args, false, vec![], false).result.unwrap_err();
        assert!(
            matches!(&error, CliError::InitFlagRequiresArtifactTarget { .. }),
            "{error}"
        );
        assert!(
            error.to_string().contains("only applies with --artifact-target"),
            "{error}"
        );
        assert!(nothing_written(dir.path()));
    }
}

// No `origin`: init still writes the config, with a warning diagnostic, and skips the
// shipping-binaries question entirely (even with binary packages present).
#[test]
fn missing_origin_writes_config_with_warning_and_skips_shipping() {
    let dir = workspace_without_origin(1);
    let out = Shared::default();
    let err = Shared::default();
    run_with_json(dir.path(), yes(InitVersioning::Independent), false, &out, &err);
    assert!(
        err.text().contains("No `origin` remote is configured"),
        "{}",
        err.text()
    );
    let report: serde_json::Value = serde_json::from_str(&out.text()).unwrap();
    assert_eq!(report["initialized"], true);
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|d| d["code"] == "init-origin-missing" && d["severity"] == "warning"),
        "{report}"
    );
    assert!(dir.path().join("callisto.toml").exists());

    // Interactively, with a binary package present, the ship question is never asked.
    let interactive_dir = workspace_without_origin(1);
    let run = run_init(
        interactive_dir.path(),
        InitArgs::default(),
        true,
        vec![Answer::Select(0), Answer::Confirm(true)],
        false,
    );
    run.result.unwrap();
    assert_eq!(run.prompter.prompts(), [VERSIONING_PROMPT, WRITE_PROMPT]);
    assert!(!config(interactive_dir.path()).contains("[release]"));
}

// `--workflow` and `--artifact-target` each require `origin`; both error up front, naming it.
#[test]
fn workflow_or_artifact_target_without_origin_errors() {
    let workflow_dir = workspace_without_origin(0);
    let args = InitArgs {
        workflow: true,
        ..yes(InitVersioning::Independent)
    };
    let error = run_init(workflow_dir.path(), args, false, vec![], false)
        .result
        .unwrap_err();
    assert!(
        matches!(&error, CliError::InitFlagRequiresOrigin { flag } if *flag == "workflow"),
        "{error}"
    );
    assert!(nothing_written(workflow_dir.path()));

    let target_dir = workspace_without_origin(1);
    let args = with_targets(&["x86_64-unknown-linux-gnu"]);
    let error = run_init(target_dir.path(), args, false, vec![], false)
        .result
        .unwrap_err();
    assert!(
        matches!(&error, CliError::InitFlagRequiresOrigin { flag } if *flag == "artifact-target"),
        "{error}"
    );
    assert!(nothing_written(target_dir.path()));
}

// Flags produce the interactive run's config and preview, without prompting.
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

// --dry-run detects, asks, and previews, then writes nothing.
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

// --dry-run --workflow lists every file it would write: config, changeset README and workflow.
#[test]
fn dry_run_with_workflow_lists_every_file() {
    let dir = workspace(0);
    let args = InitArgs {
        workflow: true,
        ..yes(InitVersioning::Independent)
    };
    let out = Shared::default();
    let err = Shared::default();
    run(
        args,
        &global(dir.path(), OutputFormat::Json, true),
        false,
        &mut Scripted::new(vec![], &out),
        &mut out.clone(),
        &mut err.clone(),
        &fake_ls_remote(env!("CARGO_PKG_VERSION")),
    )
    .unwrap();
    let report: serde_json::Value = serde_json::from_str(&out.text()).unwrap();
    let files: Vec<&str> = report["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file.as_str().unwrap())
        .collect();
    assert!(files.iter().any(|file| file.ends_with("callisto.toml")), "{files:?}");
    assert!(
        files.iter().any(|file| file.ends_with(".changeset/README.md")),
        "{files:?}"
    );
    assert!(
        files.iter().any(|file| file.ends_with("callisto-release.yml")),
        "{files:?}"
    );
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
            format!("{FAKE_COMMIT}\trefs/tags/callisto@{version}\n{FAKE_COMMIT}\trefs/tags/callisto@{version}^{{}}\n")
                .into_boxed_str(),
        ),
    }
}

// Answering yes to the interactive question writes the generated workflow.
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
    assert!(workflow.contains("with: {mode: version-pr, branch: main}"));
    assert!(workflow.contains("with: {mode: release}"));
    assert!(
        run.out.contains(".github/workflows/callisto-release.yml:"),
        "{}",
        run.out
    );
}

// `--workflow` generates without asking, even non-interactively.
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
    let note = "Merging to `main` publishes: require pull requests and reviews on `main` (GitHub → Settings → Rules).";
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

// A pre-existing `.changeset/config.json` warns per unhonoured key it declares, naming
// `fixed`'s callisto.toml replacement; init still writes normally alongside the warning.
#[test]
fn preexisting_changesets_config_json_warns_per_unhonoured_key() {
    let dir = workspace(0);
    std::fs::create_dir_all(dir.path().join(".changeset")).unwrap();
    std::fs::write(
        dir.path().join(".changeset/config.json"),
        r#"{"fixed": [["core"]], "linked": [["core"]]}"#,
    )
    .unwrap();

    let out = Shared::default();
    let err = Shared::default();
    run_with_json(dir.path(), yes(InitVersioning::Independent), false, &out, &err);
    let report: serde_json::Value = serde_json::from_str(&out.text()).unwrap();
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 2, "{report}");
    assert!(diagnostics
        .iter()
        .all(|d| d["code"] == "changesets-config-key-dropped" && d["severity"] == "warning"));
    let fixed = diagnostics
        .iter()
        .find(|d| d["message"].as_str().unwrap().contains("`fixed`"))
        .unwrap();
    assert!(
        fixed["message"].as_str().unwrap().contains("[[fixed-group]]"),
        "{fixed}"
    );
    let linked = diagnostics
        .iter()
        .find(|d| d["message"].as_str().unwrap().contains("`linked`"))
        .unwrap();
    assert!(
        linked["message"].as_str().unwrap().contains("no equivalent"),
        "{linked}"
    );
    assert!(dir.path().join("callisto.toml").exists());
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

// An unresolvable tag (offline, or an unreleased version) errors clearly, naming the fix.
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

// Declining interactively, `--no-workflow`, and `--yes` with neither flag all write nothing.
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

// An existing workflow file errors before the preview, in both normal and --dry-run runs.
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

// The two flags are mutually exclusive; the question is never asked.
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

// Shipping binaries generates plan -> build -> execute.
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
        "with: {mode: version-pr, branch: main}",
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
