use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use callisto_manifests::{detect_npm_workspace_kind, Manifest, OpenContext, WorkspaceCargoResolver};
use callisto_model::{
    CommandRunner, DepEdge, Diagnostic, DiagnosticCode, DiagnosticSeverity, Ecosystem, ManifestDecl, ManifestFormat,
    ManifestRole, Package, PackageId, PublishTarget, ReleaseTrigger,
};

use crate::config::resolve::resolve_package_config;
use crate::config::ResolvedConfig;
use crate::crosscheck::crosscheck_declared_edges;
use crate::error::GraphError;
use crate::identity::IdentityIndex;
use crate::locate::ProjectLocator;
use crate::manifest_cache::open_cached;
#[allow(unused_imports)]
use crate::resolver::{DependencyResolver, ManifestWalkResolver};

/// Returns (name-scoped claiming-ecosystem set, complete unfiltered native-key list)
/// for one path's own `list` of (ecosystem, declared-id) pairs. The first is filtered
/// to entries whose declared name equals `primary_id.name()`; the second is not filtered.
fn compute_claiming_ecosystems_and_native_keys(
    list: &[(Ecosystem, PackageId)],
    primary_id: &PackageId,
) -> (BTreeSet<Ecosystem>, Vec<(Ecosystem, String)>) {
    let claiming = list
        .iter()
        .filter(|(_, id)| id.name() == primary_id.name())
        .map(|(eco, _)| *eco)
        .collect();
    let native_keys = list.iter().map(|(eco, id)| (*eco, id.name().to_string())).collect();
    (claiming, native_keys)
}

/// The PROMOTION PREDICATE: true only when the two paths' name-scoped
/// claiming-ecosystem sets share no ecosystem. This is a disjointness test,
/// not an inequality test -- {Cargo,Npm} and {Npm} are unequal but not
/// disjoint, and must NOT promote (see AC-08).
fn claiming_sets_disjoint(a: &BTreeSet<Ecosystem>, b: &BTreeSet<Ecosystem>) -> bool {
    a.is_disjoint(b)
}

