/// Returns `true` when stdin is connected to a terminal (i.e. the process is
/// running interactively), `false` otherwise (e.g. stdin is a pipe or
/// redirected file, which is the common case in CI environments).
///
/// This centralises the TTY check so that `add` and `init` both branch on the
/// same predicate rather than each duplicating the `IsTerminal` import and
/// call site. The function is a thin wrapper over
/// [`std::io::IsTerminal::is_terminal`] and carries no additional state; it
/// can be replaced with a test double by injecting a `bool` at the call site
/// when unit testing the *callers*.
///
/// # Why a dedicated module instead of an inline call?
///
/// `std::io::IsTerminal` cannot be meaningfully unit-tested without spawning a
/// real subprocess with a controlled stdin (the trait's return value is
/// determined by the OS, not by Rust code). The integration-test coverage for
/// the non-interactive path lives in
/// `crates/callisto-cli/tests/cli_tests.rs` — specifically
/// `test_add_non_interactive_via_pipe` — which pipes stdin and asserts that
/// `callisto add` selects the non-interactive code path rather than trying to
/// drive a TTY wizard.
pub fn is_interactive() -> bool {
    use std::io::IsTerminal as _;
    std::io::stdin().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the function compiles and returns a `bool`. In a standard test
    /// environment stdin is typically *not* a terminal (tests are invoked by
    /// `cargo test`, which redirects stdin), so the expected value is `false`.
    /// This test is deliberately lightweight: the real coverage is provided by
    /// the binary-level integration test `test_add_non_interactive_via_pipe`.
    #[test]
    fn is_interactive_returns_bool_in_test_environment() {
        // `cargo test` does not allocate a TTY for stdin, so this must be false.
        let result = is_interactive();
        assert!(
            !result,
            "is_interactive() should return false in a non-TTY test environment"
        );
    }
}
