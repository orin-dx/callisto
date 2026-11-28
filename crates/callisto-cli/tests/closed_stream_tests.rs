#![cfg(unix)]

//! A closed or failing output stream never changes a command's result: notes and error cards are best effort on
//! stderr, and a reader that closes stdout ends the output, not the command. Storage writes still fail.

use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use callisto_cli::cli::{GlobalArgs, OutputFormat, ReleaseCommandArgs};
use callisto_cli::commands;
use callisto_fixtures::git::{init_repo, run_git};

/// A pipe whose reading end is already closed: every write to it fails with `BrokenPipe`.
fn closed_pipe() -> Stdio {
    let (reader, writer) = io::pipe().expect("create a pipe");
    drop(reader);
    writer.into()
}

/// A stream that refuses every write.
struct Unwritable;

impl io::Write for Unwritable {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::BrokenPipe.into())
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
}

fn callisto(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_callisto"));
    command.args(["--cwd", root.to_str().unwrap()]).args(args);
    command
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// One committed crate at 0.1.0 with no `origin`, initialized.
fn solo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"solo\"\nversion = \"0.1.0\"\nedition = \"2021\"\ndescription = \"t\"\nlicense = \"MIT\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial"]);
    dir
}

fn init(root: &Path) {
    let out = callisto(root, &["init", "--yes", "--versioning", "independent"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "init"]);
}

/// A selection with nothing to release refuses with E169 whether or not its pending-work note can be written.
#[test]
fn a_failed_note_never_replaces_the_refusal_it_explains() {
    let dir = solo();
    let root = dir.path();
    init(root);
    run_git(root, &["tag", "-a", "-m", "Release solo@0.1.0", "solo@0.1.0"]);
    let added = callisto(
        root,
        &["add", "--package", "cargo/solo:patch", "--summary", "Fix a thing"],
    )
    .output()
    .unwrap();
    assert!(added.status.success(), "{}", stderr(&added));
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "changeset"]);

    for dry_run in [true, false] {
        let global = GlobalArgs {
            format: OutputFormat::Json,
            cwd: root.to_path_buf(),
            dry_run,
        };
        let args = || ReleaseCommandArgs {
            command: None,
            packages: vec!["cargo/solo".to_owned()],
            receipt: None,
        };
        // The note is written when stderr works, so the failing run below reaches it.
        let mut written = Vec::new();
        let control = commands::release::run(args(), &global, &mut written).unwrap_err();
        let code = |error: &callisto_cli::error::CliError| miette::Diagnostic::code(error).map(|code| code.to_string());
        assert_eq!(
            code(&control).as_deref(),
            Some("E169"),
            "dry run {dry_run}: {control:?}"
        );
        let note = String::from_utf8(written).unwrap();
        assert!(note.contains("run `callisto version`"), "dry run {dry_run}: {note}");

        let refused = commands::release::run(args(), &global, &mut Unwritable).unwrap_err();
        assert_eq!(
            code(&refused).as_deref(),
            Some("E169"),
            "dry run {dry_run}: {refused:?}"
        );
    }
}

/// A usage error keeps exit 2 in both formats when its error card cannot be written.
#[test]
fn a_closed_stderr_keeps_the_usage_exit_status() {
    let dir = tempfile::tempdir().unwrap();
    for format in ["text", "json"] {
        let control = callisto(dir.path(), &["--format", format, "help", "nonexistent"])
            .output()
            .unwrap();
        assert_eq!(control.status.code(), Some(2), "{format}: {}", stderr(&control));
        let run = callisto(dir.path(), &["--format", format, "help", "nonexistent"])
            .stderr(closed_pipe())
            .output()
            .unwrap();
        assert_eq!(run.status.code(), Some(2), "{format}");
    }
}

/// `init`'s notes are best effort: the missing-origin note and the failed pending-work check cannot fail a preview.
#[test]
fn a_closed_stderr_never_fails_an_init_preview() {
    let dir = solo();
    let root = dir.path();
    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(
        root.join(".changeset/broken.md"),
        "---\n\"cargo/solo\": nonsense\n---\n\nText.\n",
    )
    .unwrap();
    let args = [
        "--format",
        "text",
        "--dry-run",
        "init",
        "--yes",
        "--versioning",
        "independent",
    ];
    let control = callisto(root, &args).output().unwrap();
    assert!(control.status.success(), "{}", stderr(&control));
    assert!(
        stderr(&control).contains("No `origin` remote is configured"),
        "{}",
        stderr(&control)
    );
    assert!(
        stderr(&control).contains("could not check for pending version work"),
        "{}",
        stderr(&control)
    );

    let run = callisto(root, &args).stderr(closed_pipe()).output().unwrap();
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(run.stdout, control.stdout);
}

