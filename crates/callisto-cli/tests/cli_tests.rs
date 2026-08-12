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
    std::fs::write(root.join("callisto.toml"), "").unwrap();

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

/// AC-001 + AC-002 + AC-014: a napi package and a maturin package that both
/// declare the same two triples must produce byte-identical platform/arch/
/// abi/hostRunner/useCross for each triple -- proving the derivation is
/// shared code, not a duplicated table. AC-014: the darwin triple's abi is
/// JSON null; the linux triple's abi is a non-null string.
#[test]
fn matrix_napi_and_maturin_share_triple_derivation() {
    use std::process::Command;

    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = []\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("callisto.toml"), "").unwrap();

    let napi_dir = root.join("napi-mod");
    std::fs::create_dir_all(&napi_dir).unwrap();
    std::fs::write(
        napi_dir.join("package.json"),
        r#"{"name":"napi-mod","napi":{"targets":["aarch64-apple-darwin","x86_64-unknown-linux-gnu"]}}"#,
    )
    .unwrap();

    let maturin_dir = root.join("maturin-mod");
    std::fs::create_dir_all(&maturin_dir).unwrap();
    std::fs::write(
        maturin_dir.join("pyproject.toml"),
        "[project]\nname = \"maturin-mod\"\nversion = \"0.1.0\"\n\n[tool.maturin]\ntargets = [\"aarch64-apple-darwin\", \"x86_64-unknown-linux-gnu\"]\n",
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_callisto");
    let output = Command::new(bin)
        .args([
            "--cwd",
            &root.to_string_lossy(),
            "--format",
            "json",
            "matrix",
        ])
        .output()
        .expect("failed to spawn callisto binary");

    assert!(
        output.status.success(),
        "matrix should exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let napi_group = &json["platformTargets"]["napi-mod"];
    let maturin_group = &json["platformTargets"]["maturin-mod"];
    assert_eq!(napi_group["kind"], "napi");
    assert_eq!(maturin_group["kind"], "maturin");

    let napi_targets = napi_group["targets"].as_array().unwrap();
    let maturin_targets = maturin_group["targets"].as_array().unwrap();
    assert_eq!(napi_targets.len(), 2);
    assert_eq!(maturin_targets.len(), 2);

    for (n, m) in napi_targets.iter().zip(maturin_targets.iter()) {
        assert_eq!(n["triple"], m["triple"]);
        assert_eq!(n["platform"], m["platform"]);
        assert_eq!(n["arch"], m["arch"]);
        assert_eq!(n["abi"], m["abi"]);
        assert_eq!(n["hostRunner"], m["hostRunner"]);
        assert_eq!(n["useCross"], m["useCross"]);

        if n["triple"] == "aarch64-apple-darwin" {
            assert!(n["abi"].is_null(), "darwin abi must serialize as null");
        }
        if n["triple"] == "x86_64-unknown-linux-gnu" {
            assert!(n["abi"].is_string(), "linux abi must be a non-null string");
        }
    }
}
