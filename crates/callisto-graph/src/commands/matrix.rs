use callisto_model::{CommandRunner, MatrixReport, PackageId};

use crate::error::GraphError;
use crate::matrix::{build_matrix_report, MatrixPackageInput};
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
        .filter(|p| {
            opts.package
                .as_deref()
                .map(|n| p.id.name() == n)
                .unwrap_or(true)
        })
        .map(|p| {
            let dir_rel = p
                .manifests
                .first()
                .and_then(|m| m.path.parent())
                .map(|d| d.to_path_buf())
                .unwrap_or_default();
            MatrixPackageInput {
                id: p.id.clone(),
                dir_abs: ws.root.join(&dir_rel),
                dir_rel: dir_rel.to_string_lossy().to_string(),
                name: p.id.name().to_string(),
            }
        })
        .collect();

    build_matrix_report(&inputs)
}
