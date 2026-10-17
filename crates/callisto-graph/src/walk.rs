use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use callisto_manifests::{Manifest, OpenContext};
#[cfg(test)]
use callisto_model::ReleaseTrigger;
use callisto_model::{
    CommandRunner, DepEdge, DepSpec, Diagnostic, DiagnosticCode, DiagnosticSeverity, Ecosystem, ManifestDecl,
    ManifestFormat, ManifestRole, Package, PackageId, PublishTarget,
};

use crate::config::resolve::{resolve_package_config, rule_applies};
use crate::config::ResolvedConfig;
use crate::error::GraphError;
use crate::identity::IdentityIndex;
use crate::locate::ProjectLocator;
use crate::manifest_cache::open_cached;

fn relative_workspace_target(root: &Path, declaring_manifest: &Path, relative: &str) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let source = root.join(declaring_manifest).parent()?.canonicalize().ok()?;
    let target = source.join(relative).canonicalize().ok()?;
    if !target.starts_with(&root) {
        return None;
    }
    Some(target)
}
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

/// Disjointness test for two paths' name-scoped claiming-ecosystem sets --
/// {Cargo,Npm} and {Npm} are unequal but not disjoint. Kept for the
/// `is_promoted_bare_name` unit tests below; actual duplicate detection is
/// `native_owner` in `build`, keyed by every declared (ecosystem, name), not
/// just each path's primary name.
#[cfg(test)]
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

/// A manifest the graph build could not resolve. Dropping it silently would shrink the release without a trace, so it
/// is reported as an error-level diagnostic that makes loading the workspace for planning fail.
fn unresolved(manifest: &Path, what: &str, cause: impl std::fmt::Display) -> Diagnostic {
    // `./package.json` and `package.json` are one manifest.
    let manifest: PathBuf = manifest
        .components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .collect();
    let manifest = manifest.as_path();
    Diagnostic {
        code: DiagnosticCode::ManifestUnresolved,
        severity: DiagnosticSeverity::Error,
        message: format!("`{}`: could not {what}: {cause}", manifest.display()),
        package: None,
        path: Some(manifest.to_path_buf()),
        escalated_by: None,
        governed_by: None,
    }
}

