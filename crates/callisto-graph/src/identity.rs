use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use callisto_model::{Ecosystem, ManifestRole, PackageId};

use crate::error::GraphError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_human_finds_prefixed_entry_for_unpromoted_package() {
        let mut index = IdentityIndex::default();
        let id = PackageId::Bare("foo".to_string());
        index.bare.insert("foo".to_string(), id.clone());
        index.native.insert((Ecosystem::Cargo, "foo".to_string()), id.clone());
        index.prefixed.insert((Ecosystem::Cargo, "foo".to_string()), id.clone());
        let resolved = index
            .resolve_human("cargo:foo", &[])
            .expect("cargo:foo must resolve via prefixed map");
        assert_eq!(resolved, id);
    }

    #[test]
    fn resolve_human_unknown_ecosystem_prefix_falls_through_to_unknown() {
        let mut index = IdentityIndex::default();
        let id = PackageId::Bare("foo".to_string());
        index.bare.insert("foo".to_string(), id.clone());
        index.native.insert((Ecosystem::Cargo, "foo".to_string()), id.clone());
        index.prefixed.insert((Ecosystem::Cargo, "foo".to_string()), id);
        let err = index.resolve_human("npm:foo", &[]).unwrap_err();
        assert!(
            matches!(err, GraphError::UnknownPackage { .. }),
            "expected UnknownPackage, got {err:?}"
        );
    }

    #[test]
    fn resolve_native_with_fallback_returns_none_on_single_cross_ecosystem_candidate() {
        let mut index = IdentityIndex::default();
        let cargo_serde = PackageId::Bare("serde".to_string());
        index
            .native
            .insert((Ecosystem::Cargo, "serde".to_string()), cargo_serde);
        let mut diagnostics = Vec::new();
        let result = index.resolve_native_with_fallback(Ecosystem::Npm, "serde", &mut diagnostics);
        assert!(
            result.is_none(),
            "a same-ecosystem miss must never silently fall back to a cross-ecosystem match"
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, callisto_model::DiagnosticCode::UnknownPackage);
        assert_eq!(diagnostics[0].severity, callisto_model::DiagnosticSeverity::Warning);
    }

    #[test]
    fn resolve_native_with_fallback_returns_none_and_one_diagnostic_for_two_candidates() {
        let mut index = IdentityIndex::default();
        index.native.insert(
            (Ecosystem::Cargo, "ambiguous-lib".to_string()),
            PackageId::Bare("ambiguous-lib".to_string()),
        );
        index.native.insert(
            (Ecosystem::Pypi, "ambiguous-lib".to_string()),
            PackageId::Prefixed {
                ecosystem: Ecosystem::Pypi,
                name: "ambiguous-lib".to_string(),
            },
        );
        let mut diagnostics = Vec::new();
        let result = index.resolve_native_with_fallback(Ecosystem::Npm, "ambiguous-lib", &mut diagnostics);
        assert!(result.is_none());
        assert_eq!(diagnostics.len(), 1, "exactly one diagnostic, not zero and not two");
        assert!(
            diagnostics[0].message.contains("cargo:ambiguous-lib")
                && diagnostics[0].message.contains("pypi:ambiguous-lib")
        );
    }

    #[test]
    fn resolve_qualified_selector_is_exact_and_misses_report_e102() {
        let mut index = IdentityIndex::default();
        let npm_only = PackageId::Bare("foo".to_string());
        index
            .native
            .insert((Ecosystem::Npm, "foo".to_string()), npm_only.clone());

        // repro/ws-qual: `cargo/foo` must not select the npm-only `foo`.
        let err = index.resolve("cargo/foo").unwrap_err();
        assert!(
            matches!(err, GraphError::UnknownPackage { .. }),
            "expected E102, got {err:?}"
        );

        assert_eq!(index.resolve("npm/foo").unwrap(), npm_only);
        assert_eq!(
            index.resolve("npm:foo").unwrap(),
            npm_only,
            "eco:name must be accepted as an alias"
        );
    }

    #[test]
    fn resolve_bare_selector_finds_a_dual_manifest_packages_non_primary_name() {
        // repro/ws-native: a Cargo+npm package keyed by its Cargo name must
        // still resolve by its npm manifest's own (different) name.
        let mut index = IdentityIndex::default();
        let owner = PackageId::Bare("x-core".to_string());
        index
            .native
            .insert((Ecosystem::Cargo, "x-core".to_string()), owner.clone());
        index
            .native
            .insert((Ecosystem::Npm, "@x/core".to_string()), owner.clone());

        assert_eq!(index.resolve("@x/core").unwrap(), owner);
        assert_eq!(index.resolve("x-core").unwrap(), owner);
    }

    #[test]
    fn resolve_bare_selector_ambiguous_across_ecosystems_reports_e103() {
        let mut index = IdentityIndex::default();
        let cargo_foo = PackageId::Bare("foo".to_string());
        let npm_foo = PackageId::Prefixed {
            ecosystem: Ecosystem::Npm,
            name: "foo".to_string(),
        };
        index
            .native
            .insert((Ecosystem::Cargo, "foo".to_string()), cargo_foo.clone());
        index
            .native
            .insert((Ecosystem::Npm, "foo".to_string()), npm_foo.clone());

        let err = index.resolve("foo").unwrap_err();
        match err {
            GraphError::AmbiguousName { name, candidates } => {
                assert_eq!(name, "foo");
                assert_eq!(candidates.len(), 2);
                assert!(candidates.contains(&cargo_foo));
                assert!(candidates.contains(&npm_foo));
            }
            other => panic!("expected AmbiguousName (E103), got {other:?}"),
        }
    }

    #[test]
    fn identifies_rejects_a_same_named_package_in_a_different_ecosystem() {
        let mut index = IdentityIndex::default();
        let cargo_foo = PackageId::Bare("foo".to_string());
        let npm_foo = PackageId::Prefixed {
            ecosystem: Ecosystem::Npm,
            name: "foo".to_string(),
        };
        index
            .native
            .insert((Ecosystem::Cargo, "foo".to_string()), cargo_foo.clone());
        index
            .native
            .insert((Ecosystem::Npm, "foo".to_string()), npm_foo.clone());

        let selector = PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "foo".to_string(),
        };
        assert!(index.identifies(&selector, &cargo_foo));
        assert!(!index.identifies(&selector, &npm_foo));
    }

    #[test]
    fn resolve_native_with_fallback_lists_the_candidates_own_native_name_not_the_primary_name() {
        // The Pypi candidate's stored value is a Cargo-primary package whose
        // pyproject.toml declares a *different* name ("foo-py") -- the
        // diagnostic must say "pypi:foo-py" (what Pypi actually calls it),
        // not "pypi:foo" (this package's unrelated Cargo/primary name).
        let mut index = IdentityIndex::default();
        let owner = PackageId::Bare("foo".to_string());
        index
            .native
            .insert((Ecosystem::Cargo, "foo".to_string()), owner.clone());
        index.native.insert((Ecosystem::Pypi, "foo-py".to_string()), owner);
        let mut diagnostics = Vec::new();
        index.resolve_native_with_fallback(Ecosystem::Npm, "foo-py", &mut diagnostics);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0].message.contains("pypi:foo-py"),
            "expected the candidate's own native name in the message, got: {}",
            diagnostics[0].message
        );
        assert!(
            !diagnostics[0].message.contains("pypi:foo)") && !diagnostics[0].message.contains("pypi:foo,"),
            "must not list the owner's unrelated primary name, got: {}",
            diagnostics[0].message
        );
    }

    #[test]
    fn resolve_native_unchanged_still_returns_cross_ecosystem_match() {
        let mut index = IdentityIndex::default();
        let cargo_serde = PackageId::Bare("serde".to_string());
        index
            .native
            .insert((Ecosystem::Cargo, "serde".to_string()), cargo_serde.clone());
        let result = index.resolve_native(Ecosystem::Npm, "serde");
        assert_eq!(
            result,
            Some(&cargo_serde),
            "resolve_native's own permissive contract is unchanged by this spec"
        );
    }
}

