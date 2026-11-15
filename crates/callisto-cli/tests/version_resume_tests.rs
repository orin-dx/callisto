//! An interrupted `version` run resumes from its checkpoint (ADR 13): resuming leaves exactly what an uninterrupted run
//! leaves, and a run whose inputs changed is refused with steps that undo it.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Lockfile tools that succeed without touching anything, except the one named by `FAIL_PROG`, which fails once per
/// `FAIL_FLAG` file. Cargo commands other than `update` run the real cargo, which workspace loading needs.
fn fake_tools(dir: &Path) -> PathBuf {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    for prog in ["cargo", "npm", "uv", "yarn", "pnpm", "bun"] {
        let script = format!(
            "#!/bin/sh\nif [ \"$FAIL_PROG\" = {prog} ] && [ ! -e \"$FAIL_FLAG\" ]; then touch \"$FAIL_FLAG\"; \
             echo simulated failure >&2; exit 1; fi\nif [ {prog} = cargo ] && [ \"$1\" != update ]; then exec \"{cargo}\" \
             \"$@\"; fi\nexit 0\n"
        );
        let path = bin.join(prog);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git").args(args).current_dir(root).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn version(workspace: &Path, bin: &Path, fail: Option<(&str, &Path)>) -> Output {
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let mut command = Command::new(env!("CARGO_BIN_EXE_callisto"));
    command
        .args(["--cwd", workspace.to_str().unwrap(), "version"])
        .env("PATH", path);
    if let Some((prog, flag)) = fail {
        command.env("FAIL_PROG", prog).env("FAIL_FLAG", flag);
    }
    command.output().unwrap()
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn crate_at(root: &Path, dir: &str, version: &str, deps: &str) {
    let name = Path::new(dir).file_name().unwrap().to_str().unwrap();
    write(
        root,
        &format!("{dir}/Cargo.toml"),
        &format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n{deps}"),
    );
    write(root, &format!("{dir}/src/lib.rs"), "");
}

/// A Cargo crate `a` with a dependent `b` and an existing changelog, and a minor changeset on `a`.
fn cargo_cascade(ws: &Path) {
    write(
        ws,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    );
    crate_at(ws, "crates/a", "1.0.0", "");
    crate_at(
        ws,
        "crates/b",
        "1.0.0",
        "[dependencies]\na = { path = \"../a\", version = \"1.0.0\" }\n",
    );
    write(ws, "crates/b/CHANGELOG.md", "# b\n\n## 1.0.0\n\n- Initial\n");
    write(ws, "Cargo.lock", "");
    write(ws, "callisto.toml", "");
    write(ws, ".changeset/one.md", "---\n\"a\": minor\n---\n\nAdd a thing\n");
}

/// Cargo and npm in one workspace: npm's refresh fails after Cargo's, and an npm dependent's spec was rewritten first.
fn cargo_and_npm(ws: &Path) {
    write(
        ws,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    );
    crate_at(ws, "crates/core", "0.3.0", "");
    write(ws, "Cargo.lock", "");
    write(ws, "callisto.toml", "");
    write(
        ws,
        "package.json",
        "{\"name\":\"root\",\"private\":true,\"workspaces\":[\"js/*\"]}\n",
    );
    write(ws, "js/ui/package.json", "{\"name\":\"ui\",\"version\":\"1.0.0\"}\n");
    write(
        ws,
        "js/app/package.json",
        "{\"name\":\"app\",\"version\":\"1.0.0\",\"dependencies\":{\"ui\":\"^1.0.0\"}}\n",
    );
    write(ws, "package-lock.json", "{}\n");
    write(
        ws,
        ".changeset/one.md",
        "---\n\"core\": minor\n\"ui\": major\n---\n\nBoth\n",
    );
}

/// A fixed group whose members inherit their version, in pre mode.
fn pre_mode_group(ws: &Path) {
    write(
        ws,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n[workspace.package]\nversion = \"2.0.0\"\n",
    );
    for name in ["a", "b"] {
        write(
            ws,
            &format!("crates/{name}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion.workspace = true\nedition = \"2021\"\n"),
        );
        write(ws, &format!("crates/{name}/src/lib.rs"), "");
    }
    write(ws, "Cargo.lock", "");
    write(
        ws,
        "callisto.toml",
        "[[fixed-group]]\nname = \"all\"\nmembers = [\"cargo/a\", \"cargo/b\"]\n",
    );
    write(ws, ".changeset/one.md", "---\n\"b\": patch\n---\n\nFix b\n");
    write(ws, ".changeset/pre.json", "{\n  \"mode\": \"pre\",\n  \"tag\": \"beta\",\n  \"initialVersions\": {\n    \"a\": \"2.0.0\",\n    \"b\": \"2.0.0\"\n  },\n  \"changesets\": []\n}\n");
}

/// Writes a workspace's files into the directory it is given.
type Shape = fn(&Path);

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    workspace: PathBuf,
}

/// A committed repository built by `shape` in `subdir` of it; `uncommitted` stays out of the first commit.
fn repo(shape: Shape, subdir: &str, uncommitted: Option<&str>) -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    let workspace = if subdir.is_empty() {
        root.clone()
    } else {
        root.join(subdir)
    };
    fs::create_dir_all(&workspace).unwrap();
    callisto_fixtures::git::init_repo(&root);
    shape(&workspace);
    let held = uncommitted.map(|path| (workspace.join(path), fs::read(workspace.join(path)).unwrap()));
    if let Some((path, _)) = &held {
        fs::remove_file(path).unwrap();
    }
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "init"]);
    if let Some((path, bytes)) = held {
        fs::write(path, bytes).unwrap();
    }
    Repo {
        _dir: dir,
        root,
        workspace,
    }
}

