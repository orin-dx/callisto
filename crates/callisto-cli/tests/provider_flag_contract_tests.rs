#![cfg(unix)]

//! The provider contract tier: the release path's external commands are held
//! to the real tools, and the fakes are held to the captured provider bytes.
//!
//! A flag the code emits must appear in the real tool's own help; a tool that
//! is not installed skips loudly (and fails under `CI` for gh, git, npm, cargo).

#[path = "common/release_harness.rs"]
mod release_harness;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use release_harness::*;
use serde_json::Value;

fn run(program: &str, args: &[&str]) -> Option<(bool, String)> {
    let output = Command::new(program).args(args).output().ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Some((output.status.success(), text))
}

fn tool_available(tool: &str) -> bool {
    if run(tool, &["--version"]).is_some_and(|(ok, _)| ok) {
        return true;
    }
    let required = matches!(tool, "gh" | "git" | "npm" | "cargo")
        && std::env::var_os("CI").is_some()
        && std::env::var_os("CALLISTO_ALLOW_MISSING_TOOLS").is_none();
    assert!(
        !required,
        "`{tool}` must be installed under CI to check its flag contract"
    );
    eprintln!("SKIPPED provider flag contract for `{tool}`: not installed");
    false
}

/// Whether `help` lists `flag`, accepting git's `--[no-]name` spelling for `--name` and `--no-name`.
fn help_lists(help: &str, flag: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    let mut spellings = vec![flag.to_owned()];
    if let Some(name) = flag.strip_prefix("--no-") {
        spellings.push(format!("--[no-]{name}"));
    } else if let Some(name) = flag.strip_prefix("--") {
        spellings.push(format!("--[no-]{name}"));
    }
    spellings.iter().any(|spelling| {
        help.match_indices(spelling.as_str()).any(|(at, _)| {
            let before = help[..at].chars().next_back();
            let after = help[at + spelling.len()..].chars().next();
            !before.is_some_and(word) && !after.is_some_and(word)
        })
    })
}

/// The real tool's help for one command shape. `None` when the tool cannot describe it here.
fn help_for(shape: &ToolShape) -> Option<String> {
    let mut args: Vec<&str> = shape.subcommand.to_vec();
    match shape.tool {
        "git" => args.push("-h"),
        _ => args.push("--help"),
    }
    let (_, mut text) = run(shape.tool, &args)?;
    if shape.tool == "npm" {
        // Global config keys (registry, access, tag) are not in a command's own usage.
        let config = run("npm", &["config", "ls", "-l"])?.1;
        for key in config
            .lines()
            .filter_map(|line| line.split_once(" = "))
            .map(|(key, _)| key.trim())
        {
            text.push_str(&format!("\n--{key}\n"));
        }
    }
    Some(text)
}