#[derive(Clone, Debug, Default)]
pub struct IdentityIndex {
    pub bare: BTreeMap<String, PackageId>,
    pub prefixed: BTreeMap<(Ecosystem, String), PackageId>,
    pub native: BTreeMap<(Ecosystem, String), PackageId>,
    /// Keyed by a platform npm manifest's own package name (e.g.
    /// `"@myorg/my-crate-linux-x64-gnu"`) -- distinct from `owner`'s name
    /// when the owning package's primary identity comes from a
    /// higher-priority ecosystem sharing the same directory (a Cargo+npm
    /// dual project: platform manifests belong to the owning
    /// `Package`, they are never independently-registered `Package`s of
    /// their own). The `ManifestRole::Platform` is carried alongside so
    /// `GroupTable::resolve` doesn't need a second disk read to recover it.
    pub platform: BTreeMap<String, (PackageId, PathBuf, ManifestRole)>,
}

impl IdentityIndex {
    /// The one selector resolver: resolves a user- or config-typed package
    /// selector (`eco:name`, `eco/name`, or a bare name) against registered
    /// native manifest names.
    ///
    /// A qualified selector is an exact `(ecosystem, native name)` lookup: no
    /// match is `E102` ([`GraphError::UnknownPackage`]), regardless of what a
    /// different ecosystem calls itself. A bare selector searches every
    /// ecosystem's native names: no match is `E102`; more than one distinct
    /// package answering to that name in different ecosystems is `E103`
    /// ([`GraphError::AmbiguousName`]). Resolving by any registered native
    /// name -- not just a package's primary/display name -- is what lets a
    /// dual-manifest package be named by either of its manifests' names.
    pub fn resolve(&self, selector: &str) -> Result<PackageId, GraphError> {
        let id = PackageId::parse(selector).unwrap_or_else(|_| PackageId::Bare(selector.to_string()));
        self.resolve_id(&id)
    }

