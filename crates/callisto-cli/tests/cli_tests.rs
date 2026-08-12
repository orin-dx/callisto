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
    assert_eq!(napi_group["source"], "napi.targets");
    assert_eq!(maturin_group["source"], "[tool.maturin].targets");

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

/// AC-004, AC-005, AC-005b: npm-only, python-only, and dual-manifest
/// packages each produce the exact runtimeVersions shape the spec pins.
#[test]
fn matrix_runtime_versions_npm_python_and_dual_manifest() {
    use std::process::Command;

    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = []\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("callisto.toml"), "").unwrap();

    let npm_dir = root.join("npm-only");
    std::fs::create_dir_all(&npm_dir).unwrap();
    std::fs::write(
        npm_dir.join("package.json"),
        r#"{"name":"npm-only","engines":{"node":">=20.0.0"}}"#,
    )
    .unwrap();

    let py_dir = root.join("py-only");
    std::fs::create_dir_all(&py_dir).unwrap();
    std::fs::write(
        py_dir.join("pyproject.toml"),
        "[project]\nname = \"py-only\"\nversion = \"0.1.0\"\nrequires-python = \">=3.9\"\n",
    )
    .unwrap();

    let dual_dir = root.join("dual-pkg");
    std::fs::create_dir_all(&dual_dir).unwrap();
    std::fs::write(
        dual_dir.join("package.json"),
        r#"{"name":"dual-pkg","engines":{"node":">=20.0.0"}}"#,
    )
    .unwrap();
    std::fs::write(
        dual_dir.join("pyproject.toml"),
        "[project]\nname = \"dual-pkg\"\nversion = \"0.1.0\"\nrequires-python = \">=3.9\"\n",
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
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        json["runtimeVersions"]["npm-only"],
        serde_json::json!([{"ecosystem":"npm","field":"engines.node","range":">=20.0.0"}])
    );
    assert_eq!(
        json["runtimeVersions"]["py-only"],
        serde_json::json!([{"ecosystem":"python","field":"requires-python","range":">=3.9"}])
    );
    assert_eq!(
        json["runtimeVersions"]["dual-pkg"],
        serde_json::json!([
            {"ecosystem":"npm","field":"engines.node","range":">=20.0.0"},
            {"ecosystem":"python","field":"requires-python","range":">=3.9"}
        ])
    );
}

/// AC-006: --package restricts output to exactly one package's entries, in
/// BOTH platformTargets and runtimeVersions. AC-007: an unknown --package
/// name exits 1 and names the package in stderr, never a structurally valid
/// empty report on stdout.
#[test]
fn matrix_package_filter_and_unknown_package_via_binary() {
    use std::process::Command;

    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = []\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("callisto.toml"), "").unwrap();

    for name in ["pkg-a", "pkg-b"] {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            format!(
                r#"{{"name":"{name}","napi":{{"targets":["aarch64-apple-darwin"]}},"engines":{{"node":">=20.0.0"}}}}"#
            ),
        )
        .unwrap();
    }

    let bin = env!("CARGO_BIN_EXE_callisto");

    let filtered = Command::new(bin)
        .args([
            "--cwd",
            &root.to_string_lossy(),
            "--format",
            "json",
            "matrix",
            "--package",
            "pkg-a",
        ])
        .output()
        .unwrap();
    assert!(filtered.status.success());
    let json: serde_json::Value = serde_json::from_slice(&filtered.stdout).unwrap();
    assert!(json["platformTargets"].get("pkg-a").is_some());
    assert!(json["platformTargets"].get("pkg-b").is_none());
    assert!(
        json["runtimeVersions"].get("pkg-a").is_some(),
        "runtimeVersions must include pkg-a"
    );
    assert!(
        json["runtimeVersions"].get("pkg-b").is_none(),
        "runtimeVersions must exclude pkg-b"
    );

    let unknown = Command::new(bin)
        .args([
            "--cwd",
            &root.to_string_lossy(),
            "--format",
            "json",
            "matrix",
            "--package",
            "does-not-exist",
        ])
        .output()
        .unwrap();
    assert!(
        !unknown.status.success(),
        "unknown --package must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(
        stderr.contains("does-not-exist"),
        "stderr must name the unknown package: {stderr}"
    );
    assert!(
        serde_json::from_slice::<serde_json::Value>(&unknown.stdout).is_err()
            || unknown.stdout.is_empty(),
        "no valid MatrixReport JSON must be printed to stdout on error"
    );
}

