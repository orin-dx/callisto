use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use callisto_model::{CommandError, CommandOutput, CommandRunner};

pub struct CliCommandRunner;

impl CommandRunner for CliCommandRunner {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
        let output = std::process::Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();

        match output {
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
                // Print stderr to the terminal before constructing the output
                // so the caller sees it even if they don't inspect the field.
                if !stderr.is_empty() {
                    eprint!("{stderr}");
                }
                Ok(CommandOutput {
                    exit_code: o.status.code(),
                    stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                    stderr,
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CommandError::NotFound {
                program: program.to_string(),
            }),
            Err(e) => Err(CommandError::Io {
                program: program.to_string(),
                message: e.to_string(),
            }),
        }
    }

    fn run_with_timeout(
        &self,
        program: &str,
        args: &[&str],
        cwd: &Path,
        timeout: Duration,
    ) -> Result<CommandOutput, CommandError> {
        let mut child = std::process::Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
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
            })?;

        // Read stdout/stderr in separate threads to prevent pipe-buffer deadlock
        // when the child produces output while we wait for it to exit.
        let stdout_handle = child.stdout.take().unwrap();
        let stderr_handle = child.stderr.take().unwrap();

        let stdout_thread = std::thread::spawn(move || {
            let mut buf = String::new();
            drop(std::io::BufReader::new(stdout_handle).read_to_string(&mut buf));
            buf
        });
        // Stream stderr line-by-line so publish progress (cargo/npm/twine
        // write to stderr) appears in real time rather than after the process
        // exits. The full text is still accumulated for caller analysis.
        let stderr_thread = std::thread::spawn(move || {
            use std::io::BufRead;
            let mut buf = String::new();
            for line in std::io::BufReader::new(stderr_handle).lines() {
                match line {
                    Ok(l) => {
                        eprintln!("{l}");
                        buf.push_str(&l);
                        buf.push('\n');
                    }
                    Err(_) => break,
                }
            }
            buf
        });

        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait().map_err(|e| CommandError::Io {
                program: program.to_string(),
                message: e.to_string(),
            })? {
                Some(s) => break s,
                None => {
                    if Instant::now() >= deadline {
                        drop(child.kill());
                        drop(child.wait());
                        drop(stdout_thread.join());
                        drop(stderr_thread.join());
                        return Err(CommandError::TimedOut {
                            program: program.to_string(),
                            seconds: timeout.as_secs(),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };

        let stdout = stdout_thread.join().unwrap_or_default();
        let stderr = stderr_thread.join().unwrap_or_default();

        Ok(CommandOutput {
            exit_code: status.code(),
            stdout,
            stderr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_with_timeout_kills_slow_process_and_returns_timed_out() {
        let runner = CliCommandRunner;
        // sleep for 1 second with a 100ms timeout — process must be killed.
        let err = runner
            .run_with_timeout(
                "sleep",
                &["1"],
                std::path::Path::new("."),
                Duration::from_millis(100),
            )
            .unwrap_err();
        assert!(
            matches!(err, CommandError::TimedOut { .. }),
            "expected TimedOut, got: {err:?}"
        );
    }

    #[test]
    fn run_with_timeout_returns_output_for_fast_process() {
        let runner = CliCommandRunner;
        let out = runner
            .run_with_timeout(
                "true",
                &[],
                std::path::Path::new("."),
                Duration::from_secs(5),
            )
            .unwrap();
        assert!(out.success());
    }

    /// F-007: subprocess stderr must be streamed line-by-line rather than
    /// buffered until process exit. The fix: switch from `read_to_string`
    /// (accumulates everything) to a `BufReader::lines` loop that emits each
    /// line to the terminal immediately while still accumulating the full
    /// text for caller analysis.
    ///
    /// We verify that `CommandOutput::stderr` still contains the full stderr
    /// text after streaming (so callers can still classify errors from it).
    #[test]
    fn run_with_timeout_stderr_is_fully_captured_after_streaming() {
        let runner = CliCommandRunner;
        // A process that prints multiple lines to stderr, one at a time.
        // Using sh -c with printf to emit to stderr.
        let out = runner
            .run_with_timeout(
                "sh",
                &["-c", "echo line1 >&2; echo line2 >&2"],
                std::path::Path::new("."),
                Duration::from_secs(5),
            )
            .unwrap();

        assert!(out.success());
        assert!(
            out.stderr.contains("line1") && out.stderr.contains("line2"),
            "stderr must contain all emitted lines even when streamed; got: {:?}",
            out.stderr
        );
    }
}
