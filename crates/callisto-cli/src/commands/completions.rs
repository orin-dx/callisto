use std::io;
use std::process::ExitCode;

use clap::CommandFactory;
use clap_complete::generate;

use crate::cli::{Cli, CompletionsArgs, GlobalArgs};
use crate::error::CliError;

pub fn handle(args: CompletionsArgs, _global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let mut cmd = Cli::command();
    generate(args.shell, &mut cmd, "callisto", &mut io::stdout());
    Ok(ExitCode::SUCCESS)
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
}
