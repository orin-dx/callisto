//! `callisto init`'s reading of an existing Changesets `.changeset/config.json`.

use std::path::{Path, PathBuf};

use callisto_model::{BranchName, Diagnostic, DiagnosticCode, DiagnosticSeverity, Ecosystem};
use globset::GlobBuilder;
use serde_json::{Map, Value};

use super::{InitAnswers, InitFacts, Versioning};

/// One `[[fixed-group]]` carried over from a Changesets `fixed` group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixedGroup {
    pub name: String,
    /// Qualified package ids.
    pub members: Vec<String>,
}

/// What a Changesets config means for `callisto init`.
#[derive(Clone, Debug, Default)]
pub struct Migration {
    path: PathBuf,
    fixed_groups: Vec<FixedGroup>,
    /// `fixed` members that matched no npm package.
    fixed_unmatched: Vec<String>,
    /// `baseBranch`, which becomes the generated workflow's branch filter.
    pub base_branch: Option<BranchName>,
    /// Scoped npm packages that would publish as public.
    pub public_scoped: Vec<String>,
    /// One warning per key `init` does not carry over.
    pub diagnostics: Vec<Diagnostic>,
}

/// A top-level `config.json` key, so each one is handled in one exhaustive match.
enum Key<'a> {
    Fixed,
    Linked,
    Ignore,
    Access,
    BaseBranch,
    UpdateInternalDependencies,
    Schema,
    Other(&'a str),
}

impl<'a> From<&'a str> for Key<'a> {
    fn from(key: &'a str) -> Self {
        match key {
            "fixed" => Self::Fixed,
            "linked" => Self::Linked,
            "ignore" => Self::Ignore,
            "access" => Self::Access,
            "baseBranch" => Self::BaseBranch,
            "updateInternalDependencies" => Self::UpdateInternalDependencies,
            "$schema" => Self::Schema,
            other => Self::Other(other),
        }
    }
}

fn config_path(root: &Path) -> PathBuf {
    root.join(".changeset/config.json")
}

/// The config's top-level object; `None` when the file is absent, unreadable, or not a JSON object.
fn read(root: &Path) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(config_path(root)).ok()?;
    match serde_json::from_str(&text) {
        Ok(Value::Object(config)) => Some(config),
        _ => None,
    }
}

fn branch(value: &Value) -> Option<BranchName> {
    BranchName::from_full_ref(&format!("refs/heads/{}", value.as_str()?))
}

/// Reads `root`'s Changesets config against the detected packages.
pub fn migrate(root: &Path, facts: &InitFacts) -> Migration {
    let Some(config) = read(root) else {
        return Migration::default();
    };
    let mut migration = Migration {
        path: config_path(root),
        ..Migration::default()
    };
    for (key, value) in &config {
        let nothing_to_drop = value.as_array().is_some_and(Vec::is_empty);
        match Key::from(key.as_str()) {
            Key::Fixed => migration.translate_fixed(value, facts),
            Key::Linked if nothing_to_drop => {}
            Key::Linked => migration.warn(
                "linked",
                "is not translated: callisto bumps every member of a linked group on each release, Changesets only \
                 the members with changesets; write a `[[linked-group]]` by hand to get that",
            ),
            Key::Ignore if nothing_to_drop => {}
            Key::Ignore => migration.warn(
                "ignore",
                "is not translated: ignored packages are still versioned and released; set `publish-to = [\"none\"]` \
                 on a package to stop its publishing",
            ),
            Key::Access => {}
            Key::BaseBranch => match branch(value) {
                Some(base) => migration.base_branch = Some(base),
                None => migration.warn("baseBranch", "is not a branch name and is ignored"),
            },
            Key::UpdateInternalDependencies => migration.warn(
                "updateInternalDependencies",
                "is not honoured; callisto always raises the ranges of dependents released with the dependency",
            ),
            Key::Schema => {}
            Key::Other(other) => migration.warn(other, "is not honoured; callisto has no equivalent"),
        }
    }
    // Changesets publishes scoped packages as restricted unless `access` is `public`.
    if config.get("access").and_then(Value::as_str) != Some("public") {
        migration.public_scoped = facts
            .packages
            .iter()
            .filter(|package| !package.platform && package.publishes_public_by_default())
            .map(|package| package.id.clone())
            .collect();
        if !migration.public_scoped.is_empty() {
            let packages = migration.public_scoped.join(", ");
            migration.warn(
                "access",
                &format!(
                    "is restricted or absent, so Changesets would publish scoped packages privately; callisto \
                     publishes these as public: {packages}. Set `publishConfig.access` in each package.json"
                ),
            );
        }
    }
    migration
}