    /// [`Self::resolve`] for an already-parsed selector.
    pub fn resolve_id(&self, id: &PackageId) -> Result<PackageId, GraphError> {
        match id {
            PackageId::Prefixed { ecosystem, name } => self
                .native
                .get(&(*ecosystem, name.clone()))
                .cloned()
                .ok_or_else(|| GraphError::UnknownPackage { id: id.clone() }),
            PackageId::Bare(name) => {
                let candidates: BTreeSet<PackageId> = self
                    .native
                    .iter()
                    .filter(|((_, n), _)| n == name)
                    .map(|(_, registered)| registered.clone())
                    .collect();
                match candidates.len() {
                    0 => Err(GraphError::UnknownPackage { id: id.clone() }),
                    1 => Ok(candidates.into_iter().next().expect("len checked above")),
                    _ => Err(GraphError::AmbiguousName {
                        name: name.clone(),
                        candidates: candidates.into_iter().collect(),
                    }),
                }
            }
        }
    }

    /// True when `selector` (an ecosystem-qualified or bare configured
    /// package identity, e.g. from `[[release.artifact]]` or
    /// `product-package`) resolves to `candidate`.
    ///
    /// The single replacement for the `selector.matches(&candidate) &&
    /// selector.ecosystem() == Some(candidate_ecosystem)` guard that was
    /// hand-rolled at every config-selector call site: `PackageId::matches`
    /// alone treats a bare id as an ecosystem wildcard, so a qualified
    /// selector for one ecosystem's package would otherwise match a
    /// same-named package in another.
    pub fn identifies(&self, selector: &PackageId, candidate: &PackageId) -> bool {
        self.resolve_id(selector).is_ok_and(|resolved| &resolved == candidate)
    }

    pub fn resolve_human(&self, name: &str, siblings: &[PackageId]) -> Result<PackageId, GraphError> {
        if let Ok(PackageId::Prefixed { ecosystem, name: n }) = PackageId::parse(name) {
            if let Some(id) = self.prefixed.get(&(ecosystem, n)) {
                return Ok(id.clone());
            }
        }

        if let Some(id) = self.bare.get(name) {
            return Ok(id.clone());
        }

        let mut candidates = Vec::new();
        for ((_eco, n), id) in &self.prefixed {
            if n == name {
                candidates.push(id.clone());
            }
        }

        if candidates.len() == 1 {
            return Ok(candidates[0].clone());
        }

        if candidates.len() > 1 && !siblings.is_empty() {
            for sib in siblings {
                for cand in &candidates {
                    if cand.ecosystem() == sib.ecosystem() {
                        return Ok(cand.clone());
                    }
                }
            }
        }

        if candidates.is_empty() {
            Err(GraphError::UnknownPackage {
                id: PackageId::parse(name).unwrap_or_else(|_| PackageId::Bare(name.to_string())),
            })
        } else {
            Err(GraphError::AmbiguousName {
                name: name.to_string(),
                candidates,
            })
        }
    }

