use std::collections::BTreeMap;

use callisto_model::{PackageId, Version};

/// Targets the solver has stopped moving; the only source of a version for a range rewrite or E319.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConvergedTargets(BTreeMap<PackageId, Version>);

impl ConvergedTargets {
    pub(super) fn settle(targets: BTreeMap<PackageId, Version>) -> Self {
        Self(targets)
    }

    pub fn get(&self, id: &PackageId) -> Option<ConvergedTarget<'_>> {
        self.0.get(id).map(ConvergedTarget)
    }

    pub fn contains_key(&self, id: &PackageId) -> bool {
        self.0.contains_key(id)
    }

    pub fn keys(&self) -> impl Iterator<Item = &PackageId> {
        self.0.keys()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PackageId, ConvergedTarget<'_>)> {
        self.0.iter().map(|(id, version)| (id, ConvergedTarget(version)))
    }
}

/// One package's settled target, borrowed from [`ConvergedTargets`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConvergedTarget<'a>(&'a Version);

impl<'a> ConvergedTarget<'a> {
    pub fn version(self) -> &'a Version {
        self.0
    }
}
