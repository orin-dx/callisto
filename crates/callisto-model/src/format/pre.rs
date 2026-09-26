use crate::{Version, VersionGrammar};
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreState {
    pub mode: PreMode,
    pub tag: String,
    #[schemars(with = "std::collections::BTreeMap<String, Version>")]
    pub initial_versions: IndexMap<String, Version>,
    pub changesets: Vec<String>,
}

impl PreState {
    pub fn entering(tag: impl Into<String>, initial_versions: impl IntoIterator<Item = (String, Version)>) -> Self {
        let mut map = IndexMap::new();
        for (pkg, ver) in initial_versions {
            map.entry(pkg).or_insert(ver);
        }
        PreState {
            mode: PreMode::Pre,
            tag: tag.into(),
            initial_versions: map,
            changesets: Vec::new(),
        }
    }

    pub fn exiting(mut self) -> Self {
        self.mode = PreMode::Exit;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PreMode {
    Pre,
    Exit,
}

pub fn parse_pre_json(input: &str) -> Result<PreState, PreJsonError> {
    let clean_input = input.strip_prefix('\u{FEFF}').unwrap_or(input);
    let val: serde_json::Value =
        serde_json::from_str(clean_input).map_err(|e| PreJsonError::Malformed { message: e.to_string() })?;

    let obj = val.as_object().ok_or_else(|| PreJsonError::Malformed {
        message: "expected a JSON object".to_string(),
    })?;

    let mode_val = obj.get("mode").ok_or(PreJsonError::MissingField { field: "mode" })?;
    let mode_str = mode_val
        .as_str()
        .ok_or(PreJsonError::WrongFieldType { field: "mode" })?;
    let mode = match mode_str {
        "pre" => PreMode::Pre,
        "exit" => PreMode::Exit,
        _ => {
            return Err(PreJsonError::InvalidMode {
                found: mode_str.to_string(),
            })
        }
    };

    let tag_val = obj.get("tag").ok_or(PreJsonError::MissingField { field: "tag" })?;
    let tag = tag_val
        .as_str()
        .ok_or(PreJsonError::WrongFieldType { field: "tag" })?
        .to_string();

    let init_val = obj.get("initialVersions").ok_or(PreJsonError::MissingField {
        field: "initialVersions",
    })?;
    let init_obj = init_val.as_object().ok_or(PreJsonError::WrongFieldType {
        field: "initialVersions",
    })?;

    let mut initial_versions = IndexMap::new();
    for (pkg, v_val) in init_obj {
        let v_str = v_val.as_str().ok_or(PreJsonError::WrongFieldType {
            field: "initialVersions",
        })?;
        let ver = Version::parse(v_str, VersionGrammar::SemVer)
            .or_else(|_| Version::parse(v_str, VersionGrammar::Pep440))
            .map_err(|source| PreJsonError::InvalidInitialVersion {
                package: pkg.clone(),
                raw: v_str.to_string(),
                source,
            })?;
        initial_versions.insert(pkg.clone(), ver);
    }

    let cs_val = obj
        .get("changesets")
        .ok_or(PreJsonError::MissingField { field: "changesets" })?;
    let cs_arr = cs_val
        .as_array()
        .ok_or(PreJsonError::WrongFieldType { field: "changesets" })?;

    let mut changesets = Vec::new();
    for (index, c_val) in cs_arr.iter().enumerate() {
        let c_str = c_val.as_str().ok_or(PreJsonError::InvalidChangesetId { index })?;
        changesets.push(c_str.to_string());
    }

    Ok(PreState {
        mode,
        tag,
        initial_versions,
        changesets,
    })
}

pub fn write_pre_json(state: &PreState) -> String {
    render_pre_json(state, "  ")
}

/// Renders `state`, then reapplies `existing`'s on-disk formatting (BOM, CRLF vs LF, indent width) instead of the
/// default 2-space bare-LF shape, so a CRLF or custom-indent `pre.json` (e.g. `core.autocrlf=true`, or hand-edited)
/// is not reformatted on a `pre exit` or mid-cycle `version` rewrite.
pub fn write_pre_json_preserving(state: &PreState, existing: &str) -> String {
    let fp = PreJsonFingerprint::detect(existing);
    let rendered = render_pre_json(state, &fp.indent_str());
    fp.apply(&rendered)
}

fn render_pre_json(state: &PreState, indent_str: &str) -> String {
    let mut map = IndexMap::new();
    map.insert("mode".to_string(), serde_json::to_value(state.mode).unwrap());
    map.insert("tag".to_string(), serde_json::to_value(&state.tag).unwrap());

    let mut init_map = IndexMap::new();
    for (pkg, ver) in &state.initial_versions {
        init_map.insert(pkg.clone(), serde_json::to_value(ver).unwrap());
    }
    map.insert("initialVersions".to_string(), serde_json::to_value(init_map).unwrap());
    map.insert(
        "changesets".to_string(),
        serde_json::to_value(&state.changesets).unwrap(),
    );

    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent_str.as_bytes());
    let mut buf = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(&map, &mut serializer).unwrap();
    let mut out = String::from_utf8(buf).unwrap();
    out.push('\n');
    out
}

/// Byte-level formatting facts (BOM, CRLF, indent) `pre.json` must preserve across a rewrite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreJsonFingerprint {
    has_bom: bool,
    crlf: bool,
    indent: PreJsonIndent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreJsonIndent {
    Spaces(u8),
    Tabs,
}

impl PreJsonFingerprint {
    fn detect(content: &str) -> Self {
        let has_bom = content.starts_with('\u{FEFF}');
        let clean = content.strip_prefix('\u{FEFF}').unwrap_or(content);
        let crlf = clean.contains("\r\n");

        // The first quote-leading line is a top-level key at indent depth 1; its leading whitespace is one unit.
        let mut indent = PreJsonIndent::Spaces(2);
        for line in clean.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('"') && line.len() > trimmed.len() {
                let leading = &line[..line.len() - trimmed.len()];
                indent = if leading.contains('\t') {
                    PreJsonIndent::Tabs
                } else {
                    PreJsonIndent::Spaces(leading.len() as u8)
                };
                break;
            }
        }

