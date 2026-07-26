use callisto_model::{CommandRunner, ComposePrBodyReport, SCHEMA_VERSION};

use crate::commands::version::plan_version;
use crate::error::GraphError;
use crate::infer::SeverityInference;
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct PrBodyOptions {
    pub existing_body: Option<String>,
    pub labels: Vec<String>,
}

pub fn compose_pr_body<R: CommandRunner, D: DependencyResolver, I: SeverityInference>(
    ws: &Workspace<'_, R, D>,
    inference: &I,
    _opts: &PrBodyOptions,
) -> Result<ComposePrBodyReport, GraphError> {
    let plan = plan_version(
        ws,
        inference,
        &crate::commands::version::VersionOptions::default(),
    )?;

    let opts = _opts;
    let mut body = String::new();
    if let Some(ref existing) = opts.existing_body {
        if let Some((prefix, _)) = existing.split_once("## Release Preview") {
            if !prefix.trim().is_empty() {
                body.push_str(prefix);
                if !prefix.ends_with('\n') {
                    body.push('\n');
                }
            }
        }
    }
    body.push_str("## Release Preview\n\n");

    for bump in &plan.bumps {
        body.push_str(&format!(
            "<details><summary>{}@{} ({})</summary>\n\n",
            bump.package.display_name(),
            bump.to.render(),
            bump.severity
        ));
        body.push_str(&format!(
            "Bumped from `{}` to `{}`.\n\n",
            bump.from.render(),
            bump.to.render()
        ));
        if let Some(ref reason) = bump.reason {
            match reason {
                callisto_model::BumpReason::Changeset { changesets } => {
                    body.push_str(&format!("- Changesets: {}\n\n", changesets.join(", ")));
                }
                callisto_model::BumpReason::Cascade { via, .. } => {
                    body.push_str(&format!(
                        "- Dependency update cascade from `{}`\n\n",
                        via.display_name()
                    ));
                }
                _ => {}
            }
        }
        body.push_str("</details>\n\n");
    }

    Ok(ComposePrBodyReport {
        schema_version: SCHEMA_VERSION,
        pr_body: body,
        diagnostics: Vec::new(),
    })
}
