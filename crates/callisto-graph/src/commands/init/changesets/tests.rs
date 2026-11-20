use std::path::Path;

use callisto_model::{DiagnosticCode, DiagnosticSeverity};

use super::super::tests::{detect_in, parse, repo, INDEPENDENT, ORIGIN};
use super::super::{render_config, InitAnswers, Versioning};
use super::*;

/// What `changeset init` writes with `@changesets/cli` 2.x, with the monorepo's `fixed` filled in.
const CHANGESETS_V2: &str = r#"{
  "$schema": "https://unpkg.com/@changesets/config@2.3.1/schema.json",
  "changelog": "@changesets/cli/changelog",
  "commit": false,
  "fixed": [["@acme/*"]],
  "linked": [],
  "access": "restricted",
  "baseBranch": "main",
  "updateInternalDependencies": "patch",
  "ignore": []
}"#;

/// `@changesets/cli` 3.x: public access, a `release` base branch, `linked` and `ignore` set, and the 3.x-only keys.
const CHANGESETS_V3: &str = r#"{
  "$schema": "https://unpkg.com/@changesets/config@3.1.1/schema.json",
  "changelog": ["@changesets/changelog-github", { "repo": "acme/mono" }],
  "commit": false,
  "fixed": [],
  "linked": [["@acme/cli", "tool"]],
  "access": "public",
  "baseBranch": "release",
  "updateInternalDependencies": "minor",
  "ignore": ["@acme/utils"],
  "privatePackages": { "version": true, "tag": false }
}"#;

/// Scoped `@acme/{cli,core,utils}`, unscoped `tool`, and a private `@acme/site`, in an npm workspace.
fn npm_workspace(config: Option<&str>) -> tempfile::TempDir {
    let package = |name: &str| format!(r#"{{"name":"{name}","version":"1.0.0"}}"#);
    let mut files = vec![
        (
            "package.json",
            r#"{"private":true,"workspaces":["packages/*"]}"#.to_owned(),
        ),
        ("packages/cli/package.json", package("@acme/cli")),
        ("packages/core/package.json", package("@acme/core")),
        ("packages/utils/package.json", package("@acme/utils")),
        ("packages/tool/package.json", package("tool")),
        (
            "packages/site/package.json",
            r#"{"name":"@acme/site","version":"1.0.0","private":true}"#.to_owned(),
        ),
    ];
    if let Some(config) = config {
        files.push((".changeset/config.json", config.to_owned()));
    }
    let files: Vec<(&str, &str)> = files.iter().map(|(path, text)| (*path, text.as_str())).collect();
    repo(&files, Some(ORIGIN))
}

fn migrate_in(root: &Path) -> Migration {
    migrate(root, &detect_in(root).unwrap())
}

/// The warning `init` gives under independent versioning, for tests that need no answers back.
fn apply(migration: &Migration) -> Option<Diagnostic> {
    migration.apply_fixed_groups(&mut InitAnswers { ..INDEPENDENT })
}

fn messages(migration: &Migration) -> Vec<&str> {
    migration.diagnostics.iter().map(|d| d.message.as_str()).collect()
}

fn message_for<'a>(migration: &'a Migration, key: &str) -> &'a str {
    let needle = format!("`{key}`");
    let found: Vec<&str> = messages(migration)
        .into_iter()
        .filter(|message| message.contains(&needle))
        .collect();
    assert_eq!(found.len(), 1, "one warning names {needle}: {:?}", messages(migration));
    found[0]
}

