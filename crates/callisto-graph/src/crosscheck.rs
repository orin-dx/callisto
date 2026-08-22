use std::collections::{BTreeMap, BTreeSet};

use callisto_model::{
    DeclaredEdge, DeclaredEdgeKind, DepEdge, Diagnostic, DiagnosticCode, DiagnosticSeverity, Package, PackageId,
    StrictFlag,
};

pub fn crosscheck_declared_edges(
    packages: &BTreeMap<PackageId, Package>,
    edges: &[DepEdge],
    declared: &[DeclaredEdge],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    let mut filtered_declared = BTreeSet::new();
    for edge in declared {
        if edge.kind == DeclaredEdgeKind::Root {
            continue;
        }
        if packages.contains_key(&edge.from) && packages.contains_key(&edge.to) {
            filtered_declared.insert((edge.from.clone(), edge.to.clone()));
        }
    }

    let mut graph_pairs = BTreeSet::new();
    for edge in edges {
        if edge.kind == callisto_model::DepKind::Dev {
            continue;
        }
        if packages.contains_key(&edge.from) && packages.contains_key(&edge.to) {
            graph_pairs.insert((edge.from.clone(), edge.to.clone()));
        }
    }

    for (from, to) in filtered_declared.difference(&graph_pairs) {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::GraphEdgeDisagreement,
            severity: DiagnosticSeverity::Warning,
            message: format!(
                "moon declares {} -> {} but no manifest declares it",
                from.display_name(),
                to.display_name()
            ),
            package: Some(from.clone()),
            path: None,
            governed_by: None,
            escalated_by: Some(StrictFlag::StrictGraph),
        });
    }

    for (from, to) in graph_pairs.difference(&filtered_declared) {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::GraphEdgeDisagreement,
            severity: DiagnosticSeverity::Warning,
            message: format!(
                "manifest declares {} -> {} but moon does not declare it",
                from.display_name(),
                to.display_name()
            ),
            package: Some(from.clone()),
            path: None,
            governed_by: None,
            escalated_by: Some(StrictFlag::StrictGraph),
        });
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use callisto_model::{DepSpec, ManifestDecl, ManifestFormat, ManifestRole, PublishTarget, ReleaseTrigger};

    use super::*;

    fn pkg(id: &str) -> Package {
        Package {
            id: PackageId::parse(id).unwrap(),
            manifests: vec![ManifestDecl::new(
                PathBuf::from("Cargo.toml"),
                ManifestRole::Canonical,
                ManifestFormat::CargoToml,
            )
            .unwrap()],
            changelog: None,
            release_trigger: ReleaseTrigger::Changeset,
            publish_to: vec![PublishTarget::None],
            tag_template: None,
        }
    }

    // Regression test for the previous behavior of `build()`, which
    // constructed a throwaway `ManifestWalkResolver` (deep-cloning every
    // Package/DepEdge) purely to satisfy this function's old
    // `&ManifestWalkResolver` parameter. `crosscheck_declared_edges` only
    // ever needed read-only access to the package map and edge slice, so
    // this test proves the crosscheck still produces correct diagnostics
    // when driven directly off borrowed `&BTreeMap`/`&[DepEdge]` inputs —
    // i.e. without any resolver (and therefore without any clone) in the
    // loop at all.
    #[test]
    fn crosscheck_uses_borrowed_inputs_without_a_resolver() {
        let a = PackageId::parse("a").unwrap();
        let b = PackageId::parse("b").unwrap();

        let mut packages = BTreeMap::new();
        packages.insert(a.clone(), pkg("a"));
        packages.insert(b.clone(), pkg("b"));

        // Manifest-derived edge: a -> b.
        let edges = vec![DepEdge {
            from: a.clone(),
            to: b.clone(),
            kind: callisto_model::DepKind::Runtime,
            spec: DepSpec::Opaque("1.0.0".to_string()),
            from_manifest: PathBuf::from("Cargo.toml"),
            inherited: false,
        }];

        // moon declares a different edge: b -> a.
        let declared = vec![DeclaredEdge {
            from: b.clone(),
            to: a.clone(),
            kind: DeclaredEdgeKind::Build,
            via: None,
        }];

        let diags = crosscheck_declared_edges(&packages, &edges, &declared);

        // Two disagreements expected: moon's b->a is undeclared by manifests,
        // and the manifest's a->b is undeclared by moon.
        assert_eq!(diags.len(), 2);
        assert!(diags.iter().all(|d| d.code == DiagnosticCode::GraphEdgeDisagreement));
    }
}
