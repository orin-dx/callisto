//! Every command's `--format json` success output carries a numeric `schemaVersion`; reports also carry their
//! documented `command` and a boolean `dryRun`. A command that cannot run offline here is listed with a reason, so a
//! new command forces a decision.
#[path = "common/release_harness.rs"]
mod release_harness;

use std::path::Path;
use std::process::Output;

use clap::CommandFactory;
use release_harness::*;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

/// Commands without a JSON success report in this test, each with why.
const EXEMPT: &[(&str, &str)] = &[
    ("completions", "prints a shell completion script, not JSON"),
    ("help", "prints help text, not JSON"),
    (
        "schema",
        "prints a JSON Schema document; it has no schemaVersion of its own and `--format` does not apply",
    ),
    (
        "release build-python",
        "needs a Python build toolchain and a Python package; its envelope is pinned by a unit test in callisto-model",
    ),
];

/// What a successful run must print.
enum Shape {
    /// The standard envelope: `schemaVersion`, this `command`, boolean `dryRun`.
    Report(&'static str),
    /// A durable document that carries its own `schemaVersion` and no envelope.
    Document,
}

struct Case {
    /// The command path as the documentation names it.
    label: &'static str,
    shape: Shape,
    output: Output,
}

fn case(label: &'static str, shape: Shape, output: Output) -> Case {
    Case { label, shape, output }
}

fn report(label: &'static str, output: Output) -> Case {
    case(label, Shape::Report(label), output)
}

fn problem(case: &Case) -> Option<String> {
    let out = &case.output;
    if !out.status.success() {
        return Some(format!(
            "exited {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let value: serde_json::Value = match serde_json::from_slice(&out.stdout) {
        Ok(value) => value,
        Err(error) => {
            return Some(format!(
                "stdout is not JSON ({error}): {}",
                String::from_utf8_lossy(&out.stdout)
            ))
        }
    };
    if !value["schemaVersion"].is_u64() {
        return Some(format!("no numeric top-level schemaVersion: {value}"));
    }
    if let Shape::Report(command) = case.shape {
        if value["command"] != command {
            return Some(format!("command is {}, documented as {command:?}", value["command"]));
        }
        if !value["dryRun"].is_boolean() {
            return Some(format!("dryRun is not a boolean: {}", value["dryRun"]));
        }
    }
    None
}

fn stdout_of(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

/// A release-commit fixture driven through the durable release path with fake publishers.
fn durable_release_cases(cases: &mut Vec<Case>) {
    let (dir, release_commit) = product_release_commit_fixture();
    let root = dir.path();
    let external = tempfile::tempdir().unwrap();
    let intent = external.path().join("release-intent.json");
    let manifest = external.path().join("artifact-manifest.json");
    let receipt = external.path().join("release-receipt.json");
    let artifacts = create_product_artifacts(external.path());
    let path = |p: &Path| p.to_str().unwrap().to_owned();

    cases.push(report(
        "release plan",
        callisto(
            root,
            &[
                "release",
                "plan",
                "--from-release-commit",
                &release_commit,
                "--decision",
                DECISION_PATH,
                "--orchestration-revision",
                &release_commit,
                "--artifact-repository",
                "example/core-crate",
                "--out",
                &path(&intent),
            ],
        ),
    ));
    cases.push(case(
        "release inspect",
        Shape::Document,
        callisto(root, &["release", "inspect", "--input", &path(&intent)]),
    ));
    cases.push(report(
        "release artifact-manifest",
        callisto(
            root,
            &[
                "release",
                "artifact-manifest",
                "--intent",
                &path(&intent),
                "--artifact-dir",
                &path(&artifacts),
                "--out",
                &path(&manifest),
            ],
        ),
    ));
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);
    cases.push(report(
        "release execute",
        execute_product(
            root,
            &intent,
            &manifest,
            &artifacts,
            &receipt,
            FakePublishers {
                bin: &bin,
                log: &log,
                forge_marker: &forge_marker,
                git_trace: &git_trace,
            },
        ),
    ));
}

/// A fresh workspace with pending work, taken through `init` and `add` like a user would.
fn workspace_cases(cases: &mut Vec<Case>) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    git(
        root,
        &["remote", "add", "origin", "https://github.com/example/widget.git"],
    );
    // The URL must look like GitHub, but the test must not reach it: an unreachable local proxy fails every request.
    git(root, &["config", "http.proxy", "http://127.0.0.1:9"]);
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(root.join(".gitignore"), "target\n").unwrap();
    std::fs::create_dir_all(root.join("crates/widget/src")).unwrap();
    std::fs::write(
        root.join("crates/widget/Cargo.toml"),
        "[package]\nname = \"widget\"\nversion = \"0.1.0\"\nedition = \"2021\"\ndescription = \"t\"\nlicense = \"MIT\"\n",
    )
    .unwrap();
    std::fs::write(root.join("crates/widget/src/lib.rs"), "pub fn f() {}\n").unwrap();

    cases.push(report("init", callisto(root, &["init", "--yes"])));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);
    let head = git(root, &["rev-parse", "HEAD"]);

    cases.push(report("matrix", callisto(root, &["matrix"])));
    cases.push(report("compose-pr-body", callisto(root, &["compose-pr-body"])));
    cases.push(report("release pending", callisto(root, &["release", "pending"])));
    cases.push(report("release", callisto(root, &["--dry-run", "release"])));
    cases.push(report(
        "add",
        callisto(
            root,
            &["--dry-run", "add", "--package", "cargo/widget:minor", "--summary", "s"],
        ),
    ));
    cases.push(report(
        "add",
        callisto(root, &["add", "--package", "cargo/widget:minor", "--summary", "s"]),
    ));
    cases.push(report("status", callisto(root, &["status"])));
    cases.push(report("version", callisto(root, &["--dry-run", "version"])));
    cases.push(report(
        "snapshot",
        callisto(root, &["--dry-run", "snapshot", "--tag", "t"]),
    ));
    cases.push(case(
        "pre enter",
        Shape::Report("pre"),
        callisto(root, &["--dry-run", "pre", "enter", "beta"]),
    ));
    cases.push(report("release", callisto(root, &["--dry-run", "release"])));

    std::fs::write(root.join("staged.txt"), "staged\n").unwrap();
    git(root, &["add", "staged.txt"]);
    cases.push(report(
        "release-pr commit-plan",
        callisto(
            root,
            &["release-pr", "commit-plan", "--base-commit", &head, "--message", "m"],
        ),
    ));

    let snapshot = callisto_model::ReleasePrSnapshotV2::new(
        callisto_model::GitHubRepository::parse("example/widget").unwrap(),
        "main".into(),
        callisto_model::CommitSha::parse(SHA).unwrap(),
        vec![],
    )
    .unwrap();
    let snapshot = serde_json::to_string(&snapshot).unwrap();
    let decided = callisto(
        root,
        &[
            "release-pr",
            "decide",
            "--snapshot",
            &snapshot,
            "--repository",
            "example/widget",
            "--base-branch",
            "main",
            "--release-branch",
            "callisto/version-packages",
        ],
    );
    let decision = String::from_utf8(decided.stdout.clone()).unwrap();
    cases.push(report("release-pr decide", decided));
    cases.push(report(
        "release-pr verify",
        callisto(
            root,
            &["release-pr", "verify", "--decision", &decision, "--snapshot", &snapshot],
        ),
    ));

    let entered = callisto(root, &["pre", "enter", "beta"]);
    cases.push(case("pre enter", Shape::Report("pre"), entered));
    cases.push(case("pre exit", Shape::Report("pre"), callisto(root, &["pre", "exit"])));
}

/// Every command clap knows that is not a pure group of subcommands, as the space-joined path.
fn command_paths() -> Vec<String> {
    fn walk(command: &clap::Command, prefix: &str, paths: &mut Vec<String>) {
        for sub in command.get_subcommands() {
            let path = if prefix.is_empty() {
                sub.get_name().to_owned()
            } else {
                format!("{prefix} {}", sub.get_name())
            };
            if !sub.is_subcommand_required_set() || sub.get_subcommands().next().is_none() {
                paths.push(path.clone());
            }
            walk(sub, &path, paths);
        }
    }
    let mut paths = Vec::new();
    let mut root = callisto_cli::Cli::command();
    root.build();
    walk(&root, "", &mut paths);
    paths
}

#[test]
fn every_json_success_output_carries_a_schema_version() {
    let mut cases = Vec::new();
    workspace_cases(&mut cases);
    durable_release_cases(&mut cases);

    let failures: Vec<String> = cases
        .iter()
        .filter_map(|case| problem(case).map(|why| format!("`callisto {}`: {why}", case.label)))
        .collect();
    assert!(failures.is_empty(), "JSON contract broken:\n{}", failures.join("\n"));

    let planned = cases
        .iter()
        .filter(|case| case.label == "release")
        .any(|case| stdout_of(&case.output).get("nothingToRelease").is_none());
    assert!(
        planned,
        "no `release --dry-run` run reported a plan, so that report shape is untested"
    );

    let known = command_paths();
    for path in &known {
        let covered = cases.iter().any(|case| case.label == path);
        let exempt = EXEMPT.iter().any(|(name, _)| name == path);
        assert!(
            covered != exempt,
            "`callisto {path}` must be in exactly one of the table or EXEMPT (covered={covered}, exempt={exempt})"
        );
    }
    for label in cases
        .iter()
        .map(|case| case.label)
        .chain(EXEMPT.iter().map(|(name, _)| *name))
    {
        assert!(known.iter().any(|path| path == label), "`{label}` is not a command");
    }
}
