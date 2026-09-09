use callisto_model::ManifestFormat;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use std::path::Path;

/// A [`GlobSet`] pair implementing npm/pnpm/Yarn-style workspace glob
/// negation. `globset`'s `GlobSetBuilder` has no negation concept of its
/// own -- a pattern with a leading `!` is just another literal
/// positive-match glob to it, so `!packages/excluded` compiles into a glob
/// that matches paths literally starting with the character `!`, which
/// never matches any real path. That silently no-ops the negation instead
/// of excluding anything.
///
/// A path is admitted only when it matches at least one `positive` pattern
/// and matches none of the `negative` (`!`-stripped) patterns -- mirroring
/// how npm/pnpm/Yarn actually resolve workspace globs (later patterns can
/// negate earlier ones; a leading `!` excludes). A positive-only pattern
/// list behaves exactly as a plain [`GlobSet`] would. An all-negative list
/// has an empty positive set, so `is_match` is always `false` -- there is no
/// positive baseline for the negation to carve an exclusion out of.
pub(crate) struct NegatableGlobSet {
    positive: GlobSet,
    negative: GlobSet,
}

impl NegatableGlobSet {
    pub(crate) fn is_match(&self, path: &Path) -> bool {
        self.positive.is_match(path) && !self.negative.is_match(path)
    }

    pub(crate) fn empty() -> Self {
        NegatableGlobSet {
            positive: GlobSet::empty(),
            negative: GlobSet::empty(),
        }
    }
}

/// Builds a [`NegatableGlobSet`] from a list of glob strings, matched with
/// `literal_separator(true)` semantics (`*` does not cross `/`; `**` does)
/// against forward-slash-normalized, workspace-relative paths. An entry
/// with a leading `!` is treated as a negative (exclusion) pattern -- see
/// [`NegatableGlobSet`].
///
/// Per-entry glob-compile-failure rule: an entry that fails to compile is
/// skipped and treated as never-matching; every other syntactically valid
/// entry in the same list is still compiled and matched normally. Never
/// panics, never returns an error.
pub(crate) fn build_globset(entries: &[String]) -> NegatableGlobSet {
    let mut positive = GlobSetBuilder::new();
    let mut negative = GlobSetBuilder::new();
    for entry in entries {
        let (builder, pattern) = match entry.strip_prefix('!') {
            Some(rest) => (&mut negative, rest),
            None => (&mut positive, entry.as_str()),
        };
        if let Ok(glob) = GlobBuilder::new(pattern).literal_separator(true).build() {
            builder.add(glob);
        }
    }
    NegatableGlobSet {
        positive: positive.build().unwrap_or_else(|_| GlobSet::empty()),
        negative: negative.build().unwrap_or_else(|_| GlobSet::empty()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn yaml_rust2_smoke_test_parses_a_trivial_mapping() {
        let docs = yaml_rust2::YamlLoader::load_from_str("packages:\n  - \"a\"\n").unwrap();
        assert_eq!(docs.len(), 1);
    }
}

/// Workspace-membership filter shared by Cargo's `[workspace]` and uv's
/// `[tool.uv.workspace]` -- both use the identical members/exclude
/// glob-array shape, including a root manifest that is itself both a
/// package/project and a workspace root (hybrid root). `NpmMembership`
/// deliberately does NOT go through this type: npm's shape genuinely
/// differs (JSON not TOML, no `exclude` concept, and a pnpm-workspace.yaml
/// fallback with its own precedence rules).
pub(crate) struct Membership {
    members: Option<NegatableGlobSet>,
    exclude: NegatableGlobSet,
    hybrid_root: bool,
}

impl Membership {
    /// `rel` must be a workspace-relative, forward-slash-normalized path.
    /// `is_root` is true exactly when `rel == Path::new(".")`.
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

    fn absent() -> Self {
        Membership {
            members: None,
            exclude: NegatableGlobSet::empty(),
            hybrid_root: false,
        }
    }
}

/// Safely parses a TOML array-of-strings item, returning `None` for any
/// other shape (bare string, table, non-string entries, etc.) instead of
/// panicking.
fn parse_toml_string_array(item: &toml_edit::Item) -> Option<Vec<String>> {
    let arr = item.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr.iter() {
        out.push(v.as_str()?.to_string());
    }
    Some(out)
}

/// Describes where an ecosystem's workspace-membership table lives inside
/// its root manifest, so [`read_membership`] can walk to it generically
/// instead of each ecosystem hand-rolling the same members/exclude/
/// hybrid-root logic around a different table path.
pub(crate) struct MembershipSpec {
    /// The root manifest's file name, e.g. `Cargo.toml`.
    manifest_file_name: &'static str,
    /// Nested table keys leading to the workspace table, walked in order
    /// from the document root, e.g. `["workspace"]` for Cargo or
    /// `["tool", "uv", "workspace"]` for uv-based Python.
    table_path: &'static [&'static str],
    /// Top-level key whose co-presence with the workspace table marks a
    /// hybrid root, e.g. `"package"` for Cargo or `"project"` for Python.
    owner_key: &'static str,
}

pub(crate) const CARGO_MEMBERSHIP_SPEC: MembershipSpec = MembershipSpec {
    manifest_file_name: ManifestFormat::CargoToml.file_name(),
    table_path: &["workspace"],
    owner_key: "package",
};

pub(crate) const PYTHON_MEMBERSHIP_SPEC: MembershipSpec = MembershipSpec {
    manifest_file_name: ManifestFormat::PyprojectToml.file_name(),
    table_path: &["tool", "uv", "workspace"],
    owner_key: "project",
};

/// NAIVE first pass: only handles (1) the manifest file entirely absent, and
/// (2) a well-formed workspace table with members/exclude arrays of
/// strings. Every other shape (workspace table absent, TOML unparseable,
/// members/exclude present but not an array-of-strings, members key
/// absent) falls back to an absent-filter (admit-all) result -- see the
/// `cargo_membership_tests`/`python_membership_tests` modules for the
/// exact fallback matrix this is pinned against.
pub(crate) fn read_membership(root: &Path, spec: &MembershipSpec) -> Membership {
    let content = match std::fs::read_to_string(root.join(spec.manifest_file_name)) {
        Ok(c) => c,
        Err(_) => return Membership::absent(),
    };
    let doc = match content.parse::<toml_edit::DocumentMut>() {
        Ok(d) => d,
        Err(_) => return Membership::absent(),
    };

    let mut cursor: Option<&toml_edit::Item> = doc.get(spec.table_path[0]);
    for key in &spec.table_path[1..] {
        cursor = cursor.and_then(|c| c.get(*key));
    }

    let hybrid_root = doc.get(spec.owner_key).is_some() && cursor.is_some();
    let Some(workspace) = cursor else {
        return Membership {
            members: None,
            exclude: NegatableGlobSet::empty(),
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
        .unwrap_or_else(NegatableGlobSet::empty);
    Membership {
        members,
        exclude,
        hybrid_root,
    }
}

pub(crate) fn read_cargo_membership(root: &Path) -> Membership {
    read_membership(root, &CARGO_MEMBERSHIP_SPEC)
}

pub(crate) fn read_python_membership(root: &Path) -> Membership {
    read_membership(root, &PYTHON_MEMBERSHIP_SPEC)
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
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = \"crates/*\"\n").unwrap();
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
    fn read_cargo_membership_members_array_honors_negated_pattern() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\", \"!crates/excluded-one\"]\n",
        )
        .unwrap();
        let m = read_cargo_membership(dir.path());
        assert!(m.admits(Path::new("crates/kept"), false));
        assert!(!m.admits(Path::new("crates/excluded-one"), false));
    }

    #[test]
    fn read_cargo_membership_empty_members_array_admits_nothing() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        let m = read_cargo_membership(dir.path());
        assert!(!m.admits(Path::new("crates/anything"), false));
    }
}

