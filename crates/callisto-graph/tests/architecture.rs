//! Enforcement test for audit pattern B: outside `callisto-model` and
//! `callisto-manifests`, no crate should hand-parse `Cargo.toml`/
//! `package.json`/`pyproject.toml` (i.e. reference one of those file names
//! as a raw string literal *and* independently invoke a raw TOML/JSON
//! parse in the same file) instead of going through the shared readers in
//! `callisto_manifests` (`cargo_package_name`/`npm_package_name`/
//! `python_package_name`, `read_identity`, `read_napi_targets`).
//!
//! This is a coarse, file-level, production-code-only grep -- a regression
//! tripwire against *new* hand-parsing, not a precise static analyzer. Test
//! modules (`#[cfg(test)] mod ...`) are stripped before checking, since
//! test fixtures legitimately write out manifest files with these names.
//!
//! A handful of files are allowlisted below because they parse these
//! manifests for a genuinely different purpose than package-identity
//! (name/version) extraction -- see each entry's reason.

use std::fs;
use std::path::PathBuf;

mod common;
use common::{collect_rust_src_files, strip_test_modules, workspace_root};

/// Files allowed to contain a manifest-filename literal alongside a raw
/// parse call in production code, and why. Paths are relative to the
/// workspace root, forward-slash separated.
const ALLOWLIST: &[(&str, &str)] = &[
    (
        "crates/callisto-graph/src/locate/root.rs",
        "workspace-root marker detection (`[workspace]` / `workspaces` field presence), \
         not package identity extraction",
    ),
    (
        "crates/callisto-graph/src/locate/membership.rs",
        "workspace membership (members/exclude/workspaces glob) parsing, not package identity \
         extraction; already routes canonical manifest file names through \
         `ManifestFormat::*.file_name()` (audit pattern B, item 3)",
    ),
    (
        "crates/callisto-graph/src/napi.rs",
        "parses package.json into a serde_json::Value only to hand to the shared \
         callisto_manifests::read_napi_targets -- the field-extraction logic itself lives in \
         callisto-manifests, not here",
    ),
    (
        "crates/callisto-graph/src/matrix.rs",
        "parses package.json/pyproject.toml generically to extract non-identity fields \
         (engines.node, requires-python, [tool.maturin]); napi.targets extraction itself is \
         delegated to the shared callisto_manifests::read_napi_targets",
    ),
    (
        "crates/callisto-graph/src/commands/release_decision.rs",
        "the serde_json parse decodes the committed release decision; historical manifest blobs \
         are parsed by callisto_manifests::read_effective_version",
    ),
];

const MANIFEST_LITERALS: [&str; 3] = ["\"Cargo.toml\"", "\"package.json\"", "\"pyproject.toml\""];
const PARSE_MARKERS: [&str; 3] = ["toml_edit::Document", "serde_json::from_str", ".parse::<toml_edit"];

