use callisto_graph::locate::LocateError;
use callisto_graph::{ConfigError, GraphError};
use callisto_model::CommandError;
use miette::Diagnostic;

/// Formats a [`CliError`] as a JSON value with a consistent error envelope:
///
/// ```json
/// {
///   "schemaVersion": 1,
///   "error": {
///     "code": "callisto::some_code",
///     "message": "human-readable error text",
///     "help": "optional guidance string or null"
///   }
/// }
/// ```
///
/// `"code"`, `"message"`, and `"help"` are always present; `"help"` is `null`
/// when the diagnostic provides no help text.  This guarantees a stable shape
/// regardless of which [`CliError`] variant is serialized.
pub fn format_error_json(err: &CliError) -> serde_json::Value {
    let code = err
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "callisto::error".to_string());
    let help = err.help().map(|h| h.to_string());
    serde_json::json!({
        "schemaVersion": callisto_model::SCHEMA_VERSION,
        "error": {
            "code": code,
            "message": err.to_string(),
            "help": help,
        }
    })
}

#[derive(Debug, thiserror::Error, Diagnostic)]
#[non_exhaustive]
pub enum CliError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Graph(#[from] GraphError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Locate(#[from] LocateError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Command(#[from] CommandError),

    #[error(transparent)]
    #[diagnostic(
        code(callisto::registry_error),
        help("verify registry credentials/authentication and network connectivity, then retry")
    )]
    Registry(#[from] callisto_model::RegistryError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    ChangesetParse(#[from] callisto_format::ParseError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    ChangesetWrite(#[from] callisto_format::WriteError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Manifest(#[from] callisto_model::ManifestError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Vcs(#[from] callisto_vcs::VcsError),

    #[error(transparent)]
    #[diagnostic(code(callisto::pre_json_error))]
    PreJson(#[from] callisto_format::PreJsonError),

    #[error("I/O error{}", match &path {
        Some(p) => format!(" accessing `{}`", p.display()),
        None => String::new(),
    })]
    #[diagnostic(
        code(callisto::io_error),
        help("check that the path exists and that you have permission to access it")
    )]
    Io {
        #[source]
        source: std::io::Error,
        path: Option<std::path::PathBuf>,
    },

    #[error("refusing to prompt interactively: stdin is not a terminal and no non-interactive flags were given")]
    #[diagnostic(
        code(callisto::not_a_tty),
        help("specify package names explicitly via `callisto add --package <name>:<severity>` in CI environments")
    )]
    NotATty,

    #[error("{0}")]
    #[diagnostic(code(callisto::error))]
    Other(String),
}

impl From<std::io::Error> for CliError {
    fn from(source: std::io::Error) -> Self {
        CliError::Io { source, path: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every JSON error object must have `"code"`, `"message"`, and `"help"` keys
    /// regardless of which variant is serialized.
    fn assert_envelope_shape(json: &serde_json::Value) {
        assert_eq!(
            json["schemaVersion"],
            serde_json::json!(callisto_model::SCHEMA_VERSION),
            "schemaVersion must be present and match SCHEMA_VERSION"
        );
        let error = &json["error"];
        assert!(
            error.is_object(),
            "top-level 'error' key must be an object, got: {error:?}"
        );
        assert!(
            error["code"].is_string(),
            "error.code must always be a string, got: {:?}",
            error["code"]
        );
        assert!(
            error["message"].is_string(),
            "error.message must always be a string, got: {:?}",
            error["message"]
        );
        // help is present as a key always; its value is either a string or null
        assert!(
            error["help"].is_string() || error["help"].is_null(),
            "error.help must be a string or null, got: {:?}",
            error["help"]
        );
    }

    #[test]
    fn format_error_json_other_has_stable_envelope() {
        let err = CliError::Other("something went wrong".to_string());
        let json = format_error_json(&err);
        assert_envelope_shape(&json);
        assert_eq!(json["error"]["code"], "callisto::error");
        assert_eq!(json["error"]["message"], "something went wrong");
        // Other has no help text
        assert!(json["error"]["help"].is_null());
    }

    #[test]
    fn format_error_json_not_a_tty_includes_code_and_help() {
        let err = CliError::NotATty;
        let json = format_error_json(&err);
        assert_envelope_shape(&json);
        assert_eq!(json["error"]["code"], "callisto::not_a_tty");
        assert!(
            !json["error"]["message"].as_str().unwrap().is_empty(),
            "message must be non-empty"
        );
        let help = json["error"]["help"].as_str().expect("NotATty must have help text");
        assert!(
            help.contains("callisto add --package"),
            "help should reference the --package flag; got: {help}"
        );
    }

    #[test]
    fn format_error_json_io_error_includes_code_and_help() {
        let err = CliError::Io {
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "file missing"),
            path: Some(std::path::PathBuf::from("/some/path")),
        };
        let json = format_error_json(&err);
        assert_envelope_shape(&json);
        assert_eq!(json["error"]["code"], "callisto::io_error");
        assert!(
            json["error"]["message"].as_str().unwrap().contains("/some/path"),
            "I/O error message should include the path"
        );
        let help = json["error"]["help"].as_str().expect("Io must have help text");
        assert!(
            help.contains("path exists"),
            "help should reference path existence check; got: {help}"
        );
    }

    /// Structural consistency: two different error variants must produce the same
    /// top-level key set (code + message + help always present).
    #[test]
    fn format_error_json_structure_is_consistent_across_variants() {
        let errors: &[CliError] = &[CliError::Other("first".to_string()), CliError::NotATty];
        let jsons: Vec<serde_json::Value> = errors.iter().map(format_error_json).collect();
        for json in &jsons {
            assert_envelope_shape(json);
        }
        // Both must have the same top-level keys
        let keys_0: std::collections::BTreeSet<String> =
            jsons[0]["error"].as_object().unwrap().keys().cloned().collect();
        let keys_1: std::collections::BTreeSet<String> =
            jsons[1]["error"].as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys_0, keys_1, "All error variants must produce the same JSON key set");
    }
}