// A `changeset init` v2 config: `fixed` globs expand to the current npm packages (a private one included), the default restricted access
// flags every scoped package that would publish public, and each other present key is reported.
#[test]
fn v2_config_expands_fixed_globs_and_flags_access_and_reports_ignored_keys() {
    let dir = npm_workspace(Some(CHANGESETS_V2));
    let migration = migrate_in(dir.path());

    let mut answers = INDEPENDENT;
    migration.apply_fixed_groups(&mut answers);
    assert_eq!(
        answers.fixed_groups,
        vec![FixedGroup {
            name: "fixed-1".to_owned(),
            members: ["npm/@acme/cli", "npm/@acme/core", "npm/@acme/site", "npm/@acme/utils"]
                .map(str::to_owned)
                .to_vec(),
        }]
    );
    assert_eq!(migration.base_branch.as_ref().map(BranchName::as_str), Some("main"));
    assert_eq!(
        migration.public_scoped,
        ["npm/@acme/cli", "npm/@acme/core", "npm/@acme/utils"]
    );
    let fixed = apply(&migration).unwrap();
    assert!(
        fixed.message.contains("add packages created later by hand"),
        "{}",
        fixed.message
    );
    let access = message_for(&migration, "access");
    assert!(
        access.contains("npm/@acme/cli") && access.contains("publishConfig.access"),
        "{access}"
    );
    for key in ["changelog", "commit", "updateInternalDependencies"] {
        assert!(message_for(&migration, key).contains("not honoured"), "{key}");
    }
    // `$schema` is metadata; empty `linked` and `ignore` drop nothing; `fixed` and `baseBranch` are carried over.
    assert_eq!(migration.diagnostics.len(), 4, "{:?}", messages(&migration));
    assert!(migration.diagnostics.iter().all(|d| {
        d.code == DiagnosticCode::ChangesetsConfigKeyDropped
            && d.severity == DiagnosticSeverity::Warning
            && d.path.as_deref() == Some(dir.path().join(".changeset/config.json").as_path())
    }));
}

// A v3 config: public access is no hazard, `linked` and `ignore` are reported without translation, and the keys
// only 3.x writes are reported like any other.
#[test]
fn v3_config_warns_linked_and_ignore_and_writes_no_groups() {
    let dir = npm_workspace(Some(CHANGESETS_V3));
    let migration = migrate_in(dir.path());

    assert!(apply(&migration).is_none());
    assert!(migration.public_scoped.is_empty());
    assert_eq!(migration.base_branch.as_ref().map(BranchName::as_str), Some("release"));
    let linked = message_for(&migration, "linked");
    assert!(linked.contains("every member"), "{linked}");
    let ignore = message_for(&migration, "ignore");
    assert!(ignore.contains("still versioned"), "{ignore}");
    for key in ["changelog", "commit", "updateInternalDependencies", "privatePackages"] {
        message_for(&migration, key);
    }
    assert_eq!(migration.diagnostics.len(), 6, "{:?}", messages(&migration));
}

