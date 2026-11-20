//! `callisto init`'s reading of an existing Changesets `.changeset/config.json`.

use std::path::{Path, PathBuf};

use callisto_model::{BranchName, Diagnostic, DiagnosticCode, DiagnosticSeverity};
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

/// Whether `pattern` means the same to a plain glob as to Changesets' micromatch: names, `*` and `?` only.
fn is_plain(pattern: &str) -> bool {
    !pattern.contains(['!', '{', '[', '('])
}

/// The primary ids of the packages whose npm name `pattern` matches.
fn matching<'a>(pattern: &str, npm: &[(&str, &'a str)]) -> Vec<&'a str> {
    let Ok(glob) = GlobBuilder::new(pattern).literal_separator(true).build() else {
        return Vec::new();
    };
    let matcher = glob.compile_matcher();
    npm.iter()
        .filter(|(name, _)| matcher.is_match(name))
        .map(|(_, id)| *id)
        .collect()
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
        match key.as_str() {
            "fixed" => migration.translate_fixed(value, facts),
            "linked" if nothing_to_drop => {}
            "linked" => migration.warn(
                "linked",
                "is not translated: callisto bumps every member of a linked group on each release, Changesets only \
                 the members with changesets; write a `[[linked-group]]` by hand to get that",
            ),
            "ignore" if nothing_to_drop => {}
            "ignore" => migration.warn(
                "ignore",
                "is not translated: ignored packages are still versioned and released; set `publish-to = [\"none\"]` \
                 on a package to stop its publishing",
            ),
            // `access` is checked below; `$schema` is metadata.
            "access" | "$schema" => {}
            "baseBranch" => match branch(value) {
                Some(base) => migration.base_branch = Some(base),
                None => migration.warn("baseBranch", "is not a branch name and is ignored"),
            },
            "updateInternalDependencies" => migration.warn(
                "updateInternalDependencies",
                "is not honoured; callisto always raises the ranges of dependents released with the dependency",
            ),
            other => migration.warn(other, "is not honoured; callisto has no equivalent"),
        }
    }
    // Changesets publishes scoped packages as restricted unless `access` is `public`.
    if config.get("access").and_then(Value::as_str) != Some("public") {
        migration.public_scoped = facts
            .packages
            .iter()
            .filter(|package| !package.platform && package.publishes_public_by_default())
            .filter_map(|package| Some(format!("npm/{}", package.npm_name.as_deref()?)))
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
        // Every name in a Changesets config is an npm package name, whatever the package's primary id.
        let npm: Vec<(&str, &str)> = facts
            .packages
            .iter()
            .filter(|package| !package.platform)
            .filter_map(|package| Some((package.npm_name.as_deref()?, package.id.as_str())))
            .collect();
        for (index, patterns) in groups.iter().enumerate() {
            let name = format!("fixed-{}", index + 1);
            if !patterns.iter().all(|pattern| is_plain(pattern)) {
                self.hold_back(&name, patterns, &npm);
                continue;
            }
            let mut members: Vec<String> = Vec::new();
            for pattern in patterns {
                let matched = matching(pattern, &npm);
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
                self.fixed_groups.push(FixedGroup { name, members });
            }
        }
    }

    /// Warns that a group is not written, with the block to paste once its members are reviewed.
    fn hold_back(&mut self, name: &str, patterns: &[String], npm: &[(&str, &str)]) {
        let excluded: Vec<&str> = patterns
            .iter()
            .filter_map(|pattern| pattern.strip_prefix('!'))
            .flat_map(|pattern| matching(pattern, npm))
            .collect();
        let included: Vec<&str> = if patterns.iter().all(|pattern| pattern.starts_with('!')) {
            npm.iter().map(|(_, id)| *id).collect()
        } else {
            patterns
                .iter()
                .filter(|pattern| !pattern.starts_with('!'))
                .flat_map(|pattern| matching(pattern, npm))
                .collect()
        };
        let mut members: Vec<&str> = Vec::new();
        for id in included.into_iter().filter(|id| !excluded.contains(id)) {
            if !members.contains(&id) {
                members.push(id);
            }
        }
        members.sort_unstable();
        let listed: Vec<String> = members.iter().map(|member| format!("\"{member}\"")).collect();
        let shown: Vec<String> = patterns.iter().map(|pattern| format!("`{pattern}`")).collect();
        self.warn(
            "fixed",
            &format!(
                "group {} ({}) uses patterns that Changesets matches differently from a plain glob, so it is not \
                 written. Check these members, then paste the block into callisto.toml:\n[[fixed-group]]\nname = \
                 \"{name}\"\nmembers = [{}]",
                name.trim_start_matches("fixed-"),
                shown.join(", "),
                listed.join(", ")
            ),
        );
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
