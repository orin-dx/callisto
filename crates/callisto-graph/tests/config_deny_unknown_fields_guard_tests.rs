//! A table added to `callisto.toml` later must reject unknown keys: every deserialized struct under `src/config` has to
//! carry `deny_unknown_fields`, so a typo is an error rather than silently ignored.

use std::{fs, path::Path};

/// Deserialized in code only, never read from the file.
const NOT_A_FILE_TABLE: &[&str] = &["RawGroupTable"];

/// `(file, struct name)` of every struct deriving `Deserialize` without `deny_unknown_fields`.
fn unguarded_structs(source: &str, file: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut attributes = String::new();
    for line in source.lines() {
        let line = line.trim();
        if line.starts_with("#[") {
            attributes.push_str(line);
            continue;
        }
        let declaration = line.strip_prefix("pub(crate) ").or_else(|| line.strip_prefix("pub "));
        let declaration = declaration.unwrap_or(line);
        if let Some(rest) = declaration.strip_prefix("struct ") {
            let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if attributes.contains("Deserialize") && !attributes.contains("deny_unknown_fields") {
                found.push((file.to_owned(), name));
            }
        }
        if !line.starts_with("//") {
            attributes.clear();
        }
    }
    found
}

#[test]
fn every_deserialized_config_struct_denies_unknown_fields() {
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/config");
    let mut files: Vec<_> = fs::read_dir(&config)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect();
    files.sort();
    assert!(files.len() >= 4, "expected the config sources in {config:?}");

    let mut deserialized = 0;
    let mut unguarded = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path).unwrap();
        // Unit tests build structs in code; only the production half defines tables.
        let production = source.split("#[cfg(test)]").next().unwrap();
        deserialized += production.matches("Deserialize)]").count();
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        unguarded.extend(
            unguarded_structs(production, &file)
                .into_iter()
                .filter(|(_, name)| !NOT_A_FILE_TABLE.contains(&name.as_str())),
        );
    }
    assert!(
        deserialized >= 10,
        "the scan found only {deserialized} derives; it has stopped seeing the structs"
    );
    assert!(
        unguarded.is_empty(),
        "these config structs lack #[serde(deny_unknown_fields)]: {unguarded:?}"
    );
}