/// Every file outside `.git`, plus the status and the index, so two trees compare exactly.
fn state(root: &Path) -> (BTreeMap<PathBuf, Vec<u8>>, String, String) {
    fn walk(dir: &Path, root: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            if path.is_dir() {
                walk(&path, root, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(&path).unwrap_or_default(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    (
        files,
        git(root, &["status", "--porcelain"]),
        git(root, &["diff", "--cached"]),
    )
}

fn has_checkpoint(root: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "-q", "--verify", "refs/callisto/checkpoint/version"])
        .current_dir(root)
        .status()
        .unwrap()
        .success()
}

#[test]
fn a_resumed_run_leaves_exactly_what_an_uninterrupted_run_leaves() {
    let cases: [(&str, Shape, &str, &str, Option<&str>); 5] = [
        ("cargo cascade", cargo_cascade, "", "cargo", None),
        ("npm fails after cargo refreshed", cargo_and_npm, "", "npm", None),
        (
            "workspace below the repository root",
            cargo_cascade,
            "ws",
            "cargo",
            None,
        ),
        ("pre mode with a fixed group", pre_mode_group, "", "cargo", None),
        (
            "uncommitted changeset",
            cargo_cascade,
            "",
            "cargo",
            Some(".changeset/one.md"),
        ),
    ];
    for (name, shape, subdir, failing, uncommitted) in cases {
        assert_resume_matches(name, shape, subdir, failing, uncommitted, |_| {});
    }
}

/// Runs `version` on two identical repositories, once uninterrupted and once with its `failing` lockfile tool failing
/// the first time and the run repeated, and asserts they end the same. `prepare` runs on each after its first commit.
fn assert_resume_matches(
    name: &str,
    shape: Shape,
    subdir: &str,
    failing: &str,
    uncommitted: Option<&str>,
    prepare: fn(&Repo),
) -> (Repo, Repo) {
    let control = repo(shape, subdir, uncommitted);
    prepare(&control);
    let bin = fake_tools(control.root.parent().unwrap());
    let out = version(&control.workspace, &bin, None);
    assert!(
        out.status.success(),
        "{name}: control run: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let resumed = repo(shape, subdir, uncommitted);
    prepare(&resumed);
    let bin = fake_tools(resumed.root.parent().unwrap());
    let flag = resumed.root.parent().unwrap().join("failed-once");
    let first = version(&resumed.workspace, &bin, Some((failing, &flag)));
    assert!(
        String::from_utf8_lossy(&first.stderr).contains("E208"),
        "{name}: first run should fail with E208"
    );
    assert!(
        has_checkpoint(&resumed.root),
        "{name}: the failed run keeps its checkpoint"
    );
    let out = version(&resumed.workspace, &bin, None);
    assert!(
        out.status.success(),
        "{name}: resume: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        !has_checkpoint(&resumed.root),
        "{name}: a completed run removes its checkpoint"
    );
    assert_eq!(
        git(&resumed.root, &["worktree", "list"]).lines().count(),
        1,
        "{name}: no worktree is left"
    );
    assert_eq!(
        state(&control.root),
        state(&resumed.root),
        "{name}: resumed tree differs from an uninterrupted run"
    );
    (control, resumed)
}

/// Cargo and npm, with the npm lockfile named `lock`.
fn npm_lock_is(ws: &Path, lock: &str) {
    cargo_and_npm(ws);
    fs::remove_file(ws.join("package-lock.json")).unwrap();
    write(ws, lock, "# lock\n");
}

#[test]
fn a_symlinked_a_binary_and_a_read_only_file_come_through_a_resume_unchanged() {
    // A symlinked yarn.lock that Cargo's failed refresh never touched stays a symlink.
    let (_, resumed) = assert_resume_matches(
        "symlinked lockfile",
        |ws| {
            npm_lock_is(ws, "shared.lock");
            fs::remove_file(ws.join("shared.lock")).unwrap();
            write(ws, "real.lock", "# yarn\n");
            std::os::unix::fs::symlink("real.lock", ws.join("yarn.lock")).unwrap();
        },
        "",
        "cargo",
        None,
        |_| {},
    );
    assert!(fs::symlink_metadata(resumed.workspace.join("yarn.lock"))
        .unwrap()
        .file_type()
        .is_symlink());

    // A binary lockfile that no tool rewrites keeps its bytes.
    let bytes = [0x00, 0xff, 0xfe, 0x01];
    let (_, resumed) = assert_resume_matches(
        "binary lockfile",
        |ws| {
            npm_lock_is(ws, "bun.lockb");
            fs::write(ws.join("bun.lockb"), [0x00, 0xff, 0xfe, 0x01]).unwrap();
        },
        "",
        "cargo",
        None,
        |_| {},
    );
    assert_eq!(fs::read(resumed.workspace.join("bun.lockb")).unwrap(), bytes);

    // A read-only manifest the run rewrites keeps its mode.
    let (_, resumed) = assert_resume_matches("read-only manifest", cargo_cascade, "", "cargo", None, |repo| {
        let manifest = repo.workspace.join("crates/b/Cargo.toml");
        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o444)).unwrap();
    });
    let mode = fs::metadata(resumed.workspace.join("crates/b/Cargo.toml"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o444);
}

#[test]
fn a_checkout_with_crlf_line_endings_still_resumes() {
    let (_, resumed) = assert_resume_matches("crlf", cargo_cascade, "", "cargo", None, |repo| {
        write(&repo.workspace, ".gitattributes", "*.md text eol=crlf\n");
        git(&repo.root, &["add", ".gitattributes"]);
        git(&repo.root, &["commit", "-qm", "attributes"]);
        // Check the changeset out again so it has CRLF endings on disk, as on Windows or with autocrlf.
        fs::remove_file(repo.workspace.join(".changeset/one.md")).unwrap();
        git(&repo.root, &["checkout", "--", ".changeset/one.md"]);
    });
    assert!(!resumed.workspace.join(".changeset/one.md").exists());
}

/// An npm workspace where `app` depends on `ui` with a range the bump still covers.
fn npm_range(ws: &Path) {
    write(
        ws,
        "package.json",
        "{\"name\":\"root\",\"private\":true,\"workspaces\":[\"js/*\"]}\n",
    );
    write(ws, "js/ui/package.json", "{\"name\":\"ui\",\"version\":\"1.0.0\"}\n");
    write(
        ws,
        "js/app/package.json",
        "{\"name\":\"app\",\"version\":\"1.0.0\",\"dependencies\":{\"ui\":\"^1.0.0\"}}\n",
    );
    write(ws, "package-lock.json", "{}\n");
    write(ws, "callisto.toml", "");
    write(ws, ".changeset/one.md", "---\n\"ui\": minor\n---\n\nUi change\n");
}

#[test]
fn a_manifest_edited_after_the_failure_refuses_the_resume() {
    let repo = repo(npm_range, "", None);
    let bin = fake_tools(repo.root.parent().unwrap());
    let flag = repo.root.parent().unwrap().join("failed-once");
    version(&repo.workspace, &bin, Some(("npm", &flag)));
    // Pinning the dependency changes what the plan would do; a resume must not run on the stale plan.
    let manifest = repo.workspace.join("js/app/package.json");
    let edited = "{\"name\":\"app\",\"version\":\"1.0.0\",\"dependencies\":{\"ui\":\"1.0.0\"}}\n";
    fs::write(&manifest, edited).unwrap();

    let out = version(&repo.workspace, &bin, None);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success() && stderr.contains("E340") && stderr.contains("js/app/package.json"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&manifest).unwrap(), edited);
}

#[test]
fn the_printed_abort_steps_never_remove_a_file_the_user_added() {
    let repo = repo(cargo_cascade, "", None);
    let bin = fake_tools(repo.root.parent().unwrap());
    let flag = repo.root.parent().unwrap().join("failed-once");
    version(&repo.workspace, &bin, Some(("cargo", &flag)));
    write(&repo.workspace, ".changeset/two.md", "---\n\"a\": patch\n---\n\nMore\n");
    write(&repo.workspace, "notes.txt", "mine\n");

    let refused = version(&repo.workspace, &bin, None);
    let stderr = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(stderr.contains("E340"), "{stderr}");
    for line in stderr.lines() {
        let step = line.trim_start_matches(|c: char| c.is_whitespace() || c == '│');
        if step.starts_with("git ") || step.starts_with("rm ") {
            let status = Command::new("sh")
                .args(["-c", step])
                .current_dir(&repo.workspace)
                .status()
                .unwrap();
            assert!(status.success(), "abort step `{step}` failed");
        }
    }

    assert!(
        repo.workspace.join(".changeset/two.md").exists() && repo.workspace.join("notes.txt").exists(),
        "the abort steps removed a file the user added"
    );
}

#[test]
fn a_dry_run_leaves_other_worktrees_registered() {
    let repo = repo(cargo_cascade, "", None);
    let bin = fake_tools(repo.root.parent().unwrap());
    let other = repo.root.parent().unwrap().join("other");
    git(
        &repo.root,
        &["worktree", "add", "--detach", "-q", other.to_str().unwrap()],
    );
    let flag = repo.root.parent().unwrap().join("failed-once");
    version(&repo.workspace, &bin, Some(("cargo", &flag)));
    // The other worktree's directory is away, as on an unmounted drive.
    let away = repo.root.parent().unwrap().join("away");
    fs::rename(&other, &away).unwrap();

    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--cwd", repo.workspace.to_str().unwrap(), "--dry-run", "version"])
        .env("PATH", path)
        .output()
        .unwrap();

    assert_eq!(
        git(&repo.root, &["worktree", "list"]).lines().count(),
        2,
        "a dry run must not prune a worktree whose directory is away"
    );
}

#[test]
fn changed_inputs_refuse_the_resume_and_the_printed_steps_undo_the_run() {
    let changes: [(&str, Shape); 2] = [
        ("callisto.toml", |root| {
            write(
                root,
                "callisto.toml",
                "[[fixed-group]]\nname = \"all\"\nmembers = [\"cargo/a\", \"cargo/b\"]\n",
            );
        }),
        ("a tag", |root| {
            git(root, &["tag", "-a", "a@1.0.0", "-m", "a"]);
        }),
    ];
    for (name, change) in changes {
        let repo = repo(cargo_cascade, "", None);
        let bin = fake_tools(repo.root.parent().unwrap());
        let before = state(&repo.root);
        version(
            &repo.workspace,
            &bin,
            Some(("cargo", &repo.root.parent().unwrap().join("failed-once"))),
        );
        change(&repo.root);

        let refused = version(&repo.workspace, &bin, None);
        let stderr = String::from_utf8_lossy(&refused.stderr).into_owned();
        assert!(
            !refused.status.success() && stderr.contains("E340"),
            "{name}: resume should be refused: {stderr}"
        );

        // The printed steps, as a user would copy them.
        for line in stderr.lines() {
            let step = line.trim_start_matches(|c: char| c.is_whitespace() || c == '│');
            if step.starts_with("git ") || step.starts_with("rm ") {
                let status = Command::new("sh")
                    .args(["-c", step])
                    .current_dir(&repo.workspace)
                    .status()
                    .unwrap();
                assert!(status.success(), "{name}: abort step `{step}` failed");
            }
        }
        if name == "a tag" {
            git(&repo.root, &["tag", "-d", "a@1.0.0"]);
        } else {
            write(&repo.root, "callisto.toml", "");
        }
        assert_eq!(
            state(&repo.root),
            before,
            "{name}: the abort steps should restore the state before the run"
        );
        assert!(
            !has_checkpoint(&repo.root),
            "{name}: the abort steps remove the checkpoint"
        );
    }
}

/// The emitted files are the run's last effects: a run that consumed its changesets but could not write the preview
/// keeps its checkpoint, and running again writes the preview.
#[test]
fn a_run_that_failed_to_write_its_preview_writes_it_when_run_again() {
    let repo = repo(cargo_cascade, "", None);
    let bin = fake_tools(repo.root.parent().unwrap());
    let out_dir = repo.root.parent().unwrap().join("out");
    fs::create_dir_all(&out_dir).unwrap();
    fs::set_permissions(&out_dir, fs::Permissions::from_mode(0o555)).unwrap();
    let preview = out_dir.join("preview.json");
    let run = |bin: &Path| {
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
        Command::new(env!("CARGO_BIN_EXE_callisto"))
            .args([
                "--cwd",
                repo.workspace.to_str().unwrap(),
                "version",
                "--emit-preview",
                preview.to_str().unwrap(),
            ])
            .env("PATH", path)
            .output()
            .unwrap()
    };

    let first = run(&bin);
    assert!(!first.status.success(), "the preview write should fail");
    assert!(
        !repo.workspace.join(".changeset/one.md").exists(),
        "the changeset was consumed before the preview"
    );
    assert!(has_checkpoint(&repo.root));

    fs::set_permissions(&out_dir, fs::Permissions::from_mode(0o755)).unwrap();
    let second = run(&bin);
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));
    assert!(preview.exists(), "running again writes the preview");
    assert!(!has_checkpoint(&repo.root));
}

#[test]
fn a_version_edited_by_hand_after_the_failure_is_refused_not_overwritten() {
    let repo = repo(cargo_cascade, "", None);
    let bin = fake_tools(repo.root.parent().unwrap());
    version(
        &repo.workspace,
        &bin,
        Some(("cargo", &repo.root.parent().unwrap().join("failed-once"))),
    );
    let manifest = repo.workspace.join("crates/a/Cargo.toml");
    let edited = fs::read_to_string(&manifest)
        .unwrap()
        .replace("version = \"1.1.0\"", "version = \"7.0.0\"");
    fs::write(&manifest, &edited).unwrap();

    let out = version(&repo.workspace, &bin, None);

    assert!(!out.status.success(), "a hand edit must not be resumed over");
    assert_eq!(fs::read_to_string(&manifest).unwrap(), edited);
}
