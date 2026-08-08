use serde::{Deserialize, Serialize};

use callisto_graph::locate::LocateError;

// Real moon wire types, sourced from `moon_pdk_api` (moon v2.4.6-compatible,
// crate version 2.0.4). These are only usable/meaningful when compiling for
// the WASM plugin target, so they (and everything that consumes them) stay
// behind the `pdk` feature.
#[cfg(feature = "pdk")]
pub use moon_pdk_api::{
    DefineExtensionConfigOutput, ExecuteExtensionInput, InitializeExtensionInput,
    InitializeExtensionOutput, RegisterExtensionInput, RegisterExtensionOutput,
};

/// Validates a moon version string against the range of moon versions this
/// extension supports.
///
/// NOTE: moon's real `RegisterExtensionInput` (see `moon_pdk_api::extension`)
/// only carries the extension's configured `id: Id` — it does not carry
/// moon's own version. moon negotiates plugin/host version compatibility out
/// of band (via the plugin manifest and `.moon/workspace.yml` `pluginVersion`
/// constraints), not over the `register_extension` wire call. This helper is
/// therefore kept as a standalone, independently-tested utility rather than
/// being wired into `register_extension`.
pub fn check_moon_version(version_str: &str) -> Result<(), LocateError> {
    if let Ok(ver) = semver::Version::parse(version_str) {
        let req = semver::VersionReq::parse(">=2.0.0, <3.0.0").unwrap();
        if req.matches(&ver) {
            return Ok(());
        }
    }
    Err(LocateError::IncompatibleMoonVersion {
        found: version_str.to_string(),
        required: ">=2.0.0, <3.0.0".to_string(),
    })
}