pub(crate) struct NpmMembership {
    globs: Option<NegatableGlobSet>,
    hybrid_root: bool,
}

impl NpmMembership {
    /// `rel` must be a workspace-relative, forward-slash-normalized path.
    /// `is_root` is true exactly when `rel == Path::new(".")`.
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
fn package_json_declares_name(root: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(root.join(ManifestFormat::PackageJson.file_name())) else {
        return false;
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    val.get("name").is_some()
}

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
fn read_package_json_workspaces(root: &Path) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(root.join(ManifestFormat::PackageJson.file_name())).ok()?;
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
        std::fs::write(dir.path().join("package.json"), r#"{"workspaces": ["packages/*"]}"#).unwrap();
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
        std::fs::write(dir.path().join("package.json"), r#"{"workspaces": ["packages/*"]}"#).unwrap();
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "packages:\n  - \"tools/*\"\n").unwrap();
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
    fn read_npm_membership_falls_back_to_absent_when_pnpm_yaml_is_malformed_and_no_package_json_workspaces() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "packages: [\"packages/*\"\n").unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_falls_back_to_package_json_when_pnpm_yaml_is_malformed_and_package_json_workspaces_present()
    {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"workspaces": ["packages/*"]}"#).unwrap();
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "packages: [\"packages/*\"\n").unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("packages/y"), false));
    }

    #[test]
    fn read_npm_membership_falls_back_to_absent_when_pnpm_yaml_has_zero_documents() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "# no packages here\n").unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_falls_back_to_absent_when_pnpm_packages_key_missing_or_wrong_scalar_type() {
        let dir_missing = tempdir().unwrap();
        std::fs::write(dir_missing.path().join("pnpm-workspace.yaml"), "other_key: true\n").unwrap();
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
        std::fs::write(dir.path().join("package.json"), r#"{"workspaces": "packages/*"}"#).unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("tools/outside"), false));
    }

    #[test]
    fn read_npm_membership_falls_back_to_absent_when_workspaces_array_has_non_string_entry() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"workspaces": ["packages/*", 42]}"#).unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("tools/outside"), false));
    }

    #[test]
    fn read_npm_membership_falls_back_to_absent_when_pnpm_packages_sequence_has_non_string_element() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "packages:\n  - \"a\"\n  - 42\n").unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_falls_back_to_absent_when_root_package_json_syntax_is_invalid() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{\"workspaces\": [\"packages/*\"],}").unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_package_json_workspaces_honors_negated_pattern() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"workspaces": ["packages/*", "!packages/excluded-one"]}"#,
        )
        .unwrap();
        let m = read_npm_membership(dir.path());
        assert!(
            m.admits(Path::new("packages/kept"), false),
            "a package not named by the negative pattern must remain a workspace member"
        );
        assert!(
            !m.admits(Path::new("packages/excluded-one"), false),
            "a `!`-prefixed workspaces entry must exclude the package, not be silently ignored"
        );
    }

    #[test]
    fn read_npm_membership_pnpm_packages_honors_negated_pattern() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages:\n  - \"packages/*\"\n  - \"!packages/excluded-one\"\n",
        )
        .unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(Path::new("packages/kept"), false));
        assert!(!m.admits(Path::new("packages/excluded-one"), false));
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
        let entries = vec!["crates/kept-example".to_string(), "crates/[unterminated".to_string()];
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
        assert!(single_star.is_match(Path::new("crates/a")), "* must match one segment");
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

    #[test]
    fn build_globset_negated_pattern_excludes_an_otherwise_matching_path() {
        let entries = vec!["packages/*".to_string(), "!packages/excluded-one".to_string()];
        let set = build_globset(&entries);
        assert!(
            set.is_match(Path::new("packages/kept")),
            "a package not named by the negative pattern must still match"
        );
        assert!(
            !set.is_match(Path::new("packages/excluded-one")),
            "a leading `!` must exclude an otherwise-matching path, not be ignored"
        );
    }

    #[test]
    fn build_globset_positive_only_list_is_unaffected_by_negation_support() {
        // Regression guard: adding negation semantics must not change
        // behavior for a plain, no-`!` pattern list.
        let set = build_globset(&["packages/*".to_string()]);
        assert!(set.is_match(Path::new("packages/a")));
        assert!(!set.is_match(Path::new("tools/a")));
    }

    #[test]
    fn build_globset_all_negative_list_matches_nothing() {
        // With no positive pattern at all, there's no baseline set for a
        // negation to carve an exclusion out of -- matches nothing, exactly
        // like real npm/pnpm/Yarn workspace-glob resolution.
        let set = build_globset(&["!packages/anything".to_string()]);
        assert!(!set.is_match(Path::new("packages/anything")));
        assert!(!set.is_match(Path::new("packages/something-else")));
    }

    #[test]
    fn build_globset_negation_applies_across_multiple_positive_patterns() {
        let entries = vec![
            "packages/*".to_string(),
            "tools/*".to_string(),
            "!packages/excluded-one".to_string(),
        ];
        let set = build_globset(&entries);
        assert!(set.is_match(Path::new("packages/kept")));
        assert!(set.is_match(Path::new("tools/kept")));
        assert!(!set.is_match(Path::new("packages/excluded-one")));
    }
}

