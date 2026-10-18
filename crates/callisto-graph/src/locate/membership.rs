use callisto_model::{ManifestError, ManifestFormat};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::locate::LocateError;

/// A manifest's text without a byte-order mark, or `None` when it does not exist. A manifest that exists but
/// cannot be read is an error: membership computed from a file we could not read would admit the wrong packages.
pub(crate) fn read_root_manifest(path: &Path) -> Result<Option<String>, LocateError> {
    match callisto_model::strict_fs::read_optional(path) {
        Ok(content) => Ok(content.map(|text| match text.strip_prefix('\u{FEFF}') {
            Some(rest) => rest.to_owned(),
            None => text,
        })),
        Err(error) => Err(LocateError::ManifestReadError {
            path: path.to_path_buf(),
            message: error.to_string(),
        }),
    }
}

pub(crate) fn parse_error(path: &Path, message: String) -> LocateError {
    LocateError::ManifestParseError {
        path: path.to_path_buf(),
        message,
    }
}

fn invalid_membership(path: &Path, message: String) -> LocateError {
    LocateError::InvalidMembership {
        path: path.to_path_buf(),
        message,
    }
}

/// Compiles `entries` for the membership `key` of the manifest at `path`; a bad glob is an error, not a skipped entry.
fn compile_globs(path: &Path, key: &str, entries: &[String]) -> Result<NegatableGlobSet, LocateError> {
    build_globset(entries).map_err(|error| invalid_membership(path, format!("`{key}`: {error}")))
}

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

/// `./packages/*` and `packages/*/` name the same directories as `packages/*`, but compiled as written they would
/// match no relative path and silently admit nothing.
fn normalize_pattern(pattern: &str) -> &str {
    let mut rest = pattern;
    while let Some(stripped) = rest.strip_prefix("./") {
        rest = stripped;
    }
    if rest.is_empty() && !pattern.is_empty() {
        return ".";
    }
    match rest.trim_end_matches('/') {
        "" => rest,
        trimmed => trimmed,
    }
}

