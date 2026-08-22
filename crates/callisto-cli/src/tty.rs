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