        PreJsonFingerprint { has_bom, crlf, indent }
    }

    fn indent_str(&self) -> String {
        match self.indent {
            PreJsonIndent::Tabs => "\t".to_string(),
            PreJsonIndent::Spaces(n) => " ".repeat(n as usize),
        }
    }

    /// Reapplies this fingerprint to `rendered`, a freshly-serialized body with bare-LF endings and no BOM.
    fn apply(&self, rendered: &str) -> String {
        let mut out = if self.crlf {
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

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PreJsonError {
    #[error("pre.json is not a valid JSON object: {message}")]
    Malformed { message: String },

    #[error("pre.json is missing required field {field:?}")]
    MissingField { field: &'static str },

    #[error("pre.json field {field:?} has the wrong type")]
    WrongFieldType { field: &'static str },

    #[error("pre.json has mode {found:?}, expected \"pre\" or \"exit\"")]
    InvalidMode { found: String },

    #[error("pre.json initialVersions[{package:?}] = {raw:?} is not a valid version: {source}")]
    InvalidInitialVersion {
        package: String,
        raw: String,
        #[source]
        source: crate::VersionParseError,
    },

    #[error("pre.json changesets[{index}] is not a string")]
    InvalidChangesetId { index: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_json_round_trips() {
        let json = r#"{
  "mode": "pre",
  "tag": "next",
  "initialVersions": {
    "foo": "1.0.0"
  },
  "changesets": [
    "cool-dragons-fly"
  ]
}
"#;
        let state = parse_pre_json(json).unwrap();
        assert_eq!(state.tag, "next");
        let written = write_pre_json(&state);
        assert_eq!(written, json);
    }

    #[test]
    fn pre_json_accepts_pep440_initial_versions() {
        // pre.json files from Python/PyPI workspaces store PEP 440 version
        // strings (e.g. "0.3.2a1") in initialVersions. parse_pre_json must
        // accept them; rejecting them would make pre-mode unusable for Python
        // packages entirely.
        let json = r#"{
  "mode": "pre",
  "tag": "beta",
  "initialVersions": {
    "my-python-pkg": "0.3.2a1"
  },
  "changesets": []
}
"#;
        let state = parse_pre_json(json).unwrap();
        assert_eq!(state.initial_versions["my-python-pkg"].raw(), "0.3.2a1");
    }

    #[test]
    fn pre_json_round_trips_pep440_version() {
        // The PEP 440 initial version string must survive a write → parse cycle
        // unchanged so that pre.json files are idempotent.
        let state = PreState::entering(
            "beta",
            [("pkg".to_string(), {
                crate::Version::parse("1.0.0a1", crate::VersionGrammar::Pep440).unwrap()
            })],
        );
        let written = write_pre_json(&state);
        let reparsed = parse_pre_json(&written).unwrap();
        assert_eq!(reparsed.initial_versions["pkg"].raw(), "1.0.0a1");
    }

    /// `PreState::exiting()` is the only public way out of pre-release mode
    /// -- previously untested end to end. Proves the mode flips to `Exit`
    /// and that this survives a real write -> parse round trip, matching
    /// `pre_json_round_trips`'s style for the "pre" mode above.
    #[test]
    fn pre_state_exiting_flips_mode_and_round_trips_as_exit() {
        let state = PreState::entering(
            "next",
            [(
                "foo".to_string(),
                Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            )],
        );
        assert_eq!(state.mode, PreMode::Pre);

        let exited = state.exiting();
        assert_eq!(exited.mode, PreMode::Exit);

        let written = write_pre_json(&exited);
        assert!(
            written.contains("\"mode\": \"exit\""),
            "written pre.json must serialize mode as \"exit\", got:\n{written}"
        );

        let reparsed = parse_pre_json(&written).unwrap();
        assert_eq!(reparsed.mode, PreMode::Exit);
    }

    #[test]
    fn parse_pre_json_rejects_non_object_json() {
        let err = parse_pre_json("[1, 2, 3]").unwrap_err();
        assert!(
            matches!(err, PreJsonError::Malformed { ref message } if message == "expected a JSON object"),
            "expected Malformed{{\"expected a JSON object\"}}, got {err:?}"
        );
    }

    #[test]
    fn parse_pre_json_rejects_unrecognized_mode_string() {
        let json = r#"{"mode": "paused", "tag": "next", "initialVersions": {}, "changesets": []}"#;
        let err = parse_pre_json(json).unwrap_err();
        assert!(
            matches!(err, PreJsonError::InvalidMode { ref found } if found == "paused"),
            "expected InvalidMode{{\"paused\"}}, got {err:?}"
        );
    }

    #[test]
    fn parse_pre_json_rejects_initial_version_that_is_neither_semver_nor_pep440() {
        let json = r#"{"mode": "pre", "tag": "next", "initialVersions": {"pkg": "not-a-version"}, "changesets": []}"#;
        let err = parse_pre_json(json).unwrap_err();
        assert!(
            matches!(err, PreJsonError::InvalidInitialVersion { ref package, ref raw, .. } if package == "pkg" && raw == "not-a-version"),
            "expected InvalidInitialVersion{{package: \"pkg\", raw: \"not-a-version\"}}, got {err:?}"
        );
    }

    /// Spec: rewriting a CRLF `pre.json` must keep CRLF, the same fingerprinting contract as the manifest editors.
    #[test]
    fn write_pre_json_preserving_keeps_crlf() {
        let existing = "{\r\n  \"mode\": \"pre\",\r\n  \"tag\": \"beta\",\r\n  \"initialVersions\": {},\r\n  \"changesets\": []\r\n}\r\n";
        let state = parse_pre_json(existing).unwrap();

        let rewritten = write_pre_json_preserving(&state, existing);

        assert!(
            rewritten.contains("\r\n"),
            "expected CRLF preserved, got: {rewritten:?}"
        );
        assert!(
            !rewritten.replace("\r\n", "").contains('\n'),
            "no bare LF should remain: {rewritten:?}"
        );
    }

    /// Spec: a tab-indented `pre.json` must be rewritten with tabs, not the default 2-space indent.
    #[test]
    fn write_pre_json_preserving_keeps_tab_indent() {
        let existing =
            "{\n\t\"mode\": \"pre\",\n\t\"tag\": \"beta\",\n\t\"initialVersions\": {},\n\t\"changesets\": []\n}\n";
        let state = parse_pre_json(existing).unwrap();

        let rewritten = write_pre_json_preserving(&state, existing);

        assert!(
            rewritten.lines().any(|l| l.starts_with('\t')),
            "expected tab indent preserved, got: {rewritten:?}"
        );
    }

    /// Spec: a BOM-prefixed `pre.json` must be rewritten with the BOM kept.
    #[test]
    fn write_pre_json_preserving_keeps_bom() {
        let existing =
            "\u{FEFF}{\n  \"mode\": \"pre\",\n  \"tag\": \"beta\",\n  \"initialVersions\": {},\n  \"changesets\": []\n}\n";
        let state = parse_pre_json(existing).unwrap();

        let rewritten = write_pre_json_preserving(&state, existing);

        assert!(
            rewritten.starts_with('\u{FEFF}'),
            "expected BOM preserved, got: {rewritten:?}"
        );
        // The BOM-stripped body must still parse back to the same state.
        let reparsed = parse_pre_json(&rewritten).unwrap();
        assert_eq!(reparsed, state);
    }
}
