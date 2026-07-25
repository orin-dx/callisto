use std::path::Path;

use callisto_model::{CommandError, CommandOutput, CommandRunner};

pub struct MoonCommandRunner;

impl CommandRunner for MoonCommandRunner {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
        let mut cmd = std::process::Command::new(program);
        cmd.args(args);
        cmd.current_dir(cwd);

        match cmd.output() {
            Ok(output) => Ok(CommandOutput {
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            }),
            Err(e) => Err(classify_host_failure(program, &e.to_string())),
        }
    }
}

pub fn classify_host_failure(program: &str, message: &str) -> CommandError {
    if looks_like_not_found(message) {
        CommandError::NotFound {
            program: program.to_string(),
        }
    } else {
        CommandError::Io {
            program: program.to_string(),
            message: message.to_string(),
        }
    }
}

fn looks_like_not_found(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("not found") || lower.contains("no such file")
}
