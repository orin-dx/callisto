//! `execute_extension`/`initialize_extension` -- the two `pdk`-feature
//! functions that transitively call `crate::runner::MoonCommandRunner`'s
//! wasm-only `CommandRunner` impl (via `Workspace::load`/`MoonProjectLocator`),
//! and so can never actually execute inside a native `cargo-llvm-cov`
//! instrumented build (see `runner_pdk.rs`'s doc comment for the underlying
//! wasm-only-extern reason). Split into their own file so
//! `--ignore-filename-regex` can exclude it from coverage reports.
//!
//! Wire-level coverage (including their error paths) lives in
//! `tests/moon_wasm_sandbox.rs`, which drives the real compiled
//! `callisto-moon.wasm` module through an actual Extism/wasmtime host.
//!
//! `register_extension`, `define_extension_config`, `resolve_subcommand`,
//! and `format_graph_error_json` stay in `extension.rs`: none of them call
//! `MoonCommandRunner`, so they link and run fine natively under
//! `--features pdk` and are exercised by real native unit tests there.

use crate::extension::{build_extension_output, format_graph_error_json, resolve_subcommand, ExecuteExtensionOutput};
use callisto_graph::locate::LocateError;

#[cfg(feature = "pdk")]
pub fn execute_extension(input: moon_pdk_api::ExecuteExtensionInput) -> ExecuteExtensionOutput {
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
                    let exit_code = if report.ok { 0 } else { 1 };
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
    input: moon_pdk_api::InitializeExtensionInput,
) -> Result<moon_pdk_api::InitializeExtensionOutput, LocateError> {
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
    let ws = callisto_graph::Workspace::load(root, &locator, &runner).map_err(|e| LocateError::Graph(Box::new(e)))?;
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

    Ok(moon_pdk_api::InitializeExtensionOutput {
        config_url: None,
        default_settings: Default::default(),
        docs_url: None,
        prompts: Vec::new(),
    })
}
