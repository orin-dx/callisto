use callisto_model::{CommandRunner, Diagnostic, DiagnosticCode, DiagnosticSeverity, MatrixReport, PackageId};

use crate::error::GraphError;
use crate::matrix::{
    add_release_artifact_groups, build_matrix_report, MatrixPackageInput, NapiCrate, ReleaseArtifactInput,
};
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct MatrixOptions {
    /// When Some, restrict the report to exactly this one registered
    /// package's PackageId::name() string. Err(GraphError::UnknownPackage)
    /// when no registered package matches.
    pub package: Option<String>,
    /// Resolve each napi package's addon crate into `manifestPath` (E204 when not exactly one).
    pub napi_crates: bool,
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

    let napi_crates = opts
        .napi_crates
        .then(|| workspace_napi_crates(ws, &all_packages))
        .transpose()?;
    let mut report = build_matrix_report(&inputs, napi_crates.as_deref())?;
    let mut artifacts = Vec::new();
    for artifact in ws
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
    {
        let resolved = all_packages.iter().find_map(|package| {
            let manifest = package
                .canonical_manifests()
                .find(|manifest| manifest.ecosystem() == callisto_model::Ecosystem::Cargo)?;
            (package.id.name() == artifact.package.name()).then_some((package, manifest))
        });
        let Some((package, manifest)) = resolved else {
            report.diagnostics.push(Diagnostic {
                code: DiagnosticCode::UnknownPackage,
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "`[[release.artifact]]` package `{}` (asset `{}`) is not a cargo package in this workspace; it has no matrix entry",
                    artifact.package, artifact.asset_name
                ),
                package: Some(artifact.package.clone()),
                path: None,
                escalated_by: None,
                governed_by: None,
            });
            continue;
        };
        artifacts.push(ReleaseArtifactInput {
            id: package.id.clone(),
            name: package.id.name().to_string(),
            dir_rel: manifest
                .path
                .parent()
                .map(|dir| dir.to_string_lossy().to_string())
                .unwrap_or_default(),
            target: artifact.target.clone(),
            asset_name: artifact.asset_name.clone(),
        });
    }
    add_release_artifact_groups(&mut report, &artifacts)?;
    Ok(report)
}

/// Every workspace cargo package that builds a napi-rs addon.
fn workspace_napi_crates<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    packages: &[&callisto_model::Package],
) -> Result<Vec<NapiCrate>, GraphError> {
    let ctx = callisto_manifests::OpenContext::for_workspace_root(&ws.root);
    let mut crates = Vec::new();
    for decl in packages
        .iter()
        .flat_map(|package| package.canonical_manifests())
        .filter(|decl| decl.format == callisto_model::ManifestFormat::CargoToml)
    {
        if let Some(lib_name) = crate::manifest_cache::open_cached(&ws.manifest_cache, decl, &ctx)?.napi_lib_name() {
            crates.push(NapiCrate {
                lib_name,
                manifest_path: decl.path.to_string_lossy().into_owned(),
            });
        }
    }
    Ok(crates)
}

fn package_dir_rel(package: &callisto_model::Package) -> String {
    package
        .manifests
        .first()
        .and_then(|manifest| manifest.path.parent())
        .map(|dir| dir.to_string_lossy().to_string())
        .unwrap_or_default()
}
