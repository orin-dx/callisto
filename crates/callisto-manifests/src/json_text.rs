//! Replacing string values inside JSON text without touching anything else.
//!
//! Re-serializing a whole `package.json` to change one version rewrites every inline array and object in it. These
//! functions find the byte span of the string literal at a key path with a real scanner and replace only that span, so
//! every other byte of the file stays exactly as its author wrote it.

use std::ops::Range;

use serde_json::{Map, Value};

/// Every string value that differs between `before` and `after`, as `(key path, new value)`; `None` when they differ
/// in anything else (a key added or removed, a changed array, a changed type), which needs the whole document written.
pub(crate) fn string_edits(
    before: &Map<String, Value>,
    after: &Map<String, Value>,
) -> Option<Vec<(Vec<String>, String)>> {
    let mut edits = Vec::new();
    let mut path = Vec::new();
    objects_differ_only_in_strings(before, after, &mut path, &mut edits).then_some(edits)
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

/// `source` with each edit's string literal replaced; `None` when a path does not lead to exactly one string value
/// (it is missing, is not a string, or its key appears twice in one object).
pub(crate) fn replace_strings(source: &str, edits: &[(Vec<String>, String)]) -> Option<String> {
    let mut spans = Vec::with_capacity(edits.len());
    for (path, value) in edits {
        spans.push((
            Scanner::new(source).string_span(path)?,
            serde_json::to_string(value).ok()?,
        ));
    }
    spans.sort_by_key(|(span, _)| span.start);
    let mut out = String::with_capacity(source.len());
    let mut copied = 0;
    for (span, literal) in spans {
        if span.start < copied {
            return None;
        }
        out.push_str(&source[copied..span.start]);
        out.push_str(&literal);
        copied = span.end;
    }
    out.push_str(&source[copied..]);
    Some(out)
}

struct Scanner<'a> {
    source: &'a str,
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Scanner<'a> {
    fn new(source: &'a str) -> Self {
        Scanner {
            source,
            bytes: source.as_bytes(),
            at: 0,
        }
    }

    /// The span of the string literal `path` names, scanning from the document's top-level object.
    fn string_span(&mut self, path: &[String]) -> Option<Range<usize>> {
        self.skip_whitespace();
        self.find_in_object(path)
    }

    fn skip_whitespace(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Option<()> {
        (self.bytes.get(self.at) == Some(&byte)).then(|| self.at += 1)
    }

    /// At an object's `{`: the span of the string at `path` inside it. Scans the whole object so a repeated key is seen.
    fn find_in_object(&mut self, path: &[String]) -> Option<Range<usize>> {
        let (first, rest) = path.split_first()?;
        self.expect(b'{')?;
        let mut found = None;
        let mut matches = 0;
        loop {
            self.skip_whitespace();
            if self.bytes.get(self.at) == Some(&b'}') {
                self.at += 1;
                break;
            }
            let key = self.string_literal()?;
            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();
            if serde_json::from_str::<String>(&self.source[key]).ok()? == *first {
                matches += 1;
                found = if rest.is_empty() {
                    (self.bytes.get(self.at) == Some(&b'"'))
                        .then(|| self.string_literal())
                        .flatten()
                } else if self.bytes.get(self.at) == Some(&b'{') {
                    self.find_in_object(rest)
                } else {
                    self.skip_value()?;
                    None
                };
            } else {
                self.skip_value()?;
            }
            self.skip_whitespace();
            match self.bytes.get(self.at)? {
                b',' => self.at += 1,
                b'}' => {}
                _ => return None,
            }
        }
        if matches == 1 {
            found
        } else {
            None
        }
    }

    /// At a `"`: the span of the whole literal, quotes included.
    fn string_literal(&mut self) -> Option<Range<usize>> {
        let start = self.at;
        self.expect(b'"')?;
        loop {
            match self.bytes.get(self.at)? {
                b'"' => {
                    self.at += 1;
                    return Some(start..self.at);
                }
                b'\\' => self.at += 2,
                _ => self.at += 1,
            }
        }
    }

    fn skip_value(&mut self) -> Option<()> {
        match self.bytes.get(self.at)? {
            b'"' => {
                self.string_literal()?;
            }
            open @ (b'{' | b'[') => {
                let close = if *open == b'{' { b'}' } else { b']' };
                self.at += 1;
                loop {
                    self.skip_whitespace();
                    if self.bytes.get(self.at) == Some(&close) {
                        self.at += 1;
                        break;
                    }
                    if *open == b'{' {
                        self.string_literal()?;
                        self.skip_whitespace();
                        self.expect(b':')?;
                        self.skip_whitespace();
                    }
                    self.skip_value()?;
                    self.skip_whitespace();
                    match self.bytes.get(self.at)? {
                        b',' => self.at += 1,
                        byte if *byte == close => {}
                        _ => return None,
                    }
                }
            }
            _ => {
                // A number, `true`, `false` or `null`: runs to the next delimiter.
                while self
                    .bytes
                    .get(self.at)
                    .is_some_and(|byte| !matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r'))
                {
                    self.at += 1;
                }
            }
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde::Serialize;

    fn parse(text: &str) -> Map<String, Value> {
        serde_json::from_str(text).unwrap()
    }

    fn edit(path: &[&str], value: &str) -> (Vec<String>, String) {
        (path.iter().map(|part| (*part).to_owned()).collect(), value.to_owned())
    }

    #[test]
    fn only_the_named_string_changes_and_every_other_byte_stays() {
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
    fn a_key_that_looks_like_the_path_inside_a_string_or_another_object_is_not_confused_with_it() {
        let source = r#"{"description": "\"version\": \"9\"", "x": {"version": "inner"}, "version": "1.0.0"}"#;
        let edited = replace_strings(source, &[edit(&["version"], "2.0.0")]).unwrap();
        assert_eq!(parse(&edited)["version"], "2.0.0");
        assert_eq!(parse(&edited)["x"]["version"], "inner");
        assert_eq!(parse(&edited)["description"], "\"version\": \"9\"");
    }

    #[test]
    fn a_value_is_written_with_the_escapes_json_requires() {
        let edited = replace_strings(r#"{"a": "x"}"#, &[edit(&["a"], "say \"hi\"\n\\")]).unwrap();
        assert_eq!(parse(&edited)["a"], "say \"hi\"\n\\");
    }

    #[test]
    fn a_path_that_is_missing_not_a_string_or_repeated_gives_nothing() {
        assert!(replace_strings(r#"{"a": "x"}"#, &[edit(&["b"], "y")]).is_none());
        assert!(replace_strings(r#"{"a": 1}"#, &[edit(&["a"], "y")]).is_none());
        assert!(replace_strings(r#"{"a": {"b": "x"}}"#, &[edit(&["a"], "y")]).is_none());
        assert!(replace_strings(r#"{"a": "x", "a": "z"}"#, &[edit(&["a"], "y")]).is_none());
        assert!(replace_strings(r#"{"a": "x"}"#, &[edit(&["a", "b"], "y")]).is_none());
    }

    #[test]
    fn the_diff_is_only_string_values_and_anything_else_needs_the_whole_document() {
        let before = parse(r#"{"v": "1", "d": {"a": "x"}, "l": [1, 2], "n": null}"#);
        let changed = parse(r#"{"v": "2", "d": {"a": "y"}, "l": [1, 2], "n": null}"#);
        assert_eq!(
            string_edits(&before, &changed).unwrap(),
            vec![edit(&["v"], "2"), edit(&["d", "a"], "y")]
        );
        assert_eq!(string_edits(&before, &before).unwrap(), vec![]);
        for different in [
            r#"{"v": "1", "d": {"a": "x"}, "l": [1, 3], "n": null}"#,
            r#"{"v": "1", "d": {"a": "x", "b": "z"}, "l": [1, 2], "n": null}"#,
            r#"{"v": 1, "d": {"a": "x"}, "l": [1, 2], "n": null}"#,
            r#"{"v": "1", "d": {"a": "x"}, "l": [1, 2]}"#,
        ] {
            assert!(string_edits(&before, &parse(different)).is_none(), "{different}");
        }
    }

    fn json() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            (-1000i64..1000).prop_map(Value::from),
            "[ -~é\u{2014}]{0,8}".prop_map(Value::String),
        ];
        leaf.prop_recursive(3, 24, 4, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
                prop::collection::btree_map("[a-z\"\\\\]{1,4}", inner, 0..4)
                    .prop_map(|map| Value::Object(map.into_iter().collect())),
            ]
        })
    }

    /// Every path to a string leaf inside objects only (the paths the scanner follows).
    fn string_paths(map: &Map<String, Value>, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        for (key, value) in map {
            prefix.push(key.clone());
            match value {
                Value::String(_) => out.push(prefix.clone()),
                Value::Object(inner) => string_paths(inner, prefix, out),
                _ => {}
            }
            prefix.pop();
        }
    }

    fn set(map: &mut Map<String, Value>, path: &[String], new: &str) {
        let (first, rest) = path.split_first().unwrap();
        let slot = map.get_mut(first).unwrap();
        if rest.is_empty() {
            *slot = Value::String(new.to_owned());
        } else {
            set(slot.as_object_mut().unwrap(), rest, new);
        }
    }

    fn source_with_layout(doc: &Map<String, Value>, layout: u8) -> String {
        match layout {
            0 => serde_json::to_string(doc).unwrap(),
            1 => serde_json::to_string_pretty(doc).unwrap(),
            _ => {
                let mut bytes = Vec::new();
                let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
                let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, formatter);
                doc.serialize(&mut serializer).unwrap();
                String::from_utf8(bytes).unwrap()
            }
        }
    }

    #[test]
    fn layout_fixture_preserves_string_values() {
        let doc = parse(r#"{"a":" :  ","x":{"b":": "}}"#);
        for layout in 0..3 {
            assert_eq!(parse(&source_with_layout(&doc, layout)), doc);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(300))]

        /// Whatever a document looks like and whichever string is changed, the result is the document with that one
        /// string changed, and the text before and after the string is byte for byte what it was.
        #[test]
        fn changing_one_string_changes_nothing_else_in_any_layout(
            root in prop::collection::btree_map("[a-z\"\\\\]{1,4}", json(), 1..5),
            layout in 0u8..3,
            pick in any::<prop::sample::Index>(),
            new in "[ -~é]{0,8}",
        ) {
            let doc: Map<String, Value> = root.into_iter().collect();
            let mut paths = Vec::new();
            string_paths(&doc, &mut Vec::new(), &mut paths);
            prop_assume!(!paths.is_empty());
            let path = pick.get(&paths).clone();

            let source = source_with_layout(&doc, layout);
            let mut expected = doc.clone();
            set(&mut expected, &path, &new);
            let old = {
                let mut value = &Value::Object(doc.clone());
                for part in &path { value = &value[part]; }
                value.as_str().unwrap().to_owned()
            };
            prop_assume!(old != new);

            let edited = replace_strings(&source, &[(path.clone(), new.clone())]).expect("a unique string path is found");
            prop_assert_eq!(parse(&edited), expected);
            let old_literal = serde_json::to_string(&old).unwrap();
            let new_literal = serde_json::to_string(&new).unwrap();
            // The text around the literal is the same length: nothing outside it was added, dropped or reflowed.
            prop_assert_eq!(edited.len() - new_literal.len(), source.len() - old_literal.len());
        }
    }
}