/// Returns true iff `paths.len() > 1` AND every distinct pair of paths in `paths`
/// has disjoint claiming-ecosystem sets in `claiming_ecosystems`.
///
/// Overlapping claiming sets indicate a true duplicate package error (handled
/// separately during graph construction), NOT a valid multi-ecosystem promotion.
#[cfg(test)]
fn is_promoted_bare_name(paths: &[PathBuf], claiming_ecosystems: &BTreeMap<PathBuf, BTreeSet<Ecosystem>>) -> bool {
    if paths.len() <= 1 {
        return false;
    }
    let mut seen = BTreeSet::new();
    for path in paths {
        if let Some(ecos) = claiming_ecosystems.get(path) {
            for eco in ecos {
                if !seen.insert(*eco) {
                    return false;
                }
            }
        }
    }
    true
}

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

        let mut package_manifest_decls: BTreeMap<PackageId, (PathBuf, Vec<ManifestDecl>)> = BTreeMap::new();
        let mut index = IdentityIndex::default();
        let mut diagnostics = Vec::new();
        let mut claiming_ecosystems: BTreeMap<PathBuf, BTreeSet<Ecosystem>> = BTreeMap::new();
        let mut path_native_keys: BTreeMap<PathBuf, Vec<(Ecosystem, String)>> = BTreeMap::new();
        let mut path_platform_keys: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
        let mut primary_ecosystems: BTreeMap<PathBuf, Ecosystem> = BTreeMap::new();
        let mut promoted_siblings: BTreeMap<String, Vec<(PackageId, BTreeSet<Ecosystem>)>> = BTreeMap::new();

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
            let mut primary_id = list[0].1.clone();
            let (this_claiming, this_native_keys) = compute_claiming_ecosystems_and_native_keys(&list, &primary_id);
            claiming_ecosystems.insert(rel_path.clone(), this_claiming.clone());
            path_native_keys.insert(rel_path.clone(), this_native_keys);
            primary_ecosystems.insert(rel_path.clone(), list[0].0);

            let mut branch_ii_promoted = false;
            if let Some(existing_members) = promoted_siblings.get(primary_id.name()) {
                let this_set = claiming_ecosystems.get(&rel_path).cloned().unwrap_or_default();
                let conflict = existing_members
                    .iter()
                    .find(|(_, member_set)| !claiming_sets_disjoint(&this_set, member_set));
                if let Some((conflicting_id, _)) = conflict {
                    let offending_path = package_manifest_decls
                        .iter()
                        .find(|(id, _)| *id == conflicting_id)
                        .map(|(_, (p, _))| p.clone())
                        .unwrap_or_default();
                    return Err(GraphError::DuplicatePackage {
                        id: primary_id,
                        paths: vec![offending_path, rel_path],
                    });
                }
                let promoted_id = PackageId::Prefixed {
                    ecosystem: primary_ecosystems[&rel_path],
                    name: primary_id.name().to_string(),
                };
                promoted_siblings
                    .entry(primary_id.name().to_string())
                    .or_default()
                    .push((promoted_id.clone(), this_set));
                primary_id = promoted_id;
                branch_ii_promoted = true;
            }

            if !branch_ii_promoted {
                index.bare.insert(primary_id.name().to_string(), primary_id.clone());
            }

            let mut decls = Vec::new();
            for (eco, id) in &list {
                let (fmt, filename) = match eco {
                    Ecosystem::Cargo => (ManifestFormat::CargoToml, "Cargo.toml"),
                    Ecosystem::Npm => (ManifestFormat::PackageJson, "package.json"),
                    Ecosystem::Pypi => (ManifestFormat::PyprojectToml, "pyproject.toml"),
                    _ => (ManifestFormat::PackageJson, "package.json"),
                };
                let manifest_rel = rel_path.join(filename);
                if let Ok(decl) = ManifestDecl::new(manifest_rel.clone(), ManifestRole::Canonical, fmt) {
                    decls.push(decl);
                }
                // For napi platform packages (os + cpu constraints in package.json),
                // also push a Platform-role decl so plan_publish can route them
                // into npm_platform_packages instead of npm_main_packages.
                if *eco == Ecosystem::Npm {
                    let role = detect_npm_role(&root.join(&manifest_rel));
                    if let ManifestRole::Platform { .. } = role {
                        if let Ok(platform_decl) = ManifestDecl::new(manifest_rel.clone(), role.clone(), fmt) {
                            decls.push(platform_decl);
                        }
                        // `id` is this manifest's own npm identity, resolved from its
                        // own `name` field -- distinct from `primary_id` whenever a
                        // higher-priority ecosystem (Cargo) shares this directory
                        // (Case D). Config authors reference the platform package by
                        // its own name in `[[fixed-group]] members`, so that must be
                        // the index key; `primary_id` is who it belongs to.
                        index
                            .platform
                            .insert(id.name().to_string(), (primary_id.clone(), manifest_rel, role));
                        path_platform_keys
                            .entry(rel_path.clone())
                            .or_default()
                            .push(id.name().to_string());
                    }
                }
                index.native.insert((*eco, id.name().to_string()), primary_id.clone());
                if id.name() == primary_id.name() {
                    index.prefixed.insert((*eco, id.name().to_string()), primary_id.clone());
                }
            }

            let current_decls = decls.clone();
            if let Some((existing_path, existing_decls)) =
                package_manifest_decls.insert(primary_id.clone(), (rel_path.clone(), decls))
            {
                let name = primary_id.name().to_string();
                let existing_set = claiming_ecosystems.get(&existing_path).cloned().unwrap_or_default();
                let current_set = claiming_ecosystems.get(&rel_path).cloned().unwrap_or_default();
                if !claiming_sets_disjoint(&existing_set, &current_set) {
                    return Err(GraphError::DuplicatePackage {
                        id: primary_id,
                        paths: vec![existing_path, rel_path],
                    });
                }
                // STALE-KEY REWRITE location (4): re-key package_manifest_decls under
                // each path's own newly-promoted Prefixed id, sourcing each path's own
                // decls -- captured via the `existing_decls` returned by `insert` above
                // and the `current_decls` clone taken before `insert` overwrote the map,
                // never via a `.get(&primary_id)` lookup after the fact (a lookup miss
                // there would silently substitute an empty Vec -- exactly the AC-04
                // manifests-bleed bug).
                let existing_id = PackageId::Prefixed {
                    ecosystem: primary_ecosystems[&existing_path],
                    name: name.clone(),
                };
                let current_id = PackageId::Prefixed {
                    ecosystem: primary_ecosystems[&rel_path],
                    name: name.clone(),
                };
                package_manifest_decls.insert(existing_id.clone(), (existing_path.clone(), existing_decls));
                package_manifest_decls.insert(current_id.clone(), (rel_path.clone(), current_decls));
                package_manifest_decls.remove(&primary_id);
                // STALE-KEY REWRITE location (1): once a bare name is promoted,
                // it MUST NOT remain in index.bare.
                index.bare.remove(&name);
                // STALE-KEY REWRITE location (2): update index.native values
                // for all native keys declared by either path so they map to
                // their path's newly-promoted Prefixed id, not the stale primary_id.
                if let Some(keys) = path_native_keys.get(&existing_path) {
                    for key in keys {
                        index.native.insert(key.clone(), existing_id.clone());
                    }
                }
                if let Some(keys) = path_native_keys.get(&rel_path) {
                    for key in keys {
                        index.native.insert(key.clone(), current_id.clone());
                    }
                }
                // STALE-KEY REWRITE location (3)/(5): update index.prefixed values
                // for the primary identities so they point to the newly-promoted
                // Prefixed id rather than the pre-promotion primary_id.
                for eco in claiming_ecosystems.get(&existing_path).cloned().unwrap_or_default() {
                    index.prefixed.insert((eco, name.clone()), existing_id.clone());
                }
                for eco in claiming_ecosystems.get(&rel_path).cloned().unwrap_or_default() {
                    index.prefixed.insert((eco, name.clone()), current_id.clone());
                }
                // STALE-KEY REWRITE location (6): update index.platform's owner-id
                // component for every platform manifest declared under either path,
                // so a platform sibling co-located with a promoted owner keeps
                // pointing at that owner's newly-promoted Prefixed id rather than
                // the stale pre-promotion primary_id.
                if let Some(keys) = path_platform_keys.get(&existing_path) {
                    for key in keys {
                        if let Some((_, manifest_rel, role)) = index.platform.get(key).cloned() {
                            index
                                .platform
                                .insert(key.clone(), (existing_id.clone(), manifest_rel, role));
                        }
                    }
                }
                if let Some(keys) = path_platform_keys.get(&rel_path) {
                    for key in keys {
                        if let Some((_, manifest_rel, role)) = index.platform.get(key).cloned() {
                            index
                                .platform
                                .insert(key.clone(), (current_id.clone(), manifest_rel, role));
                        }
                    }
                }
                promoted_siblings
                    .entry(name.clone())
                    .or_default()
                    .extend([(existing_id, existing_set), (current_id, current_set)]);
            }
        }

        let cfg = cfg.with_promoted_siblings(promoted_siblings);

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
            // in. `id` may be PackageId::Bare (unpromoted) or PackageId::Prefixed
            // (promoted, see SPEC-TRACK3B1-IDENTITY-PROMOTION-CORE); this is the
            // only place an ecosystem-prefixed [[package-set]] pattern has
            // anything to match against.
            let package_ecosystems: Vec<Ecosystem> = decls.iter().map(|d| d.ecosystem()).collect();

            // Two-pass specificity search for [[package]] rules (SPEC-002 AC-1/2/3).
            // Pass 1: find the first Prefixed rule (pattern.ecosystem().is_some())
            //         that matches this package's ID. Prefixed rules always win
            //         over Bare rules regardless of declaration order in callisto.toml.
            // Pass 2: only if pass 1 found nothing, find the first Bare rule
            //         (any rule, since no Prefixed rule matched, the first match
            //          is necessarily Bare) that matches this package's ID.
            // Within each pass, first-match-wins (TOML declaration order) applies.
            let pkg_override = resolve_package_config(&id, &cfg)?;

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
                    .find(|(pattern, _)| pattern.matches_in_ecosystems(id.name(), &package_ecosystems))
                    .map(|(_, cfg)| cfg)
            } else {
                None
            };

            let active_override = pkg_override.or(set_override);

            let release_trigger = active_override
                .and_then(|o| o.release_trigger)
                .unwrap_or(ReleaseTrigger::Changeset);

            let tag_template = active_override.and_then(|o| o.tag_template.clone());

            let changelog = if let Some(override_path) = active_override.and_then(|o| o.changelog.as_ref()) {
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
        // Packages-map keys may be PackageId::Bare or PackageId::Prefixed (a
        // promoted package, see SPEC-TRACK3B1-IDENTITY-PROMOTION-CORE). This loop
        // remains correct regardless: ecosystem information is always sourced from
        // pkg.canonical_manifests(), never from key.ecosystem(), and `pattern`
        // here is always unprefixed (prefixed rules `continue` above), so
        // pattern.matches(key) matches by name alone independent of whether `key`
        // itself is Bare or Prefixed. Do NOT use key.ecosystem().
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
                        ecosystems.iter().next().map(|e| e.prefix()).unwrap_or("cargo"),
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
                                    (inherited_dep.spec.clone(), inherited_dep.declared_in.to_path_buf())
                                } else {
                                    (entry.spec.clone(), decl.path.clone())
                                }
                            } else {
                                (entry.spec.clone(), decl.path.clone())
                            }
                        } else {
                            (entry.spec.clone(), decl.path.clone())
                        };

                        if let Some(to) =
                            index.resolve_native_with_fallback(decl.ecosystem(), &entry.name, &mut diagnostics)
                        {
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

    let has_os = map.get("os").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty());
    let has_cpu = map.get("cpu").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty());

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

    ManifestRole::Platform { platform, arch, abi }
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
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner)
            .expect("Workspace::load must succeed for an unpromoted Cargo package referenced via cargo:foo");
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
            .find(|m| matches!(m, crate::config::groups::GroupMember::PlatformManifest { .. }))
            .expect("platform member must resolve, not be silently dropped or errored");

        let crate::config::groups::GroupMember::PlatformManifest { role, name, .. } = platform_member else {
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
    /// `Cargo.toml` and differently-named `package.json` sharing one
    /// directory, per the test above) must resolve through
    /// `IdentityIndex.native`, keyed by the platform manifest's own npm
    /// name -- not the owning crate's `primary_id` name, which a
    /// sibling's dependency entry never names. Before this fix,
    /// `index.native` was keyed by `primary_id.name()` for every
    /// ecosystem in a Case D directory, so a dependency naming the
    /// platform package by its real npm name silently failed to resolve,
    /// dropping the edge with no diagnostic.
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
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load must succeed");

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

    #[test]
    fn claiming_ecosystems_is_name_scoped_not_full_path_ecosystem_set() {
        let list = vec![
            (Ecosystem::Cargo, PackageId::Bare("native-core".to_string())),
            (
                Ecosystem::Npm,
                PackageId::Bare("@myorg/native-core-linux-x64-gnu".to_string()),
            ),
        ];
        let primary_id = PackageId::Bare("native-core".to_string());
        let (claiming, native_keys) = compute_claiming_ecosystems_and_native_keys(&list, &primary_id);
        let mut expected = std::collections::BTreeSet::new();
        expected.insert(Ecosystem::Cargo);
        assert_eq!(
            claiming, expected,
            "npm Platform entry must NOT count toward the name-scoped claiming set"
        );
        assert_eq!(
            native_keys,
            vec![
                (Ecosystem::Cargo, "native-core".to_string()),
                (Ecosystem::Npm, "@myorg/native-core-linux-x64-gnu".to_string()),
            ],
            "path_native_keys must retain the COMPLETE unfiltered key set, unlike claiming_ecosystems"
        );
    }

    #[test]
    fn primary_ecosystems_records_the_actual_primary_not_an_arbitrary_set_member() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/foo", Ecosystem::Cargo, "foo");
        std::fs::write(
            root.join("crates/foo/package.json"),
            r#"{"name":"@myorg/foo","version":"0.1.0"}"#,
        )
        .unwrap();
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("Case D load must succeed");
        assert!(ws.graph.get(&PackageId::Bare("foo".to_string())).is_some());
    }

    #[test]
    fn is_promoted_when_multiple_paths_claim_same_bare_name_with_disjoint_ecosystems() {
        let p1 = PathBuf::from("crates/native-core");
        let p2 = PathBuf::from("packages/native-core");
        let mut claiming = std::collections::BTreeMap::new();
        let mut eco1 = std::collections::BTreeSet::new();
        eco1.insert(Ecosystem::Cargo);
        let mut eco2 = std::collections::BTreeSet::new();
        eco2.insert(Ecosystem::Npm);
        claiming.insert(p1.clone(), eco1);
        claiming.insert(p2.clone(), eco2);
        let paths = vec![p1, p2];
        assert!(is_promoted_bare_name(&paths, &claiming));
    }

    #[test]
    fn is_not_promoted_when_single_path_claims_bare_name() {
        let p1 = PathBuf::from("crates/native-core");
        let mut claiming = std::collections::BTreeMap::new();
        let mut eco1 = std::collections::BTreeSet::new();
        eco1.insert(Ecosystem::Cargo);
        claiming.insert(p1.clone(), eco1);
        let paths = vec![p1];
        assert!(!is_promoted_bare_name(&paths, &claiming));
    }

    #[test]
    fn is_not_promoted_when_multiple_paths_have_overlapping_ecosystems() {
        let p1 = PathBuf::from("crates/native-core");
        let p2 = PathBuf::from("crates/other-core");
        let mut claiming = std::collections::BTreeMap::new();
        let mut eco1 = std::collections::BTreeSet::new();
        eco1.insert(Ecosystem::Cargo);
        let mut eco2 = std::collections::BTreeSet::new();
        eco2.insert(Ecosystem::Cargo);
        claiming.insert(p1.clone(), eco1);
        claiming.insert(p2.clone(), eco2);
        let paths = vec![p1, p2];
        assert!(
            !is_promoted_bare_name(&paths, &claiming),
            "overlapping ecosystem sets must NOT trigger promotion; duplicate check handles it"
        );
    }

    #[test]
    fn promotion_predicate_is_disjointness_not_inequality() {
        let mut cargo_npm = BTreeSet::new();
        cargo_npm.insert(Ecosystem::Cargo);
        cargo_npm.insert(Ecosystem::Npm);
        let mut npm_only = BTreeSet::new();
        npm_only.insert(Ecosystem::Npm);
        let mut pypi_only = BTreeSet::new();
        pypi_only.insert(Ecosystem::Pypi);

        assert!(!claiming_sets_disjoint(&cargo_npm, &npm_only));
        assert!(claiming_sets_disjoint(&cargo_npm, &pypi_only));
        assert!(claiming_sets_disjoint(&npm_only, &pypi_only));
    }

    #[test]
    fn same_ecosystem_collision_still_errors_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/a", Ecosystem::Cargo, "dup");
        write_pkg(root, "crates/b", Ecosystem::Cargo, "dup");
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let err = match crate::Workspace::load(root.to_path_buf(), &locator, &runner) {
            Err(e) => e,
            Ok(_) => panic!("expected DuplicatePackage error, got Ok"),
        };
        match err {
            GraphError::DuplicatePackage { id, paths } => {
                assert_eq!(id, PackageId::Bare("dup".to_string()));
                assert_eq!(paths.len(), 2);
            }
            other => panic!("expected DuplicatePackage, got {other:?}"),
        }
    }

    #[test]
    fn case_d_colliding_with_third_disjoint_ecosystem_still_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/case-d", Ecosystem::Cargo, "hybrid");
        write_pkg(root, "crates/case-d", Ecosystem::Npm, "hybrid");
        write_pkg(root, "packages/npm-hybrid", Ecosystem::Npm, "hybrid");
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let err = match crate::Workspace::load(root.to_path_buf(), &locator, &runner) {
            Err(e) => e,
            Ok(_) => panic!("expected DuplicatePackage error, got Ok"),
        };
        match err {
            GraphError::DuplicatePackage { id, paths } => {
                assert_eq!(id, PackageId::Bare("hybrid".to_string()));
                assert_eq!(paths.len(), 2);
            }
            other => panic!("expected DuplicatePackage, got {other:?}"),
        }
    }

    #[test]
    fn disjoint_cross_ecosystem_collision_promotes_instead_of_duplicate_package() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
        write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");

        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner)
            .expect("disjoint cross-ecosystem collision must promote, not DuplicatePackage");

        let cargo_id = PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "native-core".to_string(),
        };
        let npm_id = PackageId::Prefixed {
            ecosystem: Ecosystem::Npm,
            name: "native-core".to_string(),
        };
        assert_eq!(ws.graph.packages().count(), 2);
        let cargo_pkg = ws.graph.get(&cargo_id).expect("Cargo-prefixed entry must exist");
        let npm_pkg = ws.graph.get(&npm_id).expect("Npm-prefixed entry must exist");
        assert_eq!(cargo_pkg.manifests.len(), 1);
        assert_eq!(
            cargo_pkg.manifests[0].path,
            PathBuf::from("crates/native-core/Cargo.toml")
        );
        assert_eq!(npm_pkg.manifests.len(), 1);
        assert_eq!(
            npm_pkg.manifests[0].path,
            PathBuf::from("packages/native-core/package.json")
        );
    }

    #[test]
    fn promoted_name_removed_from_index_bare() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
        write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("promotion must succeed");
        assert!(
            !ws.graph.identity().bare.contains_key("native-core"),
            "index.bare must not retain the stale pre-promotion key once both occurrences promote"
        );
        let unprefixed_lookup = ws.graph.identity().resolve_human("native-core", &[]);
        assert!(matches!(unprefixed_lookup, Err(GraphError::AmbiguousName { .. })));
    }

    #[test]
    fn promoted_native_values_point_to_prefixed_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
        write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("promotion must succeed");

        let cargo_native = ws
            .graph
            .identity()
            .native
            .get(&(Ecosystem::Cargo, "native-core".to_string()))
            .expect("Cargo native entry must exist");
        let npm_native = ws
            .graph
            .identity()
            .native
            .get(&(Ecosystem::Npm, "native-core".to_string()))
            .expect("Npm native entry must exist");

        assert_eq!(
            cargo_native,
            &PackageId::Prefixed {
                ecosystem: Ecosystem::Cargo,
                name: "native-core".to_string(),
            },
            "index.native value must point to the promoted Cargo-prefixed ID"
        );
        assert_eq!(
            npm_native,
            &PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name: "native-core".to_string(),
            },
            "index.native value must point to the promoted Npm-prefixed ID"
        );
    }

    #[test]
    fn promoted_prefixed_values_point_to_prefixed_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
        write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("promotion must succeed");

        let cargo_prefixed = ws
            .graph
            .identity()
            .prefixed
            .get(&(Ecosystem::Cargo, "native-core".to_string()))
            .expect("Cargo prefixed entry must exist");
        let npm_prefixed = ws
            .graph
            .identity()
            .prefixed
            .get(&(Ecosystem::Npm, "native-core".to_string()))
            .expect("Npm prefixed entry must exist");

        assert_eq!(
            cargo_prefixed,
            &PackageId::Prefixed {
                ecosystem: Ecosystem::Cargo,
                name: "native-core".to_string(),
            },
            "index.prefixed value must point to the promoted Cargo-prefixed ID"
        );
        assert_eq!(
            npm_prefixed,
            &PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name: "native-core".to_string(),
            },
            "index.prefixed value must point to the promoted Npm-prefixed ID"
        );
    }

    /// A platform npm manifest co-located with a Cargo owner (Case D) whose
    /// owner later gets promoted via a disjoint cross-ecosystem bare-name
    /// collision elsewhere in the workspace: `index.platform`'s stored
    /// owner id must be rewritten to the promoted Prefixed id, matching the
    /// same rewrite already applied to `index.bare`/`index.native`/
    /// `index.prefixed` for this exact scenario.
    #[test]
    fn promoted_platform_index_value_points_to_prefixed_owner_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
        std::fs::write(
            root.join("crates/native-core/package.json"),
            r#"{"name":"@myorg/native-core-linux-x64-gnu","version":"0.1.0","os":["linux"],"cpu":["x64"]}"#,
        )
        .unwrap();
        write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");

        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("promotion must succeed");

        let platform_entry = ws
            .graph
            .identity()
            .platform
            .get("@myorg/native-core-linux-x64-gnu")
            .expect("platform entry must exist");

        assert_eq!(
            platform_entry.0,
            PackageId::Prefixed {
                ecosystem: Ecosystem::Cargo,
                name: "native-core".to_string(),
            },
            "index.platform's owner id must point to the promoted Cargo-prefixed id, \
             not the stale pre-promotion bare id"
        );
    }

    #[test]
    fn unpromoted_standalone_cargo_package_retains_bare_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/single", Ecosystem::Cargo, "single");
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner)
            .expect("standalone package load must succeed");
        let bare_id = PackageId::Bare("single".to_string());
        assert!(
            ws.graph.get(&bare_id).is_some(),
            "unpromoted single-ecosystem package must register under Bare ID"
        );
        let prefixed_id = PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "single".to_string(),
        };
        assert!(
            ws.graph.get(&prefixed_id).is_none(),
            "unpromoted package must NOT register under Prefixed ID directly"
        );
        assert_eq!(
            ws.graph.identity().bare.get("single"),
            Some(&bare_id),
            "index.bare must point to the Bare ID"
        );
        let resolved = ws
            .graph
            .identity()
            .resolve_human("cargo:single", &[])
            .expect("cargo:single must resolve");
        assert_eq!(
            resolved, bare_id,
            "cargo:single human lookup must resolve to the Bare ID"
        );
    }

    #[test]
    fn case_d_single_path_multi_ecosystem_retains_bare_id_and_preserves_platform() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/hybrid", Ecosystem::Cargo, "hybrid");
        std::fs::write(
            root.join("crates/hybrid/package.json"),
            r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.1.0","os":["darwin"],"cpu":["arm64"]}"#,
        )
        .unwrap();

        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("Case D load must succeed");

        let bare_id = PackageId::Bare("hybrid".to_string());
        assert!(
            ws.graph.get(&bare_id).is_some(),
            "Case D primary package must register under Bare ID"
        );
        let platform_entry = ws
            .graph
            .identity()
            .platform
            .get("@myorg/hybrid-darwin-arm64")
            .expect("platform entry must exist in IdentityIndex.platform");
        assert_eq!(platform_entry.0, bare_id, "platform entry owner must be the Bare ID");
        assert_eq!(platform_entry.1, PathBuf::from("crates/hybrid/package.json"));
    }

    #[test]
    fn promoted_index_prefixed_holds_two_distinct_ids_not_stale_bare() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
        write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("promotion must succeed");
        let cargo_id = ws
            .graph
            .identity()
            .prefixed
            .get(&(Ecosystem::Cargo, "native-core".to_string()))
            .expect("prefixed Cargo entry must exist");
        let npm_id = ws
            .graph
            .identity()
            .prefixed
            .get(&(Ecosystem::Npm, "native-core".to_string()))
            .expect("prefixed Npm entry must exist");
        assert_eq!(
            *cargo_id,
            PackageId::Prefixed {
                ecosystem: Ecosystem::Cargo,
                name: "native-core".to_string()
            }
        );
        assert_eq!(
            *npm_id,
            PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name: "native-core".to_string()
            }
        );
        assert_ne!(
            cargo_id, npm_id,
            "a distinct-id count of 2 is required for ResolvedConfig.promoted_siblings' derivation (AC-23) to retain this name"
        );
    }

    #[test]
    fn tri_fixture_rejects_only_the_offending_member_and_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "aaa/tri", Ecosystem::Cargo, "tri");
        std::fs::write(root.join("aaa/tri/package.json"), r#"{"name":"tri","version":"0.1.0"}"#).unwrap();
        write_pkg(root, "bbb/tri", Ecosystem::Pypi, "tri");
        write_pkg(root, "ccc/tri", Ecosystem::Npm, "tri");
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let err = match crate::Workspace::load(root.to_path_buf(), &locator, &runner) {
            Err(e) => e,
            Ok(_) => panic!("expected DuplicatePackage error, got Ok"),
        };
        match err {
            GraphError::DuplicatePackage { id, paths } => {
                assert_eq!(id, PackageId::Bare("tri".to_string()));
                assert!(paths.contains(&PathBuf::from("aaa/tri")));
                assert!(paths.contains(&PathBuf::from("ccc/tri")));
                assert!(
                    !paths.contains(&PathBuf::from("bbb/tri")),
                    "the non-offending group member bbb/tri must not be named"
                );
            }
            other => panic!("expected DuplicatePackage, got {other:?}"),
        }
    }

    #[test]
    fn third_path_joins_already_promoted_group() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "aaa", Ecosystem::Cargo, "multi");
        write_pkg(root, "bbb", Ecosystem::Npm, "multi");
        write_pkg(root, "ccc", Ecosystem::Pypi, "multi");
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("N=3 promotion must succeed");
        assert_eq!(ws.graph.packages().count(), 3);
        for eco in [Ecosystem::Cargo, Ecosystem::Npm, Ecosystem::Pypi] {
            let id = PackageId::Prefixed {
                ecosystem: eco,
                name: "multi".to_string(),
            };
            let pkg = ws.graph.get(&id).unwrap_or_else(|| panic!("{eco:?} entry must exist"));
            assert_eq!(
                pkg.manifests.len(),
                1,
                "{eco:?} entry must not bleed in another path's manifest"
            );
        }
    }

    #[test]
    fn third_path_joins_already_promoted_group_reverse_iteration_order() {
        // Same three ecosystems as `third_path_joins_already_promoted_group`, but
        // path names are chosen so BTreeMap's lexicographic by-path iteration
        // visits Pypi first, then Npm, then Cargo -- the reverse ecosystem
        // sequence -- proving the outcome is order-independent (AC-17a).
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "path-a-pypi", Ecosystem::Pypi, "multi2");
        write_pkg(root, "path-b-npm", Ecosystem::Npm, "multi2");
        write_pkg(root, "path-c-cargo", Ecosystem::Cargo, "multi2");
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner)
            .expect("N=3 promotion must succeed regardless of by-path iteration order");
        assert_three_promoted_singletons(&ws, "multi2");
    }

    fn assert_three_promoted_singletons(ws: &crate::Workspace<NoopRunner>, name: &str) {
        assert_eq!(ws.graph.packages().count(), 3);
        for eco in [Ecosystem::Cargo, Ecosystem::Npm, Ecosystem::Pypi] {
            let id = PackageId::Prefixed {
                ecosystem: eco,
                name: name.to_string(),
            };
            let pkg = ws.graph.get(&id).unwrap_or_else(|| panic!("{eco:?} entry must exist"));
            assert_eq!(
                pkg.manifests.len(),
                1,
                "{eco:?} entry must not bleed in another path's manifest"
            );
        }
    }

    #[test]
    fn maturin_pyo3_layout_promotes_cargo_and_pypi_siblings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "bindings/rust", Ecosystem::Cargo, "mylib");
        write_pkg(root, "bindings/python", Ecosystem::Pypi, "mylib");
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws =
            crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("Cargo/Pypi promotion must succeed");
        let cargo_pkg = ws
            .graph
            .get(&PackageId::Prefixed {
                ecosystem: Ecosystem::Cargo,
                name: "mylib".to_string(),
            })
            .expect("Cargo entry must exist");
        let pypi_pkg = ws
            .graph
            .get(&PackageId::Prefixed {
                ecosystem: Ecosystem::Pypi,
                name: "mylib".to_string(),
            })
            .expect("Pypi entry must exist");
        assert_eq!(cargo_pkg.manifests.len(), 1);
        assert_eq!(cargo_pkg.manifests[0].path, PathBuf::from("bindings/rust/Cargo.toml"));
        assert_eq!(pypi_pkg.manifests.len(), 1);
        assert_eq!(
            pypi_pkg.manifests[0].path,
            PathBuf::from("bindings/python/pyproject.toml")
        );
    }

    #[test]
    fn case_d_package_with_no_collision_stays_bare() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/foo", Ecosystem::Cargo, "foo");
        std::fs::write(
            root.join("crates/foo/package.json"),
            r#"{"name":"@myorg/foo","version":"0.1.0"}"#,
        )
        .unwrap();
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("Case D load must succeed");
        assert_eq!(ws.graph.packages().count(), 1);
        let pkg = ws
            .graph
            .get(&PackageId::Bare("foo".to_string()))
            .expect("single Bare(foo) entry must exist, unpromoted");
        assert_eq!(
            pkg.manifests.len(),
            2,
            "both the Cargo and npm manifest belong to the one Case D package"
        );
    }

    #[test]
    fn ac12_ac18_npm_consumer_depending_on_cargo_only_name_gets_no_edge_and_diagnostic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/foo", Ecosystem::Cargo, "foo");
        std::fs::create_dir_all(root.join("packages/consumer")).unwrap();
        std::fs::write(
            root.join("packages/consumer/package.json"),
            r#"{"name":"consumer","version":"0.1.0","dependencies":{"foo":"^1.0.0"}}"#,
        )
        .unwrap();
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner)
            .expect("load must succeed even with an unresolved dependency name");
        let cargo_foo = PackageId::Bare("foo".to_string());
        assert!(
            !ws.graph.edges().iter().any(|e| e.to == cargo_foo),
            "no DepEdge must be created to the Cargo foo id from an npm consumer's same-name miss"
        );
        let diag = ws
            .graph
            .diagnostics()
            .iter()
            .find(|d| {
                d.code == callisto_model::DiagnosticCode::UnknownPackage
                    && d.severity == callisto_model::DiagnosticSeverity::Warning
                    && d.message.contains("foo")
            })
            .expect("an UnknownPackage warning diagnostic naming foo must be present");
        assert!(diag.message.contains("ambiguous") || diag.message.contains("cargo:foo"));
    }

    #[test]
    fn ac14_ac18_npm_consumer_depending_on_serde_with_cargo_serde_present_gets_no_edge_and_diagnostic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/serde", Ecosystem::Cargo, "serde");
        std::fs::create_dir_all(root.join("packages/consumer")).unwrap();
        std::fs::write(
            root.join("packages/consumer/package.json"),
            r#"{"name":"consumer","version":"0.1.0","dependencies":{"serde":"^1"}}"#,
        )
        .unwrap();
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("load must succeed");
        let cargo_serde = PackageId::Bare("serde".to_string());
        assert!(!ws.graph.edges().iter().any(|e| e.to == cargo_serde));
        let diag = ws
            .graph
            .diagnostics()
            .iter()
            .find(|d| {
                d.code == callisto_model::DiagnosticCode::UnknownPackage
                    && d.severity == callisto_model::DiagnosticSeverity::Warning
                    && d.message.contains("serde")
            })
            .expect("an UnknownPackage warning diagnostic naming serde must be present");
        assert!(diag.message.contains("ambiguous") || diag.message.contains("cargo:serde"));
    }

    #[test]
    fn ac15_ac18_npm_consumer_depending_on_ambiguous_lib_with_cargo_and_pypi_present_gets_exactly_one_diagnostic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/ambiguous-lib", Ecosystem::Cargo, "ambiguous-lib");
        write_pkg(root, "py/ambiguous-lib", Ecosystem::Pypi, "ambiguous-lib");
        std::fs::create_dir_all(root.join("packages/consumer")).unwrap();
        std::fs::write(
            root.join("packages/consumer/package.json"),
            r#"{"name":"consumer","version":"0.1.0","dependencies":{"ambiguous-lib":"^1"}}"#,
        )
        .unwrap();
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("load must succeed");
        let diags: Vec<_> = ws
            .graph
            .diagnostics()
            .iter()
            .filter(|d| d.code == callisto_model::DiagnosticCode::UnknownPackage && d.message.contains("ambiguous-lib"))
            .collect();
        assert_eq!(
            diags.len(),
            1,
            "exactly one diagnostic, not one per candidate ecosystem"
        );
    }

    #[test]
    fn unprefixed_package_rule_matching_two_promoted_siblings_is_ambiguous() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
        write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");
        std::fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"native-core\"\nrelease-trigger = \"auto\"\n",
        )
        .unwrap();
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let err = match crate::Workspace::load(root.to_path_buf(), &locator, &runner) {
            Err(e) => e,
            Ok(_) => panic!("expected AmbiguousName error, got Ok"),
        };
        match err {
            GraphError::AmbiguousName { name, candidates } => {
                assert_eq!(name, "native-core");
                assert_eq!(candidates.len(), 2);
                assert!(candidates.contains(&PackageId::Prefixed {
                    ecosystem: Ecosystem::Cargo,
                    name: "native-core".to_string()
                }));
                assert!(candidates.contains(&PackageId::Prefixed {
                    ecosystem: Ecosystem::Npm,
                    name: "native-core".to_string()
                }));
            }
            other => panic!("expected AmbiguousName, got {other:?}"),
        }
    }

    #[test]
    fn prefixed_package_rules_still_apply_correctly_to_promoted_siblings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
        write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");
        std::fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"cargo:native-core\"\nrelease-trigger = \"changeset\"\n\n[[package]]\nmatch = \"npm:native-core\"\nrelease-trigger = \"auto\"\n",
        )
        .unwrap();
        let locator = crate::locate::IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner)
            .expect("prefixed rules must not trigger the ambiguity check");
        let cargo_pkg = ws
            .graph
            .get(&PackageId::Prefixed {
                ecosystem: Ecosystem::Cargo,
                name: "native-core".to_string(),
            })
            .unwrap();
        let npm_pkg = ws
            .graph
            .get(&PackageId::Prefixed {
                ecosystem: Ecosystem::Npm,
                name: "native-core".to_string(),
            })
            .unwrap();
        assert_eq!(cargo_pkg.release_trigger, ReleaseTrigger::Changeset);
        assert_eq!(npm_pkg.release_trigger, ReleaseTrigger::Auto);
    }
}
