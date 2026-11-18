use crate::{Ecosystem, Version, VersionGrammar};
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Which Changesets `pre.json` shape a file uses; rewrites keep the shape that was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreStateShape {
    /// `mode`, `tag`, `initialVersions` and the `changesets` id list.
    V2,
    /// `mode` and `tag` only; consumed notes live in `.changeset/pre/`.
    V3,
}

/// The shape-specific fields of a `pre.json`; v3 has no baseline and no id list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged, rename_all = "camelCase")]
pub enum PreLayout {
    #[serde(rename_all = "camelCase")]
    V2 {
        initial_versions: IndexMap<String, Version>,
        changesets: Vec<String>,
    },
    V3 {},
}

/// The wire shape of a `pre.json` for schema reports: v3 omits both trailing fields.
#[derive(JsonSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct PreStateWire {
    mode: PreMode,
    tag: String,
    initial_versions: Option<std::collections::BTreeMap<String, Version>>,
    changesets: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(with = "PreStateWire")]
pub struct PreState {
    pub mode: PreMode,
    pub tag: String,
    #[serde(flatten)]
    pub layout: PreLayout,
    /// Keys this version of callisto does not know, kept in file order across rewrites.
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_json::Value>,
}

impl PreState {
    pub fn entering(tag: impl Into<String>, initial_versions: impl IntoIterator<Item = (String, Version)>) -> Self {
        let mut map = IndexMap::new();
        for (pkg, ver) in initial_versions {
            map.entry(pkg).or_insert(ver);
        }
        Self::v2(PreMode::Pre, tag, map, Vec::new())
    }

    pub fn v2(
        mode: PreMode,
        tag: impl Into<String>,
        initial_versions: IndexMap<String, Version>,
        changesets: Vec<String>,
    ) -> Self {
        PreState {
            mode,
            tag: tag.into(),
            layout: PreLayout::V2 {
                initial_versions,
                changesets,
            },
            extra: IndexMap::new(),
        }
    }

    pub fn v3(mode: PreMode, tag: impl Into<String>) -> Self {
        PreState {
            mode,
            tag: tag.into(),
            layout: PreLayout::V3 {},
            extra: IndexMap::new(),
        }
    }

    pub fn shape(&self) -> PreStateShape {
        match self.layout {
            PreLayout::V2 { .. } => PreStateShape::V2,
            PreLayout::V3 {} => PreStateShape::V3,
        }
    }

    /// The `enter`-time baseline; `None` for v3, which records none.
    pub fn initial_versions(&self) -> Option<&IndexMap<String, Version>> {
        match &self.layout {
            PreLayout::V2 { initial_versions, .. } => Some(initial_versions),
            PreLayout::V3 {} => None,
        }
    }

    pub fn initial_versions_mut(&mut self) -> Option<&mut IndexMap<String, Version>> {
        match &mut self.layout {
            PreLayout::V2 { initial_versions, .. } => Some(initial_versions),
            PreLayout::V3 {} => None,
        }
    }

    /// Ids consumed in this cycle; `None` for v3, where retained notes live in `.changeset/pre/`.
    pub fn changesets(&self) -> Option<&Vec<String>> {
        match &self.layout {
            PreLayout::V2 { changesets, .. } => Some(changesets),
            PreLayout::V3 {} => None,
        }
    }

    pub fn changesets_mut(&mut self) -> Option<&mut Vec<String>> {
        match &mut self.layout {
            PreLayout::V2 { changesets, .. } => Some(changesets),
            PreLayout::V3 {} => None,
        }
    }

    pub fn exiting(mut self) -> Self {
        self.mode = PreMode::Exit;
        self
    }

    /// Applies each baseline owner's native grammar; unresolved entries retain their parsed value.
    pub fn resolve_initial_versions(
        &mut self,
        mut ecosystem_for: impl FnMut(&str) -> Option<Ecosystem>,
    ) -> Result<(), PreJsonError> {
        let Some(initial_versions) = self.initial_versions() else {
            return Ok(());
        };
        let raw_versions = initial_versions
            .iter()
            .map(|(key, version)| (key.clone(), version.raw().to_string()))
            .collect();
        self.resolve_initial_versions_from_raw(&raw_versions, &mut ecosystem_for)
    }

