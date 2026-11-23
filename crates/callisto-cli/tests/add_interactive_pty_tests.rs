//! `callisto add`'s wizard driven through a real pseudo-terminal: keys in, rendered output and exit status out.
#![cfg(unix)]

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
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
    screen: Arc<Mutex<String>>,
}

impl Session {
    fn start(root: &Path) -> Self {
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
            .args(["--cwd", root.to_str().unwrap(), "add"])
            .env("TERM", "xterm-256color")
            .stdin(Stdio::from(tty.try_clone().unwrap()))
            .stdout(Stdio::from(tty.try_clone().unwrap()))
            .stderr(Stdio::from(tty))
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
        Session { child, master, screen }
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

    fn exit_code(&mut self) -> Option<i32> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                std::thread::sleep(Duration::from_millis(200));
                return status.code();
            }
            assert!(Instant::now() < deadline, "add did not exit:\n{}", self.screen());
            std::thread::sleep(Duration::from_millis(20));
        }
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
    let screen = add.screen();
    assert!(screen.contains("Cancelled, nothing written."), "{screen}");
    assert!(
        screen.rfind(SHOW_CURSOR) > screen.rfind(HIDE_CURSOR),
        "the cursor is left hidden:\n{screen:?}"
    );
    assert!(changesets(dir.path()).is_empty());
}

#[test]
fn esc_at_a_prompt_writes_nothing_and_exits_0() {
    let dir = workspace();
    let mut add = Session::start(dir.path());
    add.after("Packages in this change", 1, "\r");
    add.after("MAJOR", 1, "\x1b");
    assert_eq!(add.exit_code(), Some(0), "{}", add.screen());
    assert!(add.screen().contains("Cancelled, nothing written."), "{}", add.screen());
    assert!(changesets(dir.path()).is_empty());
}
