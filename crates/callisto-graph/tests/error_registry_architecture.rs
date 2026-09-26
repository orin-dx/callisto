//! Enforcement test for the one-`E####`-registry decision (`docs/errors.md`):
//! every `miette::Diagnostic`-deriving enum variant across the workspace must
//! carry its own `#[diagnostic(code(...))]`, or be `#[diagnostic(transparent)]`
//! to forward an inner error's code -- never neither, which silently drops
//! the diagnostic code a `--format json` consumer or a support conversation
//! keys off. It also fails if any `callisto::<name>`-style code survives
//! anywhere in a `code(...)` attribute; every code is now a numeric `E####`.
//!
//! This is a coarse, source-text scan (no `syn` dependency in this crate),
//! not a full parser -- see `strip_cfg_test_modules` and `find_enum_body` for
//! the brace-depth heuristics it relies on, which the workspace's own
//! `#[error("...")]` format strings happen to keep balanced.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("callisto-graph must be nested two levels under the workspace root")
        .to_path_buf()
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

/// Every crate's `src/` directory, e.g. `crates/callisto-graph/src`.
fn all_crate_src_dirs() -> Vec<PathBuf> {
    let crates_dir = workspace_root().join("crates");
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&crates_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let src = entry.path().join("src");
        if src.is_dir() {
            out.push(src);
        }
    }
    out
}

/// Strips every `#[cfg(test)] mod ... { ... }` block via brace-depth
/// tracking, so a test-double `#[diagnostic(code(callisto::foo))]` fixture
/// (or a test enum with no diagnostic at all) is never mistaken for
/// production code.
fn strip_cfg_test_modules(content: &str) -> String {
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
        } else {
            out.push_str(lines[i]);
            out.push('\n');
            i += 1;
        }
    }
    out
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

/// A variant slice is compliant if it is `#[diagnostic(transparent)]`, or
/// carries its own `code(...)`. Help text is not required here -- many
/// numeric codes are documented in `docs/errors.md` with no help text by
/// design, and this test enforces the code registry, not help coverage.
fn variant_has_code_or_is_transparent(variant_text: &str) -> bool {
    variant_text.contains("#[diagnostic(transparent)]") || variant_text.contains("code(")
}

#[test]
fn every_diagnostic_variant_has_a_code_or_is_transparent() {
    let mut violations = Vec::new();
    for src_dir in all_crate_src_dirs() {
        let mut files = Vec::new();
        collect_rust_src_files(&src_dir, &mut files);
        for file in files {
            let Ok(raw) = fs::read_to_string(&file) else {
                continue;
            };
            let content = strip_cfg_test_modules(&raw);
            for diag_enum in find_diagnostic_enums(&content) {
                for variant in split_variants(&diag_enum.body) {
                    if !variant_has_code_or_is_transparent(variant) {
                        let variant_name = variant
                            .rsplit("#[")
                            .next()
                            .unwrap_or(variant)
                            .lines()
                            .find(|l| !l.trim().is_empty())
                            .unwrap_or("<unknown variant>")
                            .trim();
                        violations.push(format!(
                            "{}: enum {} has a variant with neither a code nor #[diagnostic(transparent)]: {}",
                            file.display(),
                            diag_enum.name,
                            variant_name
                        ));
                    }
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "found error variant(s) missing a diagnostic code and not marked transparent:\n{}",
        violations.join("\n")
    );
}

#[test]
fn no_callisto_prefixed_diagnostic_codes_remain() {
    let mut violations = Vec::new();
    for src_dir in all_crate_src_dirs() {
        let mut files = Vec::new();
        collect_rust_src_files(&src_dir, &mut files);
        for file in files {
            let Ok(content) = fs::read_to_string(&file) else {
                continue;
            };
            if content.contains("code(callisto::") {
                violations.push(file.display().to_string());
            }
        }
    }
    assert!(
        violations.is_empty(),
        "found a `code(callisto::...)` diagnostic code outside the one E#### registry; \
         every diagnostic code is now numeric (see docs/errors.md): {violations:#?}"
    );
}

#[test]
fn find_diagnostic_enums_finds_a_small_fixture() {
    let src = r#"
        #[derive(Debug, thiserror::Error, miette::Diagnostic)]
        #[non_exhaustive]
        pub enum Foo {
            #[error("a")]
            #[diagnostic(code(E001), help("do it"))]
            A,
            #[error(transparent)]
            #[diagnostic(transparent)]
            B(#[from] std::io::Error),
        }
    "#;
    let enums = find_diagnostic_enums(src);
    assert_eq!(enums.len(), 1);
    assert_eq!(enums[0].name, "Foo");
    let variants = split_variants(&enums[0].body);
    assert_eq!(variants.len(), 2);
    assert!(variant_has_code_or_is_transparent(variants[0]));
    assert!(variant_has_code_or_is_transparent(variants[1]));
}

#[test]
fn detects_a_codeless_non_transparent_variant_in_a_fixture() {
    let src = r#"
        #[derive(Debug, thiserror::Error, miette::Diagnostic)]
        pub enum Foo {
            #[error("a")]
            A,
        }
    "#;
    let enums = find_diagnostic_enums(src);
    let variants = split_variants(&enums[0].body);
    assert!(!variant_has_code_or_is_transparent(variants[0]));
}
