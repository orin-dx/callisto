use std::path::Path;

use callisto_model::{CommandRunner, CommitSha, PackageId};

use crate::ConventionalError;

pub fn pre_cursor_ref_name(package: &PackageId) -> String {
    format!("refs/callisto/pre-cursor/{}", package.display_name())
}

pub fn resolve_pre_cursor(
    runner: &dyn CommandRunner,
    cwd: &Path,
    package: &PackageId,
) -> Result<Option<CommitSha>, ConventionalError> {
    let ref_name = pre_cursor_ref_name(package);
    let output = runner.run("git", &["rev-parse", "--verify", "--quiet", &ref_name], cwd)?;

    if !output.success() {
        return Ok(None);
    }

    let sha_str = output.stdout_trimmed();
    if sha_str.is_empty() {
        return Ok(None);
    }

    let sha = CommitSha::parse(sha_str).map_err(|_| ConventionalError::MalformedPreCursorRef {
        cwd: cwd.to_path_buf(),
        ref_name,
        stderr: output.stderr,
    })?;

    Ok(Some(sha))
}

pub fn advance_pre_cursor(
    runner: &dyn CommandRunner,
    cwd: &Path,
    package: &PackageId,
    sha: &CommitSha,
) -> Result<(), ConventionalError> {
    let ref_name = pre_cursor_ref_name(package);
    let output = runner.run("git", &["update-ref", &ref_name, sha.as_str()], cwd)?;

    if !output.success() {
        return Err(ConventionalError::PreCursorAdvanceFailed {
            cwd: cwd.to_path_buf(),
            ref_name,
            sha: sha.as_str().to_string(),
            stderr: output.stderr,
        });
    }

    Ok(())
}
