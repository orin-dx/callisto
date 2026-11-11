//! The `docs/errors.md` registry and the `#[diagnostic(code(E###))]` attributes in `crates/*/src` agree:
//! codes are numeric and unique, documented and not stale, and every Diagnostic variant has one or is transparent.

use std::collections::BTreeMap;
use std::fs;

mod common;
use common::{collect_rust_src_files, strip_test_modules, workspace_root};

/// Every non-test source file under `crates/*/src`, as (path, text without test modules).
fn workspace_sources() -> Vec<(String, String)> {
    let mut files = Vec::new();
    for krate in fs::read_dir(workspace_root().join("crates")).unwrap().flatten() {
        collect_rust_src_files(&krate.path().join("src"), &mut files);
    }
    files
        .iter()
        .map(|f| {
            (
                f.display().to_string(),
                strip_test_modules(&fs::read_to_string(f).unwrap()),
            )
        })
        .collect()
}

/// Each `code(...)` argument found in `text`.
fn code_args(text: &str) -> Vec<&str> {
    text.match_indices("code(")
        .filter(|(i, _)| text[..*i].ends_with(|c: char| c == '(' || c == ',' || c.is_whitespace()))
        .map(|(i, m)| text[i + m.len()..].split([')', ',']).next().unwrap().trim())
        .collect()
}

#[test]
fn codes_are_numeric_unique_and_match_the_docs() {
    let mut in_code: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let sources = workspace_sources();
    for (file, text) in &sources {
        for code in code_args(text) {
            let numeric = code
                .strip_prefix('E')
                .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
            assert!(numeric, "{file}: diagnostic code `{code}` is not E followed by digits");
            in_code.entry(code).or_default().push(file);
        }
    }
    for (code, files) in &in_code {
        assert_eq!(files.len(), 1, "duplicate diagnostic code {code}: {files:?}");
    }
    let docs = fs::read_to_string(workspace_root().join("docs/errors.md")).unwrap();
    let documented: Vec<&str> = docs
        .lines()
        .filter_map(|l| l.strip_prefix("| E")?.split(" |").next())
        .collect();
    let documented: Vec<String> = documented.iter().map(|d| format!("E{d}")).collect();
    let in_code: Vec<&str> = in_code.keys().copied().collect();
    let undocumented: Vec<_> = in_code.iter().filter(|c| !documented.iter().any(|d| d == *c)).collect();
    let stale: Vec<_> = documented.iter().filter(|d| !in_code.contains(&d.as_str())).collect();
    assert!(
        undocumented.is_empty(),
        "codes missing from docs/errors.md: {undocumented:?}"
    );
    assert!(stale.is_empty(), "documented codes absent from code: {stale:?}");
}

/// One `pub enum Name { ... }` (or `enum Name { ... }`) block found preceded
/// by a `#[derive(...)]` mentioning `Diagnostic`.
struct DiagnosticEnum {
    name: String,
    body: String,
}

/// Finds every enum in `content` whose nearest preceding `#[derive(...)]`
/// mentions `Diagnostic` (`miette::Diagnostic` or a `use`d bare `Diagnostic`),
/// and extracts its brace-balanced body.
///
/// Relies on the workspace's own convention that `#[derive(...)]` sits
/// directly above the enum, with at most a couple of attributes
/// (`#[non_exhaustive]`, `#[allow(...)]`) in between -- true of every error
/// enum in this workspace at the time this test was written.
fn find_diagnostic_enums(content: &str) -> Vec<DiagnosticEnum> {
    let mut out = Vec::new();
    let mut search_from = 0usize;
    while let Some(derive_start) = content[search_from..].find("#[derive(") {
        let derive_start = search_from + derive_start;
        let Some(derive_end_rel) = content[derive_start..].find(")]") else {
            break;
        };
        let derive_end = derive_start + derive_end_rel + 2;
        let derive_text = &content[derive_start..derive_end];
        search_from = derive_end;

        if !derive_text.contains("Diagnostic") {
            continue;
        }

        // The enum keyword must appear within a short lookahead window (only
        // other attributes, doc comments, or whitespace in between).
        let lookahead_end = (derive_end + 400).min(content.len());
        let window = &content[derive_end..lookahead_end];
        let Some(enum_kw) = window.find("enum ") else {
            continue;
        };
        let after_kw = &window[enum_kw + "enum ".len()..];
        let name_end = after_kw.find([' ', '{', '<']).unwrap_or(after_kw.len());
        let name = after_kw[..name_end].trim().to_string();

        let Some(brace_open_rel) = window[enum_kw..].find('{') else {
            continue;
        };
        let brace_open = derive_end + enum_kw + brace_open_rel;

        // Brace-depth walk to find the matching close. Balanced within
        // `#[error("...")]` format strings (each `{field}` is itself
        // balanced), so a naive char-level count still lands correctly.
        let mut depth: i32 = 0;
        let mut close = None;
        for (offset, ch) in content[brace_open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(brace_open + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else {
            continue;
        };

        out.push(DiagnosticEnum {
            name,
            body: content[brace_open + 1..close].to_string(),
        });
        search_from = close;
    }
    out
}

/// Splits an enum body into per-variant text slices at each `#[error(`
/// occurrence -- thiserror requires every variant to carry its own
/// `#[error(...)]`, so this partitions correctly regardless of field shape.
fn split_variants(body: &str) -> Vec<&str> {
    let starts: Vec<usize> = body.match_indices("#[error(").map(|(i, _)| i).collect();
    let mut slices = Vec::new();
    for (idx, &start) in starts.iter().enumerate() {
        let end = starts.get(idx + 1).copied().unwrap_or(body.len());
        slices.push(&body[start..end]);
    }
    slices
}

/// A variant slice is compliant if it is `#[diagnostic(transparent)]` or carries its own `code(...)`.
fn variant_has_code_or_is_transparent(variant_text: &str) -> bool {
    variant_text.contains("#[diagnostic(transparent)]") || variant_text.contains("code(")
}

#[test]
fn every_diagnostic_variant_has_a_code_or_is_transparent() {
    let mut violations = Vec::new();
    for (file, text) in workspace_sources() {
        for diag_enum in find_diagnostic_enums(&text) {
            for variant in split_variants(&diag_enum.body) {
                if !variant_has_code_or_is_transparent(variant) {
                    violations.push(format!(
                        "{file}: enum {}: {}",
                        diag_enum.name,
                        variant.lines().next().unwrap_or("")
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "variants with neither a code nor #[diagnostic(transparent)]:\n{}",
        violations.join("\n")
    );
}
