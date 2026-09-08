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
