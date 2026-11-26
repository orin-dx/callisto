//! `callisto add`'s wizard driven through a real pseudo-terminal: keys in, rendered output and exit status out.
#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt as _;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use callisto_fixtures::git::{init_repo, run_git};
use rustix::pty::{grantpt, openpt, ptsname, unlockpt, OpenptFlags};

const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";
/// How the answer prompt of a package list reads while it is asked, and once its answer is accepted.
const ANSWER_ASKED: &str = "? Your answer";
const ANSWER_ACCEPTED: &str = "> Your answer";

/// npm packages `a` and `b`, both released at 1.0.0, then `b` changed.
fn workspace() -> tempfile::TempDir {
    released(["a", "b"], "b")
}

/// Two npm packages, both released at 1.0.0, then `changed` changed.
fn released(names: [&str; 2], changed: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
    )
    .unwrap();
    for name in names {
        std::fs::create_dir_all(root.join(format!("packages/{name}"))).unwrap();
        std::fs::write(
            root.join(format!("packages/{name}/package.json")),
            format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
        )
        .unwrap();
    }
    std::fs::write(root.join("callisto.toml"), "").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-qm", "release"]);
    for name in names {
        run_git(root, &["tag", &format!("{name}@1.0.0")]);
    }
    std::fs::write(root.join(format!("packages/{changed}/index.js")), "x").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-qm", "change b"]);
    dir
}

/// `callisto add` on a pty: the child, the master side to type into, and everything it has printed so far.
struct Session {
    child: Child,
    master: std::fs::File,
    /// The child's terminal, kept open to read its settings after the child exits.
    tty: std::fs::File,
    screen: Arc<Mutex<String>>,
}

impl Session {
    fn start(root: &Path) -> Self {
        Self::start_with(root, &["add"], &[])
    }

    /// `callisto <args>` with `env` set; `VISUAL` and `EDITOR` are unset unless `env` names them.
    fn start_with(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Self {
        Self::launch(root, args, env, true, (120, 30))
    }

    /// `callisto <args>` with stdin and stdout on the terminal and stderr piped; read it with [`Session::stderr`].
    fn start_with_piped_stderr(root: &Path, args: &[&str]) -> Self {
        Self::launch(root, args, &[], false, (120, 30))
    }

    fn stderr(&mut self) -> String {
        let mut text = String::new();
        self.child.stderr.take().unwrap().read_to_string(&mut text).unwrap();
        text
    }

    /// `callisto add` on a terminal `columns` wide and `rows` high.
    fn start_small(root: &Path, columns: u16, rows: u16) -> Self {
        Self::launch(root, &["add"], &[], true, (columns, rows))
    }

    fn launch(
        root: &Path,
        args: &[&str],
        env: &[(&str, &str)],
        stderr_on_terminal: bool,
        (columns, rows): (u16, u16),
    ) -> Self {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let name = ptsname(&master, Vec::new()).unwrap();
        let tty = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name.to_str().unwrap())
            .unwrap();
        rustix::termios::tcsetwinsize(
            &tty,
            rustix::termios::Winsize {
                ws_row: rows,
                ws_col: columns,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_callisto"))
            .args(["--cwd", root.to_str().unwrap()])
            .args(args)
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            // The waits below match inquire's coloured rendering, so an inherited colour setting must not change it.
            .env_remove("NO_COLOR")
            .env_remove("CLICOLOR")
            .env_remove("CLICOLOR_FORCE")
            .env("TERM", "xterm-256color")
            .envs(env.iter().copied())
            .stdin(Stdio::from(tty.try_clone().unwrap()))
            .stdout(Stdio::from(tty.try_clone().unwrap()))
            .stderr(if stderr_on_terminal {
                Stdio::from(tty.try_clone().unwrap())
            } else {
                Stdio::piped()
            })
            .spawn()
            .unwrap();
        let master = std::fs::File::from(master);
        let screen = Arc::new(Mutex::new(String::new()));
        let mut reader = master.try_clone().unwrap();
        let sink = Arc::clone(&screen);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            // The read fails with EIO once the child has exited and closed the terminal.
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                sink.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });
        Session {
            child,
            master,
            tty,
            screen,
        }
    }