/// Builds a [`NegatableGlobSet`] from a list of glob strings, matched with
/// `literal_separator(true)` semantics (`*` does not cross `/`; `**` does)
/// against forward-slash-normalized, workspace-relative paths. An entry
/// with a leading `!` is treated as a negative (exclusion) pattern -- see
/// [`NegatableGlobSet`].
///
/// An entry that fails to compile is an error: skipping it would leave an exclusion or a
/// member unapplied and change which packages the workspace admits.
pub(crate) fn build_globset(entries: &[String]) -> Result<NegatableGlobSet, globset::Error> {
    let mut positive = GlobSetBuilder::new();
    let mut negative = GlobSetBuilder::new();
    for entry in entries {
        let (builder, pattern) = match entry.strip_prefix('!') {
            Some(rest) => (&mut negative, rest),
            None => (&mut positive, entry.as_str()),
        };
        builder.add(
            GlobBuilder::new(normalize_pattern(pattern))
                .literal_separator(true)
                .build()?,
        );
    }
    Ok(NegatableGlobSet {
        positive: positive.build()?,
        negative: negative.build()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_dot_slash_or_trailing_slash_names_the_same_directories() {
        for entry in ["packages/*", "./packages/*", "packages/*/", "././packages/*/"] {
            let set = build_globset(&[entry.to_string()]).unwrap();
            assert!(set.is_match(Path::new("packages/a")), "{entry}");
            assert!(!set.is_match(Path::new("other/a")), "{entry}");
        }
        let excluded = build_globset(&["packages/*".to_string(), "!./packages/skip/".to_string()]).unwrap();
        assert!(excluded.is_match(Path::new("packages/a")));
        assert!(!excluded.is_match(Path::new("packages/skip")));
        assert!(build_globset(&["./".to_string()]).unwrap().is_match(Path::new(".")));
    }

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
    /// The manifest has the workspace table, even if it lists no `members`.
    declared: bool,
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

    /// Unlike [`Self::admits`], `None` `members` (no `[workspace]` restriction present) never
    /// counts here -- only a real `members`/`exclude` glob match, or the hybrid-root case.
    pub(crate) fn admits_explicitly(&self, rel: &Path, is_root: bool) -> bool {
        if is_root && self.hybrid_root {
            return true;
        }
        match &self.members {
            Some(members) => !self.exclude.is_match(rel) && members.is_match(rel),
            None => false,
        }
    }

    /// No manifest, or a manifest that declares no workspace restriction: every directory is admitted.
    fn unrestricted() -> Self {
        Membership {
            members: None,
            exclude: NegatableGlobSet::empty(),
            hybrid_root: false,
            declared: false,
        }
    }
}

/// Parses a TOML array-of-strings item, returning `None` for any
/// other shape (bare string, table, non-string entries, etc.).
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

/// Reads the workspace membership declared by the root manifest.
///
/// A missing manifest, or one with no workspace table or no `members` key, restricts nothing. A manifest that cannot be
/// read or parsed, or whose `members`/`exclude` is not an array of valid globs, is an error naming the file: guessing
/// "every directory" there would turn vendored copies and excluded crates into releasable packages.
pub(crate) fn read_membership(root: &Path, spec: &MembershipSpec) -> Result<Membership, LocateError> {
    let path = root.join(spec.manifest_file_name);
    let Some(content) = read_root_manifest(&path)? else {
        return Ok(Membership::unrestricted());
    };
    let doc = content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| parse_error(&path, error.to_string()))?;

    let mut cursor: Option<&toml_edit::Item> = doc.get(spec.table_path[0]);
    for key in &spec.table_path[1..] {
        cursor = cursor.and_then(|c| c.get(*key));
    }

    let hybrid_root = doc.get(spec.owner_key).is_some() && cursor.is_some();
    let Some(workspace) = cursor else {
        return Ok(Membership {
            members: None,
            exclude: NegatableGlobSet::empty(),
            hybrid_root,
            declared: false,
        });
    };
    let table = spec.table_path.join(".");
    if workspace.as_table_like().is_none() {
        return Err(invalid_membership(&path, format!("`{table}` must be a table")));
    }
    let globs = |key: &str| -> Result<Option<NegatableGlobSet>, LocateError> {
        let Some(item) = workspace.get(key) else {
            return Ok(None);
        };
        let full_key = format!("{table}.{key}");
        let entries = parse_toml_string_array(item)
            .ok_or_else(|| invalid_membership(&path, format!("`{full_key}` must be an array of strings")))?;
        compile_globs(&path, &full_key, &entries).map(Some)
    };
    Ok(Membership {
        members: globs("members")?,
        exclude: globs("exclude")?.unwrap_or_else(NegatableGlobSet::empty),
        hybrid_root,
        declared: true,
    })
}

pub(crate) fn read_cargo_membership(root: &Path) -> Result<Membership, LocateError> {
    read_membership(root, &CARGO_MEMBERSHIP_SPEC)
}

pub(crate) fn read_python_membership(root: &Path) -> Result<Membership, LocateError> {
    read_membership(root, &PYTHON_MEMBERSHIP_SPEC)
}

/// The membership of each workspace nested below the workspace root, keyed by its directory (`None` where the directory
/// declares no workspace), and what has been learned about Cargo workspace manifests on the way.
#[derive(Default)]
pub(crate) struct NestedMemberships {
    cargo: HashMap<PathBuf, Membership>,
    reads: callisto_manifests::WorkspaceReads,
    npm: HashMap<PathBuf, Option<NpmMembership>>,
    python: HashMap<PathBuf, Option<Membership>>,
}

/// Whether the workspaces nested below `root` admit the package of `ecosystem` at `rel`. The workspace at `root` is
/// judged by the `read_*_membership` of each ecosystem; this adds the verdict of a nearer one, which judges the package
/// by its own list. `manifest_text` is the package's own manifest.
pub(crate) fn nested_workspace_admits(
    root: &Path,
    rel: &Path,
    ecosystem: callisto_model::Ecosystem,
    manifest_text: &str,
    nested: &mut NestedMemberships,
) -> Result<bool, LocateError> {
    use callisto_model::Ecosystem;
    match ecosystem {
        Ecosystem::Cargo => nested_cargo_workspace_admits(root, rel, manifest_text, nested),
        Ecosystem::Npm => {
            let cache = &mut nested.npm;
            judged_by_nearest_declared_workspace(
                root,
                rel,
                cache,
                |dir| read_npm_membership(dir).map(|membership| membership.globs.is_some().then_some(membership)),
                |membership, within| membership.admits(within, false),
            )
        }
        Ecosystem::Pypi => {
            let cache = &mut nested.python;
            judged_by_nearest_declared_workspace(
                root,
                rel,
                cache,
                |dir| read_python_membership(dir).map(|membership| membership.declared.then_some(membership)),
                |membership, within| membership.admits(within, false),
            )
        }
        _ => Ok(true),
    }
}

/// The nearest directory above `rel`, below `root`, whose own manifest declares a workspace judges `rel` by its list,
/// with `rel` taken relative to it. With no such directory nothing nested has an opinion.
fn judged_by_nearest_declared_workspace<M>(
    root: &Path,
    rel: &Path,
    cache: &mut HashMap<PathBuf, Option<M>>,
    read: impl Fn(&Path) -> Result<Option<M>, LocateError>,
    admits: impl Fn(&M, &Path) -> bool,
) -> Result<bool, LocateError> {
    for ancestor in rel.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() || ancestor == Path::new(".") {
            break;
        }
        if !cache.contains_key(ancestor) {
            let membership = read(&root.join(ancestor))?;
            cache.insert(ancestor.to_path_buf(), membership);
        }
        if let Some(membership) = &cache[ancestor] {
            let within = rel.strip_prefix(ancestor).unwrap_or(rel);
            return Ok(admits(membership, within));
        }
    }
    Ok(true)
}