    /// Validates the original wire spelling after each baseline owner is known.
    pub fn resolve_initial_versions_from_raw(
        &mut self,
        raw_versions: &IndexMap<String, String>,
        mut ecosystem_for: impl FnMut(&str) -> Option<Ecosystem>,
    ) -> Result<(), PreJsonError> {
        let Some(initial_versions) = self.initial_versions_mut() else {
            return Ok(());
        };
        for (package, version) in initial_versions {
            if let Some(ecosystem) = ecosystem_for(package) {
                let raw = raw_versions.get(package).map(String::as_str).unwrap_or(version.raw());
                *version = Version::parse_for_ecosystem(raw, ecosystem).map_err(|source| {
                    PreJsonError::InvalidInitialVersion {
                        package: package.clone(),
                        raw: raw.to_string(),
                        source,
                    }
                })?;
            }
        }
        Ok(())
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

    let layout = match (obj.get("initialVersions"), obj.get("changesets")) {
        (None, None) => PreLayout::V3 {},
        (None, Some(_)) => {
            return Err(PreJsonError::MissingField {
                field: "initialVersions",
            })
        }
        (Some(_), None) => return Err(PreJsonError::MissingField { field: "changesets" }),
        (Some(init_val), Some(cs_val)) => {
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

            let cs_arr = cs_val
                .as_array()
                .ok_or(PreJsonError::WrongFieldType { field: "changesets" })?;
            let mut changesets = Vec::new();
            for (index, c_val) in cs_arr.iter().enumerate() {
                let c_str = c_val.as_str().ok_or(PreJsonError::InvalidChangesetId { index })?;
                changesets.push(c_str.to_string());
            }
            PreLayout::V2 {
                initial_versions,
                changesets,
            }
        }
    };

    let extra = obj
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "mode" | "tag" | "initialVersions" | "changesets"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    Ok(PreState {
        mode,
        tag,
        layout,
        extra,
    })
}

pub fn write_pre_json(state: &PreState) -> String {
    render_pre_json(state, "  ", &[])
}

/// Renders `state`, then reapplies `existing`'s on-disk formatting (BOM, CRLF vs LF, indent width) instead of the
/// default 2-space bare-LF shape, so a CRLF or custom-indent `pre.json` (e.g. `core.autocrlf=true`, or hand-edited)
/// is not reformatted on a `pre exit` or mid-cycle `version` rewrite.
pub fn write_pre_json_preserving(state: &PreState, existing: &str) -> String {
    let fp = PreJsonFingerprint::detect(existing);
    let order: Vec<String> =
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(existing.trim_start_matches('\u{FEFF}'))
            .map(|keys| keys.keys().cloned().collect())
            .unwrap_or_default();
    let rendered = render_pre_json(state, &fp.indent_str(), &order);
    fp.apply(&rendered)
}

/// Keys present in `order` come first, in that order, so a rewrite keeps the file's own key order.
fn render_pre_json(state: &PreState, indent_str: &str, order: &[String]) -> String {
    let mut map = IndexMap::new();
    map.insert(
        "mode".to_string(),
        serde_json::to_value(state.mode).expect("a pre mode is a JSON string"),
    );
    map.insert(
        "tag".to_string(),
        serde_json::to_value(&state.tag).expect("a tag is a JSON string"),
    );

    match &state.layout {
        PreLayout::V2 {
            initial_versions,
            changesets,
        } => {
            let mut init_map = IndexMap::new();
            for (pkg, ver) in initial_versions {
                init_map.insert(
                    pkg.clone(),
                    serde_json::to_value(ver).expect("a version is a JSON string"),
                );
            }
            map.insert(
                "initialVersions".to_string(),
                serde_json::to_value(init_map).expect("a map of strings is a JSON object"),
            );
            map.insert(
                "changesets".to_string(),
                serde_json::to_value(changesets).expect("a list of strings is a JSON array"),
            );
        }
        PreLayout::V3 {} => {}
    }
    for (key, value) in &state.extra {
        map.entry(key.clone()).or_insert_with(|| value.clone());
    }
    let mut ordered: IndexMap<String, serde_json::Value> =
        order.iter().filter_map(|key| map.shift_remove_entry(key)).collect();
    ordered.extend(map);
    let map = ordered;

    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent_str.as_bytes());
    let mut buf = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(&map, &mut serializer).expect("serializing JSON values into memory cannot fail");
    let mut out = String::from_utf8(buf).expect("serde_json writes UTF-8");
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
        // PEP 440 strings such as "0.3.2a1" in initialVersions must parse, or pre-mode is unusable for Python.
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
        assert_eq!(state.initial_versions().unwrap()["my-python-pkg"].raw(), "0.3.2a1");
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
        assert_eq!(reparsed.initial_versions().unwrap()["pkg"].raw(), "1.0.0a1");
    }

    #[test]
    fn baseline_grammars_follow_their_owners_without_changing_the_wire_shape() {
        let json = r#"{"mode":"pre","tag":"beta","initialVersions":{"python":"1.2.3","cargo":"1.2.3","npm":"1.2.3","removed":"1.2.3"},"changesets":[]}"#;
        let mut state = parse_pre_json(json).unwrap();
        state
            .resolve_initial_versions(|key| match key {
                "python" => Some(Ecosystem::Pypi),
                "cargo" => Some(Ecosystem::Cargo),
                "npm" => Some(Ecosystem::Npm),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            state.initial_versions().unwrap()["python"].grammar(),
            VersionGrammar::Pep440
        );
        for key in ["cargo", "npm", "removed"] {
            assert_eq!(state.initial_versions().unwrap()[key].grammar(), VersionGrammar::SemVer);
        }
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&write_pre_json(&state)).unwrap(),
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );
    }

    #[test]
    fn baseline_rejects_a_version_valid_only_in_a_different_ecosystem() {
        for (raw, ecosystem) in [("1.2.3-alpha.beta", Ecosystem::Pypi), ("1!2.3rc1", Ecosystem::Cargo)] {
            let mut state = PreState::entering(
                "beta",
                [(
                    "package".to_string(),
                    Version::parse(raw, VersionGrammar::SemVer)
                        .or_else(|_| Version::parse(raw, VersionGrammar::Pep440))
                        .unwrap(),
                )],
            );
            let error = state.resolve_initial_versions(|_| Some(ecosystem)).unwrap_err();
            assert!(
                matches!(error, PreJsonError::InvalidInitialVersion { package, raw: found, source }
                if package == "package" && found == raw && source.grammar == ecosystem.version_grammar())
            );
        }
    }

    #[test]
    fn baseline_validation_uses_original_wire_spelling_after_pep440_fallback() {
        for raw in ["v1.2.3", " 1.2.3 "] {
            let json = serde_json::json!({
                "mode": "pre",
                "tag": "beta",
                "initialVersions": {"cargo/pkg": raw},
                "changesets": []
            })
            .to_string();
            let mut state = parse_pre_json(&json).unwrap();
            let raw_versions = IndexMap::from([("cargo/pkg".to_string(), raw.to_string())]);
            let error = state
                .resolve_initial_versions_from_raw(&raw_versions, |_| Some(Ecosystem::Cargo))
                .unwrap_err();
            assert!(matches!(error, PreJsonError::InvalidInitialVersion { raw: found, .. } if found == raw));
        }
    }

    /// `PreState::exiting()` is the only public way out of pre-release mode
    /// The mode flips to `Exit` and survives a real write -> parse round trip, matching
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

    const V3_JSON: &str = "{\n  \"mode\": \"pre\",\n  \"tag\": \"next\"\n}\n";

    #[test]
    fn v3_pre_json_parses_without_baseline_or_id_list() {
        let state = parse_pre_json(V3_JSON).unwrap();
        assert_eq!(state.shape(), PreStateShape::V3);
        assert_eq!(state.tag, "next");
        assert!(state.initial_versions().is_none());
        assert!(state.changesets().is_none());
    }

    #[test]
    fn v3_pre_json_round_trips_as_mode_and_tag_only() {
        let state = parse_pre_json(V3_JSON).unwrap();
        assert_eq!(write_pre_json(&state), V3_JSON);
        assert_eq!(write_pre_json_preserving(&state, V3_JSON), V3_JSON);
    }

    #[test]
    fn v2_pre_json_reports_v2_shape() {
        let state = parse_pre_json(r#"{"mode":"pre","tag":"a","initialVersions":{},"changesets":[]}"#).unwrap();
        assert_eq!(state.shape(), PreStateShape::V2);
        assert_eq!(PreState::entering("a", []).shape(), PreStateShape::V2);
    }

    #[test]
    fn unknown_keys_survive_a_rewrite_in_original_order() {
        for json in [
            r#"{"mode":"pre","tag":"a","zeta":1,"alpha":{"k":[true]}}"#,
            r#"{"mode":"pre","tag":"a","initialVersions":{},"zeta":1,"changesets":[],"alpha":{"k":[true]}}"#,
        ] {
            let state = parse_pre_json(json).unwrap();
            let written = write_pre_json(&state);
            let value: serde_json::Value = serde_json::from_str(&written).unwrap();
            assert_eq!(value["zeta"], 1);
            assert_eq!(value["alpha"]["k"][0], true);
            assert!(written.find("\"zeta\"").unwrap() < written.find("\"alpha\"").unwrap());
            assert_eq!(parse_pre_json(&written).unwrap(), state);
        }
    }

    #[test]
    fn a_rewrite_keeps_the_files_own_key_order() {
        let json = "{\n  \"zeta\": 1,\n  \"tag\": \"a\",\n  \"mode\": \"pre\"\n}\n";
        let state = parse_pre_json(json).unwrap();
        assert_eq!(write_pre_json_preserving(&state, json), json);
    }

    #[test]
    fn half_v3_files_are_rejected() {
        let only_baseline = r#"{"mode":"pre","tag":"a","initialVersions":{}}"#;
        assert!(matches!(
            parse_pre_json(only_baseline).unwrap_err(),
            PreJsonError::MissingField { field: "changesets" }
        ));
        let only_ids = r#"{"mode":"pre","tag":"a","changesets":[]}"#;
        assert!(matches!(
            parse_pre_json(only_ids).unwrap_err(),
            PreJsonError::MissingField {
                field: "initialVersions"
            }
        ));
    }

    #[test]
    fn exiting_a_v3_state_writes_v3() {
        let exited = parse_pre_json(V3_JSON).unwrap().exiting();
        assert_eq!(exited.shape(), PreStateShape::V3);
        let written = write_pre_json(&exited);
        assert_eq!(written, "{\n  \"mode\": \"exit\",\n  \"tag\": \"next\"\n}\n");
    }

    #[test]
    fn v3_rewrite_keeps_bom_crlf_and_tab_indent() {
        let existing = "\u{FEFF}{\r\n\t\"mode\": \"pre\",\r\n\t\"tag\": \"next\"\r\n}\r\n";
        let state = parse_pre_json(existing).unwrap();
        assert_eq!(write_pre_json_preserving(&state, existing), existing);
    }
}