#[cfg(feature = "pdk")]
pub fn register_extension(_input: RegisterExtensionInput) -> RegisterExtensionOutput {
    RegisterExtensionOutput {
        name: "callisto".to_string(),
        description: Some(
            "Unified multi-ecosystem release manager for moon workspaces".to_string(),
        ),
        plugin_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Returns callisto's moon-config schema: deliberately near-empty, since
/// callisto's real settings live in `callisto.toml` (see `callisto-graph`'s
/// manifest loading), not in moon's own `.moon/workspace.yml`
/// `extensions.callisto.config` block. moon's real
/// `DefineExtensionConfigOutput` carries a `schematic::Schema` (a typed
/// schema-description tree used to render config docs/validation), not
/// arbitrary JSON Schema -- an empty `SchemaType::Struct` with no fields is
/// its equivalent of "no config keys to declare here".
#[cfg(feature = "pdk")]
pub fn define_extension_config() -> DefineExtensionConfigOutput {
    DefineExtensionConfigOutput {
        schema: schematic::Schema::structure(schematic_types::StructType::default()),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecuteExtensionOutput {
    pub report: serde_json::Value,
    pub rendered: String,
    pub exit_code: i32,
}

#[cfg(feature = "pdk")]
fn format_graph_error_json(e: &callisto_graph::error::GraphError) -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": callisto_model::SCHEMA_VERSION,
        "error": {
            "code": "E_GRAPH",
            "message": e.to_string(),
        }
    })
}

/// Converts a serialization result into an [`ExecuteExtensionOutput`].
///
/// When `json_result` is `Ok`, the value and its pretty-printed rendering are
/// placed in the output with the supplied `exit_code`. When `json_result` is
/// `Err`, the serialization failure is surfaced as a structured error response
/// with `exit_code = 1` — preventing the host from receiving a silent
/// `null`/empty output that looks like success.
#[cfg(any(feature = "pdk", test))]
fn build_extension_output(
    json_result: serde_json::Result<serde_json::Value>,
    exit_code: i32,
) -> ExecuteExtensionOutput {
    match json_result {
        Ok(json_val) => {
            let rendered = serde_json::to_string_pretty(&json_val)
                .unwrap_or_else(|e| format!(r#"{{"error":"render failed: {e}"}}"#));
            ExecuteExtensionOutput {
                report: json_val,
                rendered,
                exit_code,
            }
        }
        Err(e) => {
            let msg = format!("internal serialization error: {e}");
            let json_val = serde_json::json!({
                "schemaVersion": callisto_model::SCHEMA_VERSION,
                "error": {
                    "code": "E_SERIALIZE",
                    "message": msg,
                }
            });
            let rendered = json_val.to_string();
            ExecuteExtensionOutput {
                report: json_val,
                rendered,
                exit_code: 1,
            }
        }
    }
}

/// Resolves the subcommand `execute_extension` should dispatch to from the
/// raw `args` moon passes on the command line: the first argument if
/// present, otherwise `"status"`.
///
/// Pulled out as its own pure function (no `CommandRunner`/workspace I/O) so
/// this fallback can be unit-tested directly, natively, without requiring a
/// real Extism guest -- unlike `execute_extension` itself, which unavoidably
/// references `crate::runner::MoonCommandRunner` and therefore only links
/// against a real wasm32 Extism host (see `runner.rs`'s `pdk`-feature impl).
///
/// NOTE: this only resolves *which* subcommand name to dispatch on; it does
/// not validate that name. The `match` in `execute_extension` treats any
/// name it doesn't specifically recognize (`"plan-publish"`/`"plan_publish"`,
/// `"validate"`) the same as no subcommand at all -- i.e. it silently falls
/// back to running `status`, exactly like an empty `args` list. There is
/// currently no distinct "unrecognized subcommand" error path.
#[cfg(feature = "pdk")]
fn resolve_subcommand(args: &[String]) -> &str {
    args.first().map(|s| s.as_str()).unwrap_or("status")
}

#[cfg(feature = "pdk")]
pub fn execute_extension(input: ExecuteExtensionInput) -> ExecuteExtensionOutput {
    use callisto_graph::commands::status::{status, StatusOptions};

    // moon's real `ExecuteExtensionInput` nests the workspace root (and cwd)
    // inside `context: MoonContext` rather than as a flat `workspace_root`
    // field. `MoonContext::workspace_root` is a `VirtualPath`; `.to_path_buf()`
    // returns the (possibly WASI-virtualized) path, matching how the old flat
    // field was consumed below.
    let root = input.context.workspace_root.to_path_buf();
    let runner = crate::runner::MoonCommandRunner;
    let locator = match crate::locator::MoonProjectLocator::new(&runner, root.clone()) {
        Ok(loc) => loc,
        Err(e) => {
            let json_val = serde_json::json!({
                "schemaVersion": callisto_model::SCHEMA_VERSION,
                "error": { "code": "E_LOCATE", "message": e.to_string() }
            });
            return ExecuteExtensionOutput {
                report: json_val.clone(),
                rendered: e.to_string(),
                exit_code: 1,
            };
        }
    };

    let ws = match callisto_graph::Workspace::load(root, &locator, &runner) {
        Ok(ws) => ws,
        Err(e) => {
            let json_val = format_graph_error_json(&e);
            return ExecuteExtensionOutput {
                report: json_val.clone(),
                rendered: e.to_string(),
                exit_code: 1,
            };
        }
    };

    let subcmd = resolve_subcommand(&input.args);

    match subcmd {
        "plan-publish" | "plan_publish" => {
            use callisto_graph::commands::publish::{plan_publish, PublishOptions};
            match plan_publish(&ws, &PublishOptions::default()) {
                Ok(report) => build_extension_output(serde_json::to_value(&report), 0),
                Err(e) => {
                    let json_val = format_graph_error_json(&e);
                    ExecuteExtensionOutput {
                        report: json_val.clone(),
                        rendered: e.to_string(),
                        exit_code: 1,
                    }
                }
            }
        }
        "validate" => {
            use callisto_graph::commands::validate::{validate, ValidateOptions};
            match validate(&ws, &ValidateOptions::default()) {
                Ok(report) => {
                    let exit_code = if report.valid { 0 } else { 1 };
                    build_extension_output(serde_json::to_value(&report), exit_code)
                }
                Err(e) => {
                    let json_val = format_graph_error_json(&e);
                    ExecuteExtensionOutput {
                        report: json_val.clone(),
                        rendered: e.to_string(),
                        exit_code: 1,
                    }
                }
            }
        }
        _ => {
            let opts = StatusOptions {
                strict: false,
                strict_graph: false,
            };

            match status(&ws, &opts) {
                Ok(report) => {
                    let has_errors = report
                        .diagnostics
                        .iter()
                        .any(|d| d.severity == callisto_model::DiagnosticSeverity::Error);
                    let exit_code = if has_errors { 1 } else { 0 };
                    build_extension_output(serde_json::to_value(&report), exit_code)
                }
                Err(e) => {
                    let json_val = format_graph_error_json(&e);
                    ExecuteExtensionOutput {
                        report: json_val.clone(),
                        rendered: e.to_string(),
                        exit_code: 1,
                    }
                }
            }
        }
    }
}

#[cfg(feature = "pdk")]
pub fn initialize_extension(
    input: InitializeExtensionInput,
) -> Result<InitializeExtensionOutput, LocateError> {
    use callisto_graph::commands::init::{init, InitOptions};

    // moon's real `InitializeExtensionInput` (= `InitializePluginInput`) only
    // carries `context: MoonContext` — there is no `confirmed`/`yes` flag in
    // the real protocol (that was a callisto-only field on the old, locally
    // invented type). `InitOptions.yes` now gates `init`'s reconcile-apply
    // path (docs/00-design.md §18 Q5.4 mechanism 1): on a first run it has
    // no effect (scaffolding an absent `callisto.toml` is always a direct
    // write), and on a re-run it decides whether detected drift (e.g. a
    // newly-appeared ecosystem) is written or only reported. There is no
    // host-side prompt surface here to relay a diff through, so defaulting
    // to `true` — auto-applying reconcile drift — is the closest behavior-
    // preserving choice to the old unconditional-write behavior.
    let root = input.context.workspace_root.to_path_buf();
    let runner = crate::runner::MoonCommandRunner;
    let locator = crate::locator::MoonProjectLocator::new(&runner, root.clone())?;
    let ws = callisto_graph::Workspace::load(root, &locator, &runner)
        .map_err(|e| LocateError::Graph(Box::new(e)))?;
    let opts = InitOptions { yes: true };

    // Run callisto's existing init-detection/scaffolding logic. moon's real
    // `InitializeExtensionOutput` (= `InitializePluginOutput`) has no field
    // that can carry an arbitrary `InitReport` (schema version, config path,
    // diagnostics) — its shape is specifically for describing settings to
    // inject into moon's own toolchain config and prompts to ask the user,
    // neither of which callisto's `InitReport` maps onto cleanly. We still
    // run `init` for its side effects (scaffolding `callisto.toml` /
    // `.changeset`) and to propagate any error, but intentionally discard
    // the returned `InitReport` rather than inventing new output fields
    // moon doesn't expect. All fields below are therefore sensible defaults:
    // callisto has no hosted config/docs URL to advertise, no moon toolchain
    // settings to pre-populate, and no interactive prompts to ask.
    // This host surface has no `--dry-run` equivalent -- a moon extension
    // initialization is always a real scaffolding write -- so the permit is
    // granted unconditionally here rather than derived from a user flag. It
    // still goes through the one sanctioned constructor.
    let permit = callisto_model::ApplyPermit::granted_unless_dry_run(false);
    let _report = init(&ws, &opts, permit.as_ref()).map_err(|e| LocateError::Graph(Box::new(e)))?;

    Ok(InitializeExtensionOutput {
        config_url: None,
        default_settings: Default::default(),
        docs_url: None,
        prompts: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_moon_version_check() {
        assert!(check_moon_version("2.0.0").is_ok());
        assert!(check_moon_version("2.4.6").is_ok());
        assert!(check_moon_version("1.45.2").is_err());
        assert!(check_moon_version("3.0.0").is_err());
    }

    // NOTE on scope: `register_extension` is the only one of the three
    // `pdk`-feature plugin functions (`register_extension`,
    // `execute_extension`, `initialize_extension`) that is safe to call
    // directly from a native (non-wasm32) unit test. The other two
    // unconditionally construct a `crate::runner::MoonCommandRunner` and
    // pass it into `MoonProjectLocator`/`Workspace::load`; under the `pdk`
    // feature, `MoonCommandRunner`'s `CommandRunner` impl calls
    // `warpgate_pdk::exec`/`into_virtual_path`, which reference
    // Extism-host-only `extern "C"` imports (`_exec_command`,
    // `_to_virtual_path`, `_alloc`, ...) that plainly don't exist for native
    // linking. This isn't a theoretical concern: a native test that actually
    // calls `MoonCommandRunner::run` (or anything that transitively calls
    // it, which `execute_extension`/`initialize_extension` both do
    // unconditionally, regardless of which runtime branch would actually be
    // taken) fails at *link* time with "Undefined symbols for architecture
    // ...: _alloc, _exec_command, ...", not a runtime panic. Wire-level
    // coverage for `execute_extension`/`initialize_extension` (including
    // their error paths) therefore lives in
    // `tests/moon_wasm_sandbox.rs`, which drives the real compiled wasm
    // module through an actual Extism/wasmtime host. See also
    // `resolve_subcommand`, extracted specifically so at least
    // `execute_extension`'s dispatch fallback is natively unit-testable
    // without hitting this constraint.
    #[cfg(feature = "pdk")]
    #[test]
    fn register_extension_returns_well_formed_metadata_for_valid_input() {
        let input = RegisterExtensionInput {
            id: moon_pdk_api::Id::raw("callisto"),
        };

        let output = register_extension(input);

        assert_eq!(output.name, "callisto");
        assert_eq!(output.plugin_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            output.description.as_deref(),
            Some("Unified multi-ecosystem release manager for moon workspaces")
        );
    }

    // `register_extension`'s only input field (`id: Id`) has no invalid
    // in-process representation once it has been deserialized into a Rust
    // `RegisterExtensionInput` -- there is no "malformed but well-typed"
    // value to construct here. The JSON deserialization boundary this test
    // would otherwise cover lives one layer up, in the `#[plugin_fn]`
    // wire wrapper (`lib.rs`'s `plugin::register_extension`, via
    // `extism_pdk::input::<Json<RegisterExtensionInput>>()`), which only
    // exists/executes inside a real Extism guest. See
    // `register_extension_malformed_json_fails_cleanly_not_panics` in
    // `tests/moon_wasm_sandbox.rs` for that boundary's real-wire coverage,
    // and `extism-pdk-derive`'s `plugin_fn` macro expansion (confirmed by
    // reading its source for this task) for why a deserialization failure
    // there is handled cleanly (`extism_pdk::unwrap!` sets the Extism error
    // and returns `-1` from the guest export) rather than panicking/trapping.

    #[cfg(feature = "pdk")]
    #[test]
    fn resolve_subcommand_defaults_to_status_when_args_empty() {
        assert_eq!(resolve_subcommand(&[]), "status");
    }

    #[cfg(feature = "pdk")]
    #[test]
    fn resolve_subcommand_uses_first_arg_when_present() {
        assert_eq!(resolve_subcommand(&["validate".to_string()]), "validate");
        assert_eq!(
            resolve_subcommand(&["plan-publish".to_string(), "--extra".to_string()]),
            "plan-publish"
        );
    }

    #[cfg(feature = "pdk")]
    #[test]
    fn resolve_subcommand_passes_through_unrecognized_names_unvalidated() {
        // `resolve_subcommand` itself does no validation -- it just picks
        // which string `execute_extension`'s `match` dispatches on. An
        // unrecognized name is passed through as-is; whether that name maps
        // to an error or a fallback is entirely the caller's `match`'s
        // decision (see the doc comment on `resolve_subcommand` and
        // `execute_extension_unrecognized_subcommand_falls_back_to_status_not_error`
        // in `tests/moon_wasm_sandbox.rs` for what `execute_extension`
        // actually does with it today).
        assert_eq!(
            resolve_subcommand(&["totally-bogus-subcommand".to_string()]),
            "totally-bogus-subcommand"
        );
    }

    #[cfg(feature = "pdk")]
    #[test]
    fn format_graph_error_json_has_stable_error_shape() {
        let err = callisto_graph::error::GraphError::Locate(LocateError::MoonUnavailable);

        let json = format_graph_error_json(&err);

        assert_eq!(
            json["schemaVersion"],
            serde_json::json!(callisto_model::SCHEMA_VERSION)
        );
        assert_eq!(json["error"]["code"], "E_GRAPH");
        assert_eq!(json["error"]["message"], err.to_string());
        assert!(!json["error"]["message"].as_str().unwrap().trim().is_empty());
    }

    // --- build_extension_output tests (no pdk feature required) ---

    /// A serialization failure must produce a structured error output with
    /// `exit_code = 1`, not a silent `null`/empty result that looks like success.
    #[test]
    fn build_extension_output_returns_error_response_on_serialize_failure() {
        // Manufacture a serde_json::Error by deserializing invalid JSON.
        let serialize_err: serde_json::Error =
            serde_json::from_str::<serde_json::Value>("not valid json").unwrap_err();

        let output = build_extension_output(Err(serialize_err), 0);

        assert_eq!(
            output.exit_code, 1,
            "a serialization failure must set exit_code=1, not the caller's proposed code"
        );
        assert_eq!(
            output.report["error"]["code"], "E_SERIALIZE",
            "error code must be E_SERIALIZE"
        );
        assert_eq!(
            output.report["schemaVersion"],
            serde_json::json!(callisto_model::SCHEMA_VERSION)
        );
        assert!(
            !output.report["error"]["message"]
                .as_str()
                .unwrap()
                .is_empty(),
            "error message must be non-empty"
        );
        assert!(
            !output.rendered.is_empty(),
            "rendered output must be non-empty even on error"
        );
    }

    /// On the success path the exit code and report value are preserved.
    #[test]
    fn build_extension_output_success_path_preserves_value_and_exit_code() {
        let value = serde_json::json!({"status": "ok", "packages": []});

        let output = build_extension_output(Ok(value.clone()), 0);

        assert_eq!(output.exit_code, 0);
        assert_eq!(output.report, value);
        assert!(
            !output.rendered.is_empty(),
            "rendered must be a non-empty pretty-printed string"
        );
    }

    /// A non-zero exit code on the success path is preserved unchanged.
    #[test]
    fn build_extension_output_success_path_preserves_nonzero_exit_code() {
        let value = serde_json::json!({"valid": false});

        let output = build_extension_output(Ok(value.clone()), 1);

        assert_eq!(output.exit_code, 1);
        assert_eq!(output.report, value);
    }
}
