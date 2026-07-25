use std::path::Path;

use callisto_model::{check_git_version, CommandError, CommandRunner};

pub fn probe_git<R: CommandRunner>(runner: &R, cwd: &Path) -> Result<(), CommandError> {
    let output = runner.run("git", &["--version"], cwd)?;
    if !output.success() {
        return Err(CommandError::NotFound {
            program: "git".to_string(),
        });
    }
    check_git_version(output.stdout_trimmed())
}
