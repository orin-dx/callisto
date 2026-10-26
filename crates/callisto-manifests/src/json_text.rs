//! Replacing string values inside JSON text without touching anything else.
//!
//! Re-serializing a whole `package.json` to change one version rewrites every inline array and object in it, so a
//! version bump becomes a noisy diff. A change that only replaces string values is made on the original text instead.

use jsonc_parser::cst::{CstInputValue, CstRootNode};
use jsonc_parser::ParseOptions;
use serde_json::{Map, Value};

/// Every string value that differs between `before` and `after`, as `(key path, new value)`; `None` when they differ
/// in anything else (a key added or removed, a changed array, a changed type), which needs the whole document written.
pub(crate) fn string_edits(
    before: &Map<String, Value>,
    after: &Map<String, Value>,
) -> Option<Vec<(Vec<String>, String)>> {
    let mut edits = Vec::new();
    objects_differ_only_in_strings(before, after, &mut Vec::new(), &mut edits).then_some(edits)
}

fn objects_differ_only_in_strings(
    before: &Map<String, Value>,
    after: &Map<String, Value>,
    path: &mut Vec<String>,
    edits: &mut Vec<(Vec<String>, String)>,
) -> bool {
    if before.len() != after.len() {
        return false;
    }
    for (key, old) in before {
        let Some(new) = after.get(key) else {
            return false;
        };
        path.push(key.clone());
        let same_shape = match (old, new) {
            (Value::String(old), Value::String(new)) => {
                if old != new {
                    edits.push((path.clone(), new.clone()));
                }
                true
            }
            (Value::Object(old), Value::Object(new)) => objects_differ_only_in_strings(old, new, path, edits),
            (old, new) => old == new,
        };
        path.pop();
        if !same_shape {
            return false;
        }
    }
    true
}

/// `source` with each edit's string value replaced; `None` when a path does not lead to an existing string.
pub(crate) fn replace_strings(source: &str, edits: &[(Vec<String>, String)]) -> Option<String> {
    let root = CstRootNode::parse(source, &ParseOptions::default()).ok()?;
    for (path, value) in edits {
        let (last, parents) = path.split_last()?;
        let mut object = root.object_value()?;
        for key in parents {
            object = object.object_value(key)?;
        }
        let property = object.get(last)?;
        property.value()?.as_string_lit()?;
        property.set_value(CstInputValue::String(value.clone()));
    }
    Some(root.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Map<String, Value> {
        serde_json::from_str(text).unwrap()
    }

    fn edit(path: &[&str], value: &str) -> (Vec<String>, String) {
        (path.iter().map(|part| (*part).to_owned()).collect(), value.to_owned())
    }

    #[test]
    fn only_the_named_strings_change_and_every_other_byte_stays() {
        let source = "{\n\t\"name\": \"lib\",  \n\t\"version\": \"1.0.0\",\n\t\"files\": [\"dist\", \"src\"],\n\t\"exports\": { \".\": \"./dist/index.js\" },\n\t\"dependencies\": {\"a\": \"^1.0.0\", \"b\": \"^2.0.0\"}\n}\n";
        let edited = replace_strings(
            source,
            &[edit(&["version"], "1.1.0"), edit(&["dependencies", "b"], "^3.0.0")],
        )
        .unwrap();
        assert_eq!(
            edited,
            source
                .replace("\"1.0.0\",\n", "\"1.1.0\",\n")
                .replace("^2.0.0", "^3.0.0")
        );
    }

    #[test]
    fn a_new_value_is_written_with_the_escapes_json_requires() {
        let edited = replace_strings(r#"{"a": "x"}"#, &[edit(&["a"], "say \"hi\"\n\\")]).unwrap();
        assert_eq!(parse(&edited)["a"], "say \"hi\"\n\\");
    }

    #[test]
    fn a_path_that_is_missing_or_not_a_string_gives_nothing() {
        assert!(replace_strings(r#"{"a": "x"}"#, &[edit(&["b"], "y")]).is_none());
        assert!(replace_strings(r#"{"a": 1}"#, &[edit(&["a"], "y")]).is_none());
        assert!(replace_strings(r#"{"a": {"b": "x"}}"#, &[edit(&["a"], "y")]).is_none());
    }

    #[test]
    fn the_diff_is_only_string_values_and_anything_else_needs_the_whole_document() {
        let before = parse(r#"{"v": "1", "d": {"a": "x"}, "l": [1, 2]}"#);
        let changed = parse(r#"{"v": "2", "d": {"a": "y"}, "l": [1, 2]}"#);
        assert_eq!(
            string_edits(&before, &changed).unwrap(),
            vec![edit(&["v"], "2"), edit(&["d", "a"], "y")]
        );
        for different in [
            r#"{"v": "1", "d": {"a": "x"}, "l": [1, 3]}"#,
            r#"{"v": "1", "d": {"a": "x", "b": "z"}, "l": [1, 2]}"#,
            r#"{"v": 1, "d": {"a": "x"}, "l": [1, 2]}"#,
        ] {
            assert!(string_edits(&before, &parse(different)).is_none(), "{different}");
        }
    }
}