/// Whether a Cargo workspace nested below `root` admits the package at `rel`. The workspace at `root` is judged by
/// [`read_cargo_membership`]; this adds the verdict of a nearer workspace that governs the package, which judges it by its
/// own `members` and `exclude`. A package that is itself a workspace root, or that no nested workspace governs, passes.
pub(crate) fn nested_cargo_workspace_admits(
    root: &Path,
    rel: &Path,
    manifest_text: &str,
    nested: &mut NestedMemberships,
) -> Result<bool, LocateError> {
    let manifest = rel.join("Cargo.toml");
    let located = |error: ManifestError| match error {
        ManifestError::Parse { path, message, .. } => parse_error(&root.join(path), message),
        ManifestError::Read { path, message } => LocateError::ManifestReadError {
            path: root.join(path),
            message,
        },
        other => LocateError::ManifestReadError {
            path: root.join(&manifest),
            message: other.to_string(),
        },
    };
    let document = callisto_manifests::parse_manifest(&manifest, manifest_text).map_err(located)?;
    let Some(governing) = nested
        .reads
        .enclosing(&manifest, &document, callisto_manifests::read_manifest_text(root))
        .map_err(located)?
    else {
        return Ok(true);
    };
    let governing_dir = governing.parent().unwrap_or(Path::new(""));
    // A workspace named by `package.workspace` from outside the package's own ancestry has no path to judge by.
    let Ok(within) = rel.strip_prefix(governing_dir) else {
        return Ok(true);
    };
    if governing_dir.as_os_str().is_empty() || within.as_os_str().is_empty() {
        return Ok(true);
    }
    if !nested.cargo.contains_key(governing_dir) {
        let membership = read_cargo_membership(&root.join(governing_dir))?;
        nested.cargo.insert(governing_dir.to_path_buf(), membership);
    }
    Ok(nested.cargo[governing_dir].admits(within, false))
}