/// AC-009: platformTargets/runtimeVersions keys and each group's targets[]
/// are lexicographically ordered across 3+ packages. AC-001b: an explicitly
/// empty napi.targets = [] still produces a present platformTargets entry.
#[test]
fn matrix_orders_keys_lexicographically_across_three_packages() {
    use std::process::Command;

    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = []\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("callisto.toml"), "").unwrap();

    let zeta = root.join("zeta");
    std::fs::create_dir_all(&zeta).unwrap();
    std::fs::write(
        zeta.join("package.json"),
        r#"{"name":"zeta","napi":{"targets":["x86_64-unknown-linux-gnu","aarch64-apple-darwin"]}}"#,
    )
    .unwrap();

    let alpha = root.join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::write(
        alpha.join("package.json"),
        r#"{"name":"alpha","napi":{"targets":[]}}"#, // AC-001b
    )
    .unwrap();

    let mid = root.join("mid");
    std::fs::create_dir_all(&mid).unwrap();
    std::fs::write(
        mid.join("package.json"),
        r#"{"name":"mid","napi":{"targets":["aarch64-apple-darwin"]}}"#,
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
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let keys: Vec<&String> = json["platformTargets"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(
        keys,
        vec!["alpha", "mid", "zeta"],
        "keys must be lexicographically ordered"
    );

    // AC-001b: alpha's entry is present-but-empty, not absent.
    assert_eq!(
        json["platformTargets"]["alpha"]["targets"],
        serde_json::json!([])
    );

    // Within zeta's group, targets[] must be ascending by triple.
    let zeta_triples: Vec<String> = json["platformTargets"]["zeta"]["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["triple"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        zeta_triples,
        vec![
            "aarch64-apple-darwin".to_string(),
            "x86_64-unknown-linux-gnu".to_string()
        ]
    );
}

/// AC-003: a workspace with no relevant manifests anywhere produces exactly
/// {"schemaVersion":1,"platformTargets":{},"runtimeVersions":{}} with no
/// "diagnostics" key, and exits 0.
#[test]
fn matrix_empty_workspace_produces_exact_empty_report_shape() {
    use std::process::Command;

    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = []\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("callisto.toml"), "").unwrap();

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
        .unwrap();
    assert!(output.status.success());

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"schemaVersion": 1, "platformTargets": {}, "runtimeVersions": {}})
    );
    assert!(
        json.as_object().unwrap().get("diagnostics").is_none(),
        "diagnostics key must be entirely absent when empty"
    );
}

