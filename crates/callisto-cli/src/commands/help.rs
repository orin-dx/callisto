use std::process::ExitCode;

use clap::CommandFactory;

use crate::cli::{Cli, GlobalArgs, HelpArgs};
use crate::error::CliError;

/// Prints the root help, or one subcommand's help when `args.command` names a
/// path -- clap's own `disable_help_subcommand` only removes its automatic
/// `help` subcommand (kept off so `--help`'s eight-command listing stays
/// exactly that); this hidden subcommand fills the same role explicitly.
pub fn handle(args: HelpArgs, _global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let mut cmd = Cli::command();
    cmd.build();

    let mut target = &mut cmd;
    for name in &args.command {
        target = target
            .find_subcommand_mut(name)
            .ok_or_else(|| CliError::HelpUnknownCommand { name: name.clone() })?;
    }

    target.print_help()?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn global() -> GlobalArgs {
        GlobalArgs {
            format: crate::cli::OutputFormat::Text,
            cwd: std::path::PathBuf::from("."),
            dry_run: false,
        }
    }

    #[test]
    fn handle_prints_root_help_with_no_command() {
        let result = handle(HelpArgs { command: vec![] }, &global());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }

    #[test]
    fn handle_prints_subcommand_help() {
        let result = handle(
            HelpArgs {
                command: vec!["status".to_string()],
            },
            &global(),
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }

    #[test]
    fn handle_prints_nested_subcommand_help() {
        let result = handle(
            HelpArgs {
                command: vec!["pre".to_string(), "enter".to_string()],
            },
            &global(),
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }

    #[test]
    fn handle_names_unknown_subcommand() {
        let err = handle(
            HelpArgs {
                command: vec!["bogus".to_string()],
            },
            &global(),
        )
        .unwrap_err();
        match err {
            CliError::HelpUnknownCommand { name } => assert_eq!(name, "bogus"),
            other => panic!("expected CliError::HelpUnknownCommand, got: {other:?}"),
        }
    }
}
