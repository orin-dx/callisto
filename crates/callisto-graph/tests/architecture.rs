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
use std::path::{Path, PathBuf};

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
];

const MANIFEST_LITERALS: [&str; 3] = ["\"Cargo.toml\"", "\"package.json\"", "\"pyproject.toml\""];
const PARSE_MARKERS: [&str; 3] = ["toml_edit::Document", "serde_json::from_str", ".parse::<toml_edit"];

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR for this test binary is crates/callisto-graph;
    // the workspace root is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("callisto-graph must be nested two levels under the workspace root")
        .to_path_buf()
}

/// Strips every `#[cfg(test)] mod ... { ... }` block from `content` via
/// brace-depth tracking, rather than truncating at the first occurrence --
/// several files in this workspace interleave multiple `#[cfg(test)]`
/// modules with production code rather than appending one at the very end
/// (e.g. `locate/membership.rs`).
fn strip_test_modules(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut out = String::with_capacity(content.len());
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "#[cfg(test)]" {
            i += 1;
            let mut depth: i32 = 0;
            let mut started = false;
            while i < lines.len() {
                for ch in lines[i].chars() {
                    match ch {
                        '{' => {
                            depth += 1;
                            started = true;
                        }
                        '}' => depth -= 1,
                        _ => {}
                    }
                }
                i += 1;
                if started && depth <= 0 {
                    break;
                }
            }
            continue;
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

fn collect_rust_src_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_src_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

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

// --- SPEC-ARCH-RELEASE-ERROR-TAXONOMY, PR1 -------------------------------
//
// Enforcement for the `GraphError::ReleaseIntentStale` (E124) struct-variant
// migration: staleness must carry a real `StaleReason`, real reasons are
// constructible only from `commands/release.rs`, and every other prior
// discard site is a tracked, shrinking migration ratchet rather than a
// silent, permanent hole.

/// This crate's own `src/` directory, independent of `workspace_root()`
/// (which points at the whole-workspace root two levels up).
fn graph_crate_src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Reads `relative` (forward-slash separated, relative to this crate's
/// `src/`, e.g. `"commands/release.rs"`) with `#[cfg(test)] mod tests { ... }`
/// blocks stripped.
fn read_production_source(relative: &str) -> String {
    let path = graph_crate_src_dir().join(relative);
    let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {path:?}: {e}"));
    strip_test_modules(&content)
}

/// `StaleReason`'s four real fresh-re-observation constructors (AC-001).
/// `legacy_unclassified` is deliberately excluded: it is the migration
/// ratchet, callable crate-wide by design during PR1-PR4.
const STALE_REASON_REAL_CONSTRUCTORS: [&str; 4] = [
    "trust_evidence_changed",
    "source_identity_changed",
    "git_remote_changed",
    "intent_differs_from_fresh_derivation",
];

/// AC-002: the real fresh-re-observation privacy boundary is the four named
/// constructors, not the `ReleaseIntentStale` variant name -- that name
/// legitimately appears crate-wide via the `StaleReason::legacy_unclassified()`
/// ratchet while PR1-PR4 migrate every non-fresh-reobservation site.
#[test]
fn release_intent_stale_is_constructed_only_by_fresh_reobservation() {
    let release_rs = graph_crate_src_dir().join("commands/release.rs");
    let mut files = Vec::new();
    collect_rust_src_files(&graph_crate_src_dir(), &mut files);

    let mut violations = Vec::new();
    for file in files {
        if file == release_rs {
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
        "StaleReason's real fresh-re-observation constructors must be referenced only from \
         commands/release.rs; every other production site must use \
         StaleReason::legacy_unclassified() during the PR1-PR4 migration: {violations:#?}"
    );
}

/// AC-001: the four real constructors carry no visibility modifier --
/// callable only from within `commands/release.rs` itself.
#[test]
fn stale_reason_constructors_are_module_private() {
    let production = read_production_source("commands/release.rs");

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
/// Two distinct categories, both pre-existing and verified by re-reading the
/// crate rather than assumed:
/// - The three release-executor files are this spec's own PR1-out-of-scope
///   sites (PR2 migrates `release_execution.rs`, PR3 `release_decision.rs`,
///   PR4 `release.rs`) -- shrink these three to nothing by PR4.
/// - The other five predate this spec entirely and belong to the companion
///   SPEC-ARCH-ERROR-SOURCE-PRESERVATION-GATE.json, which widens this same
///   check workspace-wide; not migrated here.
///
/// Either way, this test's job is to block a *sixth* (or new PR1-scope)
/// file from adopting the pattern, not to re-litigate already-tracked ones.
const MAP_ERR_IGNORE_ALLOWLIST: &[&str] = &[
    "commands/release.rs",
    "commands/release_decision.rs",
    "commands/release_execution.rs",
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

/// AC-003 (PR1 scope): zero `map_err(|_ident| ...)` source-discarding
/// closures anywhere in this crate except the three allowlisted
/// release-executor files. The allowlist must shrink to empty by PR4
/// (SPEC-ARCH-RELEASE-ERROR-TAXONOMY.json's `suggested_pr_sequence`); this
/// test still fails on a fourth, non-allowlisted file adopting the pattern.
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
        "found a new map_err(|_ident| ...) source-discarding closure outside the tracked PR1-PR4 \
         allowlist ({MAP_ERR_IGNORE_ALLOWLIST:?}); report the real error cause instead: {violations:#?}"
    );
}

/// AC-015 migration ratchet: exact `StaleReason::legacy_unclassified()` call
/// counts pinned per file as of PR1. PR2 lowers `release_execution.rs` to 0,
/// PR3 lowers `release_decision.rs` to 0, PR4 lowers `release.rs` to 0 and
/// deletes the constructor (and this test) entirely. Update a count here only
/// when the corresponding PR intentionally lowers it -- never to silence a
/// failure.
#[test]
fn legacy_unclassified_ratchet() {
    let cases: &[(&str, usize)] = &[
        ("commands/release.rs", 44),
        ("commands/release_decision.rs", 42),
        ("commands/release_execution.rs", 2),
    ];
    for (relative, expected) in cases {
        let production = read_production_source(relative);
        let actual = production.matches("StaleReason::legacy_unclassified()").count();
        assert_eq!(
            actual, *expected,
            "{relative}: StaleReason::legacy_unclassified() count drifted from the pinned PR1 ratchet"
        );
    }
}

/// AC-014: E124 is the sole owner of "reapprove" guidance -- every other
/// release-error variant's help text must describe its own real recovery
/// action instead of copying E124's.
#[test]
fn reapprove_guidance_belongs_to_e124_only() {
    let production = read_production_source("error.rs");
    let count = production.matches("reapprove").count();
    assert_eq!(
        count, 1,
        "'reapprove' must occur exactly once in error.rs (E124's help text only); found {count}"
    );
}
