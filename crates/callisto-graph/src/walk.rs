use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use callisto_manifests::{
    detect_npm_workspace_kind, Manifest, OpenContext, WorkspaceCargoResolver,
};
use callisto_model::{
    CommandRunner, DepEdge, Diagnostic, DiagnosticCode, DiagnosticSeverity, Ecosystem,
    ManifestDecl, ManifestFormat, ManifestRole, Package, PackageId, PublishTarget, ReleaseTrigger,
};

use crate::config::resolve::resolve_package_config;
use crate::config::ResolvedConfig;
use crate::crosscheck::crosscheck_declared_edges;
use crate::error::GraphError;
use crate::identity::IdentityIndex;
use crate::locate::ProjectLocator;
use crate::manifest_cache::open_cached;
use crate::resolver::ManifestWalkResolver;

impl ManifestWalkResolver {
    pub fn build<L: ProjectLocator, R: CommandRunner>(
        root: &Path,
        locator: &L,
        _runner: &R,
        cfg: &ResolvedConfig,
        manifest_cache: &RefCell<BTreeMap<PathBuf, Arc<dyn Manifest>>>,
    ) -> Result<Self, GraphError> {
        let projects = locator.projects()?;

        let cargo_workspace = if root.join("Cargo.toml").exists() {
            if let Ok(resolver) = WorkspaceCargoResolver::load(&root.join("Cargo.toml")) {
                resolver.inheritance().ok().map(Arc::new)
            } else {
                None
            }
        } else {
            None
        };

        let npm_workspace_kind = detect_npm_workspace_kind(root).ok().flatten();

        let ctx = OpenContext {
            workspace_root: root,
            cargo_workspace,
            npm_workspace_kind,
        };

        let mut package_manifest_decls: BTreeMap<PackageId, (PathBuf, Vec<ManifestDecl>)> =
            BTreeMap::new();
        let mut index = IdentityIndex::default();
        let mut diagnostics = Vec::new();

        // Use the identity already resolved by the locator (`proj.id`) rather
        // than re-reading manifests through `IdentityResolver::resolve`.
        // The locator (e.g. `IgnoreWalkLocator`) already parsed each manifest
        // to discover the project, so re-resolving from scratch is redundant
        // and fragile — in particular, `IdentityResolver` historically had no
        // `Ecosystem::Pypi` arm and would crash for Python projects.
        let mut by_path: BTreeMap<PathBuf, Vec<(Ecosystem, PackageId)>> = BTreeMap::new();
        for proj in &projects {
            by_path
                .entry(proj.path.clone())
                .or_default()
                .push((proj.ecosystem, proj.id.clone()));
        }

        for (rel_path, mut list) in by_path {
            // Explicit precedence: Cargo (0) > Npm (1) > Pypi (2) > others.
            // Do NOT rely on enum discriminant order -- sort by this named
            // priority function so the precedence survives future variant
            // additions to `Ecosystem`.
            list.sort_by_key(|a| ecosystem_primary_priority(a.0));
            let primary_id = list[0].1.clone();

            index
                .bare
                .insert(primary_id.name().to_string(), primary_id.clone());

            let mut decls = Vec::new();
            for (eco, id) in &list {
                let (fmt, filename) = match eco {
                    Ecosystem::Cargo => (ManifestFormat::CargoToml, "Cargo.toml"),
                    Ecosystem::Npm => (ManifestFormat::PackageJson, "package.json"),
                    Ecosystem::Pypi => (ManifestFormat::PyprojectToml, "pyproject.toml"),
                    _ => (ManifestFormat::PackageJson, "package.json"),
                };
                let manifest_rel = rel_path.join(filename);
                if let Ok(decl) =
                    ManifestDecl::new(manifest_rel.clone(), ManifestRole::Canonical, fmt)
                {
                    decls.push(decl);
                }
                // For napi platform packages (os + cpu constraints in package.json),
                // also push a Platform-role decl so plan_publish can route them
                // into npm_platform_packages instead of npm_main_packages.
                if *eco == Ecosystem::Npm {
                    let role = detect_npm_role(&root.join(&manifest_rel));
                    if let ManifestRole::Platform { .. } = role {
                        if let Ok(platform_decl) =
                            ManifestDecl::new(manifest_rel.clone(), role.clone(), fmt)
                        {
                            decls.push(platform_decl);
                        }
                        // `id` is this manifest's own npm identity, resolved from its
                        // own `name` field -- distinct from `primary_id` whenever a
                        // higher-priority ecosystem (Cargo) shares this directory
                        // (Case D). Config authors reference the platform package by
                        // its own name in `[[fixed-group]] members`, so that must be
                        // the index key; `primary_id` is who it belongs to.
                        index.platform.insert(
                            id.name().to_string(),
                            (primary_id.clone(), manifest_rel, role),
                        );
                    }
                }
                index
                    .native
                    .insert((*eco, id.name().to_string()), primary_id.clone());
                if id.name() == primary_id.name() {
                    index
                        .prefixed
                        .insert((*eco, id.name().to_string()), primary_id.clone());
                }
            }

            if let Some((existing_path, _)) =
                package_manifest_decls.insert(primary_id.clone(), (rel_path.clone(), decls))
            {
                return Err(GraphError::DuplicatePackage {
                    id: primary_id,
                    paths: vec![existing_path, rel_path],
                });
            }
        }

        // Tracks, per cfg.package_sets entry (by index), whether it matched at
        // least one real discovered package during this walk. A [[package-set]]
        // rule that matches nothing is almost always a typo or a stale
        // ecosystem prefix (see PackageSetMatchedNothing below) rather than
        // intentional, so it must be surfaced instead of silently ignored.
        let mut package_set_matched = vec![false; cfg.package_sets.len()];

        let mut packages = BTreeMap::new();
        for (id, (rel_path, decls)) in package_manifest_decls {
            let ch_path = rel_path.join("CHANGELOG.md");
            let mut publish_to = Vec::new();
            for decl in &decls {
                if let Ok(editor) = open_cached(manifest_cache, decl, &ctx) {
                    for target in editor.publish_targets() {
                        if target != PublishTarget::None && !publish_to.contains(&target) {
                            publish_to.push(target);
                        }
                    }
                }
            }
            if publish_to.is_empty() {
                publish_to.push(PublishTarget::None);
            }

            // The real ecosystem(s) this package's manifests were discovered
            // in. `id` itself is always PackageId::Bare here (see the
            // SPEC-002 AC-5 note below), so this is the only place an
            // ecosystem-prefixed [[package-set]] pattern has anything to
            // match against.
            let package_ecosystems: Vec<Ecosystem> = decls.iter().map(|d| d.ecosystem()).collect();

            // Two-pass specificity search for [[package]] rules (SPEC-002 AC-1/2/3).
            // Pass 1: find the first Prefixed rule (pattern.ecosystem().is_some())
            //         that matches this package's ID. Prefixed rules always win
            //         over Bare rules regardless of declaration order in callisto.toml.
            // Pass 2: only if pass 1 found nothing, find the first Bare rule
            //         (any rule, since no Prefixed rule matched, the first match
            //          is necessarily Bare) that matches this package's ID.
            // Within each pass, first-match-wins (TOML declaration order) applies.
            let pkg_override = resolve_package_config(&id, cfg);

            // Record which [[package-set]] patterns match this package,
            // independent of whether a [[package]] rule ends up shadowing the
            // fallback below — a pattern that is always shadowed still
            // "matched" for the purpose of the zero-match diagnostic.
            for (idx, (pattern, _)) in cfg.package_sets.iter().enumerate() {
                if pattern.matches_in_ecosystems(id.name(), &package_ecosystems) {
                    package_set_matched[idx] = true;
                }
            }

            // If no [[package]] rule matched, look for a [[package-set]] fallback.
            // [[package-set]] uses glob patterns and can match many packages at once;
            // [[package]] always takes priority over [[package-set]] for the same package.
            let set_override = if pkg_override.is_none() {
                cfg.package_sets
                    .iter()
                    .find(|(pattern, _)| {
                        pattern.matches_in_ecosystems(id.name(), &package_ecosystems)
                    })
                    .map(|(_, cfg)| cfg)
            } else {
                None
            };

            let active_override = pkg_override.or(set_override);

            let release_trigger = active_override
                .and_then(|o| o.release_trigger)
                .unwrap_or(ReleaseTrigger::Changeset);

            let tag_template = active_override.and_then(|o| o.tag_template.clone());

            let changelog =
                if let Some(override_path) = active_override.and_then(|o| o.changelog.as_ref()) {
                    Some(rel_path.join(override_path))
                } else {
                    Some(ch_path)
                };

            // Apply the resolved override's publish-to if the operator explicitly set it.
            //
            // A `[[package]]`/`[[package-set]]` rule has no package context at
            // config-parse time (it's just a pattern + string list), so the
            // only place the package's real, detected ecosystem is known is
            // here, once `decls` (the package's actual manifests) have been
            // walked. Reject any configured target whose `.ecosystem()` does
            // not match one of the package's detected ecosystems — e.g.
            // `publish-to = ["nuget"]` on a Cargo-only crate — rather than
            // silently accepting it and having the crate vanish from every
            // real publish downstream with zero diagnostic.
            if let Some(override_targets) = active_override.and_then(|o| o.publish_to.as_deref()) {
                for target in override_targets {
                    if let Some(target_ecosystem) = target.ecosystem() {
                        if !package_ecosystems.contains(&target_ecosystem) {
                            return Err(GraphError::PublishTargetEcosystemMismatch {
                                package: id.clone(),
                                target: target.config_str().to_string(),
                                target_ecosystem,
                                package_ecosystems,
                            });
                        }
                    }
                }
                publish_to = override_targets.to_vec();
            }

            let pkg = Package {
                id: id.clone(),
                manifests: decls,
                changelog,
                release_trigger,
                publish_to,
                tag_template,
            };
            packages.insert(id, pkg);
        }

        // A [[package-set]] rule that matched zero real packages is almost
        // always a mistake (e.g. an ecosystem prefix that doesn't correspond
        // to any discovered package, or a typo in the glob) rather than
        // intentional, so surface it as a visible, non-fatal diagnostic
        // instead of letting the rule silently do nothing. This is
        // deliberately advisory rather than a hard `GraphError`: a
        // `[[package-set]]` rule declared for a monorepo-wide callisto.toml
        // can legitimately match nothing when only part of the workspace is
        // present (e.g. a partial checkout or filtered walk), and a hard
        // error would break that case.
        for (idx, (pattern, _)) in cfg.package_sets.iter().enumerate() {
            if !package_set_matched[idx] {
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::PackageSetMatchedNothing,
                    severity: DiagnosticSeverity::Warning,
                    message: format!("[[package-set]] `{}` matched no packages", pattern.as_str()),
                    package: None,
                    path: None,
                    escalated_by: None,
                    governed_by: None,
                });
            }
        }

        // SPEC-002 AC-5: Cross-ecosystem diagnostic pass.
        //
        // For each bare [[package]] rule in cfg.packages (pattern.ecosystem() == None),
        // compute the distinct-ecosystem set: the Ecosystem values found in the canonical
        // ManifestDecls of every packages-map entry matched by this rule.
        //
        // Packages-map keys are ALWAYS PackageId::Bare (IgnoreWalkLocator builds ids
        // from raw manifest name strings via PackageId::parse, which yields Bare for
        // plain names). Ecosystem information is therefore sourced from pkg.manifests,
        // not from map keys. Calling key.ecosystem() would always return None and the
        // diagnostic would never fire — do NOT use key.ecosystem().
        //
        // The primary trigger is a single directory containing both Cargo.toml and
        // package.json (the napi case): one packages-map entry with two canonical
        // ManifestDecls whose ecosystems are {Cargo, Npm}.
        //
        // Prefixed [[package]] rules are skipped unconditionally (AC-7).
        // [[package-set]] rules are never iterated here (AC-8).
        for (pattern, _) in &cfg.packages {
            if pattern.ecosystem().is_some() {
                continue; // Prefixed rules never trigger this diagnostic (AC-7).
            }
            let ecosystems: BTreeSet<Ecosystem> = packages
                .iter()
                .filter(|(key, _)| pattern.matches(key))
                // Use the existing Package::canonical_manifests() helper
                // (package.rs) which filters to ManifestRole::Canonical.
                .flat_map(|(_, pkg)| pkg.canonical_manifests().map(|d| d.ecosystem()))
                .collect();
            if ecosystems.len() >= 2 {
                let eco_list: Vec<&str> = ecosystems.iter().map(|e| e.prefix()).collect();
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::BareRuleMatchesMultipleEcosystems,
                    severity: DiagnosticSeverity::Warning,
                    message: format!(
                        "[[package]] rule `{}` matches packages in multiple ecosystems ({}); \
                         use an ecosystem-prefixed pattern like `{}/{}` to target only one",
                        pattern.name(),
                        eco_list.join(", "),
                        ecosystems
                            .iter()
                            .next()
                            .map(|e| e.prefix())
                            .unwrap_or("cargo"),
                        pattern.name(),
                    ),
                    package: None,
                    path: None,
                    escalated_by: None,
                    governed_by: None,
                });
            }
        }

        let mut edges = Vec::new();
        let mut out_index: BTreeMap<PackageId, Vec<usize>> = BTreeMap::new();
        let mut in_index: BTreeMap<PackageId, Vec<usize>> = BTreeMap::new();

        for pkg in packages.values() {
            for decl in &pkg.manifests {
                if decl.role != ManifestRole::Canonical {
                    continue;
                }
                if let Ok(m) = open_cached(manifest_cache, decl, &ctx) {
                    for entry in m.iter_dependencies() {
                        let (spec, declaring_path) = if entry.inherited {
                            if let Some(ref inh) = ctx.cargo_workspace {
                                if let Some(inherited_dep) = inh.inherited(&entry.name) {
                                    (
                                        inherited_dep.spec.clone(),
                                        inherited_dep.declared_in.to_path_buf(),
                                    )
                                } else {
                                    (entry.spec.clone(), decl.path.clone())
                                }
                            } else {
                                (entry.spec.clone(), decl.path.clone())
                            }
                        } else {
                            (entry.spec.clone(), decl.path.clone())
                        };

                        if let Some(to) = index.resolve_native_with_fallback(
                            decl.ecosystem(),
                            &entry.name,
                            &mut diagnostics,
                        ) {
                            let idx = edges.len();
                            let edge = DepEdge {
                                from: pkg.id.clone(),
                                to: to.clone(),
                                kind: entry.kind,
                                spec,
                                from_manifest: declaring_path,
                                inherited: entry.inherited,
                            };
                            edges.push(edge);

                            out_index.entry(pkg.id.clone()).or_default().push(idx);
                            in_index.entry(to.clone()).or_default().push(idx);
                        }
                    }
                }
            }
        }

        if let Some(declared) = locator.declared_edges() {
            let cross_diags = crosscheck_declared_edges(&packages, &edges, &declared);
            diagnostics.extend(cross_diags);
        }

        Ok(ManifestWalkResolver {
            packages,
            edges,
            out_index,
            in_index,
            index,
            diagnostics,
        })
    }
}

