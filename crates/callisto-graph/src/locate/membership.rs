use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

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
