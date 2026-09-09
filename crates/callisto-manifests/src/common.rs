//! Helpers shared between ecosystem-specific manifest editors (`cargo`, `npm`, `python`).
//!
//! Only functions verified to be byte-for-byte identical in behavior across
//! ecosystems live here. Functions that merely *look* similar but encode
//! ecosystem-specific semantics (e.g. `render_at_precision`, which differs
//! between Cargo and npm on prerelease/hyphen handling) stay local to their
//! respective modules.

/// Returns true if `s` looks like a bare `major.minor.patch` semver string
/// (as opposed to a range expression like `^1.0.0` or `>=1.0.0`).
///
/// This is a syntactic heuristic: it does not fully validate the string as
/// a semver version, it just checks that it starts with an ASCII digit and
/// splits into exactly three dot-separated parts.
pub(crate) fn is_bare_semver(s: &str) -> bool {
    let chars = s.chars().next();
    if !chars.is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3
}

/// Splits a version requirement clause into its leading comparison operator
/// (if any) and the remaining version-ish text.
///
/// Recognized operators: `^`, `~`, `>=`, `>`, `<=`, `<`, `=`. If none match,
/// the empty-string prefix is returned along with the trimmed input.
pub(crate) fn split_single_operator_prefix(s: &str) -> Option<(&str, &str)> {
    let trimmed = s.trim();
    for op in ["^", "~", ">=", ">", "<=", "<", "="] {
        if let Some(rest) = trimmed.strip_prefix(op) {
            return Some((op, rest.trim()));
        }
    }
    Some(("", trimmed))
}

/// Sets `table[key]` to a scalar string value, preserving the existing
/// entry's decor (comments/whitespace) when `table[key]` is already a plain
/// value; otherwise inserts a fresh `key = "value"` entry with no decor to
/// preserve.
///
/// Takes `&mut dyn toml_edit::TableLike` rather than a concrete `Table` so
/// every call site can share this one implementation regardless of what
/// container it holds: a bare `toml_edit::Table` (Cargo's `[package]` /
/// `[workspace.package]` tables coerce to `&mut dyn TableLike` directly,
/// since `Table: TableLike`) or a `toml_edit::Item` that resolves to a
/// table-like value via `Item::as_table_like_mut()` (pyproject.toml's
/// `[project]` / `[tool.poetry]` / `[tool.flit.metadata]`, each accessed as
/// an `Item`, not a bare `Table`). Previously duplicated as
/// `cargo::set_scalar_preserving_decor` and a local closure inside
/// `python::PyprojectToml::write_version`.
pub(crate) fn set_scalar_preserving_decor(table: &mut dyn toml_edit::TableLike, key: &str, new_value: &str) {
    if let Some(item) = table.get_mut(key) {
        if let Some(val) = item.as_value_mut() {
            let decor = val.decor().clone();
            let mut new_val = toml_edit::Value::from(new_value);
            *new_val.decor_mut() = decor;
            *val = new_val;
            return;
        }
    }
    table.insert(key, toml_edit::value(new_value));
}

/// A file's line-ending style: bare LF, or CRLF (common on Windows / repos
/// with `core.autocrlf=true`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineEnding {
    Lf,
    CrLf,
}

/// Byte-level formatting facts that a CST-preserving editor must detect on
/// open and reapply verbatim on persist, since `toml_edit`/`serde_json`
/// re-serialization always produces bare-LF text with no BOM regardless of
/// what was originally on disk: whether the file opened with a UTF-8 BOM,
/// and its line-ending style.
///
/// Previously reimplemented independently -- byte-for-byte identically --
/// as a private `has_bom`/`line_ending` pair (plus a duplicate local
/// `LineEnding` enum) in both `npm::PackageJson` and
/// `python::PyprojectToml`, and entirely absent from `cargo::CargoToml`
/// (F4: a CRLF `Cargo.toml` was silently rewritten to LF on any write).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FormatFingerprint {
    pub(crate) has_bom: bool,
    pub(crate) line_ending: LineEnding,
}

impl FormatFingerprint {
    /// Detects BOM and line-ending style from `content`, the raw file text
    /// exactly as read from disk (BOM still present, if any -- this does
    /// its own BOM-stripping internally before checking for `"\r\n"`, so
    /// callers need not pre-strip).
    pub(crate) fn detect(content: &str) -> Self {
        let has_bom = content.starts_with('\u{FEFF}');
        let clean = content.strip_prefix('\u{FEFF}').unwrap_or(content);
        let line_ending = if clean.contains("\r\n") {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };
        FormatFingerprint { has_bom, line_ending }
    }