/// AC-011 (full end-to-end contract): a package with one recognised and one
/// unrecognised triple, plus a second package with only a recognised triple,
/// must exit 0, report exactly one UnrecognisedPlatformTriple diagnostic,
/// exclude the bad triple from every targets[] array, and keep every other
/// recognised triple (from both packages) present.
#[test]
fn matrix_unrecognised_triple_end_to_end_diagnostic_contract() {
    use std::process::Command;

    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = []\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("callisto.toml"), "").unwrap();

    let mixed_dir = root.join("mixed-mod");
    std::fs::create_dir_all(&mixed_dir).unwrap();
    std::fs::write(
        mixed_dir.join("package.json"),
        r#"{"name":"mixed-mod","napi":{"targets":["aarch64-apple-darwin","sparc64-unknown-linux-gnu"]}}"#,
    )
    .unwrap();

    let clean_dir = root.join("clean-mod");
    std::fs::create_dir_all(&clean_dir).unwrap();
    std::fs::write(
        clean_dir.join("package.json"),
        r#"{"name":"clean-mod","napi":{"targets":["x86_64-unknown-linux-gnu"]}}"#,
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
        "an unrecognised triple must not fail the call; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    let diagnostics = json["diagnostics"]
        .as_array()
        .expect("diagnostics must be an array");
    assert_eq!(
        diagnostics.len(),
        1,
        "exactly one diagnostic expected: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0]["code"], "unrecognised-platform-triple");

    let all_triples: Vec<String> = json["platformTargets"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|group| group["targets"].as_array().unwrap().iter())
        .map(|t| t["triple"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !all_triples.contains(&"sparc64-unknown-linux-gnu".to_string()),
        "the bad triple must not appear in any targets[] triple field: {all_triples:?}"
    );

    let mixed_triples: Vec<String> = json["platformTargets"]["mixed-mod"]["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["triple"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(mixed_triples, vec!["aarch64-apple-darwin".to_string()]);

    let clean_triples: Vec<String> = json["platformTargets"]["clean-mod"]["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["triple"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(clean_triples, vec!["x86_64-unknown-linux-gnu".to_string()]);
}

/// AC-008 + AC-003b: `--format text` and bare invocation (no --format flag)
/// must both exit 0 and produce identical non-empty, non-JSON stdout.
#[test]
fn matrix_text_format_and_bare_invocation_match_and_are_non_json() {
    use std::process::Command;

    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = []\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("callisto.toml"), "").unwrap();

    let dir = root.join("native-mod");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("package.json"),
        r#"{"name":"native-mod","napi":{"targets":["aarch64-apple-darwin"]},"engines":{"node":">=20.0.0"}}"#,
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_callisto");

    let explicit_text = Command::new(bin)
        .args([
            "--cwd",
            &root.to_string_lossy(),
            "--format",
            "text",
            "matrix",
        ])
        .output()
        .unwrap();
    assert!(explicit_text.status.success());

    let bare = Command::new(bin)
        .args(["--cwd", &root.to_string_lossy(), "matrix"])
        .output()
        .unwrap();
    assert!(bare.status.success());

    for output in [&explicit_text, &bare] {
        assert!(!output.stdout.is_empty(), "output must not be empty");
        assert!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).is_err(),
            "output must not parse as JSON"
        );
    }

    assert_eq!(
        explicit_text.stdout, bare.stdout,
        "--format text and the bare (no-flag) invocation must produce identical output"
    );
}

fn base_workspace() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("Cargo.toml"),
        "[workspace]\nmembers = []\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join("callisto.toml"), "").unwrap();
    tmp
}

fn run_matrix_json(root: &std::path::Path) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args([
            "--cwd",
            &root.to_string_lossy(),
            "--format",
            "json",
            "matrix",
        ])
        .output()
        .unwrap()
}

fn assert_error_exit_no_report(output: &std::process::Output) {
    assert!(!output.status.success(), "expected non-zero exit");
    assert!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).is_err()
            || output.stdout.is_empty(),
        "no MatrixReport JSON must be printed to stdout on error"
    );
}

/// AC-017: a package declaring both napi.targets and [tool.maturin].targets
/// exits 1 via GraphError::ConflictingPlatformTargetSources (E118), naming
/// the package and both source field names.
#[test]
fn matrix_conflicting_platform_target_sources_exits_1() {
    let tmp = base_workspace();
    let dir = tmp.path().join("conflict-pkg");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("package.json"),
        r#"{"name":"conflict-pkg","napi":{"targets":["aarch64-apple-darwin"]}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("pyproject.toml"),
        "[project]\nname = \"conflict-pkg\"\nversion = \"0.1.0\"\n\n[tool.maturin]\ntargets = [\"x86_64-unknown-linux-gnu\"]\n",
    )
    .unwrap();

    let output = run_matrix_json(tmp.path());
    assert_error_exit_no_report(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("conflict-pkg"),
        "stderr must name the package: {stderr}"
    );
    assert!(
        stderr.contains("napi.targets"),
        "stderr must name napi_source: {stderr}"
    );
    assert!(
        stderr.contains("[tool.maturin].targets"),
        "stderr must name maturin_source: {stderr}"
    );
}

