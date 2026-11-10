/// Whether stdin is a terminal; `add` and `init` share this one predicate.
pub fn is_interactive() -> bool {
    use std::io::IsTerminal as _;
    std::io::stdin().is_terminal()
}
