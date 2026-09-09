//! Regression test for F16: redundant identity resolution in
//! `MoonProjectLocator`.
//!
//! `declared_edges` used to independently call `resolve_id` for every
//! project (redundant -- `projects()`, called earlier in the same flow,
//! already resolved every project's identity once) and, worse, resolved a
//! dependency edge's target once per edge rather than once per unique target
//! project. A widely-depended-on package (here, `utils`, named by 5
//! dependents) had its manifest read and parsed once per dependent instead of
//! once total. `IdentityResolver::resolve`'s memo (shared across `projects()`
//! and `declared_edges()` via `MoonProjectLocator`'s single long-lived
//! `IdentityResolver`) fixes this without any change to `declared_edges`
//! itself.

use std::path::Path;

use callisto_graph::identity::{identity_read_count, reset_identity_read_count};
use callisto_graph::locate::ProjectLocator;
use callisto_model::{CommandError, CommandOutput, CommandRunner};
use callisto_moon::MoonProjectLocator;
use serial_test::serial;

struct MockMoonRunner {
    graph_json: String,
}

impl CommandRunner for MockMoonRunner {
    fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
        if program == "moon" && args.contains(&"project-graph") {
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: self.graph_json.clone(),
                stderr: String::new(),
            })
        } else {
            Err(CommandError::NotFound {
                program: program.to_string(),
            })
        }
    }
}

#[test]
#[serial]
fn declared_edges_reuses_identities_already_resolved_by_projects() {
    let workspace = tempfile::tempdir().expect("failed to create tempdir");
    let root = workspace.path();

    // One shared "utils" package, plus 5 dependents that all depend on it.
    let utils_dir = root.join("utils");
    std::fs::create_dir_all(&utils_dir).unwrap();
    std::fs::write(
        utils_dir.join("Cargo.toml"),
        "[package]\nname = \"utils\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let mut projects = serde_json::Map::new();
    projects.insert(
        "utils".to_string(),
        serde_json::json!({
            "id": "utils",
            "root": utils_dir.to_str().unwrap(),
            "source": "utils",
            "dependencies": []
        }),
    );

    for i in 0..5 {
        let name = format!("dep{i}");
        let dep_dir = root.join(&name);
        std::fs::create_dir_all(&dep_dir).unwrap();
        std::fs::write(
            dep_dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        projects.insert(
            name.clone(),
            serde_json::json!({
                "id": name,
                "root": dep_dir.to_str().unwrap(),
                "source": name,
                "dependencies": [
                    {"id": "utils", "scope": "production", "via": null}
                ]
            }),
        );
    }

    let graph_json = serde_json::json!({ "data": projects }).to_string();
    let runner = MockMoonRunner { graph_json };
    let locator = MoonProjectLocator::new(&runner, root.to_path_buf()).expect("MoonProjectLocator::new must succeed");

    // This counter is process-global (see callisto_graph::identity::identity_read_count).
    // This test must not run concurrently with other tests in this binary
    // that also resolve real manifests, so it is the sole
    // identity-resolving test in this file.
    reset_identity_read_count();

    let projects = locator.projects().expect("projects() must succeed");
    assert_eq!(projects.len(), 6, "utils + 5 dependents");

    let after_projects = identity_read_count();
    assert_eq!(
        after_projects, 6,
        "projects() must read/parse each of the 6 project manifests exactly once"
    );

    let edges = locator.declared_edges().expect("declared_edges() must return Some");
    assert_eq!(edges.len(), 5, "one edge per dependent onto utils");

    let after_edges = identity_read_count();
    assert_eq!(
        after_edges,
        after_projects,
        "declared_edges() named `utils` as the target of 5 separate edges, but must \
         reuse the identity projects() already resolved for it (and for every \
         `from` project) instead of re-reading any manifest -- total reads must stay \
         at {after_projects}, not grow to {}",
        after_projects + edges.len()
    );
}
