use std::process::ExitCode;

use clap::{Command, CommandFactory};
use clap_complete::generate;

use crate::cli::{Cli, CompletionsArgs, GlobalArgs};
use crate::error::CliError;
use crate::output::write_stdout;

pub fn handle(args: CompletionsArgs, _global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let mut source = Cli::command();
    source.build();
    let mut cmd = visible_only(&source);
    // Generated into a buffer, then written through the one fallible sink --
    // `generate` writing straight to stdout would panic on a closed pipe
    // (e.g. `callisto completions bash | head`) instead of exiting quietly.
    let mut buf = Vec::new();
    generate(args.shell, &mut cmd, "callisto", &mut buf);
    write_stdout(&buf)?;
    Ok(ExitCode::SUCCESS)
}

/// Rebuilds `cmd` without its hidden subcommands, recursively -- `clap_complete`'s
/// generators walk `Command::get_subcommands()` unconditionally and do not
/// consult `is_hide_set()`, so a plumbing subcommand hidden from `--help`
/// would otherwise still be offered in shell completions.
fn visible_only(cmd: &Command) -> Command {
    // The source command already carries its built-in --help/--version args
    // (cloned below via `get_arguments`); without disabling auto-injection,
    // clap's `build()` would add a second --help and panic on the duplicate.
    let mut out = Command::new(cmd.get_name().to_owned())
        .disable_help_flag(true)
        .disable_version_flag(true)
        .args(cmd.get_arguments().cloned());
    if let Some(about) = cmd.get_about() {
        out = out.about(about.clone());
    }
    if let Some(version) = cmd.get_version() {
        out = out.version(version.to_owned());
    }
    let visible_subcommands: Vec<Command> = cmd
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set())
        .map(visible_only)
        .collect();
    if !visible_subcommands.is_empty() {
        out = out.subcommands(visible_subcommands);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::ValueEnum;

    use super::*;

    #[test]
    fn handle_succeeds_for_every_supported_shell() {
        let global = GlobalArgs {
            format: crate::cli::OutputFormat::Text,
            cwd: PathBuf::from("."),
            dry_run: false,
        };
        for shell in clap_complete::Shell::value_variants() {
            let result = handle(CompletionsArgs { shell: *shell }, &global);
            assert!(result.is_ok(), "shell={shell:?}");
            assert_eq!(result.unwrap(), ExitCode::SUCCESS);
        }
    }

    #[test]
    fn handle_generates_the_binary_name_into_the_completion_script() {
        let mut cmd = Cli::command();
        let mut buf = Vec::new();
        generate(clap_complete::Shell::Bash, &mut cmd, "callisto", &mut buf);
        let script = String::from_utf8(buf).unwrap();
        assert!(script.contains("callisto"), "got:\n{script}");
    }

    /// Completions must not offer hidden plumbing subcommands, at any nesting
    /// depth -- `clap_complete`'s bash generator walks every subcommand
    /// unconditionally, ignoring `#[command(hide = true)]`, so this exercises
    /// `visible_only`'s own recursive filter rather than clap's generator.
    #[test]
    fn completions_omit_hidden_subcommands_at_every_depth() {
        let mut source = Cli::command();
        source.build();
        let mut cmd = visible_only(&source);
        let mut buf = Vec::new();
        generate(clap_complete::Shell::Bash, &mut cmd, "callisto", &mut buf);
        let script = String::from_utf8(buf).unwrap();
        for hidden in ["matrix", "schema", "compose-pr-body", "release-pr", "artifact-manifest"] {
            assert!(
                !script.contains(hidden),
                "completion script must not mention hidden subcommand {hidden:?}:\n{script}"
            );
        }
        for visible in [
            "add",
            "status",
            "version",
            "pre",
            "snapshot",
            "init",
            "release",
            "completions",
        ] {
            assert!(
                script.contains(visible),
                "completion script must still mention visible subcommand {visible:?}:\n{script}"
            );
        }
    }
}
