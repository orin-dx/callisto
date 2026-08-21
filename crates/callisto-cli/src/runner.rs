use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use callisto_model::{CommandError, CommandOutput, CommandRunner};

/// Caps how much of a subprocess's stdout/stderr this process will retain
/// in memory. The pipe is still drained in full past this point (bytes
/// beyond the cap are read and discarded, never accumulated) so a chatty
/// child can never deadlock waiting on a full OS pipe buffer -- this
/// bounds memory, not duration (`PUBLISH_TIMEOUT_SECS` bounds that
/// separately).
const MAX_CAPTURED_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

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
        run_with_timeout_impl(program, args, cwd, timeout, StderrMode::Live)
    }

    fn run_quiet(
        &self,
        program: &str,
        args: &[&str],
        cwd: &Path,
        timeout: Duration,
    ) -> Result<CommandOutput, CommandError> {
        run_with_timeout_impl(program, args, cwd, timeout, StderrMode::Quiet)
    }
}

/// Controls whether [`run_with_timeout_impl`]'s stderr reader thread prints
/// each line live as it arrives. Captured `CommandOutput.stderr` is
/// identical either way -- this only affects what's echoed to the
/// terminal while the subprocess runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StderrMode {
    Live,
    Quiet,
}

fn run_with_timeout_impl(
    program: &str,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
    stderr_mode: StderrMode,
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

    // The reader threads signal completion over a channel rather than
    // being joined directly. Killing (or the natural exit of) the
    // direct child does NOT close pipe fds held open by a descendant
    // process that inherited them (common for npm/cargo/python publish
    // lifecycle scripts spawning subprocesses). If that happens, the
    // blocking `read()` inside these threads never returns. Using
    // `recv_timeout` below lets us bound how long we wait for them
    // without ever blocking the caller indefinitely.
    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel::<String>();
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel::<String>();

    std::thread::spawn(move || {
        let mut reader = stdout_handle;
        let mut captured: Vec<u8> = Vec::new();
        let mut truncated = false;
        let mut chunk = [0u8; 65536];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    // Bytes past the cap are still read (draining the
                    // pipe so the child never blocks on a full buffer)
                    // but not retained.
                    if !truncated {
                        let remaining = MAX_CAPTURED_OUTPUT_BYTES - captured.len();
                        let take = n.min(remaining);
                        captured.extend_from_slice(&chunk[..take]);
                        if take < n {
                            truncated = true;
                        }
                    }
                }
                Err(_) => break,
            }
        }
        let mut text = String::from_utf8_lossy(&captured).into_owned();
        if truncated {
            text.push_str("\n...[output truncated]\n");
        }
        drop(stdout_tx.send(text));
    });
    // Stream stderr line-by-line so publish progress (cargo/npm/twine
    // write to stderr) appears in real time rather than after the
    // process exits, while capping the accumulated text returned to
    // the caller. Deliberately reads raw bytes rather than using
    // `BufReader::lines()`: that iterator has no size bound on a
    // single line, so a flood with no newlines at all would still
    // buffer unboundedly inside it before ever yielding a line to cap.
    // `stderr_mode` gates only the live eprintln -- captured output is
    // identical either way.
    std::thread::spawn(move || {
        let mut reader = stderr_handle;
        let mut captured: Vec<u8> = Vec::new();
        let mut truncated = false;
        let mut pending_line: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 65536];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let data = &chunk[..n];
                    for &b in data {
                        if b == b'\n' {
                            if pending_line.last() == Some(&b'\r') {
                                pending_line.pop();
                            }
                            if stderr_mode == StderrMode::Live {
                                eprintln!("{}", String::from_utf8_lossy(&pending_line));
                            }
                            pending_line.clear();
                        } else if pending_line.len() < MAX_CAPTURED_OUTPUT_BYTES {
                            pending_line.push(b);
                        }
                    }
                    if !truncated {
                        let remaining = MAX_CAPTURED_OUTPUT_BYTES - captured.len();
                        let take = data.len().min(remaining);
                        captured.extend_from_slice(&data[..take]);
                        if take < data.len() {
                            truncated = true;
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if !pending_line.is_empty() {
            if pending_line.last() == Some(&b'\r') {
                pending_line.pop();
            }
            if stderr_mode == StderrMode::Live {
                eprintln!("{}", String::from_utf8_lossy(&pending_line));
            }
        }
        let mut text = String::from_utf8_lossy(&captured).into_owned();
        if truncated {
            text.push_str("\n...[output truncated]\n");
        }
        drop(stderr_tx.send(text));
    });

    // Grace period to wait for the reader threads after the direct
    // child has exited (or been killed). Bounded so a lingering
    // descendant holding the pipe open can never hang the caller.
    const READER_GRACE: Duration = Duration::from_secs(3);

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
                    // Reader threads are intentionally not joined here:
                    // if a descendant process is still holding a pipe
                    // fd open, join() could block forever. We wait a
                    // bounded grace period for output, then abandon the
                    // threads (they leak until the descendant
                    // eventually exits and closes the fd -- a thread
                    // leak, not a memory-safety issue).
                    let stdout = stdout_rx.recv_timeout(READER_GRACE).ok();
                    let stderr = stderr_rx.recv_timeout(READER_GRACE).ok();
                    if stdout.is_none() || stderr.is_none() {
                        eprintln!(
                            "warning: `{program}` timed out and a descendant process \
                                 appears to still hold its output pipes open; captured \
                                 output may be incomplete"
                        );
                    }
                    return Err(CommandError::TimedOut {
                        program: program.to_string(),
                        seconds: timeout.as_secs(),
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };

    // The direct child has exited on its own. Even so, a descendant
    // process can still hold the pipe fds open (e.g. a backgrounded
    // job spawned by a shell script), so bound the wait here too.
    let stdout_result = stdout_rx.recv_timeout(READER_GRACE);
    let stderr_result = stderr_rx.recv_timeout(READER_GRACE);
    if stdout_result.is_err() || stderr_result.is_err() {
        eprintln!(
            "warning: `{program}` exited but a descendant process appears to still \
                 hold its output pipes open; captured output may be incomplete"
        );
    }

    Ok(CommandOutput {
        exit_code: status.code(),
        stdout: stdout_result.unwrap_or_default(),
        stderr: stderr_result.unwrap_or_default(),
    })
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

    /// Regression test for a hung-descendant deadlock: killing the direct
    /// child does not close pipe fds inherited by a grandchild process that
    /// outlives it. The direct child (`sh`) exits almost immediately, but a
    /// backgrounded grandchild (`sleep 30`) keeps stderr open well past
    /// that. If the reader threads are joined unconditionally, this call
    /// hangs for ~30s regardless of the requested timeout. The fix bounds
    /// how long we wait on the reader threads so the call always returns
    /// promptly.
    #[test]
    fn run_with_timeout_bounds_reader_join_when_descendant_holds_pipe_open() {
        let runner = CliCommandRunner;
        let start = Instant::now();
        let result = runner.run_with_timeout(
            "sh",
            &["-c", "sleep 30 >&2 & exit 0"],
            std::path::Path::new("."),
            Duration::from_secs(2),
        );
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(6),
            "run_with_timeout must not block on a descendant process holding \
             stdio pipes open; took {elapsed:?}, result: {result:?}"
        );
        assert!(
            result.is_ok(),
            "expected Ok despite a lingering descendant holding the pipe open, got: {result:?}"
        );
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

    /// Regression test for the timeout-KILL branch's own grace-period logic
    /// (runner.rs:124-146), which is structurally similar to but
    /// independently written from the normal-exit branch's grace-period
    /// logic (runner.rs:153-163) covered by
    /// `run_with_timeout_bounds_reader_join_when_descendant_holds_pipe_open`.
    /// That existing test's direct child (`sh`) exits almost immediately, so
    /// it only ever exercises the exit branch. Here the direct child itself
    /// (`sh -c "sleep 30 >&2 & sleep 10"`) outlives the configured timeout,
    /// so `run_with_timeout` must actually kill it, while a backgrounded
    /// grandchild (`sleep 30 >&2`) keeps stderr open independently of the
    /// kill.
    ///
    /// Because the warning text is emitted via `eprintln!` from inside
    /// `run_with_timeout` itself (not captured in `CommandOutput`), this
    /// test re-execs the current test binary as a child process with
    /// `--nocapture` so the real OS-level stderr can be captured and
    /// asserted on, rather than being swallowed by the outer test harness.
    #[test]
    fn run_with_timeout_warns_on_timeout_branch_when_descendant_holds_pipe_open() {
        const CHILD_ENV: &str = "CALLISTO_RUN_TIMEOUT_KILL_CHILD";

        if std::env::var(CHILD_ENV).is_ok() {
            let runner = CliCommandRunner;
            // `sh` on this platform forks a separate process for the
            // foreground `sleep 10` rather than exec-replacing itself (it
            // must remain able to reap the backgrounded job), so killing
            // the direct child does not kill either descendant. Both the
            // backgrounded `sleep 30` and the foreground `sleep 10` inherit
            // the piped stdout/stderr fds unless redirected away, so each
            // is redirected explicitly: `sleep 30`'s stdout goes to
            // /dev/null so only its stderr lingers (the condition under
            // test), and `sleep 10`'s stdout/stderr both go to /dev/null so
            // it doesn't *also* hold either pipe open, which would stack a
            // second sequential 3s grace wait on top of the first.
            drop(runner.run_with_timeout(
                "sh",
                &["-c", "sleep 30 >&2 1>/dev/null & sleep 10 >/dev/null 2>&1"],
                std::path::Path::new("."),
                Duration::from_millis(800),
            ));
            return;
        }

        let exe = std::env::current_exe().expect("current_exe should be available in tests");
        let start = Instant::now();
        let output = std::process::Command::new(exe)
            .arg("--exact")
            .arg("runner::tests::run_with_timeout_warns_on_timeout_branch_when_descendant_holds_pipe_open")
            .arg("--nocapture")
            .env(CHILD_ENV, "1")
            .output()
            .expect("failed to re-exec test binary");
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(6),
            "run_with_timeout's timeout-kill branch must not block past its \
             bounded grace period even when a descendant holds stdio pipes \
             open; took {elapsed:?}"
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(
                "timed out and a descendant process appears to still hold its output pipes open"
            ),
            "expected the timeout branch's specific warning text in child stderr, got: {stderr}"
        );
    }

    /// A subprocess that writes far more than `MAX_CAPTURED_OUTPUT_BYTES`
    /// to stdout must not have all of it retained in `CommandOutput` (an
    /// unbounded accumulation is a memory-exhaustion DoS before the
    /// wall-clock publish timeout ever fires), but the pipe must still be
    /// drained in full -- proven here by the call completing well within
    /// its timeout instead of hanging (a reader that stops calling
    /// `read()` after the cap would leave the child blocked writing to a
    /// full OS pipe buffer).
    #[test]
    fn run_with_timeout_caps_accumulated_stdout_and_still_drains_the_pipe() {
        let runner = CliCommandRunner;
        let over_cap = MAX_CAPTURED_OUTPUT_BYTES + 1_000_000;
        let start = Instant::now();
        let out = runner
            .run_with_timeout(
                "sh",
                &["-c", &format!("head -c {over_cap} /dev/zero")],
                std::path::Path::new("."),
                Duration::from_secs(30),
            )
            .unwrap();
        let elapsed = start.elapsed();

        assert!(out.success());
        assert!(
            elapsed < Duration::from_secs(15),
            "must not hang waiting for the child to finish writing past the cap; took {elapsed:?}"
        );
        assert!(
            out.stdout.len() < over_cap,
            "captured stdout must be bounded, not the full {over_cap} bytes written; got {} bytes",
            out.stdout.len()
        );
        assert!(
            out.stdout.contains("[output truncated]"),
            "truncated output must say so"
        );
    }

    /// Same as the stdout case, but for stderr -- and deliberately with NO
    /// newlines at all, so the flood can't be capped by a per-line
    /// mechanism (a `BufReader::lines()`-based reader has no size bound on
    /// a single line and would buffer the entire flood internally before
    /// ever handing a line back, defeating any cap applied only after a
    /// line is yielded).
    #[test]
    fn run_with_timeout_caps_accumulated_stderr_with_no_newlines_and_still_drains_the_pipe() {
        let runner = CliCommandRunner;
        let over_cap = MAX_CAPTURED_OUTPUT_BYTES + 1_000_000;
        let start = Instant::now();
        let out = runner
            .run_with_timeout(
                "sh",
                &["-c", &format!("head -c {over_cap} /dev/zero >&2")],
                std::path::Path::new("."),
                Duration::from_secs(30),
            )
            .unwrap();
        let elapsed = start.elapsed();

        assert!(out.success());
        assert!(
            elapsed < Duration::from_secs(15),
            "must not hang waiting for the child to finish writing past the cap; took {elapsed:?}"
        );
        assert!(
            out.stderr.len() < over_cap,
            "captured stderr must be bounded, not the full {over_cap} bytes written; got {} bytes",
            out.stderr.len()
        );
        assert!(
            out.stderr.contains("[output truncated]"),
            "truncated output must say so"
        );
    }

    /// Output comfortably under the cap must be completely unaffected --
    /// no truncation marker, byte-for-byte length preserved.
    #[test]
    fn run_with_timeout_does_not_truncate_output_under_the_cap() {
        let runner = CliCommandRunner;
        let out = runner
            .run_with_timeout(
                "sh",
                &["-c", "printf 'hello stdout'; printf 'hello stderr' >&2"],
                std::path::Path::new("."),
                Duration::from_secs(5),
            )
            .unwrap();

        assert_eq!(out.stdout, "hello stdout");
        assert!(!out.stdout.contains("[output truncated]"));
        assert!(out.stderr.contains("hello stderr"));
        assert!(!out.stderr.contains("[output truncated]"));
    }

    /// `run_quiet` must still fully capture stderr into the returned
    /// `CommandOutput` -- only the live terminal echo is suppressed, not
    /// the capture callers rely on to classify probe results (e.g.
    /// `npm view`'s 404-shaped "not published yet" text).
    #[test]
    fn run_quiet_still_captures_stderr_in_output() {
        let runner = CliCommandRunner;
        let out = runner
            .run_quiet(
                "sh",
                &["-c", "echo captured-probe-text >&2"],
                std::path::Path::new("."),
                Duration::from_secs(5),
            )
            .unwrap();
        assert!(
            out.stderr.contains("captured-probe-text"),
            "run_quiet must still capture stderr, got: {:?}",
            out.stderr
        );
    }

    /// The actual point of `run_quiet`: it must not echo stderr live to the
    /// terminal the way `run_with_timeout` does. Because the live echo goes
    /// via `eprintln!` from inside `run_quiet` itself (not captured in
    /// `CommandOutput`), this re-execs the current test binary as a child
    /// process with `--nocapture` so the real OS-level stderr can be
    /// observed and asserted on -- same pattern as
    /// `run_with_timeout_warns_on_timeout_branch_when_descendant_holds_pipe_open`.
    #[test]
    fn run_quiet_does_not_stream_stderr_live() {
        const CHILD_ENV: &str = "CALLISTO_RUN_QUIET_CHILD";

        if std::env::var(CHILD_ENV).is_ok() {
            let runner = CliCommandRunner;
            let out = runner
                .run_quiet(
                    "sh",
                    &["-c", "echo should-not-appear-live >&2"],
                    std::path::Path::new("."),
                    Duration::from_secs(5),
                )
                .expect("run_quiet must succeed");
            // Positive proof the call actually ran and captured correctly,
            // not just "produced no live output" -- a crashed or no-op
            // child would leave the negative assertion below vacuously
            // true. Printed to the child's own real stdout, entirely
            // separate from anything `run_quiet` itself captures or emits.
            assert!(
                out.stderr.contains("should-not-appear-live"),
                "run_quiet must still capture the text it doesn't stream live"
            );
            println!("CHILD_REACHED_AND_VERIFIED_CAPTURE");
            return;
        }

        let exe = std::env::current_exe().expect("current_exe should be available in tests");
        let output = std::process::Command::new(exe)
            .arg("--exact")
            .arg("runner::tests::run_quiet_does_not_stream_stderr_live")
            .arg("--nocapture")
            .env(CHILD_ENV, "1")
            .output()
            .expect("failed to re-exec test binary");

        assert!(
            output.status.success(),
            "child process must exit successfully, got: {:?}, stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("CHILD_REACHED_AND_VERIFIED_CAPTURE"),
            "child must have actually reached and verified the run_quiet call, not crashed \
             or no-op'd before it; got stdout: {stdout}"
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("should-not-appear-live"),
            "run_quiet must not stream stderr live to the terminal, got: {stderr}"
        );
    }

    /// Sibling regression guard: `run_with_timeout` (the live-streaming
    /// path `run_quiet` is deliberately different from) must still echo
    /// live -- otherwise this pair of tests could both pass by accident if
    /// `stderr_mode` were wired backwards.
    #[test]
    fn run_with_timeout_still_streams_stderr_live() {
        const CHILD_ENV: &str = "CALLISTO_RUN_TIMEOUT_LIVE_CHILD";

        if std::env::var(CHILD_ENV).is_ok() {
            let runner = CliCommandRunner;
            drop(runner.run_with_timeout(
                "sh",
                &["-c", "echo should-appear-live >&2"],
                std::path::Path::new("."),
                Duration::from_secs(5),
            ));
            return;
        }

        let exe = std::env::current_exe().expect("current_exe should be available in tests");
        let output = std::process::Command::new(exe)
            .arg("--exact")
            .arg("runner::tests::run_with_timeout_still_streams_stderr_live")
            .arg("--nocapture")
            .env(CHILD_ENV, "1")
            .output()
            .expect("failed to re-exec test binary");

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("should-appear-live"),
            "run_with_timeout must still stream stderr live, got: {stderr}"
        );
    }
}
