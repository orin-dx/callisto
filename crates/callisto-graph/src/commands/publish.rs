use callisto_model::{
    CommandRunner, CratePublish, NpmMainPublish, PublishPlan, ReleaseEntry, SCHEMA_VERSION,
};

use crate::error::GraphError;
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct PublishOptions {}

pub fn plan_publish<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    _opts: &PublishOptions,
) -> Result<PublishPlan, GraphError> {
    let mut rust_crates = Vec::new();
    let mut npm_main_packages = Vec::new();
    let mut releases = Vec::new();

    let base_versions = ws.base_versions()?;

    for pkg in ws.graph.packages() {
        let is_release = match ws.tags.last_tag(&pkg.id) {
            Some(t) => {
                if let Some(cur) = base_versions.get(&pkg.id) {
                    &t.version != cur
                } else {
                    false
                }
            }
            None => true,
        };

        if is_release {
            let ver = base_versions
                .get(&pkg.id)
                .cloned()
                .unwrap_or_else(|| callisto_model::Version::semver(1, 0, 0));

            let publishes_cargo = pkg
                .publish_to
                .iter()
                .any(|t| matches!(t, callisto_model::PublishTarget::CratesIo));
            let publishes_npm = pkg
                .publish_to
                .iter()
                .any(|t| matches!(t, callisto_model::PublishTarget::Npm { .. }));

            if publishes_cargo {
                rust_crates.push(CratePublish {
                    name: pkg.id.name().to_string(),
                    version: ver.clone(),
                    publish_to: callisto_model::RegistryKey(
                        callisto_model::RegistryKey::CRATES_IO.to_string(),
                    ),
                    registry: None,
                });
            }

            if publishes_npm {
                npm_main_packages.push(NpmMainPublish {
                    name: pkg.id.name().to_string(),
                    version: ver.clone(),
                    publish_to: callisto_model::RegistryKey(
                        callisto_model::RegistryKey::NPM.to_string(),
                    ),
                    registry: None,
                    depends_on_platforms: Vec::new(),
                });
            }

            releases.push(ReleaseEntry {
                tag_name: ws.tags.template(&pkg.id).render(&ver),
                sha: callisto_model::CommitSha::parse("0000000000000000000000000000000000000000")
                    .unwrap(),
                changelog_section: None,
            });
        }
    }

    Ok(PublishPlan {
        schema_version: SCHEMA_VERSION,
        rust_crates,
        npm_main_packages,
        npm_platform_packages: Vec::new(),
        releases,
        diagnostics: Vec::new(),
    })
}
