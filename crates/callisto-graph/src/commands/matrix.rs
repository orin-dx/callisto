use callisto_model::{CommandRunner, MatrixReport, PackageId};

use crate::error::GraphError;
use crate::matrix::{add_release_artifact_groups, build_matrix_report, MatrixPackageInput, ReleaseArtifactInput};
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct MatrixOptions {
    /// When Some, restrict the report to exactly this one registered
    /// package's PackageId::name() string. Err(GraphError::UnknownPackage)
    /// when no registered package matches.
    pub package: Option<String>,
}

pub fn matrix<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    opts: &MatrixOptions,
) -> Result<MatrixReport, GraphError> {
    let all_packages: Vec<&callisto_model::Package> = ws.graph.packages().collect();

    if let Some(ref name) = opts.package {
        if !all_packages.iter().any(|p| p.id.name() == name) {
            return Err(GraphError::UnknownPackage {
                id: PackageId::Bare(name.clone()),
            });
        }
    }

    let inputs: Vec<MatrixPackageInput> = all_packages
        .iter()
        .filter(|p| opts.package.as_deref().map(|n| p.id.name() == n).unwrap_or(true))
        .map(|p| {
            let dir_rel = package_dir_rel(p);
            MatrixPackageInput {
                id: p.id.clone(),
                dir_abs: ws.root.join(&dir_rel),
                dir_rel,
                name: p.id.name().to_string(),
            }
        })
        .collect();

    let mut report = build_matrix_report(&inputs)?;
    let artifacts: Vec<ReleaseArtifactInput> = ws
        .config
        .product_release
        .iter()
        .flat_map(|release| &release.artifacts)
        .filter(|artifact| artifact.package.ecosystem() == Some(callisto_model::Ecosystem::Cargo))
        .filter(|artifact| {
            opts.package
                .as_deref()
                .is_none_or(|name| artifact.package.name() == name)
        })
        .filter_map(|artifact| {
            let (package, manifest) = all_packages.iter().find_map(|package| {
                let manifest = package
                    .canonical_manifests()
                    .find(|manifest| manifest.ecosystem() == callisto_model::Ecosystem::Cargo)?;
                (package.id.name() == artifact.package.name()).then_some((package, manifest))
            })?;
            Some(ReleaseArtifactInput {
                id: package.id.clone(),
                name: package.id.name().to_string(),
                dir_rel: manifest
                    .path
                    .parent()
                    .map(|dir| dir.to_string_lossy().to_string())
                    .unwrap_or_default(),
                target: artifact.target.clone(),
                asset_name: artifact.asset_name.clone(),
            })
        })
        .collect();
    add_release_artifact_groups(&mut report, &artifacts)?;
    Ok(report)
}

fn package_dir_rel(package: &callisto_model::Package) -> String {
    package
        .manifests
        .first()
        .and_then(|manifest| manifest.path.parent())
        .map(|dir| dir.to_string_lossy().to_string())
        .unwrap_or_default()
}
