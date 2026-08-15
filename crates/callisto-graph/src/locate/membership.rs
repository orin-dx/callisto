use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use std::path::Path;

/// Builds a [`GlobSet`] from a list of glob strings, matched with
/// `literal_separator(true)` semantics (`*` does not cross `/`; `**` does)
/// against forward-slash-normalized, workspace-relative paths.
///
/// Per-entry glob-compile-failure rule: an entry that fails to compile is
/// skipped and treated as never-matching; every other syntactically valid
/// entry in the same list is still compiled and matched normally. Never
/// panics, never returns an error.
#[allow(dead_code)]
pub(crate) fn build_globset(entries: &[String]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for entry in entries {
        if let Ok(glob) = GlobBuilder::new(entry).literal_separator(true).build() {
            builder.add(glob);
        }
    }
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}

#[cfg(test)]
mod tests {
    #[test]
    fn yaml_rust2_smoke_test_parses_a_trivial_mapping() {
        let docs = yaml_rust2::YamlLoader::load_from_str("packages:\n  - \"a\"\n").unwrap();
        assert_eq!(docs.len(), 1);
    }
}

/// Cargo `[workspace]` members/exclude membership filter, computed once per
/// `IgnoreWalkLocator::projects()` call from the workspace root's
/// `Cargo.toml`.
#[allow(dead_code)]
pub(crate) struct CargoMembership {
    members: Option<GlobSet>,
    exclude: GlobSet,
    hybrid_root: bool,
}

impl CargoMembership {
    /// `rel` must be a workspace-relative, forward-slash-normalized path.
    /// `is_root` is true exactly when `rel == Path::new(".")`.
    #[allow(dead_code)]
    pub(crate) fn admits(&self, rel: &Path, is_root: bool) -> bool {
        if is_root && self.hybrid_root {
            return true;
        }
        if self.exclude.is_match(rel) {
            return false;
        }
        match &self.members {
            None => true,
            Some(members) => members.is_match(rel),
        }
    }
}

/// Safely parses a TOML array-of-strings item, returning `None` for any
/// other shape (bare string, table, non-string entries, etc.) instead of
/// panicking.
#[allow(dead_code)]
fn parse_toml_string_array(item: &toml_edit::Item) -> Option<Vec<String>> {
    let arr = item.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr.iter() {
        out.push(v.as_str()?.to_string());
    }
    Some(out)
}

#[allow(dead_code)]
fn absent_cargo_membership() -> CargoMembership {
    CargoMembership {
        members: None,
        exclude: GlobSet::empty(),
        hybrid_root: false,
    }
}

/// NAIVE first pass: only handles (1) Cargo.toml file entirely absent, and
/// (2) a well-formed [workspace] with members/exclude arrays of strings.
/// Every other shape (workspace table absent, TOML unparseable, members/
/// exclude present but not an array-of-strings, members key absent) is
/// NOT yet handled safely -- this deliberately panics or misbehaves on
/// those inputs today; TASK-03b/03c/03d/03e replace these naive lines.
#[allow(dead_code)]
pub(crate) fn read_cargo_membership(root: &Path) -> CargoMembership {
    let content = match std::fs::read_to_string(root.join("Cargo.toml")) {
        Ok(c) => c,
        Err(_) => return absent_cargo_membership(),
    };
    let doc = match content.parse::<toml_edit::DocumentMut>() {
        Ok(d) => d,
        Err(_) => return absent_cargo_membership(),
    };
    let hybrid_root = doc.get("package").is_some() && doc.get("workspace").is_some();
    let Some(workspace) = doc.get("workspace") else {
        return CargoMembership {
            members: None,
            exclude: GlobSet::empty(),
            hybrid_root,
        };
    };
    let members = workspace
        .get("members")
        .and_then(parse_toml_string_array)
        .map(|v| build_globset(&v));
    let exclude = workspace
        .get("exclude")
        .and_then(parse_toml_string_array)
        .map(|v| build_globset(&v))
        .unwrap_or_else(GlobSet::empty);
    CargoMembership {
        members,
        exclude,
        hybrid_root,
    }
}