impl Migration {
    fn warn(&mut self, key: &str, message: &str) {
        self.diagnostics.push(self.warning(format!("`{key}` {message}")));
    }

    fn warning(&self, message: String) -> Diagnostic {
        Diagnostic {
            code: DiagnosticCode::ChangesetsConfigKeyDropped,
            severity: DiagnosticSeverity::Warning,
            message,
            package: None,
            path: Some(self.path.clone()),
            escalated_by: None,
            governed_by: None,
        }
    }

    fn translate_fixed(&mut self, value: &Value, facts: &InitFacts) {
        let Ok(groups) = serde_json::from_value::<Vec<Vec<String>>>(value.clone()) else {
            self.warn("fixed", "is not a list of lists of package names and is not translated");
            return;
        };
        let npm: Vec<(&str, &str)> = facts
            .packages
            .iter()
            .filter(|package| !package.platform)
            .filter_map(|package| {
                let name = package.id.strip_prefix(Ecosystem::Npm.prefix())?.strip_prefix('/')?;
                Some((name, package.id.as_str()))
            })
            .collect();
        for (index, patterns) in groups.iter().enumerate() {
            let mut members: Vec<String> = Vec::new();
            for pattern in patterns {
                let matched: Vec<&str> = match GlobBuilder::new(pattern).literal_separator(true).build() {
                    Ok(glob) => {
                        let matcher = glob.compile_matcher();
                        npm.iter()
                            .filter(|(name, _)| matcher.is_match(name))
                            .map(|(_, id)| *id)
                            .collect()
                    }
                    Err(_) => Vec::new(),
                };
                if matched.is_empty() {
                    self.fixed_unmatched.push(pattern.clone());
                }
                for id in matched {
                    if !members.iter().any(|member| member == id) {
                        members.push(id.to_owned());
                    }
                }
            }
            if !members.is_empty() {
                self.fixed_groups.push(FixedGroup {
                    name: format!("fixed-{}", index + 1),
                    members,
                });
            }
        }
    }

    /// Puts the carried-over `fixed` groups into `answers` and returns the warning that says what became of them.
    pub fn apply_fixed_groups(&self, answers: &mut InitAnswers) -> Option<Diagnostic> {
        if self.fixed_groups.is_empty() && self.fixed_unmatched.is_empty() {
            return None;
        }
        let unmatched = if self.fixed_unmatched.is_empty() {
            String::new()
        } else {
            let patterns: Vec<String> = self
                .fixed_unmatched
                .iter()
                .map(|pattern| format!("`{pattern}`"))
                .collect();
            format!(" No npm package matches {}.", patterns.join(", "))
        };
        match answers.versioning {
            Versioning::Fixed => Some(self.warning(format!(
                "`fixed` is not translated: `--versioning fixed` already puts every package in one group.{unmatched}"
            ))),
            Versioning::Independent if self.fixed_groups.is_empty() => {
                Some(self.warning(format!("`fixed` is not translated.{unmatched}")))
            }
            Versioning::Independent => {
                answers.fixed_groups = self.fixed_groups.clone();
                let names: Vec<&str> = self.fixed_groups.iter().map(|group| group.name.as_str()).collect();
                Some(self.warning(format!(
                    "`fixed` became `[[fixed-group]]` ({}), with each glob expanded to the npm packages that exist now: \
                     add packages created later by hand.{unmatched}",
                    names.join(", ")
                )))
            }
        }
    }

    /// The warning for a `baseBranch` that no generated workflow consumes.
    pub fn base_branch_unused(&self) -> Option<Diagnostic> {
        let base = self.base_branch.as_ref()?;
        Some(self.warning(format!(
            "`baseBranch` `{base}` only sets the generated workflow's branch filter, and no workflow was generated; \
             run `callisto init --workflow` to generate one"
        )))
    }
}

/// `baseBranch` of `root`'s Changesets config, for a workspace that already has a `callisto.toml`.
pub fn base_branch(root: &Path) -> Option<BranchName> {
    branch(read(root)?.get("baseBranch")?)
}

#[cfg(test)]
mod tests;