    fn screen(&self) -> String {
        self.screen.lock().unwrap().clone()
    }

    /// Waits until the output contains `text` `times` times, then types `keys`.
    fn after(&mut self, text: &str, times: usize, keys: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.screen().matches(text).count() < times {
            assert!(Instant::now() < deadline, "never saw {text:?}:\n{}", self.screen());
            std::thread::sleep(Duration::from_millis(20));
        }
        // Let the prompt finish drawing and switch the terminal to raw mode before typing.
        std::thread::sleep(Duration::from_millis(200));
        self.master.write_all(keys.as_bytes()).unwrap();
    }

    /// Types `keys` at the answer prompt of package list number `list` (from 1, over the whole run), once the previous
    /// list's answer was accepted and this prompt is drawn, which is after the terminal is ready for keys.
    fn answer(&mut self, list: usize, keys: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let shown = unescaped(&self.screen());
            let from = match list {
                1 => Some(0),
                _ => shown
                    .match_indices(ANSWER_ACCEPTED)
                    .nth(list - 2)
                    .map(|(at, found)| at + found.len()),
            };
            if from.is_some_and(|from| shown[from..].contains(ANSWER_ASKED)) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "never saw the answer prompt of list {list}:\n{}",
                self.screen()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        self.master.write_all(keys.as_bytes()).unwrap();
    }