/// AC-010: malformed pyproject.toml (unterminated string) exits 1. The
/// directory also carries a valid, name-bearing package.json so it
/// registers via npm -- ignore_walk.rs silently drops a directory whose
/// SOLE manifest is a malformed pyproject.toml, so without this co-located
/// package.json matrix() would never reach the malformed file at all.
#[test]
fn matrix_malformed_pyproject_toml_exits_1() {
    let tmp = base_workspace();
    let dir = tmp.path().join("bad-py");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("package.json"), r#"{"name":"bad-py"}"#).unwrap();
    std::fs::write(
        dir.join("pyproject.toml"),
        "[tool.maturin]\ntargets = [\"unterminated\n",
    )
    .unwrap();

    let output = run_matrix_json(tmp.path());
    assert_error_exit_no_report(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pyproject.toml"),
        "stderr must name the malformed path: {stderr}"
    );
}

/// AC-010b: malformed package.json (trailing comma) exits 1. The directory
/// also carries a valid, name-bearing pyproject.toml so it registers via
/// python -- mirroring AC-010's registration workaround for the JSON side
/// of the same read pass.
#[test]
fn matrix_malformed_package_json_exits_1() {
    let tmp = base_workspace();
    let dir = tmp.path().join("bad-js");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("pyproject.toml"),
        "[project]\nname = \"bad-js\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("package.json"), r#"{"napi":{"targets":["a",]}}"#).unwrap();

    let output = run_matrix_json(tmp.path());
    assert_error_exit_no_report(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("package.json"),
        "stderr must name the malformed path: {stderr}"
    );
}

/// AC-010c: napi.targets present but not an array (a bare string) exits 1.
/// The package.json itself carries a valid "name" field so it registers on
/// its own -- ignore_walk.rs's package.json registration only requires a
/// valid top-level "name" key, independent of napi.targets' own validity.
#[test]
fn matrix_napi_targets_wrong_type_exits_1() {
    let tmp = base_workspace();
    let dir = tmp.path().join("wrong-type");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("package.json"),
        r#"{"name":"wrong-type","napi":{"targets":"aarch64-apple-darwin"}}"#,
    )
    .unwrap();

    let output = run_matrix_json(tmp.path());
    assert_error_exit_no_report(&output);
}

/// AC-010c sibling (a): [tool.maturin].targets present but a bare string,
/// not an array. The pyproject.toml carries a valid [project].name so it
/// registers on its own via the locator, independent of the
/// package.json-co-location workaround used by the two tests above.
#[test]
fn matrix_maturin_targets_wrong_type_exits_1() {
    let tmp = base_workspace();
    let dir = tmp.path().join("maturin-wrong-type");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("pyproject.toml"),
        "[project]\nname = \"maturin-wrong-type\"\nversion = \"0.1.0\"\n\n[tool.maturin]\ntargets = \"x86_64-unknown-linux-gnu\"\n",
    )
    .unwrap();

    let output = run_matrix_json(tmp.path());
    assert_error_exit_no_report(&output);
}

/// AC-010c sibling (b): engines.node present but a JSON number, not a
/// string. The package.json carries a valid "name" field so it registers
/// on its own.
#[test]
fn matrix_engines_node_wrong_type_exits_1() {
    let tmp = base_workspace();
    let dir = tmp.path().join("engines-wrong-type");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("package.json"),
        r#"{"name":"engines-wrong-type","engines":{"node":20}}"#,
    )
    .unwrap();

    let output = run_matrix_json(tmp.path());
    assert_error_exit_no_report(&output);
}
