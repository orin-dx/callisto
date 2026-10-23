//! The hints Callisto writes into a release tag's annotation so a later read can skip work.
//!
//! A hint never decides identity: a tag without one, or with one that does not parse, is read exactly as it always was.

use std::collections::BTreeMap;

const HINTS_KEY: &str = "Callisto-Hints";
const PACKAGE_KEY: &str = "Callisto-Package";
const TREE_KEY: &str = "Callisto-Tree";

/// The version of the trailers this release writes and the only one it understands.
const VERSION: &str = "1";

/// What Callisto knew about a release when it made the tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagHints {
    /// The package that made the tag, as its canonical `ecosystem/name` reference.
    pub package: String,
    /// The git tree id each directory of the package held in the tagged commit, keyed by directory (`.` for the root).
    pub trees: BTreeMap<String, String>,
}

impl TagHints {
    /// The annotation text: `first_line`, a blank line and the trailers.
    #[must_use]
    pub fn annotation(&self, first_line: &str) -> String {
        let mut text = format!(
            "{first_line}\n\n{HINTS_KEY}: {VERSION}\n{PACKAGE_KEY}: {}\n",
            self.package
        );
        for (directory, tree) in &self.trees {
            text.push_str(&format!("{TREE_KEY}: {directory} {tree}\n"));
        }
        text
    }

    /// The hints in `trailers`, the `Key: value` lines of an annotation. `None` unless the hint version is the one this
    /// release writes, there is exactly one package, and every tree line is a directory and a tree id with no
    /// directory repeated. Trailers of other tools are ignored.
    #[must_use]
    pub fn parse(trailers: &str) -> Option<TagHints> {
        let mut version = None;
        let mut package = None;
        let mut trees = BTreeMap::new();
        for (key, value) in trailers.lines().filter_map(|line| line.split_once(": ")) {
            match key {
                HINTS_KEY => {
                    if version.replace(value).is_some() {
                        return None;
                    }
                }
                PACKAGE_KEY => {
                    if value.is_empty() || package.replace(value).is_some() {
                        return None;
                    }
                }
                TREE_KEY => {
                    let (directory, tree) = value.rsplit_once(' ')?;
                    if directory.is_empty()
                        || !is_tree_id(tree)
                        || trees.insert(directory.to_owned(), tree.to_owned()).is_some()
                    {
                        return None;
                    }
                }
                _ => {}
            }
        }
        (version == Some(VERSION)).then_some(())?;
        Some(TagHints {
            package: package?.to_owned(),
            trees,
        })
    }
}

fn is_tree_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64)
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(digit: char) -> String {
        digit.to_string().repeat(40)
    }

    fn hints() -> TagHints {
        TagHints {
            package: "cargo/foo".to_owned(),
            trees: BTreeMap::from([
                ("crates/foo".to_owned(), tree('a')),
                ("crates/foo extras".to_owned(), tree('b')),
            ]),
        }
    }

    #[test]
    fn what_is_written_is_read_back() {
        let text = hints().annotation("Release foo@1.0.0");
        assert!(text.starts_with("Release foo@1.0.0\n\nCallisto-Hints: 1\n"));
        assert_eq!(TagHints::parse(&text), Some(hints()));
    }

    #[test]
    fn a_package_with_no_directories_recorded_is_still_a_hint() {
        let only_package = TagHints {
            package: "npm/@scope/foo".to_owned(),
            trees: BTreeMap::new(),
        };
        assert_eq!(
            TagHints::parse(&only_package.annotation("Release foo@1.0.0")),
            Some(only_package)
        );
    }

    #[test]
    fn an_annotation_without_hints_has_none() {
        assert_eq!(TagHints::parse("Release foo@1.0.0\n"), None);
        assert_eq!(TagHints::parse(""), None);
    }

    #[test]
    fn a_hint_of_another_version_or_without_a_version_is_ignored() {
        let text = hints().annotation("Release foo@1.0.0");
        assert_eq!(
            TagHints::parse(&text.replace("Callisto-Hints: 1", "Callisto-Hints: 2")),
            None
        );
        assert_eq!(TagHints::parse(&text.replace("Callisto-Hints: 1\n", "")), None);
        assert_eq!(
            TagHints::parse(&text.replace("Callisto-Hints: 1", "Callisto-Hints: ")),
            None
        );
    }

    #[test]
    fn a_garbled_trailer_makes_the_whole_hint_unusable() {
        let text = hints().annotation("Release foo@1.0.0");
        let without_package = text.replace("Callisto-Package: cargo/foo\n", "");
        let twice = format!("{text}Callisto-Package: cargo/bar\n");
        let short_tree = text.replace(&tree('a'), "abc123");
        let uppercase_tree = text.replace(&tree('a'), &"A".repeat(40));
        let no_tree_id = text.replace(&format!("crates/foo {}", tree('a')), "crates/foo");
        let repeated = format!("{text}Callisto-Tree: crates/foo {}\n", tree('c'));
        for garbled in [without_package, twice, short_tree, uppercase_tree, no_tree_id, repeated] {
            assert_eq!(TagHints::parse(&garbled), None, "{garbled}");
        }
    }

    #[test]
    fn trailers_of_other_tools_are_ignored() {
        let text = format!(
            "{}Signed-off-by: A Person <a@example.com>\n",
            hints().annotation("Release foo@1.0.0")
        );
        assert_eq!(TagHints::parse(&text), Some(hints()));
    }
}
