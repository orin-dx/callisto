use callisto_graph::locate::LocateError;
use callisto_graph::{ConfigError, GraphError};
use callisto_model::CommandError;
use miette::Diagnostic;

/// Formats a [`CliError`] as a JSON value with a consistent error envelope:
///
/// ```json
/// {
///   "schemaVersion": 1,
///   "command": "add",
///   "error": {
///     "code": "E211",
///     "message": "human-readable error text",
///     "help": "optional guidance string or null",
///     "path": "optional filesystem path or null"
///   }
/// }
/// ```
///
/// `command` is the subcommand that was parsed before the error occurred
/// (see [`crate::cli::Command::name`]), not derived from the error itself --
/// an error can occur before enough context exists to reconstruct it any
/// other way (e.g. an argument-parsing failure).
///
/// `"code"`, `"message"`, `"help"`, and `"path"` are always present;
/// `"help"`/`"path"` are `null` when the diagnostic provides none. This
/// guarantees a stable shape regardless of which [`CliError`] variant is
/// serialized.
pub fn format_error_json(command: &str, err: &CliError) -> serde_json::Value {
    let code = err.code().map(|c| c.to_string()).unwrap_or_else(|| "E000".to_string());
    let help = err.help().map(|h| h.to_string());
    let path = err.path().map(|p| p.display().to_string());
    serde_json::json!({
        "schemaVersion": callisto_model::SCHEMA_VERSION,
        "command": command,
        "error": {
            "code": code,
            "message": err.to_string(),
            "help": help,
            "path": path,
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
        code(E211),
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
    #[diagnostic(transparent)]
    ReleaseIntent(#[from] callisto_model::ReleaseIntentError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Model(#[from] callisto_model::ModelError),

    #[error("interactive prompt failed: {0}")]
    #[diagnostic(
        code(E269),
        help("pass flags explicitly to skip interactive prompts, or run in a terminal")
    )]
    Interactive(#[from] dialoguer::Error),

    #[error(transparent)]
    #[diagnostic(
        code(E212),
        help("Check that .changeset/pre.json is valid JSON and was not partially written.")
    )]
    PreJson(#[from] callisto_format::PreJsonError),

    #[error("I/O error{}", match &path {
        Some(p) => format!(" accessing `{}`", p.display()),
        None => String::new(),
    })]
    #[diagnostic(
        code(E213),
        help("check that the path exists and that you have permission to access it")
    )]
    Io {
        #[source]
        source: std::io::Error,
        path: Option<std::path::PathBuf>,
    },

    #[error("refusing to prompt interactively: stdin is not a terminal; missing: --package")]
    #[diagnostic(
        code(E214),
        help("specify package names explicitly via `callisto add --package <name>:<severity>` in CI environments")
    )]
    NotATty,

    #[error("release intent `{}` declares schema version {found}, but this build reads version {expected}", path.display())]
    #[diagnostic(
        code(E215),
        help("re-run `callisto release plan` to derive a fresh intent: an intent is bound to one build's operation graph and is never reused across versions")
    )]
    ReleaseIntentSchemaUnsupported {
        path: std::path::PathBuf,
        found: String,
        expected: u8,
    },

    #[error("release artifact-manifest needs an output file; remove --dry-run because it records build output")]
    #[diagnostic(code(E216), help("re-run `callisto release artifact-manifest` without --dry-run"))]
    ReleaseArtifactManifestDryRun,

    #[error("release plan needs an output file; remove --dry-run because planning is already read-only")]
    #[diagnostic(code(E217), help("re-run `callisto release plan --out <file>` without --dry-run"))]
    ReleasePlanDryRun,

    #[error("release execute cannot run with --dry-run")]
    #[diagnostic(code(E218), help("remove --dry-run; release execute has no read-only mode"))]
    ReleaseExecuteDryRun,

    #[error("release intent declares no binary artifact slots; no artifact manifest can be created")]
    #[diagnostic(code(E219), help("plan the release with --orchestration-revision and --artifact-repository so the intent declares artifact slots"))]
    ReleaseNoArtifactSlots,

    #[error("artifact manifests require a Git commit release source")]
    #[diagnostic(code(E220), help("plan the release from a Git commit source"))]
    ReleaseManifestSourceNotGit,

    #[error("artifact `{asset}` must be a regular file directly in `{dir}`")]
    #[diagnostic(
        code(E221),
        help("place the built asset as a plain file (not a symlink or directory) directly in the artifact directory")
    )]
    ReleaseArtifactNotRegularFile { asset: String, dir: String },

    #[error("artifact `{asset}` resolves outside `{dir}`")]
    #[diagnostic(
        code(E222),
        help("remove symlinks so every asset resolves inside the artifact directory")
    )]
    ReleaseArtifactEscapesDirectory { asset: String, dir: String },

    #[error("cannot create artifact manifest: {detail}")]
    #[diagnostic(
        code(E223),
        help("check that the artifact directory holds exactly the assets the intent declares")
    )]
    ReleaseManifestInvalid { detail: String },

    #[error("invalid merged release commit `{raw}`: {detail}")]
    #[diagnostic(code(E224), help("pass the full commit SHA of the merged release commit"))]
    ReleaseCommitInvalid { raw: String, detail: String },

    #[error("release decision {decision} must be inside selected source root {root}")]
    #[diagnostic(
        code(E225),
        help("point --decision at a file inside the source root selected with --source-root")
    )]
    ReleaseDecisionOutsideSource { decision: String, root: String },

    #[error("invalid release decision path {path}: {detail}")]
    #[diagnostic(code(E226), help("pass a repository-relative decision path without `..` components"))]
    ReleaseDecisionPathInvalid { path: String, detail: String },

    #[error("invalid release package `{raw}`: {detail}; use an exact ecosystem-qualified identity such as cargo/callisto-cli")]
    #[diagnostic(
        code(E227),
        help("name each package as <ecosystem>/<name>, for example cargo/callisto-cli")
    )]
    ReleasePackageInvalid { raw: String, detail: String },

    #[error("invalid orchestration revision `{revision}`: {detail}")]
    #[diagnostic(code(E228), help("pass the full commit SHA of the orchestration workflow revision"))]
    ReleaseOrchestrationRevisionInvalid { revision: String, detail: String },

    #[error("invalid artifact repository `{repository}`: {detail}")]
    #[diagnostic(code(E229), help("pass the forge repository as <owner>/<repo>"))]
    ReleaseArtifactRepositoryInvalid { repository: String, detail: String },

    #[error("[release].forge-repository is `{configured}`, but the release targets `{requested}`")]
    #[diagnostic(
        code(E230),
        help("plan with an --artifact-repository matching [release].forge-repository in callisto.toml")
    )]
    ReleaseForgeRepositoryMismatch { configured: String, requested: String },

    #[error("product release planning requires --orchestration-revision and --artifact-repository")]
    #[diagnostic(code(E231), help("pass both --orchestration-revision and --artifact-repository"))]
    ReleaseOrchestrationFlagsRequired,

    #[error("release intent declares no binary artifact slots; omit --artifact-manifest and --artifact-dir")]
    #[diagnostic(
        code(E232),
        help("drop --artifact-manifest and --artifact-dir for an intent without artifact slots")
    )]
    ReleaseUnexpectedArtifactInputs,

    #[error("release intent declares binary artifact slots; provide both --artifact-manifest and --artifact-dir")]
    #[diagnostic(code(E233), help("pass both --artifact-manifest and --artifact-dir"))]
    ReleaseMissingArtifactInputs,

    #[error("release run envelope is not valid for this intent: {detail}")]
    #[diagnostic(
        code(E234),
        help("re-check the orchestration revision and artifact manifest against the intent")
    )]
    ReleaseEnvelopeInvalid { detail: String },

    #[error("cannot issue terminal release receipt: {detail}")]
    #[diagnostic(
        code(E235),
        help("re-run `callisto release execute`; it adopts effects that already landed")
    )]
    ReleaseReceiptIssue { detail: String },

    #[error("invalid release intent {path}: {detail}")]
    #[diagnostic(code(E236), help("re-run `callisto release plan` to derive a fresh intent"))]
    ReleaseIntentInvalid { path: String, detail: String },

    #[error("invalid artifact manifest {path}: {detail}")]
    #[diagnostic(code(E237), help("re-run `callisto release artifact-manifest` to regenerate it"))]
    ArtifactManifestFileInvalid { path: String, detail: String },

    #[error("invalid JSON in {path}: {detail}")]
    #[diagnostic(code(E238), help("check the file is complete, well-formed JSON"))]
    ReleaseJsonInvalid { path: String, detail: String },

    #[error("this workspace cannot release locally: {reason}")]
    #[diagnostic(
        code(E239),
        help("release it from CI with `callisto release plan`, `callisto release artifact-manifest`, then `callisto release execute`; `callisto release --dry-run` still previews it locally")
    )]
    ReleaseRequiresCiRoute { reason: String },

    #[error("stdin is not a terminal, so init needs flags instead of prompts; missing: {}", .missing.join(", "))]
    #[diagnostic(
        code(E240),
        help("re-run `callisto init --yes` with the listed flags, or run it in a terminal to be asked")
    )]
    InitRequiresYes { missing: Vec<&'static str> },

    #[error("init --yes is missing required flag(s): {}", .missing.join(", "))]
    #[diagnostic(code(E241), help("supply the listed flags; see `callisto init --help`"))]
    InitMissingFlags { missing: Vec<&'static str> },

    #[error("--workflow and --no-workflow are mutually exclusive")]
    #[diagnostic(code(E242), help("pass only one of --workflow or --no-workflow"))]
    InitWorkflowFlagsConflict,

    #[error("workspace is already in pre-release mode")]
    #[diagnostic(
        code(E243),
        help("run `callisto pre exit` first, or delete .changeset/pre.json manually to reset")
    )]
    PreAlreadyActive,

    #[error("workspace is not in pre-release mode")]
    #[diagnostic(code(E244), help("callisto pre enter <tag>"))]
    PreNotActive,

    #[error("invalid package spec `{spec}`; expected `package-name:severity`")]
    #[diagnostic(code(E270), help("pass a colon-separated pair, for example `cargo/foo:patch`"))]
    AddInvalidPackageSpec { spec: String },

    #[error("Invalid severity `{value}`. Must be none, patch, minor, or major.")]
    #[diagnostic(code(E271), help("pass one of: none, patch, minor, major"))]
    InvalidSeverity { value: String },

    #[error("no packages found in workspace")]
    #[diagnostic(
        code(E272),
        help("check that callisto.toml and package manifests are discoverable from the current directory")
    )]
    AddNoPackagesInWorkspace,

    #[error("no packages selected for changeset")]
    #[diagnostic(
        code(E273),
        help("select at least one package, or pass `--package` flags instead of running interactively")
    )]
    AddNoPackagesSelected,

    #[error("--summary is required when specifying packages via --package in non-interactive mode")]
    #[diagnostic(code(E274), help("pass `--summary \"description\"` alongside `--package`"))]
    AddSummaryRequired,

    #[error("--summary cannot be empty")]
    #[diagnostic(code(E275), help("provide a non-empty description of the change"))]
    AddSummaryEmpty,

    #[error("--emit-decision writes a file; remove --dry-run or drop --emit-decision")]
    #[diagnostic(code(E276), help("remove --dry-run, or drop --emit-decision"))]
    VersionEmitDecisionDryRun,

    #[error("--strict: workspace graph has error diagnostics:\n{}", .messages.join("\n"))]
    #[diagnostic(
        code(E277),
        help("resolve each listed diagnostic, or drop --strict if it is expected")
    )]
    StrictDiagnosticsPresent { messages: Vec<String> },

    #[error("pre-release tag cannot be empty")]
    #[diagnostic(code(E279), help("pass a non-empty tag, for example `callisto pre enter beta`"))]
    PreTagEmpty,

    #[error("workspace is not in pre-release mode (already exited)")]
    #[diagnostic(code(E280), help("run `callisto version` to finalize the release"))]
    PreAlreadyExited,

    #[error("--{flag} is not valid JSON: {detail}")]
    #[diagnostic(code(E281), help("check the JSON is well-formed and matches the expected schema"))]
    ReleasePrArgJsonInvalid { flag: &'static str, detail: String },

    #[error("release-pr commit-plan cannot write --out with --dry-run; omit --out")]
    #[diagnostic(code(E282), help("re-run without --dry-run, or drop --out"))]
    ReleasePrCommitPlanDryRun,

    #[error("no such subcommand `{name}`")]
    #[diagnostic(code(E283), help("run `callisto --help` to list commands"))]
    HelpUnknownCommand { name: String },
}