#[cfg(test)]
mod cargo_membership_tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn read_cargo_membership_admits_all_when_no_cargo_toml_file_exists() {
        let dir = tempdir().unwrap();
        let m = read_cargo_membership(dir.path());
        assert!(m.admits(Path::new("crates/anything"), false));
    }

    #[test]
    fn read_cargo_membership_honors_well_formed_members_and_exclude() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/scratch-example\"]\n",
        )
        .unwrap();
        let m = read_cargo_membership(dir.path());
        assert!(m.admits(Path::new("crates/kept-example"), false));
        assert!(!m.admits(Path::new("crates/scratch-example"), false));
        assert!(!m.admits(Path::new("tools/outside"), false));
    }

    #[test]
    fn read_cargo_membership_falls_back_to_absent_when_members_is_bare_string() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = \"crates/*\"\n",
        )
        .unwrap();
        let m = read_cargo_membership(dir.path());
        assert!(m.admits(Path::new("tools/outside"), false));
    }

    #[test]
    fn read_cargo_membership_falls_back_to_excluding_nothing_when_exclude_is_bare_string() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\nexclude = \"crates/scratch-example\"\n",
        )
        .unwrap();
        let m = read_cargo_membership(dir.path());
        assert!(m.admits(Path::new("crates/scratch-example"), false));
        assert!(!m.admits(Path::new("tools/outside"), false));
    }

    #[test]
    fn read_cargo_membership_detects_hybrid_root_and_exempts_it_from_exclude() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"root-crate\"\nversion = \"0.1.0\"\n\n[workspace]\nmembers = [\"crates/*\"]\nexclude = [\".\"]\n",
        )
        .unwrap();
        let m = read_cargo_membership(dir.path());
        assert!(m.admits(Path::new("."), true));
    }

    #[test]
    fn read_cargo_membership_admits_all_when_workspace_table_absent_but_file_exists() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"solo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let m = read_cargo_membership(dir.path());
        assert!(m.admits(Path::new("crates/anything"), false));
    }

    #[test]
    fn read_cargo_membership_falls_back_to_absent_when_root_toml_is_unparseable() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace\nmembers = [\n").unwrap();
        let m = read_cargo_membership(dir.path());
        assert!(m.admits(Path::new("crates/anything"), false));
    }

    #[test]
    fn read_cargo_membership_honors_exclude_when_no_members_key_present() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nexclude = [\"crates/foo\"]\n",
        )
        .unwrap();
        let m = read_cargo_membership(dir.path());
        assert!(!m.admits(Path::new("crates/foo"), false));
        assert!(m.admits(Path::new("crates/kept"), false));
    }

    #[test]
    fn read_cargo_membership_empty_members_array_admits_nothing() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        let m = read_cargo_membership(dir.path());
        assert!(!m.admits(Path::new("crates/anything"), false));
    }
}

#[allow(dead_code)]
pub(crate) struct NpmMembership {
    globs: Option<GlobSet>,
    hybrid_root: bool,
}

impl NpmMembership {
    /// `rel` must be a workspace-relative, forward-slash-normalized path.
    /// `is_root` is true exactly when `rel == Path::new(".")`.
    #[allow(dead_code)]
    pub(crate) fn admits(&self, rel: &Path, is_root: bool) -> bool {
        if is_root && self.hybrid_root {
            return true;
        }
        match &self.globs {
            None => true,
            Some(globs) => globs.is_match(rel),
        }
    }
}

/// True when the root package.json exists, parses as JSON, and declares a
/// "name" field. Independent of which arm (package.json "workspaces" or a
/// sibling pnpm-workspace.yaml) governs the rest of npm membership -- see
/// AC-16/AC-16b/AC-17/AC-10d.
#[allow(dead_code)]
fn package_json_declares_name(root: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(root.join("package.json")) else {
        return false;
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    val.get("name").is_some()
}

#[allow(dead_code)]
pub(crate) fn read_npm_membership(root: &Path) -> NpmMembership {
    let hybrid_root = package_json_declares_name(root);
    if let Some(packages) = read_pnpm_packages(root) {
        return NpmMembership {
            globs: Some(build_globset(&packages)),
            hybrid_root,
        };
    }
    NpmMembership {
        globs: read_package_json_workspaces(root).map(|v| build_globset(&v)),
        hybrid_root,
    }
}

/// YAML-parse-Err, zero-documents, packages-key-missing-or-wrong-shape, and
/// a non-string sequence element all safely fall back to `None` (which
/// causes `read_npm_membership` to fall through to
/// `read_package_json_workspaces` or admit-all).
#[allow(dead_code)]
fn read_pnpm_packages(root: &Path) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(root.join("pnpm-workspace.yaml")).ok()?;
    let docs = yaml_rust2::YamlLoader::load_from_str(&content).ok()?;
    let doc = docs.first()?;
    let packages = doc["packages"].as_vec()?;
    let mut out = Vec::with_capacity(packages.len());
    for item in packages {
        out.push(item.as_str()?.to_string());
    }
    Some(out)
}

/// File-absent, field-absent, non-array "workspaces", and non-string array
/// entries all safely fall back to `None` (a plain `?`-chain), which causes
/// `read_npm_membership` to admit-all.
#[allow(dead_code)]
fn read_package_json_workspaces(root: &Path) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(root.join("package.json")).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    let field = val.get("workspaces")?;
    let arr = field.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        out.push(v.as_str()?.to_string());
    }
    Some(out)
}