    /// Look up a package by its native (manifest-declared) name.
    ///
    /// First tries the exact `(eco, name)` key. If that misses -- a
    /// package in one ecosystem depending on a different ecosystem's
    /// package by bare name (e.g. an npm package listing a cargo crate)
    /// -- falls back to scanning all ecosystems for `name`.
    ///
    /// Fallback finds exactly one match: returns it. Finds more than one
    /// (two ecosystems both have `name`): returns `None` to prevent
    /// silent misresolution -- callers holding a `&mut Vec<Diagnostic>`
    /// should use [`Self::resolve_native_with_fallback`] instead, to get
    /// a diagnostic.
    pub fn resolve_native(&self, eco: Ecosystem, name: &str) -> Option<&PackageId> {
        // Fast path: exact ecosystem match.
        if let Some(id) = self.native.get(&(eco, name.to_string())) {
            return Some(id);
        }

        // Cross-ecosystem fallback.
        let mut candidates: Vec<&PackageId> = self
            .native
            .iter()
            .filter(|((e, n), _)| *e != eco && n == name)
            .map(|(_, id)| id)
            .collect();

        // Deduplicate by pointer identity (multiple entries for the same ID
        // across different ecosystems, e.g. a package that is both cargo and
        // npm, should not be treated as ambiguous).
        candidates.dedup_by(|a, b| a == b);

        match candidates.len() {
            1 => Some(candidates[0]),
            // 0 = not found; >1 = true ambiguity → caller should diagnose.
            _ => None,
        }
    }

    /// Like [`Self::resolve_native`] but pushes a [`callisto_model::Diagnostic`] when
    /// cross-ecosystem ambiguity is detected (two packages with the same bare
    /// name in different ecosystems).
    pub fn resolve_native_with_fallback<'a>(
        &'a self,
        eco: Ecosystem,
        name: &str,
        diagnostics: &mut Vec<callisto_model::Diagnostic>,
    ) -> Option<&'a PackageId> {
        // Fast path: exact ecosystem match.
        if let Some(id) = self.native.get(&(eco, name.to_string())) {
            return Some(id);
        }

        // Cross-ecosystem fallback: collect unique IDs from other ecosystems,
        // keeping each candidate's own native name (`n`, the map key) rather
        // than its owning package's primary name (`id.name()`) -- a
        // dual-manifest package's manifests can declare different names, and
        // the diagnostic must name what the *other* ecosystem actually calls
        // it, not this package's unrelated primary/display name.
        let mut candidates: Vec<(Ecosystem, &str, &PackageId)> = self
            .native
            .iter()
            .filter(|((e, n), _)| *e != eco && n == name)
            .map(|((e, n), id)| (*e, n.as_str(), id))
            .collect();
        candidates.dedup_by(|a, b| a.2 == b.2);

        match candidates.len() {
            0 => None,
            _ => {
                let candidate_names: Vec<String> = candidates
                    .iter()
                    .map(|(e, n, _)| format!("{}:{n}", e.prefix()))
                    .collect();
                diagnostics.push(callisto_model::Diagnostic {
                    code: callisto_model::DiagnosticCode::UnknownPackage,
                    severity: callisto_model::DiagnosticSeverity::Warning,
                    // No ecosystem-prefix advice here: this name comes from a
                    // manifest's own dependency field, which cannot carry an
                    // `eco:name` prefix -- the ambiguity can only be resolved
                    // by renaming one of the candidates.
                    message: format!(
                        "dependency name `{name}` is ambiguous across ecosystems: {}",
                        candidate_names.join(", "),
                    ),
                    package: None,
                    path: None,
                    escalated_by: None,
                    governed_by: None,
                });
                None
            }
        }
    }

    pub fn native_name(&self, id: &PackageId, eco: Ecosystem) -> Option<&str> {
        for ((e, name), registered_id) in &self.native {
            if e == &eco && registered_id == id {
                return Some(name.as_str());
            }
        }
        None
    }

    pub fn native_names(&self, id: &PackageId) -> impl Iterator<Item = (Ecosystem, &str)> {
        let mut results = Vec::new();
        for ((e, name), registered_id) in &self.native {
            if registered_id == id {
                results.push((*e, name.as_str()));
            }
        }
        results.into_iter()
    }

    pub fn display_form(&self, id: &PackageId) -> String {
        id.display_name()
    }

    /// The platform manifests attached to `owner` from their own
    /// directories, as (npm name, manifest path). Excludes a co-located platform
    /// manifest that is also one of `owner`'s canonical manifests.
    pub fn attached_platforms<'a>(
        &'a self,
        owner: &'a callisto_model::Package,
    ) -> impl Iterator<Item = (&'a str, &'a Path)> + 'a {
        self.platforms_of(&owner.id)
            .filter(|(_, path)| !owner.canonical_manifests().any(|m| m.path == *path))
    }

    pub fn platforms_of(&self, owner: &PackageId) -> impl Iterator<Item = (&str, &Path)> {
        let mut results = Vec::new();
        for (name, (plat_owner, path, _role)) in &self.platform {
            if plat_owner == owner {
                results.push((name.as_str(), path.as_path()));
            }
        }
        results.into_iter()
    }
}