    /// Reapplies this fingerprint to `rendered` -- a freshly-serialized
    /// document body with bare-LF line endings and no BOM -- producing the
    /// exact text that should be written to disk.
    pub(crate) fn apply(&self, rendered: &str) -> String {
        let mut out = if self.line_ending == LineEnding::CrLf {
            rendered.replace("\r\n", "\n").replace('\n', "\r\n")
        } else {
            rendered.to_string()
        };
        if self.has_bom {
            out = format!("\u{FEFF}{out}");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_bare_semver_accepts_three_part_numeric() {
        assert!(is_bare_semver("1.2.3"));
        assert!(is_bare_semver("0.0.0"));
    }

    #[test]
    fn is_bare_semver_rejects_non_digit_start() {
        assert!(!is_bare_semver("^1.2.3"));
        assert!(!is_bare_semver("a.b.c"));
        assert!(!is_bare_semver(""));
    }

    #[test]
    fn is_bare_semver_rejects_wrong_part_count() {
        assert!(!is_bare_semver("1.2"));
        assert!(!is_bare_semver("1.2.3.4"));
        assert!(!is_bare_semver("1"));
    }

    #[test]
    fn is_bare_semver_rejects_prerelease_suffix() {
        // A prerelease suffix on the patch segment (e.g. "3-beta") plus its
        // own dot-separated identifier pushes the split count to 4, so this
        // is correctly rejected as "not bare".
        assert!(!is_bare_semver("1.2.3-beta.1"));
    }

    #[test]
    fn split_single_operator_prefix_recognizes_all_operators() {
        assert_eq!(split_single_operator_prefix("^1.2.3"), Some(("^", "1.2.3")));
        assert_eq!(split_single_operator_prefix("~1.2.3"), Some(("~", "1.2.3")));
        assert_eq!(split_single_operator_prefix(">=1.2.3"), Some((">=", "1.2.3")));
        assert_eq!(split_single_operator_prefix(">1.2.3"), Some((">", "1.2.3")));
        assert_eq!(split_single_operator_prefix("<=1.2.3"), Some(("<=", "1.2.3")));
        assert_eq!(split_single_operator_prefix("<1.2.3"), Some(("<", "1.2.3")));
        assert_eq!(split_single_operator_prefix("=1.2.3"), Some(("=", "1.2.3")));
    }

    #[test]
    fn split_single_operator_prefix_no_operator_returns_empty_prefix() {
        assert_eq!(split_single_operator_prefix("1.2.3"), Some(("", "1.2.3")));
    }

    #[test]
    fn split_single_operator_prefix_trims_whitespace() {
        assert_eq!(split_single_operator_prefix("  ^1.2.3  "), Some(("^", "1.2.3")));
        assert_eq!(split_single_operator_prefix(">=  1.2.3"), Some((">=", "1.2.3")));
    }

    #[test]
    fn set_scalar_preserving_decor_keeps_existing_comment_on_a_bare_table() {
        let mut doc: toml_edit::DocumentMut = "[package]\nversion = \"0.1.0\" # keep me\n".parse().unwrap();
        let table = doc.get_mut("package").and_then(|p| p.as_table_mut()).unwrap();

        set_scalar_preserving_decor(table, "version", "0.2.0");

        assert_eq!(doc.to_string(), "[package]\nversion = \"0.2.0\" # keep me\n");
    }

    #[test]
    fn set_scalar_preserving_decor_inserts_fresh_entry_when_key_absent_on_a_bare_table() {
        let mut doc: toml_edit::DocumentMut = "[package]\nname = \"pkg\"\n".parse().unwrap();
        let table = doc.get_mut("package").and_then(|p| p.as_table_mut()).unwrap();

        set_scalar_preserving_decor(table, "version", "1.0.0");

        assert_eq!(doc.to_string(), "[package]\nname = \"pkg\"\nversion = \"1.0.0\"\n");
    }

    /// Same behavior, but through the `Item::as_table_like_mut()` path
    /// pyproject.toml's `write_version` uses (a `[project]`/`[tool.poetry]`
    /// table is held as an `Item`, not a bare `Table`).
    #[test]
    fn set_scalar_preserving_decor_keeps_existing_comment_via_item_table_like() {
        let mut doc: toml_edit::DocumentMut = "[project]\nversion = \"0.1.0\" # keep me\n".parse().unwrap();
        let table = doc.get_mut("project").and_then(|p| p.as_table_like_mut()).unwrap();

        set_scalar_preserving_decor(table, "version", "0.2.0");

        assert_eq!(doc.to_string(), "[project]\nversion = \"0.2.0\" # keep me\n");
    }

    #[test]
    fn format_fingerprint_detects_plain_lf_no_bom() {
        let fp = FormatFingerprint::detect("[project]\nversion = \"1.0.0\"\n");
        assert_eq!(
            fp,
            FormatFingerprint {
                has_bom: false,
                line_ending: LineEnding::Lf
            }
        );
    }

    #[test]
    fn format_fingerprint_detects_crlf_no_bom() {
        let fp = FormatFingerprint::detect("[project]\r\nversion = \"1.0.0\"\r\n");
        assert_eq!(
            fp,
            FormatFingerprint {
                has_bom: false,
                line_ending: LineEnding::CrLf
            }
        );
    }

    #[test]
    fn format_fingerprint_detects_bom_with_crlf() {
        let fp = FormatFingerprint::detect("\u{FEFF}[project]\r\nversion = \"1.0.0\"\r\n");
        assert_eq!(
            fp,
            FormatFingerprint {
                has_bom: true,
                line_ending: LineEnding::CrLf
            }
        );
    }

    #[test]
    fn format_fingerprint_detects_bom_with_lf() {
        let fp = FormatFingerprint::detect("\u{FEFF}[project]\nversion = \"1.0.0\"\n");
        assert_eq!(
            fp,
            FormatFingerprint {
                has_bom: true,
                line_ending: LineEnding::Lf
            }
        );
    }

    #[test]
    fn format_fingerprint_apply_round_trips_crlf_and_bom() {
        let fp = FormatFingerprint {
            has_bom: true,
            line_ending: LineEnding::CrLf,
        };
        let rendered = "[project]\nversion = \"1.0.0\"\n";
        assert_eq!(fp.apply(rendered), "\u{FEFF}[project]\r\nversion = \"1.0.0\"\r\n");
    }

    #[test]
    fn format_fingerprint_apply_is_identity_for_plain_lf_no_bom() {
        let fp = FormatFingerprint {
            has_bom: false,
            line_ending: LineEnding::Lf,
        };
        let rendered = "[project]\nversion = \"1.0.0\"\n";
        assert_eq!(fp.apply(rendered), rendered);
    }
}
