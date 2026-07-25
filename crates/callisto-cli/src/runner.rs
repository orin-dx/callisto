use std::path::Path;
use std::process::Stdio;

use callisto_model::{CommandError, CommandOutput, CommandRunner};

pub struct CliCommandRunner;

impl CommandRunner for CliCommandRunner {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
        let output = std::process::Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();

        match output {
            Ok(o) => {
                let out = CommandOutput {
                    exit_code: o.status.code(),
                    stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
                };
                if !out.stderr.is_empty() {
                    eprint!("{}", out.stderr);
                }
                Ok(out)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CommandError::NotFound {
                program: program.to_string(),
            }),
            Err(e) => Err(CommandError::Io {
                program: program.to_string(),
                message: e.to_string(),
            }),
        }
    }
}
