//! A large, deterministic Cargo or pnpm workspace in a real Git repository, built with one `git fast-import` stream.
//!
//! Every package is tagged `pkg-<n>@1.0.0`, each at one of the first `extra_commits + 1` commits in turn, as if released
//! at different times. The commits after the first each edit one package, and the last commit adds a changeset to every
//! tenth package, so `status`, `version` and `release --dry-run` have real work.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const TIMESTAMP: u64 = 1_700_000_000;

/// The packages a generated repository holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageKind {
    /// Cargo crates under `crates/`.
    Cargo,
    /// pnpm packages under `packages/`, each naming the previous one by a relative `workspace:` path.
    PnpmRelative,
}

impl PackageKind {
    fn dir(self, index: usize) -> String {
        match self {
            PackageKind::Cargo => format!("crates/pkg-{index}"),
            PackageKind::PnpmRelative => format!("packages/pkg-{index}"),
        }
    }

    fn selector(self, index: usize) -> String {
        match self {
            PackageKind::Cargo => format!("cargo/pkg-{index}"),
            PackageKind::PnpmRelative => format!("npm/pkg-{index}"),
        }
    }
}

/// Shape of the generated repository.
#[derive(Clone, Copy, Debug)]
pub struct ScaledWorkspace {
    /// Which ecosystem the packages are in.
    pub kind: PackageKind,
    /// Packages, each depending on the previous one except every fourth.
    pub packages: usize,
    /// Commits after the tagged one, each editing one package's source.
    pub extra_commits: usize,
}

impl ScaledWorkspace {
    /// Writes the repository under `root`, which must exist and be empty.
    pub fn build(self, root: &Path) {
        git(root, &["init", "-q", "-b", "main"]);
        let stream = self.stream();
        let mut child = Command::new("git")
            .args(["fast-import", "--quiet"])
            .current_dir(root)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&stream).unwrap();
        assert!(child.wait().unwrap().success(), "git fast-import");
        git(root, &["reset", "-q", "--hard", "main"]);
        git(
            root,
            &["remote", "add", "origin", "https://github.com/example/bench.git"],
        );
    }

    fn stream(self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut mark = 1;

        let mut files = vec![("callisto.toml".to_owned(), "# bench\n".to_owned())];
        match self.kind {
            PackageKind::Cargo => files.push((
                "Cargo.toml".to_owned(),
                "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n".to_owned(),
            )),
            PackageKind::PnpmRelative => {
                files.push((
                    "pnpm-workspace.yaml".to_owned(),
                    "packages:\n  - \"packages/*\"\n".to_owned(),
                ));
                files.push(("package.json".to_owned(), "{\"private\":true}\n".to_owned()));
            }
        }
        for index in 0..self.packages {
            let dir = self.kind.dir(index);
            match self.kind {
                PackageKind::Cargo => files.push((format!("{dir}/Cargo.toml"), manifest(index))),
                PackageKind::PnpmRelative => files.push((format!("{dir}/package.json"), pnpm_manifest(index))),
            }
            files.push((format!("{dir}/src/lib.rs"), "pub fn f() {}\n".to_owned()));
        }
        commit(&mut out, mark, "chore: scaffold the workspace", &files);

        for step in 0..self.extra_commits {
            mark += 1;
            let target = (step * 7919) % self.packages;
            let files = [(
                format!("{}/src/lib.rs", self.kind.dir(target)),
                format!("pub fn f() {{}}\n// edit {step}\n"),
            )];
            commit(&mut out, mark, &format!("fix: edit pkg-{target}"), &files);
        }

        // Independent versioning: packages were released at different points in history, so their tags are spread
        // over the commits instead of sharing one.
        for index in 0..self.packages {
            let name = format!("pkg-{index}@1.0.0");
            let message = format!("Release {name}\n");
            let target = 1 + index % (self.extra_commits + 1);
            write!(
                out,
                "tag {name}\nfrom :{target}\ntagger Bench <bench@example.com> {TIMESTAMP} +0000\ndata {}\n{message}\n",
                message.len()
            )
            .unwrap();
        }

        mark += 1;
        let changesets: Vec<(String, String)> = (0..self.packages)
            .step_by(10)
            .map(|index| {
                (
                    format!(".changeset/bench-{index}.md"),
                    format!(
                        "---\n\"{}\": patch\n---\n\nFix pkg-{index}.\n",
                        self.kind.selector(index)
                    ),
                )
            })
            .collect();
        commit(&mut out, mark, "chore: add changesets", &changesets);
        out
    }
}

fn manifest(index: usize) -> String {
    let mut text = format!("[package]\nname = \"pkg-{index}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n");
    if !index.is_multiple_of(4) {
        let previous = index - 1;
        text.push_str(&format!(
            "\n[dependencies]\npkg-{previous} = {{ path = \"../pkg-{previous}\", version = \"1.0.0\" }}\n"
        ));
    }
    text
}

fn pnpm_manifest(index: usize) -> String {
    let dependencies = if index.is_multiple_of(4) {
        String::new()
    } else {
        let previous = index - 1;
        format!(",\"dependencies\":{{\"pkg-{previous}\":\"workspace:../pkg-{previous}\"}}")
    };
    format!("{{\"name\":\"pkg-{index}\",\"version\":\"1.0.0\"{dependencies}}}\n")
}

fn commit(out: &mut Vec<u8>, mark: usize, message: &str, files: &[(String, String)]) {
    write!(
        out,
        "commit refs/heads/main\nmark :{mark}\ncommitter Bench <bench@example.com> {} +0000\ndata {}\n{message}\n",
        TIMESTAMP + mark as u64,
        message.len()
    )
    .unwrap();
    for (path, content) in files {
        write!(out, "M 100644 inline {path}\ndata {}\n{content}\n", content.len()).unwrap();
    }
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git").args(args).current_dir(root).status().unwrap();
    assert!(status.success(), "git {args:?}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_tagged_repository_with_the_requested_shape() {
        let dir = tempfile::tempdir().unwrap();
        ScaledWorkspace {
            kind: PackageKind::Cargo,
            packages: 12,
            extra_commits: 5,
        }
        .build(dir.path());
        let tags = Command::new("git")
            .args(["tag", "--list"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&tags.stdout).lines().count(), 12);
        let commits = Command::new("git")
            .args(["rev-list", "--count", "HEAD"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&commits.stdout).trim(), "7");
        assert!(dir.path().join("crates/pkg-11/Cargo.toml").is_file());
        assert!(dir.path().join(".changeset/bench-10.md").is_file());
    }

    #[test]
    fn a_pnpm_workspace_names_the_previous_package_by_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        ScaledWorkspace {
            kind: PackageKind::PnpmRelative,
            packages: 6,
            extra_commits: 2,
        }
        .build(dir.path());
        let manifest = std::fs::read_to_string(dir.path().join("packages/pkg-5/package.json")).unwrap();
        assert!(manifest.contains("\"pkg-4\":\"workspace:../pkg-4\""), "{manifest}");
        let changeset = std::fs::read_to_string(dir.path().join(".changeset/bench-0.md")).unwrap();
        assert!(changeset.contains("\"npm/pkg-0\""), "{changeset}");
    }
}