#[test]
fn no_hand_rolled_manifest_parsing_outside_model_and_manifests_crates() {
    let root = workspace_root();
    let crates_dir = root.join("crates");

    let crate_dirs = fs::read_dir(&crates_dir)
        .unwrap_or_else(|e| panic!("could not read workspace crates dir at {crates_dir:?}: {e}"));

    let mut violations = Vec::new();

    for entry in crate_dirs.flatten() {
        let crate_path = entry.path();
        if !crate_path.is_dir() {
            continue;
        }
        let crate_name = crate_path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if crate_name == "callisto-model" || crate_name == "callisto-manifests" {
            continue;
        }

        let src_dir = crate_path.join("src");
        if !src_dir.is_dir() {
            continue;
        }

        let mut files = Vec::new();
        collect_rust_src_files(&src_dir, &mut files);

        for file in files {
            let Ok(content) = fs::read_to_string(&file) else {
                continue;
            };
            let production = strip_test_modules(&content);

            let has_manifest_literal = MANIFEST_LITERALS.iter().any(|lit| production.contains(lit));
            let has_raw_parse = PARSE_MARKERS.iter().any(|marker| production.contains(marker));

            if has_manifest_literal && has_raw_parse {
                let rel = file
                    .strip_prefix(&root)
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .replace('\\', "/");
                if !ALLOWLIST.iter().any(|(path, _reason)| *path == rel) {
                    violations.push(rel);
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "found hand-rolled manifest parsing outside callisto-model/callisto-manifests (a \
         Cargo.toml/package.json/pyproject.toml literal alongside a raw toml_edit/serde_json \
         parse call) in production code -- route through callisto_manifests::{{cargo_package_name, \
         npm_package_name, python_package_name, read_identity, read_napi_targets}} instead, or add \
         a justified entry to this test's ALLOWLIST: {violations:#?}"
    );
}

/// Keeps the allowlist honest: an entry naming a file that no longer exists
/// (renamed, deleted, or fully migrated) should be removed, not silently
/// carried forward.
#[test]
fn allowlist_entries_reference_files_that_still_exist() {
    let root = workspace_root();
    for (path, _reason) in ALLOWLIST {
        assert!(
            root.join(path).exists(),
            "allowlisted path no longer exists, remove this entry: {path}"
        );
    }
}

// --- Typed release errors ------------------------------------------------
//
// Enforcement for the `GraphError::ReleaseIntentStale` (E124) struct-variant
// migration: staleness carries a real `StaleReason`, constructible only
// from the `commands/release/` module (a directory module since the
// provider-port split; `StaleReason` itself lives in its `mod.rs`). The `StaleReason::legacy_unclassified()`
// migration ratchet used during PR1-PR3 reached zero callers across all
// three release-executor files in PR4 and was deleted, along with the
// ratchet's own pinned-count test.

/// This crate's own `src/` directory, independent of `workspace_root()`
/// (which points at the whole-workspace root two levels up).
fn graph_crate_src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Reads `relative` (forward-slash separated, relative to this crate's
/// `src/`, e.g. `"commands/release/mod.rs"`) with `#[cfg(test)] mod tests { ... }`
/// blocks stripped.
fn read_production_source(relative: &str) -> String {
    let path = graph_crate_src_dir().join(relative);
    let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {path:?}: {e}"));
    strip_test_modules(&content)
}

/// `StaleReason`'s four fresh-re-observation constructors.
const STALE_REASON_REAL_CONSTRUCTORS: [&str; 4] = [
    "trust_evidence_changed",
    "source_identity_changed",
    "git_remote_changed",
    "intent_differs_from_fresh_derivation",
];

/// `GraphError::ReleaseIntentStale` is constructed only via one of
/// `StaleReason`'s four fresh-re-observation constructors, and only from
/// the `commands/release/` module; no other construction site exists workspace-wide.
#[test]
fn release_intent_stale_is_constructed_only_by_fresh_reobservation() {
    let release_module = graph_crate_src_dir().join("commands/release");
    let mut files = Vec::new();
    collect_rust_src_files(&graph_crate_src_dir(), &mut files);

    let mut violations = Vec::new();
    for file in files {
        if file.starts_with(&release_module) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };
        let production = strip_test_modules(&content);
        for name in STALE_REASON_REAL_CONSTRUCTORS {
            if production.contains(name) {
                violations.push(format!("{}: references `{name}`", file.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "StaleReason's fresh-re-observation constructors must be referenced only from \
         the commands/release/ module: {violations:#?}"
    );
}

/// The four real constructors carry no visibility modifier --
/// callable only from within the `commands/release/` module itself.
#[test]
fn stale_reason_constructors_are_module_private() {
    let production = read_production_source("commands/release/mod.rs");

    let mut violations = Vec::new();
    for name in STALE_REASON_REAL_CONSTRUCTORS {
        let needle = format!("fn {name}");
        for line in production.lines() {
            if line.contains(&needle) && line.contains("pub") {
                violations.push(format!("`{name}` has a visibility modifier: {}", line.trim()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "StaleReason's real constructors must carry no pub/pub(crate)/pub(super) visibility: {violations:#?}"
    );
}

/// Files still permitted to discard their `map_err` closure's bound error.
/// The release-executor files are not allowlisted. The remaining five
/// predate this check and are not migrated yet.
///
/// This test's job is to block a new file from adopting the pattern, not to
/// re-litigate already-tracked ones.
const MAP_ERR_IGNORE_ALLOWLIST: &[&str] = &[
    "locate/ignore_walk.rs",
    "config/resolve.rs",
    "cascade.rs",
    "commands/publish.rs",
    "commands/snapshot.rs",
];

/// True if `content` contains a `map_err(|_ident| ...)` closure -- one whose
/// bound error parameter is never used, evading `clippy::map_err_ignore`
/// (which only matches the bare `|_|` spelling).
fn contains_map_err_ignore(content: &str) -> bool {
    let marker = "map_err(|_";
    let mut search_from = 0;
    while let Some(relative_pos) = content[search_from..].find(marker) {
        // `marker` itself ends in the closure parameter's leading `_`.
        let ident_start = search_from + relative_pos + marker.len() - 1;
        let rest = &content[ident_start..];
        let ident_len = rest
            .char_indices()
            .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
            .count();
        if rest[ident_len..].starts_with('|') {
            return true;
        }
        search_from = ident_start + ident_len.max(1);
    }
    false
}

/// Zero `map_err(|_ident| ...)` source-discarding closures anywhere
/// in this crate except the allowlisted, pre-existing files; this test fails on any
/// non-allowlisted file adopting the pattern.
#[test]
fn release_modules_never_discard_error_sources() {
    let src_dir = graph_crate_src_dir();
    let mut files = Vec::new();
    collect_rust_src_files(&src_dir, &mut files);

    let mut violations = Vec::new();
    for file in files {
        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };
        let production = strip_test_modules(&content);
        if !contains_map_err_ignore(&production) {
            continue;
        }
        let relative = file
            .strip_prefix(&src_dir)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        if !MAP_ERR_IGNORE_ALLOWLIST.contains(&relative.as_str()) {
            violations.push(relative);
        }
    }

    assert!(
        violations.is_empty(),
        "found a new map_err(|_ident| ...) source-discarding closure outside the \
         allowlist ({MAP_ERR_IGNORE_ALLOWLIST:?}); report the real error cause instead: {violations:#?}"
    );
}

/// True if `content` grants an `ApplyPermit` from a literal `true`/`false` instead of the
/// parsed `--dry-run` flag, ignoring whitespace so a reformatted call is still caught.
fn grants_permit_from_literal(content: &str) -> bool {
    let compact: String = content.split_whitespace().collect();
    ["true", "false"].iter().any(|literal| {
        compact.contains(&format!("granted_unless_dry_run({literal})"))
            || compact.contains(&format!("granted_unless_dry_run({literal},)"))
    })
}

/// `ApplyPermit::granted_unless_dry_run` must take the parsed flag read once near the top of a
/// handler. A literal re-derives the decision and can silently disagree with `--dry-run`.
#[test]
fn apply_permits_are_never_granted_from_a_literal() {
    let root = workspace_root();
    let mut violations = Vec::new();
    for entry in fs::read_dir(root.join("crates")).unwrap().flatten() {
        let src_dir = entry.path().join("src");
        if !src_dir.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        collect_rust_src_files(&src_dir, &mut files);
        for file in files {
            if file.file_name().and_then(|n| n.to_str()) == Some("permit.rs") {
                continue;
            }
            let Ok(content) = fs::read_to_string(&file) else {
                continue;
            };
            if grants_permit_from_literal(&strip_test_modules(&content)) {
                violations.push(file.strip_prefix(&root).unwrap_or(&file).to_string_lossy().into_owned());
            }
        }
    }
    assert!(
        violations.is_empty(),
        "pass the parsed dry-run flag to ApplyPermit::granted_unless_dry_run, never a literal: {violations:#?}"
    );
}

#[test]
fn literal_permit_detector_catches_a_reformatted_call_and_ignores_the_flag() {
    assert!(grants_permit_from_literal("ApplyPermit::granted_unless_dry_run(false)"));
    assert!(grants_permit_from_literal(
        "ApplyPermit::granted_unless_dry_run(\n    true,\n)"
    ));
    assert!(!grants_permit_from_literal(
        "ApplyPermit::granted_unless_dry_run(global.dry_run)"
    ));
}