/// Every command treats a closed stdout the same: it ends the output, not the command, in both formats.
#[test]
fn a_closed_stdout_ends_output_not_the_command() {
    let dir = solo();
    let root = dir.path();
    init(root);
    let added = callisto(
        root,
        &["add", "--package", "cargo/solo:patch", "--summary", "Fix a thing"],
    )
    .output()
    .unwrap();
    assert!(added.status.success(), "{}", stderr(&added));
    for format in ["text", "json"] {
        for args in [
            ["schema"].as_slice(),
            &["help"],
            &["help", "status"],
            &["completions", "bash"],
            &["status"],
            &["matrix"],
            &["--dry-run", "version"],
            &["--dry-run", "release"],
            &["release", "pending"],
        ] {
            let args: Vec<&str> = ["--format", format].iter().chain(args).copied().collect();
            let control = callisto(root, &args).output().unwrap();
            assert_eq!(control.status.code(), Some(0), "{args:?}: {}", stderr(&control));
            let run = callisto(root, &args).stdout(closed_pipe()).output().unwrap();
            assert_eq!(run.status.code(), Some(0), "{args:?}: {}", stderr(&run));
            assert_eq!(run.stderr, control.stderr, "{args:?}");
        }
    }
}

/// A stdout that refuses writes for a reason other than a closed reader: a non-blocking socket already full, so every
/// write fails with `WouldBlock`.
fn full_stdout() -> (std::os::unix::net::UnixStream, Stdio) {
    use std::io::Write as _;

    let (writer, reader) = std::os::unix::net::UnixStream::pair().unwrap();
    writer.set_nonblocking(true).unwrap();
    let mut filling = &writer;
    let full = (0..100_000)
        .find_map(|_| filling.write(&[0u8; 4096]).err())
        .expect("the socket fills");
    assert_eq!(full.kind(), io::ErrorKind::WouldBlock, "{full:?}");
    (reader, std::os::fd::OwnedFd::from(writer).into())
}

/// Help and version are output like any other: a stdout that fails for a reason other than a closed reader fails them
/// with E213, while a closed one still ends them quietly, and a missing subcommand stays a usage error.
#[test]
fn help_and_version_output_goes_through_the_stdout_sink() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        ["help"].as_slice(),
        &["--help"],
        &["status", "--help"],
        &["--version"],
        &["-V"],
    ] {
        let (_reader, stdout) = full_stdout();
        let failed = callisto(dir.path(), args).stdout(stdout).output().unwrap();
        assert_eq!(failed.status.code(), Some(1), "{args:?}: {}", stderr(&failed));
        assert!(stderr(&failed).contains("E213"), "{args:?}: {}", stderr(&failed));
        let closed = callisto(dir.path(), args).stdout(closed_pipe()).output().unwrap();
        assert_eq!(closed.status.code(), Some(0), "{args:?}: {}", stderr(&closed));
        let control = callisto(dir.path(), args).output().unwrap();
        assert_eq!(control.status.code(), Some(0), "{args:?}");
        assert!(!control.stdout.is_empty(), "{args:?}");
    }
    let usage = callisto(dir.path(), &[]).output().unwrap();
    assert_eq!(usage.status.code(), Some(2), "{}", stderr(&usage));
}

/// A closed stdout does not excuse a storage failure: a changeset that cannot be written still fails the command.
#[test]
fn a_storage_failure_still_fails_with_stdout_closed() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = solo();
    let root = dir.path();
    init(root);
    let changesets = root.join(".changeset");
    fs::set_permissions(&changesets, fs::Permissions::from_mode(0o555)).unwrap();
    for format in ["text", "json"] {
        let run = callisto(
            root,
            &[
                "--format",
                format,
                "add",
                "--package",
                "cargo/solo:patch",
                "--summary",
                "Fix a thing",
            ],
        )
        .stdout(closed_pipe())
        .output()
        .unwrap();
        assert_eq!(run.status.code(), Some(1), "{format}: {}", stderr(&run));
    }
    fs::set_permissions(&changesets, fs::Permissions::from_mode(0o755)).unwrap();
}