fn check_tool(tool: &str) {
    if !tool_available(tool) {
        return;
    }
    let mut checked = 0;
    let mut failures = Vec::new();
    for shape in TOOL_SHAPES.iter().filter(|shape| shape.tool == tool) {
        // `git rev-parse -h` lists no flags; `git_rev_parse_flags_are_understood_by_real_git` probes them.
        if shape.tool == "git" && shape.subcommand == ["rev-parse"] {
            continue;
        }
        let Some(help) = help_for(shape) else {
            eprintln!("SKIPPED `{tool} {}`: help unavailable", shape.subcommand.join(" "));
            continue;
        };
        for flag in shape.flags.iter().filter(|flag| **flag != "--") {
            checked += 1;
            if !help_lists(&help, flag) {
                failures.push(format!(
                    "`{tool} {}` help does not list `{flag}`",
                    shape.subcommand.join(" ")
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "flags emitted but absent from the real tool: {failures:#?}"
    );
    eprintln!("RAN flag contract against real `{tool}`: {checked} flags checked");
}

#[test]
fn every_emitted_gh_flag_exists_in_real_gh_help() {
    check_tool("gh");
}

#[test]
fn every_emitted_git_flag_exists_in_real_git_help() {
    check_tool("git");
}

#[test]
fn every_emitted_cargo_flag_exists_in_real_cargo_help() {
    check_tool("cargo");
}

#[test]
fn every_emitted_npm_flag_exists_in_real_npm_help() {
    check_tool("npm");
}

/// Real `git rev-parse` echoes an option it does not know, so a known flag is
/// one whose output is not itself.
#[test]
fn git_rev_parse_flags_are_understood_by_real_git() {
    if !tool_available("git") {
        return;
    }
    let repo = tempfile::tempdir().unwrap();
    let git_in = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    git_in(&["init", "-q"]);
    git_in(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.com",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--allow-empty",
        "-qm",
        "x",
    ]);
    let shape = tool_shape("git", &["rev-parse"]).expect("rev-parse shape");
    for flag in shape.flags {
        let output = if *flag == "--verify" || *flag == "--quiet" {
            git_in(&["rev-parse", "--verify", "--quiet", "HEAD"])
        } else {
            git_in(&["rev-parse", flag])
        };
        assert!(
            !output.is_empty() && output != *flag,
            "real git did not understand `rev-parse {flag}` (printed {output:?})"
        );
    }
    eprintln!(
        "RAN flag contract against real `git rev-parse`: {} flags probed",
        shape.flags.len()
    );
}

/// Every `"--flag` literal in the release-path argv builders must be a flag the
/// contract tests above verify against the real tools.
#[test]
fn every_flag_literal_in_the_release_path_sources_is_in_the_tool_shapes() {
    let graph = Path::new(env!("CARGO_MANIFEST_DIR")).join("../callisto-graph/src/commands");
    let mut files: Vec<PathBuf> = vec![graph.join("release_artifacts.rs")];
    for directory in [graph.join("release"), graph.join("release/provider")] {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    let known: BTreeSet<&str> = TOOL_SHAPES
        .iter()
        .flat_map(|shape| shape.flags.iter().copied())
        .collect();
    let mut unknown = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        let production = text.split("#[cfg(test)]").next().unwrap();
        for (at, _) in production.match_indices("\"--") {
            let name: String = production[at + 1..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if name.len() > 2 && !known.contains(name.as_str()) {
                unknown.push(format!("{}: {name}", file.file_name().unwrap().to_string_lossy()));
            }
        }
    }
    assert!(
        unknown.is_empty(),
        "flags emitted by the release path but missing from TOOL_SHAPES (add them so the real tool is asked about them): {unknown:?}"
    );
}

fn key_paths(value: &Value, prefix: &str, paths: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                let path = format!("{prefix}.{key}");
                paths.insert(path.clone());
                key_paths(inner, &path, paths);
            }
        }
        Value::Array(items) => items
            .iter()
            .for_each(|item| key_paths(item, &format!("{prefix}[]"), paths)),
        _ => {}
    }
}

fn shape_of_body(body: &str) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    key_paths(&serde_json::from_str(body).expect("body is JSON"), "", &mut paths);
    paths
}

fn header_names(head: &str) -> BTreeSet<String> {
    head.lines()
        .skip(1)
        .filter_map(|line| line.trim_end_matches('\r').split_once(':'))
        .map(|(name, _)| name.trim().to_ascii_lowercase())
        .collect()
}

struct FakeBin {
    _dir: tempfile::TempDir,
    rig: Rig,
    forge_tag: &'static str,
}

impl FakeBin {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let rig = Rig::new(dir.path(), "1111111111111111111111111111111111111111");
        Self {
            _dir: dir,
            rig,
            forge_tag: "callisto@0.2.0",
        }
    }

    fn run(&self, tool: &str, args: &[&str]) -> (Option<i32>, String, String) {
        let output = Command::new(self.rig.bin.join(tool))
            .args(args)
            .env("CALLISTO_TEST_LOG", &self.rig.log)
            .env("CALLISTO_TEST_GIT_TRACE", &self.rig.git_trace)
            .env("CALLISTO_TEST_GH_CALLS", &self.rig.gh_calls)
            .env("CALLISTO_TEST_FORGE_MARKER", &self.rig.forge_marker)
            .env(
                "CALLISTO_TEST_ARTIFACT_MARKER",
                self.rig.log.with_extension("artifact-marker"),
            )
            .env("CALLISTO_TEST_FORGE_TAG", self.forge_tag)
            .env(
                "CALLISTO_TEST_FORGE_COMMITISH",
                "1111111111111111111111111111111111111111",
            )
            .env("CALLISTO_TEST_REAL_GIT", system_git())
            .output()
            .unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
}

const RELEASE_ENDPOINT: &str = "repos/example/core/releases/tags/callisto@0.2.0";

