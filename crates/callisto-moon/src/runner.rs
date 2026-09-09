#[cfg(not(feature = "pdk"))]
use std::path::Path;

use callisto_model::CommandError;
#[cfg(not(feature = "pdk"))]
use callisto_model::{CommandOutput, CommandRunner};

pub struct MoonCommandRunner;

// The `#[cfg(feature = "pdk")]` `CommandRunner` impl for `MoonCommandRunner`
// (the wasm32-wasip1 guest path, calling `warpgate_pdk::exec`) lives in
// `runner_pdk.rs`, split out so `cargo-llvm-cov` can exclude that file --
// see that file's module doc comment for why.

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
            Err(e) => Err(classify_native_io_error(program, &e)),
        }
    }
}

// Native spawn failures carry a real `std::io::Error`, so classification
// uses `ErrorKind::NotFound` directly (mirroring
// `callisto_cli::runner::CliCommandRunner::run`) rather than the
// message-text heuristic below: that heuristic is a fallback for the `pdk`
// (wasm) path in `runner_pdk.rs`, where host-exec failures carry no
// `ErrorKind` at all.
#[cfg(not(feature = "pdk"))]
fn classify_native_io_error(program: &str, e: &std::io::Error) -> CommandError {
    if e.kind() == std::io::ErrorKind::NotFound {
        CommandError::NotFound {
            program: program.to_string(),
        }
    } else {
        CommandError::Io {
            program: program.to_string(),
            message: e.to_string(),
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

    /// Proves the native path classifies via `ErrorKind::NotFound` directly
    /// rather than sniffing the error's `Display` text: a synthetic
    /// `io::Error` carrying `ErrorKind::NotFound` but a message that does
    /// NOT contain "not found" or "no such file" (so it would fail the old
    /// string-heuristic classification) must still classify as
    /// `CommandError::NotFound`.
    #[cfg(not(feature = "pdk"))]
    #[test]
    fn classify_native_io_error_uses_error_kind_not_message_text() {
        let e = std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "totally unrelated wording, e.g. locale-specific",
        );
        let err = classify_native_io_error("ghost-cmd", &e);
        assert_eq!(
            err,
            CommandError::NotFound {
                program: "ghost-cmd".to_string()
            }
        );
    }

    /// Sibling check: a non-`NotFound` `ErrorKind` whose message happens to
    /// contain "not found" text must still classify as `Io`, not
    /// `NotFound` -- proving the native path keys off `ErrorKind`, not the
    /// message.
    #[cfg(not(feature = "pdk"))]
    #[test]
    fn classify_native_io_error_does_not_misclassify_on_message_text_alone() {
        let e = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "file not found in a weird phrasing",
        );
        let err = classify_native_io_error("git", &e);
        assert_eq!(
            err,
            CommandError::Io {
                program: "git".to_string(),
                message: e.to_string(),
            }
        );
    }
}
