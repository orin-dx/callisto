//! `init` with a missing or non-hosted `origin`, and without a terminal, through the real binary.
use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const SECRET: &str = "secret-vcs-host/team-repo.git";

fn workspace(origin: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"solo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "").unwrap();
    callisto_fixtures::git::run_git(root, &["add", "."]);
    callisto_fixtures::git::run_git(root, &["commit", "-q", "-m", "workspace"]);
    if let Some(origin) = origin {
        callisto_fixtures::git::run_git(root, &["remote", "add", "origin", origin]);
    }
    dir
}

fn init(root: &Path, format: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--format", format, "--cwd", root.to_str().unwrap(), "init"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("callisto binary should be invocable")
}

fn both(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

// A missing or non-hosted origin gives one note, init still writes, and the remote value is never printed.
#[test]
fn a_missing_or_non_hosted_origin_notes_once_and_init_continues() {
    for (origin, marker) in [(None, "No `origin` remote"), (Some(SECRET), "is not a hosted remote")] {
        for format in ["text", "json"] {
            let dir = workspace(origin);
            let output = init(dir.path(), format, &["--yes"]);
            let text = both(&output);
            assert_eq!(output.status.code(), Some(0), "{origin:?} {format}: {text}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(stderr.matches(marker).count(), 1, "{origin:?} {format}: {text}");
            assert!(!text.contains("secret-vcs-host"), "{origin:?} {format}: {text}");
            assert!(
                origin.is_none() || (text.contains("URL has no scheme") && text.contains("git@host:owner/repo")),
                "{origin:?} {format}: {text}"
            );
            assert!(dir.path().join("callisto.toml").exists(), "{origin:?} {format}");
            assert!(!dir.path().join(".github").exists(), "{origin:?} {format}");
        }
    }
}

// A flag that needs a hosted origin fails with E299 and names the push-URL commands, without the remote value.
#[test]
fn a_workflow_or_artifact_target_without_a_hosted_origin_is_e299_with_the_push_help() {
    for origin in [None, Some(SECRET)] {
        for flag in [
            &["--workflow"][..],
            &["--artifact-target", "x86_64-unknown-linux-gnu"][..],
        ] {
            for format in ["text", "json"] {
                let dir = workspace(origin);
                let mut args = vec!["--yes"];
                args.extend_from_slice(flag);
                let output = init(dir.path(), format, &args);
                let text = both(&output);
                let squashed: String = text.split_whitespace().collect();
                let context = format!("{origin:?} {flag:?} {format}: {text}");
                assert_eq!(output.status.code(), Some(1), "{context}");
                assert!(text.contains("E299"), "{context}");
                assert!(
                    origin.is_none() || squashed.contains("gitremoteget-url--pushorigin"),
                    "{context}"
                );
                assert!(
                    origin.is_none() || squashed.contains("gitremoteset-url--pushorigin<url>"),
                    "{context}"
                );
                assert!(!text.contains("secret-vcs-host"), "{context}");
                assert!(
                    origin.is_none() || squashed.contains("nothosted:URLhasnoscheme"),
                    "{context}"
                );
                assert!(!dir.path().join("callisto.toml").exists(), "{context}");
            }
        }
    }
}

// Without a terminal (or in JSON) and without --yes, init never prompts: it fails with E240.
#[test]
fn init_without_a_terminal_or_yes_never_prompts_and_fails_with_e240() {
    for format in ["text", "json"] {
        let dir = workspace(None);
        let output = init(dir.path(), format, &[]);
        let text = both(&output);
        assert_eq!(output.status.code(), Some(2), "{format}: {text}");
        assert!(text.contains("E240") && text.contains("--yes"), "{format}: {text}");
        assert!(!dir.path().join("callisto.toml").exists(), "{format}");
    }
}
