//! Dev-only byte-compat corpus and in-memory test doubles for callisto's crates.

pub mod corpus;

#[cfg(feature = "graph")]
pub mod graph_builder;
#[cfg(feature = "graph")]
pub mod runner;
#[cfg(feature = "graph")]
pub mod scenario;

#[cfg(feature = "graph")]
pub use graph_builder::{GraphBuilder, InMemoryGraph, PackageBuilder};
#[cfg(feature = "graph")]
pub use runner::ReplayCommandRunner;
#[cfg(feature = "graph")]
pub use scenario::Scenario;

#[cfg(test)]
mod tests {
    use super::corpus::*;

    #[test]
    fn test_corpus_samples_are_non_empty() {
        assert!(valid_changeset_sample().contains("my-pkg"));
        assert!(valid_pre_json_sample().contains("beta"));
        assert!(cargo_workspace_toml_sample().contains("[workspace]"));
        assert!(npm_package_json_sample().contains("@scoped/web-app"));
        assert!(pyproject_toml_sample().contains("py-service"));
        assert!(go_mod_sample().contains("github.com/myorg/goservice"));
    }
}