#[cfg(test)]
mod ac09e_probe {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn probe_unparseable_package_json_before_04e() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{\"workspaces\": [\"packages/*\"],}").unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(std::path::Path::new("packages/anything"), false));
    }

    #[test]
    fn probe_pnpm_non_string_seq_before_04e() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "packages:\n  - \"a\"\n  - 42\n").unwrap();
        let m = read_npm_membership(dir.path());
        assert!(m.admits(std::path::Path::new("packages/anything"), false));
    }
}

#[cfg(test)]
mod python_membership_tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn read_python_membership_admits_all_when_no_pyproject_toml_file_exists() {
        let dir = tempdir().unwrap();
        let m = read_python_membership(dir.path());
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_python_membership_honors_well_formed_members_and_exclude() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.uv.workspace]\nmembers = [\"packages/*\"]\nexclude = [\"packages/examples/demo\"]\n",
        )
        .unwrap();
        let m = read_python_membership(dir.path());
        assert!(m.admits(Path::new("packages/kept-example"), false));
        assert!(!m.admits(Path::new("packages/examples/demo"), false));
        assert!(!m.admits(Path::new("tools/outside"), false));
    }

    #[test]
    fn read_python_membership_admits_all_when_workspace_table_absent_but_file_exists() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"solo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let m = read_python_membership(dir.path());
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_python_membership_falls_back_to_absent_when_root_toml_is_unparseable() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("pyproject.toml"), "[tool.uv.workspace\nmembers = [\n").unwrap();
        let m = read_python_membership(dir.path());
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_python_membership_detects_hybrid_root_and_exempts_it_from_exclude() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"root-pkg\"\nversion = \"0.1.0\"\n\n[tool.uv.workspace]\nmembers = [\"packages/*\"]\nexclude = [\".\"]\n",
        )
        .unwrap();
        let m = read_python_membership(dir.path());
        assert!(m.admits(Path::new("."), true));
    }
}