// The hazard names only packages that would change visibility: an explicit `publishConfig.access`, an unscoped
// name and a private package never do.
#[test]
fn restricted_access_flags_only_scoped_packages_without_publish_config_access() {
    let dir = repo(
        &[
            ("package.json", r#"{"private":true,"workspaces":["packages/*"]}"#),
            ("packages/a/package.json", r#"{"name":"@acme/a","version":"1.0.0"}"#),
            (
                "packages/b/package.json",
                r#"{"name":"@acme/b","version":"1.0.0","publishConfig":{"access":"restricted"}}"#,
            ),
            (
                "packages/c/package.json",
                r#"{"name":"@acme/c","version":"1.0.0","publishConfig":{"access":"public"}}"#,
            ),
            ("packages/d/package.json", r#"{"name":"d","version":"1.0.0"}"#),
            (
                "packages/e/package.json",
                r#"{"name":"@acme/e","version":"1.0.0","private":true}"#,
            ),
            (".changeset/config.json", r#"{"baseBranch":"main"}"#),
        ],
        Some(ORIGIN),
    );
    let migration = migrate_in(dir.path());
    assert_eq!(migration.public_scoped, ["npm/@acme/a"]);
}

// A pattern that matches no npm package is named; a group left with no member is not written.
#[test]
fn fixed_patterns_matching_nothing_are_named_and_write_no_group() {
    let dir = npm_workspace(Some(r#"{"fixed":[["@other/*","missing"]],"access":"public"}"#));
    let migration = migrate_in(dir.path());
    let mut answers = INDEPENDENT;
    let fixed = migration.apply_fixed_groups(&mut answers).unwrap().message;
    assert!(answers.fixed_groups.is_empty());
    assert!(fixed.contains("@other/*") && fixed.contains("missing"), "{fixed}");
    assert!(
        fixed.contains("is not translated") && !fixed.contains("became"),
        "{fixed}"
    );
}

// A `*` stays inside one path segment, so `@acme/c*` never reaches `tool`; exact names match exactly.
#[test]
fn fixed_globs_match_names_the_way_changesets_does() {
    let dir = npm_workspace(Some(
        r#"{"fixed":[["@acme/c*","tool"],["@acme/utils"]],"access":"public"}"#,
    ));
    let migration = migrate_in(dir.path());
    let mut answers = INDEPENDENT;
    migration.apply_fixed_groups(&mut answers);
    let groups: Vec<(&str, Vec<&str>)> = answers
        .fixed_groups
        .iter()
        .map(|group| (group.name.as_str(), group.members.iter().map(String::as_str).collect()))
        .collect();
    assert_eq!(
        groups,
        [
            ("fixed-1", vec!["npm/@acme/cli", "npm/@acme/core", "npm/tool"]),
            ("fixed-2", vec!["npm/@acme/utils"])
        ]
    );
}

// A group whose patterns micromatch reads differently from a plain glob is held back, never dropped silently: the
// warning names the group and carries the block to paste after review. A plain group beside them still translates.
#[test]
fn groups_with_extglob_negation_brace_or_class_patterns_are_held_back_with_a_paste_block() {
    let dir = npm_workspace(Some(
        r#"{"fixed":[["@acme/@(cli|core)"],["!@acme/utils"],["@acme/{cli,core}"],["@acme/[cu]*"],["@acme/utils","tool"]],"access":"public"}"#,
    ));
    let migration = migrate_in(dir.path());
    let mut answers = INDEPENDENT;
    migration.apply_fixed_groups(&mut answers);
    assert_eq!(
        answers.fixed_groups,
        vec![FixedGroup {
            name: "fixed-5".to_owned(),
            members: ["npm/@acme/utils", "npm/tool"].map(str::to_owned).to_vec(),
        }]
    );
    let held: Vec<&str> = messages(&migration)
        .into_iter()
        .filter(|message| message.contains("`fixed`"))
        .collect();
    assert_eq!(held.len(), 4, "{held:?}");
    for (message, pattern) in held
        .iter()
        .zip(["@acme/@(cli|core)", "!@acme/utils", "@acme/{cli,core}", "@acme/[cu]*"])
    {
        assert!(
            message.contains(pattern) && message.contains("not written"),
            "{message}"
        );
        assert!(message.contains("[[fixed-group]]\nname = \"fixed-"), "{message}");
    }
    // The block lists today's members, so a lone negation names every other package.
    assert!(
        held[1].contains(r#"members = ["npm/@acme/cli", "npm/@acme/core", "npm/@acme/site", "npm/tool"]"#),
        "{}",
        held[1]
    );
    assert!(
        held[2].contains(r#"members = ["npm/@acme/cli", "npm/@acme/core"]"#),
        "{}",
        held[2]
    );
}

// `?` stands for one character in both matchers, so a group using only it translates.
#[test]
fn a_question_mark_pattern_translates() {
    let dir = npm_workspace(Some(r#"{"fixed":[["@acme/co?e","tool"]],"access":"public"}"#));
    let mut answers = INDEPENDENT;
    migrate_in(dir.path()).apply_fixed_groups(&mut answers);
    assert_eq!(answers.fixed_groups[0].members, ["npm/@acme/core", "npm/tool"]);
}

/// A Cargo crate `native-lib` with an npm package `@scope/native-lib` beside it, and a plain `@scope/other`.
fn colocated_workspace(config: &str) -> tempfile::TempDir {
    repo(
        &[
            ("Cargo.toml", "[workspace]\nmembers = [\"native\"]\nresolver = \"2\"\n"),
            ("package.json", r#"{"private":true,"workspaces":["native","other"]}"#),
            (
                "native/Cargo.toml",
                "[package]\nname = \"native-lib\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
            ),
            ("native/src/lib.rs", "\n"),
            (
                "native/package.json",
                r#"{"name":"@scope/native-lib","version":"1.0.0"}"#,
            ),
            ("other/package.json", r#"{"name":"@scope/other","version":"1.0.0"}"#),
            (".changeset/config.json", config),
        ],
        Some(ORIGIN),
    )
}

// Every name in a Changesets config is an npm package name: a package whose primary id is Cargo's still answers to
// its npm name, for groups and for the access check.
#[test]
fn colocated_cargo_and_npm_packages_are_found_by_their_npm_name() {
    let dir = colocated_workspace(r#"{"fixed":[["@scope/*"]],"access":"restricted"}"#);
    let facts = detect_in(dir.path()).unwrap();
    let native = facts
        .packages
        .iter()
        .find(|package| package.id == "cargo/native-lib")
        .unwrap();
    assert_eq!(native.npm_name.as_deref(), Some("@scope/native-lib"));

    let migration = migrate(dir.path(), &facts);
    let mut answers = INDEPENDENT;
    migration.apply_fixed_groups(&mut answers);
    assert_eq!(
        answers.fixed_groups[0].members,
        ["cargo/native-lib", "npm/@scope/other"]
    );
    assert_eq!(migration.public_scoped, ["npm/@scope/native-lib", "npm/@scope/other"]);
}

// No file, a file that is not a JSON object: silent, nothing written.
#[test]
fn no_changesets_config_object_migrates_nothing() {
    let dir = npm_workspace(None);
    let migration = migrate_in(dir.path());
    assert!(apply(&migration).is_none() && migration.public_scoped.is_empty());
    assert!(migration.diagnostics.is_empty() && migration.base_branch.is_none());

    std::fs::create_dir_all(dir.path().join(".changeset")).unwrap();
    std::fs::write(dir.path().join(".changeset/config.json"), "[1, 2, 3]").unwrap();
    assert!(migrate_in(dir.path()).diagnostics.is_empty(), "not a JSON object");
}

// A malformed value is reported, not translated or guessed at.
#[test]
fn malformed_values_are_reported_and_not_translated() {
    let dir = npm_workspace(Some(r#"{"fixed":"@acme/*","baseBranch":7,"access":"public"}"#));
    let migration = migrate_in(dir.path());
    assert!(apply(&migration).is_none() && migration.base_branch.is_none());
    assert!(message_for(&migration, "fixed").contains("list of lists"));
    assert!(message_for(&migration, "baseBranch").contains("not a branch name"));
}

// Fixed versioning already groups everything; carried-over groups are dropped and the drop is reported.
#[test]
fn fixed_versioning_drops_carried_over_groups_with_a_warning() {
    let dir = npm_workspace(Some(CHANGESETS_V2));
    let migration = migrate_in(dir.path());
    let facts = detect_in(dir.path()).unwrap();

    let mut independent = INDEPENDENT;
    let carried = migration.apply_fixed_groups(&mut independent).unwrap();
    assert!(carried.message.contains("became"), "{}", carried.message);
    let config = parse(&render_config(&facts, &independent));
    let groups = config["fixed-group"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["name"].as_str(), Some("fixed-1"));
    assert_eq!(groups[0]["members"].as_array().unwrap().len(), 4);

    let mut fixed = InitAnswers {
        versioning: Versioning::Fixed,
        ..INDEPENDENT
    };
    let dropped = migration.apply_fixed_groups(&mut fixed).unwrap();
    assert!(dropped.message.contains("`fixed`") && dropped.message.contains("--versioning fixed"));
    let config = parse(&render_config(&facts, &fixed));
    assert_eq!(config["fixed-group"][0]["name"].as_str(), Some("all"));

    let none = migrate_in(npm_workspace(None).path());
    assert!(none.apply_fixed_groups(&mut fixed).is_none(), "nothing to drop");
}

// `baseBranch` has no config key; it is reported only when no workflow consumes it.
#[test]
fn base_branch_is_reported_unused_only_when_present() {
    let dir = npm_workspace(Some(r#"{"baseBranch":"release","access":"public"}"#));
    let migration = migrate_in(dir.path());
    assert!(migration.diagnostics.is_empty());
    assert!(migration.base_branch_unused().unwrap().message.contains("release"));
    assert_eq!(
        base_branch(dir.path()).as_ref().map(BranchName::as_str),
        Some("release")
    );
    let bare = npm_workspace(None);
    assert!(migrate_in(bare.path()).base_branch_unused().is_none());
    assert!(base_branch(bare.path()).is_none());
}