/// Reads `package.json` at `abs_path` and returns `ManifestRole::Platform`
/// when the manifest declares both `os` and `cpu` constraint arrays (the napi
/// platform-package convention). Returns `ManifestRole::Canonical` for all
/// other npm packages and on any read/parse failure.
///
/// Note: this performs a second `fs::read` on each npm `package.json` because
/// the `manifest_cache` stores `Arc<dyn Manifest>` (which does not expose raw
/// JSON fields like `os`/`cpu`) rather than a raw `serde_json::Value`. Fixing
/// the redundancy would require a `Manifest::npm_role()` extension method in
/// the `callisto-manifests` crate.
fn detect_npm_role(abs_path: &Path) -> ManifestRole {
    let Ok(bytes) = std::fs::read(abs_path) else {
        return ManifestRole::Canonical;
    };
    let Ok(Value::Object(map)) = serde_json::from_slice(&bytes) else {
        return ManifestRole::Canonical;
    };

    let has_os = map
        .get("os")
        .and_then(|v| v.as_array())
        .is_some_and(|a| !a.is_empty());
    let has_cpu = map
        .get("cpu")
        .and_then(|v| v.as_array())
        .is_some_and(|a| !a.is_empty());

    if !has_os || !has_cpu {
        return ManifestRole::Canonical;
    }

    let Some(platform) = map
        .get("os")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return ManifestRole::Canonical;
    };

    let Some(arch) = map
        .get("cpu")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return ManifestRole::Canonical;
    };

    // npm's standard `os`/`cpu` manifest fields have no libc/ABI concept at
    // all, so a real disk-discovered Linux napi platform package always
    // produced `abi: None` here -- but `napi.rs::role_to_triple`'s Linux
    // match arms all require a concrete ABI, meaning such a package could
    // never resolve to its triple. napi-rs's own package-generation
    // convention encodes the ABI in the package *name*'s suffix instead
    // (e.g. "@scope/pkg-linux-x64-gnu"), so infer it from there when the
    // platform is linux.
    let abi = if platform == "linux" {
        map.get("name")
            .and_then(|v| v.as_str())
            .and_then(napi_linux_abi_from_package_name)
    } else {
        None
    };

    ManifestRole::Platform {
        platform,
        arch,
        abi,
    }
}