impl From<std::io::Error> for CliError {
    fn from(source: std::io::Error) -> Self {
        CliError::Io { source, path: None }
    }
}

impl CliError {
    /// The filesystem path this error concerns, when it carries a
    /// structured one. `Io` is the only variant with a real `PathBuf`
    /// field; every other path-carrying variant already embeds its path as
    /// a `String` inside its own message (covered by `error.message`), so
    /// this stays `None` for those rather than re-parsing a message string.
    fn path(&self) -> Option<&std::path::Path> {
        match self {
            CliError::Io { path, .. } => path.as_deref(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every JSON error object must have `"code"`, `"message"`, `"help"`, and
    /// `"path"` keys, plus a top-level `"command"`, regardless of which
    /// variant is serialized.
    fn assert_envelope_shape(command: &str, json: &serde_json::Value) {
        assert_eq!(
            json["schemaVersion"],
            serde_json::json!(callisto_model::SCHEMA_VERSION),
            "schemaVersion must be present and match SCHEMA_VERSION"
        );
        assert_eq!(
            json["command"],
            serde_json::json!(command),
            "command must be present and match"
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
        // help/path are present as keys always; each value is either a string or null
        assert!(
            error["help"].is_string() || error["help"].is_null(),
            "error.help must be a string or null, got: {:?}",
            error["help"]
        );
        assert!(
            error["path"].is_string() || error["path"].is_null(),
            "error.path must be a string or null, got: {:?}",
            error["path"]
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
    fn format_error_json_typed_variant_has_stable_envelope() {
        let err = CliError::AddSummaryEmpty;
        let json = format_error_json("add", &err);
        assert_envelope_shape("add", &json);
        assert_eq!(json["error"]["code"], "E275");
        assert_eq!(json["error"]["message"], "--summary cannot be empty");
        assert!(json["error"]["help"].is_string());
        assert!(json["error"]["path"].is_null(), "AddSummaryEmpty carries no path");
    }

    #[test]
    fn format_error_json_not_a_tty_includes_code_and_help() {
        let err = CliError::NotATty;
        let json = format_error_json("add", &err);
        assert_envelope_shape("add", &json);
        assert_eq!(json["error"]["code"], "E214");
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

    /// `add --summary x` with no `--package` and a non-terminal stdin must
    /// name the actually-missing flag (`--package`), not claim generically
    /// that no flags at all were given -- `--summary` was one.
    #[test]
    fn not_a_tty_message_names_the_missing_package_flag() {
        let message = CliError::NotATty.to_string();
        assert!(
            message.contains("missing: --package"),
            "message must name --package as the missing flag; got: {message}"
        );
    }

    #[test]
    fn format_error_json_io_error_includes_code_help_and_path() {
        let err = CliError::Io {
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "file missing"),
            path: Some(std::path::PathBuf::from("/some/path")),
        };
        let json = format_error_json("status", &err);
        assert_envelope_shape("status", &json);
        assert_eq!(json["error"]["code"], "E213");
        assert!(
            json["error"]["message"].as_str().unwrap().contains("/some/path"),
            "I/O error message should include the path"
        );
        assert_eq!(
            json["error"]["path"], "/some/path",
            "error.path must carry the structured path, not just the message text"
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
        let errors: &[CliError] = &[CliError::AddSummaryEmpty, CliError::NotATty];
        let jsons: Vec<serde_json::Value> = errors.iter().map(|e| format_error_json("add", e)).collect();
        for json in &jsons {
            assert_envelope_shape("add", json);
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
            let json = format_error_json("release", error);
            let code = json["error"]["code"].as_str().unwrap();
            assert!(
                code.starts_with('E') && code[1..].chars().all(|c| c.is_ascii_digit()),
                "{code}"
            );
            assert!(json["error"]["help"].is_string(), "{code} lacks help");
        }
        let codes: std::collections::BTreeSet<_> = errors
            .iter()
            .map(|e| format_error_json("release", e)["error"]["code"].to_string())
            .collect();
        assert_eq!(codes.len(), errors.len(), "release error codes must be unique");
    }

    /// Every variant that replaced a `CliError::Other(String)` call site carries its own
    /// code and help, and the codes are pairwise distinct.
    #[test]
    fn retired_other_call_sites_now_carry_code_and_help() {
        let errors = [
            CliError::Interactive(dialoguer::Error::IO(std::io::Error::other("x"))),
            CliError::AddInvalidPackageSpec { spec: "x".to_owned() },
            CliError::InvalidSeverity { value: "x".to_owned() },
            CliError::AddNoPackagesInWorkspace,
            CliError::AddNoPackagesSelected,
            CliError::AddSummaryRequired,
            CliError::AddSummaryEmpty,
            CliError::VersionEmitDecisionDryRun,
            CliError::StrictDiagnosticsPresent {
                messages: vec!["x".to_owned()],
            },
            CliError::PreTagEmpty,
            CliError::PreAlreadyExited,
            CliError::HelpUnknownCommand { name: "x".to_owned() },
            CliError::ReleasePrArgJsonInvalid {
                flag: "snapshot",
                detail: "x".to_owned(),
            },
            CliError::ReleasePrCommitPlanDryRun,
        ];
        for error in &errors {
            let json = format_error_json("add", error);
            let code = json["error"]["code"].as_str().unwrap();
            assert!(
                code.starts_with('E') && code[1..].chars().all(|c| c.is_ascii_digit()),
                "{code}"
            );
            assert!(json["error"]["help"].is_string(), "{code} lacks help");
        }
        let codes: std::collections::BTreeSet<_> = errors
            .iter()
            .map(|e| format_error_json("add", e)["error"]["code"].to_string())
            .collect();
        assert_eq!(codes.len(), errors.len(), "retired-Other error codes must be unique");
    }
}
