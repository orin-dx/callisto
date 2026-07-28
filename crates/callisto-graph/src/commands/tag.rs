use callisto_model::{CommandRunner, CreatedTag, PublishPlan, TagReport, SCHEMA_VERSION};

use crate::error::GraphError;
use crate::resolver::DependencyResolver;
use crate::Workspace;

pub fn create_tags<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    plan: &PublishPlan,
) -> Result<TagReport, GraphError> {
    let mut tags = Vec::new();

    for release in &plan.releases {
        let tag_str = release.tag_name.as_str();
        let output = ws
            .runner
            .run("git", &["tag", "--list", tag_str], &ws.root)?;

        let already_existed = output.success() && !output.stdout_trimmed().is_empty();

        if !already_existed {
            let create_out = ws.runner.run(
                "git",
                &["tag", "-a", tag_str, "-m", &format!("Release {}", tag_str)],
                &ws.root,
            )?;
            if !create_out.success() {
                return Err(GraphError::Command(callisto_model::CommandError::Io {
                    program: "git".to_string(),
                    message: create_out.stderr,
                }));
            }
        }

        tags.push(CreatedTag {
            package: release.package.clone(),
            tag_name: release.tag_name.clone(),
            sha: release.sha.clone(),
        });
    }

    Ok(TagReport {
        schema_version: SCHEMA_VERSION,
        created_tags: tags,
        diagnostics: Vec::new(),
    })
}
