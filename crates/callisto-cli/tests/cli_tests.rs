use callisto_cli::cli::{Cli, Command, OutputFormat};
use std::path::PathBuf;

#[test]
fn test_cli_parse_global_args() {
    use clap::Parser;
    let cli = Cli::parse_from(["callisto", "--format", "json", "--cwd", "/tmp", "status"]);
    assert_eq!(cli.global.format, OutputFormat::Json);
    assert_eq!(cli.global.cwd, PathBuf::from("/tmp"));
    assert!(matches!(cli.command, Command::Status(_)));
}

#[test]
fn test_cli_parse_add_command() {
    use clap::Parser;
    let cli = Cli::parse_from([
        "callisto",
        "add",
        "--package",
        "foo:minor",
        "--summary",
        "Added feature foo",
    ]);
    if let Command::Add(args) = cli.command {
        assert_eq!(args.packages, vec!["foo:minor"]);
        assert_eq!(args.summary, Some("Added feature foo".to_string()));
    } else {
        panic!("Expected Add command");
    }
}

/// Integration test verifying that `callisto add` selects the non-interactive
/// error path when stdin is not a terminal and no `--package` flags are given.
///
/// This test pipes `/dev/null` as stdin (no TTY) and invokes the real binary.
/// `tty::is_interactive()` returns `false` in that condition, so the command
/// must reject the invocation with a `NotATty` error rather than attempting to
/// open a `dialoguer` wizard on a non-TTY handle.
///
/// The binary is located via the `CARGO_BIN_EXE_callisto` env var that Cargo
/// injects at test compile time, so no hard-coded path is needed.
#[test]
fn test_add_non_interactive_via_pipe() {
    use std::process::{Command, Stdio};
    use tempfile::TempDir;

    // Build a minimal workspace so the binary can locate a root.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // git init (callisto needs a git repo for some operations; tolerate failure)
    drop(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .output(),
    );

    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = []\nresolver = \"2\"\n",
    )
    .unwrap();

    // callisto.toml must exist for `load_workspace` to succeed.
    std::fs::write(root.join("callisto.toml"), "[workspace]\n").unwrap();

    let bin = env!("CARGO_BIN_EXE_callisto");

    // Pipe /dev/null as stdin — not a TTY.
    let stdin_file = std::fs::File::open(std::path::Path::new("/dev/null")).unwrap();

    let output = Command::new(bin)
        .args(["--cwd", &root.to_string_lossy(), "add"])
        .stdin(stdin_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to spawn callisto binary");

    // The command must exit with a non-zero code (NotATty error path).
    assert!(
        !output.status.success(),
        "callisto add with piped stdin and no --package flags should fail with NotATty"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("stdin is not a terminal") || stderr.contains("not_a_tty"),
        "stderr should mention the TTY check; got: {stderr}"
    );
}
