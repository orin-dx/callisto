use std::collections::BTreeSet;

use callisto_model::{
    DeclaredEdge, DeclaredEdgeKind, Diagnostic, DiagnosticCode, DiagnosticSeverity, StrictFlag,
};

use crate::resolver::ManifestWalkResolver;

pub fn crosscheck_declared_edges(
    graph: &ManifestWalkResolver,
    declared: &[DeclaredEdge],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    let mut filtered_declared = BTreeSet::new();
    for edge in declared {
        if edge.kind == DeclaredEdgeKind::Root {
            continue;
        }
        if graph.get(&edge.from).is_some() && graph.get(&edge.to).is_some() {
            filtered_declared.insert((edge.from.clone(), edge.to.clone()));
        }
    }

    let mut graph_pairs = BTreeSet::new();
    for edge in graph.edges() {
        if edge.kind == callisto_model::DepKind::Dev {
            continue;
        }
        if graph.get(&edge.from).is_some() && graph.get(&edge.to).is_some() {
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
