use std::path::Path;

use callisto_model::{CommandError, CommandOutput, CommandRunner};

pub struct MoonCommandRunner;

// Under wasm32-wasip1 (the `pdk` build target), the WASI guest cannot spawn
// child processes at all — `std::process::Command` fails unconditionally.
// moon's PDK bridges process execution back to the host via the
// `exec_command` Extism host function; `warpgate_pdk::exec` is the typed
// wrapper around that host-fn call. A dedicated R&D spike confirmed
// gitoxide's low-level crates cannot read object content under
// wasm32-wasip1 either (memmap2 has no WASI backend), ruling out
// native in-guest git access as an alternative to this host-exec seam;
// see the "gix-in-WASM spike ruled out" finding in the external
// callisto-findings ledger (sibling repo: callisto-findings/FINDINGS.md)
// for the full writeup.
//
// IMPORTANT (bug found and fixed via black-box testing against the real
// wasm sandbox, see `tests/moon_wasm_sandbox.rs`): `warpgate`'s host-side
// `exec_command` implementation (`warpgate::host::exec_command`, in the
// pinned `warpgate` 0.30.5 this crate resolves to transitively) does NOT
// report "command not found" as a normal `ExecCommandOutput`/error value the
// guest's `warpgate_pdk::exec` can catch. Instead, when the requested
// program can't be resolved on `PATH`
// (`system_env::find_command_on_path` returns `None`), the host function's
// own Rust closure returns `Err(WarpgatePluginError::MissingCommand)`
// directly. Extism host functions that return `Err` from their native
// closure abort the *entire* plugin call as a host-function failure — this
// is NOT routed back into the guest as a value `warpgate_pdk::exec`'s
// `Result` can represent; the guest's Rust code (this `run` method
// included) never resumes. From the caller's perspective (moon, or this
// crate's own `PluginContainer::call_func_with` in tests), the whole
// `execute_extension`/`initialize_extension` wire call fails outright
// (`WarpgatePluginError::FailedPluginCall`), not a clean
// `ExecuteExtensionOutput { exit_code: 1, report: { "error": ... } } }`
// response. This directly contradicted this crate's "execute_extension
// never panics/traps; every failure is caught into the report" invariant
// for the single most common real-world failure mode (the host tool the
// extension shells out to -- `moon` itself, or `git` -- being missing).
//
// The fix: proactively check whether `program` exists BEFORE calling
// `warpgate_pdk::exec` at all, via `warpgate_pdk::command_exists` (which
// shells out to `which`/`Get-Command`, not to `program` itself, so a
// missing `program` is reported as a normal `false` rather than tripping
// the same `MissingCommand` trap -- `which`/`Get-Command` being absent is
// an accepted, far less likely residual risk that `warpgate_pdk` itself
// takes on for every consumer). This turns "program not found" into the
// clean `CommandError::NotFound` this method already promises to produce,
// without ever calling `exec` for a program we already know is absent.
#[cfg(feature = "pdk")]
impl CommandRunner for MoonCommandRunner {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
        use warpgate_pdk::{get_host_environment, into_virtual_path, ExecCommandInput};

        let host_env = get_host_environment().map_err(|e| classify_host_failure(program, &e.to_string()))?;

        if !warpgate_pdk::command_exists(&host_env, program) {
            return Err(CommandError::NotFound {
                program: program.to_string(),
            });
        }

        let cwd = match into_virtual_path(cwd) {
            Ok(vpath) => Some(vpath),
            Err(e) => return Err(classify_host_failure(program, &e.to_string())),
        };

        let input = ExecCommandInput {
            command: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd,
            ..ExecCommandInput::default()
        };

        match warpgate_pdk::exec(input) {
            Ok(output) => Ok(CommandOutput {
                exit_code: Some(output.exit_code),
                stdout: output.stdout,
                stderr: output.stderr,
            }),
            Err(e) => Err(classify_host_failure(program, &e.to_string())),
        }
    }
}

// Native (non-WASM) builds of this crate — e.g. `cargo check`/`cargo test`
// without the `pdk` feature — never run inside a WASI guest, so plain
// subprocess spawning works and keeps this path testable off the wasm
// target.
#[cfg(not(feature = "pdk"))]
impl CommandRunner for MoonCommandRunner {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
        let mut cmd = std::process::Command::new(program);
        cmd.args(args);
        cmd.current_dir(cwd);

        match cmd.output() {
            Ok(output) => Ok(CommandOutput {
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            }),
            Err(e) => Err(classify_host_failure(program, &e.to_string())),
        }
    }
}

pub fn classify_host_failure(program: &str, message: &str) -> CommandError {
    if looks_like_not_found(message) {
        CommandError::NotFound {
            program: program.to_string(),
        }
    } else {
        CommandError::Io {
            program: program.to_string(),
            message: message.to_string(),
        }
    }
}

fn looks_like_not_found(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("not found") || lower.contains("no such file")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_not_found_failures() {
        let err = classify_host_failure("ghost-cmd", "exec: \"ghost-cmd\": executable file not found in $PATH");
        assert_eq!(
            err,
            CommandError::NotFound {
                program: "ghost-cmd".to_string()
            }
        );
    }

    #[test]
    fn classifies_other_failures_as_io() {
        let err = classify_host_failure("git", "permission denied");
        assert_eq!(
            err,
            CommandError::Io {
                program: "git".to_string(),
                message: "permission denied".to_string()
            }
        );
    }

    #[cfg(not(feature = "pdk"))]
    #[test]
    fn native_runner_executes_and_captures_output() {
        let runner = MoonCommandRunner;
        let cwd = std::env::current_dir().unwrap();
        let out = runner.run("echo", &["hello"], &cwd).unwrap();
        assert!(out.success());
        assert_eq!(out.stdout_trimmed(), "hello");
    }

    #[cfg(not(feature = "pdk"))]
    #[test]
    fn native_runner_reports_not_found() {
        let runner = MoonCommandRunner;
        let cwd = std::env::current_dir().unwrap();
        let err = runner
            .run("callisto-definitely-not-a-real-binary", &[], &cwd)
            .unwrap_err();
        assert!(matches!(err, CommandError::NotFound { .. }));
    }
}