impl ManifestWalkResolver {
    pub fn build<L: ProjectLocator, R: CommandRunner>(
        root: &Path,
        locator: &L,
        _runner: &R,
        cfg: &ResolvedConfig,
        manifest_cache: &RefCell<BTreeMap<PathBuf, Arc<dyn Manifest>>>,
    ) -> Result<Self, GraphError> {
        let ctx = OpenContext::for_workspace_root(root)?;
        let mut diagnostics = Vec::new();

        let discovered = discover(root, locator, manifest_cache, &ctx, &mut diagnostics)?;
        let Identified {
            package_manifest_decls,
            index,
            promoted_siblings,
            unresolved,
        } = assign_identities(discovered, manifest_cache, &ctx)?;
        diagnostics.extend(unresolved);

        let cfg = cfg.with_promoted_siblings(promoted_siblings);

        let packages = resolve_packages(package_manifest_decls, &cfg, manifest_cache, &ctx, &mut diagnostics)?;

        let (edges, out_index, in_index) = build_edges(root, &packages, &index, manifest_cache, &ctx, &mut diagnostics);

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

/// What the locator found: every project's identities by directory, and the npm platform packages attached to an
/// owner directory instead of standing as packages of their own.
struct Discovered {
    by_path: BTreeMap<PathBuf, Vec<(Ecosystem, PackageId)>>,
    attached_platforms: BTreeMap<PathBuf, (PathBuf, String, ManifestRole)>,
}

/// Each package's manifests under its final identity, and the index that resolves every name to that identity.
struct Identified {
    package_manifest_decls: BTreeMap<PackageId, (PathBuf, Vec<ManifestDecl>)>,
    index: IdentityIndex,
    /// An output of identity assignment: the sibling ids a bare name was promoted into.
    promoted_siblings: BTreeMap<String, Vec<(PackageId, BTreeSet<Ecosystem>)>>,
    /// Manifests that could not be opened while assigning identities.
    unresolved: Vec<Diagnostic>,
}

fn discover<L: ProjectLocator>(
    root: &Path,
    locator: &L,
    manifest_cache: &RefCell<BTreeMap<PathBuf, Arc<dyn Manifest>>>,
    ctx: &OpenContext<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Discovered, GraphError> {
    let (projects, platform_candidates) = locator.projects_and_platform_candidates()?;

    // The locator already parsed each manifest, so reuse `proj.id`.
    let mut by_path: BTreeMap<PathBuf, Vec<(Ecosystem, PackageId)>> = BTreeMap::new();
    for proj in &projects {
        by_path
            .entry(proj.path.clone())
            .or_default()
            .push((proj.ecosystem, proj.id.clone()));
    }

    let attached_platforms = platform_owners(root, &by_path, &platform_candidates, manifest_cache, ctx, diagnostics);
    Ok(Discovered {
        by_path,
        attached_platforms,
    })
}

fn assign_identities(
    discovered: Discovered,
    manifest_cache: &RefCell<BTreeMap<PathBuf, Arc<dyn Manifest>>>,
    ctx: &OpenContext<'_>,
) -> Result<Identified, GraphError> {
    let Discovered {
        by_path,
        attached_platforms,
    } = discovered;
    let mut assigner = IdentityAssigner::new(manifest_cache, ctx);
    for (rel_path, list) in by_path {
        if attached_platforms.contains_key(&rel_path) {
            continue;
        }
        assigner.assign(rel_path, list)?;
    }
    assigner.attach_platforms(attached_platforms);
    Ok(assigner.finish())
}

/// The order-dependent identity assignment: whether a path's identity is bare or ecosystem-prefixed depends on the
/// names seen in earlier paths, so this is one incremental pass that owns all of its bookkeeping.
struct IdentityAssigner<'a> {
    manifest_cache: &'a RefCell<BTreeMap<PathBuf, Arc<dyn Manifest>>>,
    ctx: &'a OpenContext<'a>,
    package_manifest_decls: BTreeMap<PackageId, (PathBuf, Vec<ManifestDecl>)>,
    index: IdentityIndex,
    claiming_ecosystems: BTreeMap<PathBuf, BTreeSet<Ecosystem>>,
    // Every (ecosystem, native name) any path declares, anywhere in that path's
    // manifest list -- not only its primary name -- belongs to exactly one directory.
    native_owner: BTreeMap<(Ecosystem, String), PathBuf>,
    path_native_keys: BTreeMap<PathBuf, Vec<(Ecosystem, String)>>,
    path_platform_keys: BTreeMap<PathBuf, Vec<String>>,
    primary_ecosystems: BTreeMap<PathBuf, Ecosystem>,
    promoted_siblings: BTreeMap<String, Vec<(PackageId, BTreeSet<Ecosystem>)>>,
    unresolved: Vec<Diagnostic>,
}

impl<'a> IdentityAssigner<'a> {
    fn new(manifest_cache: &'a RefCell<BTreeMap<PathBuf, Arc<dyn Manifest>>>, ctx: &'a OpenContext<'a>) -> Self {
        IdentityAssigner {
            manifest_cache,
            ctx,
            package_manifest_decls: BTreeMap::new(),
            index: IdentityIndex::default(),
            claiming_ecosystems: BTreeMap::new(),
            native_owner: BTreeMap::new(),
            path_native_keys: BTreeMap::new(),
            path_platform_keys: BTreeMap::new(),
            primary_ecosystems: BTreeMap::new(),
            promoted_siblings: BTreeMap::new(),
            unresolved: Vec::new(),
        }
    }

    fn assign(&mut self, rel_path: PathBuf, mut list: Vec<(Ecosystem, PackageId)>) -> Result<(), GraphError> {
        // Explicit precedence: Cargo (0) > Npm (1) > Pypi (2) > others.
        // Do NOT rely on enum discriminant order -- sort by this named
        // priority function so the precedence survives future variant
        // additions to `Ecosystem`.
        list.sort_by_key(|a| ecosystem_primary_priority(a.0));

        // E100 whenever any (ecosystem, native name) this path declares -- primary
        // or not -- is already owned by a different path; names both. Checked before
        // any promotion bookkeeping so a dual-manifest directory's non-primary name
        // (e.g. an npm sibling of a Cargo-primary package) is covered too, not just
        // each path's own primary name.
        if let Some((eco, id)) = list.iter().find(|(eco, id)| {
            self.native_owner
                .get(&(*eco, id.name().to_string()))
                .is_some_and(|p| *p != rel_path)
        }) {
            let owner_path = self.native_owner[&(*eco, id.name().to_string())].clone();
            return Err(GraphError::DuplicatePackage {
                id: id.clone(),
                paths: vec![owner_path, rel_path],
            });
        }
        for (eco, id) in &list {
            self.native_owner
                .insert((*eco, id.name().to_string()), rel_path.clone());
        }

        let mut primary_id = list[0].1.clone();
        let (this_claiming, this_native_keys) = compute_claiming_ecosystems_and_native_keys(&list, &primary_id);
        self.claiming_ecosystems.insert(rel_path.clone(), this_claiming.clone());
        self.path_native_keys.insert(rel_path.clone(), this_native_keys);
        self.primary_ecosystems.insert(rel_path.clone(), list[0].0);

        let mut branch_ii_promoted = false;
        if self.promoted_siblings.contains_key(primary_id.name()) {
            let this_set = self.claiming_ecosystems.get(&rel_path).cloned().unwrap_or_default();
            let promoted_id = PackageId::Prefixed {
                ecosystem: self.primary_ecosystems[&rel_path],
                name: primary_id.name().to_string(),
            };
            self.promoted_siblings
                .entry(primary_id.name().to_string())
                .or_default()
                .push((promoted_id.clone(), this_set));
            primary_id = promoted_id;
            branch_ii_promoted = true;
        }

        if !branch_ii_promoted {
            self.index
                .bare
                .insert(primary_id.name().to_string(), primary_id.clone());
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
            let canonical_decl = match ManifestDecl::new(manifest_rel.clone(), ManifestRole::Canonical, fmt) {
                Ok(decl) => {
                    decls.push(decl.clone());
                    Some(decl)
                }
                Err(error) => {
                    self.unresolved
                        .push(unresolved(&manifest_rel, "declare its manifest", error));
                    None
                }
            };
            // For napi platform packages (os + cpu constraints in package.json),
            // also push a Platform-role decl so release derivation can route them
            // as platform publishes instead of main packages. Reuses
            // the canonical decl's manifest handle via the shared
            // `manifest_cache` (`npm_role()`) instead of a second, raw
            // `fs::read` of the same package.json -- the handle opened here
            // is the same one `publish_targets`/`iter_dependencies` reuse
            // further down.
            if *eco == Ecosystem::Npm {
                let opened = canonical_decl
                    .as_ref()
                    .map(|decl| open_cached(self.manifest_cache, decl, self.ctx));
                let manifest = match opened {
                    Some(Ok(manifest)) => Some(manifest),
                    Some(Err(error)) => {
                        self.unresolved.push(unresolved(
                            &manifest_rel,
                            "open its manifest to read its platform role",
                            error,
                        ));
                        None
                    }
                    None => None,
                };
                let role = manifest
                    .and_then(|m| m.npm_role())
                    .map(|npm_role| match npm_role {
                        callisto_manifests::NpmRole::Platform { platform, arch, abi } => {
                            ManifestRole::Platform { platform, arch, abi }
                        }
                    })
                    .unwrap_or(ManifestRole::Canonical);
                if let ManifestRole::Platform { .. } = role {
                    match ManifestDecl::new(manifest_rel.clone(), role.clone(), fmt) {
                        Ok(platform_decl) => decls.push(platform_decl),
                        Err(error) => {
                            self.unresolved
                                .push(unresolved(&manifest_rel, "declare its platform manifest", error))
                        }
                    }
                    // `id` is this manifest's own npm identity, resolved from its
                    // own `name` field -- distinct from `primary_id` whenever a
                    // higher-priority ecosystem (Cargo) shares this directory.
                    // Config authors reference the platform package by
                    // its own name in `[[fixed-group]] members`, so that must be
                    // the index key; `primary_id` is who it belongs to.
                    self.index
                        .platform
                        .insert(id.name().to_string(), (primary_id.clone(), manifest_rel, role));
                    self.path_platform_keys
                        .entry(rel_path.clone())
                        .or_default()
                        .push(id.name().to_string());
                }
            }
            self.index
                .native
                .insert((*eco, id.name().to_string()), primary_id.clone());
            if id.name() == primary_id.name() {
                self.index
                    .prefixed
                    .insert((*eco, id.name().to_string()), primary_id.clone());
            }
        }

        let current_decls = decls.clone();
        if let Some((existing_path, existing_decls)) = self
            .package_manifest_decls
            .insert(primary_id.clone(), (rel_path.clone(), decls))
        {
            self.promotion_rekey(primary_id, existing_path, existing_decls, rel_path, current_decls);
        }
        Ok(())
    }

    /// A second path just claimed the bare name `primary_id`, so both paths are promoted to ecosystem-prefixed ids
    /// and every key already stored under the stale bare id is re-keyed.
    fn promotion_rekey(
        &mut self,
        primary_id: PackageId,
        existing_path: PathBuf,
        existing_decls: Vec<ManifestDecl>,
        rel_path: PathBuf,
        current_decls: Vec<ManifestDecl>,
    ) {
        // Both paths' claiming ecosystems are guaranteed disjoint here: an
        // overlap would already have failed at the native_owner check above.
        let name = primary_id.name().to_string();
        // STALE-KEY REWRITE location (4): re-key package_manifest_decls under
        // each path's own newly-promoted Prefixed id, sourcing each path's own
        // decls -- captured via the `existing_decls` returned by `insert` above
        // and the `current_decls` clone taken before `insert` overwrote the map,
        // never via a `.get(&primary_id)` lookup after the fact (a lookup miss
        // there would silently substitute an empty Vec -- the
        // manifests-bleed bug).
        let existing_id = PackageId::Prefixed {
            ecosystem: self.primary_ecosystems[&existing_path],
            name: name.clone(),
        };
        let current_id = PackageId::Prefixed {
            ecosystem: self.primary_ecosystems[&rel_path],
            name: name.clone(),
        };
        self.package_manifest_decls
            .insert(existing_id.clone(), (existing_path.clone(), existing_decls));
        self.package_manifest_decls
            .insert(current_id.clone(), (rel_path.clone(), current_decls));
        self.package_manifest_decls.remove(&primary_id);
        // STALE-KEY REWRITE location (1): once a bare name is promoted,
        // it MUST NOT remain in index.bare.
        self.index.bare.remove(&name);
        // STALE-KEY REWRITE location (2): update index.native values
        // for all native keys declared by either path so they map to
        // their path's newly-promoted Prefixed id, not the stale primary_id.
        if let Some(keys) = self.path_native_keys.get(&existing_path) {
            for key in keys {
                self.index.native.insert(key.clone(), existing_id.clone());
            }
        }
        if let Some(keys) = self.path_native_keys.get(&rel_path) {
            for key in keys {
                self.index.native.insert(key.clone(), current_id.clone());
            }
        }
        // STALE-KEY REWRITE location (3)/(5): update index.prefixed values
        // for the primary identities so they point to the newly-promoted
        // Prefixed id rather than the pre-promotion primary_id.
        for eco in self
            .claiming_ecosystems
            .get(&existing_path)
            .cloned()
            .unwrap_or_default()
        {
            self.index.prefixed.insert((eco, name.clone()), existing_id.clone());
        }
        for eco in self.claiming_ecosystems.get(&rel_path).cloned().unwrap_or_default() {
            self.index.prefixed.insert((eco, name.clone()), current_id.clone());
        }
        // STALE-KEY REWRITE location (6): update index.platform's owner-id
        // component for every platform manifest declared under either path,
        // so a platform sibling co-located with a promoted owner keeps
        // pointing at that owner's newly-promoted Prefixed id rather than
        // the stale pre-promotion primary_id.
        if let Some(keys) = self.path_platform_keys.get(&existing_path) {
            for key in keys {
                if let Some((_, manifest_rel, role)) = self.index.platform.get(key).cloned() {
                    self.index
                        .platform
                        .insert(key.clone(), (existing_id.clone(), manifest_rel, role));
                }
            }
        }
        if let Some(keys) = self.path_platform_keys.get(&rel_path) {
            for key in keys {
                if let Some((_, manifest_rel, role)) = self.index.platform.get(key).cloned() {
                    self.index
                        .platform
                        .insert(key.clone(), (current_id.clone(), manifest_rel, role));
                }
            }
        }
        let existing_set = self
            .claiming_ecosystems
            .get(&existing_path)
            .cloned()
            .unwrap_or_default();
        let current_set = self.claiming_ecosystems.get(&rel_path).cloned().unwrap_or_default();
        self.promoted_siblings
            .entry(name.clone())
            .or_default()
            .extend([(existing_id, existing_set), (current_id, current_set)]);
    }

    /// After every path is assigned, so each owner's id is final (post-promotion).
    fn attach_platforms(&mut self, attached_platforms: BTreeMap<PathBuf, (PathBuf, String, ManifestRole)>) {
        for (platform_dir, (owner_dir, name, role)) in attached_platforms {
            let manifest_rel = platform_dir.join("package.json");
            let owner = self.package_manifest_decls.iter_mut().find(|(_, (path, decls))| {
                *path == owner_dir && decls.iter().any(|d| d.format == ManifestFormat::PackageJson)
            });
            let Some((owner_id, (_, decls))) = owner else {
                self.unresolved.push(unresolved(
                    &manifest_rel,
                    "attach it to its owner",
                    format!("`{}` has no npm manifest to own it", owner_dir.display()),
                ));
                continue;
            };
            match ManifestDecl::new(manifest_rel.clone(), role.clone(), ManifestFormat::PackageJson) {
                Ok(decl) => {
                    decls.push(decl);
                    self.index.platform.insert(name, (owner_id.clone(), manifest_rel, role));
                }
                Err(error) => self
                    .unresolved
                    .push(unresolved(&manifest_rel, "declare its platform manifest", error)),
            }
        }
    }

    fn finish(self) -> Identified {
        Identified {
            package_manifest_decls: self.package_manifest_decls,
            index: self.index,
            promoted_siblings: self.promoted_siblings,
            unresolved: self.unresolved,
        }
    }
}

/// Applies `[[package]]` and `[[package-set]]` rules to each identified package, then reports rules that matched
/// nothing or matched across ecosystems.
fn resolve_packages(
    package_manifest_decls: BTreeMap<PackageId, (PathBuf, Vec<ManifestDecl>)>,
    cfg: &ResolvedConfig,
    manifest_cache: &RefCell<BTreeMap<PathBuf, Arc<dyn Manifest>>>,
    ctx: &OpenContext<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<BTreeMap<PackageId, Package>, GraphError> {
    // Tracks, per cfg.package_sets entry (by index), whether it matched at
    // least one real discovered package during this walk. A [[package-set]]
    // rule that matches nothing is almost always a typo or a stale
    // ecosystem prefix (see PackageSetMatchedNothing below) rather than
    // intentional, so it must be surfaced instead of silently ignored.
    let mut package_set_matched = vec![false; cfg.package_sets.len()];

    // Same tracking, for `[[package]]` rules: an exact-id rule that matches zero real
    // packages (e.g. an ecosystem prefix the package doesn't actually have) is just as
    // likely a typo as an empty `[[package-set]]` glob, so it gets the same diagnostic.
    let mut package_rule_matched = vec![false; cfg.packages.len()];

    let mut packages = BTreeMap::new();
    for (id, (rel_path, decls)) in package_manifest_decls {
        let ch_path = rel_path.join("CHANGELOG.md");
        let mut publish_to = Vec::new();
        // Attached platform manifests publish as their owner's platform operations, not as targets.
        for decl in decls.iter().filter(|d| d.role == ManifestRole::Canonical) {
            match open_cached(manifest_cache, decl, ctx) {
                Ok(editor) => {
                    for target in editor.publish_targets() {
                        if target != PublishTarget::None && !publish_to.contains(&target) {
                            publish_to.push(target);
                        }
                    }
                }
                Err(error) => diagnostics.push(unresolved(
                    &decl.path,
                    "open its manifest to read its publish targets",
                    error,
                )),
            }
        }
        if publish_to.is_empty() {
            publish_to.push(PublishTarget::None);
        }

        // The real ecosystem(s) this package's manifests were discovered
        // in. `id` may be PackageId::Bare (unpromoted) or PackageId::Prefixed
        // (promoted); this is the
        // only place an ecosystem-prefixed [[package-set]] pattern has
        // anything to match against.
        let package_ecosystems: Vec<Ecosystem> = decls.iter().map(|d| d.ecosystem()).collect();

        // Two-pass specificity search for [[package]] rules.
        // Pass 1: find the first Prefixed rule (pattern.ecosystem().is_some())
        //         that matches this package's ID. Prefixed rules always win
        //         over Bare rules regardless of declaration order in callisto.toml.
        // Pass 2: only if pass 1 found nothing, find the first Bare rule
        //         (any rule, since no Prefixed rule matched, the first match
        //          is necessarily Bare) that matches this package's ID.
        // Within each pass, first-match-wins (TOML declaration order) applies.
        let pkg_override = resolve_package_config(&id, &package_ecosystems, cfg)?;

        // Record which [[package-set]] patterns match this package,
        // independent of whether a [[package]] rule ends up shadowing the
        // fallback below — a pattern that is always shadowed still
        // "matched" for the purpose of the zero-match diagnostic.
        for (idx, (pattern, _)) in cfg.package_sets.iter().enumerate() {
            if pattern.matches_in_ecosystems(id.name(), &package_ecosystems) {
                package_set_matched[idx] = true;
            }
        }
        for (idx, (rule_id, _)) in cfg.packages.iter().enumerate() {
            if rule_applies(rule_id, &id, &package_ecosystems) {
                package_rule_matched[idx] = true;
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

        // Single source of truth for the default: ReleaseTrigger's #[default].
        let release_trigger = active_override.and_then(|o| o.release_trigger).unwrap_or_default();

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
            // The override replaces *which* targets the package publishes
            // to, but must not silently drop manifest-derived fields
            // (npm publishConfig.access/registry) the config itself
            // didn't set -- config wins when explicit, manifest fills gaps.
            publish_to = override_targets
                .iter()
                .map(|target| {
                    let manifest_target = target
                        .ecosystem()
                        .and_then(|eco| publish_to.iter().find(|m| m.ecosystem() == Some(eco)));
                    match manifest_target {
                        Some(manifest_target) => target.clone().merge_manifest_gaps(manifest_target),
                        None => target.clone(),
                    }
                })
                .collect();
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

    // A [[package]] rule that matched zero real packages is the same kind of mistake as an
    // empty [[package-set]] match above (e.g. `cargo/foo` when `foo` only exists in npm) --
    // advisory for the same reason: a monorepo-wide callisto.toml against a partial checkout
    // can legitimately match nothing.
    for (idx, (rule_id, _)) in cfg.packages.iter().enumerate() {
        if !package_rule_matched[idx] {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::PackageRuleMatchedNothing,
                severity: DiagnosticSeverity::Warning,
                message: format!("[[package]] `{rule_id}` matched no packages"),
                package: None,
                path: None,
                escalated_by: None,
                governed_by: None,
            });
        }
    }

    // Cross-ecosystem diagnostic pass.
    //
    // For each bare [[package]] rule in cfg.packages (pattern.ecosystem() == None),
    // compute the distinct-ecosystem set: the Ecosystem values found in the canonical
    // ManifestDecls of every packages-map entry matched by this rule.
    //
    // Packages-map keys may be PackageId::Bare or PackageId::Prefixed (a
    // promoted package). This loop
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
    // Prefixed [[package]] rules are skipped unconditionally.
    // [[package-set]] rules are never iterated here.
    for (pattern, _) in &cfg.packages {
        if pattern.ecosystem().is_some() {
            continue; // Prefixed rules never trigger this diagnostic.
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

    Ok(packages)
}

type EdgeIndex = BTreeMap<PackageId, Vec<usize>>;

/// One edge per dependency declaration of each canonical manifest that resolves to a workspace package.
fn build_edges(
    root: &Path,
    packages: &BTreeMap<PackageId, Package>,
    index: &IdentityIndex,
    manifest_cache: &RefCell<BTreeMap<PathBuf, Arc<dyn Manifest>>>,
    ctx: &OpenContext<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<DepEdge>, EdgeIndex, EdgeIndex) {
    let mut edges = Vec::new();
    let mut out_index: BTreeMap<PackageId, Vec<usize>> = BTreeMap::new();
    let mut in_index: BTreeMap<PackageId, Vec<usize>> = BTreeMap::new();

    for pkg in packages.values() {
        for decl in &pkg.manifests {
            if decl.role != ManifestRole::Canonical {
                continue;
            }
            let m = match open_cached(manifest_cache, decl, ctx) {
                Ok(m) => m,
                Err(error) => {
                    diagnostics.push(unresolved(
                        &decl.path,
                        "open its manifest to read its dependencies",
                        error,
                    ));
                    continue;
                }
            };
            {
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

                    let workspace_target = match &spec {
                        DepSpec::WorkspaceProtocol(workspace) => workspace
                            .relative_path()
                            .and_then(|relative| relative_workspace_target(root, &decl.path, relative))
                            .and_then(|target| {
                                packages.values().find(|package| {
                                    package.canonical_manifests().any(|manifest| {
                                        manifest.ecosystem() == Ecosystem::Npm
                                            && root
                                                .join(manifest.path.parent().unwrap_or(Path::new("")))
                                                .canonicalize()
                                                .ok()
                                                .as_ref()
                                                == Some(&target)
                                    })
                                })
                            })
                            .map(|package| package.id.clone()),
                        _ => None,
                    };
                    let to = match &spec {
                        DepSpec::WorkspaceProtocol(workspace) if workspace.relative_path().is_some() => {
                            workspace_target
                        }
                        _ => workspace_target.or_else(|| {
                            index
                                .resolve_native_with_fallback(decl.ecosystem(), &entry.package_name, diagnostics)
                                .cloned()
                        }),
                    };
                    if let Some(to) = to {
                        let idx = edges.len();
                        let edge = DepEdge {
                            from: pkg.id.clone(),
                            to: to.clone(),
                            name: entry.name,
                            kind: entry.kind,
                            spec,
                            from_manifest: declaring_path,
                            inherited: entry.inherited,
                            locator: entry.locator,
                        };
                        edges.push(edge);

                        out_index.entry(pkg.id.clone()).or_default().push(idx);
                        in_index.entry(to).or_default().push(idx);
                    } else if matches!(&spec, DepSpec::WorkspaceProtocol(workspace) if workspace.relative_path().is_some())
                    {
                        diagnostics.push(Diagnostic {
                                code: DiagnosticCode::UnknownPackage,
                                severity: DiagnosticSeverity::Warning,
                                message: format!(
                                    "relative pnpm workspace dependency `{}` from `{}` does not identify an admitted npm workspace member",
                                    entry.name,
                                    decl.path.display()
                                ),
                                package: Some(pkg.id.clone()),
                                path: Some(decl.path.clone()),
                                escalated_by: None,
                                governed_by: None,
                            });
                    }
                }
            }
        }
    }

    (edges, out_index, in_index)
}

/// Maps each npm-only platform package directory (`os`+`cpu`)
/// to the owner directory whose `package.json` names it in `optionalDependencies`,
/// plus the platform's own name and role. `non_members` are platform packages
/// outside the npm workspace, attachable but otherwise not discovered. A member
/// platform named by no owner, or by more than one, stays its own package with a
/// diagnostic.
fn platform_owners(
    root: &Path,
    by_path: &BTreeMap<PathBuf, Vec<(Ecosystem, PackageId)>>,
    non_members: &[callisto_model::ProjectRoot],
    manifest_cache: &RefCell<BTreeMap<PathBuf, Arc<dyn Manifest>>>,
    ctx: &OpenContext<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> BTreeMap<PathBuf, (PathBuf, String, ManifestRole)> {
    let open_npm = |rel_path: &Path| -> Result<Arc<dyn Manifest>, Diagnostic> {
        let manifest_rel = rel_path.join("package.json");
        let decl = ManifestDecl::new(
            manifest_rel.clone(),
            ManifestRole::Canonical,
            ManifestFormat::PackageJson,
        )
        .map_err(|error| unresolved(&manifest_rel, "declare its manifest", error))?;
        open_cached(manifest_cache, &decl, ctx)
            .map_err(|error| unresolved(&manifest_rel, "open its manifest to read its platform role", error))
    };
    let platform_role = |manifest: &Arc<dyn Manifest>| match manifest.npm_role()? {
        callisto_manifests::NpmRole::Platform { platform, arch, abi } => {
            Some(ManifestRole::Platform { platform, arch, abi })
        }
    };

    let mut platforms = Vec::new();
    let mut owners_of: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for (rel_path, list) in by_path {
        let Some((_, id)) = list.iter().find(|(eco, _)| *eco == Ecosystem::Npm) else {
            continue;
        };
        let manifest = match open_npm(rel_path) {
            Ok(manifest) => manifest,
            Err(problem) => {
                diagnostics.push(problem);
                continue;
            }
        };
        // A co-located platform manifest already belongs to its directory's package.
        match platform_role(&manifest) {
            Some(role) if list.len() == 1 => platforms.push((rel_path.clone(), id.name().to_string(), role, true)),
            _ => {
                for dep in manifest.iter_dependencies() {
                    if dep.kind == callisto_model::DepKind::Optional {
                        owners_of.entry(dep.name).or_default().push(rel_path.clone());
                    }
                }
            }
        }
    }
    for candidate in non_members {
        if by_path.contains_key(&candidate.path) {
            continue;
        }
        // A Cargo/Python package physically sits beside this candidate's package.json even
        // though it wasn't discovered as a workspace member here (e.g. filtered out by that
        // ecosystem's own membership) -- attaching it as a bare platform manifest would
        // misattach a directory that already hosts a real, distinct package.
        let beside = |name: &str| callisto_model::strict_fs::exists(&root.join(&candidate.path).join(name));
        match (beside("Cargo.toml"), beside("pyproject.toml")) {
            (Ok(false), Ok(false)) => {}
            (Err(error), _) | (_, Err(error)) => {
                diagnostics.push(unresolved(
                    &candidate.path,
                    "check whether another package shares its directory",
                    error,
                ));
                continue;
            }
            _ => continue,
        }
        let role = match open_npm(&candidate.path) {
            Ok(manifest) => platform_role(&manifest),
            Err(problem) => {
                diagnostics.push(problem);
                None
            }
        };
        if let Some(role) = role {
            platforms.push((candidate.path.clone(), candidate.id.name().to_string(), role, false));
        }
    }

    let mut attached = BTreeMap::new();
    for (rel_path, name, role, member) in platforms {
        match owners_of.get(&name).map(Vec::as_slice) {
            Some([owner]) => {
                attached.insert(rel_path, (owner.clone(), name, role));
            }
            None if !member => {}
            owners => {
                let reason = match owners {
                    Some(owners) => format!(
                        "is named in the optionalDependencies of {} packages ({})",
                        owners.len(),
                        owners
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    None => "is not named in any npm package's optionalDependencies".to_string(),
                };
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::PlatformPackageWithoutOwner,
                    severity: DiagnosticSeverity::Warning,
                    message: format!(
                        "npm platform package `{name}` {reason}; {}",
                        if member {
                            "it is released as its own package instead of as a platform manifest of its owner"
                        } else {
                            "it is outside the npm workspace, so it is not released at all"
                        }
                    ),
                    package: None,
                    path: Some(rel_path.join("package.json")),
                    escalated_by: None,
                    governed_by: None,
                });
            }
        }
    }
    attached
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
mod tests;
