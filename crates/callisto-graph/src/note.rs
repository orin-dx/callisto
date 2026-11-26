use std::collections::BTreeSet;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

/// Writes advisory text to `err` as given: a note, a warning, an error card or a child's echoed output. Never a
/// report, a prompt or text a decision depends on. Best effort: a closed or failing stream cannot change the result.
pub fn advise(err: &mut dyn Write, text: impl std::fmt::Display) {
    // The result is already decided; a lost advisory line cannot change it.
    drop(write!(err, "{text}"));
}

/// Advises `message` on stderr the first time this process sees it; returns whether it was new.
pub(crate) fn note_once(message: String) -> bool {
    // A command can load the workspace several times; the note describes the workspace, not the load.
    static SEEN: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    let first = SEEN
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(message.clone());
    if first {
        advise(&mut std::io::stderr(), format_args!("{message}\n"));
    }
    first
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::{advise, note_once};

    #[test]
    fn a_message_prints_once_and_a_different_one_prints_again() {
        assert!(note_once("note: unit-test message A".into()));
        assert!(!note_once("note: unit-test message A".into()));
        assert!(note_once("note: unit-test message B".into()));
    }

    /// `advise` writes its text as given, and returns nothing a caller could propagate when the stream fails.
    #[test]
    fn advise_writes_as_given_and_ignores_a_failing_stream() {
        struct Closed;
        impl io::Write for Closed {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }

        let mut written = Vec::new();
        advise(&mut written, format_args!("note: {}\n", 1));
        assert_eq!(written, b"note: 1\n");
        advise(&mut Closed, "lost");
    }
}
