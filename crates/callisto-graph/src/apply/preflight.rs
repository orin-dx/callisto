//! Proves, before the first write, that every file a version plan touches can be written and committed, so a bad
//! target stops the run with nothing changed instead of leaving earlier manifests rewritten.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use callisto_model::ApplyPermit;

use super::ApplyOptions;
use crate::cascade::DepWriteTarget;
use crate::error::GraphError;
use crate::plan::{VersionPlan, VersionWriteTarget};

/// What a plan does to a path, which decides what must hold for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Target {
    /// A manifest the plan edits: it must exist as a readable regular file and its directory must take a new file.
    Existing,
    /// A file the plan may create or replace, such as a changelog or `pre.json`.
    Creatable,
    /// A file the plan deletes: only the path itself is checked.
    Removed,
}

/// Every path `plan` writes or deletes under `opts`, once each.
fn targets(plan: &VersionPlan, opts: &ApplyOptions) -> BTreeSet<(PathBuf, Target)> {
    let mut targets = BTreeSet::new();
    for bump in &plan.bumps {
        for write in &bump.writes {
            let path = match write {
                VersionWriteTarget::Manifest(path) => path,
                VersionWriteTarget::CargoWorkspacePackage { root_manifest } => root_manifest,
            };
            targets.insert((path.clone(), Target::Existing));
        }
    }
    for rewrite in &plan.rewrites {
        let path = match &rewrite.key.target {
            DepWriteTarget::Manifest(path) => path,
            DepWriteTarget::CargoWorkspaceDependency { root_manifest } => root_manifest,
        };
        targets.insert((path.clone(), Target::Existing));
    }
    targets.extend(
        plan.platform_writes
            .iter()
            .map(|write| (write.manifest.clone(), Target::Existing)),
    );
    targets.extend(
        plan.optional_dep_updates
            .iter()
            .map(|update| (update.manifest.clone(), Target::Existing)),
    );
    if !opts.transient {
        targets.extend(
            plan.changelog_writes
                .iter()
                .map(|write| (write.changelog_path.clone(), Target::Creatable)),
        );
        targets.extend(
            plan.consumed_changesets
                .iter()
                .map(|path| (path.clone(), Target::Removed)),
        );
        if plan.pre_state_update.is_some() {
            targets.insert((plan.pre_json_path.clone(), Target::Creatable));
        } else if let Some(path) = &plan.delete_pre_json {
            targets.insert((path.clone(), Target::Removed));
        }
    }
    targets
}

/// Fails with every target that cannot be written, before anything is.
///
/// # Errors
///
/// [`GraphError::ApplyPreflight`] naming each unwritable path and why.
pub(super) fn check(
    root: &Path,
    plan: &VersionPlan,
    opts: &ApplyOptions,
    permit: &ApplyPermit,
) -> Result<(), GraphError> {
    let problems: Vec<String> = targets(plan, opts)
        .into_iter()
        .filter_map(|(path, target)| {
            problem(root, &path, target, permit).map(|reason| format!("`{}`: {reason}", path.display()))
        })
        .collect();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(GraphError::ApplyPreflight { problems })
    }
}

fn problem(root: &Path, relative: &Path, target: Target, permit: &ApplyPermit) -> Option<String> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
    {
        return Some("is outside the workspace".to_string());
    }
    // Git cannot stage a path through a link, and replacing a link with a file would change what the repo records.
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Some("is, or sits beneath, a symbolic link, which Git cannot stage".to_string());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Some(format!("cannot be inspected: {error}")),
        }
    }
    if target == Target::Removed {
        return None;
    }

    let path = root.join(relative);
    match std::fs::metadata(&path) {
        Ok(metadata) if !metadata.is_file() => return Some("is not a regular file".to_string()),
        Ok(_) => {
            if let Err(error) = std::fs::File::open(&path) {
                return Some(format!("cannot be read: {error}"));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if target == Target::Existing {
                return Some("does not exist".to_string());
            }
        }
        Err(error) => return Some(format!("cannot be inspected: {error}")),
    }
    callisto_model::atomic::probe_atomic_write(&path, permit)
        .err()
        .map(|error| format!("cannot be written: {error}"))
}

#[cfg(test)]
mod tests;