/// Infers a Linux napi-rs platform package's libc ABI from its package
/// name's trailing suffix. Returns `None` when the name has no recognized
/// suffix -- this infers when the signal is present, it doesn't invent an
/// ABI that isn't actually there.
fn napi_linux_abi_from_package_name(name: &str) -> Option<String> {
    for abi in ["gnueabihf", "gnu", "musl"] {
        if name.ends_with(&format!("-{abi}")) {
            return Some(abi.to_string());
        }
    }
    None
}

/// Explicit ecosystem precedence for primary-ID selection when a single
/// project directory contains manifests from multiple ecosystems (e.g., both
/// `Cargo.toml` and `package.json`). Lower value = higher priority.
///
/// Precedence: Cargo (0) > Npm (1) > Pypi (2) > others (255).
///
/// This function is used instead of `Ecosystem`'s derived `Ord` so that the
/// ordering is stable even if the `Ecosystem` variant sequence changes.
fn ecosystem_primary_priority(e: Ecosystem) -> u8 {
    match e {
        Ecosystem::Cargo => 0,
        Ecosystem::Npm => 1,
        Ecosystem::Pypi => 2,
        _ => u8::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_pkg(root: &std::path::Path, rel: &str, eco: Ecosystem, name: &str) {
        std::fs::create_dir_all(root.join(rel)).unwrap();
        match eco {
            Ecosystem::Cargo => std::fs::write(
                root.join(rel).join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
            )
            .unwrap(),
            Ecosystem::Npm => std::fs::write(
                root.join(rel).join("package.json"),
                format!(r#"{{"name":"{name}","version":"0.1.0"}}"#),
            )
            .unwrap(),
            Ecosystem::Pypi => std::fs::write(
                root.join(rel).join("pyproject.toml"),
                format!("[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
            )
            .unwrap(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn ac16a_fixed_group_member_resolves_via_prefixed_unpromoted_cargo_package() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/foo", Ecosystem::Cargo, "foo");
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"g\"\nmembers = [\"cargo:foo\"]\n",
        )
        .unwrap();
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect(
            "Workspace::load must succeed for an unpromoted Cargo package referenced via cargo:foo",
        );
        let native = ws
            .graph
            .identity()
            .native
            .get(&(Ecosystem::Cargo, "foo".to_string()))
            .expect("native entry must exist");
        let prefixed = ws
            .graph
            .identity()
            .prefixed
            .get(&(Ecosystem::Cargo, "foo".to_string()))
            .expect("prefixed entry must exist for AC-02");
        assert_eq!(
            prefixed, native,
            "prefixed and native must resolve to the identical PackageId for an unpromoted single-ecosystem package"
        );
        let group = ws
            .config
            .groups
            .fixed
            .get(&callisto_model::GroupName("g".to_string()))
            .expect("group must exist");
        assert_eq!(group.members.len(), 1);
    }

    #[test]
    fn ac03_fixed_group_member_naming_absent_ecosystem_is_missing_group_member() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/foo", Ecosystem::Cargo, "foo");
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"g\"\nmembers = [\"npm:foo\"]\n",
        )
        .unwrap();
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let err = match crate::Workspace::load(root.to_path_buf(), &locator, &runner) {
            Err(e) => e,
            Ok(_) => panic!("expected MissingGroupMember error, got Ok"),
        };
        match err {
            GraphError::MissingGroupMember { member, .. } => {
                assert_eq!(member, "npm:foo");
            }
            other => panic!("expected MissingGroupMember, got {other:?}"),
        }
    }

    struct NoopRunner;
    impl CommandRunner for NoopRunner {
        fn run(
            &self,
            _program: &str,
            _args: &[&str],
            _cwd: &Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            panic!("this test's workspace build never needs to shell out");
        }
    }

    /// End-to-end: a real disk-discovered napi platform manifest (Case D --
    /// a `Cargo.toml` and a differently-named `package.json` sharing one
    /// directory, the platform npm package's own identity distinct from the
    /// owning crate's) must resolve through `[[fixed-group]] members`
    /// naming the platform package by its own npm name, via
    /// `IdentityIndex.platform` -- not the hand-constructed
    /// `GroupMember::PlatformManifest` fixtures other tests use, which
    /// bypass this wiring entirely and would not have caught the gap this
    /// test pins.
    #[test]
    fn real_platform_manifest_resolves_via_fixed_group_and_feeds_napi_drift() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"@myorg/my-crate-linux-x64-gnu","version":"0.1.0","os":["linux"],"cpu":["x64"]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"my-group\"\nmembers = [\"my-crate\", \"@myorg/my-crate-linux-x64-gnu\"]\n",
        )
        .unwrap();

        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner)
            .expect("group resolution must succeed now that the real platform manifest resolves");

        let group = ws
            .config
            .groups
            .fixed
            .get(&callisto_model::GroupName("my-group".to_string()))
            .expect("fixed group must exist");

        let platform_member = group
            .members
            .iter()
            .find(|m| {
                matches!(
                    m,
                    crate::config::groups::GroupMember::PlatformManifest { .. }
                )
            })
            .expect("platform member must resolve, not be silently dropped or errored");

        let crate::config::groups::GroupMember::PlatformManifest { role, name, .. } =
            platform_member
        else {
            unreachable!()
        };
        assert_eq!(name, "@myorg/my-crate-linux-x64-gnu");
        assert_eq!(
            crate::napi::role_to_triple(role).as_deref(),
            Some("x86_64-unknown-linux-gnu"),
            "role must be the real, disk-derived role, not the old hardcoded \
             platform=\"unknown\" stub -- got: {role:?}"
        );

        // Feed straight into napi_drift, matching the task's own framing:
        // "napi_drift receives real group members".
        let declared = vec!["x86_64-unknown-linux-gnu".to_string()];
        let diagnostics = crate::napi::napi_drift(group, &declared, root);
        assert!(
            diagnostics.is_empty(),
            "declared napi.targets matches the real group member; expected no drift \
             diagnostics, got: {diagnostics:?}"
        );
    }

    /// A real `optionalDependencies` edge onto a Case D platform package (a
    /// `Cargo.toml` and a differently-named `package.json` sharing one
    /// directory, per the test above) must resolve through `IdentityIndex.native`,
    /// keyed by the platform manifest's own npm name -- not the owning
    /// crate's `primary_id` name, which is what a sibling package's
    /// dependency entry never names. Before this fix, `index.native` was
    /// keyed by `primary_id.name()` for every ecosystem in a Case D
    /// directory, so a dependency naming the platform package by its real
    /// npm name silently failed to resolve and the edge was dropped with no
    /// diagnostic.
    #[test]
    fn optional_dependency_on_case_d_platform_package_resolves_via_native_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"@myorg/my-crate-linux-x64-gnu","version":"0.1.0","os":["linux"],"cpu":["x64"]}"#,
        )
        .unwrap();

        std::fs::create_dir_all(root.join("consumer")).unwrap();
        std::fs::write(
            root.join("consumer/package.json"),
            r#"{"name":"@myorg/consumer","version":"0.1.0","optionalDependencies":{"@myorg/my-crate-linux-x64-gnu":"0.1.0"}}"#,
        )
        .unwrap();

        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner)
            .expect("workspace load must succeed");

        let owning_crate = callisto_model::PackageId::Bare("my-crate".to_string());
        let consumer = callisto_model::PackageId::Bare("@myorg/consumer".to_string());

        let edge = ws.graph.edges().iter().find(|e| e.from == consumer).expect(
            "consumer's optionalDependencies edge onto the Case D platform package must \
                 resolve, not be silently dropped",
        );
        assert_eq!(
            edge.to, owning_crate,
            "the platform package belongs to the owning crate (Case D); the edge must resolve \
             to the owning crate's identity, not fail to resolve at all"
        );
    }

    /// Real disk-discovered Linux napi platform packages always have
    /// `abi: None` from `detect_npm_role` (npm's standard `os`/`cpu`
    /// manifest fields have no libc/ABI concept), but `napi.rs::role_to_triple`'s
    /// Linux match arms all require `Some("gnu")`/`Some("musl")`/
    /// `Some("gnueabihf")` -- so before this fix, `role_to_triple` could
    /// never resolve a real, disk-discovered Linux platform package to its
    /// triple, and `napi_drift` would spuriously report it as a
    /// declared-but-missing group member even though it's genuinely present
    /// on disk with the correct name.
    #[test]
    fn detect_npm_role_infers_linux_gnu_abi_from_package_name_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.json");
        std::fs::write(
            &path,
            r#"{"name":"@scope/my-lib-linux-x64-gnu","version":"1.0.0","os":["linux"],"cpu":["x64"]}"#,
        )
        .unwrap();

        let role = detect_npm_role(&path);

        assert_eq!(
            crate::napi::role_to_triple(&role).as_deref(),
            Some("x86_64-unknown-linux-gnu"),
            "expected a resolvable triple, got role: {role:?}"
        );
    }

    #[test]
    fn detect_npm_role_infers_linux_musl_abi_from_package_name_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.json");
        std::fs::write(
            &path,
            r#"{"name":"@scope/my-lib-linux-arm64-musl","version":"1.0.0","os":["linux"],"cpu":["arm64"]}"#,
        )
        .unwrap();

        let role = detect_npm_role(&path);

        assert_eq!(
            crate::napi::role_to_triple(&role).as_deref(),
            Some("aarch64-unknown-linux-musl"),
            "expected a resolvable triple, got role: {role:?}"
        );
    }

    #[test]
    fn detect_npm_role_infers_linux_gnueabihf_abi_from_package_name_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.json");
        std::fs::write(
            &path,
            r#"{"name":"@scope/my-lib-linux-arm-gnueabihf","version":"1.0.0","os":["linux"],"cpu":["arm"]}"#,
        )
        .unwrap();

        let role = detect_npm_role(&path);

        assert_eq!(
            crate::napi::role_to_triple(&role).as_deref(),
            Some("armv7-unknown-linux-gnueabihf"),
            "expected a resolvable triple, got role: {role:?}"
        );
    }

    /// Non-Linux platforms have no ABI concept in `role_to_triple`'s table
    /// (`abi` is always `None` there) -- must not be affected by the
    /// name-suffix inference at all.
    #[test]
    fn detect_npm_role_does_not_infer_abi_for_non_linux_platforms() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.json");
        std::fs::write(
            &path,
            r#"{"name":"@scope/my-lib-darwin-arm64-gnu","version":"1.0.0","os":["darwin"],"cpu":["arm64"]}"#,
        )
        .unwrap();

        let role = detect_npm_role(&path);

        assert_eq!(
            role,
            ManifestRole::Platform {
                platform: "darwin".to_string(),
                arch: "arm64".to_string(),
                abi: None,
            }
        );
    }

    /// A Linux platform package whose name has no recognized ABI suffix at
    /// all stays `abi: None` -- this function infers when it can, it
    /// doesn't invent an ABI that isn't actually signaled anywhere.
    #[test]
    fn detect_npm_role_leaves_abi_none_when_linux_name_has_no_recognized_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.json");
        std::fs::write(
            &path,
            r#"{"name":"@scope/my-lib-linux-x64","version":"1.0.0","os":["linux"],"cpu":["x64"]}"#,
        )
        .unwrap();

        let role = detect_npm_role(&path);

        assert_eq!(
            role,
            ManifestRole::Platform {
                platform: "linux".to_string(),
                arch: "x64".to_string(),
                abi: None,
            }
        );
    }
}
