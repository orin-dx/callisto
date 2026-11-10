use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

/// Prints `message` to stderr the first time this process sees it; returns whether it printed.
pub(crate) fn note_once(message: String) -> bool {
    // A command can load the workspace several times; the note describes the workspace, not the load.
    static SEEN: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    let first = SEEN
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(message.clone());
    if first {
        eprintln!("{message}");
    }
    first
}

#[cfg(test)]
mod tests {
    use super::note_once;

    #[test]
    fn a_message_prints_once_and_a_different_one_prints_again() {
        assert!(note_once("note: unit-test message A".into()));
        assert!(!note_once("note: unit-test message A".into()));
        assert!(note_once("note: unit-test message B".into()));
    }
}
