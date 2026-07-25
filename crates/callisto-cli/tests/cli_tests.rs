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