#[cfg(test)]
mod cargo_membership_tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn read_cargo_membership_admits_all_when_no_cargo_toml_file_exists() {
        let dir = tempdir().unwrap();
        let m = read_cargo_membership(dir.path()).unwrap();
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
        let m = read_cargo_membership(dir.path()).unwrap();
        assert!(m.admits(Path::new("crates/kept-example"), false));
        assert!(!m.admits(Path::new("crates/scratch-example"), false));
        assert!(!m.admits(Path::new("tools/outside"), false));
    }

    #[test]
    fn read_cargo_membership_rejects_members_that_is_a_bare_string() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = \"crates/*\"\n").unwrap();
        let err = read_cargo_membership(dir.path()).err().unwrap();
        assert!(
            matches!(&err, LocateError::InvalidMembership { path, message } if path.ends_with("Cargo.toml") && message.contains("workspace.members")),
            "{err:?}"
        );
    }

    #[test]
    fn read_cargo_membership_rejects_exclude_that_is_a_bare_string() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\nexclude = \"crates/scratch-example\"\n",
        )
        .unwrap();
        let err = read_cargo_membership(dir.path()).err().unwrap();
        assert!(
            matches!(&err, LocateError::InvalidMembership { message, .. } if message.contains("workspace.exclude")),
            "{err:?}"
        );
    }

    #[test]
    fn read_cargo_membership_rejects_a_non_string_entry_and_an_uncompilable_glob() {
        let dir = tempdir().unwrap();
        for members in ["[\"crates/*\", 42]", "[\"crates/[unterminated\"]"] {
            std::fs::write(
                dir.path().join("Cargo.toml"),
                format!("[workspace]\nmembers = {members}\n"),
            )
            .unwrap();
            let err = read_cargo_membership(dir.path()).err().unwrap();
            assert!(
                matches!(err, LocateError::InvalidMembership { .. }),
                "{members}: {err:?}"
            );
        }
    }

    #[test]
    fn read_cargo_membership_rejects_a_workspace_that_is_not_a_table() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "workspace = \"crates/*\"\n").unwrap();
        let err = read_cargo_membership(dir.path()).err().unwrap();
        assert!(matches!(err, LocateError::InvalidMembership { .. }), "{err:?}");
    }

    #[test]
    fn read_cargo_membership_reports_a_manifest_that_cannot_be_read() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), b"[workspace]\nmembers = [\"caf\xe9\"]\n").unwrap();
        let err = read_cargo_membership(dir.path()).err().unwrap();
        assert!(matches!(err, LocateError::ManifestReadError { .. }), "{err:?}");
    }

    #[test]
    fn read_cargo_membership_detects_hybrid_root_and_exempts_it_from_exclude() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"root-crate\"\nversion = \"0.1.0\"\n\n[workspace]\nmembers = [\"crates/*\"]\nexclude = [\".\"]\n",
        )
        .unwrap();
        let m = read_cargo_membership(dir.path()).unwrap();
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
        let m = read_cargo_membership(dir.path()).unwrap();
        assert!(m.admits(Path::new("crates/anything"), false));
    }

    #[test]
    fn read_cargo_membership_rejects_a_root_toml_that_is_unparseable() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace\nmembers = [\n").unwrap();
        let err = read_cargo_membership(dir.path()).err().unwrap();
        assert!(
            matches!(&err, LocateError::ManifestParseError { path, .. } if path.ends_with("Cargo.toml")),
            "{err:?}"
        );
    }

    #[test]
    fn read_cargo_membership_honors_exclude_when_no_members_key_present() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nexclude = [\"crates/foo\"]\n",
        )
        .unwrap();
        let m = read_cargo_membership(dir.path()).unwrap();
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
        let m = read_cargo_membership(dir.path()).unwrap();
        assert!(m.admits(Path::new("crates/kept"), false));
        assert!(!m.admits(Path::new("crates/excluded-one"), false));
    }

    #[test]
    fn read_cargo_membership_empty_members_array_admits_nothing() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        let m = read_cargo_membership(dir.path()).unwrap();
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

    /// See [`Membership::admits_explicitly`]: `None` globs never counts here.
    pub(crate) fn admits_explicitly(&self, rel: &Path, is_root: bool) -> bool {
        if is_root && self.hybrid_root {
            return true;
        }
        match &self.globs {
            Some(globs) => globs.is_match(rel),
            None => false,
        }
    }
}

/// Reads npm membership: `pnpm-workspace.yaml` `packages` when present, else `package.json` `workspaces` (an array, or
/// Yarn classic's `{ "packages": [..] }`). Like [`read_membership`], an unreadable, unparseable or wrongly shaped file
/// is an error; a missing file or a missing key restricts nothing.
pub(crate) fn read_npm_membership(root: &Path) -> Result<NpmMembership, LocateError> {
    let package_json = root.join(ManifestFormat::PackageJson.file_name());
    let manifest = match read_root_manifest(&package_json)? {
        Some(content) => {
            let value = serde_json::from_str::<serde_json::Value>(&content)
                .map_err(|error| parse_error(&package_json, error.to_string()))?;
            if !value.is_object() {
                return Err(invalid_membership(&package_json, "must be a JSON object".to_string()));
            }
            Some(value)
        }
        None => None,
    };
    let hybrid_root = manifest.as_ref().is_some_and(|value| value.get("name").is_some());

    let pnpm = root.join("pnpm-workspace.yaml");
    if let Some(packages) = read_pnpm_packages(&pnpm)? {
        return Ok(NpmMembership {
            globs: Some(compile_globs(&pnpm, "packages", &packages)?),
            hybrid_root,
        });
    }
    let globs = match manifest.as_ref().and_then(|value| value.get("workspaces")) {
        None => None,
        Some(field) => {
            let entries = package_json_workspace_entries(field).ok_or_else(|| {
                invalid_membership(
                    &package_json,
                    "`workspaces` must be an array of strings or an object with a `packages` array of strings"
                        .to_string(),
                )
            })?;
            Some(compile_globs(&package_json, "workspaces", &entries)?)
        }
    };
    Ok(NpmMembership { globs, hybrid_root })
}