    /// Delivers SIGINT to the child itself, as `kill -INT` or a closing terminal would, with no Ctrl-C byte typed.
    fn interrupt(&self) {
        let status = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// The terminal is back in line mode with echo and Ctrl-C signals, as the shell expects after the child exits.
    fn assert_terminal_restored(&self) {
        let out = Command::new("stty")
            .arg("-a")
            .stdin(self.tty.try_clone().unwrap())
            .output()
            .unwrap();
        let settings = String::from_utf8_lossy(&out.stdout);
        let flags: Vec<&str> = settings.split(|c: char| c.is_whitespace() || c == ';').collect();
        assert!(
            flags.contains(&"icanon") && flags.contains(&"echo") && flags.contains(&"isig"),
            "the terminal is left raw:\n{settings}"
        );
    }

    fn exit_code(&mut self) -> Option<i32> {
        self.status().code()
    }

    fn status(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                std::thread::sleep(Duration::from_millis(200));
                return status;
            }
            assert!(Instant::now() < deadline, "add did not exit:\n{}", self.screen());
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Session {
    /// A failed assertion must not leave the child running on its terminal.
    fn drop(&mut self) {
        // Fails only when the child has already exited, which is the goal.
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

fn changesets(root: &Path) -> Vec<String> {
    std::fs::read_dir(root.join(".changeset"))
        .map(|entries| {
            entries
                .map(|e| e.unwrap().path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
                .filter(|path| path.file_name().is_some_and(|name| name != "README.md"))
                .map(|path| std::fs::read_to_string(path).unwrap())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_changed_package_is_marked_so_enter_through_the_wizard_records_it() {
    let dir = workspace();
    let mut add = Session::start(dir.path());
    add.answer(1, "\r");
    add.answer(2, "\r");
    add.answer(3, "\r");
    add.after("Changelog summary", 1, "Fix b\r");
    add.after("Write this changeset?", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    let screen = add.screen();
    assert!(screen.contains("1 * b\r\n  2   a\r\n"), "{screen}");
    assert!(screen.contains("patch  b"), "the review lists the bump:\n{screen}");
    assert!(screen.contains("Next:"), "{screen}");
    assert_eq!(changesets(dir.path()), ["---\nb: patch\n---\n\nFix b\n"]);
}

#[test]
fn packages_and_bumps_are_picked_by_number_range_or_name() {
    let dir = workspace();
    let mut add = Session::start(dir.path());
    // The list is `1 * b`, `2   a`; the bump questions list the picked packages in that order.
    add.answer(1, "1-2\r");
    add.answer(2, "a\r");
    add.answer(3, "1\r");
    add.after("Changelog summary", 1, "Rework\r");
    add.after("Write this changeset?", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    assert_eq!(changesets(dir.path()), ["---\nb: minor\na: major\n---\n\nRework\n"]);
}

#[test]
fn an_answer_that_names_nothing_listed_is_asked_again_with_the_reason() {
    let dir = workspace();
    let mut add = Session::start(dir.path());
    add.answer(1, "c\r");
    add.after("`c` is not a number, a range or a listed name", 1, "\x7f3\r");
    add.after("`3` is not in the list (1-2)", 1, "\x7f2\r");
    add.answer(2, "\r");
    add.answer(3, "\r");
    add.after("Changelog summary", 1, "Fix a\r");
    add.after("Write this changeset?", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    assert_eq!(
        add.screen().matches("Packages in this change").count(),
        1,
        "the list is printed once"
    );
    assert_eq!(changesets(dir.path()), ["---\na: patch\n---\n\nFix a\n"]);
}

#[test]
fn esc_or_ctrl_c_at_a_list_writes_nothing_and_exits_130() {
    // A quick double Esc is two Escape keys, not an arrow's escape sequence.
    for key in ["\x1b", "\x03", "1\x1b", "\x1b\x1b"] {
        let dir = workspace();
        let mut add = Session::start(dir.path());
        add.answer(1, key);
        assert_eq!(add.exit_code(), Some(130), "{key:?}: {}", add.screen());
        let screen = add.screen();
        assert!(
            screen.contains("E343") && screen.contains("Cancelled, nothing written."),
            "{key:?}: {screen}"
        );
        add.assert_terminal_restored();
        assert!(changesets(dir.path()).is_empty());
    }
}

#[test]
fn esc_at_a_prompt_writes_nothing_and_exits_130() {
    let dir = workspace();
    let mut add = Session::start(dir.path());
    add.answer(1, "\r");
    add.answer(2, "\x1b");
    assert_eq!(add.exit_code(), Some(130), "{}", add.screen());
    assert!(add.screen().contains("E343"), "{}", add.screen());
    assert!(add.screen().contains("Cancelled, nothing written."), "{}", add.screen());
    assert!(changesets(dir.path()).is_empty());
}

/// Walks the wizard to the summary question.
fn to_summary(add: &mut Session) {
    add.answer(1, "\r");
    add.answer(2, "\r");
    add.answer(3, "\r");
}

#[test]
fn json_format_never_prompts_even_on_a_terminal() {
    let dir = workspace();
    let mut add = Session::start_with(dir.path(), &["--format", "json", "add"], &[]);
    assert_eq!(add.exit_code(), Some(2), "{}", add.screen());
    let screen = add.screen();
    assert!(
        screen.contains("E214") && screen.contains("missing: --package, --summary"),
        "{screen}"
    );
    assert!(!screen.contains("Packages in this change"), "{screen}");
    assert!(changesets(dir.path()).is_empty());
}

#[test]
fn a_piped_stderr_never_prompts_even_with_a_terminal_on_stdin() {
    let dir = workspace();
    let mut add = Session::start_with_piped_stderr(dir.path(), &["add"]);
    assert_eq!(add.exit_code(), Some(2), "{}", add.screen());
    let stderr = add.stderr();
    assert!(stderr.contains("E214") && stderr.contains("--package"), "{stderr}");
    assert!(!add.screen().contains("Packages in this change"), "{}", add.screen());
    assert!(changesets(dir.path()).is_empty());
}

#[test]
fn an_editor_that_saves_nothing_cancels_and_exits_0() {
    let dir = workspace();
    let editor = r#"sh -c ': > "$0"'"#;
    let mut add = Session::start_with(dir.path(), &["add"], &[("EDITOR", editor)]);
    to_summary(&mut add);
    add.after("Changelog summary", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    assert!(add.screen().contains("Cancelled, nothing written."), "{}", add.screen());
    assert!(changesets(dir.path()).is_empty());
}

#[test]
fn an_editor_that_cannot_start_warns_and_asks_for_the_summary_inline() {
    let dir = workspace();
    let mut add = Session::start_with(dir.path(), &["add"], &[("EDITOR", "callisto-no-such-editor --wait")]);
    to_summary(&mut add);
    add.after("Changelog summary", 1, "\r");
    add.after("summary (Esc cancels)", 1, "Fix b\r");
    add.after("Write this changeset?", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    let screen = add.screen();
    assert!(
        screen.contains("warning: cannot use $EDITOR (`callisto-no-such-editor --wait`)"),
        "{screen}"
    );
    assert_eq!(changesets(dir.path()), ["---\nb: patch\n---\n\nFix b\n"]);
}

#[test]
fn json_format_refuses_to_prompt_before_checking_the_summary() {
    let dir = workspace();
    let mut add = Session::start_with(dir.path(), &["--format", "json", "add", "--summary", ""], &[]);
    assert_eq!(add.exit_code(), Some(2), "{}", add.screen());
    let screen = add.screen();
    assert!(screen.contains("E214") && screen.contains("--package"), "{screen}");
    assert!(changesets(dir.path()).is_empty());
}

#[test]
fn ctrl_c_at_the_text_input_restores_the_cursor_writes_nothing_and_exits_130() {
    let dir = workspace();
    let mut add = Session::start(dir.path());
    to_summary(&mut add);
    add.after("Changelog summary", 1, "\x03");
    assert_eq!(add.exit_code(), Some(130), "{}", add.screen());
    assert!(add.screen().contains("E343"), "{}", add.screen());
    let screen = add.screen();
    assert!(screen.contains("Cancelled, nothing written."), "{screen}");
    assert!(
        screen.rfind(SHOW_CURSOR) > screen.rfind(HIDE_CURSOR),
        "the cursor is left hidden:\n{screen:?}"
    );
    add.assert_terminal_restored();
    assert!(changesets(dir.path()).is_empty());
}

#[test]
fn an_empty_editor_setting_is_unset_and_the_summary_is_asked_inline() {
    let dir = workspace();
    let mut add = Session::start_with(dir.path(), &["add"], &[("VISUAL", ""), ("EDITOR", "")]);
    to_summary(&mut add);
    // An empty answer re-asks instead of opening an editor.
    add.after("Changelog summary", 1, "\r");
    add.after("summary (Esc cancels)", 1, "Fix b\r");
    add.after("Write this changeset?", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    assert_eq!(changesets(dir.path()), ["---\nb: patch\n---\n\nFix b\n"]);
}

#[test]
fn an_editor_that_exits_non_zero_cancels_even_after_saving() {
    let dir = workspace();
    let editor = r#"sh -c 'echo draft > "$0"; exit 1'"#;
    let mut add = Session::start_with(dir.path(), &["add"], &[("EDITOR", editor)]);
    to_summary(&mut add);
    add.after("Changelog summary", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    let screen = add.screen();
    assert!(screen.contains("Cancelled, nothing written."), "{screen}");
    assert!(!screen.contains("Write this changeset?"), "{screen}");
    assert!(changesets(dir.path()).is_empty());
}

/// Prompts read Ctrl-C as a key; a SIGINT from outside (`kill -INT`) ends the process by default, writing nothing.
#[test]
fn a_sigint_delivered_to_the_process_at_a_prompt_ends_it_and_writes_nothing() {
    let dir = workspace();
    let mut add = Session::start(dir.path());
    to_summary(&mut add);
    add.after("Changelog summary", 1, "");
    add.interrupt();
    assert_eq!(add.status().signal(), Some(2), "{}", add.screen());
    assert!(changesets(dir.path()).is_empty());
}

#[test]
fn a_sigint_delivered_to_the_process_at_a_list_ends_it_and_writes_nothing() {
    let dir = workspace();
    let mut add = Session::start(dir.path());
    add.answer(1, "");
    add.interrupt();
    assert_eq!(add.status().signal(), Some(2), "{}", add.screen());
    assert!(changesets(dir.path()).is_empty());
}

#[test]
fn whitespace_at_the_inline_summary_prompt_asks_again() {
    let dir = workspace();
    let mut add = Session::start_with(dir.path(), &["add"], &[("VISUAL", ""), ("EDITOR", "")]);
    to_summary(&mut add);
    add.after("Changelog summary", 1, "   \r");
    add.after("the summary cannot be blank", 1, "Fix b\r");
    add.after("Write this changeset?", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    assert_eq!(changesets(dir.path()), ["---\nb: patch\n---\n\nFix b\n"]);
}

#[test]
fn a_blank_summary_flag_fails_before_the_review_and_writes_nothing() {
    for blank in ["", "  \t "] {
        let dir = workspace();
        let mut add = Session::start_with(dir.path(), &["add", "--summary", blank], &[]);
        assert_eq!(add.exit_code(), Some(2), "{}", add.screen());
        assert!(!add.screen().contains("Packages in this change"), "{}", add.screen());
        assert!(changesets(dir.path()).is_empty());
    }
}

#[test]
fn the_editor_does_not_see_registry_tokens() {
    let dir = workspace();
    let editor = r#"sh -c 'printf "%s" "${CARGO_REGISTRIES_PRIVATE_TOKEN:-no token}" > "$0"'"#;
    let mut add = Session::start_with(
        dir.path(),
        &["add"],
        &[("EDITOR", editor), ("CARGO_REGISTRIES_PRIVATE_TOKEN", "secret")],
    );
    to_summary(&mut add);
    add.after("Changelog summary", 1, "\r");
    add.after("Write this changeset?", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    assert_eq!(changesets(dir.path()), ["---\nb: patch\n---\n\nno token\n"]);
}

/// Ctrl-C in a line-mode editor reaches Callisto too; as git does, the editor's exit status decides.
#[test]
fn a_sigint_while_the_editor_runs_is_left_to_the_editor() {
    let dir = workspace();
    let editor = r#"sh -c 'kill -INT $PPID; sleep 0.3; printf "Fix b" > "$0"'"#;
    let mut add = Session::start_with(dir.path(), &["add"], &[("EDITOR", editor)]);
    to_summary(&mut add);
    add.after("Changelog summary", 1, "\r");
    add.after("Write this changeset?", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    assert_eq!(changesets(dir.path()), ["---\nb: patch\n---\n\nFix b\n"]);
}

#[test]
fn after_the_editor_a_sigint_takes_its_default_action_again() {
    let dir = workspace();
    let editor = r#"sh -c 'printf "Fix b" > "$0"'"#;
    let mut add = Session::start_with(dir.path(), &["add"], &[("EDITOR", editor)]);
    to_summary(&mut add);
    add.after("Changelog summary", 1, "\r");
    add.after("Write this changeset?", 1, "");
    add.interrupt();
    assert_eq!(add.status().signal(), Some(2), "{}", add.screen());
    assert!(changesets(dir.path()).is_empty());
}

#[test]
fn declining_the_review_writes_nothing_and_exits_0() {
    let dir = workspace();
    let mut add = Session::start(dir.path());
    to_summary(&mut add);
    add.after("Changelog summary", 1, "Fix b\r");
    add.after("Write this changeset?", 1, "n\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    assert!(add.screen().contains("Cancelled, nothing written."), "{}", add.screen());
    assert!(changesets(dir.path()).is_empty());
}

#[test]
fn an_editor_that_cannot_be_used_warns_once_and_asks_inline_whatever_the_reason() {
    for (editor, reason) in [
        (r#"sh -c 'printf "\377" > "$0"'"#, "not UTF-8"),
        ("\"unterminated", "split into words"),
    ] {
        let dir = workspace();
        let mut add = Session::start_with(dir.path(), &["add"], &[("EDITOR", editor)]);
        to_summary(&mut add);
        add.after("Changelog summary", 1, "\r");
        add.after("summary (Esc cancels)", 1, "Fix b\r");
        add.after("Write this changeset?", 1, "\r");
        assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
        let screen = add.screen();
        assert_eq!(screen.matches("warning: cannot use $EDITOR").count(), 1, "{screen}");
        assert!(screen.contains(reason), "{screen}");
        assert_eq!(changesets(dir.path()), ["---\nb: patch\n---\n\nFix b\n"]);
    }
}

#[test]
fn declining_the_init_confirmation_writes_nothing_and_exits_0() {
    let dir = workspace();
    std::fs::remove_file(dir.path().join("callisto.toml")).unwrap();
    let mut init = Session::start_with(dir.path(), &["init"], &[]);
    init.after("How should packages be versioned?", 1, "\r");
    init.after("Write callisto.toml?", 1, "n\r");
    assert_eq!(init.exit_code(), Some(0), "{}", init.screen());
    assert!(
        init.screen().contains("Cancelled, nothing written."),
        "{}",
        init.screen()
    );
    assert!(!dir.path().join("callisto.toml").exists());
}

#[test]
fn json_format_never_prompts_init_even_on_a_terminal() {
    let dir = workspace();
    std::fs::remove_file(dir.path().join("callisto.toml")).unwrap();
    let mut init = Session::start_with(dir.path(), &["--format", "json", "init"], &[]);
    assert_eq!(init.exit_code(), Some(2), "{}", init.screen());
    assert!(
        init.screen().contains("E240") && init.screen().contains("--yes"),
        "{}",
        init.screen()
    );
    assert!(
        !init.screen().contains("How should packages be versioned?"),
        "{}",
        init.screen()
    );
    assert!(!dir.path().join("callisto.toml").exists());
}

#[test]
fn esc_at_an_init_prompt_writes_nothing_and_exits_130() {
    let dir = workspace();
    std::fs::remove_file(dir.path().join("callisto.toml")).unwrap();
    let mut init = Session::start_with(dir.path(), &["init"], &[]);
    init.after("How should packages be versioned?", 1, "\x1b");
    assert_eq!(init.exit_code(), Some(130), "{}", init.screen());
    assert!(init.screen().contains("E343"), "{}", init.screen());
    assert!(
        init.screen().contains("Cancelled, nothing written."),
        "{}",
        init.screen()
    );
    init.assert_terminal_restored();
    assert!(!dir.path().join("callisto.toml").exists());
}

/// Two npm packages, never released, whose names differ only in their last word.
fn long_names(names: [&str; 2]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
    )
    .unwrap();
    for (n, name) in names.iter().enumerate() {
        std::fs::create_dir_all(root.join(format!("packages/p{n}"))).unwrap();
        std::fs::write(
            root.join(format!("packages/p{n}/package.json")),
            format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
        )
        .unwrap();
    }
    std::fs::write(root.join("callisto.toml"), "").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-qm", "init"]);
    dir
}

/// The text a terminal shows, without escape sequences or line breaks, so a wrapped line reads as one.
fn plain(screen: &str) -> String {
    unescaped(screen).replace(['\r', '\n'], "")
}

/// The text a terminal shows, without escape sequences.
fn unescaped(screen: &str) -> String {
    let mut text = String::new();
    let mut chars = screen.chars();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                if chars.next() == Some('[') {
                    // A control sequence ends at its first byte in `@`..=`~`.
                    chars.find(|c| ('@'..='~').contains(c));
                }
            }
            c => text.push(c),
        }
    }
    text
}

/// The lists are printed once and never redrawn, so a pane smaller than they are cannot misdraw them.
#[test]
fn in_a_40_by_10_pane_long_names_are_shown_in_full_and_picked_by_name() {
    let names = [
        "callisto-fixture-package-for-wrapping-left",
        "callisto-fixture-package-for-wrapping-right",
    ];
    let dir = long_names(names);
    let mut add = Session::start_small(dir.path(), 40, 10);
    add.answer(1, &format!("{}\r", names[1]));
    add.answer(2, "\r");
    add.answer(3, "\r");
    add.after("Changelog summary", 1, "Fix\r");
    add.after("Write this changeset?", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    let screen = add.screen();
    // The first escape sequence is the answer prompt's, after the first list is printed whole.
    let lists = &screen[..screen.find('\x1b').unwrap()];
    assert!(
        lists.ends_with(
            "  1 * callisto-fixture-package-for-wrapping-left\r\n  2 * callisto-fixture-package-for-wrapping-right\r\nEnter keeps the * ones, or type numbers, ranges or names. Esc cancels.\r\n"
        ),
        "the list moves the cursor or restyles:\n{screen:?}"
    );
    let shown = plain(lists);
    for name in names {
        assert!(shown.contains(name), "{name} is not shown in full:\n{shown}");
    }
    assert_eq!(
        changesets(dir.path()),
        [format!("---\n{}: patch\n---\n\nFix\n", names[1])]
    );
}

/// With nothing marked, the first list does not offer Enter (it needs a pick); the bump lists do.
#[test]
fn with_nothing_marked_only_the_bump_lists_offer_enter_for_none() {
    let dir = workspace();
    run_git(dir.path(), &["tag", "-f", "b@1.0.0"]);
    let mut add = Session::start(dir.path());
    add.answer(1, "a\r");
    add.answer(2, "\x03");
    assert_eq!(add.exit_code(), Some(130), "{}", add.screen());
    let screen = plain(&add.screen());
    let (first, bumps) = screen
        .split_once("Which are a MAJOR bump?")
        .expect("the bump list is asked");
    assert!(first.contains("Type numbers, ranges or names. Esc cancels."), "{first}");
    assert!(!first.contains("Enter picks none"), "{first}");
    assert!(bumps.contains("Enter picks none, or type numbers"), "{bumps}");
}

#[test]
fn enter_with_nothing_picked_at_the_first_list_is_the_no_packages_error() {
    let dir = workspace();
    run_git(dir.path(), &["tag", "-f", "b@1.0.0"]);
    let mut add = Session::start(dir.path());
    add.answer(1, "\r");
    assert_eq!(add.exit_code(), Some(1), "{}", add.screen());
    assert!(add.screen().contains("E273"), "{}", add.screen());
    assert!(changesets(dir.path()).is_empty());
}

/// Packages named `1` and `2`, with `2` changed so it is listed first: typing `1` picks the package named `1`, not
/// row 1, because an exact name wins over a row number.
#[test]
fn an_exact_name_wins_over_a_row_number() {
    let dir = released(["1", "2"], "2");
    let mut add = Session::start(dir.path());
    add.answer(1, "1\r");
    add.answer(2, "\r");
    add.answer(3, "\r");
    add.after("Changelog summary", 1, "Fix\r");
    add.after("Write this changeset?", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    let screen = add.screen();
    assert!(screen.contains("1 * 2\r\n  2   1\r\n"), "{screen}");
    assert_eq!(changesets(dir.path()), ["---\n1: patch\n---\n\nFix\n"]);
}

/// A package named `42` in a workspace of one: no row 42 exists, so typing `42` names the package.
#[test]
fn a_package_named_with_digits_is_picked_by_name_when_that_row_does_not_exist() {
    let dir = long_names(["42", "other"]);
    std::fs::remove_dir_all(dir.path().join("packages/p1")).unwrap();
    run_git(dir.path(), &["add", "-A"]);
    run_git(dir.path(), &["commit", "-qm", "one package"]);
    let mut add = Session::start(dir.path());
    add.answer(1, "42\r");
    add.answer(2, "\r");
    add.answer(3, "\r");
    add.after("Changelog summary", 1, "Fix\r");
    add.after("Write this changeset?", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    assert_eq!(changesets(dir.path()), ["---\n42: patch\n---\n\nFix\n"]);
}

#[test]
fn the_wizard_runs_on_a_dumb_terminal() {
    let dir = workspace();
    let mut add = Session::start_with(dir.path(), &["add"], &[("TERM", "dumb")]);
    to_summary(&mut add);
    add.after("Changelog summary", 1, "Fix b\r");
    add.after("Write this changeset?", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    assert_eq!(changesets(dir.path()), ["---\nb: patch\n---\n\nFix b\n"]);
}

/// A Cargo workspace with binaries `app` and `tool` and a GitHub `origin`, so `init` asks which is the product.
fn two_binaries() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\", \"tool\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    for member in ["app", "tool"] {
        std::fs::create_dir_all(root.join(member).join("src")).unwrap();
        std::fs::write(
            root.join(member).join("Cargo.toml"),
            format!("[package]\nname = \"{member}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        std::fs::write(root.join(member).join("src/main.rs"), "fn main() {}\n").unwrap();
    }
    init_repo(root);
    run_git(
        root,
        &["remote", "add", "origin", "https://github.com/example/tools.git"],
    );
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-qm", "init"]);
    dir
}

/// Runs `init` up to the product question, where `answers` are typed in turn, then to the end; returns `callisto.toml`.
fn init_with_product_answers(answers: impl FnOnce(&mut Session)) -> String {
    let dir = two_binaries();
    let mut init = Session::start_with(dir.path(), &["init"], &[]);
    init.after("How should packages be versioned?", 1, "\r");
    init.after("Ship binaries", 1, "y\r");
    answers(&mut init);
    init.after("Artifact target triples", 1, "x86_64-unknown-linux-gnu\r");
    init.after("Generate a GitHub Actions release workflow?", 1, "n\r");
    init.after("Write callisto.toml?", 1, "\r");
    assert_eq!(init.exit_code(), Some(0), "{}", init.screen());
    std::fs::read_to_string(dir.path().join("callisto.toml")).unwrap()
}

#[test]
fn init_asks_again_until_exactly_one_product_package_is_named() {
    let written = init_with_product_answers(|init| {
        // The prompt does not repeat a reason it is already showing, so each refusal differs from the one before.
        init.answer(1, "\r");
        init.after("pick exactly one", 1, "x\r");
        init.after("`x` is not a number, a range or a listed name", 1, "\x7f1 2\r");
        init.after("pick exactly one", 2, "\x7f\x7f\x7fcargo/tool\r");
    });
    assert!(written.contains("product-package = \"cargo/tool\""), "{written}");
}

#[test]
fn init_picks_the_product_package_by_number() {
    let written = init_with_product_answers(|init| init.answer(1, "2\r"));
    assert!(written.contains("product-package = \"cargo/tool\""), "{written}");
}

#[test]
fn init_picks_the_product_package_by_its_bare_name_as_the_flag_does() {
    let written = init_with_product_answers(|init| init.answer(1, "tool\r"));
    assert!(written.contains("product-package = \"cargo/tool\""), "{written}");
}

/// One never-released npm package named `name`, alone in its workspace.
fn single(name: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let manifest = serde_json::json!({ "name": name, "version": "1.0.0" });
    std::fs::write(dir.path().join("package.json"), manifest.to_string()).unwrap();
    std::fs::write(dir.path().join("callisto.toml"), "").unwrap();
    dir
}

/// Picks the only package, no major or minor bump, and gives the summary `Fix`.
fn pick_the_only_package_as_a_patch(add: &mut Session) {
    add.answer(1, "1\r");
    add.answer(2, "\r");
    add.answer(3, "\r");
    add.after("Changelog summary", 1, "Fix\r");
}

#[test]
fn a_name_a_changeset_cannot_hold_is_refused_before_the_review() {
    for args in [&["add"][..], &["add", "--dry-run"]] {
        let dir = single("bad\nname");
        let mut add = Session::start_with(dir.path(), args, &[]);
        pick_the_only_package_as_a_patch(&mut add);
        assert_ne!(add.exit_code(), Some(0), "{args:?}: {}", add.screen());
        let screen = add.screen();
        assert!(screen.contains("E268"), "{args:?}: {screen}");
        assert!(
            !screen.contains("This changeset records") && !screen.contains("Write this changeset?"),
            "{args:?}: nothing is reviewed or asked:\n{screen}"
        );
        assert!(changesets(dir.path()).is_empty(), "{args:?}: nothing is written");
    }
}

#[test]
fn a_name_with_a_leading_tab_is_written_so_status_reads_it_back() {
    let dir = single("\tlead");
    let mut add = Session::start(dir.path());
    pick_the_only_package_as_a_patch(&mut add);
    add.after("Write this changeset?", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    let status = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--cwd", dir.path().to_str().unwrap(), "status", "--format", "json"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(report["pending"], 1, "{:?}: {report}", changesets(dir.path()));
    assert!(report.get("diagnostics").is_none(), "{report}");
}
