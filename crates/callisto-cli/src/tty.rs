/// Returns `true` when stdin is a terminal (interactive), `false` otherwise
/// (e.g. a pipe or redirected file -- the common CI case).
///
/// Centralizes the TTY check so `add` and `init` share one predicate
/// instead of each duplicating `IsTerminal`. Thin wrapper over
/// [`std::io::IsTerminal::is_terminal`], no state -- callers inject a
/// `bool` test double rather than testing this directly, since the OS
/// determines the trait's return value, not Rust code. Non-interactive-path
/// coverage lives in `cli_tests.rs`'s `test_add_non_interactive_via_pipe`
/// (pipes stdin, asserts `callisto add` skips the TTY wizard).
pub fn is_interactive() -> bool {
    use std::io::IsTerminal as _;
    std::io::stdin().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-DX-STATUS-ADD AC-07: `add` must enter the wizard whenever stdin is
    // a TTY, regardless of stdout -- guards against a future edit growing a
    // stdout TTY check into this function. There is no pty harness in this
    // crate's dev-dependencies to fake a real stdin TTY in-process, so this
    // pins the source text instead of driving a real subprocess;
    // `cli_tests.rs`'s `test_add_non_interactive_via_pipe` covers the
    // reachable negative case (piped stdin -> NotATty).
    #[test]
    fn is_interactive_checks_stdin_only_not_stdout() {
        let needle = ["stdout", "()", ".", "is_terminal"].concat();
        let source = include_str!("tty.rs");
        assert!(
            !source.contains(&needle),
            "is_interactive must never gate on stdout's TTY-ness"
        );
        // Smoke-test the call succeeds in whatever TTY state the test runner has.
        let _: bool = is_interactive();
    }
}
