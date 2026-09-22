//! Release profiles must not share a destination, judged on the canonical
//! destination rather than on how it is spelled.

use std::{fs, path::Path, process::Command};

const HEAD: &str = "[release]\nproduct-package = \"cargo/core-crate\"\n\n[[release.artifact]]\npackage = \"cargo/core-crate\"\ntarget = \"aarch64-apple-darwin\"\nasset-name = \"callisto-aarch64-apple-darwin.tar.gz\"\n\n[[release.artifact]]\npackage = \"cargo/core-crate\"\ntarget = \"x86_64-unknown-linux-gnu\"\nasset-name = \"callisto-x86_64-unknown-linux-gnu.tar.gz\"\n\n[[release.artifact]]\npackage = \"cargo/core-crate\"\ntarget = \"x86_64-unknown-linux-musl\"\nasset-name = \"callisto-x86_64-unknown-linux-musl.tar.gz\"\n\n[[release.artifact]]\npackage = \"cargo/core-crate\"\ntarget = \"wasm32-wasip1\"\nasset-name = \"callisto-moon.wasm\"\n";

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git").args(args).current_dir(root).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs `callisto status` over a workspace whose `callisto.toml` is `HEAD` plus `fragment`.
fn status_with(fragment: &str) -> (bool, Vec<String>, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Callisto Test"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/core\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/core/src")).unwrap();
    fs::write(
        root.join("crates/core/Cargo.toml"),
        "[package]\nname = \"core-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("crates/core/src/lib.rs"), "\n").unwrap();
    fs::write(root.join("callisto.toml"), format!("{HEAD}{fragment}")).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial"]);
    let out = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--format", "json", "--cwd", root.to_str().unwrap(), "status"])
        .output()
        .unwrap();
    let mut codes = Vec::new();
    for stream in [&out.stdout, &out.stderr] {
        let text = String::from_utf8_lossy(stream);
        for token in text.split(|c: char| !c.is_ascii_alphanumeric()) {
            if token.len() == 4 && token.starts_with('E') && token[1..].chars().all(|c| c.is_ascii_digit()) {
                codes.push(token.to_owned());
            }
        }
    }
    codes.sort();
    codes.dedup();
    let detail = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), codes, detail)
}

const E197: &str = "E197";

#[test]
fn red_c8_distinct_builtin_registries_in_different_profiles_are_accepted() {
    let (ok, codes, detail) = status_with(
        "\n[release.profiles.a]\nforge-repository = \"org/one\"\nregistry-routes = { cratesIo = \"cratesIo\" }\n\n[release.profiles.b]\nforge-repository = \"org/two\"\nregistry-routes = { npm = \"npm\" }\n",
    );
    assert!(ok && !codes.iter().any(|c| c == E197), "codes {codes:?}: {detail}");
}

#[test]
fn red_c8_registry_url_spellings_of_one_destination_are_rejected() {
    let (ok, codes, detail) = status_with(
        "\n[release.profiles.a]\nforge-repository = \"org/one\"\nregistry-routes = { cratesIo = \"r1\" }\n\n[release.profiles.b]\nforge-repository = \"org/two\"\nregistry-routes = { cratesIo = \"r2\" }\n\n[registries.r1]\nkind = \"cargo\"\nurl = \"https://x.example/\"\n\n[registries.r2]\nkind = \"cargo\"\nurl = \"https://X.example:443\"\n",
    );
    assert!(
        !ok && codes.iter().any(|c| c == E197),
        "expected E197, got ok={ok} codes {codes:?}: {detail}"
    );
}

#[test]
fn red_c8_forge_repository_case_variants_are_rejected() {
    let (ok, codes, detail) = status_with(
        "\n[release.profiles.a]\nforge-repository = \"org/one\"\n\n[release.profiles.b]\nforge-repository = \"Org/One\"\n",
    );
    assert!(
        !ok && codes.iter().any(|c| c == E197),
        "expected E197, got ok={ok} codes {codes:?}: {detail}"
    );
}

#[test]
fn red_c8_forge_repository_dot_git_suffix_is_rejected() {
    let (ok, codes, detail) = status_with(
        "\n[release.profiles.a]\nforge-repository = \"org/one\"\n\n[release.profiles.b]\nforge-repository = \"org/one.git\"\n",
    );
    assert!(
        !ok && codes.iter().any(|c| c == E197),
        "expected E197, got ok={ok} codes {codes:?}: {detail}"
    );
}

#[test]
fn red_c8_non_production_profile_routing_to_the_production_registry_is_rejected() {
    let (ok, codes, detail) = status_with(
        "\n[release.profiles.rehearsal]\nforge-repository = \"org/rehearsal\"\nregistry-routes = { cratesIo = \"cratesIo\" }\n",
    );
    assert!(
        !ok && codes.iter().any(|c| c == E197),
        "expected E197, got ok={ok} codes {codes:?}: {detail}"
    );
}