/// Whether the npm workspace in `dir` lists packages (`pnpm-workspace.yaml` `packages` or `package.json` `workspaces`).
pub(crate) fn declares_npm_workspace(dir: &Path) -> Result<bool, LocateError> {
    read_npm_membership(dir).map(|membership| membership.globs.is_some())
}

/// The `packages` list of `pnpm-workspace.yaml`, or `None` when the file is absent, empty, or has no `packages` key
/// (pnpm then restricts nothing).
fn read_pnpm_packages(path: &Path) -> Result<Option<Vec<String>>, LocateError> {
    let Some(content) = read_root_manifest(path)? else {
        return Ok(None);
    };
    let docs = yaml_rust2::YamlLoader::load_from_str(&content).map_err(|error| parse_error(path, error.to_string()))?;
    let Some(doc) = docs.first() else {
        return Ok(None);
    };
    if doc.is_null() {
        return Ok(None);
    }
    if doc.as_hash().is_none() {
        return Err(invalid_membership(
            path,
            "must be a mapping with a `packages` list".to_string(),
        ));
    }
    let packages = &doc["packages"];
    if packages.is_badvalue() || packages.is_null() {
        return Ok(None);
    }
    let not_strings = || invalid_membership(path, "`packages` must be a list of strings".to_string());
    packages
        .as_vec()
        .ok_or_else(not_strings)?
        .iter()
        .map(|item| item.as_str().map(str::to_owned).ok_or_else(not_strings))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn package_json_workspace_entries(field: &serde_json::Value) -> Option<Vec<String>> {
    let list = field.as_array().or_else(|| field.get("packages")?.as_array())?;
    list.iter().map(|entry| entry.as_str().map(str::to_owned)).collect()
}

#[cfg(test)]
mod npm_membership_tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn read_npm_membership_admits_all_when_no_package_json_file_exists() {
        let dir = tempdir().unwrap();
        let m = read_npm_membership(dir.path()).unwrap();
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_admits_all_when_package_json_present_but_no_workspaces_field() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"name": "root"}"#).unwrap();
        let m = read_npm_membership(dir.path()).unwrap();
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_honors_well_formed_package_json_workspaces() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"workspaces": ["packages/*"]}"#).unwrap();
        let m = read_npm_membership(dir.path()).unwrap();
        assert!(m.admits(Path::new("packages/kept"), false));
        assert!(!m.admits(Path::new("tools/outside"), false));
    }

    #[test]
    fn read_npm_membership_empty_workspaces_array_admits_nothing() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"workspaces": []}"#).unwrap();
        let m = read_npm_membership(dir.path()).unwrap();
        assert!(!m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_prefers_pnpm_packages_over_package_json_workspaces() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"workspaces": ["packages/*"]}"#).unwrap();
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "packages:\n  - \"tools/*\"\n").unwrap();
        let m = read_npm_membership(dir.path()).unwrap();
        assert!(m.admits(Path::new("tools/x"), false));
        assert!(!m.admits(Path::new("packages/y"), false));
    }

    #[test]
    fn read_npm_membership_pnpm_empty_packages_list_admits_nothing() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "packages: []\n").unwrap();
        let m = read_npm_membership(dir.path()).unwrap();
        assert!(!m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_rejects_a_top_level_that_is_not_a_mapping_or_object() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "- packages/*\n").unwrap();
        let err = read_npm_membership(dir.path()).err().unwrap();
        assert!(matches!(err, LocateError::InvalidMembership { .. }), "{err:?}");
        std::fs::remove_file(dir.path().join("pnpm-workspace.yaml")).unwrap();

        std::fs::write(dir.path().join("package.json"), "[]").unwrap();
        let err = read_npm_membership(dir.path()).err().unwrap();
        assert!(matches!(err, LocateError::InvalidMembership { .. }), "{err:?}");
    }

    #[test]
    fn read_npm_membership_rejects_workspaces_that_is_null_or_an_object_without_packages() {
        let dir = tempdir().unwrap();
        for workspaces in ["null", r#"{"nohoist": ["**/x"]}"#] {
            std::fs::write(
                dir.path().join("package.json"),
                format!(r#"{{"workspaces": {workspaces}}}"#),
            )
            .unwrap();
            let err = read_npm_membership(dir.path()).err().unwrap();
            assert!(
                matches!(err, LocateError::InvalidMembership { .. }),
                "{workspaces}: {err:?}"
            );
        }
    }

    #[test]
    fn read_npm_membership_rejects_a_malformed_pnpm_yaml() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "packages: [\"packages/*\"\n").unwrap();
        let err = read_npm_membership(dir.path()).err().unwrap();
        assert!(
            matches!(&err, LocateError::ManifestParseError { path, .. } if path.ends_with("pnpm-workspace.yaml")),
            "{err:?}"
        );
    }

    #[test]
    fn read_npm_membership_rejects_a_malformed_pnpm_yaml_even_when_package_json_has_workspaces() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"workspaces": ["packages/*"]}"#).unwrap();
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "packages: [\"packages/*\"\n").unwrap();
        let err = read_npm_membership(dir.path()).err().unwrap();
        assert!(matches!(err, LocateError::ManifestParseError { .. }), "{err:?}");
    }

    #[test]
    fn read_npm_membership_admits_all_when_pnpm_yaml_has_zero_documents() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "# no packages here\n").unwrap();
        let m = read_npm_membership(dir.path()).unwrap();
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_npm_membership_admits_all_when_pnpm_packages_key_is_missing_and_rejects_a_scalar() {
        let dir_missing = tempdir().unwrap();
        std::fs::write(dir_missing.path().join("pnpm-workspace.yaml"), "other_key: true\n").unwrap();
        let m_missing = read_npm_membership(dir_missing.path()).unwrap();
        assert!(m_missing.admits(Path::new("packages/anything"), false));

        let dir_wrong_type = tempdir().unwrap();
        std::fs::write(
            dir_wrong_type.path().join("pnpm-workspace.yaml"),
            "packages: \"packages/*\"\n",
        )
        .unwrap();
        let err = read_npm_membership(dir_wrong_type.path()).err().unwrap();
        assert!(matches!(err, LocateError::InvalidMembership { .. }), "{err:?}");
    }

    #[test]
    fn read_npm_membership_rejects_workspaces_that_is_a_bare_string() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"workspaces": "packages/*"}"#).unwrap();
        let err = read_npm_membership(dir.path()).err().unwrap();
        assert!(
            matches!(&err, LocateError::InvalidMembership { path, .. } if path.ends_with("package.json")),
            "{err:?}"
        );
    }

    #[test]
    fn read_npm_membership_reads_yarn_classic_object_workspaces() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"workspaces": {"packages": ["packages/*"], "nohoist": ["**/x"]}}"#,
        )
        .unwrap();
        let m = read_npm_membership(dir.path()).unwrap();
        assert!(m.admits(Path::new("packages/kept"), false));
        assert!(!m.admits(Path::new("tools/outside"), false));
    }

    #[test]
    fn read_npm_membership_reads_a_package_json_that_starts_with_a_byte_order_mark() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            "\u{FEFF}{\"workspaces\": [\"packages/*\"]}",
        )
        .unwrap();
        let m = read_npm_membership(dir.path()).unwrap();
        assert!(!m.admits(Path::new("tools/outside"), false));
    }

    #[test]
    fn read_npm_membership_rejects_a_workspaces_array_with_a_non_string_entry() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"workspaces": ["packages/*", 42]}"#).unwrap();
        let err = read_npm_membership(dir.path()).err().unwrap();
        assert!(matches!(err, LocateError::InvalidMembership { .. }), "{err:?}");
    }

    #[test]
    fn read_npm_membership_rejects_a_pnpm_packages_sequence_with_a_non_string_element() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "packages:\n  - \"a\"\n  - 42\n").unwrap();
        let err = read_npm_membership(dir.path()).err().unwrap();
        assert!(matches!(err, LocateError::InvalidMembership { .. }), "{err:?}");
    }

    #[test]
    fn read_npm_membership_rejects_a_root_package_json_with_invalid_syntax() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{\"workspaces\": [\"packages/*\"],}").unwrap();
        let err = read_npm_membership(dir.path()).err().unwrap();
        assert!(
            matches!(&err, LocateError::ManifestParseError { path, .. } if path.ends_with("package.json")),
            "{err:?}"
        );
    }

    #[test]
    fn read_npm_membership_package_json_workspaces_honors_negated_pattern() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"workspaces": ["packages/*", "!packages/excluded-one"]}"#,
        )
        .unwrap();
        let m = read_npm_membership(dir.path()).unwrap();
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
        let m = read_npm_membership(dir.path()).unwrap();
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
        let m = read_npm_membership(dir.path()).unwrap();
        assert!(m.admits(Path::new("."), true));
    }
}

