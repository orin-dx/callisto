use callisto_model::{Version, VersionGrammar};
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

    let mut out = serde_json::to_string_pretty(&map).unwrap();
    out.push('\n');
    out
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
        source: callisto_model::VersionParseError,
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
                callisto_model::Version::parse("1.0.0a1", callisto_model::VersionGrammar::Pep440).unwrap()
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
}
