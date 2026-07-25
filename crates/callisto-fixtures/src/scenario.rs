use std::collections::BTreeMap;

use callisto_graph::cascade::{run_cascade, CascadeInput, SpecRewrite};
use callisto_graph::config::{CascadeConfig, GroupTable};
use callisto_model::{BumpReason, DiagnosticCode, PackageId, Severity, Version};

use crate::graph_builder::InMemoryGraph;

pub struct Scenario {
    pub name: &'static str,
    pub graph: InMemoryGraph,
    pub base: BTreeMap<PackageId, Version>,
    pub groups: GroupTable,
    pub cascade: CascadeConfig,
    pub seed: BTreeMap<PackageId, Severity>,
    pub reasons: BTreeMap<PackageId, BumpReason>,
    pub expected_severities: BTreeMap<PackageId, Severity>,
    pub expected_rewrites: Vec<SpecRewrite>,
    pub expected_diagnostics: Vec<DiagnosticCode>,
}

impl Scenario {
    pub fn assert(&self) {
        let input = CascadeInput {
            graph: &self.graph,
            groups: &self.groups,
            cfg: &self.cascade,
            seed: &self.seed,
            reasons: &self.reasons,
            named_by: &BTreeMap::new(),
            base: &self.base,
            pre: None,
        };

        let outcome = run_cascade(input).unwrap();
        assert_eq!(outcome.severities, self.expected_severities);
    }
}