#[cfg(test)]
mod glob_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn build_globset_rejects_an_invalid_entry_instead_of_skipping_it() {
        let entries = vec!["crates/kept-example".to_string(), "crates/[unterminated".to_string()];
        assert!(build_globset(&entries).is_err());
    }

    #[test]
    fn build_globset_rejects_a_list_of_only_invalid_entries() {
        assert!(build_globset(&["crates/[unterminated".to_string()]).is_err());
    }

    #[test]
    fn build_globset_literal_separator_true_means_star_does_not_cross_slash_but_double_star_does() {
        let single_star = build_globset(&["crates/*".to_string()]).unwrap();
        assert!(single_star.is_match(Path::new("crates/a")), "* must match one segment");
        assert!(
            !single_star.is_match(Path::new("crates/a/b")),
            "* must NOT cross a path separator -- this is what literal_separator(true) guarantees"
        );

        let double_star = build_globset(&["crates/**".to_string()]).unwrap();
        assert!(
            double_star.is_match(Path::new("crates/a/b")),
            "** must match zero or more full path segments recursively"
        );
    }

    #[test]
    fn build_globset_negated_pattern_excludes_an_otherwise_matching_path() {
        let entries = vec!["packages/*".to_string(), "!packages/excluded-one".to_string()];
        let set = build_globset(&entries).unwrap();
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
        let set = build_globset(&["packages/*".to_string()]).unwrap();
        assert!(set.is_match(Path::new("packages/a")));
        assert!(!set.is_match(Path::new("tools/a")));
    }

    #[test]
    fn build_globset_all_negative_list_matches_nothing() {
        // With no positive pattern at all, there's no baseline set for a
        // negation to carve an exclusion out of -- matches nothing, exactly
        // like real npm/pnpm/Yarn workspace-glob resolution.
        let set = build_globset(&["!packages/anything".to_string()]).unwrap();
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
        let set = build_globset(&entries).unwrap();
        assert!(set.is_match(Path::new("packages/kept")));
        assert!(set.is_match(Path::new("tools/kept")));
        assert!(!set.is_match(Path::new("packages/excluded-one")));
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
        let m = read_python_membership(dir.path()).unwrap();
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
        let m = read_python_membership(dir.path()).unwrap();
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
        let m = read_python_membership(dir.path()).unwrap();
        assert!(m.admits(Path::new("packages/anything"), false));
    }

    #[test]
    fn read_python_membership_rejects_a_uv_workspace_of_the_wrong_shape() {
        let dir = tempdir().unwrap();
        for workspace in [
            "members = \"packages/*\"",
            "members = [\"packages/*\", 1]",
            "exclude = \"x\"",
        ] {
            std::fs::write(
                dir.path().join("pyproject.toml"),
                format!("[tool.uv.workspace]\n{workspace}\n"),
            )
            .unwrap();
            let err = read_python_membership(dir.path()).err().unwrap();
            assert!(
                matches!(err, LocateError::InvalidMembership { .. }),
                "{workspace}: {err:?}"
            );
        }
    }

    #[test]
    fn read_python_membership_rejects_a_root_toml_that_is_unparseable() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("pyproject.toml"), "[tool.uv.workspace\nmembers = [\n").unwrap();
        let err = read_python_membership(dir.path()).err().unwrap();
        assert!(matches!(err, LocateError::ManifestParseError { .. }), "{err:?}");
    }

    #[test]
    fn read_python_membership_detects_hybrid_root_and_exempts_it_from_exclude() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"root-pkg\"\nversion = \"0.1.0\"\n\n[tool.uv.workspace]\nmembers = [\"packages/*\"]\nexclude = [\".\"]\n",
        )
        .unwrap();
        let m = read_python_membership(dir.path()).unwrap();
        assert!(m.admits(Path::new("."), true));
    }
}