/// The fake forge is templated from the captured release, so its answers carry
/// exactly the captured status line, header names and JSON key set.
#[test]
fn the_fake_gh_serves_the_captured_release_shape() {
    let fake = FakeBin::new();
    fs::write(&fake.rig.forge_marker, "published").unwrap();
    fs::write(
        fake.rig.log.with_extension("artifact-marker"),
        format!("a.tar.gz|10|{}\n", "a".repeat(64)),
    )
    .unwrap();
    let (code, stdout, _) = fake.run("gh", &["api", "--include", "--method", "GET", RELEASE_ENDPOINT]);
    assert_eq!(code, Some(0));
    let (head, body) = fixtures::split_raw_http(&stdout);
    let (captured_head, captured_body) = fixtures::split_raw_http(fixtures::GITHUB_RELEASE_PUBLISHED);
    assert_eq!(head.lines().next(), captured_head.lines().next());
    assert_eq!(header_names(head), header_names(captured_head));
    assert_eq!(shape_of_body(body), shape_of_body(captured_body));
    let release: Value = serde_json::from_str(body).unwrap();
    assert_eq!(release["tag_name"], "callisto@0.2.0");
    assert_eq!(release["draft"], false);

    fs::write(&fake.rig.forge_marker, "draft").unwrap();
    let (code, stdout, stderr) = fake.run("gh", &["api", "--include", "--method", "GET", RELEASE_ENDPOINT]);
    assert_eq!(
        code,
        Some(1),
        "the tag endpoint does not serve drafts and gh exits 1 for a 404"
    );
    assert!(stdout.starts_with("HTTP/2.0 404 Not Found"), "{stdout}");
    assert!(stderr.contains("HTTP 404"), "{stderr}");
    let (_, listing, _) = fake.run(
        "gh",
        &[
            "api",
            "--include",
            "--method",
            "GET",
            "repos/example/core/releases?per_page=100&page=1",
        ],
    );
    let (list_head, list_body) = fixtures::split_raw_http(&listing);
    let (captured_list_head, captured_list_body) = fixtures::split_raw_http(fixtures::GITHUB_RELEASE_LIST);
    assert_eq!(header_names(list_head), header_names(captured_list_head));
    assert_eq!(shape_of_body(list_body), shape_of_body(captured_list_body));
    let listed: Value = serde_json::from_str(list_body).unwrap();
    assert_eq!(listed[0]["draft"], true);
}

#[test]
fn every_fake_rejects_unknown_subcommands_and_flags_like_the_real_tool() {
    let fake = FakeBin::new();
    let cases: &[(&str, &[&str], &str)] = &[
        (
            "gh",
            &["api", "--repo", "o/r", RELEASE_ENDPOINT],
            "unknown flag: --repo",
        ),
        ("gh", &["release", "create", "v1", "--bogus"], "unknown flag: --bogus"),
        (
            "gh",
            &["release", "delete", "v1"],
            "unknown command \"delete\" for \"gh release\"",
        ),
        ("gh", &["pr", "list"], "unknown command \"pr\" for \"gh\""),
        (
            "gh",
            &["attestation", "verify", "f", "--bundle", "b"],
            "unknown flag: --bundle",
        ),
        (
            "cargo",
            &["publish", "--dry-run"],
            "unexpected argument '--dry-run' found",
        ),
        ("cargo", &["build"], "no such command: `build`"),
        (
            "cargo",
            &["info", "callisto@1.0.0", "--offline"],
            "unexpected argument '--offline' found",
        ),
        ("git", &["push", "--force", "origin", "v1"], "unknown option `-force'"),
        ("git", &["ls-remote", "--tags", "origin"], "unknown option `-tags'"),
    ];
    for (tool, args, message) in cases {
        let (code, _, stderr) = fake.run(tool, args);
        assert_ne!(code, Some(0), "`{tool} {args:?}` must fail");
        assert!(stderr.contains(message), "`{tool} {args:?}` stderr was: {stderr}");
    }
    let (code, ..) = fake.run(
        "git",
        &[
            "ls-remote",
            "https://example.invalid/r.git",
            "refs/tags/x",
            "refs/tags/x^{}",
        ],
    );
    assert_eq!(code, Some(0));
}

#[test]
fn the_argv_allow_list_check_flags_an_unlisted_flag_or_command() {
    let mut violations = Vec::new();
    for line in [
        "gh api --include --method GET repos/o/r/releases/tags/t",
        "gh api --repo o/r x",
        "gh pr list",
        "cargo publish --manifest-path /x --locked",
        "cargo info foo --registry crates-io",
        "cargo info foo --offline",
        "git -c tag.gpgSign=false tag -a -m msg --no-sign -- v1 abc",
        "git tag --force v1",
    ] {
        argv_violations(line, &mut violations);
    }
    assert_eq!(violations.len(), 4, "{violations:#?}");
}