#[cfg(test)]
mod npm_membership_tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn read_npm_membership_admits_all_when_no_package_json_file_exists() {
        let dir = tempdir().unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_admits_all_when_package_json_present_but_no_workspaces_field() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"name": "root"}"#).unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_honors_well_formed_package_json_workspaces() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"workspaces": ["packages/*"]}"#,
        )
        .unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("packages/kept"), false));
        assert!(!m.admits(Path::new("tools/outside"), false));
    }

    #[test]
    fn read_npm_membership_empty_workspaces_array_admits_nothing() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"workspaces": []}"#).unwrap();
        let m = read_npm_membership(dir.path());
        assert!(!m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_prefers_pnpm_packages_over_package_json_workspaces() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"workspaces": ["packages/*"]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages:\n  - \"tools/*\"\n",
        )
        .unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("tools/x"), false));
        assert!(!m.admits(Path::new("packages/y"), false));
    }

    #[test]
    fn read_npm_membership_pnpm_empty_packages_list_admits_nothing() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "packages: []\n").unwrap();
        let m = read_npm_membership(dir.path());
        assert!(!m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_falls_back_to_absent_when_pnpm_yaml_is_malformed_and_no_package_json_workspaces(
    ) {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: [\"packages/*\"\n",
        )
        .unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_falls_back_to_package_json_when_pnpm_yaml_is_malformed_and_package_json_workspaces_present(
    ) {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"workspaces": ["packages/*"]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: [\"packages/*\"\n",
        )
        .unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("packages/y"), false));
    }

    #[test]
    fn read_npm_membership_falls_back_to_absent_when_pnpm_yaml_has_zero_documents() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "# no packages here\n",
        )
        .unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_falls_back_to_absent_when_pnpm_packages_key_missing_or_wrong_scalar_type(
    ) {
        let dir_missing = tempdir().unwrap();
        std::fs::write(
            dir_missing.path().join("pnpm-workspace.yaml"),
            "other_key: true\n",
        )
        .unwrap();
        let m_missing = read_npm_membership(dir_missing.path());
        assert!(m_missing.admits(Path::new("packages/anything"), false));

        let dir_wrong_type = tempdir().unwrap();
        std::fs::write(
            dir_wrong_type.path().join("pnpm-workspace.yaml"),
            "packages: \"packages/*\"\n",
        )
        .unwrap();
        let m_wrong_type = read_npm_membership(dir_wrong_type.path());
        assert!(m_wrong_type.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_falls_back_to_absent_when_workspaces_is_bare_string() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"workspaces": "packages/*"}"#,
        )
        .unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("tools/outside"), false));
    }

    #[test]
    fn read_npm_membership_falls_back_to_absent_when_workspaces_array_has_non_string_entry() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"workspaces": ["packages/*", 42]}"#,
        )
        .unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("tools/outside"), false));
    }

    #[test]
    fn read_npm_membership_falls_back_to_absent_when_pnpm_packages_sequence_has_non_string_element()
    {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages:\n  - \"a\"\n  - 42\n",
        )
        .unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_falls_back_to_absent_when_root_package_json_syntax_is_invalid() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            "{\"workspaces\": [\"packages/*\"],}",
        )
        .unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_hybrid_root_is_admitted_even_when_workspaces_glob_does_not_match_root() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name": "root-package", "workspaces": ["packages/*"]}"#,
        )
        .unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("."), true));
    }
}

#[cfg(test)]
mod glob_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn build_globset_skips_invalid_entry_and_matches_valid_entry() {
        let entries = vec![
            "crates/kept-example".to_string(),
            "crates/[unterminated".to_string(),
        ];
        let set = build_globset(&entries);
        assert!(
            set.is_match(Path::new("crates/kept-example")),
            "well-formed entry must still match"
        );
        assert!(
            !set.is_match(Path::new("crates/[unterminated")),
            "an invalid glob source string must never match anything"
        );
    }

    #[test]
    fn build_globset_of_all_invalid_entries_matches_nothing_without_panicking() {
        let entries = vec!["crates/[unterminated".to_string()];
        let set = build_globset(&entries);
        assert!(!set.is_match(Path::new("crates/[unterminated")));
    }

    #[test]
    fn build_globset_literal_separator_true_means_star_does_not_cross_slash_but_double_star_does() {
        let single_star = build_globset(&["crates/*".to_string()]);
        assert!(
            single_star.is_match(Path::new("crates/a")),
            "* must match one segment"
        );
        assert!(
            !single_star.is_match(Path::new("crates/a/b")),
            "* must NOT cross a path separator -- this is what literal_separator(true) guarantees"
        );

        let double_star = build_globset(&["crates/**".to_string()]);
        assert!(
            double_star.is_match(Path::new("crates/a/b")),
            "** must match zero or more full path segments recursively"
        );
    }
}

#[cfg(test)]
mod ac09e_probe {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn probe_unparseable_package_json_before_04e() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            "{\"workspaces\": [\"packages/*\"],}",
        )
        .unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(std::path::Path::new("packages/anything"), false));
    }

    #[test]
    fn probe_pnpm_non_string_seq_before_04e() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages:\n  - \"a\"\n  - 42\n",
        )
        .unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(std::path::Path::new("packages/anything"), false));
    }
}
