use std::path::{Path, PathBuf};

use callisto_model::{CommandRunner, Package};

use crate::error::GraphError;
use crate::tags::TagIndex;

pub fn package_paths(pkg: &Package) -> Vec<PathBuf> {
    let mut set = std::collections::BTreeSet::new();
    for m in &pkg.manifests {
        if let Some(parent) = m.path.parent() {
            if parent.as_os_str().is_empty() {
                set.insert(PathBuf::from("."));
            } else {
                set.insert(parent.to_path_buf());
            }
        }
    }
    set.into_iter().collect()
}

pub fn changed_since_last_tag<R: CommandRunner>(
    runner: &R,
    root: &Path,
    pkg: &Package,
    tags: &TagIndex,
) -> Result<bool, GraphError> {
    let Some(last) = tags.last_tag(&pkg.id) else {
        return Ok(true);
    };

    if let Ok(repo) = callisto_vcs::GitRepository::discover(root) {
        if let Ok(commits) = repo.commits_since(Some(&last.name.0)) {
            if !commits.is_empty() {
                return Ok(true);
            }
        }
    }

    let paths = package_paths(pkg);
    let mut args = vec!["diff", "--quiet", last.name.as_str(), "--"];
    let path_strs: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    for p in &path_strs {
        args.push(p);
    }

    let output = runner.run("git", &args, root)?;
    Ok(!output.success())
}
