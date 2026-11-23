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

/// npm packages `a` and `b`, both released at 1.0.0, then `b` changed.
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
    )
    .unwrap();
    for name in ["a", "b"] {
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
    run_git(root, &["tag", "a@1.0.0"]);
    run_git(root, &["tag", "b@1.0.0"]);
    std::fs::write(root.join("packages/b/index.js"), "x").unwrap();
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
        Self::launch(root, args, env, true)
    }

    /// `callisto <args>` with stdin and stdout on the terminal and stderr piped; read it with [`Session::stderr`].
    fn start_with_piped_stderr(root: &Path, args: &[&str]) -> Self {
        Self::launch(root, args, &[], false)
    }

    fn stderr(&mut self) -> String {
        let mut text = String::new();
        self.child.stderr.take().unwrap().read_to_string(&mut text).unwrap();
        text
    }

    fn launch(root: &Path, args: &[&str], env: &[(&str, &str)], stderr_on_terminal: bool) -> Self {
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
                ws_row: 30,
                ws_col: 120,
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
            .envs(env.iter().copied())
            .env("TERM", "xterm-256color")
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

    /// Delivers SIGINT to the child itself, as `kill -INT` or a closing terminal would, with no Ctrl-C byte typed.
    fn interrupt(&self) {
        let status = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// The terminal is back in line mode with echo, as the shell expects after the child exits.
    fn assert_terminal_restored(&self) {
        let out = Command::new("stty")
            .arg("-a")
            .stdin(self.tty.try_clone().unwrap())
            .output()
            .unwrap();
        let settings = String::from_utf8_lossy(&out.stdout);
        let flags: Vec<&str> = settings.split(|c: char| c.is_whitespace() || c == ';').collect();
        assert!(
            flags.contains(&"icanon") && flags.contains(&"echo"),
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
fn the_changed_package_is_ticked_so_enter_through_the_wizard_records_it() {
    let dir = workspace();
    let mut add = Session::start(dir.path());
    add.after("Packages in this change", 1, "\r");
    add.after("MAJOR", 1, "\r");
    add.after("MINOR", 1, "\r");
    add.after("Changelog summary", 1, "Fix b\r");
    add.after("Write this changeset?", 1, "\r");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    let screen = add.screen();
    assert!(screen.contains("b (changed)"), "{screen}");
    assert!(screen.contains("patch  b"), "the review lists the bump:\n{screen}");
    assert!(screen.contains("Next:"), "{screen}");
    assert_eq!(changesets(dir.path()), ["---\nb: patch\n---\n\nFix b\n"]);
}

#[test]
fn ctrl_c_at_a_prompt_restores_the_cursor_writes_nothing_and_exits_130() {
    let dir = workspace();
    let mut add = Session::start(dir.path());
    add.after("Packages in this change", 1, "\x03");
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
fn esc_at_a_prompt_writes_nothing_and_exits_130() {
    let dir = workspace();
    let mut add = Session::start(dir.path());
    add.after("Packages in this change", 1, "\r");
    add.after("MAJOR", 1, "\x1b");
    assert_eq!(add.exit_code(), Some(130), "{}", add.screen());
    assert!(add.screen().contains("E343"), "{}", add.screen());
    assert!(add.screen().contains("Cancelled, nothing written."), "{}", add.screen());
    assert!(changesets(dir.path()).is_empty());
}

/// Walks the wizard to the summary question.
fn to_summary(add: &mut Session) {
    add.after("Packages in this change", 1, "\r");
    add.after("MAJOR", 1, "\r");
    add.after("MINOR", 1, "\r");
}

#[test]
fn json_format_never_prompts_even_on_a_terminal() {
    let dir = workspace();
    let mut add = Session::start_with(dir.path(), &["--format", "json", "add"], &[]);
    assert_eq!(add.exit_code(), Some(2), "{}", add.screen());
    let screen = add.screen();
    assert!(screen.contains("E214") && screen.contains("--package"), "{screen}");
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
    add.after("Changelog summary", 2, "Fix b\r");
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
    add.after("Changelog summary", 2, "Fix b\r");
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
fn whitespace_at_the_inline_summary_prompt_asks_again() {
    let dir = workspace();
    let mut add = Session::start_with(dir.path(), &["add"], &[("VISUAL", ""), ("EDITOR", "")]);
    to_summary(&mut add);
    add.after("Changelog summary", 1, "   \r");
    add.after("Changelog summary", 2, "Fix b\r");
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
        add.after("Changelog summary", 2, "Fix b\r");
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
