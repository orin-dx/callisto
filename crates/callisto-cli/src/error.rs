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
    #[diagnostic(transparent)]
    ReleasePrDecision(#[from] callisto_model::ReleasePrDecisionError),

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

    #[error("release intent `{}` declares schema version {found}, but this build reads version {expected}", path.display())]
    #[diagnostic(
        code(callisto::release_intent_schema_unsupported),
        help("re-run `callisto release plan` to derive a fresh intent: an intent is bound to one build's operation graph and is never reused across versions")
    )]
    ReleaseIntentSchemaUnsupported {
        path: std::path::PathBuf,
        found: String,
        expected: u8,
    },

    #[error("release artifact-manifest needs an output file; remove --dry-run because it records build output")]
    #[diagnostic(
        code(callisto::release_artifact_manifest_dry_run),
        help("re-run `callisto release artifact-manifest` without --dry-run")
    )]
    ReleaseArtifactManifestDryRun,

    #[error("release plan needs an output file; remove --dry-run because planning is already read-only")]
    #[diagnostic(
        code(callisto::release_plan_dry_run),
        help("re-run `callisto release plan --out <file>` without --dry-run")
    )]
    ReleasePlanDryRun,

    #[error("release execute cannot run with --dry-run")]
    #[diagnostic(
        code(callisto::release_execute_dry_run),
        help("remove --dry-run; release execute has no read-only mode")
    )]
    ReleaseExecuteDryRun,

    #[error("release intent declares no binary artifact slots; no artifact manifest can be created")]
    #[diagnostic(code(callisto::release_no_artifact_slots), help("plan the release with --orchestration-revision and --artifact-repository so the intent declares artifact slots"))]
    ReleaseNoArtifactSlots,

    #[error("artifact manifests require a Git commit release source")]
    #[diagnostic(
        code(callisto::release_manifest_source_not_git),
        help("plan the release from a Git commit source")
    )]
    ReleaseManifestSourceNotGit,

    #[error("artifact `{asset}` must be a regular file directly in `{dir}`")]
    #[diagnostic(
        code(callisto::release_artifact_not_regular_file),
        help("place the built asset as a plain file (not a symlink or directory) directly in the artifact directory")
    )]
    ReleaseArtifactNotRegularFile { asset: String, dir: String },

    #[error("artifact `{asset}` resolves outside `{dir}`")]
    #[diagnostic(
        code(callisto::release_artifact_escapes_directory),
        help("remove symlinks so every asset resolves inside the artifact directory")
    )]
    ReleaseArtifactEscapesDirectory { asset: String, dir: String },

    #[error("cannot create artifact manifest: {detail}")]
    #[diagnostic(
        code(callisto::release_manifest_creation_failed),
        help("check that the artifact directory holds exactly the assets the intent declares")
    )]
    ReleaseManifestInvalid { detail: String },

    #[error("invalid merged release commit `{raw}`: {detail}")]
    #[diagnostic(
        code(callisto::release_commit_invalid),
        help("pass the full commit SHA of the merged release commit")
    )]
    ReleaseCommitInvalid { raw: String, detail: String },

    #[error("release decision {decision} must be inside selected source root {root}")]
    #[diagnostic(
        code(callisto::release_decision_outside_source),
        help("point --decision at a file inside the source root selected with --source-root")
    )]
    ReleaseDecisionOutsideSource { decision: String, root: String },

    #[error("invalid release decision path {path}: {detail}")]
    #[diagnostic(
        code(callisto::release_decision_path_invalid),
        help("pass a repository-relative decision path without `..` components")
    )]
    ReleaseDecisionPathInvalid { path: String, detail: String },

    #[error("invalid release package `{raw}`: {detail}; use an exact ecosystem-qualified identity such as cargo/callisto-cli")]
    #[diagnostic(
        code(callisto::release_package_invalid),
        help("name each package as <ecosystem>/<name>, for example cargo/callisto-cli")
    )]
    ReleasePackageInvalid { raw: String, detail: String },

    #[error("invalid orchestration revision `{revision}`: {detail}")]
    #[diagnostic(
        code(callisto::release_orchestration_revision_invalid),
        help("pass the full commit SHA of the orchestration workflow revision")
    )]
    ReleaseOrchestrationRevisionInvalid { revision: String, detail: String },

    #[error("invalid artifact repository `{repository}`: {detail}")]
    #[diagnostic(
        code(callisto::release_artifact_repository_invalid),
        help("pass the forge repository as <owner>/<repo>")
    )]
    ReleaseArtifactRepositoryInvalid { repository: String, detail: String },

    #[error("[release].forge-repository is `{configured}`, but the release targets `{requested}`")]
    #[diagnostic(
        code(callisto::release_forge_repository_mismatch),
        help("plan with an --artifact-repository matching [release].forge-repository in callisto.toml")
    )]
    ReleaseForgeRepositoryMismatch { configured: String, requested: String },

    #[error("product release planning requires --orchestration-revision and --artifact-repository")]
    #[diagnostic(
        code(callisto::release_orchestration_flags_required),
        help("pass both --orchestration-revision and --artifact-repository")
    )]
    ReleaseOrchestrationFlagsRequired,

    #[error("release intent declares no binary artifact slots; omit --artifact-manifest and --artifact-dir")]
    #[diagnostic(
        code(callisto::release_unexpected_artifact_inputs),
        help("drop --artifact-manifest and --artifact-dir for an intent without artifact slots")
    )]
    ReleaseUnexpectedArtifactInputs,

    #[error("release intent declares binary artifact slots; provide both --artifact-manifest and --artifact-dir")]
    #[diagnostic(
        code(callisto::release_missing_artifact_inputs),
        help("pass both --artifact-manifest and --artifact-dir")
    )]
    ReleaseMissingArtifactInputs,

    #[error("release run envelope is not valid for this intent: {detail}")]
    #[diagnostic(
        code(callisto::release_envelope_invalid),
        help("re-check the orchestration revision and artifact manifest against the intent")
    )]
    ReleaseEnvelopeInvalid { detail: String },

    #[error("cannot issue terminal release receipt: {detail}")]
    #[diagnostic(
        code(callisto::release_receipt_issue_failed),
        help("re-run `callisto release execute`; it adopts effects that already landed")
    )]
    ReleaseReceiptIssue { detail: String },

    #[error("invalid release intent {path}: {detail}")]
    #[diagnostic(
        code(callisto::release_intent_invalid),
        help("re-run `callisto release plan` to derive a fresh intent")
    )]
    ReleaseIntentInvalid { path: String, detail: String },

    #[error("invalid artifact manifest {path}: {detail}")]
    #[diagnostic(
        code(callisto::release_artifact_manifest_invalid),
        help("re-run `callisto release artifact-manifest` to regenerate it")
    )]
    ArtifactManifestFileInvalid { path: String, detail: String },

    #[error("invalid JSON in {path}: {detail}")]
    #[diagnostic(
        code(callisto::release_json_invalid),
        help("check the file is complete, well-formed JSON")
    )]
    ReleaseJsonInvalid { path: String, detail: String },

    #[error("this workspace cannot release locally: {reason}")]
    #[diagnostic(
        code(callisto::release_requires_ci_route),
        help("release it from CI with `callisto release plan`, `callisto release artifact-manifest`, then `callisto release execute`; `callisto release --dry-run` still previews it locally")
    )]
    ReleaseRequiresCiRoute { reason: String },

    #[error("stdin is not a terminal, so init needs flags instead of prompts; missing: {}", .missing.join(", "))]
    #[diagnostic(
        code(callisto::init_requires_yes),
        help("re-run `callisto init --yes` with the listed flags, or run it in a terminal to be asked")
    )]
    InitRequiresYes { missing: Vec<&'static str> },

    #[error("init --yes is missing required flag(s): {}", .missing.join(", "))]
    #[diagnostic(
        code(callisto::init_missing_flags),
        help("supply the listed flags; see `callisto init --help`")
    )]
    InitMissingFlags { missing: Vec<&'static str> },

    #[error("--workflow and --no-workflow are mutually exclusive")]
    #[diagnostic(
        code(callisto::init_workflow_flags_conflict),
        help("pass only one of --workflow or --no-workflow")
    )]
    InitWorkflowFlagsConflict,

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
    fn from_io_error_wraps_source_with_no_path() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: CliError = io_err.into();
        match err {
            CliError::Io { source, path } => {
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
                assert!(path.is_none(), "blanket From<io::Error> must not synthesize a path");
            }
            other => panic!("expected CliError::Io, got: {other:?}"),
        }
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

    #[test]
    fn typed_release_errors_carry_code_and_help() {
        let errors = [
            CliError::ReleaseArtifactManifestDryRun,
            CliError::ReleasePlanDryRun,
            CliError::ReleaseExecuteDryRun,
            CliError::ReleaseNoArtifactSlots,
            CliError::ReleaseManifestSourceNotGit,
            CliError::ReleaseArtifactNotRegularFile {
                asset: "x".to_owned(),
                dir: "x".to_owned(),
            },
            CliError::ReleaseArtifactEscapesDirectory {
                asset: "x".to_owned(),
                dir: "x".to_owned(),
            },
            CliError::ReleaseManifestInvalid { detail: "x".to_owned() },
            CliError::ReleaseCommitInvalid {
                raw: "x".to_owned(),
                detail: "x".to_owned(),
            },
            CliError::ReleaseDecisionOutsideSource {
                decision: "x".to_owned(),
                root: "x".to_owned(),
            },
            CliError::ReleaseDecisionPathInvalid {
                path: "x".to_owned(),
                detail: "x".to_owned(),
            },
            CliError::ReleasePackageInvalid {
                raw: "x".to_owned(),
                detail: "x".to_owned(),
            },
            CliError::ReleaseOrchestrationRevisionInvalid {
                revision: "x".to_owned(),
                detail: "x".to_owned(),
            },
            CliError::ReleaseArtifactRepositoryInvalid {
                repository: "x".to_owned(),
                detail: "x".to_owned(),
            },
            CliError::ReleaseForgeRepositoryMismatch {
                configured: "x".to_owned(),
                requested: "x".to_owned(),
            },
            CliError::ReleaseOrchestrationFlagsRequired,
            CliError::ReleaseUnexpectedArtifactInputs,
            CliError::ReleaseMissingArtifactInputs,
            CliError::ReleaseEnvelopeInvalid { detail: "x".to_owned() },
            CliError::ReleaseReceiptIssue { detail: "x".to_owned() },
            CliError::ReleaseIntentInvalid {
                path: "x".to_owned(),
                detail: "x".to_owned(),
            },
            CliError::ArtifactManifestFileInvalid {
                path: "x".to_owned(),
                detail: "x".to_owned(),
            },
            CliError::ReleaseJsonInvalid {
                path: "x".to_owned(),
                detail: "x".to_owned(),
            },
        ];
        for error in &errors {
            let json = format_error_json(error);
            let code = json["error"]["code"].as_str().unwrap();
            assert!(
                code.starts_with("callisto::release_") && code != "callisto::error",
                "{code}"
            );
            assert!(json["error"]["help"].is_string(), "{code} lacks help");
        }
        let codes: std::collections::BTreeSet<_> = errors
            .iter()
            .map(|e| format_error_json(e)["error"]["code"].to_string())
            .collect();
        assert_eq!(codes.len(), errors.len(), "release error codes must be unique");
    }
}
