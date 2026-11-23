use std::sync::atomic::{AtomicU8, Ordering};

/// Whether stdin is a terminal; `add` and `init` share this one predicate.
pub fn is_interactive() -> bool {
    use std::io::IsTerminal as _;
    std::io::stdin().is_terminal()
}

/// Exit status for Ctrl-C, as shells report SIGINT.
pub const CANCELLED_BY_SIGINT: i32 = 130;

/// What owns the terminal when Ctrl-C arrives.
const ELSEWHERE: u8 = 0;
const PROMPT: u8 = 1;
/// `$EDITOR` shares our process group, so its Ctrl-C reaches us too.
const EDITOR: u8 = 2;

static AT: AtomicU8 = AtomicU8::new(ELSEWHERE);

/// Runs `ask` so that Ctrl-C at any of its prompts ends like Esc: the cursor dialoguer hid is shown, nothing is written,
/// and the exit status is 130. Outside prompts Ctrl-C exits 130 as before; inside `$EDITOR` it is left to the editor.
pub fn prompting<T>(ask: impl FnOnce() -> T) -> T {
    // Without the handler Ctrl-C still exits, only with the cursor hidden, so a failure to install it is not fatal.
    drop(ctrlc::set_handler(|| match AT.load(Ordering::SeqCst) {
        EDITOR => {}
        PROMPT => {
            drop(dialoguer::console::Term::stderr().show_cursor());
            crate::output::prompt_line("\nCancelled, nothing written.");
            std::process::exit(CANCELLED_BY_SIGINT);
        }
        _ => std::process::exit(CANCELLED_BY_SIGINT),
    }));
    in_state(PROMPT, ask)
}

/// Runs `edit` with Ctrl-C left to the editor.
pub fn with_editor<T>(edit: impl FnOnce() -> T) -> T {
    in_state(EDITOR, edit)
}

fn in_state<T>(state: u8, run: impl FnOnce() -> T) -> T {
    let before = AT.swap(state, Ordering::SeqCst);
    let result = run();
    AT.store(before, Ordering::SeqCst);
    result
}
