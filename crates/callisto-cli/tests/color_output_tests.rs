//! `status` and `release --dry-run` render
//! ANSI color and box-drawing tables through the one `NO_COLOR`/`CLICOLOR_FORCE`/
//! `FORCE_COLOR` decision, driven through the compiled binary so the check
//! covers the real (piped, non-TTY) stdout comfy-table and anstream see.

use std::fs;
use std::process::{Command, Stdio};
use tempfile::TempDir;

const ESC: u8 = 0x1b;
// comfy-table's UTF8_FULL preset border char, e.g. U+2502 BOX DRAWINGS LIGHT VERTICAL.
const BOX_DRAWING: char = '\u{2502}';

/// A workspace with one never-tagged crate, so both `status` and
/// `release --dry-run` have something to render.
fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    callisto_fixtures::git::init_repo(root);

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/demo\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/demo/src")).unwrap();
    fs::write(
        root.join("crates/demo/Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n",
    )
    .unwrap();
    fs::write(root.join("crates/demo/src/lib.rs"), "pub fn hi() {}\n").unwrap();
    fs::write(root.join("callisto.toml"), "").unwrap();

    assert!(Command::new("git")
        .args(["add", "-A"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", "init"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());

    tmp
}

/// Runs the real binary with a piped (non-TTY) stdout/stderr and the given extra env vars.
fn run(root: &std::path::Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--cwd", &root.to_string_lossy()])
        .args(args)
        .env_clear()
        .envs(env.iter().copied())
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", std::env::var_os("HOME").unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to spawn callisto binary")
}

fn has_ansi_escape(bytes: &[u8]) -> bool {
    bytes.contains(&ESC)
}

/// `FORCE_COLOR` forces `status` to render ANSI color and a box-drawing table.
#[test]
fn status_with_force_color_renders_ansi_and_box_drawing_table() {
    let tmp = fixture();
    let out = run(tmp.path(), &["status"], &[("FORCE_COLOR", "1")]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        has_ansi_escape(&out.stdout),
        "expected ANSI escapes: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(BOX_DRAWING), "expected box-drawing table: {text}");
}

/// `CLICOLOR_FORCE` alone also forces color on for `status`.
#[test]
fn status_with_clicolor_force_renders_ansi() {
    let tmp = fixture();
    let out = run(tmp.path(), &["status"], &[("CLICOLOR_FORCE", "1")]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(has_ansi_escape(&out.stdout));
}

/// `NO_COLOR` (non-empty) wins over `FORCE_COLOR`/`CLICOLOR_FORCE` for `status`.
#[test]
fn status_no_color_wins_over_force_vars() {
    let tmp = fixture();
    let out = run(
        tmp.path(),
        &["status"],
        &[("NO_COLOR", "1"), ("FORCE_COLOR", "1"), ("CLICOLOR_FORCE", "1")],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!has_ansi_escape(&out.stdout));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains(BOX_DRAWING), "expected no box-drawing table: {text}");
}

/// With no env signal and a piped (non-TTY) stdout, `status` renders plain text.
#[test]
fn status_default_non_tty_has_no_color_or_box_drawing() {
    let tmp = fixture();
    let out = run(tmp.path(), &["status"], &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!has_ansi_escape(&out.stdout));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains(BOX_DRAWING), "expected no box-drawing table: {text}");
    assert!(text.contains("demo 0.1.0 (pending: none)"), "got: {text}");
}

/// `FORCE_COLOR` forces `release --dry-run` to render ANSI color and a box-drawing table.
#[test]
fn release_dry_run_with_force_color_renders_ansi_and_box_drawing_table() {
    let tmp = fixture();
    let out = run(tmp.path(), &["--dry-run", "release"], &[("FORCE_COLOR", "1")]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(has_ansi_escape(&out.stdout), "expected ANSI escapes: {text}");
    assert!(text.contains(BOX_DRAWING), "expected box-drawing table: {text}");
    assert!(text.contains("demo"), "got: {text}");
}

/// With no env signal and a piped (non-TTY) stdout, `release --dry-run` renders plain text.
#[test]
fn release_dry_run_default_non_tty_has_no_color_or_box_drawing() {
    let tmp = fixture();
    let out = run(tmp.path(), &["--dry-run", "release"], &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!has_ansi_escape(&out.stdout));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains(BOX_DRAWING), "expected no box-drawing table: {text}");
    assert!(text.starts_with("Release plan:\n"), "got: {text}");
}

/// No command's default text output matches `schema v\d+` (that ban is
/// universal except `callisto schema`'s own output).
#[test]
fn status_and_release_text_output_never_mentions_schema_version() {
    let tmp = fixture();
    for (args, env) in [
        (vec!["status"], vec![]),
        (vec!["status"], vec![("FORCE_COLOR", "1")]),
        (vec!["--dry-run", "release"], vec![]),
        (vec!["--dry-run", "release"], vec![("FORCE_COLOR", "1")]),
    ] {
        let out = run(tmp.path(), &args, &env);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            !schema_v_regex(&text),
            "{args:?} with env {env:?} must not mention a schema version: {text}"
        );
    }
}

fn schema_v_regex(text: &str) -> bool {
    text.match_indices("schema v")
        .any(|(i, m)| text[i + m.len()..].chars().next().is_some_and(|c| c.is_ascii_digit()))
}
